#![allow(dead_code)]
use std::collections::BTreeMap;

use serde_json::Value;
use studio_presets::{Code, Package, PresetError, builtin};
use studio_project::ProjectPath;

pub type Files = BTreeMap<ProjectPath, Vec<u8>>;

pub fn p(path: &str) -> ProjectPath {
    ProjectPath::try_from(path.to_owned()).unwrap()
}

pub fn base() -> Files {
    builtin::get("editorial").unwrap().files().clone()
}

fn sha(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    Sha256::digest(bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

fn fix(value: &mut Value, files: &Files) {
    match value {
        Value::Object(map) => {
            if let (Some(Value::String(path)), true, true) = (
                map.get("path"),
                map.contains_key("sha256"),
                map.contains_key("bytes"),
            ) && let Some(bytes) = files.get(&p(&path.clone()))
            {
                map.insert("sha256".into(), Value::String(sha(bytes)));
                map.insert("bytes".into(), Value::from(bytes.len() as u64));
            }
            for v in map.values_mut() {
                fix(v, files);
            }
        }
        Value::Array(items) => items.iter_mut().for_each(|v| fix(v, files)),
        _ => {}
    }
}

/// Edit the manifest JSON, then recompute every declared size/digest so only the edit matters.
pub fn edit_manifest(files: &mut Files, f: impl FnOnce(&mut Value)) {
    let mut manifest: Value = serde_json::from_slice(&files[&p("preset.json")]).unwrap();
    f(&mut manifest);
    fix(&mut manifest, files);
    files.insert(
        p("preset.json"),
        serde_json::to_vec_pretty(&manifest).unwrap(),
    );
}

/// Recompute digests after changing file contents.
pub fn reseal(files: &mut Files) {
    edit_manifest(files, |_| {});
}

pub fn expect_code(result: Result<Package, PresetError>, code: Code) -> PresetError {
    let err = result.expect_err("expected failure");
    assert!(err.has(code), "expected {code:?}, got {err}");
    err
}
