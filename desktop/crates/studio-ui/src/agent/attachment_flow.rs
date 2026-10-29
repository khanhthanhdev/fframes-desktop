//! Turning what the user attaches into chips the engine has accepted.
//!
//! The composer shows a chip only after the engine checked the file or image (deny list,
//! project boundary, size, decoding), and every check runs off the UI thread. A file inside
//! the project is checked in one step. A file outside it is staged first: the engine returns
//! the canonical path, the panel asks the user to confirm that exact path, and only then is
//! the one-shot token redeemed. A token that expired, was used or belongs to a dialog the
//! user left is never reused; the file is staged again.

use std::path::{Component, Path, PathBuf};
use std::time::{Duration, Instant};

use gpui::{Image, ImageFormat};
use studio_engine::agent::{
    AttachmentRequest, PreparedAttachment, StagedAttachment, WorkflowError,
};

use super::composer::{AttachRequest, AttachmentDraft};
use super::composer_host::Staged;
use super::*;

/// How long a staged confirmation is trusted here. The engine's own limit is five minutes;
/// asking again slightly earlier means a late click never meets an expired token.
const STAGE_TRUST: Duration = Duration::from_secs(4 * 60);

/// An image file larger than this is not even read; the engine refuses it too.
const IMAGE_READ_CAP: u64 = 16 * 1024 * 1024;

