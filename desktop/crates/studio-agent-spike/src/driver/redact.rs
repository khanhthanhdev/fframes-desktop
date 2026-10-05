//! Secret redaction applied before any text reaches events, transcripts or diagnostics.

use super::redact_sensitive_string;
use std::sync::Arc;

pub const REDACTION_MARK: &str = "[REDACTED]";
/// Longest un-whitespaced run held back while waiting for a token to complete.
const MAX_HELD_WORD_BYTES: usize = 4096;
/// Prefixes after which the next word is an opaque credential value.
const TRIGGERS: [&str; 5] = ["Bearer ", "token=", "api_key=", "sk-", "ghp_"];

fn is_value_end(c: char) -> bool {
    c.is_whitespace() || matches!(c, '"' | '\'' | ';' | ',')
}

/// True when `raw` ends inside (or right before) a credential value, so the first word
/// of whatever follows must be treated as that value.
fn value_pending(raw: &str) -> bool {
    TRIGGERS.iter().any(|trigger| {
        raw.rfind(trigger)
            .is_some_and(|pos| !raw[pos + trigger.len()..].contains(is_value_end))
    })
}

/// Exact configured secret values plus the generic token-pattern redaction.
#[derive(Clone, Debug, Default)]
pub struct Redactor {
    secrets: Vec<String>,
}

impl Redactor {
    pub fn new(secrets: impl IntoIterator<Item = String>) -> Self {
        let mut secrets: Vec<String> = secrets
            .into_iter()
            .filter(|s| !s.is_empty() && !REDACTION_MARK.contains(s.as_str()))
            .collect();
        // Longest first so a secret containing another secret is replaced whole.
        secrets.sort_by_key(|s| std::cmp::Reverse(s.len()));
        secrets.dedup();
        Self { secrets }
    }

    /// Replaces complete secrets and generic token patterns.
    pub fn redact(&self, text: &str) -> String {
        redact_sensitive_string(&self.replace_secrets(text))
    }

    fn replace_secrets(&self, text: &str) -> String {
        let mut out = text.to_owned();
        for secret in &self.secrets {
            if out.contains(secret.as_str()) {
                out = out.replace(secret.as_str(), REDACTION_MARK);
            }
        }
        out
    }

    fn longest(&self) -> usize {
        self.secrets.first().map_or(0, String::len)
    }

    /// Start of the *longest* proper secret prefix that `text` ends with, if any
    /// (smallest start over all secrets), so no prefix of any secret is ever released.
    fn trailing_secret_prefix(&self, text: &str) -> Option<usize> {
        let mut start: Option<usize> = None;
        for secret in &self.secrets {
            for (end, _) in secret.char_indices().rev().filter(|(i, _)| *i > 0) {
                if text.ends_with(&secret[..end]) {
                    let candidate = text.len() - end;
                    start = Some(start.map_or(candidate, |s| s.min(candidate)));
                    break;
                }
            }
        }
        start
    }
}

/// Redacts a chunked stream. Text is released only up to the start of the last two
/// words (so `Bearer <token>` and `token=<value>` stay together) and never through a
/// possible secret prefix. A credential value that straddles a release boundary is
/// redacted from state carried between releases.
#[derive(Debug)]
pub struct StreamRedactor {
    redactor: Arc<Redactor>,
    pending: String,
    /// A forced cut happened inside a long word; drop the rest of that word.
    suppress_word: bool,
    /// The previous release ended at a credential trigger; redact the next word.
    value_pending: bool,
}

impl StreamRedactor {
    pub fn new(redactor: Arc<Redactor>) -> Self {
        Self {
            redactor,
            pending: String::new(),
            suppress_word: false,
            value_pending: false,
        }
    }

