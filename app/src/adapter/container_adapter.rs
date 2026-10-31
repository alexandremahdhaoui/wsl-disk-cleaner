use std::io::ErrorKind;
use std::path::PathBuf;
use std::process::{Command, Stdio};

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
    pub before_images: &'static [&'static [&'static str]],
    pub prune_images: &'static [&'static str],
    pub after_images: &'static [&'static [&'static str]],
}

const KEEP_HOLDER_PREFIX: &str = "wdc-keep-";
const HOLDER_COMMAND: &str = "true";

pub const CONTAINER_ENGINES: &[ContainerEngine] = &[
    ContainerEngine {
        bin: "podman",
        name: "Podman",
        what: "stopped containers, unused volumes and unused images",
        size: EngineSize::ReclaimableFromSystemDf,
        before_images: &[&["container", "prune", "-f"], &["volume", "prune", "-f"]],
        prune_images: &["image", "prune", "-a", "-f"],
        after_images: &[],
    },
    ContainerEngine {
        bin: "docker",
        name: "Docker",
        what: "stopped containers, unused volumes, unused images and build cache",
        size: EngineSize::ReclaimableFromSystemDf,
        before_images: &[
            &["container", "prune", "-f"],
            &["volume", "prune", "-a", "-f"],
        ],
        prune_images: &["image", "prune", "-a", "-f"],
        after_images: &[&["builder", "prune", "-a", "-f"]],
    },
    ContainerEngine {
        bin: "nerdctl",
        name: "nerdctl",
        what: "stopped containers, unused volumes, unused images and build cache",
        size: EngineSize::RootlessDataDir(".local/share/containerd"),
        before_images: &[
            &["container", "prune", "-f"],
            &["volume", "prune", "-a", "-f"],
        ],
        prune_images: &["image", "prune", "-a", "-f"],
        after_images: &[&["builder", "prune", "-a", "-f"]],
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
    keep_images: Vec<String>,
}

enum Probe {
    Missing,
    Output(String),
}

impl ContainerAdapter {
    pub fn new(engine: &'static ContainerEngine, home: PathBuf, keep_images: Vec<String>) -> Self {
        Self {
            engine,
            home,
            keep_images,
        }
    }

    fn has_image(&self, image: &str) -> bool {
        Command::new(self.engine.bin)
            .args(["image", "inspect", image])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .is_ok_and(|status| status.success())
    }

    fn present_keep_images(&self) -> Vec<String> {
        self.keep_images
            .iter()
            .filter(|image| self.has_image(image))
            .cloned()
            .collect()
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
        let kept = self.present_keep_images();
        Ok(Discovery::Found(vec![item_for(
            self.engine,
            path,
            bytes,
            &kept,
        )]))
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

fn engine_command(engine: &ContainerEngine, args: &[&str]) -> Vec<String> {
    std::iter::once(engine.bin)
        .chain(args.iter().copied())
        .map(String::from)
        .collect()
}

fn holder_name(index: usize) -> String {
    format!("{KEEP_HOLDER_PREFIX}{index}")
}

fn engine_commands(engine: &ContainerEngine, kept: &[String]) -> Vec<Vec<String>> {
    let before = engine
        .before_images
        .iter()
        .map(|args| engine_command(engine, args));
    let create_holders = kept.iter().enumerate().map(|(index, image)| {
        let name = holder_name(index);
        engine_command(
            engine,
            &[
                "create",
                "--pull=never",
                "--name",
                &name,
                image,
                HOLDER_COMMAND,
            ],
        )
    });
    let prune = std::iter::once(engine_command(engine, engine.prune_images));
    let remove_holders =
        (0..kept.len()).map(|index| engine_command(engine, &["rm", "-v", &holder_name(index)]));
    let after = engine
        .after_images
        .iter()
        .map(|args| engine_command(engine, args));
    before
        .chain(create_holders)
        .chain(prune)
        .chain(remove_holders)
        .chain(after)
        .collect()
}

fn item_for(
    engine: &ContainerEngine,
    path: Option<PathBuf>,
    bytes: Option<u64>,
    kept: &[String],
) -> CleanupItem {
    let label = if kept.is_empty() {
        engine.what.to_string()
    } else {
        format!("{}, keeping {}", engine.what, kept.join(", "))
    };
    CleanupItem {
        source: engine.name.into(),
        label,
        path,
        required_markers: vec![],
        bytes,
        commands: engine_commands(engine, kept),
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
        let item = item_for(&CONTAINER_ENGINES[0], None, Some(1), &[]);
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
    fn a_kept_image_holds_a_container_only_while_images_are_pruned() {
        let kept = vec!["localhost/lab:latest".to_string()];
        let item = item_for(&CONTAINER_ENGINES[0], None, Some(1), &kept);
        let commands: Vec<_> = item.commands.iter().map(|c| c.join(" ")).collect();
        assert_eq!(
            commands,
            vec![
                "podman container prune -f",
                "podman volume prune -f",
                "podman create --pull=never --name wdc-keep-0 localhost/lab:latest true",
                "podman image prune -a -f",
                "podman rm -v wdc-keep-0"
            ]
        );
        assert!(item.label.ends_with("keeping localhost/lab:latest"));
    }

    #[test]
    fn docker_cleans_its_build_cache_after_the_kept_images_are_released() {
        let kept = vec!["lab".to_string()];
        let commands = engine_commands(&CONTAINER_ENGINES[1], &kept);
        assert_eq!(
            commands.last().unwrap().join(" "),
            "docker builder prune -a -f"
        );
        assert_eq!(
            commands[commands.len() - 2].join(" "),
            "docker rm -v wdc-keep-0"
        );
    }

    #[test]
    fn a_kept_image_that_the_engine_lacks_is_skipped() {
        let adapter = ContainerAdapter::new(
            &MISSING,
            PathBuf::from("/"),
            vec!["localhost/lab:latest".to_string()],
        );
        assert!(adapter.present_keep_images().is_empty());
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

    static MISSING: ContainerEngine = ContainerEngine {
        bin: "wdc-engine-that-does-not-exist",
        name: "Missing",
        what: "nothing",
        size: EngineSize::ReclaimableFromSystemDf,
        before_images: &[],
        prune_images: &[],
        after_images: &[],
    };

    #[test]
    fn a_missing_engine_reports_not_installed() {
        let adapter = ContainerAdapter::new(&MISSING, PathBuf::from("/"), vec![]);
        assert_eq!(adapter.discover().unwrap(), Discovery::NotInstalled);
    }
}
