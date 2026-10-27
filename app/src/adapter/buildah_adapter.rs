use std::io::ErrorKind;
use std::process::Command;

use anyhow::{bail, Context, Result};
use wdc_core::port::{CleanupSource, PortError};
use wdc_core::types::{CleanupItem, Discovery};

const SOURCE: &str = "Buildah";

pub struct BuildahAdapter;

impl BuildahAdapter {
    fn discover_items(&self) -> Result<Discovery> {
        let out = match Command::new("buildah")
            .args(["containers", "--json"])
            .output()
        {
            Err(e) if e.kind() == ErrorKind::NotFound => return Ok(Discovery::NotInstalled),
            other => other.context("running buildah containers --json")?,
        };
        if !out.status.success() {
            bail!(
                "buildah containers --json exited with {}: {}",
                out.status,
                String::from_utf8_lossy(&out.stderr).trim()
            );
        }
        let count = count_containers(&String::from_utf8_lossy(&out.stdout))?;
        if count == 0 {
            return Ok(Discovery::Found(vec![]));
        }
        Ok(Discovery::Found(vec![CleanupItem {
            source: SOURCE.into(),
            label: format!("{count} leftover build containers"),
            path: None,
            required_markers: vec![],
            bytes: None,
            commands: vec![vec!["buildah".into(), "rm".into(), "--all".into()]],
        }]))
    }
}

impl CleanupSource for BuildahAdapter {
    fn name(&self) -> String {
        SOURCE.into()
    }

    fn discover(&self) -> Result<Discovery, PortError> {
        Ok(self.discover_items()?)
    }
}

fn count_containers(json: &str) -> Result<usize> {
    let containers: Option<Vec<serde_json::Value>> =
        serde_json::from_str(json.trim()).context("parsing buildah containers JSON")?;
    Ok(containers.map_or(0, |list| list.len()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn buildah_prints_null_when_it_has_no_containers() {
        assert_eq!(count_containers("null\n").unwrap(), 0);
    }

    #[test]
    fn each_listed_container_counts_once() {
        let json = r#"[{"id":"a","containername":"x-working-container"},{"id":"b"}]"#;
        assert_eq!(count_containers(json).unwrap(), 2);
    }
}
