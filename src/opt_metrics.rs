//! Optimizer measurement harness — static, byte-exact codegen metrics.
//!
//! Every metric here is a pure function of the bytes a word compiled to, read
//! straight out of the in-process JIT region via the existing
//! [`Wf64Session::debug_words`] ranges. No kernel change is required. The point
//! is a deterministic, zero-flake gate: compile the `bench/corpus/*.f` files,
//! measure each word's body, and compare against a checked-in golden so any
//! optimizer change yields a reviewable before/after diff and a CI failure on
//! any wrong-direction move.
//!
//! Counts come from an iced-x86 instruction *decode* bounded by `[start, end)`,
//! never a raw `0xE8`/`0xE9` byte scan (those values alias inside immediates and
//! inside `do_lit`'s inline `.quad`). The one subtlety the decoder must be told
//! about is `do_lit`: the compiler emits `call do_lit` immediately followed by 8
//! bytes of inline literal data — a linear disassembler would mis-decode those 8
//! bytes, so we resolve `do_lit`'s address once (by probing a literal word) and
//! step the decoder past the inline quad whenever we hit it.

use std::collections::{BTreeMap, HashSet};
use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};
use iced_x86::{Code, Decoder, DecoderOptions, FlowControl, Instruction, Mnemonic, OpKind, Register};
use serde::{Deserialize, Serialize};

use crate::Wf64Session;

/// Bumped if the on-disk baseline JSON shape changes.
pub const SCHEMA_VERSION: u32 = 1;

/// Per-word static codegen metrics. The first five fields plus `body_hash` are
/// the gate; `instruction_count` / `rbp_adjust_count` are advisory until the v2
/// stack scheduler lands and is re-blessed.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct WordMetrics {
    /// Compiled body size in bytes = `end - start`. The primary size axis.
    pub byte_length: u64,
    /// Near CALL (`Code::Call_rel32_64`) count — the only call form the compiler
    /// emits. Drops on inline (T2/T5), fold (T1), TCO (T4). Rising = regression.
    pub call_count_E8: u32,
    /// Near JMP (`Code::Jmp_rel32_64`) count. Raw; the TCO predicate is
    /// `tail_is_jmp`, since the kernel also emits 0xE9 for if/loop joins.
    pub jmp_count_E9: u32,
    /// TCO-success predicate: the body's last unconditional control transfer is a
    /// near JMP to this word (self tail-recursion) or to another word — not a
    /// local if/loop label.
    pub tail_is_jmp: bool,
    /// `call do_lit` sequences (CALL whose resolved target == do_lit's address).
    /// Sharpest literal-fold (T1) signal: a missed fold leaves a 13B do_lit
    /// literal + operator CALL.
    pub do_lit_count: u32,
    /// BLAKE3 of the rel32-normalized body (every branch displacement zeroed), a
    /// position-independent "did codegen change at all" tripwire. Report-only.
    pub body_hash: String,
    /// Total decoded instructions (advisory): coarse complement to byte_length,
    /// secondary fingerprint of stack scheduling.
    pub instruction_count: u32,
    /// In-body `add/sub rbp, imm` count (advisory): the v2 scheduler coalesces
    /// per-op stack-pointer adjusts. ~0 pre-optimizer.
    pub rbp_adjust_count: u32,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct FileTotals {
    pub bytes: u64,
    pub instr: u64,
    pub calls: u64,
    pub jmps: u64,
    pub do_lit: u64,
}

/// All of one corpus file's own newly-defined words, plus the guards that keep
/// numbers from two different kernels / two different corpus revisions from ever
/// being compared.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct FileMetrics {
    pub schema_version: u32,
    pub kernel_fingerprint: String,
    pub corpus_file_hash: String,
    pub words: BTreeMap<String, WordMetrics>,
    pub file_totals: FileTotals,
}

/// Outcome of comparing freshly-measured metrics against a committed baseline.
#[derive(Debug)]
pub enum GateOutcome {
    /// No gate metric moved.
    Clean,
    /// Some gate metric strictly improved, none regressed (notes for the report).
    Improved(Vec<String>),
    /// At least one gate metric moved the wrong way (CI-failing messages).
    Regressed(Vec<String>),
    /// Fingerprint / corpus-hash mismatch: cannot compare, must re-bless.
    Stale(String),
}

// ---------------------------------------------------------------------------
// Decoding
// ---------------------------------------------------------------------------

