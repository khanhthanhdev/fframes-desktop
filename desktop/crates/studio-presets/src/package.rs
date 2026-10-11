//! Versioned preset packages: manifest, in-memory verification, bounded no-follow directory
//! import/export with no-clobber staged publication, and canonical hashing.

use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    io::{Read, Write},
    path::Path,
};

use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use studio_project::ProjectPath;

use crate::{
    model::{
        Code, Design, Diagnostic, PRESET_SCHEMA, PresetError, TOKEN_SCHEMA, TokenDef, TokenValue,
        check_schema, json_guard,
    },
    resolve::{OverrideLayer, ResolvedSnapshot, TokenSet, hex, resolve},
};

pub const MANIFEST_FILE: &str = "preset.json";
pub const MAX_FILES: usize = 64;
pub const MAX_TOTAL_BYTES: u64 = 24 * 1024 * 1024;
pub const MAX_FONT_BYTES: u64 = 8 * 1024 * 1024;
pub const MAX_IMAGE_BYTES: u64 = 8 * 1024 * 1024;
pub const MAX_TEXT_BYTES: u64 = 256 * 1024;
pub const MAX_DIR_DEPTH: usize = 4;
pub const MAX_PATH_LEN: usize = 128;
pub const MAX_COMPONENT_LEN: usize = 64;
const MAX_FONTS: usize = 16;
const MAX_ASSETS: usize = 32;
const MAX_EXAMPLES: usize = 16;
const MAX_NOTICES: usize = 16;
const HASH_FORMAT: &str = "fframes-preset-hash-v1";

