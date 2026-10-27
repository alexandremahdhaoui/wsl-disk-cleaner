use std::io::ErrorKind;
use std::path::PathBuf;
use std::process::Command;

use anyhow::{bail, Context};
use wdc_core::port::{CleanupSource, PortError};
use wdc_core::types::{CleanupItem, Discovery};

pub struct ToolCache {
    pub name: &'static str,
    pub locate: &'static [&'static str],
    pub clean: &'static [&'static str],
}

pub const TOOL_CACHES: &[ToolCache] = &[
    ToolCache {
        name: "Go build cache",
        locate: &["go", "env", "GOCACHE"],
        clean: &["go", "clean", "-cache", "-testcache"],
    },
    ToolCache {
        name: "Go module cache",
        locate: &["go", "env", "GOMODCACHE"],
        clean: &["go", "clean", "-modcache"],
    },
    ToolCache {
        name: "uv cache",
        locate: &["uv", "cache", "dir"],
        clean: &["uv", "cache", "clean"],
    },
    ToolCache {
        name: "pnpm store",
        locate: &["pnpm", "store", "path"],
        clean: &["pnpm", "store", "prune"],
    },
    ToolCache {
        name: "npm cache",
        locate: &["npm", "config", "get", "cache"],
        clean: &["npm", "cache", "clean", "--force"],
    },
];

pub struct ToolCacheAdapter {
    cache: &'static ToolCache,
}

impl ToolCacheAdapter {
    pub fn new(cache: &'static ToolCache) -> Self {
        Self { cache }
    }

    fn discover_items(&self) -> anyhow::Result<Discovery> {
        let locate = self.cache.locate.join(" ");
        let (bin, args) = self
            .cache
            .locate
            .split_first()
            .context("reading the locate command")?;
        let out = match Command::new(bin).args(args).output() {
            Err(e) if e.kind() == ErrorKind::NotFound => return Ok(Discovery::NotInstalled),
            other => other.with_context(|| format!("running {locate}"))?,
        };
        if !out.status.success() {
            bail!(
                "{locate} exited with {}: {}",
                out.status,
                String::from_utf8_lossy(&out.stderr).trim()
            );
        }
        let path = String::from_utf8_lossy(&out.stdout).trim().to_string();
        if path.is_empty() {
            bail!("{locate} printed no path");
        }
        let path = PathBuf::from(path);
        if !path.exists() {
            return Ok(Discovery::Found(vec![]));
        }
        Ok(Discovery::Found(vec![CleanupItem {
            source: self.cache.name.into(),
            label: self.cache.name.into(),
            path: Some(path),
            required_markers: vec![],
            bytes: None,
            commands: vec![self.cache.clean.iter().map(|s| s.to_string()).collect()],
        }]))
    }
}

impl CleanupSource for ToolCacheAdapter {
    fn name(&self) -> String {
        self.cache.name.into()
    }

    fn discover(&self) -> Result<Discovery, PortError> {
        Ok(self.discover_items()?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_tool_whose_binary_is_missing_reports_not_installed() {
        static MISSING: ToolCache = ToolCache {
            name: "missing",
            locate: &["wdc-binary-that-does-not-exist"],
            clean: &["true"],
        };
        let discovery = ToolCacheAdapter::new(&MISSING).discover().unwrap();
        assert_eq!(discovery, Discovery::NotInstalled);
    }
}