/// Zero a near (rel32) or short (rel8) branch displacement in the normalization
/// buffer so `body_hash` is independent of where the word landed in memory.
fn zero_rel(norm: &mut [u8], pos: usize, len: usize, near: bool) {
    let rel = if near { 4 } else { 1 };
    if len >= rel {
        let end = pos + len;
        for b in &mut norm[end - rel..end] {
            *b = 0;
        }
    }
}

/// Decode one word body `[start, end)` (absolute in-process addresses) into its
/// static metrics. `do_lit_addr`, if known, lets us skip `do_lit`'s 8-byte inline
/// literal so the decode stays aligned.
pub fn decode_word(start: u64, end: u64, do_lit_addr: Option<u64>) -> WordMetrics {
    let len = (end - start) as usize;
    // SAFETY: start..end is a live, executable range inside the session's JIT
    // region (it is the word we are about to be able to call). We only read it.
    let code: &[u8] = unsafe { std::slice::from_raw_parts(start as *const u8, len) };
    let mut norm = code.to_vec();

    let mut dec = Decoder::with_ip(64, code, start, DecoderOptions::NONE);
    let mut insn = Instruction::default();

    let mut call_count = 0u32;
    let mut jmp_count = 0u32;
    let mut do_lit_count = 0u32;
    let mut instr_count = 0u32;
    let mut rbp_adjust = 0u32;
    let mut last_uncond_is_jmp = false;
    let mut last_uncond_target: Option<u64> = None;

    while dec.can_decode() {
        let pos = dec.position();
        dec.decode_out(&mut insn);
        if insn.is_invalid() {
            // Decoder already advanced one byte; just resync.
            continue;
        }
        instr_count += 1;
        let ilen = insn.len();

        // rbp data-stack-pointer adjust (advisory T3 fingerprint).
        if matches!(insn.mnemonic(), Mnemonic::Add | Mnemonic::Sub)
            && insn.op0_kind() == OpKind::Register
            && insn.op0_register() == Register::RBP
            && matches!(
                insn.op1_kind(),
                OpKind::Immediate8
                    | OpKind::Immediate8to64
                    | OpKind::Immediate32
                    | OpKind::Immediate32to64
            )
        {
            rbp_adjust += 1;
        }

        match insn.code() {
            Code::Call_rel32_64 => {
                call_count += 1;
                zero_rel(&mut norm, pos, ilen, true);
                let target = insn.near_branch_target();
                last_uncond_is_jmp = false;
                last_uncond_target = Some(target);
                if Some(target) == do_lit_addr {
                    do_lit_count += 1;
                    // Step past the 8-byte inline literal that follows the call.
                    let skip = (dec.position() + 8).min(len);
                    let _ = dec.set_position(skip);
                    dec.set_ip(start + skip as u64);
                }
            }
            Code::Jmp_rel32_64 => {
                jmp_count += 1;
                zero_rel(&mut norm, pos, ilen, true);
                last_uncond_is_jmp = true;
                last_uncond_target = Some(insn.near_branch_target());
            }
            Code::Jmp_rel8_64 => {
                zero_rel(&mut norm, pos, ilen, false);
                last_uncond_is_jmp = true;
                last_uncond_target = Some(insn.near_branch_target());
            }
            _ => {
                if insn.flow_control() == FlowControl::ConditionalBranch {
                    // 0F 8x rel32 (len >= 6) vs 7x rel8 (len 2).
                    zero_rel(&mut norm, pos, ilen, ilen >= 5);
                }
            }
        }
    }

    // tail_is_jmp: the final unconditional transfer is a JMP that leaves this
    // word — to itself (TCO'd self-recursion, target == start) or to another
    // word (target outside [start, end)). A backward loop jmp targets a local
    // label strictly inside the body and does not count.
    let tail_is_jmp = last_uncond_is_jmp
        && match last_uncond_target {
            Some(t) => t == start || t < start || t >= end,
            None => false,
        };

    WordMetrics {
        byte_length: end - start,
        call_count_E8: call_count,
        jmp_count_E9: jmp_count,
        tail_is_jmp,
        do_lit_count,
        body_hash: blake3::hash(&norm).to_hex().to_string(),
        instruction_count: instr_count,
        rbp_adjust_count: rbp_adjust,
    }
}

