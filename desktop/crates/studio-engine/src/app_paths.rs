use crate::EngineError;
use std::{
    fs,
    path::{Path, PathBuf},
};

#[derive(Debug, Clone)]
pub struct AppPaths {
    pub data: PathBuf,
}
impl AppPaths {
    pub fn system() -> Result<Self, EngineError> {
        #[cfg(target_os = "linux")]
        let base = std::env::var_os("XDG_DATA_HOME")
            .map(PathBuf::from)
            .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".local/share")));
        #[cfg(target_os = "macos")]
        let base =
            std::env::var_os("HOME").map(|h| PathBuf::from(h).join("Library/Application Support"));
        #[cfg(windows)]
        let base = std::env::var_os("LOCALAPPDATA").map(PathBuf::from);
        let base = base.filter(|p| p.is_absolute()).ok_or_else(|| EngineError::Diagnostic("Platform app-data directory unavailable; configure HOME/XDG_DATA_HOME or LOCALAPPDATA".into()))?;
        Self::new(base.join("fframes-studio"))
    }
    pub fn new(data: impl Into<PathBuf>) -> Result<Self, EngineError> {
        let data = data.into();
        fs::create_dir_all(&data)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&data, fs::Permissions::from_mode(0o700))?;
        }
        Ok(Self {
            data: fs::canonicalize(data)?,
        })
    }
    pub fn database(&self) -> PathBuf {
        self.data.join("studio.sqlite3")
    }
    pub fn project(&self, id: &studio_project::ProjectId) -> PathBuf {
        self.data.join("projects").join(String::from(id.clone()))
    }
    /// A small, local poster from the project's last successful preview build.
    pub fn project_preview(&self, id: &studio_project::ProjectId) -> PathBuf {
        self.project(id).join("preview.png")
    }
    pub(crate) fn save_project_preview(
        &self,
        id: &studio_project::ProjectId,
        frame: &crate::PreviewFrame,
    ) -> Result<(), String> {
        frame.validate(&frame.response.envelope.identity)?;
        let header = &frame.response.header;
        // Protocol rows may contain padding; image buffers must be tightly packed.
        let pixels = frame
            .pixels
            .chunks(header.stride_bytes as usize)
            .take(header.height as usize)
            .flat_map(|row| row[..header.width as usize * 4].iter().copied())
            .collect();
        let image = image::RgbaImage::from_raw(header.width, header.height, pixels)
            .ok_or("Invalid preview pixels")?;
        let thumbnail = image::DynamicImage::ImageRgba8(image)
            .thumbnail(header.width.min(480), header.height.min(270));
        let dir = self.project(id);
        fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
        let mut file = tempfile::NamedTempFile::new_in(dir).map_err(|e| e.to_string())?;
        thumbnail
            .write_to(file.as_file_mut(), image::ImageFormat::Png)
            .map_err(|e| e.to_string())?;
        file.persist(self.project_preview(id))
            .map_err(|e| e.to_string())?;
        Ok(())
    }
    /// Imported (verified) preset packages, one directory per package.
    pub fn presets(&self) -> PathBuf {
        self.data.join("presets")
    }
    pub fn builds(&self) -> PathBuf {
        self.data.join("builds")
    }
    /// Per-project agent workspace root (outside portable source and Git).
    pub fn agent(&self, id: &studio_project::ProjectId) -> PathBuf {
        self.project(id).join("agent")
    }
    /// Stable agent working directory: the same path for every task of the project.
    pub fn agent_draft(&self, id: &studio_project::ProjectId) -> PathBuf {
        self.agent(id).join("draft")
    }
    /// Durable draft ownership marker, kept beside (never inside) the draft.
    pub fn agent_draft_state(&self, id: &studio_project::ProjectId) -> PathBuf {
        self.agent(id).join("draft-state.json")
    }
    /// Directory holding one subdirectory per stored agent session.
    pub fn agent_sessions(&self, id: &studio_project::ProjectId) -> PathBuf {
        self.agent(id).join("sessions")
    }
    /// One stored session: `manifest.json`, `meta.json` and `conversation.jsonl`.
    pub fn agent_session_dir(&self, id: &studio_project::ProjectId, session: &str) -> PathBuf {
        let dir = if is_valid_session_id(session) {
            session
        } else {
            ".invalid"
        };
        self.agent_sessions(id).join(dir)
    }
    /// Names the session that the single-session API serves; absent until a session exists.
    pub fn agent_session_current(&self, id: &studio_project::ProjectId) -> PathBuf {
        self.agent_sessions(id).join("current")
    }
    /// Retained failed drafts are moved here before the stable draft is refreshed.
    pub fn agent_archive(&self, id: &studio_project::ProjectId) -> PathBuf {
        self.agent(id).join("archive")
    }
    pub fn contains_source(&self, source: &Path) -> bool {
        self.data.starts_with(source) || source.starts_with(&self.data)
    }
}

