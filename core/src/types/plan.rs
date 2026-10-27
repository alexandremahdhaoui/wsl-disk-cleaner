use std::path::PathBuf;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CleanupItem {
    pub source: String,
    pub label: String,
    pub path: Option<PathBuf>,
    pub required_markers: Vec<PathBuf>,
    pub bytes: Option<u64>,
    pub commands: Vec<Vec<String>>,
}

impl CleanupItem {
    pub fn known_bytes(&self) -> u64 {
        self.bytes.unwrap_or(0)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Discovery {
    NotInstalled,
    Found(Vec<CleanupItem>),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Rule {
    FactsUnavailable,
    NotWslRootFilesystem,
    ProtectedPath,
    MissingMarker,
    TrackedByGit,
}

impl Rule {
    pub fn name(self) -> &'static str {
        match self {
            Rule::FactsUnavailable => "facts-unavailable",
            Rule::NotWslRootFilesystem => "not-wsl-root-filesystem",
            Rule::ProtectedPath => "protected-path",
            Rule::MissingMarker => "missing-marker",
            Rule::TrackedByGit => "tracked-by-git",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Refusal {
    pub rule: Rule,
    pub detail: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlanEntry {
    pub item: CleanupItem,
    pub refusal: Option<Refusal>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnavailableCleaner {
    pub source: String,
    pub reason: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct CleanupPlan {
    pub used_bytes: u64,
    pub entries: Vec<PlanEntry>,
    pub empty: Vec<String>,
    pub unavailable: Vec<UnavailableCleaner>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CleanOutcome {
    Freed(u64),
    Refused(Refusal),
    Failed(String),
}
