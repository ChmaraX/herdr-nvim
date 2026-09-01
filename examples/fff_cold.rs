//! Measures fff-search's cold per-invocation cost -- fresh process, no
//! prior index, no warm cache -- on an arbitrary repo. The picker's popup
//! is a fresh process per invocation (no resident index), so this is the
//! number that determines whether that lifecycle is viable on a given repo
//! size. See docs/fff-picker.md for measured results.
//!
//! Run: `cargo run --release --example fff_cold -- /path/to/big/repo [query]`

use std::time::{Duration, Instant};

use fff_search::file_picker::FilePicker;
use fff_search::frecency::FrecencyTracker;
use fff_search::{
    FFFMode, FilePickerOptions, FuzzySearchOptions, PaginationArgs, QueryParser, SharedFilePicker,
    SharedFrecency,
};

fn main() -> anyhow::Result<()> {
    let mut args = std::env::args().skip(1);
    let root = args.next().unwrap_or_else(|| ".".to_owned());
    let query_str = args.next().unwrap_or_else(|| "picker".to_owned());

    let t_process = Instant::now();

    // Frecency DB: throwaway temp dir (cost included -- a real cold popup
    // would open one too, or skip it entirely).
    let tmp = std::env::temp_dir().join(format!("fff-cold-{}", std::process::id()));
    std::fs::create_dir_all(&tmp)?;

    let t0 = Instant::now();
    let shared_picker = SharedFilePicker::default();
    let shared_frecency = SharedFrecency::default();
    let frecency = FrecencyTracker::open(tmp.join("frecency"))?;
    shared_frecency.init(frecency)?;
    let setup_elapsed = t0.elapsed();

    let t0 = Instant::now();
    FilePicker::new_with_shared_state(
        shared_picker.clone(),
        shared_frecency.clone(),
        FilePickerOptions {
            base_path: root.clone().into(),
            mode: FFFMode::Ai,
            watch: false,
            ..Default::default()
        },
    )?;
    if !shared_picker.wait_for_scan(Duration::from_secs(60)) {
        anyhow::bail!("scan did not finish within 60s");
    }
    let scan_elapsed = t0.elapsed();

    let guard = shared_picker.read().map_err(|e| anyhow::anyhow!("{e:?}"))?;
    let picker = guard.as_ref().unwrap();
    let file_count = picker.live_file_count();

    let parser = QueryParser::default();
    let query = parser.parse(&query_str);
    let t0 = Instant::now();
    let results = picker.fuzzy_search(
        &query,
        None,
        FuzzySearchOptions {
            max_threads: 0,
            current_file: None,
            pagination: PaginationArgs {
                offset: 0,
                limit: 8,
            },
            ..Default::default()
        },
    );
    let query_elapsed = t0.elapsed();

    println!("repo:            {root}");
    println!("indexed files:   {file_count}");
    println!("frecency setup:  {setup_elapsed:?}");
    println!("scan + index:    {scan_elapsed:?}");
    println!(
        "query {query_str:?}: {} matches in {query_elapsed:?}",
        results.total_matched
    );
    for item in results.items.iter().take(5) {
        println!("  {}", item.relative_path(picker));
    }
    println!("TOTAL cold (setup+scan+query): {:?}", t_process.elapsed());

    drop(guard);
    std::fs::remove_dir_all(&tmp).ok();
    Ok(())
}
