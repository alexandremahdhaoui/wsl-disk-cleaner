use std::io::ErrorKind;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::Deserialize;

const CONFIG_RELATIVE_PATH: &str = ".config/wsl-disk-cleaner/config.json";

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct WireConfig {
    #[serde(default)]
    keep_images: Vec<String>,
}

pub fn config_path(home: &Path) -> PathBuf {
    home.join(CONFIG_RELATIVE_PATH)
}

pub fn load_keep_images(path: &Path) -> Result<Vec<String>> {
    let text = match std::fs::read_to_string(path) {
        Ok(text) => text,
        Err(error) if error.kind() == ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => {
            return Err(error).with_context(|| format!("reading {}", path.display()));
        }
    };
    let config: WireConfig =
        serde_json::from_str(&text).with_context(|| format!("parsing {}", path.display()))?;
    Ok(config.keep_images)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::adapter::test_dir::TestDir;

    #[test]
    fn a_missing_config_keeps_nothing() {
        let dir = TestDir::new("keep-missing");
        assert!(load_keep_images(&dir.path().join("none.json"))
            .unwrap()
            .is_empty());
    }

    #[test]
    fn the_config_lists_the_images_to_keep() {
        let dir = TestDir::new("keep-list");
        let path = dir.path().join("config.json");
        std::fs::write(&path, r#"{"keep_images":["localhost/lab:latest"]}"#).unwrap();
        assert_eq!(
            load_keep_images(&path).unwrap(),
            vec!["localhost/lab:latest"]
        );
    }

    #[test]
    fn a_broken_config_is_refused_by_name() {
        let dir = TestDir::new("keep-broken");
        let path = dir.path().join("config.json");
        std::fs::write(&path, "not json").unwrap();
        let error = format!("{:#}", load_keep_images(&path).unwrap_err());
        assert!(error.contains("config.json"));
    }

    #[test]
    fn a_misspelled_key_is_refused_so_a_kept_image_is_never_lost_quietly() {
        let dir = TestDir::new("keep-typo");
        let path = dir.path().join("config.json");
        std::fs::write(&path, r#"{"keep_image":["lab"]}"#).unwrap();
        assert!(load_keep_images(&path).is_err());
    }
}