/// SPDX identifiers accepted for bundled resources.
pub const ALLOWED_LICENSES: &[&str] = &[
    "MIT",
    "Apache-2.0",
    "OFL-1.1",
    "CC0-1.0",
    "CC-BY-4.0",
    "BSD-2-Clause",
    "BSD-3-Clause",
    "ISC",
    "Unlicense",
];

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FileRef {
    pub path: ProjectPath,
    pub sha256: String,
    pub bytes: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Notice {
    pub spdx: String,
    pub file: FileRef,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FontDecl {
    pub family: String,
    pub file: FileRef,
    /// Path of an entry in `licenses`.
    pub license: ProjectPath,
}

/// A reusable image (`png`, `jpg`/`jpeg`, `webp` or `svg`) or example text.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ResourceDecl {
    pub file: FileRef,
    pub license: ProjectPath,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Manifest {
    pub schema: u32,
    pub token_schema: u32,
    pub id: String,
    pub name: String,
    pub version: String,
    pub description: String,
    pub author: String,
    pub design: Design,
    pub tokens: FileRef,
    pub guide: FileRef,
    /// License of the preset's own original content (tokens, guide, examples); an entry of `licenses`.
    pub license: ProjectPath,
    pub licenses: Vec<Notice>,
    #[serde(default)]
    pub fonts: Vec<FontDecl>,
    #[serde(default)]
    pub assets: Vec<ResourceDecl>,
    #[serde(default)]
    pub examples: Vec<ResourceDecl>,
}

#[derive(Clone, Copy)]
enum Role<'a> {
    Tokens,
    Guide,
    Notice(&'a Notice),
    Font(&'a FontDecl),
    Asset,
    Example,
}

impl Manifest {
    fn entries(&self) -> Vec<(&FileRef, Role<'_>)> {
        let mut out = vec![(&self.tokens, Role::Tokens), (&self.guide, Role::Guide)];
        out.extend(self.licenses.iter().map(|n| (&n.file, Role::Notice(n))));
        out.extend(self.fonts.iter().map(|f| (&f.file, Role::Font(f))));
        out.extend(self.assets.iter().map(|a| (&a.file, Role::Asset)));
        out.extend(self.examples.iter().map(|e| (&e.file, Role::Example)));
        out
    }

    fn file_refs_mut(&mut self) -> Vec<&mut FileRef> {
        let mut out = vec![&mut self.tokens, &mut self.guide];
        out.extend(self.licenses.iter_mut().map(|n| &mut n.file));
        out.extend(self.fonts.iter_mut().map(|f| &mut f.file));
        out.extend(self.assets.iter_mut().map(|a| &mut a.file));
        out.extend(self.examples.iter_mut().map(|e| &mut e.file));
        out
    }

    fn validate_fields(&self) -> Vec<Diagnostic> {
        let mut errors = Vec::new();
        fn bad(errors: &mut Vec<Diagnostic>, field: &str, why: &str) {
            errors.push(Diagnostic::error(Code::InvalidValue, field, why));
        }
        let slug = !self.id.is_empty()
            && self.id.len() <= 48
            && self
                .id
                .bytes()
                .next()
                .is_some_and(|c| c.is_ascii_lowercase() || c.is_ascii_digit())
            && self
                .id
                .bytes()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == b'-');
        if !slug {
            bad(
                &mut errors,
                "id",
                "id must be 1..=48 characters of [a-z0-9-] starting with [a-z0-9]",
            );
        }
        let text_ok = |s: &str, max: usize, empty_ok: bool| {
            (empty_ok || !s.trim().is_empty()) && s.len() <= max && !s.chars().any(char::is_control)
        };
        if !text_ok(&self.name, 64, false) {
            bad(
                &mut errors,
                "name",
                "name must be 1..=64 characters without control characters",
            );
        }
        if !text_ok(&self.description, 512, true) {
            bad(
                &mut errors,
                "description",
                "description must be at most 512 characters",
            );
        }
        if !text_ok(&self.author, 128, false) {
            bad(&mut errors, "author", "author must be 1..=128 characters");
        }
        let parts: Vec<&str> = self.version.split('.').collect();
        if parts.len() != 3
            || parts
                .iter()
                .any(|p| p.is_empty() || p.len() > 5 || !p.bytes().all(|c| c.is_ascii_digit()))
        {
            bad(&mut errors, "version", "version must be MAJOR.MINOR.PATCH");
        }
        if let Err(d) = self.design.validate("design") {
            errors.push(d);
        }
        if self.fonts.len() > MAX_FONTS
            || self.assets.len() > MAX_ASSETS
            || self.examples.len() > MAX_EXAMPLES
            || self.licenses.len() > MAX_NOTICES
        {
            bad(
                &mut errors,
                "manifest",
                "too many declared fonts, assets, examples or licenses",
            );
        }
        errors
    }
}

fn sha256_hex(bytes: &[u8]) -> String {
    hex(&Sha256::digest(bytes))
}

fn err(code: Code, field: impl Into<String>, message: impl Into<String>) -> Diagnostic {
    Diagnostic::error(code, field, message)
}

/// A fully verified preset: every path, size, digest, font, media type and license has been checked.
#[derive(Debug, Clone)]
pub struct Package {
    manifest: Manifest,
    tokens: TokenSet,
    files: BTreeMap<ProjectPath, Vec<u8>>,
    hash: String,
}

impl Package {
    /// Verify an in-memory file tree (relative `ProjectPath` -> bytes). No filesystem access.
    pub fn from_files(files: BTreeMap<ProjectPath, Vec<u8>>) -> Result<Self, PresetError> {
        let mut errors = check_tree(&files);
        if !errors.is_empty() {
            return Err(PresetError::new(errors));
        }
        let manifest_path = ProjectPath::try_from(MANIFEST_FILE.to_owned()).expect("valid");
        let raw = files.get(&manifest_path).ok_or_else(|| {
            PresetError::one(
                Code::MissingResource,
                MANIFEST_FILE,
                "preset.json is required",
            )
        })?;
        if raw.len() as u64 > MAX_TEXT_BYTES {
            return Err(PresetError::one(
                Code::TooLarge,
                MANIFEST_FILE,
                "preset.json is too large",
            ));
        }
        json_guard(raw, MANIFEST_FILE)?;
        check_schema(raw, MANIFEST_FILE, PRESET_SCHEMA)?;
        let manifest: Manifest = serde_json::from_slice(raw).map_err(|e| {
            PresetError::one(
                Code::Malformed,
                MANIFEST_FILE,
                format!("invalid manifest: {e}"),
            )
        })?;
        if manifest.token_schema != TOKEN_SCHEMA {
            return Err(PresetError::one(
                Code::UnsupportedSchema,
                "token_schema",
                format!(
                    "token schema {} is unsupported; this build reads {TOKEN_SCHEMA}",
                    manifest.token_schema
                ),
            ));
        }
        errors.extend(manifest.validate_fields());
        verify_declared(&manifest, &files, &mut errors);
        if !errors.is_empty() {
            return Err(PresetError::new(errors));
        }

        let tokens = TokenSet::parse(&files[&manifest.tokens.path], &manifest.design)?;
        let declared: BTreeSet<&str> = manifest.fonts.iter().map(|f| f.family.as_str()).collect();
        for (name, def) in tokens.defs() {
            if let TokenDef::Literal(TokenValue::Typography(t)) = def
                && !declared.contains(t.family.as_str())
            {
                errors.push(err(
                    Code::FontMismatch,
                    format!("tokens.{name}.value.family"),
                    format!(
                        "family `{}` has no bundled font; declare it in `fonts` with its file and license (system fonts are never substituted)",
                        t.family
                    ),
                ));
            }
        }
        if !errors.is_empty() {
            return Err(PresetError::new(errors));
        }
        let hash = canonical_hash(&manifest, &tokens);
        Ok(Self {
            manifest,
            tokens,
            files,
            hash,
        })
    }

    pub fn manifest(&self) -> &Manifest {
        &self.manifest
    }
    pub fn tokens(&self) -> &TokenSet {
        &self.tokens
    }
    pub fn id(&self) -> &str {
        &self.manifest.id
    }
    /// Stable SHA-256 (hex) over normalized metadata, normalized tokens and every resource digest.
    pub fn hash(&self) -> &str {
        &self.hash
    }
    /// Every package file keyed by package-relative path, including `preset.json`.
    pub fn files(&self) -> &BTreeMap<ProjectPath, Vec<u8>> {
        &self.files
    }
    pub fn file(&self, path: &ProjectPath) -> Option<&[u8]> {
        self.files.get(path).map(Vec::as_slice)
    }
    pub fn guide(&self) -> &[u8] {
        &self.files[&self.manifest.guide.path]
    }
    /// Defaults with optional project and scene override layers applied.
    pub fn snapshot(
        &self,
        project: Option<&OverrideLayer>,
        scene: Option<&OverrideLayer>,
    ) -> ResolvedSnapshot {
        resolve(&self.tokens, project, scene)
    }
}

/// Path, count, size and case-collision checks that need no manifest.
fn check_tree(files: &BTreeMap<ProjectPath, Vec<u8>>) -> Vec<Diagnostic> {
    let mut errors = Vec::new();
    if files.len() > MAX_FILES {
        errors.push(err(
            Code::TooLarge,
            "files",
            format!("more than {MAX_FILES} files"),
        ));
        return errors;
    }
    let mut total = 0u64;
    let mut folded = BTreeSet::new();
    for (path, bytes) in files {
        let p = path.as_str();
        let comps: Vec<&str> = p.split('/').collect();
        if p.len() > MAX_PATH_LEN || comps.iter().any(|c| c.len() > MAX_COMPONENT_LEN) {
            errors.push(err(Code::InvalidPath, p, "path or name too long"));
        }
        if comps.len() > MAX_DIR_DEPTH + 1 {
            errors.push(err(Code::InvalidPath, p, "directories nested too deeply"));
        }
        if !folded.insert(p.to_lowercase()) {
            errors.push(err(
                Code::DuplicatePath,
                p,
                "path collides case-insensitively with another file",
            ));
        }
        total += bytes.len() as u64;
    }
    if total > MAX_TOTAL_BYTES {
        errors.push(err(
            Code::TooLarge,
            "files",
            format!("package exceeds {MAX_TOTAL_BYTES} bytes"),
        ));
    }
    errors
}

fn is_text(bytes: &[u8]) -> bool {
    std::str::from_utf8(bytes).is_ok_and(|s| !s.contains('\0'))
}

fn verify_declared(
    manifest: &Manifest,
    files: &BTreeMap<ProjectPath, Vec<u8>>,
    errors: &mut Vec<Diagnostic>,
) {
    let entries = manifest.entries();
    let mut seen: BTreeSet<&ProjectPath> = BTreeSet::new();
    for (file, _) in &entries {
        if !seen.insert(&file.path) || file.path.as_str() == MANIFEST_FILE {
            errors.push(err(
                Code::DuplicatePath,
                file.path.as_str(),
                "path is declared more than once",
            ));
        }
    }
    let notices: BTreeMap<&ProjectPath, &Notice> = manifest
        .licenses
        .iter()
        .map(|n| (&n.file.path, n))
        .collect();
    let mut need_license = vec![("license", &manifest.license)];
    for (file, role) in &entries {
        let field = file.path.as_str();
        let limit = match role {
            Role::Font(_) => MAX_FONT_BYTES,
            Role::Asset => MAX_IMAGE_BYTES,
            _ => MAX_TEXT_BYTES,
        };
        let Some(bytes) = files.get(&file.path) else {
            errors.push(err(
                Code::MissingResource,
                field,
                "declared file is missing from the package",
            ));
            continue;
        };
        if bytes.len() as u64 > limit || file.bytes > limit {
            errors.push(err(
                Code::TooLarge,
                field,
                format!("file exceeds {limit} bytes"),
            ));
            continue;
        }
        if bytes.len() as u64 != file.bytes
            || file.sha256.len() != 64
            || !file
                .sha256
                .bytes()
                .all(|c| c.is_ascii_digit() || (b'a'..=b'f').contains(&c))
            || sha256_hex(bytes) != file.sha256
        {
            errors.push(err(
                Code::HashMismatch,
                field,
                "size or SHA-256 differs from the manifest",
            ));
            continue;
        }
        match role {
            Role::Tokens | Role::Guide => {
                if !is_text(bytes) {
                    errors.push(err(Code::Malformed, field, "expected UTF-8 text"));
                }
            }
            Role::Notice(n) => verify_notice(n, bytes, errors),
            Role::Font(font) => {
                need_license.push((field, &font.license));
                verify_font(font, bytes, errors);
            }
            Role::Asset => verify_image(&file.path, bytes, errors),
            Role::Example => {
                if !is_text(bytes) {
                    errors.push(err(Code::Malformed, field, "example must be UTF-8 text"));
                }
            }
        }
    }
    for decl in manifest.assets.iter().chain(&manifest.examples) {
        need_license.push((decl.file.path.as_str(), &decl.license));
    }
    for (owner, license) in need_license {
        if !notices.contains_key(license) {
            errors.push(err(
                Code::MissingLicense,
                owner,
                format!(
                    "license notice `{}` is not declared in `licenses`",
                    license.as_str()
                ),
            ));
        }
    }
    let declared: BTreeSet<&ProjectPath> = entries.iter().map(|(f, _)| &f.path).collect();
    for path in files.keys() {
        if path.as_str() != MANIFEST_FILE && !declared.contains(path) {
            errors.push(err(
                Code::UndeclaredFile,
                path.as_str(),
                "file is not declared in preset.json",
            ));
        }
    }
}

fn verify_notice(notice: &Notice, bytes: &[u8], errors: &mut Vec<Diagnostic>) {
    let field = notice.file.path.as_str();
    if !ALLOWED_LICENSES.contains(&notice.spdx.as_str()) {
        errors.push(err(
            Code::InvalidLicense,
            field,
            format!("SPDX id `{}` is not in the accepted list", notice.spdx),
        ));
        return;
    }
    let Ok(text) = std::str::from_utf8(bytes) else {
        errors.push(err(
            Code::InvalidLicense,
            field,
            "license notice must be UTF-8 text",
        ));
        return;
    };
    let lower = text.to_lowercase();
    let marker = match notice.spdx.as_str() {
        "OFL-1.1" => Some("sil open font license"),
        "MIT" => Some("permission is hereby granted"),
        "Apache-2.0" => Some("apache license"),
        _ => None,
    };
    if text.trim().len() < 40 || marker.is_some_and(|m| !lower.contains(m)) {
        errors.push(err(
            Code::InvalidLicense,
            field,
            format!(
                "notice text does not match declared license {}",
                notice.spdx
            ),
        ));
    }
}

fn font_families(bytes: &[u8]) -> Result<BTreeSet<String>, String> {
    if bytes.starts_with(b"ttcf") {
        return Err("font collections (.ttc) are not supported".into());
    }
    let face = ttf_parser::Face::parse(bytes, 0).map_err(|e| format!("unreadable font: {e}"))?;
    let mut families = BTreeSet::new();
    for name in face.names() {
        if matches!(
            name.name_id,
            ttf_parser::name_id::FAMILY
                | ttf_parser::name_id::TYPOGRAPHIC_FAMILY
                | ttf_parser::name_id::WWS_FAMILY
        ) && let Some(text) = name.to_string()
        {
            families.insert(text);
        }
    }
    Ok(families)
}

fn verify_font(font: &FontDecl, bytes: &[u8], errors: &mut Vec<Diagnostic>) {
    let field = font.file.path.as_str();
    let ext = field
        .rsplit('.')
        .next()
        .unwrap_or_default()
        .to_ascii_lowercase();
    if ext != "ttf" && ext != "otf" {
        errors.push(err(
            Code::UnsupportedMedia,
            field,
            "font files must be .ttf or .otf",
        ));
        return;
    }
    match font_families(bytes) {
        Err(why) => errors.push(err(Code::UnsupportedMedia, field, why)),
        Ok(families) if !families.contains(&font.family) => errors.push(err(
            Code::FontMismatch,
            field,
            format!(
                "declared family `{}` is not a family name in the font (found: {})",
                font.family,
                families.into_iter().collect::<Vec<_>>().join(", ")
            ),
        )),
        Ok(_) => {}
    }
}

fn verify_image(path: &ProjectPath, bytes: &[u8], errors: &mut Vec<Diagnostic>) {
    let field = path.as_str();
    let ext = field
        .rsplit('.')
        .next()
        .unwrap_or_default()
        .to_ascii_lowercase();
    let ok = match ext.as_str() {
        "png" => bytes.starts_with(b"\x89PNG\r\n\x1a\n"),
        "jpg" | "jpeg" => bytes.starts_with(&[0xFF, 0xD8, 0xFF]),
        "webp" => bytes.len() > 12 && &bytes[..4] == b"RIFF" && &bytes[8..12] == b"WEBP",
        "svg" => std::str::from_utf8(bytes).is_ok_and(|s| {
            let lower = s.to_lowercase();
            lower.contains("<svg")
                && !lower.contains("<script")
                && !lower.contains("<foreignobject")
                && !lower.contains("href=\"http")
        }),
        _ => {
            errors.push(err(
                Code::UnsupportedMedia,
                field,
                "unsupported asset type; use png, jpg, jpeg, webp or svg",
            ));
            return;
        }
    };
    if !ok {
        errors.push(err(
            Code::UnsupportedMedia,
            field,
            format!("content is not a valid .{ext} (signature or safety check failed)"),
        ));
    }
}

// ------------------------------------------------------------------------------- canonical hash

fn canonical_value(value: &Value, out: &mut String) {
    match value {
        Value::Object(map) => {
            let mut keys: Vec<&String> = map.keys().collect();
            keys.sort();
            out.push('{');
            for (i, key) in keys.into_iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                out.push_str(&serde_json::to_string(key).expect("string"));
                out.push(':');
                canonical_value(&map[key], out);
            }
            out.push('}');
        }
        Value::Array(items) => {
            out.push('[');
            for (i, item) in items.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                canonical_value(item, out);
            }
            out.push(']');
        }
        other => out.push_str(&serde_json::to_string(other).expect("scalar")),
    }
}

