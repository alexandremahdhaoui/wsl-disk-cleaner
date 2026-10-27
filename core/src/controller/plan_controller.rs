use std::sync::Arc;
use std::thread;

use super::GuardController;
use crate::port::{CleanupSource, PathFacts, PortError};
use crate::types::{
    CleanupItem, CleanupPlan, Discovery, PlanEntry, Refusal, Rule, UnavailableCleaner,
};
use crate::util::error_chain;

pub trait Planner: Send + Sync {
    fn plan(&self) -> CleanupPlan;
}

pub struct PlanController {
    sources: Vec<Arc<dyn CleanupSource>>,
    guard: Arc<GuardController>,
    facts: Arc<dyn PathFacts>,
}

enum SourceResult {
    NotInstalled,
    Entries(Vec<PlanEntry>),
    Failed(String),
}

impl PlanController {
    pub fn new(
        sources: Vec<Arc<dyn CleanupSource>>,
        guard: Arc<GuardController>,
        facts: Arc<dyn PathFacts>,
    ) -> Self {
        Self {
            sources,
            guard,
            facts,
        }
    }

    fn result_for(&self, source: &dyn CleanupSource) -> SourceResult {
        match self.entries_for(source) {
            Ok(Some(entries)) => SourceResult::Entries(entries),
            Ok(None) => SourceResult::NotInstalled,
            Err(e) => SourceResult::Failed(format!("discovering items: {}", error_chain(&*e))),
        }
    }

    fn entries_for(&self, source: &dyn CleanupSource) -> Result<Option<Vec<PlanEntry>>, PortError> {
        let Discovery::Found(items) = source.discover()? else {
            return Ok(None);
        };
        Ok(Some(
            items
                .into_iter()
                .map(|item| self.entry_for(item))
                .filter(|entry| entry.item.bytes != Some(0) || entry.refusal.is_some())
                .collect(),
        ))
    }

    fn entry_for(&self, mut item: CleanupItem) -> PlanEntry {
        if let Some(path) = &item.path {
            match self.facts.size(path) {
                Ok(bytes) => item.bytes = Some(bytes),
                Err(e) => {
                    let refusal = Refusal {
                        rule: Rule::FactsUnavailable,
                        detail: format!("measuring {}: {}", path.display(), error_chain(&*e)),
                    };
                    return PlanEntry {
                        item,
                        refusal: Some(refusal),
                    };
                }
            }
        }
        let refusal = self.guard.check(&item);
        PlanEntry { item, refusal }
    }
}

