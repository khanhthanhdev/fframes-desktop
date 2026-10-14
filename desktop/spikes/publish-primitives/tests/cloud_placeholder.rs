//! A real Cloud Files API placeholder (what OneDrive Files On-Demand creates) is refused:
//! it is reported as `CloudPlaceholder`, never opened, moved over or moved.
#![cfg(windows)]

use std::{fs, io::ErrorKind, os::windows::ffi::OsStrExt, path::Path};

use publish_primitives::{Dir, EntryKind};
use windows_sys::{
    Win32::Storage::CloudFilters::{
        CF_CREATE_FLAG_NONE, CF_HARDLINK_POLICY_NONE, CF_HYDRATION_POLICY,
        CF_HYDRATION_POLICY_FULL, CF_HYDRATION_POLICY_MODIFIER_NONE, CF_INSYNC_POLICY_NONE,
        CF_PLACEHOLDER_CREATE_FLAG_MARK_IN_SYNC, CF_PLACEHOLDER_CREATE_INFO,
        CF_PLACEHOLDER_MANAGEMENT_POLICY_DEFAULT, CF_POPULATION_POLICY,
        CF_POPULATION_POLICY_ALWAYS_FULL, CF_POPULATION_POLICY_MODIFIER_NONE,
        CF_REGISTER_FLAG_NONE, CF_SYNC_POLICIES, CF_SYNC_REGISTRATION, CfCreatePlaceholders,
        CfRegisterSyncRoot, CfUnregisterSyncRoot,
    },
    core::GUID,
};

fn wide(path: &Path) -> Vec<u16> {
    path.as_os_str().encode_wide().chain([0]).collect()
}

struct SyncRoot(Vec<u16>);

impl Drop for SyncRoot {
    fn drop(&mut self) {
        // SAFETY: NUL-terminated path registered below.
        unsafe { CfUnregisterSyncRoot(self.0.as_ptr()) };
    }
}

fn register_sync_root(path: &Path) -> SyncRoot {
    let path = wide(path);
    let name: Vec<u16> = "fframes publish spike\0".encode_utf16().collect();
    let version: Vec<u16> = "1.0\0".encode_utf16().collect();
    let identity = b"fframes-spike";
    let registration = CF_SYNC_REGISTRATION {
        StructSize: size_of::<CF_SYNC_REGISTRATION>() as u32,
        ProviderName: name.as_ptr(),
        ProviderVersion: version.as_ptr(),
        SyncRootIdentity: identity.as_ptr().cast(),
        SyncRootIdentityLength: identity.len() as u32,
        FileIdentity: identity.as_ptr().cast(),
        FileIdentityLength: identity.len() as u32,
        ProviderId: GUID::from_u128(0x6f1d_2a3c_51b4_4c1e_9a57_0c2e_8d3f_a001),
    };
    let policies = CF_SYNC_POLICIES {
        StructSize: size_of::<CF_SYNC_POLICIES>() as u32,
        Hydration: CF_HYDRATION_POLICY {
            Primary: CF_HYDRATION_POLICY_FULL,
            Modifier: CF_HYDRATION_POLICY_MODIFIER_NONE,
        },
        Population: CF_POPULATION_POLICY {
            Primary: CF_POPULATION_POLICY_ALWAYS_FULL,
            Modifier: CF_POPULATION_POLICY_MODIFIER_NONE,
        },
        InSync: CF_INSYNC_POLICY_NONE,
        HardLink: CF_HARDLINK_POLICY_NONE,
        PlaceholderManagement: CF_PLACEHOLDER_MANAGEMENT_POLICY_DEFAULT,
    };
    // SAFETY: all pointers are valid for the call.
    let hr = unsafe {
        CfRegisterSyncRoot(
            path.as_ptr(),
            &registration,
            &policies,
            CF_REGISTER_FLAG_NONE,
        )
    };
    assert!(hr >= 0, "CfRegisterSyncRoot failed: {hr:#x}");
    SyncRoot(path)
}

