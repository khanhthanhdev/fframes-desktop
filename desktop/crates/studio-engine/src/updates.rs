//! Whether a newer fframes Studio has been released, and updating to it. Asks GitHub for
//! the latest release with the system's `curl` (shipped with Windows 10 and later, macOS
//! and most Linux distributions), so the app needs no HTTP client. A Windows install made
//! by Setup.exe can download the release's Setup.exe, check it against the release's
//! `SHA256SUMS.txt` (as `install.ps1` does) and run it silently once the app has quit.
use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
};

use sha2::{Digest, Sha256};

use serde::Deserialize;

/// The repository whose releases are the app's (the one `install.ps1` installs from).
pub const RELEASES_REPOSITORY: &str = "khanhthanhdev/fframes-desktop";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AvailableUpdate {
    /// The released version, without the tag's `v`.
    pub version: String,
    /// The release page, where the installer and notes are.
    pub url: String,
}

#[derive(Deserialize)]
struct LatestRelease {
    tag_name: String,
    html_url: String,
}

/// The latest release when it is newer than `current`. Blocks for up to 15 seconds; run it
/// off the UI thread.
pub fn check_for_update(current: &str) -> Result<Option<AvailableUpdate>, String> {
    let output = curl(&[
        "--max-time",
        "15",
        "--header",
        "Accept: application/vnd.github+json",
        &format!("https://api.github.com/repos/{RELEASES_REPOSITORY}/releases/latest"),
    ])
    .map_err(|e| format!("Cannot check for updates: {e}"))?;
    newer_release(current, &output)
}

/// Runs the system `curl` with `args` and returns what it wrote to standard output.
fn curl(args: &[&str]) -> Result<Vec<u8>, String> {
    let mut command = Command::new("curl");
    command.args(["--silent", "--show-error", "--fail", "--location"]);
    command.args(args);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        command.creation_flags(CREATE_NO_WINDOW);
    }
    let output = command
        .output()
        .map_err(|e| format!("curl is unavailable ({e})"))?;
    if !output.status.success() {
        return Err(String::from_utf8_lossy(&output.stderr).trim().to_owned());
    }
    Ok(output.stdout)
}

/// The per-user installer every release publishes (`install.ps1` runs the same file).
pub const SETUP_NAME: &str = "fframes-studio-x86_64-pc-windows-msvc-setup.exe";

/// The folder Setup.exe installed the running app into, when it did: Studio can then update
/// itself by running a newer Setup.exe. `None` for a portable ZIP, Scoop or a development
/// build, which update another way.
pub fn setup_install_dir() -> Option<PathBuf> {
    if !cfg!(windows) {
        return None;
    }
    let exe = std::env::current_exe().ok()?;
    // <app>\bin\fframes-studio.exe, with Inno Setup's uninstaller beside bin.
    let app = exe.parent()?.parent()?;
    app.join("unins000.exe").is_file().then(|| app.to_owned())
}

/// Downloads `update`'s Setup.exe into `dir` and checks it against the release's
/// `SHA256SUMS.txt`. A Setup.exe already there with the right checksum is reused, so an
/// update is downloaded once; older ones are removed. Blocks for the whole download (about
/// 300 MB); run it off the UI thread.
pub fn download_update(update: &AvailableUpdate, dir: &Path) -> Result<PathBuf, String> {
    let base = format!(
        "https://github.com/{RELEASES_REPOSITORY}/releases/download/v{}",
        update.version
    );
    let sums = curl(&["--max-time", "60", &format!("{base}/SHA256SUMS.txt")])
        .map_err(|e| format!("Cannot download the update's checksums: {e}"))?;
    let expected =
        expected_sha256(&String::from_utf8_lossy(&sums), SETUP_NAME).ok_or_else(|| {
            format!(
                "The {} release has no checksum for its Setup",
                update.version
            )
        })?;
    fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    let setup = dir.join(format!("fframes-studio-{}-setup.exe", update.version));
    if sha256_file(&setup).is_ok_and(|actual| actual == expected) {
        return Ok(setup);
    }
    let part = setup.with_extension("exe.part");
    curl(&[
        "--max-time",
        "3600",
        "--output",
        &part.to_string_lossy(),
        &format!("{base}/{SETUP_NAME}"),
    ])
    .map_err(|e| format!("Cannot download the update: {e}"))?;
    let actual = sha256_file(&part).map_err(|e| e.to_string())?;
    if actual != expected {
        let _ = fs::remove_file(&part);
        return Err(format!(
            "The downloaded update does not match its checksum (expected {expected}, got {actual}); nothing will be installed"
        ));
    }
    fs::rename(&part, &setup).map_err(|e| e.to_string())?;
    // Earlier updates' installers are no longer needed.
    for entry in fs::read_dir(dir).into_iter().flatten().flatten() {
        if entry.path() != setup {
            let _ = fs::remove_file(entry.path());
        }
    }
    Ok(setup)
}

