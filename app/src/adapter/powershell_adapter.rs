use std::collections::HashMap;
use std::process::{Command, Stdio};

use anyhow::{bail, Context, Result};
use serde::Deserialize;
use wdc_core::port::{PortError, UsageSource};
use wdc_core::types::{DiskImage, SizeNode, Volume, VolumeUsage};

use crate::util::powershell_encoded_command;

const SCRIPT: &str = include_str!("windows_scan.ps1");
const DRIVE_ROOT: &str = r"C:\";

#[derive(Deserialize)]
struct WireScan {
    capacity: u64,
    used: u64,
    walked: u64,
    #[serde(default)]
    dirs: Vec<WireEntry>,
    #[serde(default)]
    images: Vec<WireEntry>,
}

#[derive(Deserialize, Clone)]
struct WireEntry {
    path: String,
    bytes: u64,
}

pub struct PowershellAdapter {
    min_bytes: u64,
    max_depth: usize,
}

impl PowershellAdapter {
    pub fn new(min_bytes: u64, max_depth: usize) -> Self {
        Self {
            min_bytes,
            max_depth,
        }
    }
}

impl UsageSource for PowershellAdapter {
    fn name(&self) -> String {
        "windows".into()
    }

    fn usage(&self) -> Result<VolumeUsage, PortError> {
        Ok(self.scan()?)
    }
}

impl PowershellAdapter {
    fn scan(&self) -> Result<VolumeUsage> {
        let script = SCRIPT
            .replace("__MIN_BYTES__", &self.min_bytes.to_string())
            .replace("__MAX_DEPTH__", &self.max_depth.to_string());
        let out = Command::new("powershell.exe")
            .args(["-NoProfile", "-NonInteractive", "-EncodedCommand"])
            .arg(powershell_encoded_command(&script))
            .stdin(Stdio::null())
            .output()
            .context("running powershell.exe")?;
        if !out.status.success() {
            bail!(
                "powershell.exe exited with {}: {}",
                out.status,
                String::from_utf8_lossy(&out.stderr).trim()
            );
        }
        parse_scan(&String::from_utf8_lossy(&out.stdout))
    }
}

fn parse_scan(output: &str) -> Result<VolumeUsage> {
    let json = output
        .lines()
        .rev()
        .find(|line| line.trim_start().starts_with('{'))
        .context("reading the powershell scan output: no JSON line")?;
    let wire: WireScan = serde_json::from_str(json).context("parsing the powershell scan JSON")?;
    Ok(VolumeUsage {
        volume: Volume::Windows,
        capacity_bytes: wire.capacity,
        used_bytes: wire.used,
        walked_bytes: wire.walked,
        tree: tree_from_dirs(DRIVE_ROOT, &wire.dirs),
        disk_images: wire
            .images
            .into_iter()
            .map(|image| DiskImage {
                path: image.path,
                bytes: image.bytes,
            })
            .collect(),
    })
}

fn parent_of(path: &str) -> String {
    let Some(split) = path.trim_end_matches('\\').rfind('\\') else {
        return String::new();
    };
    let parent = &path[..split];
    if parent.ends_with(':') {
        return format!("{parent}\\");
    }
    parent.to_string()
}

fn tree_from_dirs(root: &str, dirs: &[WireEntry]) -> Vec<SizeNode> {
    let mut by_parent: HashMap<String, Vec<WireEntry>> = HashMap::new();
    for dir in dirs {
        by_parent
            .entry(parent_of(&dir.path).to_lowercase())
            .or_default()
            .push(dir.clone());
    }
    build_children(root, &by_parent)
}

fn build_children(parent: &str, by_parent: &HashMap<String, Vec<WireEntry>>) -> Vec<SizeNode> {
    let mut children: Vec<SizeNode> = by_parent
        .get(&parent.to_lowercase())
        .into_iter()
        .flatten()
        .map(|entry| SizeNode {
            name: entry
                .path
                .rsplit('\\')
                .next()
                .unwrap_or_default()
                .to_string(),
            path: entry.path.clone(),
            bytes: entry.bytes,
            children: build_children(&entry.path, by_parent),
        })
        .collect();
    children.sort_by_key(|child| std::cmp::Reverse(child.bytes));
    children
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_flat_dir_list_becomes_a_tree_sorted_by_size() {
        let out = r#"{"capacity":952,"used":847,"walked":840,"dirs":[{"path":"C:\\Users","bytes":224},{"path":"C:\\Program Files (x86)\\Steam","bytes":451},{"path":"C:\\Program Files (x86)","bytes":465}],"images":[{"path":"C:\\Users\\a\\wsl\\ext4.vhdx","bytes":199}]}"#;
        let usage = parse_scan(out).unwrap();
        let top: Vec<_> = usage
            .tree
            .iter()
            .map(|n| (n.name.as_str(), n.bytes))
            .collect();
        assert_eq!(top, vec![("Program Files (x86)", 465), ("Users", 224)]);
        assert_eq!(usage.tree[0].children[0].name, "Steam");
        assert_eq!(usage.disk_images[0].bytes, 199);
        assert_eq!(usage.walked_bytes, 840);
    }

    #[test]
    fn the_parent_of_a_top_level_folder_is_the_drive_root() {
        assert_eq!(parent_of(r"C:\Users"), r"C:\");
        assert_eq!(parent_of(r"C:\Users\alexa"), r"C:\Users");
    }

    #[test]
    fn output_without_json_is_an_error() {
        assert!(parse_scan("WARNING: something\n").is_err());
    }

    #[test]
    fn the_script_carries_no_unreplaced_placeholder_after_rendering() {
        let script = SCRIPT
            .replace("__MIN_BYTES__", "1")
            .replace("__MAX_DEPTH__", "2");
        assert!(!script.contains("__"));
    }
}
