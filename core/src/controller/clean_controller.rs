use std::sync::Arc;

use super::GuardController;
use crate::port::{CommandRunner, PathFacts};
use crate::types::{CleanOutcome, CleanupItem};
use crate::util::error_chain;

pub trait Cleaner: Send + Sync {
    fn clean(&self, item: &CleanupItem) -> CleanOutcome;
}

pub struct CleanController {
    runner: Arc<dyn CommandRunner>,
    guard: Arc<GuardController>,
    facts: Arc<dyn PathFacts>,
}

impl CleanController {
    pub fn new(
        runner: Arc<dyn CommandRunner>,
        guard: Arc<GuardController>,
        facts: Arc<dyn PathFacts>,
    ) -> Self {
        Self {
            runner,
            guard,
            facts,
        }
    }
}

impl Cleaner for CleanController {
    fn clean(&self, item: &CleanupItem) -> CleanOutcome {
        if let Some(refusal) = self.guard.check(item) {
            return CleanOutcome::Refused(refusal);
        }
        if item.commands.is_empty() || item.commands.iter().any(Vec::is_empty) {
            return CleanOutcome::Failed(format!(
                "cleaning {}: the item has an empty command",
                item.label
            ));
        }
        let before = match self.facts.root_used_bytes() {
            Ok(bytes) => bytes,
            Err(e) => {
                return CleanOutcome::Failed(format!(
                    "reading used bytes before cleaning {}: {}",
                    item.label,
                    error_chain(&*e)
                ))
            }
        };
        for command in &item.commands {
            if let Err(e) = self.runner.run(command) {
                return CleanOutcome::Failed(format!(
                    "running {}: {}",
                    command.join(" "),
                    error_chain(&*e)
                ));
            }
        }
        match self.facts.root_used_bytes() {
            Ok(after) => CleanOutcome::Freed(before.saturating_sub(after)),
            Err(e) => CleanOutcome::Failed(format!(
                "reading used bytes after cleaning {}: {}",
                item.label,
                error_chain(&*e)
            )),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};
    use std::sync::Mutex;

    use super::*;
    use crate::port::{MockCommandRunner, MockPathFacts};
    use crate::types::Rule;

    fn item(path: &str) -> CleanupItem {
        CleanupItem {
            source: "cargo".into(),
            label: "cargo target".into(),
            path: Some(PathBuf::from(path)),
            required_markers: vec![],
            bytes: Some(10),
            commands: vec![vec!["cargo".into(), "clean".into()]],
        }
    }

    fn facts(device_of_item: u64) -> Arc<MockPathFacts> {
        let mut facts = MockPathFacts::new();
        facts.expect_canonical().returning(|p| Ok(p.to_path_buf()));
        facts.expect_device().returning(move |p| {
            Ok(if p == Path::new("/") {
                1
            } else {
                device_of_item
            })
        });
        facts.expect_exists().returning(|_| true);
        facts.expect_tracked_files().returning(|_| Ok(0));
        let readings = Mutex::new(vec![400, 1000]);
        facts
            .expect_root_used_bytes()
            .returning(move || Ok(readings.lock().unwrap().pop().unwrap()));
        Arc::new(facts)
    }

    fn controller(runner: MockCommandRunner, facts: Arc<MockPathFacts>) -> CleanController {
        let guard = Arc::new(GuardController::new(facts.clone(), vec![]));
        CleanController::new(Arc::new(runner), guard, facts)
    }

    #[test]
    fn a_clean_runs_the_item_command_and_reports_the_drop_in_used_bytes() {
        let mut runner = MockCommandRunner::new();
        runner
            .expect_run()
            .withf(|argv| argv == ["cargo".to_string(), "clean".to_string()])
            .times(1)
            .returning(|_| Ok(()));
        let outcome = controller(runner, facts(1)).clean(&item("/home/u/w/target"));
        assert_eq!(outcome, CleanOutcome::Freed(600));
    }

    #[test]
    fn the_guard_runs_again_before_the_command_and_a_refusal_never_runs_it() {
        let mut runner = MockCommandRunner::new();
        runner.expect_run().times(0);
        let outcome = controller(runner, facts(2)).clean(&item("/mnt/c/Users"));
        assert!(
            matches!(outcome, CleanOutcome::Refused(r) if r.rule == Rule::NotWslRootFilesystem)
        );
    }

    #[test]
    fn a_failing_command_is_reported_with_its_argv_and_cause() {
        let mut runner = MockCommandRunner::new();
        runner
            .expect_run()
            .returning(|_| Err("exit status 101".into()));
        let outcome = controller(runner, facts(1)).clean(&item("/home/u/w/target"));
        assert_eq!(
            outcome,
            CleanOutcome::Failed("running cargo clean: exit status 101".into())
        );
    }

    #[test]
    fn every_command_of_an_item_runs_in_order_and_a_failure_stops_the_rest() {
        let mut runner = MockCommandRunner::new();
        let mut sequence = mockall::Sequence::new();
        runner
            .expect_run()
            .withf(|argv| argv[1] == "container")
            .times(1)
            .in_sequence(&mut sequence)
            .returning(|_| Ok(()));
        runner
            .expect_run()
            .withf(|argv| argv[1] == "volume")
            .times(1)
            .in_sequence(&mut sequence)
            .returning(|_| Err("boom".into()));
        let mut engine = item("/home/u/w/target");
        engine.path = None;
        engine.commands = ["container", "volume", "image"]
            .iter()
            .map(|kind| vec!["podman".to_string(), kind.to_string(), "prune".to_string()])
            .collect();
        let outcome = controller(runner, facts(1)).clean(&engine);
        assert_eq!(
            outcome,
            CleanOutcome::Failed("running podman volume prune: boom".into())
        );
    }

    #[test]
    fn an_item_without_a_command_never_reaches_the_runner() {
        let mut runner = MockCommandRunner::new();
        runner.expect_run().times(0);
        let mut empty = item("/home/u/w/target");
        empty.commands.clear();
        let outcome = controller(runner, facts(1)).clean(&empty);
        assert!(matches!(outcome, CleanOutcome::Failed(_)));
    }
}
