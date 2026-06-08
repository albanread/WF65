//! Optimizer static gate — the zero-flake CI check.
//!
//! For every `bench/corpus/*.f` file: compile it, measure each word's compiled
//! body, and compare against its committed golden in `bench/baseline/`. Any
//! wrong-direction move of a gate metric (byte_length / call_count / jmp_count /
//! do_lit_count up, or tail_is_jmp flipping true->false) fails CI. Improvements
//! pass (the optimizer is supposed to improve); a fingerprint/corpus-hash
//! mismatch reports "regenerate" rather than emitting false regressions.
//!
//! No timing code on this path — it cannot flake. Build with the feature:
//!   cargo test --features opt-metrics --test optimizer_static_gate
//!
//! Without the feature the test compiles to a no-op so the default
//! `cargo test` stays green.

#[cfg(feature = "opt-metrics")]
#[test]
fn optimizer_static_gate() {
    use wf64::opt_metrics::{self, GateOutcome};
    use wf64::Wf64Session;

    let mut session = Wf64Session::new().expect("boot WF65 session");
    let do_lit = opt_metrics::probe_do_lit(&mut session).expect("probe do_lit");

    let corpus = opt_metrics::corpus_files().expect("read corpus dir");
    assert!(!corpus.is_empty(), "no corpus files found under bench/corpus");

    let mut failures: Vec<String> = Vec::new();
    for path in &corpus {
        let live = opt_metrics::measure_file(&mut session, path, do_lit)
            .unwrap_or_else(|e| panic!("measure {}: {e:#}", path.display()));
        let bpath = opt_metrics::baseline_path(path);
        if !bpath.exists() {
            failures.push(format!(
                "{}: no committed baseline — run `cargo run --bin opt-bench --features opt-metrics -- --bless`",
                path.display()
            ));
            continue;
        }
        let base = opt_metrics::read_baseline(&bpath).expect("read baseline");
        match opt_metrics::compare(&live, &base, false) {
            GateOutcome::Clean | GateOutcome::Improved(_) => {}
            GateOutcome::Regressed(msgs) => {
                failures.push(format!("{}:\n  {}", path.display(), msgs.join("\n  ")));
            }
            GateOutcome::Stale(m) => {
                failures.push(format!("{}: {m}", path.display()));
            }
        }
    }

    assert!(
        failures.is_empty(),
        "optimizer static gate found regressions:\n{}",
        failures.join("\n")
    );
}
