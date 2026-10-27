use std::fs;
use std::io::ErrorKind;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::Context;
use wdc_core::port::{CleanupSource, PortError};
use wdc_core::types::{CleanupItem, Discovery};

const CACHEDIR_TAG: &str = "CACHEDIR.TAG";
const SOURCE: &str = "Cargo";

pub struct CargoTargetAdapter {
    search_root: PathBuf,
}

impl CargoTargetAdapter {
    pub fn new(search_root: PathBuf) -> Self {
        Self { search_root }
    }
}

impl CleanupSource for CargoTargetAdapter {
    fn name(&self) -> String {
        SOURCE.into()
    }

    fn discover(&self) -> Result<Discovery, PortError> {
        match Command::new("cargo").arg("--version").output() {
            Err(e) if e.kind() == ErrorKind::NotFound => return Ok(Discovery::NotInstalled),
            other => other.context("running cargo --version")?,
        };
        let device = fs::metadata(&self.search_root)
            .with_context(|| format!("reading {}", self.search_root.display()))?
            .dev();
        let mut targets = Vec::new();
        find_targets(&self.search_root, device, &mut targets);
        Ok(Discovery::Found(
            targets.into_iter().map(item_for).collect(),
        ))
    }
}

fn find_targets(dir: &Path, device: u64, found: &mut Vec<PathBuf>) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let Ok(file_type) = entry.file_type() else {
            continue;
        };
        if !file_type.is_dir() || entry.file_name() == ".git" {
            continue;
        }
        let Ok(meta) = entry.metadata() else {
            continue;
        };
        if meta.dev() != device {
            continue;
        }
        let path = entry.path();
        if entry.file_name() == "target" && path.join(CACHEDIR_TAG).is_file() {
            found.push(path);
            continue;
        }
        find_targets(&path, device, found);
    }
}

fn item_for(target: PathBuf) -> CleanupItem {
    let manifest = target
        .parent()
        .map(|parent| parent.join("Cargo.toml"))
        .unwrap_or_default();
    CleanupItem {
        source: SOURCE.into(),
        label: "build output".into(),
        required_markers: vec![target.join(CACHEDIR_TAG), manifest.clone()],
        bytes: None,
        commands: vec![vec![
            "cargo".into(),
            "clean".into(),
            "--manifest-path".into(),
            manifest.display().to_string(),
            "--target-dir".into(),
            target.display().to_string(),
        ]],
        path: Some(target),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::adapter::test_dir::TestDir;

    #[test]
    fn discovery_finds_tagged_target_folders_and_skips_git_and_nested_targets() {
        let dir = TestDir::new("cargo");
        dir.file("a/target/CACHEDIR.TAG", 1);
        dir.file("a/target/debug/target/CACHEDIR.TAG", 1);
        dir.file("b/target/untagged.txt", 1);
        dir.file(".git/target/CACHEDIR.TAG", 1);
        dir.file("c/d/target/CACHEDIR.TAG", 1);
        let device = fs::metadata(dir.path()).unwrap().dev();
        let mut found = Vec::new();
        find_targets(dir.path(), device, &mut found);
        found.sort();
        assert_eq!(
            found,
            vec![dir.path().join("a/target"), dir.path().join("c/d/target")]
        );
    }

    #[test]
    fn an_item_cleans_its_own_target_dir_through_cargo_and_requires_tag_and_manifest() {
        let item = item_for(PathBuf::from("/w/songe/target"));
        assert_eq!(
            item.commands,
            vec![vec![
                "cargo",
                "clean",
                "--manifest-path",
                "/w/songe/Cargo.toml",
                "--target-dir",
                "/w/songe/target"
            ]]
        );
        assert_eq!(
            item.required_markers,
            vec![
                PathBuf::from("/w/songe/target/CACHEDIR.TAG"),
                PathBuf::from("/w/songe/Cargo.toml")
            ]
        );
    }
}
