//! opt-bench — measure WF65 corpus codegen and gate it against committed
//! goldens. Static, deterministic, zero-flake. The before/after numbers that
//! gate every optimizer change.
//!
//!   cargo run --bin opt-bench --features opt-metrics                # report vs baseline
//!   cargo run --bin opt-bench --features opt-metrics -- --bless     # (re)generate goldens
//!   cargo run --bin opt-bench --features opt-metrics -- --check     # gate logic, exit code
//!   cargo run --bin opt-bench --features opt-metrics -- --strict    # stale-better baseline fails
//!   cargo run --bin opt-bench --features opt-metrics -- --diff a.json b.json
//!
//! Exit codes: 0 = clean, 1 = regression, 2 = stale baseline / needs --bless.

use std::path::Path;
use std::process::ExitCode;

use anyhow::Result;
use wf64::opt_metrics::{self, FileMetrics, GateOutcome};
use wf64::Wf64Session;

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let bless = args.iter().any(|a| a == "--bless");
    let strict = args.iter().any(|a| a == "--strict");
    let dynamic = args.iter().any(|a| a == "--dynamic");
    let _check = args.iter().any(|a| a == "--check"); // report and --check share the gate path

    if let Some(i) = args.iter().position(|a| a == "--diff") {
        let a = args.get(i + 1);
        let b = args.get(i + 2);
        return match (a, b) {
            (Some(a), Some(b)) => match diff_baselines(Path::new(a), Path::new(b)) {
                Ok(code) => code,
                Err(e) => {
                    eprintln!("opt-bench --diff failed: {e:#}");
                    ExitCode::from(2)
                }
            },
            _ => {
                eprintln!("usage: opt-bench --diff <old.json> <new.json>");
                ExitCode::from(2)
            }
        };
    }

    match run(bless, strict, dynamic) {
        Ok(code) => code,
        Err(e) => {
            eprintln!("opt-bench failed: {e:#}");
            ExitCode::from(2)
        }
    }
}

fn run(bless: bool, strict: bool, dynamic: bool) -> Result<ExitCode> {
    let mut session = Wf64Session::new()?;
    let do_lit = opt_metrics::probe_do_lit(&mut session)?;
    match do_lit {
        Some(a) => println!("do_lit resolved at {a:#x}"),
        None => println!("do_lit NOT resolved — do_lit_count is advisory this run"),
    }
    let corpus = opt_metrics::corpus_files()?;
    if corpus.is_empty() {
        eprintln!("no corpus files under {}", opt_metrics::corpus_dir().display());
        return Ok(ExitCode::from(2));
    }

    println!(
        "\n{} corpus file(s) | mode: {}\n",
        corpus.len(),
        if bless { "BLESS" } else { "GATE" }
    );

    let mut any_regression = false;
    let mut any_stale = false;
    let mut tot_bytes_saved: i64 = 0;
    let mut tot_calls_elim: i64 = 0;
    let mut tot_jmps_gained: i64 = 0;

    for path in &corpus {
        let name = path.file_name().unwrap().to_string_lossy();
        let live = opt_metrics::measure_file(&mut session, path, do_lit)?;
        let bpath = opt_metrics::baseline_path(path);

        if bless {
            opt_metrics::write_baseline(&bpath, &live)?;
            println!(
                "  blessed {:<26} {:>3} words  {:>5}B  {:>3} calls  {:>3} do_lit",
                name, live.words.len(), live.file_totals.bytes, live.file_totals.calls, live.file_totals.do_lit
            );
            continue;
        }

        if !bpath.exists() {
            println!("  {name:<26} NO BASELINE — run --bless");
            any_stale = true;
            continue;
        }
        let base = opt_metrics::read_baseline(&bpath)?;
        let (bs, ce, jg) = headline(&live, &base);
        tot_bytes_saved += bs;
        tot_calls_elim += ce;
        tot_jmps_gained += jg;

        match opt_metrics::compare(&live, &base, strict) {
            GateOutcome::Clean => println!("  {name:<26} ok"),
            GateOutcome::Improved(notes) => {
                println!("  {name:<26} IMPROVED (re-bless to lock in):");
                for n in notes {
                    println!("      + {n}");
                }
            }
            GateOutcome::Regressed(msgs) => {
                any_regression = true;
                println!("  {name:<26} REGRESSED:");
                for m in msgs {
                    println!("      ! {m}");
                }
            }
            GateOutcome::Stale(m) => {
                any_stale = true;
                println!("  {name:<26} STALE: {m}");
            }
        }
    }

    if !bless {
        println!(
            "\nheadline vs baseline:  {} bytes saved   {} calls eliminated   {} tail-jmps gained",
            tot_bytes_saved, tot_calls_elim, tot_jmps_gained
        );
    }

    if dynamic {
        println!("\nadvisory dynamic (rdtsc, off the gate path — never affects exit code):");
        match wf64::opt_timing::run_dynamic(&mut session, 11, 2) {
            Ok(lines) => {
                for l in lines {
                    println!("{l}");
                }
            }
            Err(e) => eprintln!("  dynamic layer skipped: {e:#}"),
        }
    }

    Ok(if any_regression {
        ExitCode::from(1)
    } else if any_stale {
        ExitCode::from(2)
    } else {
        ExitCode::SUCCESS
    })
}

/// Per-file headline: bytes saved / calls eliminated / tail-jmps gained vs the
/// baseline (only the wins; regressions show in the gate output).
fn headline(live: &FileMetrics, base: &FileMetrics) -> (i64, i64, i64) {
    let mut bytes = 0i64;
    let mut calls = 0i64;
    let mut jmps = 0i64;
    for (name, l) in &live.words {
        if let Some(b) = base.words.get(name) {
            bytes += (b.byte_length as i64 - l.byte_length as i64).max(0);
            calls += (b.call_count_E8 as i64 - l.call_count_E8 as i64).max(0);
            if !b.tail_is_jmp && l.tail_is_jmp {
                jmps += 1;
            }
        }
    }
    (bytes, calls, jmps)
}

fn diff_baselines(a: &Path, b: &Path) -> Result<ExitCode> {
    let old = opt_metrics::read_baseline(a)?;
    let new = opt_metrics::read_baseline(b)?;
    let (bs, ce, jg) = headline(&new, &old);
    println!("diff {} -> {}", a.display(), b.display());
    println!("  {bs} bytes saved   {ce} calls eliminated   {jg} tail-jmps gained");
    for (name, n) in &new.words {
        if let Some(o) = old.words.get(name) {
            if o != n {
                println!(
                    "  {name}: bytes {}->{}  calls {}->{}  do_lit {}->{}  tail_jmp {}->{}",
                    o.byte_length, n.byte_length, o.call_count_E8, n.call_count_E8,
                    o.do_lit_count, n.do_lit_count, o.tail_is_jmp, n.tail_is_jmp
                );
            }
        }
    }
    Ok(ExitCode::SUCCESS)
}