impl Planner for PlanController {
    fn plan(&self) -> CleanupPlan {
        let results: Vec<(String, SourceResult)> = thread::scope(|scope| {
            let handles: Vec<_> = self
                .sources
                .iter()
                .map(|source| scope.spawn(move || self.result_for(source.as_ref())))
                .collect();
            self.sources
                .iter()
                .zip(handles)
                .map(|(source, handle)| {
                    let result = handle.join().unwrap_or_else(|_| {
                        SourceResult::Failed("discovering items: the discovery panicked".into())
                    });
                    (source.name(), result)
                })
                .collect()
        });
        let mut plan = CleanupPlan {
            used_bytes: self.facts.root_used_bytes().unwrap_or(0),
            ..CleanupPlan::default()
        };
        for (source, result) in results {
            match result {
                SourceResult::NotInstalled => {}
                SourceResult::Entries(entries) if entries.is_empty() => plan.empty.push(source),
                SourceResult::Entries(entries) => plan.entries.extend(entries),
                SourceResult::Failed(reason) => {
                    plan.unavailable.push(UnavailableCleaner { source, reason })
                }
            }
        }
        plan
    }
}

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};

    use super::*;
    use crate::port::{MockCleanupSource, MockPathFacts};

    fn item(source: &str, path: Option<&str>, bytes: Option<u64>) -> CleanupItem {
        CleanupItem {
            source: source.into(),
            label: source.into(),
            path: path.map(PathBuf::from),
            required_markers: vec![],
            bytes,
            commands: vec![vec!["true".into()]],
        }
    }

    fn source_with(
        name: &'static str,
        result: Result<Discovery, &'static str>,
    ) -> Arc<dyn CleanupSource> {
        let mut source = MockCleanupSource::new();
        source.expect_name().return_const(name.to_string());
        source
            .expect_discover()
            .return_once(move || result.map_err(|e| e.into()));
        Arc::new(source)
    }

    fn facts() -> Arc<MockPathFacts> {
        let mut facts = MockPathFacts::new();
        facts.expect_canonical().returning(|p| Ok(p.to_path_buf()));
        facts.expect_device().returning(|_| Ok(1));
        facts.expect_exists().returning(|_| true);
        facts.expect_root_used_bytes().returning(|| Ok(1000));
        facts
            .expect_tracked_files()
            .returning(|p| Ok(usize::from(p == Path::new("/home/u/repo/target"))));
        facts.expect_size().returning(|p| {
            if p == Path::new("/home/u/empty") {
                return Ok(0);
            }
            if p == Path::new("/home/u/locked") {
                return Err("permission denied".into());
            }
            Ok(7)
        });
        Arc::new(facts)
    }

    fn controller(sources: Vec<Arc<dyn CleanupSource>>) -> PlanController {
        let facts = facts();
        let guard = Arc::new(GuardController::new(facts.clone(), vec![]));
        PlanController::new(sources, guard, facts)
    }

    #[test]
    fn a_path_item_gets_its_size_from_path_facts_and_keeps_its_order() {
        let plan = controller(vec![source_with(
            "Cargo",
            Ok(Discovery::Found(vec![
                item("Cargo", Some("/home/u/a/target"), None),
                item("Cargo", Some("/home/u/b/target"), None),
            ])),
        )])
        .plan();
        let paths: Vec<_> = plan
            .entries
            .iter()
            .map(|e| (e.item.path.clone().unwrap(), e.item.bytes))
            .collect();
        assert_eq!(
            paths,
            vec![
                (PathBuf::from("/home/u/a/target"), Some(7)),
                (PathBuf::from("/home/u/b/target"), Some(7))
            ]
        );
        assert_eq!(plan.used_bytes, 1000);
    }

    #[test]
    fn a_source_whose_items_are_all_empty_is_listed_as_nothing_to_clean() {
        let plan = controller(vec![source_with(
            "uv cache",
            Ok(Discovery::Found(vec![item(
                "uv cache",
                Some("/home/u/empty"),
                None,
            )])),
        )])
        .plan();
        assert!(plan.entries.is_empty());
        assert_eq!(plan.empty, vec!["uv cache".to_string()]);
    }

    #[test]
    fn an_item_of_unknown_size_stays_in_the_plan() {
        let plan = controller(vec![source_with(
            "Buildah",
            Ok(Discovery::Found(vec![item("Buildah", None, None)])),
        )])
        .plan();
        assert_eq!(plan.entries.len(), 1);
    }

    #[test]
    fn a_tool_that_is_not_installed_leaves_no_trace_in_the_plan() {
        let plan = controller(vec![source_with("nerdctl", Ok(Discovery::NotInstalled))]).plan();
        assert!(plan.entries.is_empty() && plan.empty.is_empty() && plan.unavailable.is_empty());
    }

    #[test]
    fn a_guarded_item_stays_in_the_plan_with_its_refusal() {
        let plan = controller(vec![source_with(
            "Cargo",
            Ok(Discovery::Found(vec![item(
                "Cargo",
                Some("/home/u/repo/target"),
                None,
            )])),
        )])
        .plan();
        assert_eq!(
            plan.entries[0].refusal.as_ref().map(|r| r.rule),
            Some(Rule::TrackedByGit)
        );
    }

    #[test]
    fn an_unmeasurable_item_is_refused_as_facts_unavailable() {
        let plan = controller(vec![source_with(
            "Cargo",
            Ok(Discovery::Found(vec![item(
                "Cargo",
                Some("/home/u/locked"),
                None,
            )])),
        )])
        .plan();
        assert_eq!(
            plan.entries[0].refusal.as_ref().map(|r| r.rule),
            Some(Rule::FactsUnavailable)
        );
    }

    #[test]
    fn a_failing_source_is_listed_as_unavailable_and_the_others_still_plan() {
        let plan = controller(vec![
            source_with("Docker", Err("the docker daemon is not running")),
            source_with(
                "Podman",
                Ok(Discovery::Found(vec![item("Podman", None, Some(5))])),
            ),
        ])
        .plan();
        assert_eq!(plan.entries.len(), 1);
        assert_eq!(
            plan.unavailable,
            vec![UnavailableCleaner {
                source: "Docker".into(),
                reason: "discovering items: the docker daemon is not running".into()
            }]
        );
    }
}