/// Session ids come from the UI and from directory names: letters, digits and `-` only, so an
/// id can never leave the sessions directory.
pub fn is_valid_session_id(id: &str) -> bool {
    !id.is_empty() && id.len() <= 64 && id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
}

#[cfg(test)]
mod tests {
    use super::*;
    use fframes_studio_protocol::*;

    #[test]
    fn project_posters_survive_restart_and_replace_without_row_padding_or_channel_swaps() {
        let temp = tempfile::tempdir().unwrap();
        let paths = AppPaths::new(temp.path()).unwrap();
        let id = studio_project::ProjectId::try_from("poster-project".to_owned()).unwrap();
        let identity = PreviewIdentity {
            project_id: String::from(id.clone()),
            open_session: "poster-session".into(),
            source_revision: "a".repeat(64),
            worker_generation: 1,
        };
        let mut header = FrameHeader::new_straight_rgba("a".repeat(64), 1, 4, 0, 3, 2).unwrap();
        header.stride_bytes = 16;
        header.payload_len = 32;
        let mut frame = crate::PreviewFrame {
            response: ScaledFrameResponse {
                envelope: PreviewEnvelope {
                    contract_version: PREVIEW_CONTRACT_VERSION,
                    identity: identity.clone(),
                    request_id: 4,
                },
                frame_index: 0,
                seek_serial: 0,
                scale: 1.,
                header,
                render_duration_micros: 10,
                record: BinaryRecordHeader {
                    kind: BinaryRecordKind::FrameRgba8,
                    identity,
                    request_id: 4,
                    offset: 0,
                    payload_len: 32,
                },
                editor_metadata: None,
                backend_change: None,
            },
            pixels: vec![
                211, 32, 7, 255, 211, 32, 7, 255, 211, 32, 7, 255, 0, 0, 0, 0, 11, 73, 199, 128,
                11, 73, 199, 128, 11, 73, 199, 128, 0, 0, 0, 0,
            ],
        };
        paths.save_project_preview(&id, &frame).unwrap();
        let reopened = AppPaths::new(temp.path()).unwrap();
        let poster = image::open(reopened.project_preview(&id))
            .unwrap()
            .into_rgba8();
        assert_eq!(poster.dimensions(), (3, 2), "do not upscale small previews");
        assert_eq!(poster.get_pixel(2, 0).0, [211, 32, 7, 255]);
        assert_eq!(poster.get_pixel(0, 1).0, [11, 73, 199, 128]);

        frame.pixels[..4].copy_from_slice(&[19, 181, 43, 255]);
        reopened.save_project_preview(&id, &frame).unwrap();
        let updated = fs::read(reopened.project_preview(&id)).unwrap();
        assert_eq!(
            image::load_from_memory(&updated)
                .unwrap()
                .into_rgba8()
                .get_pixel(0, 0)
                .0,
            [19, 181, 43, 255]
        );
        frame.pixels.pop();
        assert!(reopened.save_project_preview(&id, &frame).is_err());
        assert_eq!(fs::read(reopened.project_preview(&id)).unwrap(), updated);
    }
}
