//! `captions`: the caption tracks of an immutable revision, read with the same parser the
//! video's media loader uses, so a track that loads here loads in the video.
use super::*;
use fframes_media::{CaptionCue, CaptionTrack};
use studio_project::revision::SourceFile;

/// Caption files a summary reads; later ones are reported as skipped.
const MAX_CAPTION_TRACKS: usize = 64;
/// One caption file; larger files are reported, not read.
const MAX_CAPTION_FILE_BYTES: u64 = 8 * 1024 * 1024;
/// Speakers listed per track summary.
const MAX_SPEAKERS: usize = 16;

fn format_of(path: &ProjectPath) -> Option<&'static str> {
    let (_, extension) = path.as_str().rsplit_once('.')?;
    if extension.eq_ignore_ascii_case("vtt") {
        Some("vtt")
    } else if extension.eq_ignore_ascii_case("json") {
        Some("json")
    } else {
        None
    }
}

/// `Ok(None)` for JSON that is not a caption track (a Lottie file, data, ...).
fn load(root: &Path, file: &SourceFile, format: &str) -> Result<Option<CaptionTrack>, String> {
    if file.size > MAX_CAPTION_FILE_BYTES {
        return Err(format!(
            "file is {} bytes; captions are read up to {MAX_CAPTION_FILE_BYTES}",
            file.size
        ));
    }
    let mut bytes = Vec::with_capacity(file.size as usize);
    file.path
        .open_file(root)
        .map_err(|error| error.to_string())?
        .take(MAX_CAPTION_FILE_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|error| error.to_string())?;
    let text = String::from_utf8(bytes).map_err(|_| "file is not UTF-8".to_owned())?;
    let track = if format == "vtt" {
        CaptionTrack::parse(&text).map(Some)
    } else {
        CaptionTrack::parse_json_document(&text)
    };
    track.map_err(|error| error.to_string())
}

/// The name `Captions::from_media` and `ctx.get_subtitles` take: the path inside media/.
fn media_name(path: &ProjectPath) -> &str {
    path.as_str()
        .strip_prefix("media/")
        .unwrap_or(path.as_str())
}

fn summary(file: &SourceFile, format: &str, track: &Result<CaptionTrack, String>) -> Value {
    let mut entry = json!({
        "path": file.path.as_str(),
        "media_name": media_name(&file.path),
        "format": format,
    });
    match track {
        Err(error) => {
            entry["status"] = json!("invalid");
            entry["error"] = json!(error);
        }
        Ok(track) => {
            let cues = track.cues();
            let mut speakers: Vec<&str> = Vec::new();
            for speaker in cues.iter().filter_map(|cue| cue.speaker.as_deref()) {
                if !speakers.contains(&speaker) && speakers.len() < MAX_SPEAKERS {
                    speakers.push(speaker);
                }
            }
            entry["status"] = json!("ok");
            entry["cues"] = json!(cues.len());
            entry["word_timed_cues"] = json!(cues.iter().filter(|c| !c.words.is_empty()).count());
            entry["words"] = json!(cues.iter().map(|c| c.words.len()).sum::<usize>());
            entry["start"] = json!(cues.iter().map(|c| c.start).reduce(f64::min));
            entry["end"] = json!(cues.iter().map(|c| c.end).reduce(f64::max));
            entry["speakers"] = json!(speakers);
            entry["has_metadata"] = json!(cues.iter().any(|cue| {
                !cue.metadata.is_empty() || cue.words.iter().any(|w| !w.metadata.is_empty())
            }));
        }
    }
    entry
}