fn create_placeholder(dir: &Path, name: &str, size: i64) {
    let base = wide(dir);
    let relative: Vec<u16> = name.encode_utf16().chain([0]).collect();
    let identity = name.as_bytes();
    let mut info = CF_PLACEHOLDER_CREATE_INFO {
        RelativeFileName: relative.as_ptr(),
        FileIdentity: identity.as_ptr().cast(),
        FileIdentityLength: identity.len() as u32,
        Flags: CF_PLACEHOLDER_CREATE_FLAG_MARK_IN_SYNC,
        ..Default::default()
    };
    info.FsMetadata.FileSize = size;
    info.FsMetadata.BasicInfo.FileAttributes = 0x80; // FILE_ATTRIBUTE_NORMAL
    let mut processed = 0u32;
    // SAFETY: all pointers are valid for the call.
    let hr = unsafe {
        CfCreatePlaceholders(
            base.as_ptr(),
            &mut info,
            1,
            CF_CREATE_FLAG_NONE,
            &mut processed,
        )
    };
    assert!(
        hr >= 0 && info.Result >= 0,
        "CfCreatePlaceholders failed: {hr:#x} / {:#x}",
        info.Result
    );
}

#[test]
fn a_cloud_placeholder_is_refused_and_never_opened_or_moved() {
    let temp = tempfile::tempdir().unwrap();
    let sync = temp.path().join("cloud");
    fs::create_dir(&sync).unwrap();
    let _registration = register_sync_root(&sync);
    create_placeholder(&sync, "scene.rs", 1234);
    fs::write(sync.join("staged"), "staged").unwrap();

    // What a process sees before it opts in with PHCM_EXPOSE_PLACEHOLDERS (the first
    // `Dir::open_root` does that).
    {
        use std::os::windows::fs::MetadataExt;
        for path in [sync.clone(), sync.join("scene.rs")] {
            let attributes = fs::symlink_metadata(&path).unwrap().file_attributes();
            eprintln!(
                "default compatibility mode: {} attributes {attributes:#x} (reparse point: {})",
                path.display(),
                attributes & 0x400 != 0
            );
        }
    }

    // The sync root is itself a cloud reparse point: refused as a project root and as a
    // directory below one.
    let parent = Dir::open_root(temp.path()).unwrap();
    let root_stat = parent.stat("cloud").unwrap().unwrap();
    eprintln!(
        "sync root: kind {:?}, tag {:#010x}",
        root_stat.kind, root_stat.reparse_tag
    );
    assert_eq!(root_stat.kind, EntryKind::CloudPlaceholder);
    let error = Dir::open_root(&sync).unwrap_err();
    eprintln!("open_root(sync root): {error}");
    assert!(
        error.to_string().contains("cloud-files placeholder"),
        "{error}"
    );
    assert!(parent.open_child("cloud").is_err());

    // Inside it (entered through the experiment hook), the placeholder file is refused.
    let nt_path = format!(
        "\\??\\{}",
        fs::canonicalize(&sync)
            .unwrap()
            .display()
            .to_string()
            .trim_start_matches(r"\\?\")
    );
    let dir = publish_primitives::open_unbound(&nt_path).unwrap();
    let stat = dir.stat("scene.rs").unwrap().unwrap();
    eprintln!(
        "placeholder: kind {:?}, tag {:#010x}, size {}",
        stat.kind, stat.reparse_tag, stat.size
    );
    assert_eq!(stat.kind, EntryKind::CloudPlaceholder);
    assert!(publish_primitives::is_cloud_reparse_tag(stat.reparse_tag));

    let error = dir.open_regular("scene.rs").unwrap_err();
    eprintln!("open_regular: {error}");
    assert!(
        error.to_string().contains("cloud-files placeholder"),
        "{error}"
    );
    let error = dir
        .rename_noreplace("scene.rs", &dir, "moved.rs")
        .unwrap_err();
    assert!(
        error.to_string().contains("cloud-files placeholder"),
        "{error}"
    );
    let error = dir
        .rename_noreplace("staged", &dir, "scene.rs")
        .unwrap_err();
    assert_eq!(error.kind(), ErrorKind::AlreadyExists, "{error}");
    // Nothing was hydrated or moved.
    let after = dir.stat("scene.rs").unwrap().unwrap();
    assert_eq!(after.id, stat.id);
    assert_eq!(after.kind, EntryKind::CloudPlaceholder);
    assert!(dir.stat("moved.rs").unwrap().is_none());
}
