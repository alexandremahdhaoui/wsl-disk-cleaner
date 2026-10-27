use std::collections::HashSet;
use std::fs;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{anyhow, bail, Context};
use wdc_core::port::{PathFacts, PortError, UsageSource};
use wdc_core::types::{SizeNode, Volume, VolumeUsage};

use crate::util::parse_df;

pub struct WslFsAdapter {
    root: PathBuf,
    min_bytes: u64,
    max_depth: usize,
}

impl WslFsAdapter {
    pub fn new(root: PathBuf, min_bytes: u64, max_depth: usize) -> Self {
        Self {
            root,
            min_bytes,
            max_depth,
        }
    }
}

impl UsageSource for WslFsAdapter {
    fn name(&self) -> String {
        "wsl".into()
    }

    fn usage(&self) -> Result<VolumeUsage, PortError> {
        let (capacity_bytes, used_bytes) = df(&self.root)?;
        let device = fs::symlink_metadata(&self.root)
            .with_context(|| format!("reading {}", self.root.display()))?
            .dev();
        let root = Walker::new(device, self.min_bytes, self.max_depth).walk(&self.root, 0);
        Ok(VolumeUsage {
            volume: Volume::Wsl,
            capacity_bytes,
            used_bytes,
            walked_bytes: root.bytes,
            tree: root.children,
            disk_images: vec![],
        })
    }
}

impl PathFacts for WslFsAdapter {
    fn canonical(&self, path: &Path) -> Result<PathBuf, PortError> {
        Ok(fs::canonicalize(path).with_context(|| format!("canonicalizing {}", path.display()))?)
    }

    fn device(&self, path: &Path) -> Result<u64, PortError> {
        Ok(fs::metadata(path)
            .with_context(|| format!("reading metadata of {}", path.display()))?
            .dev())
    }

    fn exists(&self, path: &Path) -> bool {
        path.exists()
    }

    fn tracked_files(&self, path: &Path) -> Result<usize, PortError> {
        let probe = git(path, &["rev-parse", "--is-inside-work-tree"])?;
        if !probe.status.success() {
            let stderr = String::from_utf8_lossy(&probe.stderr);
            if stderr.contains("not a git repository") {
                return Ok(0);
            }
            return Err(anyhow!("git rev-parse in {}: {}", path.display(), stderr.trim()).into());
        }
        if String::from_utf8_lossy(&probe.stdout).trim() != "true" {
            return Err(anyhow!("{} is inside a .git folder", path.display()).into());
        }
        let listing = git(path, &["ls-files", "-z"])?;
        if !listing.status.success() {
            let stderr = String::from_utf8_lossy(&listing.stderr);
            return Err(anyhow!("git ls-files in {}: {}", path.display(), stderr.trim()).into());
        }
        Ok(listing
            .stdout
            .split(|b| *b == 0)
            .filter(|f| !f.is_empty())
            .count())
    }

    fn size(&self, path: &Path) -> Result<u64, PortError> {
        let device = fs::symlink_metadata(path)
            .with_context(|| format!("reading {}", path.display()))?
            .dev();
        Ok(Walker::new(device, u64::MAX, 0).walk(path, 0).bytes)
    }

    fn root_used_bytes(&self) -> Result<u64, PortError> {
        Ok(df(&self.root)?.1)
    }
}

fn git(path: &Path, args: &[&str]) -> anyhow::Result<std::process::Output> {
    Command::new("git")
        .arg("-C")
        .arg(path)
        .args(args)
        .output()
        .with_context(|| format!("running git {} in {}", args.join(" "), path.display()))
}