fn canonical_hash(manifest: &Manifest, tokens: &TokenSet) -> String {
    let mut m = manifest.clone();
    m.licenses.sort_by(|a, b| a.file.path.cmp(&b.file.path));
    m.fonts.sort_by(|a, b| a.file.path.cmp(&b.file.path));
    m.assets.sort_by(|a, b| a.file.path.cmp(&b.file.path));
    m.examples.sort_by(|a, b| a.file.path.cmp(&b.file.path));
    let mut value = serde_json::to_value(&m).expect("manifest serializes");
    let object = value.as_object_mut().expect("manifest is an object");
    // The authored file's formatting must not matter; its normalized meaning is hashed instead.
    object.remove("tokens");
    object.insert(
        "tokens_normalized".into(),
        serde_json::from_str(&tokens.to_canonical_json()).expect("canonical tokens are JSON"),
    );
    object.insert("format".into(), Value::String(HASH_FORMAT.into()));
    let mut text = String::new();
    canonical_value(&value, &mut text);
    sha256_hex(text.as_bytes())
}

// ------------------------------------------------------------------------------ directory I/O

fn io_diag(code: Code, path: &Path, e: impl ToString) -> PresetError {
    PresetError::one(code, path.display().to_string(), e.to_string())
}

/// Reads a directory without following links; any link/special file or limit breach is a failure.
pub fn read_dir_files(root: &Path) -> Result<BTreeMap<ProjectPath, Vec<u8>>, PresetError> {
    let meta = fs::symlink_metadata(root).map_err(|e| io_diag(Code::Io, root, e))?;
    if !meta.is_dir() {
        return Err(io_diag(
            Code::UnsafeFile,
            root,
            "preset root must be a real directory (links are not followed)",
        ));
    }
    let mut files = BTreeMap::new();
    let mut total = 0u64;
    let mut stack = vec![(String::new(), 0usize)];
    while let Some((rel, depth)) = stack.pop() {
        let dir = if rel.is_empty() {
            root.to_path_buf()
        } else {
            root.join(&rel)
        };
        let mut names = Vec::new();
        for entry in fs::read_dir(&dir).map_err(|e| io_diag(Code::Io, &dir, e))? {
            let entry = entry.map_err(|e| io_diag(Code::Io, &dir, e))?;
            let name = entry.file_name().into_string().map_err(|_| {
                io_diag(Code::InvalidPath, &entry.path(), "file names must be UTF-8")
            })?;
            names.push(name);
            if names.len() > MAX_FILES * 2 {
                return Err(io_diag(Code::TooLarge, &dir, "too many directory entries"));
            }
        }
        names.sort();
        for name in names {
            let child_rel = if rel.is_empty() {
                name
            } else {
                format!("{rel}/{name}")
            };
            let abs = root.join(&child_rel);
            // Validate the name before touching it: Windows resolves a non-portable name such
            // as `name.` to a different file, so a later check would see the wrong entry.
            let path = ProjectPath::try_from(child_rel.clone()).map_err(|e| {
                io_diag(
                    Code::InvalidPath,
                    &abs,
                    format!("{}: {}", e.reason, e.action),
                )
            })?;
            let meta = fs::symlink_metadata(&abs).map_err(|e| io_diag(Code::Io, &abs, e))?;
            let kind = meta.file_type();
            if kind.is_symlink() {
                return Err(io_diag(
                    Code::UnsafeFile,
                    &abs,
                    "symbolic links are not allowed",
                ));
            } else if kind.is_dir() {
                if depth + 1 > MAX_DIR_DEPTH {
                    return Err(io_diag(
                        Code::TooDeep,
                        &abs,
                        "directories nested too deeply",
                    ));
                }
                stack.push((child_rel, depth + 1));
            } else if kind.is_file() {
                if files.len() >= MAX_FILES {
                    return Err(io_diag(
                        Code::TooLarge,
                        root,
                        format!("more than {MAX_FILES} files"),
                    ));
                }
                let limit = MAX_FONT_BYTES.max(MAX_IMAGE_BYTES);
                if meta.len() > limit || total + meta.len() > MAX_TOTAL_BYTES {
                    return Err(io_diag(
                        Code::TooLarge,
                        &abs,
                        "file or package exceeds size limits",
                    ));
                }
                let mut file = path.open_file(root).map_err(|e| {
                    io_diag(
                        Code::UnsafeFile,
                        &abs,
                        format!("{}: {}", e.reason, e.action),
                    )
                })?;
                if !file
                    .metadata()
                    .map_err(|e| io_diag(Code::Io, &abs, e))?
                    .is_file()
                {
                    return Err(io_diag(Code::UnsafeFile, &abs, "not a regular file"));
                }
                let mut bytes = Vec::with_capacity(meta.len() as usize);
                (&mut file)
                    .take(limit + 1)
                    .read_to_end(&mut bytes)
                    .map_err(|e| io_diag(Code::Io, &abs, e))?;
                if bytes.len() as u64 > limit {
                    return Err(io_diag(
                        Code::TooLarge,
                        &abs,
                        "file grew beyond the size limit",
                    ));
                }
                total += bytes.len() as u64;
                files.insert(path, bytes);
            } else {
                return Err(io_diag(
                    Code::UnsafeFile,
                    &abs,
                    "special files (fifo, socket, device) are not allowed",
                ));
            }
        }
    }
    Ok(files)
}

