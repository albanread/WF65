//! Optimizer measurement — the ADVISORY dynamic layer.
//!
//! Strictly off the gate path: nothing here is ever asserted, and the static
//! gate test contains no timing code, so Windows timer jitter can never flake
//! CI. This layer only narrates "the static byte/call win shows up in cycles
//! too". Numbers go to git-ignored `bench/baseline/<file>.timing.json`
//! companions — never committed, never byte-compared, never gating.
//!
//! To measure the *word* rather than the Rust→Forth `call_xt` boundary cost, the
//! timing loop runs inside Forth: a generated `__b` word brackets an
//! M-iteration loop of `<args> <hot> drop` between two in-Forth RDTSC reads and
//! returns the cycle delta, so one `call_xt` amortizes over M invocations. We
//! take the median (robust to the scheduler/turbo tail) and the min (preemption
//! only adds cycles) over K outer reps, with the measuring thread pinned to one
//! core at high priority.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};

use crate::opt_metrics::{baseline_dir, corpus_dir};
use crate::Wf64Session;

/// One hot-word entry from `bench/manifest.json`.
#[derive(Clone, Debug, Deserialize)]
pub struct HotEntry {
    pub file: String,
    #[serde(default)]
    pub transform: u32,
    #[serde(default)]
    pub arg: String,
    pub iterations: u64,
    #[serde(default)]
    pub verdict: bool,
}

#[derive(Debug, Deserialize)]
struct Manifest {
    words: BTreeMap<String, HotEntry>,
}

/// Read `bench/manifest.json` -> { hot_word -> entry }.
pub fn read_manifest() -> Result<BTreeMap<String, HotEntry>> {
    let path = corpus_dir().parent().unwrap().join("manifest.json");
    let s = std::fs::read_to_string(&path)
        .with_context(|| format!("read manifest {}", path.display()))?;
    let m: Manifest = serde_json::from_str(&s)?;
    Ok(m.words)
}

/// Advisory timing result for one hot word.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct TimingStats {
    pub median_cycles: u64,
    pub min_cycles: u64,
    pub mad_cycles: u64,
    pub inner: u64,
    pub reps: u32,
    pub warmup: u32,
}

fn median(sorted: &[u64]) -> u64 {
    let n = sorted.len();
    if n == 0 {
        return 0;
    }
    sorted[n / 2]
}

/// Pin the measuring thread to one core at high priority to shrink the scheduler
/// noise band. Best-effort: failures are ignored (advisory layer).
#[cfg(windows)]
pub fn pin_realtime() {
    use windows::Win32::System::Threading::{
        GetCurrentProcess, GetCurrentThread, SetPriorityClass, SetThreadAffinityMask,
        HIGH_PRIORITY_CLASS,
    };
    unsafe {
        let _ = SetThreadAffinityMask(GetCurrentThread(), 1usize); // core 0
        let _ = SetPriorityClass(GetCurrentProcess(), HIGH_PRIORITY_CLASS);
    }
}

#[cfg(not(windows))]
pub fn pin_realtime() {}