/// Resolve `do_lit`'s absolute address by compiling a probe word that pushes a
/// large literal and reading the target of the `call do_lit` it emits. Returns
/// `Ok(None)` if the expected `call rel32 ; .quad <value>` shape isn't found (in
/// which case do_lit_count degrades to advisory; call_count + byte_length still
/// capture the fold win). Leaves the session reset.
pub fn probe_do_lit(session: &mut Wf64Session) -> Result<Option<u64>> {
    const PROBE: i64 = 999_983; // prime, large enough to force a do_lit literal
    session.reset();
    session.eval(": __dolit_probe 999983 ;\n").context("probe eval")?;
    let found = session
        .debug_words()
        .into_iter()
        .find(|(n, _, _)| n == "__dolit_probe");
    let result = if let Some((_, start, end)) = found {
        let len = (end - start) as usize;
        let code: &[u8] = unsafe { std::slice::from_raw_parts(start as *const u8, len) };
        let mut dec = Decoder::with_ip(64, code, start, DecoderOptions::NONE);
        if dec.can_decode() {
            let insn = dec.decode();
            if insn.code() == Code::Call_rel32_64 {
                let off = dec.position(); // just past the call
                if off + 8 <= len {
                    let mut q = [0u8; 8];
                    q.copy_from_slice(&code[off..off + 8]);
                    if i64::from_le_bytes(q) == PROBE {
                        Some(insn.near_branch_target())
                    } else {
                        None
                    }
                } else {
                    None
                }
            } else {
                None
            }
        } else {
            None
        }
    } else {
        None
    };
    session.reset();
    Ok(result)
}

// ---------------------------------------------------------------------------
// File measurement
// ---------------------------------------------------------------------------

fn manifest_dir() -> &'static Path {
    Path::new(env!("CARGO_MANIFEST_DIR"))
}

/// BLAKE3 over every `kernel/*.masm` source file (name + bytes, sorted). Stamped
/// into each baseline so numbers from two different kernels are never compared.
pub fn kernel_fingerprint() -> Result<String> {
    let kdir = manifest_dir().join("kernel");
    let mut files: Vec<PathBuf> = std::fs::read_dir(&kdir)
        .with_context(|| format!("read kernel dir {}", kdir.display()))?
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| p.extension().map(|x| x == "masm").unwrap_or(false))
        .collect();
    files.sort();
    let mut hasher = blake3::Hasher::new();
    for f in &files {
        hasher.update(f.file_name().unwrap().to_string_lossy().as_bytes());
        hasher.update(&std::fs::read(f)?);
    }
    Ok(hasher.finalize().to_hex().to_string())
}

/// Measure every word a corpus file defines. Resets the session, snapshots the
/// word set, loads the file, asserts the load left the stack balanced, then
/// attributes only the newly-created words and decodes each body.
pub fn measure_file(
    session: &mut Wf64Session,
    path: &Path,
    do_lit_addr: Option<u64>,
) -> Result<FileMetrics> {
    session.reset();
    let before: HashSet<String> = session
        .debug_words()
        .into_iter()
        .map(|(n, _, _)| n)
        .collect();

    session
        .load_source_file(path)
        .with_context(|| format!("compile corpus {}", path.display()))?;

    if session.depth() != 0 {
        bail!(
            "corpus {} left {} cell(s) on the data stack at load — must be balanced",
            path.display(),
            session.depth()
        );
    }

    let src = std::fs::read(path)?;
    let corpus_file_hash = blake3::hash(&src).to_hex().to_string();

    // Only colon definitions (dh_tfa == 0x82) are real code the optimizer
    // transforms; CREATE-flavoured words (constants/variables/buffers, 0x91)
    // have data bodies and are skipped.
    const TFA_COLON: u8 = 0x82;
    let mut words = BTreeMap::new();
    let mut totals = FileTotals::default();
    for (name, start, end, tfa) in session.debug_words_typed() {
        if before.contains(&name) || end <= start || tfa != TFA_COLON {
            continue;
        }
        let m = decode_word(start, end, do_lit_addr);
        totals.bytes += m.byte_length;
        totals.instr += m.instruction_count as u64;
        totals.calls += m.call_count_E8 as u64;
        totals.jmps += m.jmp_count_E9 as u64;
        totals.do_lit += m.do_lit_count as u64;
        words.insert(name, m);
    }

    Ok(FileMetrics {
        schema_version: SCHEMA_VERSION,
        kernel_fingerprint: kernel_fingerprint()?,
        corpus_file_hash,
        words,
        file_totals: totals,
    })
}

// ---------------------------------------------------------------------------
// Corpus / baseline locations + IO
// ---------------------------------------------------------------------------

pub fn corpus_dir() -> PathBuf {
    manifest_dir().join("bench").join("corpus")
}

pub fn baseline_dir() -> PathBuf {
    manifest_dir().join("bench").join("baseline")
}

