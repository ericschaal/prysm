//! `cargo xtask release-artifacts`: the CI artifacts of a release, checked
//! against their manifests. `packaging/release/publish.sh` runs it on the
//! downloaded artifacts before it publishes anything.
use crate::manifest::{Manifest, sha256};
use anyhow::{Context, Result};
use std::fmt;
use std::fs;
use std::path::{Component, Path, PathBuf};

/// Why a file cannot be released.
#[derive(Debug, PartialEq, Eq)]
enum Problem {
    /// Not a plain file name: a path, `..` or empty.
    NotAFileName,
    /// `dir` already has a file of that name.
    Exists,
    /// Another artifact has a file of the same name.
    InTwoArtifacts,
    /// Built as another version.
    Version(String),
    /// Built from another commit.
    Commit(String),
    /// Built from a tree with uncommitted changes.
    Dirty,
    /// The manifest records it, but its directory does not have it.
    Missing(PathBuf),
    /// Its SHA-256 is not the recorded one.
    Changed,
}

/// A file that cannot be released, and why.
#[derive(Debug, PartialEq, Eq)]
struct Refusal {
    name: String,
    problem: Problem,
}

impl fmt::Display for Refusal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} ", self.name)?;
        match &self.problem {
            Problem::NotAFileName => f.write_str("is not a plain file name"),
            Problem::Exists => f.write_str("is already in the release directory"),
            Problem::InTwoArtifacts => f.write_str("is in two artifacts"),
            Problem::Version(version) => write!(f, "is version {version}"),
            Problem::Commit(commit) => write!(f, "is from {commit}"),
            Problem::Dirty => f.write_str("was built from a tree with uncommitted changes"),
            Problem::Missing(dir) => write!(f, "is missing from {}", dir.display()),
            Problem::Changed => f.write_str("does not match its SHA-256"),
        }
    }
}

impl std::error::Error for Refusal {}

/// Checks every file that a `DIR/*/manifest.json` records (one directory per
/// CI artifact, each a `cargo xtask dist` output) against its entry: this
/// `version` and `commit`, a clean tree, the SHA-256, and a plain file name
/// that neither another artifact nor `dir` has. Only when all pass are the
/// files moved up into `dir`, so a refusal leaves the directory as it was.
pub fn run(dir: &Path, version: &str, commit: &str) -> Result<()> {
    for (from, name) in check(dir, version, commit)? {
        let to = dir.join(&name);
        fs::rename(&from, &to)
            .with_context(|| format!("moving {} to {}", from.display(), to.display()))?;
    }
    Ok(())
}

/// The files to move, as (path, name), after checking each.
fn check(dir: &Path, version: &str, commit: &str) -> Result<Vec<(PathBuf, String)>> {
    let mut manifests = Vec::new();
    for entry in fs::read_dir(dir).with_context(|| format!("reading {}", dir.display()))? {
        let manifest = entry?.path().join("manifest.json");
        if manifest.is_file() {
            manifests.push(manifest);
        }
    }
    manifests.sort();
    let mut files: Vec<(PathBuf, String)> = Vec::new();
    for path in manifests {
        let artifact = path.parent().expect("a manifest is in a directory");
        for (name, entry) in Manifest::read(&path)?.artifacts() {
            let file = artifact.join(name);
            let problem = if !is_file_name(name) {
                Some(Problem::NotAFileName)
            } else if fs::symlink_metadata(dir.join(name)).is_ok() {
                Some(Problem::Exists)
            } else if files.iter().any(|(_, seen)| seen == name) {
                Some(Problem::InTwoArtifacts)
            } else if entry.version != version {
                Some(Problem::Version(entry.version.clone()))
            } else if entry.commit != commit {
                Some(Problem::Commit(entry.commit.clone()))
            } else if entry.dirty {
                Some(Problem::Dirty)
            } else if !file.is_file() {
                Some(Problem::Missing(artifact.to_owned()))
            } else if sha256(&file)? != entry.sha256 {
                Some(Problem::Changed)
            } else {
                None
            };
            if let Some(problem) = problem {
                return Err(Refusal {
                    name: name.to_owned(),
                    problem,
                }
                .into());
            }
            files.push((file, name.to_owned()));
        }
    }
    Ok(files)
}

