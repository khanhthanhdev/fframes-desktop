use std::{
    fs,
    io::{self, Read, Write},
    path::Path,
};

use crate::{
    OpenProject, ProjectError, ProjectPath, SourceInventory,
    lifecycle::{sync_directory, write_manifest},
    paths::io_error,
};

/// Copies without overwriting a collision. The manifest is committed only after durable bytes.
pub fn copy_asset(project: &OpenProject, source: &Path) -> Result<OpenProject, ProjectError> {
    copy_asset_observed(project, source, || {})
}

/// The observer runs after streaming, for deterministic mutation/failure tests.
pub fn copy_asset_observed(
    project: &OpenProject,
    source: &Path,
    copied: impl FnOnce(),
) -> Result<OpenProject, ProjectError> {
    copy_asset_inner(project, source, copied, || false)
}

/// Cooperative cancellation is checked between chunks and immediately before publication.
pub fn copy_asset_with_cancel(
    project: &OpenProject,
    source: &Path,
    cancelled: impl Fn() -> bool,
) -> Result<OpenProject, ProjectError> {
    copy_asset_inner(project, source, || {}, cancelled)
}

fn copy_asset_inner(
    project: &OpenProject,
    source: &Path,
    copied: impl FnOnce(),
    cancelled: impl Fn() -> bool,
) -> Result<OpenProject, ProjectError> {
    let check_cancel = || {
        if cancelled() {
            Err(ProjectError::new(
                source,
                "asset",
                "Asset copy cancelled",
                "Reopen the project to retry",
            ))
        } else {
            Ok(())
        }
    };
    check_cancel()?;
    if SourceInventory::scan(&project.root)?.revision != project.inventory.revision {
        return Err(ProjectError::new(
            &project.root,
            "source",
            "source changed",
            "Refresh the project and retry",
        ));
    }
    let name = source
        .file_name()
        .and_then(|s| s.to_str())
        .ok_or_else(|| io_error(source, "asset name must be UTF-8"))?;
    let source_root = source
        .parent()
        .ok_or_else(|| io_error(source, "missing parent"))?;
    let mut input = ProjectPath::try_from(name.to_owned())?.open_file(source_root)?;
    let media = project.root.join("media");
    if !media.exists() {
        fs::create_dir(&media).map_err(|e| io_error(&media, e))?;
    }
    ProjectPath::try_from("media".to_owned())?.resolve_existing(&project.root)?;
    let mut temp = tempfile::NamedTempFile::new_in(&media).map_err(|e| io_error(&media, e))?;
    let mut buffer = [0; 64 * 1024];
    loop {
        check_cancel()?;
        let count = input.read(&mut buffer).map_err(|e| io_error(source, e))?;
        if count == 0 {
            break;
        }
        temp.write_all(&buffer[..count])
            .map_err(|e| io_error(&media, e))?;
    }
    temp.as_file().sync_all().map_err(|e| io_error(&media, e))?;
    copied();
    let temporary = temp
        .path()
        .strip_prefix(&project.root)
        .unwrap()
        .to_string_lossy()
        .replace('\\', "/");
    check_source(project, &temporary)?;
    check_cancel()?;
    let mut suffix = 0;
    let (target, owned) = loop {
        let candidate = if suffix == 0 {
            name.to_owned()
        } else {
            format!("{suffix}-{name}")
        };
        let relative = ProjectPath::try_from(format!("media/{candidate}"))?;
        ProjectPath::try_from("media".to_owned())?.resolve_existing(&project.root)?;
        // persist_noclobber closes the collision race at publication.
        match temp.persist_noclobber(media.join(candidate)) {
            Ok(file) => break (relative, file),
            Err(e) if e.error.kind() == io::ErrorKind::AlreadyExists => {
                temp = e.file;
                suffix += 1;
            }
            Err(e) => return Err(io_error(&media, e.error)),
        }
    };
    let mut manifest = project.manifest.clone();
    manifest.assets.push(target.clone());
    let commit = || {
        sync_directory(&media)?;
        check_source(project, target.as_str())?;
        check_cancel()?;
        write_manifest(&project.root, &manifest)
    };
    if let Err(error) = commit() {
        // A directory-sync error may follow a successful manifest rename. Never delete
        // bytes already referenced by that manifest, or another writer's replacement.
        let referenced = crate::open(&project.root)
            .map(|p| p.manifest.assets.contains(&target))
            .unwrap_or(true);
        if !referenced && let Ok(path) = target.resolve_existing(&project.root) {
            let current = fs::metadata(&path).map_err(|e| io_error(&path, e))?;
            let original = owned.metadata().map_err(|e| io_error(&path, e))?;
            #[cfg(unix)]
            let unchanged = {
                use std::os::unix::fs::MetadataExt;
                current.dev() == original.dev() && current.ino() == original.ino()
            };
            #[cfg(not(unix))]
            let unchanged = current.len() == original.len()
                && current.modified().ok() == original.modified().ok();
            if unchanged {
                let _ = fs::remove_file(path);
                let _ = sync_directory(&media);
            }
        }
        return Err(error);
    }
    crate::open(&project.root)
}

fn check_source(project: &OpenProject, owned: &str) -> Result<(), ProjectError> {
    let inventory = SourceInventory::scan(&project.root)?;
    if inventory
        .files
        .into_iter()
        .filter(|f| f.path.as_str() != owned)
        .collect::<Vec<_>>()
        != project.inventory.files
    {
        return Err(ProjectError::new(
            &project.root,
            "source",
            "source changed while copying the asset",
            "Refresh the project and retry; external edits were preserved",
        ));
    }
    Ok(())
}
