use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use wdc_core::types::{CleanupItem, CleanupPlan};
use wdc_core::util::human_bytes;

pub const RECLAIM: Color = Color::Cyan;
pub const PROTECTED: Color = Color::Blue;
pub const DELETE: Color = Color::Red;
pub const QUIET: Color = Color::DarkGray;
pub const SEGMENT_COLORS: [Color; 5] = [
    Color::Cyan,
    Color::Magenta,
    Color::Yellow,
    Color::Blue,
    Color::Green,
];

const GB: u64 = 1_000_000_000;
const SPINNER: [&str; 4] = ["◐", "◓", "◑", "◒"];

pub fn size_text(bytes: Option<u64>) -> String {
    bytes.map_or_else(|| "unknown".to_string(), human_bytes)
}

pub fn size_style(bytes: Option<u64>) -> Style {
    match bytes {
        Some(b) if b >= 10 * GB => Style::default()
            .fg(Color::Yellow)
            .add_modifier(Modifier::BOLD),
        Some(b) if b >= GB => Style::default().add_modifier(Modifier::BOLD),
        Some(b) if b >= 100_000_000 => Style::default(),
        _ => Style::default().fg(QUIET),
    }
}

pub fn size_span(bytes: Option<u64>) -> Span<'static> {
    Span::styled(format!("{:>9}", size_text(bytes)), size_style(bytes))
}

pub fn spinner_frame(tick: usize) -> &'static str {
    SPINNER[tick % SPINNER.len()]
}

pub fn quiet(text: impl Into<String>) -> Span<'static> {
    Span::styled(text.into(), Style::default().fg(QUIET))
}

pub fn bold(text: impl Into<String>) -> Span<'static> {
    Span::styled(text.into(), Style::default().add_modifier(Modifier::BOLD))
}

pub fn key_hints(hints: &[(&str, &str)]) -> Line<'static> {
    let mut spans = Vec::new();
    for (key, action) in hints {
        spans.push(bold(key.to_string()));
        spans.push(quiet(format!(" {action}   ")));
    }
    Line::from(spans)
}

pub fn short_path(path: &str, home: &str, width: usize) -> String {
    let path = match path.strip_prefix(home) {
        Some(rest) if !home.is_empty() && (rest.is_empty() || rest.starts_with('/')) => {
            format!("~{rest}")
        }
        _ => path.to_string(),
    };
    if path.chars().count() <= width {
        return path;
    }
    let parts: Vec<&str> = path.split('/').collect();
    let head = parts[0];
    let mut tail: Vec<&str> = Vec::new();
    for part in parts[1..].iter().rev() {
        let candidate = format!("{head}/…/{part}/{}", tail.join("/"));
        if candidate.chars().count() > width && !tail.is_empty() {
            break;
        }
        tail.insert(0, part);
    }
    format!("{head}/…/{}", tail.join("/"))
}

pub fn path_spans(path: &str) -> Vec<Span<'static>> {
    match path.rsplit_once('/') {
        Some((parent, name)) => vec![quiet(format!("{parent}/")), Span::raw(name.to_string())],
        None => vec![Span::raw(path.to_string())],
    }
}

pub fn strip_cells(segments: &[u64], total: u64, width: usize) -> Vec<usize> {
    let mut cells = Vec::with_capacity(segments.len());
    let mut used = 0;
    for bytes in segments {
        let share = if total == 0 {
            0
        } else {
            ((*bytes as f64 / total as f64) * width as f64).round() as usize
        };
        let share = share.min(width - used);
        used += share;
        cells.push(share);
    }
    cells
}

pub fn strip(segments: &[(u64, Style)], total: u64, width: usize) -> Line<'static> {
    let bytes: Vec<u64> = segments.iter().map(|(b, _)| *b).collect();
    let cells = strip_cells(&bytes, total, width);
    let mut spans: Vec<Span> = segments
        .iter()
        .zip(&cells)
        .filter(|(_, n)| **n > 0)
        .map(|((_, style), n)| Span::styled("█".repeat(*n), *style))
        .collect();
    let filled: usize = cells.iter().sum();
    if filled < width {
        spans.push(quiet("░".repeat(width - filled)));
    }
    Line::from(spans)
}

pub struct PlanGroup {
    pub source: String,
    pub bytes: u64,
    pub entries: Vec<usize>,
}