/// Verify a preset directory and publish a copy at `dest` (which must not exist).
/// Nothing is written to `dest`'s parent unless the whole package verified.
pub fn import_dir(source: &Path, dest: &Path) -> Result<Package, PresetError> {
    let package = Package::from_files(read_dir_files(source)?)?;
    publish(&package, dest)?;
    Ok(package)
}

/// Write a verified package to `dest` (which must not exist) in canonical directory form.
pub fn export_dir(package: &Package, dest: &Path) -> Result<(), PresetError> {
    publish(package, dest)
}

/// Stage beside `dest`, verify the staged copy byte-for-byte, then publish without replacement.
pub fn publish(package: &Package, dest: &Path) -> Result<(), PresetError> {
    let name = dest.file_name().and_then(|n| n.to_str()).ok_or_else(|| {
        io_diag(
            Code::InvalidPath,
            dest,
            "destination needs a UTF-8 final name",
        )
    })?;
    if name.starts_with(".preset-import-") {
        return Err(io_diag(Code::InvalidPath, dest, "reserved staging prefix"));
    }
    if fs::symlink_metadata(dest).is_ok() {
        return Err(io_diag(
            Code::AlreadyExists,
            dest,
            "destination already exists; presets are never replaced",
        ));
    }
    let parent = match dest.parent() {
        Some(p) if !p.as_os_str().is_empty() => p,
        _ => Path::new("."),
    };
    fs::create_dir_all(parent).map_err(|e| io_diag(Code::Io, parent, e))?;
    let stage = tempfile::Builder::new()
        .prefix(".preset-import-")
        .tempdir_in(parent)
        .map_err(|e| io_diag(Code::Io, parent, e))?;
    for (path, bytes) in package.files() {
        let target = stage.path().join(path.as_str());
        if let Some(dir) = target.parent() {
            fs::create_dir_all(dir).map_err(|e| io_diag(Code::Io, dir, e))?;
        }
        let mut file = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&target)
            .map_err(|e| io_diag(Code::Io, &target, e))?;
        file.write_all(bytes)
            .map_err(|e| io_diag(Code::Io, &target, e))?;
        file.sync_all().map_err(|e| io_diag(Code::Io, &target, e))?;
    }
    let staged = Package::from_files(read_dir_files(stage.path())?)?;
    if staged.hash() != package.hash() || staged.files() != package.files() {
        return Err(io_diag(
            Code::HashMismatch,
            stage.path(),
            "staged copy differs from the verified package",
        ));
    }
    sync_tree(stage.path())?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(stage.path(), fs::Permissions::from_mode(0o755))
            .map_err(|e| io_diag(Code::Io, stage.path(), e))?;
    }
    let staged_path = stage.keep();
    if let Err(e) = rename_noreplace(&staged_path, dest) {
        let _ = fs::remove_dir_all(&staged_path);
        return Err(e);
    }
    sync_dir(parent);
    Ok(())
}