fn df(path: &Path) -> anyhow::Result<(u64, u64)> {
    let out = Command::new("df")
        .args(["-B1", "--output=size,used"])
        .arg(path)
        .output()
        .context("running df")?;
    if !out.status.success() {
        bail!(
            "df {} exited with {}: {}",
            path.display(),
            out.status,
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    parse_df(&String::from_utf8_lossy(&out.stdout))
        .with_context(|| format!("reading df output for {}", path.display()))
}

struct Walker {
    device: u64,
    min_bytes: u64,
    max_depth: usize,
    seen_hard_links: HashSet<u64>,
}

impl Walker {
    fn new(device: u64, min_bytes: u64, max_depth: usize) -> Self {
        Self {
            device,
            min_bytes,
            max_depth,
            seen_hard_links: HashSet::new(),
        }
    }

    fn walk(&mut self, path: &Path, depth: usize) -> SizeNode {
        let mut node = SizeNode {
            name: path
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_else(|| path.display().to_string()),
            path: path.display().to_string(),
            bytes: 0,
            children: vec![],
        };
        let Ok(entries) = fs::read_dir(path) else {
            return node;
        };
        for entry in entries.flatten() {
            let Ok(meta) = entry.metadata() else {
                continue;
            };
            if meta.dev() != self.device {
                continue;
            }
            if !meta.is_dir() {
                node.bytes += self.file_bytes(&meta);
                continue;
            }
            let child = self.walk(&entry.path(), depth + 1);
            node.bytes += child.bytes;
            if child.bytes >= self.min_bytes && depth < self.max_depth {
                node.children.push(child);
            }
        }
        node.children
            .sort_by_key(|child| std::cmp::Reverse(child.bytes));
        node
    }

    fn file_bytes(&mut self, meta: &fs::Metadata) -> u64 {
        if meta.nlink() > 1 && !self.seen_hard_links.insert(meta.ino()) {
            return 0;
        }
        meta.blocks() * 512
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::adapter::test_dir::TestDir;

    #[test]
    fn walk_keeps_only_children_above_the_threshold_sorted_by_size() {
        let dir = TestDir::new("walk");
        dir.file("big/a.bin", 64 * 1024);
        dir.file("big/b.bin", 64 * 1024);
        dir.file("small/c.bin", 1);
        let device = fs::metadata(dir.path()).unwrap().dev();
        let node = Walker::new(device, 100 * 1024, 4).walk(dir.path(), 0);
        let names: Vec<_> = node.children.iter().map(|c| c.name.as_str()).collect();
        assert_eq!(names, vec!["big"]);
        assert!(node.bytes >= 128 * 1024);
    }

    #[test]
    fn a_hard_linked_file_counts_once_like_du() {
        let dir = TestDir::new("hardlink");
        dir.file("a/data.bin", 256 * 1024);
        fs::create_dir_all(dir.path().join("b")).unwrap();
        fs::hard_link(dir.path().join("a/data.bin"), dir.path().join("b/data.bin")).unwrap();
        let device = fs::metadata(dir.path()).unwrap().dev();
        let bytes = Walker::new(device, u64::MAX, 0).walk(dir.path(), 0).bytes;
        let one_copy = fs::metadata(dir.path().join("a/data.bin"))
            .unwrap()
            .blocks()
            * 512;
        assert_eq!(bytes, one_copy);
    }

    #[test]
    fn walk_stops_keeping_children_past_the_max_depth() {
        let dir = TestDir::new("depth");
        dir.file("a/b/c/d.bin", 64 * 1024);
        let device = fs::metadata(dir.path()).unwrap().dev();
        let node = Walker::new(device, 1, 2).walk(dir.path(), 0);
        assert_eq!(node.children.len(), 1);
        assert_eq!(node.children[0].children.len(), 1);
        assert!(node.children[0].children[0].children.is_empty());
        assert!(node.children[0].children[0].bytes >= 64 * 1024);
    }

    #[test]
    fn a_folder_outside_git_has_no_tracked_files() {
        let dir = TestDir::new("nogit");
        let adapter = WslFsAdapter::new(PathBuf::from("/"), 1, 1);
        assert_eq!(adapter.tracked_files(dir.path()).unwrap(), 0);
    }

    #[test]
    fn a_folder_with_a_committed_file_counts_its_tracked_files() {
        let dir = TestDir::new("git");
        dir.file("target/kept.txt", 10);
        let run = |args: &[&str]| {
            let status = Command::new("git")
                .arg("-C")
                .arg(dir.path())
                .args(args)
                .output()
                .unwrap()
                .status;
            assert!(status.success(), "git {args:?}");
        };
        run(&["init", "-q"]);
        run(&["add", "target/kept.txt"]);
        let adapter = WslFsAdapter::new(PathBuf::from("/"), 1, 1);
        assert_eq!(
            adapter.tracked_files(&dir.path().join("target")).unwrap(),
            1
        );
    }
}