pub fn plan_groups(plan: &CleanupPlan) -> Vec<PlanGroup> {
    let mut groups: Vec<PlanGroup> = Vec::new();
    for (index, entry) in plan.entries.iter().enumerate() {
        if entry.refusal.is_some() {
            continue;
        }
        let group = match groups
            .iter_mut()
            .position(|g| g.source == entry.item.source)
        {
            Some(position) => &mut groups[position],
            None => {
                groups.push(PlanGroup {
                    source: entry.item.source.clone(),
                    bytes: 0,
                    entries: vec![],
                });
                groups.last_mut().expect("a group was just pushed")
            }
        };
        group.bytes += entry.item.known_bytes();
        group.entries.push(index);
    }
    for group in &mut groups {
        group
            .entries
            .sort_by_key(|i| std::cmp::Reverse(plan.entries[*i].item.known_bytes()));
    }
    groups.sort_by_key(|g| std::cmp::Reverse(g.bytes));
    groups
}

pub fn protected_entries(plan: &CleanupPlan) -> Vec<usize> {
    let mut protected: Vec<usize> = (0..plan.entries.len())
        .filter(|i| plan.entries[*i].refusal.is_some())
        .collect();
    protected.sort_by_key(|i| std::cmp::Reverse(plan.entries[*i].item.known_bytes()));
    protected
}

pub fn display_order(plan: &CleanupPlan) -> Vec<usize> {
    plan_groups(plan)
        .into_iter()
        .flat_map(|g| g.entries)
        .chain(protected_entries(plan))
        .collect()
}

pub fn item_target(item: &CleanupItem, home: &str, width: usize) -> String {
    match &item.path {
        Some(path) => short_path(&path.display().to_string(), home, width),
        None => item.label.clone(),
    }
}

pub fn commands_text(item: &CleanupItem) -> String {
    item.commands
        .iter()
        .map(|c| c.join(" "))
        .collect::<Vec<_>>()
        .join(", then ")
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;
    use wdc_core::types::{PlanEntry, Refusal, Rule};

    fn entry(source: &str, bytes: u64, refused: bool) -> PlanEntry {
        PlanEntry {
            item: CleanupItem {
                source: source.into(),
                label: source.into(),
                path: Some(PathBuf::from(format!("/w/{source}/{bytes}"))),
                required_markers: vec![],
                bytes: Some(bytes),
                commands: vec![vec!["true".into()]],
            },
            refusal: refused.then(|| Refusal {
                rule: Rule::ProtectedPath,
                detail: "kept".into(),
            }),
        }
    }

    #[test]
    fn groups_sort_by_total_size_and_protected_items_come_last() {
        let plan = CleanupPlan {
            entries: vec![
                entry("Cargo", 10, false),
                entry("npm cache", 50, false),
                entry("Cargo", 30, false),
                entry("Cargo", 99, true),
            ],
            ..CleanupPlan::default()
        };
        let groups: Vec<_> = plan_groups(&plan)
            .into_iter()
            .map(|g| (g.source, g.bytes, g.entries))
            .collect();
        assert_eq!(
            groups,
            vec![
                ("npm cache".to_string(), 50, vec![1]),
                ("Cargo".to_string(), 40, vec![2, 0])
            ]
        );
        assert_eq!(display_order(&plan), vec![1, 2, 0, 3]);
    }

    #[test]
    fn a_home_path_starts_with_a_tilde() {
        assert_eq!(short_path("/home/u/.npm", "/home/u", 80), "~/.npm");
        assert_eq!(short_path("/home/user2/x", "/home/u", 80), "/home/user2/x");
    }

    #[test]
    fn a_long_path_keeps_its_head_and_as_much_of_its_end_as_fits() {
        let path = "/home/u/workspaces/songe/forge-dev-codegen/demo/tui-rust/target";
        assert_eq!(short_path(path, "/home/u", 30), "~/…/demo/tui-rust/target");
    }

    #[test]
    fn a_long_last_folder_is_never_cut() {
        let path = "/home/u/a/a-very-long-folder-name-that-does-not-fit";
        assert_eq!(
            short_path(path, "/home/u", 10),
            "~/…/a-very-long-folder-name-that-does-not-fit"
        );
    }

    #[test]
    fn strip_cells_split_the_width_by_share_and_never_overflow() {
        assert_eq!(strip_cells(&[50, 25], 100, 40), vec![20, 10]);
        assert_eq!(strip_cells(&[100, 100], 100, 40), vec![40, 0]);
        assert_eq!(strip_cells(&[5], 0, 40), vec![0]);
    }

    #[test]
    fn an_unknown_size_reads_unknown() {
        assert_eq!(size_text(None), "unknown");
        assert_eq!(size_text(Some(1_500_000_000)), "1.5 GB");
    }
}