    /// Returns the portion that is safe to release now (possibly empty).
    pub fn push(&mut self, chunk: &str) -> String {
        self.pending.push_str(chunk);
        self.pending = self.redactor.replace_secrets(&self.pending);
        if self.suppress_word {
            match self.pending.find(char::is_whitespace) {
                Some(end) => {
                    self.pending.drain(..end);
                    self.suppress_word = false;
                }
                None => {
                    self.pending.clear();
                    return String::new();
                }
            }
        }
        let mut cut = self.word_boundary();
        if let Some(prefix) = self.redactor.trailing_secret_prefix(&self.pending) {
            cut = cut.min(prefix);
        }
        if self.pending.len() > MAX_HELD_WORD_BYTES && cut == 0 {
            let keep = self.redactor.longest().max(64);
            let mut forced = self.pending.len() - keep.min(self.pending.len());
            while !self.pending.is_char_boundary(forced) {
                forced += 1;
            }
            cut = forced;
            self.suppress_word = true;
        }
        self.release(cut)
    }

    /// Releases held text except a trailing possible secret prefix. Used at event and
    /// turn boundaries: waiting for a word boundary would reorder events, but a secret
    /// prefix is never released because the rest may arrive after the boundary.
    pub fn flush_at_boundary(&mut self) -> String {
        self.pending = self.redactor.replace_secrets(&self.pending);
        let cut = self
            .redactor
            .trailing_secret_prefix(&self.pending)
            .unwrap_or(self.pending.len());
        self.release(cut)
    }

    /// Releases everything that is still held; only for the true end of the stream.
    pub fn finish(&mut self) -> String {
        self.suppress_word = false;
        self.pending = self.redactor.replace_secrets(&self.pending);
        self.release(self.pending.len())
    }

    fn release(&mut self, cut: usize) -> String {
        let raw: String = self.pending.drain(..cut).collect();
        if raw.is_empty() {
            return raw;
        }
        let mut out = String::new();
        let mut rest: &str = &raw;
        if self.value_pending {
            match rest.find(is_value_end) {
                Some(end) => {
                    if end > 0 {
                        out.push_str(REDACTION_MARK);
                    }
                    rest = &rest[end..];
                    self.value_pending = false;
                }
                None => {
                    // The whole release is still the value; it stays pending.
                    return REDACTION_MARK.to_owned();
                }
            }
        }
        out.push_str(&redact_sensitive_string(rest));
        self.value_pending = value_pending(rest);
        out
    }

