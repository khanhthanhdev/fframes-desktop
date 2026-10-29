//! Arguments and capabilities of a project worker's one-shot export
//! (`studio_worker --export-v1 --output <file> [settings]`).
//!
//! Workers built before export settings existed accept only `--export-v1 --output` and
//! ignore every other argument, so a client never passes settings to a worker that did
//! not first answer [`EXPORT_CAPABILITIES_FLAG`]: such a worker exits with an error on
//! that flag instead of printing capabilities.
use serde::{Deserialize, Serialize};
use std::fmt;

pub const EXPORT_FLAG: &str = "--export-v1";
/// Prints [`ExportCapabilities`] as one JSON document on stdout and exits.
pub const EXPORT_CAPABILITIES_FLAG: &str = "--export-capabilities-v1";
pub const EXPORT_CAPABILITIES_VERSION: u32 = 1;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ExportContainer {
    Mp4,
    Webm,
}

impl ExportContainer {
    pub const ALL: [Self; 2] = [Self::Mp4, Self::Webm];

    pub fn extension(self) -> &'static str {
        match self {
            Self::Mp4 => "mp4",
            Self::Webm => "webm",
        }
    }

    /// The audio encoder this container is written with.
    pub fn audio_encoder(self) -> &'static str {
        match self {
            Self::Mp4 => "aac",
            Self::Webm => "libopus",
        }
    }

    /// Audio bitrate in bits per second.
    pub fn audio_bitrate(self) -> u64 {
        match self {
            Self::Mp4 => 192_000,
            Self::Webm => 128_000,
        }
    }

    pub fn holds(self, codec: ExportCodec) -> bool {
        match self {
            Self::Mp4 => matches!(
                codec,
                ExportCodec::Mpeg4 | ExportCodec::H264 | ExportCodec::Hevc
            ),
            Self::Webm => codec == ExportCodec::Vp9,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ExportCodec {
    Mpeg4,
    H264,
    Hevc,
    Vp9,
}

impl ExportCodec {
    pub const ALL: [Self; 4] = [Self::Mpeg4, Self::H264, Self::Hevc, Self::Vp9];

    /// FFmpeg encoders that produce this codec, in order of preference. Only software
    /// encoders: hardware ones can be listed by the build and still fail to open. H.264
    /// prefers libx264 (Windows); the macOS and Linux SDKs ship OpenH264 instead.
    pub fn encoders(self) -> &'static [&'static str] {
        match self {
            Self::Mpeg4 => &["mpeg4"],
            Self::H264 => &["libx264", "libopenh264"],
            Self::Hevc => &["libx265"],
            Self::Vp9 => &["libvpx-vp9"],
        }
    }

    /// The codec name FFmpeg reports for a stream of this codec.
    pub fn stream_codec_name(self) -> &'static str {
        match self {
            Self::Mpeg4 => "mpeg4",
            Self::H264 => "h264",
            Self::Hevc => "hevc",
            Self::Vp9 => "vp9",
        }
    }

    /// Video bitrate in bits per second for a frame of `width`×`height` at `fps`: the
    /// codec's 1080p30 rate scaled by pixels per second. Exports encode at this rate
    /// and size estimates use it, so both agree.
    pub fn video_bitrate(self, width: u32, height: u32, fps: u32) -> u64 {
        let at_1080p30: u64 = match self {
            Self::Mpeg4 => 8_000_000,
            Self::H264 => 6_000_000,
            Self::Hevc | Self::Vp9 => 4_000_000,
        };
        let pixels_per_second = u64::from(width) * u64::from(height) * u64::from(fps);
        (at_1080p30 * pixels_per_second / (1920 * 1080 * 30)).max(250_000)
    }
}

/// How to encode an export. The default is what every worker produced before settings
/// existed: MPEG-4 in MP4 at the video's size and frame rate.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct ExportSettings {
    pub container: ExportContainer,
    pub codec: ExportCodec,
    /// Output size relative to the video, in (0, 1].
    pub scale: f64,
    /// Output frame rate; it must divide the video's. `None` keeps the video's.
    pub fps: Option<u32>,
}