/// Whether `name` is one plain path component, so joining it stays in the
/// directory.
fn is_file_name(name: &str) -> bool {
    let mut components = Path::new(name).components();
    matches!(
        (components.next(), components.next()),
        (Some(Component::Normal(file)), None) if file == name
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::manifest::Entry;
    use crate::process::target_dir;

    const COMMIT: &str = "c0ffee";

    /// A fresh directory with one artifact directory per (name, files); each
    /// file holds its name and `edit` may change its entry.
    fn release(test: &str, layout: &[(&str, &[&str])], edit: impl Fn(&str, &mut Entry)) -> PathBuf {
        let dir = target_dir().join("xtask/release-test").join(test);
        let _ = fs::remove_dir_all(&dir);
        for (artifact, names) in layout {
            let artifact = dir.join(artifact);
            fs::create_dir_all(&artifact).unwrap();
            let mut manifest = Manifest::default();
            for &name in *names {
                fs::write(artifact.join(name), name).unwrap();
                let mut entry = Entry {
                    version: "1.2.0".into(),
                    commit: COMMIT.into(),
                    dirty: false,
                    sha256: sha256(&artifact.join(name)).unwrap(),
                    built_at: "2026-11-17T12:00:00Z".into(),
                };
                edit(name, &mut entry);
                manifest.insert(name.to_owned(), entry);
            }
            manifest.write(&artifact.join("manifest.json")).unwrap();
        }
        dir
    }

    const LAYOUT: &[(&str, &[&str])] = &[
        ("prysm-dist-linux-amd64", &["a.deb", "a.tar"]),
        ("prysm-dist-tv", &["full.ipk", "slim.ipk"]),
    ];

    fn refusal(test: &str, layout: &[(&str, &[&str])], edit: impl Fn(&str, &mut Entry)) -> Refusal {
        let dir = release(test, layout, edit);
        let error = run(&dir, "1.2.0", COMMIT).unwrap_err();
        let refusal = error.downcast::<Refusal>().unwrap();
        assert!(!dir.join("a.deb").exists(), "a refusal moves nothing");
        fs::remove_dir_all(dir).unwrap();
        refusal
    }

    fn only(name: &'static str, change: impl Fn(&mut Entry)) -> impl Fn(&str, &mut Entry) {
        move |file, entry| {
            if file == name {
                change(entry);
            }
        }
    }

    #[test]
    fn checked_files_move_up() {
        let dir = release("moved", LAYOUT, |_, _| {});
        fs::write(dir.join("release.json"), "{}").unwrap();
        run(&dir, "1.2.0", COMMIT).unwrap();
        for name in ["a.deb", "a.tar", "full.ipk", "slim.ipk"] {
            assert_eq!(fs::read_to_string(dir.join(name)).unwrap(), name);
        }
        assert!(!dir.join("prysm-dist-tv/slim.ipk").exists());
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn a_file_in_two_artifacts_is_refused() {
        let layout: &[(&str, &[&str])] = &[("one", &["a.deb"]), ("two", &["a.deb"])];
        let refused = refusal("twice", layout, |_, _| {});
        assert_eq!(refused.problem, Problem::InTwoArtifacts);
        assert_eq!(refused.to_string(), "a.deb is in two artifacts");
    }

    #[test]
    fn another_version_is_refused() {
        let edit = only("slim.ipk", |entry| entry.version = "1.1.0".into());
        let refused = refusal("version", LAYOUT, edit);
        assert_eq!(
            refused,
            Refusal {
                name: "slim.ipk".into(),
                problem: Problem::Version("1.1.0".into())
            }
        );
    }

    #[test]
    fn another_commit_is_refused() {
        let edit = only("a.tar", |entry| entry.commit = "other".into());
        let refused = refusal("commit", LAYOUT, edit);
        assert_eq!(refused.problem, Problem::Commit("other".into()));
        assert_eq!(refused.to_string(), "a.tar is from other");
    }

    #[test]
    fn a_dirty_tree_is_refused() {
        let refused = refusal(
            "dirty",
            LAYOUT,
            only("full.ipk", |entry| entry.dirty = true),
        );
        assert_eq!(refused.problem, Problem::Dirty);
    }

    #[test]
    fn a_missing_file_is_refused() {
        let dir = release("missing", LAYOUT, |_, _| {});
        fs::remove_file(dir.join("prysm-dist-tv/full.ipk")).unwrap();
        let error = run(&dir, "1.2.0", COMMIT).unwrap_err();
        assert_eq!(
            error.downcast::<Refusal>().unwrap().problem,
            Problem::Missing(dir.join("prysm-dist-tv"))
        );
        assert!(!dir.join("a.deb").exists(), "a refusal moves nothing");
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn a_changed_file_is_refused() {
        let edit = only("a.deb", |entry| entry.sha256 = "0".repeat(64));
        assert_eq!(refusal("changed", LAYOUT, edit).problem, Problem::Changed);
    }

    #[test]
    fn a_name_that_is_not_a_plain_file_name_is_refused() {
        for name in ["../a.deb", "sub/a.deb", "/tmp/a.deb", "a.deb/", ".", ""] {
            let dir = release("not-a-file-name", &[("one", &["b.deb"])], |_, _| {});
            let manifest_path = dir.join("one/manifest.json");
            let mut manifest = Manifest::read(&manifest_path).unwrap();
            let entry = manifest.artifacts().next().unwrap().1.clone();
            manifest.insert(name.to_owned(), entry);
            manifest.write(&manifest_path).unwrap();
            let error = run(&dir, "1.2.0", COMMIT).unwrap_err();
            let refused = error.downcast::<Refusal>().unwrap();
            assert_eq!(refused.problem, Problem::NotAFileName, "{name:?}");
            assert!(!dir.join("b.deb").exists(), "a refusal moves nothing");
            fs::remove_dir_all(dir).unwrap();
        }
    }

    #[test]
    fn a_file_already_in_the_release_directory_is_refused() {
        let dir = release("exists", LAYOUT, |_, _| {});
        fs::write(dir.join("slim.ipk"), "earlier").unwrap();
        let error = run(&dir, "1.2.0", COMMIT).unwrap_err();
        let refused = error.downcast::<Refusal>().unwrap();
        assert_eq!(
            refused.to_string(),
            "slim.ipk is already in the release directory"
        );
        assert!(!dir.join("a.deb").exists(), "a refusal moves nothing");
        assert_eq!(fs::read_to_string(dir.join("slim.ipk")).unwrap(), "earlier");
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn a_broken_manifest_fails() {
        let dir = release("broken", LAYOUT, |_, _| {});
        fs::write(dir.join("prysm-dist-tv/manifest.json"), "{").unwrap();
        let error = run(&dir, "1.2.0", COMMIT).unwrap_err();
        assert!(error.downcast_ref::<Refusal>().is_none());
        assert!(format!("{error:#}").contains("parsing"), "{error:#}");
        fs::remove_dir_all(dir).unwrap();
    }
}