    fn word_boundary(&self) -> usize {
        let mut starts = Vec::new();
        let mut in_word = false;
        for (index, ch) in self.pending.char_indices() {
            if ch.is_whitespace() {
                in_word = false;
            } else if !in_word {
                in_word = true;
                starts.push(index);
            }
        }
        if starts.len() >= 2 {
            starts[starts.len() - 2]
        } else {
            0
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn stream(secret: &str) -> StreamRedactor {
        StreamRedactor::new(Arc::new(Redactor::new([secret.to_owned()])))
    }

    #[test]
    fn complete_secret_is_replaced() {
        let r = Redactor::new(["hunter2-value".to_owned()]);
        let out = r.redact("login hunter2-value ok");
        assert!(!out.contains("hunter2-value"));
        assert!(out.contains(REDACTION_MARK));
    }

    #[test]
    fn secret_split_across_chunks_is_never_released() {
        let mut s = stream("split-secret");
        let mut out = String::new();
        for chunk in ["Streaming ", "split-", "secret", " response"] {
            out.push_str(&s.push(chunk));
        }
        out.push_str(&s.finish());
        assert!(!out.contains("split-secret"), "{out}");
        assert!(!out.contains("split-"), "{out}");
        assert!(out.contains(REDACTION_MARK));
        assert!(out.contains("Streaming") && out.contains("response"));
    }

    #[test]
    fn longest_matching_prefix_is_retained_across_event_boundaries() {
        // "aaa" is a prefix; flushing at a boundary must hold all of it, not just the
        // shortest matching prefix, or the pieces "aa" + "a-secret" would reassemble.
        let mut s = stream("aaa-secret");
        let mut out = String::new();
        out.push_str(&s.push("aaa"));
        out.push_str(&s.flush_at_boundary());
        out.push_str(&s.push("-secret done"));
        out.push_str(&s.finish());
        assert!(!out.contains("aaa-secret"), "{out}");
        assert!(!out.contains("aa"), "no fragment released: {out}");
        assert!(out.contains(REDACTION_MARK) && out.contains("done"));

        // Repeated prefixes interleaved with other events.
        let mut s = stream("abab-secret-xyz");
        let mut out = String::new();
        for chunk in ["ab", "ab", "ab", "ab", "-secret-", "xyz!"] {
            out.push_str(&s.push(chunk));
            out.push_str(&s.flush_at_boundary());
        }
        out.push_str(&s.finish());
        assert!(!out.contains("abab-secret-xyz"), "{out}");
        assert!(!out.replace(REDACTION_MARK, "").contains("secret"), "{out}");
    }

    #[test]
    fn a_held_secret_prefix_survives_a_turn_flush() {
        let mut s = stream("aaa-secret");
        let mut out = s.push("end aaa");
        out.push_str(&s.flush_at_boundary());
        // Next turn completes the secret.
        out.push_str(&s.push("-secret next"));
        out.push_str(&s.finish());
        assert!(!out.contains("aaa-secret"), "{out}");
        assert!(!out.contains("-secret"), "{out}");
    }

    #[test]
    fn token_value_in_a_later_chunk_is_redacted() {
        let mut s = stream("unused-secret");
        let mut out = String::new();
        for chunk in ["header token=", "abc123xyz", " tail words here"] {
            out.push_str(&s.push(chunk));
        }
        out.push_str(&s.finish());
        assert!(!out.contains("abc123xyz"), "{out}");
    }

    #[test]
    fn generic_token_context_survives_event_boundaries() {
        for (head, value) in [
            ("Authorization: Bearer ", "opaque-value-123"),
            ("config token=", "opaque-value-123"),
            ("key sk-", "opaque-value-123"),
        ] {
            let mut s = stream("unused-secret");
            let mut out = s.push(head);
            // A tool event arrives between the trigger and the value.
            out.push_str(&s.flush_at_boundary());
            out.push_str(&s.push(&format!("{value} and more")));
            out.push_str(&s.flush_at_boundary());
            out.push_str(&s.push(" trailing words"));
            out.push_str(&s.finish());
            assert!(!out.contains(value), "{head}: {out}");
            assert!(out.contains("more") && out.contains("trailing"), "{out}");
        }
        // A value split over several boundary releases stays redacted throughout.
        let mut s = stream("unused-secret");
        let mut out = s.push("Bearer ");
        out.push_str(&s.flush_at_boundary());
        for part in ["abc", "def", "ghi"] {
            out.push_str(&s.push(part));
            out.push_str(&s.flush_at_boundary());
        }
        out.push_str(&s.push(" ok"));
        out.push_str(&s.finish());
        assert!(
            !out.contains("abc") && !out.contains("def") && !out.contains("ghi"),
            "{out}"
        );
    }

    #[test]
    fn overlong_word_is_force_cut_without_leaking_the_tail() {
        let mut s = stream("zz-secret");
        let mut out = s.push("token=");
        out.push_str(&s.push(&"a".repeat(MAX_HELD_WORD_BYTES + 100)));
        out.push_str(&s.push("bbbbbbbbbbbb tail"));
        out.push_str(&s.finish());
        assert!(!out.contains("aaaaaaaa"), "{}", &out[..out.len().min(200)]);
        assert!(!out.contains("bbbbbbbb"));
        assert!(out.contains("tail"));
    }

    #[test]
    fn multibyte_text_survives_holding() {
        let mut s = stream("secret");
        let mut out = String::new();
        for chunk in ["héllo wörld ", "日本語 テキスト ", "end"] {
            out.push_str(&s.push(chunk));
        }
        out.push_str(&s.finish());
        assert_eq!(out, "héllo wörld 日本語 テキスト end");
    }
}
