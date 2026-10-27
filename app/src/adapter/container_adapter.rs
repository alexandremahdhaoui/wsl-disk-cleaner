use std::io::ErrorKind;
use std::path::PathBuf;
use std::process::Command;

use anyhow::{bail, Context, Result};
use serde::Deserialize;
use wdc_core::port::{CleanupSource, PortError};
use wdc_core::types::{CleanupItem, Discovery};

use crate::util::parse_human_size;

pub enum EngineSize {
    ReclaimableFromSystemDf,
    RootlessDataDir(&'static str),
}

pub struct ContainerEngine {
    pub bin: &'static str,
    pub name: &'static str,
    pub what: &'static str,
    pub size: EngineSize,
    pub commands: &'static [&'static [&'static str]],
}

pub const CONTAINER_ENGINES: &[ContainerEngine] = &[
    ContainerEngine {
        bin: "podman",
        name: "Podman",
        what: "stopped containers, unused volumes and unused images",
        size: EngineSize::ReclaimableFromSystemDf,
        commands: &[
            &["container", "prune", "-f"],
            &["volume", "prune", "-f"],
            &["image", "prune", "-a", "-f"],
        ],
    },
    ContainerEngine {
        bin: "docker",
        name: "Docker",
        what: "stopped containers, unused volumes, unused images and build cache",
        size: EngineSize::ReclaimableFromSystemDf,
        commands: &[
            &["container", "prune", "-f"],
            &["volume", "prune", "-a", "-f"],
            &["image", "prune", "-a", "-f"],
            &["builder", "prune", "-a", "-f"],
        ],
    },
    ContainerEngine {
        bin: "nerdctl",
        name: "nerdctl",
        what: "stopped containers, unused volumes, unused images and build cache",
        size: EngineSize::RootlessDataDir(".local/share/containerd"),
        commands: &[&["system", "prune", "--all", "--force", "--volumes"]],
    },
];

#[derive(Deserialize)]
struct WireDfRow {
    #[serde(rename = "Reclaimable")]
    reclaimable: String,
}

pub struct ContainerAdapter {
    engine: &'static ContainerEngine,
    home: PathBuf,
}

enum Probe {
    Missing,
    Output(String),
}

impl ContainerAdapter {
    pub fn new(engine: &'static ContainerEngine, home: PathBuf) -> Self {
        Self { engine, home }
    }

