use std::fmt::Write as _;
use std::io::IsTerminal;
use std::sync::Arc;

use anyhow::{bail, Result};
use clap::{Parser, Subcommand};
use wdc_core::controller::{Cleaner, Planner, Scanner};
use wdc_core::types::{CleanupPlan, ScanResult, SizeNode};
use wdc_core::util::human_bytes;

use super::present::{commands_text, item_target, plan_groups, protected_entries, size_text};
use super::tui_driver;

const TEXT_PATH_WIDTH: usize = 200;

#[derive(Parser)]
#[command(
    name = "wdc",
    version,
    about = "See who uses disk space on WSL and Windows. Clean WSL build caches with guardrails."
)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Command,
}

#[derive(Subcommand)]
pub enum Command {
    #[command(about = "Show the biggest folders on WSL and on the Windows C drive")]
    Scan,
    #[command(about = "Plan a cleanup of WSL build caches, pick items, confirm, then clean")]
    Clean {
        #[arg(long, help = "Print the plan and delete nothing")]
        dry_run: bool,
    },
}

pub struct CliDriver {
    scanner: Arc<dyn Scanner>,
    planner: Arc<dyn Planner>,
    cleaner: Arc<dyn Cleaner>,
    home: String,
}

impl CliDriver {
    pub fn new(
        scanner: Arc<dyn Scanner>,
        planner: Arc<dyn Planner>,
        cleaner: Arc<dyn Cleaner>,
        home: String,
    ) -> Self {
        Self {
            scanner,
            planner,
            cleaner,
            home,
        }
    }

    pub fn run(&self, cli: Cli) -> Result<()> {
        let interactive = std::io::stdin().is_terminal() && std::io::stdout().is_terminal();
        match cli.command {
            Command::Scan if interactive => tui_driver::run_scan(self.scanner.as_ref()),
            Command::Scan => {
                print!("{}", render_scan_text(&self.scanner.scan()));
                Ok(())
            }
            Command::Clean { dry_run: true } => {
                print!("{}", render_plan_text(&self.planner.plan(), &self.home));
                Ok(())
            }
            Command::Clean { dry_run: false } if interactive => tui_driver::run_clean(
                self.planner.as_ref(),
                self.cleaner.as_ref(),
                &self.home,
            ),
            Command::Clean { dry_run: false } => bail!(
                "refusing clean: not-interactive: wdc clean needs a terminal to confirm. Use wdc clean --dry-run to print the plan"
            ),
        }
    }
}

pub fn render_plan_text(plan: &CleanupPlan, home: &str) -> String {
    let mut out = String::new();
    let groups = plan_groups(plan);
    let total: u64 = groups.iter().map(|g| g.bytes).sum();
    let _ = writeln!(out, "{} can be freed", human_bytes(total));
    for group in &groups {
        let _ = writeln!(out, "\n{}", group.source);
        for index in &group.entries {
            let item = &plan.entries[*index].item;
            let _ = writeln!(
                out,
                "  {:>9}  {}",
                size_text(item.bytes),
                item_target(item, home, TEXT_PATH_WIDTH)
            );
            let _ = writeln!(out, "             runs {}", commands_text(item));
        }
    }
    if !plan.empty.is_empty() {
        let _ = writeln!(out, "\nNothing to clean in {}", plan.empty.join(", "));
    }
    for unavailable in &plan.unavailable {
        let _ = writeln!(
            out,
            "\n{} could not be checked: {}",
            unavailable.source, unavailable.reason
        );
    }
    let protected = protected_entries(plan);
    if !protected.is_empty() {
        let _ = writeln!(out, "\nProtected");
        for index in protected {
            let entry = &plan.entries[index];
            let _ = writeln!(
                out,
                "  {:>9}  {}",
                size_text(entry.item.bytes),
                item_target(&entry.item, home, TEXT_PATH_WIDTH)
            );
            if let Some(refusal) = &entry.refusal {
                let _ = writeln!(
                    out,
                    "             {}: {}",
                    refusal.rule.name(),
                    refusal.detail
                );
            }
        }
    }
    out
}

pub fn render_scan_text(result: &ScanResult) -> String {
    let mut out = String::new();
    for report in &result.reports {
        let usage = &report.usage;
        let _ = writeln!(
            out,
            "{}: {} used of {}",
            usage.volume.label(),
            human_bytes(usage.used_bytes),
            human_bytes(usage.capacity_bytes)
        );
        for note in &report.notes {
            let _ = writeln!(out, "  ! {:>9}  {}", human_bytes(note.bytes), note.label);
            let _ = writeln!(out, "      {}", note.hint);
        }
        write_tree(&mut out, &usage.tree, 1);
    }
    for failure in &result.failures {
        let _ = writeln!(out, "{}: scan failed: {}", failure.source, failure.reason);
    }
    out
}

fn write_tree(out: &mut String, nodes: &[SizeNode], depth: usize) {
    for node in nodes {
        let _ = writeln!(
            out,
            "{}{:>9}  {}",
            "  ".repeat(depth),
            human_bytes(node.bytes),
            node.name
        );
        write_tree(out, &node.children, depth + 1);
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;
    use wdc_core::types::{CleanupItem, PlanEntry, Refusal, Rule, UnavailableCleaner};

    fn entry(source: &str, path: &str, bytes: u64, refusal: Option<Refusal>) -> PlanEntry {
        PlanEntry {
            item: CleanupItem {
                source: source.into(),
                label: "build output".into(),
                path: Some(PathBuf::from(path)),
                required_markers: vec![],
                bytes: Some(bytes),
                commands: vec![vec!["cargo".into(), "clean".into()]],
            },
            refusal,
        }
    }

    #[test]
    fn the_dry_run_groups_items_and_lists_empty_unavailable_and_protected_after() {
        let plan = CleanupPlan {
            used_bytes: 0,
            entries: vec![
                entry("Cargo", "/home/u/w/a/target", 21_000_000_000, None),
                entry(
                    "Cargo",
                    "/home/u/.claude/x/target",
                    4_000_000_000,
                    Some(Refusal {
                        rule: Rule::ProtectedPath,
                        detail: "overlaps protected /home/u/.claude".into(),
                    }),
                ),
            ],
            empty: vec!["Podman".into(), "uv cache".into()],
            unavailable: vec![UnavailableCleaner {
                source: "Docker".into(),
                reason: "the daemon is not running".into(),
            }],
        };
        assert_eq!(
            render_plan_text(&plan, "/home/u"),
            "21.0 GB can be freed\n\
             \n\
             Cargo\n\
             \x20   21.0 GB  ~/w/a/target\n\
             \x20            runs cargo clean\n\
             \n\
             Nothing to clean in Podman, uv cache\n\
             \n\
             Docker could not be checked: the daemon is not running\n\
             \n\
             Protected\n\
             \x20    4.0 GB  ~/.claude/x/target\n\
             \x20            protected-path: overlaps protected /home/u/.claude\n"
        );
    }
}
