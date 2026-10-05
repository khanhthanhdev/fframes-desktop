use std::{
    fs,
    io::{Read, Write},
    path::{Path, PathBuf},
};

use sha2::{Digest, Sha256};

use crate::{
    ProjectError, SourceInventory, SourceRevision,
    lifecycle::{atomic_write, sync_directory},
    paths::io_error,
};

/// Immutable manifests reference durable content-addressed objects; source is never restored implicitly.
pub struct Checkpoints {
    pub root: PathBuf,
}
impl Checkpoints {
    pub fn new(root: &Path) -> Result<Self, ProjectError> {
        for dir in ["objects", "checkpoints", "drafts"] {
            fs::create_dir_all(root.join(dir)).map_err(|e| io_error(root, e))?;
        }
        Ok(Self { root: root.into() })
    }
    pub fn capture(&self, source: &Path) -> Result<SourceRevision, ProjectError> {
        let inventory = SourceInventory::scan(source)?;
        for file in &inventory.files {
            let object = self.root.join("objects").join(&file.sha256);
            if object.exists() {
                self.verify_object(&object, &file.sha256)?;
                continue;
            }
            let mut input = file.path.open_file(source)?;
            let mut temp = tempfile::NamedTempFile::new_in(self.root.join("objects"))
                .map_err(|e| io_error(&object, e))?;
            let digest = stream(&mut input, &mut temp).map_err(|e| io_error(&object, e))?;
            if digest != file.sha256 {
                return Err(io_error(
                    source,
                    "source changed while checkpointing; retry",
                ));
            }
            temp.as_file()
                .sync_all()
                .map_err(|e| io_error(&object, e))?;
            match temp.persist_noclobber(&object) {
                Ok(_) => (),
                Err(e) if e.error.kind() == std::io::ErrorKind::AlreadyExists => {
                    self.verify_object(&object, &file.sha256)?
                }
                Err(e) => return Err(io_error(&object, e.error)),
            }
        }
        sync_directory(&self.root.join("objects"))?;
        if SourceInventory::scan(source)?.revision != inventory.revision {
            return Err(io_error(
                source,
                "source changed while checkpointing; retry",
            ));
        }
        let manifest = self.manifest_path(&inventory.revision);
        if !manifest.exists() {
            atomic_write(&manifest, &serde_json::to_vec(&inventory).unwrap())?;
        }
        self.load(&inventory.revision)?;
        Ok(inventory.revision)
    }
    pub fn manifest_path(&self, revision: &SourceRevision) -> PathBuf {
        self.root
            .join("checkpoints")
            .join(format!("{}.json", revision.as_str()))
    }
    pub fn load(&self, revision: &SourceRevision) -> Result<SourceInventory, ProjectError> {
        let path = self.manifest_path(revision);
        let inventory: SourceInventory =
            serde_json::from_slice(&fs::read(&path).map_err(|e| io_error(&path, e))?)
                .map_err(|e| io_error(&path, e))?;
        if &inventory.revision != revision {
            return Err(io_error(&path, "checkpoint identity mismatch"));
        }
        inventory.verify()?;
        for file in &inventory.files {
            let object = self.object_path(&file.sha256)?;
            if fs::metadata(&object)
                .map_err(|e| io_error(&object, e))?
                .len()
                != file.size
            {
                return Err(io_error(&object, "checkpoint object size mismatch"));
            }
            self.verify_object(&object, &file.sha256)?;
        }
        Ok(inventory)
    }
    fn object_path(&self, hash: &str) -> Result<PathBuf, ProjectError> {
        let _: SourceRevision = hash
            .to_owned()
            .try_into()
            .map_err(|e: String| io_error(&self.root, e))?;
        Ok(self.root.join("objects").join(hash))
    }
    fn verify_object(&self, path: &Path, digest: &str) -> Result<(), ProjectError> {
        let hash = stream(
            &mut fs::File::open(path).map_err(|e| io_error(path, e))?,
            &mut std::io::sink(),
        )
        .map_err(|e| io_error(path, e))?;
        if hash != digest {
            return Err(ProjectError::new(
                path,
                "checkpoint",
                "object integrity failure",
                "Preserve history and repair the missing/corrupt object from backup",
            ));
        }
        Ok(())
    }
    /// Streams a content-addressed object into `output`, verifying that its bytes hash
    /// to `hash` and have `size` bytes. A missing, truncated or corrupt object is an
    /// error and may leave a partial write in `output` for the caller to discard.
    pub fn copy_object(
        &self,
        hash: &str,
        size: u64,
        output: &mut impl Write,
    ) -> Result<(), ProjectError> {
        let path = self.object_path(hash)?;
        let mut input = fs::File::open(&path).map_err(|e| io_error(&path, e))?;
        let mut counted = CountingWriter {
            inner: output,
            count: 0,
        };
        let digest = stream(&mut input, &mut counted).map_err(|e| io_error(&path, e))?;
        if digest != hash || counted.count != size {
            return Err(ProjectError::new(
                &path,
                "checkpoint",
                "object integrity failure",
                "Preserve history and repair the missing/corrupt object from backup",
            ));
        }
        Ok(())
    }
    /// Verifies that every object of `revision` exists with the recorded size and
    /// hash, and returns its verified inventory.
    pub fn verified_inventory(
        &self,
        revision: &SourceRevision,
    ) -> Result<SourceInventory, ProjectError> {
        self.load(revision)
    }
    pub fn draft(&self, revision: &SourceRevision, name: &str) -> Result<PathBuf, ProjectError> {
        let inventory = self.load(revision)?;
        let path = self
            .root
            .join("drafts")
            .join(crate::ProjectPath::try_from(name.to_owned())?.as_str());
        fs::create_dir(&path).map_err(|e| io_error(&path, e))?;
        for file in inventory.files {
            let target = path.join(file.path.as_str());
            fs::create_dir_all(target.parent().unwrap()).map_err(|e| io_error(&target, e))?;
            let mut output = fs::OpenOptions::new()
                .create_new(true)
                .write(true)
                .open(&target)
                .map_err(|e| io_error(&target, e))?;
            let hash = stream(
                &mut fs::File::open(self.object_path(&file.sha256)?)
                    .map_err(|e| io_error(&target, e))?,
                &mut output,
            )
            .map_err(|e| io_error(&target, e))?;
            if hash != file.sha256 {
                return Err(io_error(&target, "object changed"));
            }
            output.sync_all().map_err(|e| io_error(&target, e))?;
            #[cfg(unix)]
            if file.executable {
                use std::os::unix::fs::PermissionsExt;
                fs::set_permissions(&target, fs::Permissions::from_mode(0o755))
                    .map_err(|e| io_error(&target, e))?;
            }
            sync_directory(target.parent().unwrap())?;
        }
        sync_directory(&path)?;
        sync_directory(path.parent().unwrap())?;
        if !SourceInventory::scan(&path)?.matches_revision(revision) {
            return Err(io_error(&path, "checkpoint manifest integrity failure"));
        }
        Ok(path)
    }
}

