use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::Arc;

use anyhow::{Context, Result};
use clap::Parser;
use wdc_app::adapter::buildah_adapter::BuildahAdapter;
use wdc_app::adapter::cargo_target_adapter::CargoTargetAdapter;
use wdc_app::adapter::container_adapter::{ContainerAdapter, CONTAINER_ENGINES};
use wdc_app::adapter::powershell_adapter::PowershellAdapter;
use wdc_app::adapter::process_adapter::ProcessAdapter;
use wdc_app::adapter::tool_cache_adapter::{ToolCacheAdapter, TOOL_CACHES};
use wdc_app::adapter::wsl_fs_adapter::WslFsAdapter;
use wdc_app::driver::cli_driver::{Cli, CliDriver};
use wdc_core::controller::{CleanController, GuardController, PlanController, ScanController};
use wdc_core::port::{CleanupSource, PathFacts, UsageSource};

const MIN_ROW_BYTES: u64 = 1_000_000_000;
const MAX_TREE_DEPTH: usize = 8;

fn main() -> ExitCode {
    let cli = Cli::parse();
    match build_driver().and_then(|driver| driver.run(cli)) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("wdc: {e:#}");
            ExitCode::FAILURE
        }
    }
}

fn build_driver() -> Result<CliDriver> {
    let home = PathBuf::from(std::env::var("HOME").context("reading HOME")?);
    let wsl = Arc::new(WslFsAdapter::new(
        PathBuf::from("/"),
        MIN_ROW_BYTES,
        MAX_TREE_DEPTH,
    ));
    let facts: Arc<dyn PathFacts> = wsl.clone();
    let guard = Arc::new(GuardController::new(
        facts.clone(),
        vec![home.join(".claude")],
    ));

    let usage_sources: Vec<Arc<dyn UsageSource>> = vec![
        wsl,
        Arc::new(PowershellAdapter::new(MIN_ROW_BYTES, MAX_TREE_DEPTH)),
    ];
    let mut cleanup_sources: Vec<Arc<dyn CleanupSource>> = vec![
        Arc::new(CargoTargetAdapter::new(home.clone())),
        Arc::new(BuildahAdapter),
    ];
    cleanup_sources.extend(
        TOOL_CACHES
            .iter()
            .map(|cache| Arc::new(ToolCacheAdapter::new(cache)) as Arc<dyn CleanupSource>),
    );
    cleanup_sources.extend(CONTAINER_ENGINES.iter().map(|engine| {
        Arc::new(ContainerAdapter::new(engine, home.clone())) as Arc<dyn CleanupSource>
    }));

    let scanner = Arc::new(ScanController::new(usage_sources, MIN_ROW_BYTES));
    let planner = Arc::new(PlanController::new(
        cleanup_sources,
        guard.clone(),
        facts.clone(),
    ));
    let cleaner = Arc::new(CleanController::new(Arc::new(ProcessAdapter), guard, facts));
    Ok(CliDriver::new(
        scanner,
        planner,
        cleaner,
        home.display().to_string(),
    ))
}