/// The components of `path` for a lexical comparison: no `.` or `..`, no verbatim prefix,
/// and case folded where the file system is case-insensitive.
fn lexical(path: &Path) -> Vec<String> {
    let fold = |text: String| {
        if cfg!(windows) {
            text.to_lowercase()
        } else {
            text
        }
    };
    let mut parts: Vec<String> = Vec::new();
    for component in path.components() {
        match component {
            Component::Prefix(prefix) => {
                let text = prefix.as_os_str().to_string_lossy().into_owned();
                parts.push(fold(text.trim_start_matches(r"\\?\").to_owned()));
            }
            Component::RootDir => parts.push("/".to_owned()),
            Component::CurDir => {}
            Component::ParentDir => {
                if parts.len() > 1 {
                    parts.pop();
                }
            }
            Component::Normal(name) => parts.push(fold(name.to_string_lossy().into_owned())),
        }
    }
    parts
}

/// Whether `path` names something below `root`. Only decides which check runs first; the
/// engine resolves links and decides for real.
pub(super) fn is_inside(root: &Path, path: &Path) -> bool {
    if !path.is_absolute() {
        return true;
    }
    let (root, path) = (lexical(root), lexical(path));
    path.len() > root.len() && path.starts_with(&root)
}

fn file_name(path: &Path) -> String {
    path.file_name().map_or_else(
        || path.display().to_string(),
        |n| n.to_string_lossy().into_owned(),
    )
}

fn image_format(path: &Path) -> Option<ImageFormat> {
    let extension = path.extension()?.to_str()?.to_ascii_lowercase();
    let mime = match extension.as_str() {
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "webp" => "image/webp",
        "gif" => "image/gif",
        "bmp" => "image/bmp",
        "tif" | "tiff" => "image/tiff",
        _ => return None,
    };
    ImageFormat::from_mime_type(mime)
}

/// Reads an image file, refusing one over the cap before reading it.
fn read_image(path: &Path) -> Result<Vec<u8>, String> {
    let name = file_name(path);
    let meta = std::fs::metadata(path).map_err(|e| format!("{name} could not be read: {e}"))?;
    if meta.len() > IMAGE_READ_CAP {
        return Err(format!("{name} is larger than 16 MiB."));
    }
    std::fs::read(path).map_err(|e| format!("{name} could not be read: {e}"))
}

impl ConversationPanel {
    pub(super) fn attach_requested(&mut self, request: AttachRequest, cx: &mut Context<Self>) {
        match request {
            AttachRequest::PickFiles => self.pick(false, cx),
            AttachRequest::PickImages => self.pick(true, cx),
            AttachRequest::Paths(paths) => {
                for path in paths {
                    self.attach_path(path, cx);
                }
            }
            AttachRequest::Images(images) => {
                for image in images {
                    self.attach_pasted_image(image, cx);
                }
            }
        }
    }

    fn pick(&mut self, images: bool, cx: &mut Context<Self>) {
        let picker = cx.prompt_for_paths(gpui::PathPromptOptions {
            files: true,
            directories: false,
            multiple: true,
            prompt: Some(
                if images {
                    "Attach image"
                } else {
                    "Attach file"
                }
                .into(),
            ),
        });
        let generation = self.host.generation;
        cx.spawn(async move |this, cx| {
            let chosen = picker.await;
            let _ = this.update(cx, |panel, cx| {
                if panel.host.generation != generation {
                    return;
                }
                match chosen {
                    Ok(Ok(Some(paths))) => {
                        for path in paths {
                            if images {
                                panel.attach_image_file(path, cx);
                            } else {
                                panel.attach_path(path, cx);
                            }
                        }
                    }
                    Ok(Ok(None)) => {}
                    other => panel.say(format!("The file picker failed: {other:?}"), cx),
                }
            });
        })
        .detach();
    }

    /// A file the user chose or pasted: inside the project it is checked at once, outside
    /// it is staged for confirmation.
    fn attach_path(&mut self, path: PathBuf, cx: &mut Context<Self>) {
        let inside = self
            .project_root
            .as_deref()
            .is_some_and(|root| is_inside(root, &path));
        if inside {
            let draft = AttachmentDraft::file(file_name(&path));
            self.prepare(AttachmentRequest::File { path }, draft, cx);
        } else {
            self.stage(path, cx);
        }
    }

    fn attach_image_file(&mut self, path: PathBuf, cx: &mut Context<Self>) {
        let generation = self.host.generation;
        let label = file_name(&path);
        let format = image_format(&path);
        let read = cx
            .background_executor()
            .spawn(async move { read_image(&path) });
        self.host.preparing += 1;
        cx.spawn(async move |this, cx| {
            let bytes = read.await;
            let _ = this.update(cx, |panel, cx| {
                if panel.host.generation != generation {
                    return;
                }
                panel.host.preparing = panel.host.preparing.saturating_sub(1);
                match bytes {
                    Ok(bytes) => {
                        let draft = match format {
                            Some(format) => AttachmentDraft::image(
                                label.clone(),
                                Arc::new(Image::from_bytes(format, bytes.clone())),
                            ),
                            None => AttachmentDraft {
                                image: None,
                                ..AttachmentDraft::file(label.clone())
                            },
                        };
                        let draft = AttachmentDraft {
                            kind: super::composer::AttachmentKind::Image,
                            ..draft
                        };
                        panel.prepare(AttachmentRequest::Image { label, bytes }, draft, cx);
                    }
                    Err(reason) => panel.attach_refused(reason, cx),
                }
            });
        })
        .detach();
    }

    fn attach_pasted_image(&mut self, image: Arc<Image>, cx: &mut Context<Self>) {
        let request = AttachmentRequest::Image {
            label: "Pasted image".into(),
            bytes: image.bytes().to_vec(),
        };
        self.prepare(request, AttachmentDraft::image("Pasted image", image), cx);
    }

    fn attach_refused(&mut self, reason: impl Into<String>, cx: &mut Context<Self>) {
        let reason = reason.into();
        self.host.last_refusal = Some(reason.clone());
        self.say(reason, cx);
    }

    /// Has the engine check `request` off the UI thread; an accepted one becomes a chip
    /// showing `draft`, a refused one shows the engine's reason.
    fn prepare(
        &mut self,
        request: AttachmentRequest,
        draft: AttachmentDraft,
        cx: &mut Context<Self>,
    ) {
        let Some(workflow) = self.workflow().cloned() else {
            return;
        };
        let generation = self.host.generation;
        self.host.preparing += 1;
        let (done, result) = async_channel::bounded(1);
        workflow.prepare_attachment_with(request, move |outcome| {
            let _ = done.try_send(outcome);
        });
        cx.spawn(async move |this, cx| {
            let outcome = result.recv().await;
            let _ = this.update(cx, |panel, cx| {
                if panel.host.generation != generation {
                    return;
                }
                panel.host.preparing = panel.host.preparing.saturating_sub(1);
                match outcome {
                    Ok(outcome) => panel.attachment_checked(draft, outcome, cx),
                    Err(_) => panel.attach_refused("The attachment check did not finish.", cx),
                }
            });
        })
        .detach();
    }

    fn attachment_checked(
        &mut self,
        draft: AttachmentDraft,
        outcome: Result<PreparedAttachment, WorkflowError>,
        cx: &mut Context<Self>,
    ) {
        match outcome {
            Ok(prepared) => {
                let id = self
                    .host
                    .composer
                    .update(cx, |composer, cx| composer.add_attachment(draft, cx));
                self.host.prepared.insert(id, prepared);
                self.host.last_refusal = None;
                cx.notify();
            }
            Err(error) => self.attach_refused(error.to_string(), cx),
        }
    }

    /// Asks the engine to check a file outside the project; the result waits for the
    /// user's confirmation.
    fn stage(&mut self, path: PathBuf, cx: &mut Context<Self>) {
        let Some(workflow) = self.workflow().cloned() else {
            return;
        };
        let generation = self.host.generation;
        self.host.preparing += 1;
        let (done, result) = async_channel::bounded(1);
        workflow.stage_attachment_with(path, move |outcome| {
            let _ = done.try_send(outcome);
        });
        cx.spawn(async move |this, cx| {
            let outcome = result.recv().await;
            let _ = this.update(cx, |panel, cx| {
                if panel.host.generation != generation {
                    return;
                }
                panel.host.preparing = panel.host.preparing.saturating_sub(1);
                match outcome {
                    Ok(Ok(staged)) => panel.host.stage.push_back(Staged {
                        staged,
                        at: Instant::now(),
                    }),
                    Ok(Err(error)) => panel.attach_refused(error.to_string(), cx),
                    Err(_) => panel.attach_refused("The attachment check did not finish.", cx),
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// The user confirmed the front staged file. A token past its trust time is not
    /// redeemed: the file is staged again and the user confirms the fresh result.
    fn confirm_stage(&mut self, cx: &mut Context<Self>) {
        let Some(Staged { staged, at }) = self.host.stage.pop_front() else {
            return;
        };
        if at.elapsed() > STAGE_TRUST {
            self.say(
                "That confirmation timed out; the file is being checked again.",
                cx,
            );
            self.stage(staged.path, cx);
            return;
        }
        let StagedAttachment { token, label, .. } = staged;
        self.prepare(
            AttachmentRequest::Staged { token },
            AttachmentDraft::file(label),
            cx,
        );
        cx.notify();
    }

    /// The user left the dialog: the token is dropped and expires in the engine.
    fn cancel_stage(&mut self, cx: &mut Context<Self>) {
        self.host.stage.pop_front();
        cx.notify();
    }

    /// The confirmation for the front staged file, naming the path the engine resolved.
    pub(super) fn stage_card(&mut self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let front = self.host.stage.front()?;
        let body = format!(
            "{}\n{} bytes. The agent receives a copy of this file's content; it is not part of the project.",
            studio_bootstrap::external_path(&front.staged.path).display(),
            front.staged.size
        );
        Some(
            Notice::new(
                "agent-stage",
                UiTone::Warning,
                "Attach a file from outside the project?",
            )
            .body(body)
            .action(self.action_button(
                "agent-stage-confirm",
                "Attach",
                true,
                Emphasis::Primary,
                cx,
                |this, _, cx| this.confirm_stage(cx),
            ))
            .action(self.action_button(
                "agent-stage-cancel",
                "Cancel",
                true,
                Emphasis::Neutral,
                cx,
                |this, _, cx| this.cancel_stage(cx),
            ))
            .into_any_element(),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_path_below_the_root_is_inside() {
        let root = Path::new(if cfg!(windows) {
            r"C:\work\project"
        } else {
            "/work/project"
        });
        let below = root.join("media").join("a.png");
        assert!(is_inside(root, &below));
        assert!(!is_inside(root, root));
        assert!(!is_inside(
            root,
            &root.parent().unwrap().join("other").join("a.png")
        ));
        // A sibling that only shares the name prefix is outside.
        let sibling = root.with_file_name("project-two").join("a.png");
        assert!(!is_inside(root, &sibling));
    }

    #[test]
    fn dot_dot_cannot_climb_out_of_the_root() {
        let root = Path::new(if cfg!(windows) {
            r"C:\work\project"
        } else {
            "/work/project"
        });
        let escaping = root.join("media").join("..").join("..").join("secret.txt");
        assert!(!is_inside(root, &escaping));
        let staying = root.join("media").join("..").join("notes.txt");
        assert!(is_inside(root, &staying));
    }

    #[cfg(windows)]
    #[test]
    fn the_verbatim_prefix_and_case_do_not_matter_on_windows() {
        let root = Path::new(r"C:\Work\Project");
        assert!(is_inside(root, Path::new(r"\\?\c:\work\project\a.png")));
    }

    #[test]
    fn image_extensions_pick_a_thumbnail_format() {
        assert!(image_format(Path::new("a.JPG")).is_some());
        assert!(image_format(Path::new("a.png")).is_some());
        assert!(image_format(Path::new("a.txt")).is_none());
        assert!(image_format(Path::new("noext")).is_none());
    }
}