impl Default for ExportSettings {
    fn default() -> Self {
        Self {
            container: ExportContainer::Mp4,
            codec: ExportCodec::Mpeg4,
            scale: 1.,
            fps: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExportSettingsError(pub String);

impl fmt::Display for ExportSettingsError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for ExportSettingsError {}

impl ExportSettings {
    pub fn is_default(&self) -> bool {
        *self == Self::default()
    }

    /// Checks the settings against a video's frame rate.
    pub fn validate(&self, video_fps: u32) -> Result<(), ExportSettingsError> {
        let error = |message: String| Err(ExportSettingsError(message));
        if !self.container.holds(self.codec) {
            return error(format!(
                "{} cannot hold {} video",
                self.container.extension(),
                self.codec.stream_codec_name()
            ));
        }
        if !(self.scale.is_finite() && self.scale > 0. && self.scale <= 1.) {
            return error(format!("scale {} must be in (0, 1]", self.scale));
        }
        if let Some(fps) = self.fps
            && (fps == 0 || fps > video_fps || !video_fps.is_multiple_of(fps))
        {
            return error(format!(
                "frame rate {fps} must divide the video's {video_fps} fps"
            ));
        }
        Ok(())
    }

    /// The arguments that select these settings after `--export-v1`. Default settings
    /// add none, so a default export runs exactly as on workers without settings.
    pub fn to_args(&self) -> Vec<String> {
        if self.is_default() {
            return Vec::new();
        }
        let mut args = vec![
            "--container".to_owned(),
            self.container.extension().to_owned(),
            "--codec".to_owned(),
            self.codec.stream_codec_name().to_owned(),
            "--scale".to_owned(),
            self.scale.to_string(),
        ];
        if let Some(fps) = self.fps {
            args.extend(["--fps".to_owned(), fps.to_string()]);
        }
        args
    }

    /// Parses the arguments written by [`Self::to_args`]; missing ones keep the default.
    pub fn from_args(args: &[String]) -> Result<Self, ExportSettingsError> {
        let value = |flag: &str| -> Result<Option<&str>, ExportSettingsError> {
            match args.iter().position(|arg| arg == flag) {
                None => Ok(None),
                Some(index) => args
                    .get(index + 1)
                    .map(|value| Some(value.as_str()))
                    .ok_or_else(|| ExportSettingsError(format!("{flag} needs a value"))),
            }
        };
        let mut settings = Self::default();
        if let Some(container) = value("--container")? {
            settings.container = ExportContainer::ALL
                .into_iter()
                .find(|c| c.extension() == container)
                .ok_or_else(|| ExportSettingsError(format!("unknown container {container}")))?;
        }
        if let Some(codec) = value("--codec")? {
            settings.codec = ExportCodec::ALL
                .into_iter()
                .find(|c| c.stream_codec_name() == codec)
                .ok_or_else(|| ExportSettingsError(format!("unknown codec {codec}")))?;
        }
        if let Some(scale) = value("--scale")? {
            settings.scale = scale
                .parse()
                .map_err(|_| ExportSettingsError(format!("invalid scale {scale}")))?;
        }
        if let Some(fps) = value("--fps")? {
            settings.fps = Some(
                fps.parse()
                    .map_err(|_| ExportSettingsError(format!("invalid frame rate {fps}")))?,
            );
        }
        Ok(settings)
    }
}

/// A container and codec this worker can encode.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExportFormat {
    pub container: ExportContainer,
    pub codec: ExportCodec,
}

/// What [`EXPORT_CAPABILITIES_FLAG`] prints.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExportCapabilities {
    pub version: u32,
    pub width: u32,
    pub height: u32,
    pub fps: u32,
    /// The worker reads a timing document from `--timing-file`.
    pub timing: bool,
    /// The worker reads a props document from `--props-file`. Workers older than it say
    /// nothing, which reads as `false`.
    #[serde(default)]
    pub props: bool,
    /// Every container and codec whose encoders this worker's FFmpeg has.
    pub formats: Vec<ExportFormat>,
}

impl ExportCapabilities {
    /// Whether these settings can be exported with this worker; the reason otherwise.
    pub fn accepts(&self, settings: &ExportSettings) -> Result<(), ExportSettingsError> {
        settings.validate(self.fps)?;
        if !self.formats.contains(&ExportFormat {
            container: settings.container,
            codec: settings.codec,
        }) {
            return Err(ExportSettingsError(format!(
                "this SDK's FFmpeg has no {} encoder for {}",
                settings.codec.stream_codec_name(),
                settings.container.extension()
            )));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_settings_add_no_arguments_and_others_round_trip() {
        assert!(ExportSettings::default().to_args().is_empty());
        assert_eq!(
            ExportSettings::from_args(&[]).unwrap(),
            ExportSettings::default()
        );
        let settings = ExportSettings {
            container: ExportContainer::Webm,
            codec: ExportCodec::Vp9,
            scale: 0.5,
            fps: Some(30),
        };
        assert_eq!(
            ExportSettings::from_args(&settings.to_args()).unwrap(),
            settings
        );
    }

    #[test]
    fn invalid_settings_are_rejected_with_a_reason() {
        let settings = |container, codec, scale, fps| ExportSettings {
            container,
            codec,
            scale,
            fps,
        };
        use ExportCodec::*;
        use ExportContainer::*;
        assert!(settings(Webm, H264, 1., None).validate(60).is_err());
        assert!(settings(Mp4, Vp9, 1., None).validate(60).is_err());
        assert!(settings(Mp4, H264, 0., None).validate(60).is_err());
        assert!(settings(Mp4, H264, 1.5, None).validate(60).is_err());
        assert!(settings(Mp4, H264, f64::NAN, None).validate(60).is_err());
        assert!(settings(Mp4, H264, 1., Some(25)).validate(60).is_err());
        assert!(settings(Mp4, H264, 1., Some(0)).validate(60).is_err());
        assert!(settings(Mp4, H264, 1., Some(120)).validate(60).is_err());
        assert!(settings(Mp4, H264, 0.5, Some(30)).validate(60).is_ok());
        for args in [
            &["--codec", "av1"][..],
            &["--container", "mkv"],
            &["--scale", "half"],
            &["--fps"],
        ] {
            let args: Vec<String> = args.iter().map(|a| (*a).to_owned()).collect();
            assert!(ExportSettings::from_args(&args).is_err(), "{args:?}");
        }
    }

    #[test]
    fn capabilities_accept_only_listed_formats() {
        let capabilities = ExportCapabilities {
            version: EXPORT_CAPABILITIES_VERSION,
            width: 1920,
            height: 1080,
            fps: 30,
            timing: true,
            props: false,
            formats: vec![ExportFormat {
                container: ExportContainer::Mp4,
                codec: ExportCodec::Mpeg4,
            }],
        };
        assert!(capabilities.accepts(&ExportSettings::default()).is_ok());
        // Capabilities of a worker from before props read as "no props".
        let mut old = serde_json::to_value(&capabilities).unwrap();
        old.as_object_mut().unwrap().remove("props");
        let old: ExportCapabilities = serde_json::from_value(old).unwrap();
        assert!(!old.props);
        let h264 = ExportSettings {
            codec: ExportCodec::H264,
            ..Default::default()
        };
        assert!(capabilities.accepts(&h264).is_err());
    }

    #[test]
    fn h264_prefers_libx264_and_falls_back_to_openh264() {
        assert_eq!(ExportCodec::H264.encoders(), ["libx264", "libopenh264"]);
    }

    #[test]
    fn bitrate_scales_with_pixels_per_second() {
        assert_eq!(ExportCodec::H264.video_bitrate(1920, 1080, 30), 6_000_000);
        assert_eq!(ExportCodec::H264.video_bitrate(960, 540, 30), 1_500_000);
        assert_eq!(ExportCodec::H264.video_bitrate(1920, 1080, 60), 12_000_000);
        assert_eq!(ExportCodec::Vp9.video_bitrate(64, 36, 10), 250_000);
    }
}