/// Time one hot word: reset, load its file, build the in-Forth bench loop, warm
/// up, then take K reps. `inner` is the per-call loop count M.
pub fn time_hot(
    session: &mut Wf64Session,
    hot: &str,
    entry: &HotEntry,
    inner: u64,
    reps: u32,
    warmup: u32,
) -> Result<TimingStats> {
    session.reset();
    let path = corpus_dir().join(&entry.file);
    session
        .load_source_file(&path)
        .with_context(|| format!("load {} for timing {hot}", path.display()))?;

    // Build: a 64-bit RDTSC reader, and a bench word that brackets an
    // M-iteration loop of `<args> <hot> drop` between two reads, leaving the
    // cycle delta on the stack. Every hot word leaves exactly one result, so a
    // single `drop` keeps the loop body net-zero.
    let prelude = format!(
        ": rdtsc-u rdtsc 32 lshift or ;\n: __b rdtsc-u {m} 0 ?do {args} {hot} drop loop rdtsc-u swap - ;\n",
        m = inner,
        args = entry.arg,
        hot = hot,
    );
    session
        .eval(&prelude)
        .with_context(|| format!("define bench loop for {hot}"))?;

    // Resolve __b's xt (its code start) from the runtime word table.
    let xt = session
        .debug_words()
        .into_iter()
        .find(|(n, _, _)| n == "__b")
        .map(|(_, s, _)| s)
        .ok_or_else(|| anyhow::anyhow!("bench word __b not found after definition"))?;

    let base_depth = session.depth();
    let mut run_once = |session: &mut Wf64Session| -> Result<u64> {
        session.call_xt(xt)?;
        if session.depth() != base_depth + 1 {
            bail!(
                "bench loop for {hot} left depth {} (expected {})",
                session.depth(),
                base_depth + 1
            );
        }
        Ok(session.pop() as u64)
    };

    for _ in 0..warmup {
        run_once(session)?;
    }
    let mut samples: Vec<u64> = Vec::with_capacity(reps as usize);
    for _ in 0..reps {
        let delta = run_once(session)?;
        samples.push(delta / inner.max(1));
    }
    samples.sort_unstable();
    let med = median(&samples);
    let mut devs: Vec<u64> = samples.iter().map(|&x| x.abs_diff(med)).collect();
    devs.sort_unstable();
    Ok(TimingStats {
        median_cycles: med,
        min_cycles: *samples.first().unwrap_or(&0),
        mad_cycles: median(&devs),
        inner,
        reps,
        warmup,
    })
}

// ---------------------------------------------------------------------------
// Timing baseline IO (git-ignored companions)
// ---------------------------------------------------------------------------

pub fn timing_path(basename: &str) -> PathBuf {
    baseline_dir().join(format!("{basename}.timing.json"))
}

pub fn write_timing(basename: &str, stats: &BTreeMap<String, TimingStats>) -> Result<()> {
    let path = timing_path(basename);
    if let Some(p) = path.parent() {
        std::fs::create_dir_all(p)?;
    }
    std::fs::write(&path, serde_json::to_string_pretty(stats)? + "\n")?;
    Ok(())
}

pub fn read_timing(basename: &str) -> Option<BTreeMap<String, TimingStats>> {
    let path = timing_path(basename);
    let s = std::fs::read_to_string(path).ok()?;
    serde_json::from_str(&s).ok()
}

/// Run the dynamic layer over every manifest hot word, write the git-ignored
/// per-file timing companions, and return human-readable report lines (median ±
/// MAD, plus current/baseline ratio when a prior timing file exists).
pub fn run_dynamic(session: &mut Wf64Session, reps: u32, warmup: u32) -> Result<Vec<String>> {
    pin_realtime();
    let manifest = read_manifest()?;

    // Group hot words by corpus file basename so each file gets one companion.
    let mut by_file: BTreeMap<String, BTreeMap<String, TimingStats>> = BTreeMap::new();
    let mut order: Vec<(String, String)> = Vec::new(); // (basename, hot) in manifest order
    let mut report = Vec::new();

    for (hot, entry) in &manifest {
        let basename = Path::new(&entry.file)
            .file_stem()
            .unwrap()
            .to_string_lossy()
            .to_string();
        let stats = time_hot(session, hot, entry, entry.iterations, reps, warmup)?;
        let prior = read_timing(&basename).and_then(|m| m.get(hot).cloned());
        let ratio = prior
            .as_ref()
            .filter(|p| p.median_cycles > 0)
            .map(|p| stats.median_cycles as f64 / p.median_cycles as f64);
        report.push(format!(
            "  {hot:<14} {:>6} cyc/iter  ± {:<4} (min {})  {}",
            stats.median_cycles,
            stats.mad_cycles,
            stats.min_cycles,
            match ratio {
                Some(r) => format!("ratio vs baseline {r:.3}"),
                None => "(no prior timing)".to_string(),
            },
        ));
        by_file.entry(basename.clone()).or_default().insert(hot.clone(), stats);
        order.push((basename, hot.clone()));
    }

    for (basename, stats) in &by_file {
        write_timing(basename, stats)?;
    }
    Ok(report)
}