/// Runs `setup` silently once this process has exited, then starts Studio again when
/// `relaunch`. Returns once the helper is started; the caller quits.
pub fn install_after_exit(setup: &Path, relaunch: bool) -> Result<(), String> {
    let quote = |path: &Path| format!("'{}'", path.to_string_lossy().replace('\'', "''"));
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    let mut script = format!(
        "Wait-Process -Id {} -ErrorAction SilentlyContinue; \
         $setup = Start-Process -FilePath {} -ArgumentList '/VERYSILENT','/SUPPRESSMSGBOXES','/NORESTART' -Wait -PassThru",
        std::process::id(),
        quote(setup),
    );
    if relaunch {
        script.push_str(&format!(
            "; if ($setup.ExitCode -eq 0) {{ Start-Process -FilePath {} }}",
            quote(&exe)
        ));
    }
    let spawn = |flags: u32| {
        let mut command = Command::new("powershell");
        command.args(["-NoProfile", "-NonInteractive", "-Command", &script]);
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            command.creation_flags(flags);
        }
        #[cfg(not(windows))]
        let _ = flags;
        command.spawn().map(drop)
    };
    // A hidden console, not none: what the helper starts may need one.
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    const CREATE_BREAKAWAY_FROM_JOB: u32 = 0x0100_0000;
    // The helper must outlive Studio: leave a job object that kills its processes when it
    // closes (one a launcher may have put Studio in). A job that forbids that refuses the
    // flag, and the helper starts inside it.
    spawn(CREATE_NO_WINDOW | CREATE_BREAKAWAY_FROM_JOB)
        .or_else(|_| spawn(CREATE_NO_WINDOW))
        .map_err(|e| format!("Cannot start the update: {e}"))
}

/// The lowercase SHA-256 `sums` (a `sha256sum` listing) gives for `name`.
fn expected_sha256(sums: &str, name: &str) -> Option<String> {
    sums.lines().find_map(|line| {
        let mut fields = line.split_whitespace();
        let (hash, file) = (fields.next()?, fields.next()?);
        (fields.next().is_none()
            && file.trim_start_matches('*') == name
            && hash.len() == 64
            && hash.chars().all(|c| c.is_ascii_hexdigit()))
        .then(|| hash.to_ascii_lowercase())
    })
}

fn sha256_file(path: &Path) -> std::io::Result<String> {
    let mut hasher = Sha256::new();
    std::io::copy(&mut fs::File::open(path)?, &mut hasher)?;
    Ok(format!("{:x}", hasher.finalize()))
}

/// The release described by GitHub's `releases/latest` answer `json`, when it is newer
/// than `current`.
fn newer_release(current: &str, json: &[u8]) -> Result<Option<AvailableUpdate>, String> {
    let release: LatestRelease = serde_json::from_slice(json)
        .map_err(|e| format!("Cannot check for updates: unexpected answer ({e})"))?;
    let version = release.tag_name.trim_start_matches('v');
    let parse = |v: &str| {
        semver::Version::parse(v).map_err(|e| format!("Cannot check for updates: version {v}: {e}"))
    };
    Ok(
        (parse(version)? > parse(current)?).then(|| AvailableUpdate {
            version: version.to_owned(),
            url: release.html_url,
        }),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn answer(tag: &str) -> Vec<u8> {
        format!(
            r#"{{"tag_name":"{tag}","html_url":"https://example.invalid/{tag}","draft":false}}"#
        )
        .into_bytes()
    }

    #[test]
    fn only_a_newer_release_is_an_update() {
        let update = newer_release("0.1.4", &answer("v0.1.10")).unwrap().unwrap();
        assert_eq!(
            update.version, "0.1.10",
            "compared as versions, not as text"
        );
        assert_eq!(update.url, "https://example.invalid/v0.1.10");
        assert_eq!(newer_release("0.1.4", &answer("v0.1.4")).unwrap(), None);
        assert_eq!(newer_release("0.2.0", &answer("v0.1.9")).unwrap(), None);
        assert_eq!(
            newer_release("0.2.0-rc.1", &answer("0.2.0"))
                .unwrap()
                .unwrap()
                .version,
            "0.2.0"
        );
        assert!(newer_release("0.1.4", br#"{"message":"Not Found"}"#).is_err());
        assert!(newer_release("0.1.4", &answer("nightly")).is_err());
    }

    #[test]
    fn the_setup_checksum_is_the_line_naming_exactly_the_setup() {
        let setup = "A".repeat(64);
        let zip = "b".repeat(64);
        let sums = format!(
            "{zip}  fframes-studio-x86_64-pc-windows-msvc.zip\n\
             {}  {SETUP_NAME}.sig\n\
             {setup} *{SETUP_NAME}\r\n",
            "c".repeat(64)
        );
        assert_eq!(expected_sha256(&sums, SETUP_NAME), Some("a".repeat(64)));
        assert_eq!(
            expected_sha256(&format!("{zip}  other.exe\n"), SETUP_NAME),
            None
        );
        assert_eq!(
            expected_sha256(&format!("abc  {SETUP_NAME}\n"), SETUP_NAME),
            None
        );
    }
}