/// All `bench/corpus/*.f` files, sorted for stable ordering.
pub fn corpus_files() -> Result<Vec<PathBuf>> {
    let dir = corpus_dir();
    let mut v: Vec<PathBuf> = std::fs::read_dir(&dir)
        .with_context(|| format!("read corpus dir {}", dir.display()))?
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| p.extension().map(|x| x == "f").unwrap_or(false))
        .collect();
    v.sort();
    Ok(v)
}

pub fn baseline_path(corpus: &Path) -> PathBuf {
    let base = corpus.file_stem().unwrap().to_string_lossy().to_string();
    baseline_dir().join(format!("{base}.json"))
}

pub fn write_baseline(path: &Path, m: &FileMetrics) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let json = serde_json::to_string_pretty(m)?;
    std::fs::write(path, json + "\n")?;
    Ok(())
}

pub fn read_baseline(path: &Path) -> Result<FileMetrics> {
    let s = std::fs::read_to_string(path)
        .with_context(|| format!("read baseline {}", path.display()))?;
    Ok(serde_json::from_str(&s)?)
}

// ---------------------------------------------------------------------------
// Diff / gate
// ---------------------------------------------------------------------------

fn reg(name: &str, metric: &str, base: u64, live: u64) -> String {
    format!("{name}: {metric} REGRESSED {base} -> {live}")
}

/// Compare freshly-measured `live` metrics against a committed `baseline`. Gate
/// metrics must never move the wrong way; improvements are reported, not failed
/// (unless `strict`).
pub fn compare(live: &FileMetrics, baseline: &FileMetrics, strict: bool) -> GateOutcome {
    if live.kernel_fingerprint != baseline.kernel_fingerprint {
        return GateOutcome::Stale("kernel changed — regenerate baseline (opt-bench --bless)".into());
    }
    if live.corpus_file_hash != baseline.corpus_file_hash {
        return GateOutcome::Stale("corpus source changed — regenerate baseline (opt-bench --bless)".into());
    }

    let mut regress = Vec::new();
    let mut improve = Vec::new();

    for name in live.words.keys() {
        if !baseline.words.contains_key(name) {
            regress.push(format!("new word `{name}` absent from baseline — re-bless"));
        }
    }
    for name in baseline.words.keys() {
        if !live.words.contains_key(name) {
            regress.push(format!("baseline word `{name}` dropped from corpus — re-bless"));
        }
    }

    for (name, l) in &live.words {
        let Some(b) = baseline.words.get(name) else { continue };
        if l.byte_length > b.byte_length {
            regress.push(reg(name, "byte_length", b.byte_length, l.byte_length));
        } else if l.byte_length < b.byte_length {
            improve.push(format!("{name}: byte_length {} -> {} (-{})", b.byte_length, l.byte_length, b.byte_length - l.byte_length));
        }
        if l.call_count_E8 > b.call_count_E8 {
            regress.push(reg(name, "call_count_E8", b.call_count_E8 as u64, l.call_count_E8 as u64));
        } else if l.call_count_E8 < b.call_count_E8 {
            improve.push(format!("{name}: call_count_E8 {} -> {} (-{})", b.call_count_E8, l.call_count_E8, b.call_count_E8 - l.call_count_E8));
        }
        if l.jmp_count_E9 > b.jmp_count_E9 {
            regress.push(reg(name, "jmp_count_E9", b.jmp_count_E9 as u64, l.jmp_count_E9 as u64));
        }
        if l.do_lit_count > b.do_lit_count {
            regress.push(reg(name, "do_lit_count", b.do_lit_count as u64, l.do_lit_count as u64));
        } else if l.do_lit_count < b.do_lit_count {
            improve.push(format!("{name}: do_lit_count {} -> {} (-{})", b.do_lit_count, l.do_lit_count, b.do_lit_count - l.do_lit_count));
        }
        if b.tail_is_jmp && !l.tail_is_jmp {
            regress.push(format!("{name}: tail_is_jmp true -> false (TCO lost)"));
        } else if !b.tail_is_jmp && l.tail_is_jmp {
            improve.push(format!("{name}: tail_is_jmp false -> true (TCO gained)"));
        }
    }

    if !regress.is_empty() {
        GateOutcome::Regressed(regress)
    } else if !improve.is_empty() {
        if strict {
            let mut msgs = vec!["--strict: baseline understates the optimizer — re-bless".to_string()];
            msgs.extend(improve);
            GateOutcome::Regressed(msgs)
        } else {
            GateOutcome::Improved(improve)
        }
    } else {
        GateOutcome::Clean
    }
}