fn sync_dir(path: &Path) {
    #[cfg(unix)]
    if let Ok(dir) = fs::File::open(path) {
        let _ = dir.sync_all();
    }
    #[cfg(not(unix))]
    let _ = path;
}

fn sync_tree(root: &Path) -> Result<(), PresetError> {
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        for entry in fs::read_dir(&dir).map_err(|e| io_diag(Code::Io, &dir, e))? {
            let entry = entry.map_err(|e| io_diag(Code::Io, &dir, e))?;
            if entry.file_type().is_ok_and(|t| t.is_dir()) {
                stack.push(entry.path());
            }
        }
        sync_dir(&dir);
    }
    Ok(())
}

fn rename_noreplace(from: &Path, to: &Path) -> Result<(), PresetError> {
    let exists = || {
        io_diag(
            Code::AlreadyExists,
            to,
            "destination already exists; presets are never replaced",
        )
    };
    #[cfg(target_os = "linux")]
    {
        use std::{ffi::CString, os::unix::ffi::OsStrExt};
        let a = CString::new(from.as_os_str().as_bytes())
            .map_err(|e| io_diag(Code::InvalidPath, from, e))?;
        let b = CString::new(to.as_os_str().as_bytes())
            .map_err(|e| io_diag(Code::InvalidPath, to, e))?;
        const RENAME_NOREPLACE: libc::c_uint = 1;
        // SAFETY: both strings are NUL-terminated and outlive the call.
        let rc = unsafe {
            libc::syscall(
                libc::SYS_renameat2,
                libc::AT_FDCWD,
                a.as_ptr(),
                libc::AT_FDCWD,
                b.as_ptr(),
                RENAME_NOREPLACE,
            )
        };
        if rc == 0 {
            return Ok(());
        }
        let e = std::io::Error::last_os_error();
        match e.raw_os_error() {
            Some(libc::EEXIST | libc::ENOTEMPTY) => return Err(exists()),
            Some(libc::ENOSYS | libc::EINVAL) => {}
            _ => return Err(io_diag(Code::Io, to, e)),
        }
    }
    // Filesystems/platforms without an atomic no-replace rename: best-effort check, then rename.
    // `fs::rename` cannot replace a non-empty directory.
    if fs::symlink_metadata(to).is_ok() {
        return Err(exists());
    }
    fs::rename(from, to).map_err(|e| io_diag(Code::Io, to, e))
}

