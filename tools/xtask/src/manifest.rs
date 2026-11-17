//! `dist/manifest.json`: what each artifact in `dist/` was built from.
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fs;
use std::io::ErrorKind;
use std::path::Path;

/// The manifest: one entry per artifact, keyed and sorted by file name.
#[derive(Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Manifest {
    artifacts: BTreeMap<String, Entry>,
}

/// One artifact.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Entry {
    /// The product version (`VERSION`).
    pub version: String,
    /// The commit the artifact was built from.
    pub commit: String,
    /// Whether the working tree had uncommitted changes.
    pub dirty: bool,
    /// Lower-case hex SHA-256 of the file.
    pub sha256: String,
    /// When it was built, RFC 3339 in UTC.
    pub built_at: String,
}

impl Manifest {
    /// Reads `path`; a missing file is an empty manifest.
    pub fn read(path: &Path) -> Result<Self> {
        let text = match fs::read_to_string(path) {
            Ok(text) => text,
            Err(error) if error.kind() == ErrorKind::NotFound => return Ok(Self::default()),
            Err(error) => return Err(error).with_context(|| format!("reading {}", path.display())),
        };
        serde_json::from_str(&text).with_context(|| format!("parsing {}", path.display()))
    }

    /// Records `file`, replacing an earlier entry for it and keeping the rest.
    pub fn insert(&mut self, file: String, entry: Entry) {
        self.artifacts.insert(file, entry);
    }

    /// Keeps only the entries whose file `keep` accepts.
    pub fn retain(&mut self, mut keep: impl FnMut(&str) -> bool) {
        self.artifacts.retain(|file, _| keep(file));
    }

    /// Pretty-printed JSON with a final newline.
    pub fn to_json(&self) -> String {
        let mut json = serde_json::to_string_pretty(self).expect("the manifest serialises");
        json.push('\n');
        json
    }

    /// Writes `path` through a temporary file, so a reader never sees half of it.
    pub fn write(&self, path: &Path) -> Result<()> {
        let temporary = path.with_extension("json.tmp");
        fs::write(&temporary, self.to_json())
            .and_then(|()| fs::rename(&temporary, path))
            .with_context(|| format!("writing {}", path.display()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(commit: &str) -> Entry {
        Entry {
            version: "1.0.0-alpha.0".into(),
            commit: commit.into(),
            dirty: false,
            sha256: "ab".repeat(32),
            built_at: "2026-11-16T12:00:00Z".into(),
        }
    }

    #[test]
    fn inserting_replaces_one_entry_and_keeps_the_others() {
        let mut manifest = Manifest::default();
        manifest.insert("b".into(), entry("old"));
        manifest.insert("a".into(), entry("kept"));
        manifest.insert("b".into(), entry("new"));
        assert_eq!(manifest.artifacts.len(), 2);
        assert_eq!(manifest.artifacts["a"].commit, "kept");
        assert_eq!(manifest.artifacts["b"].commit, "new");
    }

    #[test]
    fn retaining_drops_the_other_entries() {
        let mut manifest = Manifest::default();
        manifest.insert("gone".into(), entry("a"));
        manifest.insert("kept".into(), entry("b"));
        manifest.retain(|file| file == "kept");
        assert_eq!(manifest.artifacts.keys().collect::<Vec<_>>(), ["kept"]);
    }

    #[test]
    fn json_is_sorted_by_file_and_round_trips() {
        let mut manifest = Manifest::default();
        manifest.insert("prysm-headless-linux-arm64".into(), entry("c1"));
        manifest.insert("prysm-headless-linux-amd64".into(), entry("c2"));
        let json = manifest.to_json();
        assert!(json.ends_with("}\n"));
        assert!(json.find("linux-amd64") < json.find("linux-arm64"));
        let fields = ["version", "commit", "dirty", "sha256", "built_at"].map(|f| json.find(f));
        assert!(fields.is_sorted(), "fields keep their declared order");
        assert_eq!(serde_json::from_str::<Manifest>(&json).unwrap(), manifest);
    }

    #[test]
    fn unknown_fields_are_refused() {
        let json = r#"{"artifacts": {}, "extra": 1}"#;
        assert!(serde_json::from_str::<Manifest>(json).is_err());
    }
}
