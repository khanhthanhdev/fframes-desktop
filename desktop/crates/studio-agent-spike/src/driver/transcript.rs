//! Resident transcript bounded in bytes, with paged history for the UI.

use super::events::{MAX_DELTA_EVENT_BYTES, MessageRole};
use serde::{Deserialize, Serialize};
use std::collections::VecDeque;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TranscriptEntry {
    /// Monotonic position; survives eviction of older entries.
    pub index: u64,
    pub role: MessageRole,
    pub text: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TranscriptPage {
    pub entries: Vec<TranscriptEntry>,
    /// Pass this as `from` for the following page; `None` at the end.
    pub next: Option<u64>,
    /// Entries older than this index were evicted to stay inside the byte bound.
    pub first_retained: u64,
}

/// Allocation cost charged per entry on top of its text, so many tiny entries cannot
/// retain millions of allocations inside a nominal byte budget.
pub const ENTRY_OVERHEAD_BYTES: usize = 96;

#[derive(Debug)]
pub struct Transcript {
    entries: VecDeque<TranscriptEntry>,
    bytes: usize,
    max_bytes: usize,
    next_index: u64,
}

impl Transcript {
    pub fn new(max_bytes: usize) -> Self {
        Self {
            entries: VecDeque::new(),
            bytes: 0,
            max_bytes: max_bytes.max(MAX_DELTA_EVENT_BYTES * 2),
            next_index: 0,
        }
    }

    /// Text plus per-entry overhead currently retained.
    pub fn resident_bytes(&self) -> usize {
        self.bytes
    }

    pub fn resident_entries(&self) -> usize {
        self.entries.len()
    }

    pub fn append(&mut self, role: MessageRole, text: &str) {
        if text.is_empty() {
            return;
        }
        let mut remaining = text;
        while !remaining.is_empty() {
            if let Some(last) = self.entries.back_mut()
                && last.role == role
                && last.text.len() < MAX_DELTA_EVENT_BYTES
            {
                let room = MAX_DELTA_EVENT_BYTES - last.text.len();
                let (head, tail) = split_at_boundary(remaining, room);
                if !head.is_empty() {
                    last.text.push_str(head);
                    self.bytes += head.len();
                    remaining = tail;
                    continue;
                }
            }
            let (head, tail) = split_at_boundary(remaining, MAX_DELTA_EVENT_BYTES);
            self.entries.push_back(TranscriptEntry {
                index: self.next_index,
                role,
                text: head.to_owned(),
            });
            self.next_index += 1;
            self.bytes += head.len() + ENTRY_OVERHEAD_BYTES;
            remaining = tail;
        }
        while self.bytes > self.max_bytes && self.entries.len() > 1 {
            if let Some(old) = self.entries.pop_front() {
                self.bytes -= old.text.len() + ENTRY_OVERHEAD_BYTES;
            }
        }
    }

    pub fn page(&self, from: u64, limit: usize) -> TranscriptPage {
        let first_retained = self.entries.front().map_or(self.next_index, |e| e.index);
        let start = from.max(first_retained);
        let entries: Vec<TranscriptEntry> = self
            .entries
            .iter()
            .filter(|e| e.index >= start)
            .take(limit.max(1))
            .cloned()
            .collect();
        let next = entries
            .last()
            .map(|e| e.index + 1)
            .filter(|next| *next < self.next_index);
        TranscriptPage {
            entries,
            next,
            first_retained,
        }
    }
}

fn split_at_boundary(text: &str, max: usize) -> (&str, &str) {
    if text.len() <= max {
        return (text, "");
    }
    let mut end = max;
    while end > 0 && !text.is_char_boundary(end) {
        end -= 1;
    }
    text.split_at(end)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resident_bytes_stay_bounded_and_pages_report_eviction() {
        let mut transcript = Transcript::new(4 * 1024 * 1024);
        let chunk = "x".repeat(10_000);
        for _ in 0..1000 {
            transcript.append(MessageRole::Agent, &chunk);
        }
        assert!(transcript.resident_bytes() <= 4 * 1024 * 1024 + MAX_DELTA_EVENT_BYTES);
        assert!(transcript.resident_entries() <= 4 * 1024 * 1024 / ENTRY_OVERHEAD_BYTES);
        let page = transcript.page(0, 10);
        assert!(page.first_retained > 0, "oldest history was evicted");
        assert_eq!(page.entries[0].index, page.first_retained);
        let mut seen = page.entries.len();
        let mut cursor = page.next;
        while let Some(from) = cursor {
            let page = transcript.page(from, 50);
            seen += page.entries.len();
            cursor = page.next;
        }
        assert_eq!(
            seen as u64,
            transcript.next_index - page.first_retained,
            "paging covers every retained entry exactly once"
        );
    }

    #[test]
    fn alternating_tiny_entries_are_bounded_by_allocation_cost_not_just_text() {
        let mut transcript = Transcript::new(4 * 1024 * 1024);
        for i in 0..2_000_000u32 {
            let role = if i % 2 == 0 {
                MessageRole::Agent
            } else {
                MessageRole::Thought
            };
            transcript.append(role, "x");
        }
        assert!(transcript.resident_bytes() <= 4 * 1024 * 1024 + MAX_DELTA_EVENT_BYTES);
        assert!(
            transcript.resident_entries() <= 4 * 1024 * 1024 / ENTRY_OVERHEAD_BYTES + 1,
            "{} entries retained",
            transcript.resident_entries()
        );
        // Newest history survives eviction.
        let page = transcript.page(0, 5);
        assert!(page.first_retained > 0);
    }

    #[test]
    fn roles_do_not_merge_and_multibyte_text_splits_on_boundaries() {
        let mut transcript = Transcript::new(4 * 1024 * 1024);
        transcript.append(MessageRole::Agent, "a");
        transcript.append(MessageRole::Thought, "b");
        transcript.append(MessageRole::Agent, &"é".repeat(MAX_DELTA_EVENT_BYTES));
        let page = transcript.page(0, 100);
        assert_eq!(page.entries[0].text, "a");
        assert_eq!(page.entries[1].role, MessageRole::Thought);
        let total: usize = page.entries.iter().skip(2).map(|e| e.text.len()).sum();
        assert_eq!(total, 2 * MAX_DELTA_EVENT_BYTES);
    }
}