// ------------------------------------------------------------------------------------ authoring

/// Authoring helper: recompute every size/digest in `preset.json` from the files beside it.
/// Run through `cargo run -p studio-presets --example seal -- <preset-dir>`.
pub fn seal_dir(dir: &Path) -> Result<(), PresetError> {
    let files = read_dir_files(dir)?;
    let manifest_path = ProjectPath::try_from(MANIFEST_FILE.to_owned()).expect("valid");
    let raw = files.get(&manifest_path).ok_or_else(|| {
        PresetError::one(
            Code::MissingResource,
            MANIFEST_FILE,
            "preset.json is required",
        )
    })?;
    let mut manifest: Manifest = serde_json::from_slice(raw).map_err(|e| {
        PresetError::one(
            Code::Malformed,
            MANIFEST_FILE,
            format!("invalid manifest: {e}"),
        )
    })?;
    for file in manifest.file_refs_mut() {
        let bytes = files.get(&file.path).ok_or_else(|| {
            PresetError::one(
                Code::MissingResource,
                file.path.as_str(),
                "declared file is missing",
            )
        })?;
        file.sha256 = sha256_hex(bytes);
        file.bytes = bytes.len() as u64;
    }
    let mut text = serde_json::to_string_pretty(&manifest).expect("manifest serializes");
    text.push('\n');
    let target = dir.join(MANIFEST_FILE);
    fs::write(&target, text).map_err(|e| io_diag(Code::Io, &target, e))
}
