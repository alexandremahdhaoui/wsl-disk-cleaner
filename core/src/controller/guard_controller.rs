use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::port::{PathFacts, PortError};
use crate::types::{CleanupItem, Refusal, Rule};
use crate::util::error_chain;

pub struct GuardController {
    facts: Arc<dyn PathFacts>,
    protected_roots: Vec<PathBuf>,
}

impl GuardController {
    pub fn new(facts: Arc<dyn PathFacts>, protected_roots: Vec<PathBuf>) -> Self {
        Self {
            facts,
            protected_roots,
        }
    }

    pub fn check(&self, item: &CleanupItem) -> Option<Refusal> {
        let path = item.path.as_ref()?;
        self.check_path(path, &item.required_markers).err()
    }

    fn check_path(&self, path: &Path, markers: &[PathBuf]) -> Result<(), Refusal> {
        let canonical = self
            .facts
            .canonical(path)
            .map_err(|e| facts_unavailable("resolving", path, e))?;
        let root = Path::new("/");
        let root_device = self
            .facts
            .device(root)
            .map_err(|e| facts_unavailable("reading the device of", root, e))?;
        let device = self
            .facts
            .device(&canonical)
            .map_err(|e| facts_unavailable("reading the device of", &canonical, e))?;
        if device != root_device {
            return Err(refuse(
                Rule::NotWslRootFilesystem,
                format!("{} is not on the WSL root filesystem", canonical.display()),
            ));
        }
        if let Some(protected) = self.protected_root_overlapping(&canonical) {
            return Err(refuse(
                Rule::ProtectedPath,
                format!(
                    "{} overlaps protected {}",
                    canonical.display(),
                    protected.display()
                ),
            ));
        }
        if let Some(marker) = markers.iter().find(|m| !self.facts.exists(m)) {
            return Err(refuse(
                Rule::MissingMarker,
                format!("{} does not exist", marker.display()),
            ));
        }
        let tracked = self
            .facts
            .tracked_files(&canonical)
            .map_err(|e| facts_unavailable("listing git tracked files in", &canonical, e))?;
        if tracked > 0 {
            return Err(refuse(
                Rule::TrackedByGit,
                format!(
                    "{} holds {tracked} files tracked by git",
                    canonical.display()
                ),
            ));
        }
        Ok(())
    }

    fn protected_root_overlapping(&self, path: &Path) -> Option<PathBuf> {
        self.protected_roots
            .iter()
            .flat_map(|root| [root.clone(), self.facts.canonical(root).unwrap_or_default()])
            .filter(|root| !root.as_os_str().is_empty())
            .find(|root| path.starts_with(root) || root.starts_with(path))
    }
}

fn refuse(rule: Rule, detail: String) -> Refusal {
    Refusal { rule, detail }
}

fn facts_unavailable(action: &str, path: &Path, error: PortError) -> Refusal {
    refuse(
        Rule::FactsUnavailable,
        format!("{action} {}: {}", path.display(), error_chain(&*error)),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::port::MockPathFacts;

    const ROOT_DEVICE: u64 = 1;

    struct Case {
        name: &'static str,
        path: Option<&'static str>,
        device: u64,
        markers_exist: bool,
        tracked: usize,
        canonical_fails: bool,
        want: Option<Rule>,
    }

    fn base(name: &'static str, path: &'static str) -> Case {
        Case {
            name,
            path: Some(path),
            device: ROOT_DEVICE,
            markers_exist: true,
            tracked: 0,
            canonical_fails: false,
            want: None,
        }
    }

    fn facts_for(case: &Case) -> MockPathFacts {
        let mut facts = MockPathFacts::new();
        let canonical_fails = case.canonical_fails;
        facts.expect_canonical().returning(move |p| {
            if canonical_fails && !p.starts_with("/home/u/.claude") {
                return Err("permission denied".into());
            }
            Ok(p.to_path_buf())
        });
        let device = case.device;
        facts.expect_device().returning(move |p| {
            if p == Path::new("/") {
                return Ok(ROOT_DEVICE);
            }
            Ok(device)
        });
        let markers_exist = case.markers_exist;
        facts.expect_exists().returning(move |_| markers_exist);
        let tracked = case.tracked;
        facts.expect_tracked_files().returning(move |_| Ok(tracked));
        facts
    }

    fn item_for(case: &Case) -> CleanupItem {
        CleanupItem {
            source: "test".into(),
            label: "test".into(),
            path: case.path.map(PathBuf::from),
            required_markers: vec![PathBuf::from("/marker")],
            bytes: Some(1),
            commands: vec![vec!["true".into()]],
        }
    }

    #[test]
    fn every_guard_rule_refuses_by_name_and_a_clean_cache_passes() {
        let cases = vec![
            base("a plain cache folder passes", "/home/u/.cache/go-build"),
            Case {
                path: None,
                ..base("an item without a path passes", "")
            },
            Case {
                device: 2,
                want: Some(Rule::NotWslRootFilesystem),
                ..base("a windows drive is refused", "/mnt/c/Users")
            },
            Case {
                want: Some(Rule::ProtectedPath),
                ..base(
                    "a path inside claude work is refused",
                    "/home/u/.claude/jobs/x/target",
                )
            },
            Case {
                want: Some(Rule::ProtectedPath),
                ..base("a path holding claude work is refused", "/home/u")
            },
            Case {
                markers_exist: false,
                want: Some(Rule::MissingMarker),
                ..base("a missing marker is refused", "/home/u/w/target")
            },
            Case {
                tracked: 3,
                want: Some(Rule::TrackedByGit),
                ..base("a path with tracked files is refused", "/home/u/w/target")
            },
            Case {
                canonical_fails: true,
                want: Some(Rule::FactsUnavailable),
                ..base("an unresolvable path is refused", "/home/u/w/target")
            },
        ];
        for case in cases {
            let guard = GuardController::new(
                Arc::new(facts_for(&case)),
                vec![PathBuf::from("/home/u/.claude")],
            );
            let got = guard.check(&item_for(&case)).map(|r| r.rule);
            assert_eq!(got, case.want, "{}", case.name);
        }
    }
}
