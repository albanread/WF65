# WF65 optimizer measurement corpus

A deterministic before/after harness that gates the **Forth-aware optimizer**
work on real codegen numbers instead of intuition. Compile a curated corpus,
measure each compiled word's body, and compare against checked-in goldens so
every optimizer change is a reviewable diff and any regression fails CI.

See [DESIGN.md](DESIGN.md) for the full rationale (synthesized from a 3-lens
design panel: static-determinism, dynamic-realism, optimizer-coverage).

## Layout

```
bench/
  corpus/*.f              the headless, pure-compute corpus (one file per category)
  baseline/*.json         checked-in golden static metrics (one per corpus file)
  baseline/*.timing.json  advisory dynamic numbers (git-ignored; see DESIGN.md)
  manifest.json           hot-word -> arg/iterations map (for the dynamic layer)
```

The harness itself is `src/opt_metrics.rs` (the shared measurement core, behind
the `opt-metrics` cargo feature), driven by two consumers that cannot drift:

- **`tests/optimizer_static_gate.rs`** — the CI gate (`#[test]`, zero timing on
  its path, byte-exact).
- **`src/bin/opt_bench.rs`** (`opt-bench`) — the human report + `--bless` tool.

## Commands

```sh
# report current codegen vs the committed baseline (exit 0 clean / 1 regression / 2 stale)
cargo run --bin opt-bench --features opt-metrics

# (re)generate the goldens — run after an intended codegen change, commit in the SAME PR
cargo run --bin opt-bench --features opt-metrics -- --bless

# run the CI gate
cargo test --features opt-metrics --test optimizer_static_gate

# compare two baseline snapshots
cargo run --bin opt-bench --features opt-metrics -- --diff old.json new.json
```

Without `--features opt-metrics` the module, bin, and test all vanish from the
build graph — the shipping binaries never pull in the decoder/hasher deps, and a
plain `cargo test` stays green.

## Metrics

Gate (deterministic, fails CI on a wrong-direction move):

| metric | meaning |
|---|---|
| `byte_length` | compiled body size (`end - start`). Primary size axis. |
| `call_count_E8` | near CALLs — drop on inline (T2/T5), fold (T1), TCO (T4). The verdict metric where an inline can legitimately grow bytes. |
| `jmp_count_E9` | near JMPs (raw). |
| `tail_is_jmp` | TCO predicate: the body's last unconditional transfer is a JMP leaving the word. Monotone — once true, must stay true. |
| `do_lit_count` | `call do_lit` sequences — the sharpest missed-literal-fold signal. |

Advisory (recorded, not gated until the v2 scheduler lands and is re-blessed):
`instruction_count`, `rbp_adjust_count`, plus the `body_hash` "codegen changed
at all" tripwire. All counts come from an iced-x86 **decode** bounded by
`[start, end)`, never a raw `0xE8`/`0xE9` byte scan, and the `do_lit` inline
`.quad` is stepped over so the decode stays aligned. Only colon definitions
(`dh_tfa == 0x82`) are measured — CREATE words (constants/variables/buffers) are
data, not optimizer targets, and are skipped.

## Corpus map (transform -> file)

| transform | file | verdict words |
|---|---|---|
| 1 literal fold | `arith-fold.f` | `fold-mix`, `addk` |
| 2 bare-op inline | `bareop-inline.f` | `bare-chain`, `bare-mem` |
| 3 rbp scheduling | `stack-shuffle.f` | `long-arith`, `shuffle-a` |
| 4 TCO | `tco-tail.f` | `relay`, `countdown` |
| 5 stack-op inline | `stackop-inline.f` | `mix-stackops`, `swaps` |
| 6 real-world | `real-fib.f`, `real-mixed.f`, `real-mandel-iter.f` | `fib-iter`, `fact`, `dot-prod`, `str-hash`, `mandel-iter` |

Each file also carries a couple of non-verdict *sanity* stressors (degenerate
shapes the optimizer might special-case) for coverage; verdicts are read from
the composable words above and the real-world files.

## Workflow per optimizer change

1. Implement the kernel transform.
2. `cargo run --bin opt-bench --features opt-metrics` — read the per-word
   before/after. A win shows as `IMPROVED` (calls eliminated / bytes saved /
   tail-jmps gained); an unintended regression fails.
3. `--bless` to re-commit the goldens, in the **same PR** as the kernel change,
   so the metric movement is a reviewed diff.

## Adding a corpus word

Keep it **headless** (no gfx/canvas/window/event), **pure compute** (no `.` /
`type` in the timed path), bounded, and stack-balanced at load (the harness
asserts `depth() == 0`). Use only words in `lib/core.f` or the `PRIMITIVES`
table. A buggy word that writes a wild pointer will crash the harness at load
(the self-check exercises it once) rather than corrupt a measurement — fix the
word. After adding, `--bless` and commit the new golden alongside.