    fn run(&self, args: &[&str]) -> Result<Probe> {
        let line = format!("{} {}", self.engine.bin, args.join(" "));
        let out = match Command::new(self.engine.bin).args(args).output() {
            Err(e) if e.kind() == ErrorKind::NotFound => return Ok(Probe::Missing),
            other => other.with_context(|| format!("running {line}"))?,
        };
        if !out.status.success() {
            bail!(
                "{line} exited with {}: {}",
                out.status,
                String::from_utf8_lossy(&out.stderr).trim()
            );
        }
        Ok(Probe::Output(format!(
            "{}{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        )))
    }

    fn discover_items(&self) -> Result<Discovery> {
        let Probe::Output(version) = self.run(&["--version"])? else {
            return Ok(Discovery::NotInstalled);
        };
        if emulated_engine(self.engine.bin, &version).is_some() {
            return Ok(Discovery::NotInstalled);
        }
        let (path, bytes) = match self.engine.size {
            EngineSize::ReclaimableFromSystemDf => {
                let Probe::Output(df) = self.run(&["system", "df", "--format", "{{json .}}"])?
                else {
                    return Ok(Discovery::NotInstalled);
                };
                (None, Some(total_reclaimable(&df)?))
            }
            EngineSize::RootlessDataDir(relative) => {
                let dir = self.home.join(relative);
                if !dir.is_dir() {
                    bail!(
                        "{} does not exist, and wdc only measures rootless containerd",
                        dir.display()
                    );
                }
                (Some(dir), None)
            }
        };
        if bytes == Some(0) {
            return Ok(Discovery::Found(vec![]));
        }
        Ok(Discovery::Found(vec![item_for(self.engine, path, bytes)]))
    }
}

impl CleanupSource for ContainerAdapter {
    fn name(&self) -> String {
        self.engine.name.into()
    }

    fn discover(&self) -> Result<Discovery, PortError> {
        Ok(self.discover_items()?)
    }
}

fn emulated_engine(bin: &str, version: &str) -> Option<&'static str> {
    let version = version.to_lowercase();
    CONTAINER_ENGINES
        .iter()
        .map(|engine| engine.bin)
        .find(|other| *other != bin && version.contains(other))
}

fn total_reclaimable(output: &str) -> Result<u64> {
    output
        .lines()
        .filter(|line| line.trim_start().starts_with('{'))
        .map(|line| {
            let row: WireDfRow = serde_json::from_str(line)
                .with_context(|| format!("parsing system df row {line:?}"))?;
            parse_human_size(&row.reclaimable)
        })
        .sum()
}

fn item_for(engine: &ContainerEngine, path: Option<PathBuf>, bytes: Option<u64>) -> CleanupItem {
    CleanupItem {
        source: engine.name.into(),
        label: engine.what.into(),
        path,
        required_markers: vec![],
        bytes,
        commands: engine
            .commands
            .iter()
            .map(|args| {
                std::iter::once(engine.bin)
                    .chain(args.iter().copied())
                    .map(String::from)
                    .collect()
            })
            .collect(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const PODMAN_DF: &str = r#"{"Type":"Images","Total":395,"Active":1,"RawSize":4186000000,"RawReclaimable":4095000000,"TotalCount":395,"Size":"4.186GB","Reclaimable":"4.095GB (98%)"}
{"Type":"Containers","Total":74,"Active":0,"RawSize":861700,"RawReclaimable":861700,"TotalCount":74,"Size":"861.7kB","Reclaimable":"861.7kB (100%)"}
{"Type":"Local Volumes","Total":377,"Active":0,"RawSize":82460000000,"RawReclaimable":82460000000,"TotalCount":377,"Size":"82.46GB","Reclaimable":"82.46GB (100%)"}
"#;

    #[test]
    fn an_engine_is_one_item_whose_size_sums_every_reclaimable_kind() {
        assert_eq!(total_reclaimable(PODMAN_DF).unwrap(), 86_555_861_700);
    }

    #[test]
    fn an_engine_item_prunes_containers_then_volumes_then_images() {
        let item = item_for(&CONTAINER_ENGINES[0], None, Some(1));
        let commands: Vec<_> = item.commands.iter().map(|c| c.join(" ")).collect();
        assert_eq!(
            commands,
            vec![
                "podman container prune -f",
                "podman volume prune -f",
                "podman image prune -a -f"
            ]
        );
        assert_eq!(item.source, "Podman");
    }

    #[test]
    fn a_docker_that_answers_as_podman_is_named_an_emulator() {
        assert_eq!(
            emulated_engine("docker", "podman version 5.7.0"),
            Some("podman")
        );
        assert_eq!(emulated_engine("docker", "Docker version 27.3.1"), None);
        assert_eq!(emulated_engine("podman", "podman version 5.7.0"), None);
    }

    #[test]
    fn a_noise_line_from_the_docker_shim_is_skipped() {
        let out = "Emulate Docker CLI using podman.\n{\"Type\":\"Images\",\"Reclaimable\":\"1GB (10%)\"}\n";
        assert_eq!(total_reclaimable(out).unwrap(), 1_000_000_000);
    }

    #[test]
    fn a_missing_engine_reports_not_installed() {
        static MISSING: ContainerEngine = ContainerEngine {
            bin: "wdc-engine-that-does-not-exist",
            name: "Missing",
            what: "nothing",
            size: EngineSize::ReclaimableFromSystemDf,
            commands: &[],
        };
        let adapter = ContainerAdapter::new(&MISSING, PathBuf::from("/"));
        assert_eq!(adapter.discover().unwrap(), Discovery::NotInstalled);
    }
}