fn stream(input: &mut impl Read, output: &mut impl Write) -> std::io::Result<String> {
    let mut hash = Sha256::new();
    let mut buffer = [0; 64 * 1024];
    loop {
        let count = input.read(&mut buffer)?;
        if count == 0 {
            break;
        }
        hash.update(&buffer[..count]);
        output.write_all(&buffer[..count])?;
    }
    Ok(format!("{:x}", hash.finalize()))
}

pub fn copy_draft(source: &Path, destination: &Path) -> Result<(), ProjectError> {
    let objects = tempfile::tempdir().map_err(|e| io_error(destination, e))?;
    let store = Checkpoints::new(objects.path())?;
    let revision = store.capture(source)?;
    let draft = store.draft(&revision, "copy")?;
    if destination.exists() {
        return Err(io_error(destination, "draft destination already exists"));
    }
    // Destination may be on a different filesystem from temporary object storage.
    fs::create_dir(destination).map_err(|e| io_error(destination, e))?;
    for file in store.load(&revision)?.files {
        let path = destination.join(file.path.as_str());
        fs::create_dir_all(path.parent().unwrap()).map_err(|e| io_error(&path, e))?;
        fs::copy(draft.join(file.path.as_str()), &path).map_err(|e| io_error(&path, e))?;
        #[cfg(unix)]
        if file.executable {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&path, fs::Permissions::from_mode(0o755))
                .map_err(|e| io_error(&path, e))?;
        }
    }
    Ok(())
}

struct CountingWriter<'a, W: Write> {
    inner: &'a mut W,
    count: u64,
}
impl<W: Write> Write for CountingWriter<'_, W> {
    fn write(&mut self, buffer: &[u8]) -> std::io::Result<usize> {
        let written = self.inner.write(buffer)?;
        self.count += written as u64;
        Ok(written)
    }
    fn flush(&mut self) -> std::io::Result<()> {
        self.inner.flush()
    }
}