fn cue_json(index: usize, cue: &CaptionCue) -> Value {
    let mut entry = json!({
        "index": index,
        "start": cue.start,
        "end": cue.end,
        "text": cue.display_text(),
    });
    if let Some(id) = &cue.id {
        entry["id"] = json!(id);
    }
    if let Some(speaker) = &cue.speaker {
        entry["speaker"] = json!(speaker);
    }
    if !cue.metadata.is_empty() {
        entry["metadata"] = Value::Object((*cue.metadata).clone());
    }
    if !cue.words.is_empty() {
        let words: Vec<Value> = cue
            .words
            .iter()
            .map(|word| {
                let mut entry = json!({"text": word.text, "start": word.start, "end": word.end});
                if let Some(id) = &word.id {
                    entry["id"] = json!(id);
                }
                if !word.space_before {
                    entry["space_before"] = json!(false);
                }
                if !word.metadata.is_empty() {
                    entry["metadata"] = Value::Object((*word.metadata).clone());
                }
                entry
            })
            .collect();
        entry["words"] = json!(words);
    }
    entry
}

/// Without `path`, a summary of every caption track in `media/`; with it, that track's
/// cues from `offset`, at most `limit` of them.
pub(super) fn captions(
    root: &Path,
    files: &[SourceFile],
    path: Option<&ProjectPath>,
    offset: usize,
    limit: usize,
    cancelled: &(dyn Fn() -> bool + Sync),
) -> Result<Value, ToolError> {
    let candidates = files
        .iter()
        .filter(|file| file.kind == FileKind::Media)
        .filter_map(|file| Some((file, format_of(&file.path)?)));
    let Some(path) = path else {
        let mut tracks = Vec::new();
        let mut skipped = Vec::new();
        for (file, format) in candidates {
            if cancelled() {
                return Err(tool_error(ToolErrorCode::Cancelled, "call cancelled"));
            }
            if tracks.len() == MAX_CAPTION_TRACKS {
                skipped.push(file.path.as_str());
                continue;
            }
            match load(root, file, format) {
                Ok(None) => {}
                Ok(Some(track)) => tracks.push(summary(file, format, &Ok(track))),
                Err(error) => tracks.push(summary(file, format, &Err(error))),
            }
        }
        return Ok(json!({"tracks": tracks, "not_read": skipped}));
    };
    let Some((file, format)) = candidates.into_iter().find(|(file, _)| file.path == *path) else {
        return Err(tool_error(
            ToolErrorCode::NotFound,
            format!(
                "`{}` is not a .vtt or .json file under media/ in this revision",
                path.as_str()
            ),
        ));
    };
    let track = match load(root, file, format) {
        Ok(Some(track)) => Ok(track),
        Ok(None) => {
            return Err(tool_error(
                ToolErrorCode::NotFound,
                format!(
                    "`{}` is JSON without a top-level `cues` array, not a caption track",
                    path.as_str()
                ),
            ));
        }
        Err(error) => Err(error),
    };
    let mut reply = json!({"track": summary(file, format, &track)});
    if let Ok(track) = &track {
        let cues = track.cues();
        let end = offset.saturating_add(limit).min(cues.len());
        let listed: Vec<Value> = (offset.min(end)..end)
            .map(|index| cue_json(index, &cues[index]))
            .collect();
        reply["cues"] = json!(listed);
        reply["next_offset"] = json!((end < cues.len()).then_some(end));
    }
    Ok(reply)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn project(files: &[(&str, &str)]) -> (tempfile::TempDir, Vec<SourceFile>) {
        let temp = tempfile::tempdir().unwrap();
        let listed = files
            .iter()
            .map(|(path, content)| {
                let full = temp.path().join(path);
                std::fs::create_dir_all(full.parent().unwrap()).unwrap();
                std::fs::write(&full, content).unwrap();
                SourceFile {
                    path: ProjectPath::try_from((*path).to_owned()).unwrap(),
                    kind: if path.starts_with("media/") {
                        FileKind::Media
                    } else {
                        FileKind::Other
                    },
                    size: content.len() as u64,
                    sha256: String::new(),
                    executable: false,
                }
            })
            .collect();
        (temp, listed)
    }

    const SPEECH: &str = r#"{"cues":[
        {"start":0.2,"end":2.6,"speaker":"A","words":[
            {"text":"Hello","start":0.2,"end":0.7,"metadata":{"effect":"underline"}},
            {"text":"!","start":0.7,"end":0.8,"space_before":false}]},
        {"start":3,"end":5,"text":"Plain phrase.","speaker":"B"},
        {"start":5,"end":6,"text":"Last."}]}"#;

    #[test]
    fn summary_lists_caption_tracks_with_field_errors_and_skips_other_json() {
        let (temp, files) = project(&[
            ("media/speech.json", SPEECH),
            (
                "media/intro.vtt",
                "WEBVTT\n\n00:00.000 --> 00:01.500\nHi there\n",
            ),
            // Word ends before it starts: the parser names the field.
            (
                "media/broken.json",
                r#"{"cues":[{"start":0,"end":2,"words":[{"text":"x","start":1,"end":0.5}]}]}"#,
            ),
            ("media/anim.json", r#"{"v":"5.7","layers":[]}"#),
            ("notes/other.vtt", "WEBVTT\n"),
        ]);
        let reply = captions(temp.path(), &files, None, 0, 50, &|| false).unwrap();
        let tracks = reply["tracks"].as_array().unwrap();
        let by_path = |path: &str| tracks.iter().find(|t| t["path"] == path).cloned();
        assert_eq!(tracks.len(), 3, "{reply}");
        assert!(by_path("media/anim.json").is_none());
        let speech = by_path("media/speech.json").unwrap();
        assert_eq!(speech["status"], "ok");
        assert_eq!(speech["media_name"], "speech.json");
        assert_eq!(
            (
                &speech["cues"],
                &speech["word_timed_cues"],
                &speech["words"]
            ),
            (&json!(3), &json!(1), &json!(2))
        );
        assert_eq!(
            (&speech["start"], &speech["end"]),
            (&json!(0.2), &json!(6.0))
        );
        assert_eq!(speech["speakers"], json!(["A", "B"]));
        assert_eq!(speech["has_metadata"], true);
        assert_eq!(by_path("media/intro.vtt").unwrap()["cues"], 1);
        let broken = by_path("media/broken.json").unwrap();
        assert_eq!(broken["status"], "invalid");
        assert!(
            broken["error"]
                .as_str()
                .unwrap()
                .contains("cues[0].words[0]"),
            "{broken}"
        );
    }

    #[test]
    fn a_track_pages_its_cues_with_words_and_metadata() {
        let (temp, files) = project(&[("media/speech.json", SPEECH)]);
        let path = ProjectPath::try_from("media/speech.json".to_owned()).unwrap();
        let first = captions(temp.path(), &files, Some(&path), 0, 2, &|| false).unwrap();
        let cues = first["cues"].as_array().unwrap();
        assert_eq!(cues.len(), 2);
        assert_eq!(cues[0]["text"], "Hello!");
        assert_eq!(
            cues[0]["words"][0]["metadata"],
            json!({"effect": "underline"})
        );
        assert_eq!(cues[0]["words"][1]["space_before"], false);
        assert!(cues[0]["words"][0].get("space_before").is_none());
        assert_eq!(cues[1]["text"], "Plain phrase.");
        assert!(cues[1].get("words").is_none());
        assert_eq!(first["next_offset"], 2);
        let rest = captions(temp.path(), &files, Some(&path), 2, 2, &|| false).unwrap();
        assert_eq!(rest["cues"][0]["index"], 2);
        assert_eq!(rest["cues"].as_array().unwrap().len(), 1);
        assert_eq!(rest["next_offset"], Value::Null);
    }

    #[test]
    fn a_path_that_is_not_a_caption_track_is_not_found() {
        let (temp, files) = project(&[("media/anim.json", r#"{"layers":[]}"#)]);
        for path in ["media/anim.json", "media/missing.vtt"] {
            let path = ProjectPath::try_from(path.to_owned()).unwrap();
            let error = captions(temp.path(), &files, Some(&path), 0, 50, &|| false).unwrap_err();
            assert_eq!(error.code, ToolErrorCode::NotFound, "{error:?}");
        }
    }
}
