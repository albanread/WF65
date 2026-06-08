//! Rust runtime functions the kernel calls via `@extern`.
//!
//! All I/O is routed through a thread-local `Session` (see `lib.rs`).
//! Tests use a session whose `input`/`output` are in-memory buffers;
//! the interactive REPL wrapper uses a session backed by stdin/stdout.
//!
//! Routing through a session-scoped buffer (instead of writing directly
//! to stdout) is what makes the test harness possible: each `#[test]`
//! owns its session, feeds input, reads output, and inspects the data
//! stack — no global state, no pipes, no temp files.

use std::cell::RefCell;
use std::io::Write;

fn normalize_float_token(text: &str) -> Option<String> {
    if text.is_empty() {
        return None;
    }
    if !text.bytes().any(|b| b == b'.' || b == b'e' || b == b'E') {
        return None;
    }

    let mut out = text.trim().to_string();
    if out.is_empty() {
        return None;
    }

    if let Some(pos) = out.find(['e', 'E']) {
        if pos + 1 == out.len() {
            out.push('0');
        } else {
            let bytes = out.as_bytes();
            if (bytes[pos + 1] == b'+' || bytes[pos + 1] == b'-') && pos + 2 == out.len() {
                out.push('0');
            }
        }
    }

    Some(out)
}

/// I/O backing for one session. Tests use the `Buffered` variant; the
/// interactive REPL uses `Live`.
pub enum Io {
    /// In-memory buffers — for tests. `input` is consumed by
    /// `rt_read_line`, `output` accumulates bytes written by emit/type.
    Buffered {
        input: Vec<u8>,
        in_cursor: usize,
        pending_key: Option<u8>,
        output: Vec<u8>,
    },
    /// Real stdin/stdout — for the interactive REPL.
    Live {
        pending_key: Option<u8>,
    },
}

impl Io {
    pub fn new_buffered() -> Self {
        Io::Buffered { input: Vec::new(), in_cursor: 0, pending_key: None, output: Vec::new() }
    }
}

#[cfg(windows)]
unsafe extern "C" {
    fn _kbhit() -> i32;
    fn _getwch() -> u16;
}

thread_local! {
    /// The currently-active session's I/O. `Wf64Session::enter` swaps
    /// itself in for the duration of an `eval`; outside of that it's
    /// `None` and runtime calls will panic.
    static CURRENT_IO: RefCell<Option<Io>> = const { RefCell::new(None) };
}

/// Install `io` as the current session's I/O, run `f`, swap out and
/// return both the function's result and the (possibly-mutated) Io.
/// Restores any previously-installed Io on the way out.
///
/// Not panic-safe: if `f` panics, `CURRENT_IO` is left holding the new
/// `Io` and the test thread dies. That's acceptable for harness code —
/// production paths shouldn't panic past this point.
pub fn with_io<R>(io: Io, f: impl FnOnce() -> R) -> (R, Io) {
    let prev = CURRENT_IO.with(|cell| cell.replace(Some(io)));
    let result = f();
    let io_after = CURRENT_IO
        .with(|cell| cell.replace(prev))
        .expect("CURRENT_IO must be Some inside with_io");
    (result, io_after)
}

/// Quick accessor used by runtime functions: panic if there's no
/// session bound (that would indicate a logic bug in the harness).
fn with_current_io<R>(f: impl FnOnce(&mut Io) -> R) -> R {
    CURRENT_IO.with(|cell| {
        let mut borrow = cell.borrow_mut();
        let io = borrow
            .as_mut()
            .expect("WF64 runtime called outside of a Wf64Session::eval");
        f(io)
    })
}

/// Print a signed cell in the current BASE, followed by a single
/// space, no newline.  `base` is read by the kernel from
/// `UP + user_BASE` and passed in as arg 2 — keeps this function
/// stateless and dodges the need for the runtime to know the UP
/// layout.  Falls back to decimal (base 10) if the kernel passes
/// an out-of-range value (< 2 or > 36).
#[no_mangle]
pub extern "C" fn rt_print_int(n: u64, base: u64) -> u64 {
    let s = n as i64;
    let b = if (2..=36).contains(&base) { base as u32 } else { 10 };
    let mut buf = String::with_capacity(24);
    if b == 10 {
        // Hot path: signed-decimal via the std formatter is faster
        // than the manual radix-conversion loop below, and matches
        // what callers used to see exactly.
        use std::fmt::Write;
        let _ = write!(&mut buf, "{s} ");
    } else {
        // Generic signed radix conversion.  Negative numbers print
        // as `-MAGNITUDE` in the chosen base; matches the high-level
        // `.` in core.f which does the same via `<# … sign #>`.
        let (sign, mag) = if s < 0 {
            ('-', (s as i128).unsigned_abs())
        } else {
            (' ', s as u128)
        };
        let mut digits: Vec<u8> = Vec::with_capacity(24);
        let mut v = mag;
        if v == 0 {
            digits.push(b'0');
        } else {
            while v > 0 {
                let d = (v % b as u128) as u8;
                digits.push(if d < 10 { b'0' + d } else { b'A' + (d - 10) });
                v /= b as u128;
            }
        }
        if sign == '-' {
            buf.push('-');
        }
        for &d in digits.iter().rev() {
            buf.push(d as char);
        }
        buf.push(' ');
    }
    write_bytes(buf.as_bytes());
    0
}

/// Print the live Forth stack without consuming it.
///
/// The kernel passes its internal TOS cache plus DSP/SP0 so we can
/// reconstruct the logical stack shape without forcing a restart or a
/// spill/reload cycle through forth_main.
#[no_mangle]
pub extern "C" fn rt_dot_s(tos: u64, dsp: u64, sp0: u64, rsp: u64) -> u64 {
    let depth = if dsp > sp0 {
        0usize
    } else {
        ((sp0 - dsp) / 8 + 1) as usize
    };

    if depth == 0 {
        write_bytes(format!("[empty sp={dsp:#x} rp={rsp:#x}]").as_bytes());
        return 0;
    }

    write_bytes(format!("[{depth} sp={dsp:#x} rp={rsp:#x}] ").as_bytes());
    write_bytes(format!("{} ", tos as i64).as_bytes());
    for index in 1..depth {
        let addr = dsp + (index as u64 - 1) * 8;
        let value = unsafe { (addr as *const i64).read_unaligned() };
        write_bytes(format!("{value} ").as_bytes());
    }
    0
}

/// Forth-tuned breakpoint dump.
///
/// Called by `brk` / `int3` before the INT 3 instruction so the human
/// sees a readable Forth state before the raw VEH register dump.
///
/// Arguments (Win64, 5-arg):
///   tos   — cached TOS register
///   dsp   — data stack pointer (points at NOS)
///   sp0   — initial DSP (base of data stack)
///   rsp   — Forth return stack pointer at the point of the breakpoint
///   up    — user area pointer (= rsp_top since region layout makes them equal)
///
/// # Safety
/// All pointers come from the live JIT session arena.
#[no_mangle]
pub extern "C" fn rt_forth_brk(tos: u64, dsp: u64, sp0: u64, rsp: u64, up: u64) -> u64 {
    let mut out = String::with_capacity(512);

    out.push_str("\n=== Forth Breakpoint ==================================================\n");

    // ── Data stack ──────────────────────────────────────────────────
    let depth = if dsp > sp0 {
        0usize
    } else {
        ((sp0 - dsp) / 8 + 1) as usize
    };
    out.push_str(&format!("Data stack [{depth}]:\n"));
    if depth == 0 {
        out.push_str("  (empty)\n");
    } else {
        out.push_str(&format!("  TOS: {:>20}  {:#018x}\n", tos as i64, tos));
        for i in 1..depth {
            let addr = dsp + (i as u64 - 1) * 8;
            let v = unsafe { (addr as *const u64).read_unaligned() };
            out.push_str(&format!("  {:>3}: {:>20}  {:#018x}\n", i, v as i64, v));
        }
    }

    // ── Return stack ────────────────────────────────────────────────
    // rsp_top == up (region layout: return stack grows from up downward).
    let rstack_depth = if rsp >= up { 0usize } else { ((up - rsp) / 8) as usize };
    let rstack_show  = rstack_depth.min(16);
    out.push_str(&format!("Return stack [{rstack_depth} cells, showing {rstack_show}]:\n"));
    for i in 0..rstack_show {
        let addr = rsp + i as u64 * 8;
        let v = unsafe { (addr as *const u64).read_unaligned() };
        out.push_str(&format!("  [{i}]: {v:#018x}\n"));
    }

    // ── Key user variables ───────────────────────────────────────────
    // Safety: up points into the live session user area.
    let uread = |off: u64| unsafe { *((up + off) as *const u64) };
    let base         = uread(0x00);
    let state        = uread(0x08);
    let latest       = uread(0x10);
    let here         = uread(0x18);
    let latestxt     = uread(0x78);
    let handler      = uread(0x80);
    let throw_code   = uread(0x88);
    let current      = uread(0x1500);
    let forth_wid    = uread(0x1508);
    let order_count  = uread(0x1510);
    out.push_str("User variables:\n");
    out.push_str(&format!("  BASE={base:<5}  STATE={state:<3}  HERE={here:#x}  LATEST={latest:#x}\n"));
    out.push_str(&format!("  LATESTXT={latestxt:#x}  HANDLER={handler:#x}  THROW={throw_code}\n"));
    out.push_str(&format!("  CURRENT={current:#x}  FORTH-WID={forth_wid:#x}  ORDER={order_count}\n"));
    let show_ctx = (order_count as usize).min(16);
    for i in 0..show_ctx {
        let wid = uread(0x1528 + i as u64 * 8);
        out.push_str(&format!("  CONTEXT[{i}]={wid:#x}\n"));
    }

    out.push_str("=======================================================================\n");
    write_bytes(out.as_bytes());
    0
}

/// Per-word trace hook, called from the interpreter before each word executes.
///
/// Arguments (Win64, 4-arg):
///   nt    — name token (pointer to counted string: length byte then chars)
///   tos   — current TOS
///   dsp   — current DSP (points at NOS)
///   sp0   — initial DSP (base of data stack)
///
/// # Safety
/// `nt` points into the live JIT dictionary arena.
#[no_mangle]
pub extern "C" fn rt_forth_trace(nt: u64, tos: u64, dsp: u64, sp0: u64) -> u64 {
    let name = unsafe {
        let len = *(nt as *const u8) as usize;
        let bytes = std::slice::from_raw_parts((nt + 1) as *const u8, len);
        std::str::from_utf8(bytes).unwrap_or("<?>")
    };

    let depth = if dsp > sp0 {
        0usize
    } else {
        ((sp0 - dsp) / 8 + 1) as usize
    };

    let mut out = format!("» {name:<16}  (");
    if depth == 0 {
        out.push_str(" empty");
    } else {
        out.push_str(&format!(" {}", tos as i64));
        for i in 1..depth {
            let addr = dsp + (i as u64 - 1) * 8;
            let v = unsafe { (addr as *const i64).read_unaligned() };
            out.push_str(&format!(" {v}"));
        }
    }
    out.push_str(" )\n");
    write_bytes(out.as_bytes());
    0
}

/// Write one byte to current output.
#[no_mangle]
pub extern "C" fn rt_emit(ch: u64) -> u64 {
    let byte = ch as u8;
    write_bytes(&[byte]);
    0
}

/// Write `len` bytes from `addr` to current output.
///
/// # Safety
/// The JITed `type` primitive guarantees `[addr, addr+len)` is readable.
#[no_mangle]
pub extern "C" fn rt_type(addr: u64, len: u64) -> u64 {
    if len == 0 {
        return 0;
    }
    let slice = unsafe { std::slice::from_raw_parts(addr as *const u8, len as usize) };
    write_bytes(slice);
    0
}

/// Cooperative-bye: this no longer terminates the process. The kernel's
/// `bye` primitive sets `user_BYE_REQ` directly and quit returns
/// cleanly. The interactive REPL wrapper turns that clean return into a
/// `process::exit` itself.
///
/// Kept exported because the win32 bindings list it; harmless no-op now.
#[no_mangle]
pub extern "C" fn rt_bye(_code: u64) -> u64 {
    0
}

/// Read one line of input into `buf` (at most `cap` bytes, terminator
/// not included).
///
/// Return value:
///   * `0..=cap` — number of bytes written (0 means empty line — *not*
///     end of input).
///   * `u64::MAX` (all 1s) — end of input. The kernel's `accept`
///     forwards this to `quit`, which treats it as an implicit `bye`.
///
/// This separation lets the REPL handle blank lines correctly while
/// still terminating cleanly on stdin EOF or end of a buffered input.
///
/// In `Buffered` mode reads from the session's in-memory input buffer.
/// In `Live` mode reads from stdin via `BufRead::read_line`, which
/// works equally on consoles and on redirected stdin.
///
/// # Safety
/// The kernel's `accept` guarantees `[buf, buf+cap)` is writable.
#[no_mangle]
pub extern "C" fn rt_read_line(buf: u64, cap: u64) -> u64 {
    const EOF: u64 = u64::MAX;
    let cap = cap as usize;
    if cap == 0 {
        return EOF;
    }
    let dst = unsafe { std::slice::from_raw_parts_mut(buf as *mut u8, cap) };

    with_current_io(|io| match io {
        Io::Buffered { input, in_cursor, .. } => {
            if *in_cursor >= input.len() {
                return EOF;
            }
            // Find next LF (or end-of-input).
            let start = *in_cursor;
            let rest = &input[start..];
            let lf_off = rest.iter().position(|&b| b == b'\n');
            let line_end = match lf_off {
                Some(off) => start + off,
                None => input.len(),
            };
            let n = (line_end - start).min(cap);
            dst[..n].copy_from_slice(&input[start..start + n]);
            // Advance cursor past the line + its LF (if any).
            *in_cursor = match lf_off {
                Some(_) => line_end + 1,
                None => line_end,
            };
            // Strip a trailing CR (handles CRLF inputs).
            let mut count = n as u64;
            if count > 0 && dst[count as usize - 1] == b'\r' {
                count -= 1;
            }
            count
        }
        Io::Live { .. } => {
            use std::io::{self, BufRead};
            let stdin = io::stdin();
            let mut handle = stdin.lock();
            let mut line = String::new();
            match handle.read_line(&mut line) {
                Ok(0) => EOF,
                Ok(_) => {
                    let bytes = line.as_bytes();
                    let mut len = bytes.len();
                    if len > 0 && bytes[len - 1] == b'\n' {
                        len -= 1;
                    }
                    if len > 0 && bytes[len - 1] == b'\r' {
                        len -= 1;
                    }
                    let n = len.min(cap);
                    dst[..n].copy_from_slice(&bytes[..n]);
                    n as u64
                }
                Err(_) => EOF,
            }
        }
    })
}

#[no_mangle]
pub extern "C" fn rt_read_key() -> u64 {
    with_current_io(|io| match io {
        Io::Buffered { input, in_cursor, pending_key, .. } => {
            if let Some(byte) = pending_key.take() {
                return byte as u64;
            }
            if *in_cursor >= input.len() {
                return 0;
            }
            let byte = input[*in_cursor];
            *in_cursor += 1;
            byte as u64
        }
        Io::Live { pending_key } => {
            if let Some(byte) = pending_key.take() {
                return byte as u64;
            }
            use std::io::Read;

            let stdin = std::io::stdin();
            let mut handle = stdin.lock();
            let mut buf = [0u8; 1];
            match handle.read_exact(&mut buf) {
                Ok(()) => buf[0] as u64,
                Err(_) => 0,
            }
        }
    })
}

#[no_mangle]
pub extern "C" fn rt_key_q() -> u64 {
    with_current_io(|io| match io {
        Io::Buffered { input, in_cursor, pending_key, .. } => {
            if pending_key.is_some() {
                return u64::MAX;
            }
            if *in_cursor >= input.len() {
                return 0;
            }
            *pending_key = Some(input[*in_cursor]);
            *in_cursor += 1;
            u64::MAX
        }
        Io::Live { pending_key } => {
            if pending_key.is_some() {
                return u64::MAX;
            }

            #[cfg(windows)]
            unsafe {
                if _kbhit() == 0 {
                    return 0;
                }
                let wide = _getwch();
                if wide <= 0xFF {
                    *pending_key = Some(wide as u8);
                    return u64::MAX;
                }
            }

            0
        }
    })
}

#[no_mangle]
pub extern "C" fn rt_to_float(addr: u64, len: u64, out_bits: u64) -> u64 {
    if len == 0 || out_bits == 0 {
        return 0;
    }

    let bytes = unsafe { std::slice::from_raw_parts(addr as *const u8, len as usize) };
    let Ok(text) = std::str::from_utf8(bytes) else {
        return 0;
    };
    let Some(normalized) = normalize_float_token(text) else {
        return 0;
    };
    let Ok(value) = normalized.parse::<f64>() else {
        return 0;
    };

    unsafe { (out_bits as *mut u64).write_unaligned(value.to_bits()) };
    u64::MAX
}

// ── File include support ──────────────────────────────────────────────
//
// `included` ( c-addr u -- ) reads a Forth source file and evaluates it.
// We implement this without re-entering the JIT by reading the file into
// a Rust-owned Vec and exposing its address+len to Forth, which then calls
// the existing `evaluate` word. A stack allows nested includes: each call
// to `rt_slurp_file` pushes a new Vec; `rt_slurp_pop` releases the top.

thread_local! {
    static SLURP_STACK: RefCell<Vec<Vec<u8>>> = RefCell::new(Vec::new());
}

/// Read file at path (c-addr u) into a Rust-owned Vec, push it onto
/// SLURP_STACK, and return a pointer to the content (stable until
/// `rt_slurp_pop` is called). Returns 0 on any error (file not found,
/// UTF-8 etc.).
#[no_mangle]
pub extern "C" fn rt_slurp_file(path_addr: u64, path_len: u64) -> u64 {
    let path_bytes =
        unsafe { std::slice::from_raw_parts(path_addr as *const u8, path_len as usize) };
    let Ok(path_str) = std::str::from_utf8(path_bytes) else {
        return 0;
    };
    match std::fs::read(path_str.trim()) {
        Ok(bytes) => SLURP_STACK.with(|s| {
            let mut stack = s.borrow_mut();
            stack.push(bytes);
            stack.last().map(|v| v.as_ptr() as u64).unwrap_or(0)
        }),
        Err(_) => 0,
    }
}

/// Return the byte length of the top slurped file (0 if stack is empty).
#[no_mangle]
pub extern "C" fn rt_slurp_len() -> u64 {
    SLURP_STACK.with(|s| {
        s.borrow().last().map(|v| v.len() as u64).unwrap_or(0)
    })
}

/// Pop (and free) the top slurped file from the stack.
#[no_mangle]
pub extern "C" fn rt_slurp_pop() -> u64 {
    SLURP_STACK.with(|s| {
        s.borrow_mut().pop();
        0
    })
}

// ── GC runtime functions ─────────────────────────────────────────────
//
// V1b plumbing.  These get called from the kernel's GC primitives
// (`vec-alloc-floats!`, `(gc)`, …) via the same @extern mechanism
// the rest of the kernel uses for I/O and number parsing.
//
// All addresses are absolute (the kernel computes them from UP +
// offset where needed).  Return value: 0 on success, u64::MAX on
// error.

use crate::gc;

/// Threshold above which an allocation is routed through paged_gc's
/// large-object path (which can fail for lack of contiguous free
/// pages even when the total free count is enough — see #9 in
/// forth_gc_needs.md).  Mirrors `newgc_core::page_heap::PAGE_SIZE_CELLS`.
const PAGE_SIZE_CELLS: u64 = 8192;

/// Allocate a FloatVec of `n_cells` payload cells, store the tagged
/// pointer at `*slot_addr`.  `slot_addr` is the absolute address of
/// a HEAPPTR slot (caller's responsibility to ensure it's inside
/// `user_HEAPPTR_REGION`).
///
/// On allocation failure for a large object (> 1 page payload), we
/// run a `collect_full` (Tenured compaction) and retry once before
/// surfacing failure — this rescues the fragmentation-defeat case
/// where total free pages are enough but no contiguous run is.
///
/// Returns 0 on success, u64::MAX on allocation failure (heap full
/// even after compaction, or `n_cells > u32::MAX`).
#[no_mangle]
pub extern "C" fn rt_vec_alloc_floats(up: u64, n_cells: u64, slot_addr: u64) -> u64 {
    if n_cells > u32::MAX as u64 {
        eprintln!("vec-alloc-floats!: requested length {n_cells} exceeds u32::MAX");
        return u64::MAX;
    }
    let n = n_cells as u32;
    if let Some(tagged) = gc::alloc_floatvec(n) {
        unsafe { *(slot_addr as *mut u64) = tagged; }
        return 0;
    }
    if n_cells > PAGE_SIZE_CELLS {
        let regions = gc_root_regions(up);
        unsafe { gc::collect_full(&regions); }
        if let Some(tagged) = gc::alloc_floatvec(n) {
            unsafe { *(slot_addr as *mut u64) = tagged; }
            return 0;
        }
    }
    eprintln!("vec-alloc-floats!: out of GC heap (requested {n_cells} cells)");
    u64::MAX
}

/// Allocate a RefVec of `n_cells` (all initialised to nil), store
/// the tagged pointer at `*slot_addr`.  Fragmentation-retry path
/// mirrors `rt_vec_alloc_floats`.
#[no_mangle]
pub extern "C" fn rt_vec_alloc_refs(up: u64, n_cells: u64, slot_addr: u64) -> u64 {
    if n_cells > u32::MAX as u64 {
        eprintln!("vec-alloc-refs!: requested length {n_cells} exceeds u32::MAX");
        return u64::MAX;
    }
    let n = n_cells as u32;
    if let Some(tagged) = gc::alloc_refvec(n) {
        unsafe { *(slot_addr as *mut u64) = tagged; }
        return 0;
    }
    if n_cells > PAGE_SIZE_CELLS {
        let regions = gc_root_regions(up);
        unsafe { gc::collect_full(&regions); }
        if let Some(tagged) = gc::alloc_refvec(n) {
            unsafe { *(slot_addr as *mut u64) = tagged; }
            return 0;
        }
    }
    eprintln!("vec-alloc-refs!: out of GC heap (requested {n_cells} cells)");
    u64::MAX
}

/// Read the (base, next) pair for both GC root regions —
/// HEAPPTR and LITERAL — from the user area.  Returns
/// `(heapptr_pair, literal_pair)`.  Either pair may be (b, b)
/// meaning empty.
fn gc_root_regions(up: u64) -> [(u64, u64); 2] {
    let heapptr_next = unsafe { *((up + crate::USER_HEAPPTR_NEXT) as *const u64) };
    let heapptr_base = up + crate::USER_HEAPPTR_BASE;
    let literal_next = unsafe { *((up + crate::USER_LITERAL_NEXT) as *const u64) };
    let literal_base = up + crate::USER_LITERAL_BASE;
    [(heapptr_base, heapptr_next), (literal_base, literal_next)]
}

/// Run a major GC.  Walks BOTH the HEAPPTR region and the LITERAL
/// region as roots (V2s: compile-time string literals live in the
/// second region).  Caller passes `up` (the UP register value).
#[no_mangle]
pub extern "C" fn rt_gc_collect(up: u64) -> u64 {
    let regions = gc_root_regions(up);
    for (base, next) in regions {
        if next < base {
            eprintln!("(gc): root region NEXT (0x{next:x}) is below BASE \
                       (0x{base:x}) — probable user-area corruption");
            return u64::MAX;
        }
    }
    unsafe { gc::collect_major(&regions); }
    0
}

/// Run a minor GC.  Same root set as `rt_gc_collect`.
#[no_mangle]
pub extern "C" fn rt_gc_collect_minor(up: u64) -> u64 {
    let regions = gc_root_regions(up);
    for (base, next) in regions {
        if next < base {
            return u64::MAX;
        }
    }
    unsafe { gc::collect_minor(&regions); }
    0
}

/// True (returns 1) when paged_gc's allocation budget has been
/// exhausted and the next allocation should be preceded by a
/// minor GC.  Retained for diagnostics / future opt-out callers;
/// the kernel allocators now go through `rt_gc_auto_step` instead.
///
/// Returns 0 when no collection is needed.  Never errors.
#[no_mangle]
pub extern "C" fn rt_gc_should_collect() -> u64 {
    if gc::should_collect() { 1 } else { 0 }
}

/// Combined auto-trigger: if `should_collect()` is true, run
/// `collect_auto` (which chooses minor vs major based on tenure
/// pressure — see docs/forth_gc_needs.md item #7).  Otherwise
/// no-op.  Folds the previous "rt_gc_should_collect + maybe
/// rt_gc_collect_minor" two-call pattern into one extern call
/// from kernel-side allocators.
///
/// Returns 0 always; the auto-cycle's outcome is observable
/// indirectly via `gc-cycle`.
#[no_mangle]
pub extern "C" fn rt_gc_auto_step(up: u64) -> u64 {
    if !gc::should_collect() {
        return 0;
    }
    let regions = gc_root_regions(up);
    unsafe { gc::collect_auto(&regions); }
    0
}

/// Full stop-the-world collection — compacts Tenured.  Called by
/// the large-object alloc retry path when `try_alloc_large`
/// failed for lack of contiguous free pages (paged_gc's page
/// allocator is linear-scan; scattered free pages can defeat a
/// multi-page request even when the total free count is enough).
/// Per docs/forth_gc_needs.md item #9.
#[no_mangle]
pub extern "C" fn rt_gc_collect_full(up: u64) -> u64 {
    let regions = gc_root_regions(up);
    for (base, next) in regions {
        if next < base {
            return u64::MAX;
        }
    }
    unsafe { gc::collect_full(&regions); }
    0
}

/// Current value of the GC cycle counter — monotonically incremented
/// by one on every successful collection (major or minor).  Exposed
/// as the Forth word `gc-cycle ( -- n )`.  Reset to 0 by session
/// reset between harness tests.
#[no_mangle]
pub extern "C" fn rt_gc_cycle_count() -> u64 {
    gc::gc_cycle_count()
}

// ── Managed strings (V2s) ────────────────────────────────────────────
//
// See docs/strings_design.md for the full surface.  Stage A: allocate
// a String from arbitrary bytes, byte-equality.  The other operations
// (`$len`, `$>addr`, `@$`, `!$`) are pure assembly — see
// kernel/strings.masm.

/// Allocate a new `String` GC object and copy `len` bytes from
/// `src_addr` into its payload.  `src_addr` may point anywhere in
/// readable memory (PAD, dictionary heap, slurped-file buffer); the
/// copy is independent of the source after this call returns.
///
/// Same fragmentation-retry path as `rt_vec_alloc_*` for strings
/// whose payload exceeds one page (~64 KB).
///
/// Returns the tagged pointer (low 3 bits = `TAG_STRING`) on
/// success, `u64::MAX` on allocation failure or oversized input.
///
/// # Safety
/// `src_addr..src_addr+len` must be readable.  Kernel-side `>$`
/// guarantees this — it gets the (c-addr, u) pair directly from
/// the Forth data stack.
#[no_mangle]
pub extern "C" fn rt_string_from_bytes(up: u64, src_addr: u64, len: u64) -> u64 {
    if len > u32::MAX as u64 {
        eprintln!(">$: requested length {len} exceeds u32::MAX");
        return u64::MAX;
    }
    let n = len as u32;
    let tagged = match gc::alloc_string(n) {
        Some(t) => t,
        None => {
            // Retry via collect_full if this is a large object.
            let payload_cells = ((len + 7) / 8) as u64;
            if payload_cells > PAGE_SIZE_CELLS {
                let regions = gc_root_regions(up);
                unsafe { gc::collect_full(&regions); }
                match gc::alloc_string(n) {
                    Some(t) => t,
                    None => {
                        eprintln!(">$: out of GC heap (requested {len} bytes, post-compaction retry failed)");
                        return u64::MAX;
                    }
                }
            } else {
                eprintln!(">$: out of GC heap (requested {len} bytes)");
                return u64::MAX;
            }
        }
    };
    if len > 0 {
        // Payload lives at base + 8 (one header cell).  Strip tag
        // to get the base.
        let base = tagged & !7;
        let dst = (base + 8) as *mut u8;
        unsafe {
            std::ptr::copy_nonoverlapping(src_addr as *const u8, dst, len as usize);
        }
    }
    tagged
}

// ── MutStringBuilder runtime (V2s stage C1) ──────────────────────────

/// Read (length, capacity) from a builder's 2-cell header.
/// Caller MUST have verified the tag.
#[inline]
unsafe fn builder_header(base: u64) -> (u32, u32) {
    let hdr = unsafe { *(base as *const u64) };
    let cap = unsafe { *((base + 8) as *const u64) };
    let length = ((hdr >> 5) & 0xFF_FFFF) as u32;
    (length, cap as u32)
}

/// Write a new length into a builder's header word, preserving the
/// type and GC bits.
#[inline]
unsafe fn builder_set_length(base: u64, new_length: u32) {
    let p = base as *mut u64;
    let hdr = unsafe { *p };
    // Clear bits 5..29 (length field), OR in the new length.
    let cleared = hdr & !(0xFF_FFFF << 5);
    let new_hdr = cleared | ((new_length as u64 & 0xFF_FFFF) << 5);
    unsafe { *p = new_hdr; }
}

/// Allocate a fresh `MutStringBuilder` with the given capacity in
/// bytes.  Returns the tagged pointer, or `u64::MAX` on failure.
/// Auto-trigger + fragmentation-retry mirror the other allocators.
#[no_mangle]
pub extern "C" fn rt_sb_new(up: u64, capacity_bytes: u64) -> u64 {
    if capacity_bytes > u32::MAX as u64 {
        eprintln!("sb-new: capacity {capacity_bytes} exceeds u32::MAX");
        return u64::MAX;
    }
    let cap = capacity_bytes as u32;
    let tagged = match gc::alloc_builder(cap) {
        Some(t) => t,
        None => {
            // Builder body cells = 2 + ceil(cap/8). Threshold against
            // PAGE_SIZE_CELLS for the fragmentation-retry decision.
            let payload_cells = ((capacity_bytes + 7) / 8) as u64;
            let total_cells = 2 + payload_cells;
            if total_cells > PAGE_SIZE_CELLS {
                let regions = gc_root_regions(up);
                unsafe { gc::collect_full(&regions); }
                match gc::alloc_builder(cap) {
                    Some(t) => t,
                    None => {
                        eprintln!("sb-new: out of GC heap (cap={capacity_bytes}, post-compaction retry failed)");
                        return u64::MAX;
                    }
                }
            } else {
                eprintln!("sb-new: out of GC heap (cap={capacity_bytes})");
                return u64::MAX;
            }
        }
    };
    tagged
}

/// Append `len` bytes from `src_addr` to the builder identified by
/// `builder_tagged`.  Caller MUST have verified the tag is
/// `TAG_BUILDER`.  Returns 0 on success or `u64::MAX` if the
/// append would overflow capacity (used as the -2062 throw signal
/// in the kernel-side wrapper).
#[no_mangle]
pub extern "C" fn rt_sb_append_bytes(
    builder_tagged: u64,
    src_addr: u64,
    len: u64,
) -> u64 {
    if len == 0 {
        return 0;
    }
    let base = builder_tagged & !7;
    let (length, capacity) = unsafe { builder_header(base) };
    let new_length = length as u64 + len;
    if new_length > capacity as u64 {
        eprintln!("sb-append: would overflow capacity \
                   ({length} + {len} > {capacity})");
        return u64::MAX;
    }
    let dst = (base + 16 + length as u64) as *mut u8;
    unsafe {
        std::ptr::copy_nonoverlapping(src_addr as *const u8, dst, len as usize);
        builder_set_length(base, new_length as u32);
    }
    0
}

/// Append the UTF-8 encoding of `codepoint` to the builder.  Returns
/// 0 on success, `u64::MAX` on capacity overflow or an invalid
/// codepoint (surrogate or > U+10FFFF).
#[no_mangle]
pub extern "C" fn rt_sb_append_codepoint(
    builder_tagged: u64,
    codepoint: u64,
) -> u64 {
    let cp = match char::from_u32(codepoint as u32) {
        Some(c) => c,
        None => {
            eprintln!("sb-append-c: invalid codepoint {codepoint:#x}");
            return u64::MAX;
        }
    };
    let mut buf = [0u8; 4];
    let encoded = cp.encode_utf8(&mut buf);
    rt_sb_append_bytes(
        builder_tagged,
        encoded.as_ptr() as u64,
        encoded.len() as u64,
    )
}

/// Append the decimal representation of a signed integer `n` to
/// the builder.  Returns 0 on success, `u64::MAX` on overflow.
#[no_mangle]
pub extern "C" fn rt_sb_append_int(builder_tagged: u64, n: u64) -> u64 {
    let s = (n as i64).to_string();
    rt_sb_append_bytes(
        builder_tagged,
        s.as_ptr() as u64,
        s.len() as u64,
    )
}

/// Finalise the builder to a fresh immutable `String`.  Allocates
/// a new String of `builder.length` bytes, copies the payload,
/// then resets `builder.length` to 0 (capacity retained) per the
/// V2s design — the builder remains usable.  Returns the tagged
/// String pointer, or `u64::MAX` on allocation failure.
#[no_mangle]
pub extern "C" fn rt_sb_to_string(up: u64, builder_tagged: u64) -> u64 {
    let base = builder_tagged & !7;
    let (length, _capacity) = unsafe { builder_header(base) };
    let payload_addr = base + 16;
    let tagged = rt_string_from_bytes(up, payload_addr, length as u64);
    if tagged == u64::MAX {
        return u64::MAX;
    }
    unsafe { builder_set_length(base, 0); }
    tagged
}

// ── String operations (V2s stage C2) ─────────────────────────────────

/// Read (payload_addr, length) from a String tagged pointer.
/// Caller MUST have verified the tag is `TAG_STRING`.
#[inline]
unsafe fn string_payload(tagged: u64) -> (*const u8, usize) {
    let base = (tagged & !7) as *const u64;
    let hdr = unsafe { *base };
    let len = ((hdr >> 5) & 0xFF_FFFF) as usize;
    let payload = unsafe { (base as *const u8).add(8) };
    (payload, len)
}

/// Concatenate two `String` payloads into a fresh `String`.
/// Caller must have type-checked both inputs.  Returns tagged
/// `String` ptr or `u64::MAX` on failure.
///
/// Allocates directly via `gc::alloc_string` (rather than going
/// through `rt_string_from_bytes`) so we can write the two source
/// halves into the payload without a phantom memcpy from a synthetic
/// "merged" buffer.  Fragmentation-retry mirrors the other paths.
#[no_mangle]
pub extern "C" fn rt_string_concat(up: u64, tagged_a: u64, tagged_b: u64) -> u64 {
    let (a_ptr, a_len) = unsafe { string_payload(tagged_a) };
    let (b_ptr, b_len) = unsafe { string_payload(tagged_b) };
    let total = a_len + b_len;
    if total > u32::MAX as usize {
        eprintln!("$+: combined length {total} exceeds u32::MAX");
        return u64::MAX;
    }
    let tagged = match gc::alloc_string(total as u32) {
        Some(t) => t,
        None => {
            let payload_cells = ((total + 7) / 8) as u64;
            if payload_cells > PAGE_SIZE_CELLS {
                let regions = gc_root_regions(up);
                unsafe { gc::collect_full(&regions); }
                match gc::alloc_string(total as u32) {
                    Some(t) => t,
                    None => {
                        eprintln!("$+: out of GC heap (combined len {total}, post-compaction retry failed)");
                        return u64::MAX;
                    }
                }
            } else {
                eprintln!("$+: out of GC heap (combined len {total})");
                return u64::MAX;
            }
        }
    };
    let base = tagged & !7;
    let dst = (base + 8) as *mut u8;
    unsafe {
        if a_len > 0 {
            std::ptr::copy_nonoverlapping(a_ptr, dst, a_len);
        }
        if b_len > 0 {
            std::ptr::copy_nonoverlapping(b_ptr, dst.add(a_len), b_len);
        }
    }
    tagged
}

/// Build a fresh `String` from the byte slice `tagged[start..end)`.
/// Caller guarantees `start <= end <= len`.  Returns tagged String
/// or `u64::MAX` on alloc failure.
#[no_mangle]
pub extern "C" fn rt_string_slice(
    up: u64,
    tagged: u64,
    start: u64,
    end: u64,
) -> u64 {
    let (payload, len) = unsafe { string_payload(tagged) };
    let len = len as u64;
    if start > end || end > len {
        eprintln!("$slice: bounds [{start}, {end}) out of range for len {len}");
        return u64::MAX;
    }
    let slice_len = end - start;
    let src = unsafe { payload.add(start as usize) };
    rt_string_from_bytes(up, src as u64, slice_len)
}

/// Find the first occurrence of `needle` in `haystack` (byte
/// search, no Unicode normalisation).  Returns the byte index or
/// `u64::MAX` (= -1 in Forth-signed) if not found.  Empty needle
/// matches at index 0.  Caller type-checks both inputs.
#[no_mangle]
pub extern "C" fn rt_string_find(needle: u64, haystack: u64) -> u64 {
    let (n_ptr, n_len) = unsafe { string_payload(needle) };
    let (h_ptr, h_len) = unsafe { string_payload(haystack) };
    if n_len == 0 {
        return 0;
    }
    if n_len > h_len {
        return u64::MAX;
    }
    let n_slice = unsafe { std::slice::from_raw_parts(n_ptr, n_len) };
    let h_slice = unsafe { std::slice::from_raw_parts(h_ptr, h_len) };
    // Naive memmem; good enough for V2s stage C2.
    for i in 0..=(h_len - n_len) {
        if &h_slice[i..i + n_len] == n_slice {
            return i as u64;
        }
    }
    u64::MAX
}

/// True iff `prefix` matches the first `prefix.len` bytes of `s`.
#[no_mangle]
pub extern "C" fn rt_string_starts(prefix: u64, s: u64) -> u64 {
    let (p_ptr, p_len) = unsafe { string_payload(prefix) };
    let (s_ptr, s_len) = unsafe { string_payload(s) };
    if p_len > s_len {
        return 0;
    }
    let p_slice = unsafe { std::slice::from_raw_parts(p_ptr, p_len) };
    let s_slice = unsafe { std::slice::from_raw_parts(s_ptr, p_len) };
    if p_slice == s_slice { u64::MAX } else { 0 }
}

/// True iff `suffix` matches the last `suffix.len` bytes of `s`.
#[no_mangle]
pub extern "C" fn rt_string_ends(suffix: u64, s: u64) -> u64 {
    let (p_ptr, p_len) = unsafe { string_payload(suffix) };
    let (s_ptr, s_len) = unsafe { string_payload(s) };
    if p_len > s_len {
        return 0;
    }
    let s_tail = unsafe { s_ptr.add(s_len - p_len) };
    let p_slice = unsafe { std::slice::from_raw_parts(p_ptr, p_len) };
    let s_slice = unsafe { std::slice::from_raw_parts(s_tail, p_len) };
    if p_slice == s_slice { u64::MAX } else { 0 }
}

/// Lexicographic byte compare.  Returns -1 if a < b, 0 if equal,
/// +1 if a > b.  Same convention as memcmp's sign / Rust's
/// `Ord::cmp`.
#[no_mangle]
pub extern "C" fn rt_string_cmp(tagged_a: u64, tagged_b: u64) -> u64 {
    let (a_ptr, a_len) = unsafe { string_payload(tagged_a) };
    let (b_ptr, b_len) = unsafe { string_payload(tagged_b) };
    let a = unsafe { std::slice::from_raw_parts(a_ptr, a_len) };
    let b = unsafe { std::slice::from_raw_parts(b_ptr, b_len) };
    match a.cmp(b) {
        std::cmp::Ordering::Less    => (-1i64) as u64,
        std::cmp::Ordering::Equal   => 0,
        std::cmp::Ordering::Greater => 1,
    }
}

/// FxHash-style 64-bit hash of the byte payload.  Cheap,
/// non-cryptographic; suitable for hash-table keys within a
/// single process run.  Identical inputs hash identically across
/// calls.
#[no_mangle]
pub extern "C" fn rt_string_hash(tagged: u64) -> u64 {
    let (ptr, len) = unsafe { string_payload(tagged) };
    let bytes = unsafe { std::slice::from_raw_parts(ptr, len) };
    let mut h: u64 = 0xcbf29ce484222325; // FNV-1a 64-bit basis
    for &b in bytes {
        h ^= b as u64;
        h = h.wrapping_mul(0x100000001b3);
    }
    h
}

/// ASCII-only case-insensitive equality.  Non-ASCII bytes
/// compare as-is (no Unicode folding — that's a later stage).
#[no_mangle]
pub extern "C" fn rt_string_ci_eq(tagged_a: u64, tagged_b: u64) -> u64 {
    let (a_ptr, a_len) = unsafe { string_payload(tagged_a) };
    let (b_ptr, b_len) = unsafe { string_payload(tagged_b) };
    if a_len != b_len {
        return 0;
    }
    let a = unsafe { std::slice::from_raw_parts(a_ptr, a_len) };
    let b = unsafe { std::slice::from_raw_parts(b_ptr, b_len) };
    for i in 0..a_len {
        if a[i].eq_ignore_ascii_case(&b[i]) {
            continue;
        }
        return 0;
    }
    u64::MAX
}

/// Trim ASCII whitespace from both ends, return a fresh String.
/// (Unicode whitespace handling is V2s stage D.)
#[no_mangle]
pub extern "C" fn rt_string_trim(up: u64, tagged: u64) -> u64 {
    let (ptr, len) = unsafe { string_payload(tagged) };
    let bytes = unsafe { std::slice::from_raw_parts(ptr, len) };
    let mut start = 0;
    let mut end = len;
    while start < end && bytes[start].is_ascii_whitespace() {
        start += 1;
    }
    while end > start && bytes[end - 1].is_ascii_whitespace() {
        end -= 1;
    }
    let new_len = (end - start) as u64;
    rt_string_from_bytes(up, unsafe { ptr.add(start) as u64 }, new_len)
}

/// Trim ASCII whitespace from the left only.
#[no_mangle]
pub extern "C" fn rt_string_ltrim(up: u64, tagged: u64) -> u64 {
    let (ptr, len) = unsafe { string_payload(tagged) };
    let bytes = unsafe { std::slice::from_raw_parts(ptr, len) };
    let mut start = 0;
    while start < len && bytes[start].is_ascii_whitespace() {
        start += 1;
    }
    let new_len = (len - start) as u64;
    rt_string_from_bytes(up, unsafe { ptr.add(start) as u64 }, new_len)
}

/// Trim ASCII whitespace from the right only.
#[no_mangle]
pub extern "C" fn rt_string_rtrim(up: u64, tagged: u64) -> u64 {
    let (ptr, len) = unsafe { string_payload(tagged) };
    let bytes = unsafe { std::slice::from_raw_parts(ptr, len) };
    let mut end = len;
    while end > 0 && bytes[end - 1].is_ascii_whitespace() {
        end -= 1;
    }
    rt_string_from_bytes(up, ptr as u64, end as u64)
}

/// Format a signed integer as a decimal `String`.
#[no_mangle]
pub extern "C" fn rt_int_to_string(up: u64, n: u64) -> u64 {
    let s = (n as i64).to_string();
    rt_string_from_bytes(up, s.as_ptr() as u64, s.len() as u64)
}

// ── V2s stage E — UTF-8-aware ops, floats, char$, $words ────────────

/// Codepoint count: number of Unicode scalar values in `tagged`.
/// Walks the UTF-8 payload counting non-continuation bytes (bytes
/// whose top two bits are NOT `10`).  Returns `u64::MAX` on
/// malformed UTF-8.
#[no_mangle]
pub extern "C" fn rt_string_clen(tagged: u64) -> u64 {
    let (ptr, len) = unsafe { string_payload(tagged) };
    let bytes = unsafe { std::slice::from_raw_parts(ptr, len) };
    let Ok(s) = std::str::from_utf8(bytes) else {
        return u64::MAX;
    };
    s.chars().count() as u64
}

/// Read the codepoint at character index `char_idx`.  Returns the
/// codepoint as u32 (in u64) on success, or `u64::MAX` if the
/// index is out of range OR the payload is malformed UTF-8.
#[no_mangle]
pub extern "C" fn rt_string_cat(tagged: u64, char_idx: u64) -> u64 {
    let (ptr, len) = unsafe { string_payload(tagged) };
    let bytes = unsafe { std::slice::from_raw_parts(ptr, len) };
    let Ok(s) = std::str::from_utf8(bytes) else {
        return u64::MAX;
    };
    match s.chars().nth(char_idx as usize) {
        Some(c) => c as u64,
        None => u64::MAX,
    }
}

/// True (returns -1) iff the payload is well-formed UTF-8.
/// Cheap predicate; no allocation.
#[no_mangle]
pub extern "C" fn rt_string_valid(tagged: u64) -> u64 {
    let (ptr, len) = unsafe { string_payload(tagged) };
    let bytes = unsafe { std::slice::from_raw_parts(ptr, len) };
    if std::str::from_utf8(bytes).is_ok() { u64::MAX } else { 0 }
}

/// Allocate a single-codepoint `String` containing the UTF-8
/// encoding of `codepoint`.  Returns `u64::MAX` for invalid
/// codepoints (surrogates or > U+10FFFF).
#[no_mangle]
pub extern "C" fn rt_char_to_string(up: u64, codepoint: u64) -> u64 {
    let Some(c) = char::from_u32(codepoint as u32) else {
        eprintln!("char$: invalid codepoint {codepoint:#x}");
        return u64::MAX;
    };
    let mut buf = [0u8; 4];
    let encoded = c.encode_utf8(&mut buf);
    rt_string_from_bytes(up, encoded.as_ptr() as u64, encoded.len() as u64)
}

/// Unicode-aware uppercase conversion.  Allocates a fresh String;
/// the result's byte length may differ from the input's (e.g.
/// German ß uppercases to "SS").  Returns `u64::MAX` on alloc
/// failure or malformed UTF-8 in the input.
#[no_mangle]
pub extern "C" fn rt_string_upper(up: u64, tagged: u64) -> u64 {
    let (ptr, len) = unsafe { string_payload(tagged) };
    let bytes = unsafe { std::slice::from_raw_parts(ptr, len) };
    let Ok(s) = std::str::from_utf8(bytes) else {
        eprintln!("$upper: input is not valid UTF-8");
        return u64::MAX;
    };
    let upper: String = s.to_uppercase();
    rt_string_from_bytes(up, upper.as_ptr() as u64, upper.len() as u64)
}

/// Unicode-aware lowercase conversion.  Same shape as `rt_string_upper`.
#[no_mangle]
pub extern "C" fn rt_string_lower(up: u64, tagged: u64) -> u64 {
    let (ptr, len) = unsafe { string_payload(tagged) };
    let bytes = unsafe { std::slice::from_raw_parts(ptr, len) };
    let Ok(s) = std::str::from_utf8(bytes) else {
        eprintln!("$lower: input is not valid UTF-8");
        return u64::MAX;
    };
    let lower: String = s.to_lowercase();
    rt_string_from_bytes(up, lower.as_ptr() as u64, lower.len() as u64)
}

/// Parse `tagged` as an f64.  On success writes the bits to
/// `*out_bits` and returns `u64::MAX` (Forth true).  On failure
/// returns 0 (Forth false) and `*out_bits` is unchanged.
/// Trims surrounding ASCII whitespace before parsing.
#[no_mangle]
pub extern "C" fn rt_string_to_float(tagged: u64, out_bits: u64) -> u64 {
    let (ptr, len) = unsafe { string_payload(tagged) };
    let bytes = unsafe { std::slice::from_raw_parts(ptr, len) };
    let Ok(text) = std::str::from_utf8(bytes) else {
        return 0;
    };
    let trimmed = text.trim();
    // Accept Forth-style trailing 'e' / 'e+' / 'e-' (just like
    // normalize_float_token does for the legacy `>float`).
    let mut owned: String;
    let candidate = if let Some(s) = normalize_float_token(trimmed) {
        owned = s;
        owned.as_str()
    } else {
        trimmed
    };
    match candidate.parse::<f64>() {
        Ok(v) => {
            unsafe { *(out_bits as *mut u64) = v.to_bits(); }
            u64::MAX
        }
        Err(_) => 0,
    }
}

/// Format an f64 (passed as raw bits) as a `String`.  Uses Rust's
/// default Display for f64 — `{}` formatting — which produces
/// shortest-round-trip output.  Some output examples: 1, 1.5,
/// -3.14159, inf, NaN.
#[no_mangle]
pub extern "C" fn rt_float_to_string(up: u64, bits: u64) -> u64 {
    let v = f64::from_bits(bits);
    let s = format!("{v}");
    rt_string_from_bytes(up, s.as_ptr() as u64, s.len() as u64)
}

/// Append the formatted-float representation of `bits` to the
/// builder.  Returns 0 on success, `u64::MAX` on capacity overflow.
#[no_mangle]
pub extern "C" fn rt_sb_append_float(builder_tagged: u64, bits: u64) -> u64 {
    let v = f64::from_bits(bits);
    let s = format!("{v}");
    rt_sb_append_bytes(builder_tagged, s.as_ptr() as u64, s.len() as u64)
}

/// Whitespace-tokenise `haystack`: split on runs of ASCII
/// whitespace, dropping empty pieces (so leading/trailing/repeated
/// whitespace doesn't produce zero-length tokens).  Same writes-to-
/// caller-buffer protocol as `rt_string_split_into`.
///
/// `_up` is accepted for signature symmetry but unused — pieces
/// allocate via the no-retry path so a mid-loop collect_full
/// can't move earlier pieces.
#[no_mangle]
pub extern "C" fn rt_string_words_into(
    _up: u64,
    haystack: u64,
    out_addr: u64,
    max_parts: u64,
) -> u64 {
    let (h_ptr, h_len) = unsafe { string_payload(haystack) };
    let h = unsafe { std::slice::from_raw_parts(h_ptr, h_len) };
    let out = out_addr as *mut u64;
    let mut written: u64 = 0;
    let mut i = 0usize;
    while i < h_len {
        // Skip whitespace.
        while i < h_len && h[i].is_ascii_whitespace() {
            i += 1;
        }
        if i >= h_len { break; }
        // Mark token start; advance over non-whitespace.
        let start = i;
        while i < h_len && !h[i].is_ascii_whitespace() {
            i += 1;
        }
        if written >= max_parts {
            eprintln!("$words: too many tokens (max {max_parts})");
            return u64::MAX;
        }
        let piece = alloc_string_copy_no_retry(
            unsafe { h_ptr.add(start) },
            i - start,
        );
        if piece == u64::MAX {
            eprintln!("$words: out of GC heap mid-tokenise");
            return u64::MAX;
        }
        unsafe { *out.add(written as usize) = piece; }
        written += 1;
    }
    written
}

/// True iff `needle` occurs anywhere in `haystack` (byte search).
/// Empty needle is always considered contained.
#[no_mangle]
pub extern "C" fn rt_string_contains(needle: u64, haystack: u64) -> u64 {
    let (n_ptr, n_len) = unsafe { string_payload(needle) };
    let (h_ptr, h_len) = unsafe { string_payload(haystack) };
    if n_len == 0 {
        return u64::MAX;
    }
    if n_len > h_len {
        return 0;
    }
    let n = unsafe { std::slice::from_raw_parts(n_ptr, n_len) };
    let h = unsafe { std::slice::from_raw_parts(h_ptr, h_len) };
    for i in 0..=(h_len - n_len) {
        if &h[i..i + n_len] == n {
            return u64::MAX;
        }
    }
    0
}

/// Find the LAST occurrence of `needle` in `haystack`.  Empty
/// needle returns `haystack.len`.  Returns `u64::MAX` (-1) if no
/// match.
#[no_mangle]
pub extern "C" fn rt_string_rfind(needle: u64, haystack: u64) -> u64 {
    let (n_ptr, n_len) = unsafe { string_payload(needle) };
    let (h_ptr, h_len) = unsafe { string_payload(haystack) };
    if n_len == 0 {
        return h_len as u64;
    }
    if n_len > h_len {
        return u64::MAX;
    }
    let n = unsafe { std::slice::from_raw_parts(n_ptr, n_len) };
    let h = unsafe { std::slice::from_raw_parts(h_ptr, h_len) };
    // Scan backwards from the last possible start position.
    let mut i = h_len - n_len;
    loop {
        if &h[i..i + n_len] == n {
            return i as u64;
        }
        if i == 0 { break; }
        i -= 1;
    }
    u64::MAX
}

/// Return a fresh String containing `tagged` repeated `n` times.
/// `n == 0` returns an empty String; very large `n` may overflow
/// u32 length and is rejected.
#[no_mangle]
pub extern "C" fn rt_string_repeat(up: u64, tagged: u64, n: u64) -> u64 {
    let (src_ptr, src_len) = unsafe { string_payload(tagged) };
    let Some(total) = (src_len as u64).checked_mul(n) else {
        eprintln!("$repeat: overflow ({src_len} × {n})");
        return u64::MAX;
    };
    if total > u32::MAX as u64 {
        eprintln!("$repeat: result {total} exceeds u32::MAX");
        return u64::MAX;
    }
    let total = total as usize;
    let tagged_out = match gc::alloc_string(total as u32) {
        Some(t) => t,
        None => {
            let payload_cells = ((total + 7) / 8) as u64;
            if payload_cells > PAGE_SIZE_CELLS {
                let regions = gc_root_regions(up);
                unsafe { gc::collect_full(&regions); }
                match gc::alloc_string(total as u32) {
                    Some(t) => t,
                    None => {
                        eprintln!("$repeat: out of GC heap (len {total}, post-compaction retry failed)");
                        return u64::MAX;
                    }
                }
            } else {
                eprintln!("$repeat: out of GC heap (len {total})");
                return u64::MAX;
            }
        }
    };
    if total > 0 && src_len > 0 {
        let base = tagged_out & !7;
        let dst = (base + 8) as *mut u8;
        unsafe {
            for i in 0..(n as usize) {
                std::ptr::copy_nonoverlapping(src_ptr, dst.add(i * src_len), src_len);
            }
        }
    }
    tagged_out
}

/// Replace every occurrence of `needle` in `haystack` with `repl`,
/// returning a fresh String.  Empty needle returns `haystack`
/// untouched (we don't loop forever on the zero-length match).
#[no_mangle]
pub extern "C" fn rt_string_replace(
    up: u64,
    needle: u64,
    repl: u64,
    haystack: u64,
) -> u64 {
    let (n_ptr, n_len) = unsafe { string_payload(needle) };
    let (r_ptr, r_len) = unsafe { string_payload(repl) };
    let (h_ptr, h_len) = unsafe { string_payload(haystack) };

    if n_len == 0 {
        // Defensive: return a fresh copy of haystack.  Could return
        // haystack itself but the design is "operations return fresh
        // Strings."
        return rt_string_from_bytes(up, h_ptr as u64, h_len as u64);
    }

    let n = unsafe { std::slice::from_raw_parts(n_ptr, n_len) };
    let h = unsafe { std::slice::from_raw_parts(h_ptr, h_len) };
    let r = unsafe { std::slice::from_raw_parts(r_ptr, r_len) };

    // First pass: count matches to size the output exactly.
    let mut count: usize = 0;
    if n_len <= h_len {
        let mut i = 0;
        while i + n_len <= h_len {
            if &h[i..i + n_len] == n {
                count += 1;
                i += n_len;
            } else {
                i += 1;
            }
        }
    }

    if count == 0 {
        return rt_string_from_bytes(up, h_ptr as u64, h_len as u64);
    }

    // Output size = h_len + count * (r_len - n_len).  Avoid signed
    // arithmetic by adding contribution-per-match carefully.
    let removed = count * n_len;
    let added   = count * r_len;
    let new_len = h_len - removed + added;
    if new_len > u32::MAX as usize {
        eprintln!("$replace: result {new_len} exceeds u32::MAX");
        return u64::MAX;
    }

    let tagged_out = match gc::alloc_string(new_len as u32) {
        Some(t) => t,
        None => {
            let payload_cells = ((new_len + 7) / 8) as u64;
            if payload_cells > PAGE_SIZE_CELLS {
                let regions = gc_root_regions(up);
                unsafe { gc::collect_full(&regions); }
                match gc::alloc_string(new_len as u32) {
                    Some(t) => t,
                    None => {
                        eprintln!("$replace: out of GC heap (len {new_len}, post-compaction retry failed)");
                        return u64::MAX;
                    }
                }
            } else {
                eprintln!("$replace: out of GC heap (len {new_len})");
                return u64::MAX;
            }
        }
    };

    // Second pass: copy with substitutions.
    let base = tagged_out & !7;
    let dst = (base + 8) as *mut u8;
    let mut di = 0usize;
    let mut i = 0usize;
    while i + n_len <= h_len {
        if &h[i..i + n_len] == n {
            unsafe {
                if r_len > 0 {
                    std::ptr::copy_nonoverlapping(r_ptr, dst.add(di), r_len);
                }
            }
            di += r_len;
            i += n_len;
        } else {
            unsafe { *dst.add(di) = h[i]; }
            di += 1;
            i += 1;
        }
    }
    // Trailing bytes after the last possible match start.
    while i < h_len {
        unsafe { *dst.add(di) = h[i]; }
        di += 1;
        i += 1;
    }
    debug_assert_eq!(di, new_len);
    tagged_out
}

/// Allocate a fresh `String` and copy bytes into it.  Direct
/// shortcut for code paths that must NOT trigger
/// fragmentation-retry / collect_full mid-loop (which would
/// invalidate previously-emitted pointers held outside any GC
/// root region).  Returns `u64::MAX` on alloc failure, leaving
/// retry / GC decisions to the caller.
fn alloc_string_copy_no_retry(src: *const u8, len: usize) -> u64 {
    if len > u32::MAX as usize {
        return u64::MAX;
    }
    let Some(tagged) = gc::alloc_string(len as u32) else {
        return u64::MAX;
    };
    if len > 0 {
        let base = tagged & !7;
        let dst = (base + 8) as *mut u8;
        unsafe { std::ptr::copy_nonoverlapping(src, dst, len); }
    }
    tagged
}

/// Split `haystack` on every occurrence of `sep`, writing the
/// resulting String tagged pointers into the cells starting at
/// `out_addr`.  Returns the number of parts written.  Empty `sep`
/// is rejected (would yield infinite parts) — returns `u64::MAX`.
///
/// Caller must ensure `out_addr` has room for at least `max_parts`
/// cells; the kernel-side wrapper enforces this against a fixed
/// buffer.  If more parts would be produced than fit, returns
/// `u64::MAX` (the caller must surface as -2058 / overflow).
///
/// `_up` is accepted for signature consistency with the other
/// allocators but not used — see `alloc_string_copy_no_retry`:
/// each piece allocation skips the fragmentation-retry path so a
/// mid-loop collect_full can't move earlier pieces out from under
/// the destination slots (which aren't a GC root region).
#[no_mangle]
pub extern "C" fn rt_string_split_into(
    _up: u64,
    sep: u64,
    haystack: u64,
    out_addr: u64,
    max_parts: u64,
) -> u64 {
    let (s_ptr, s_len) = unsafe { string_payload(sep) };
    let (h_ptr, h_len) = unsafe { string_payload(haystack) };
    if s_len == 0 {
        eprintln!("$split: empty separator");
        return u64::MAX;
    }
    let s = unsafe { std::slice::from_raw_parts(s_ptr, s_len) };
    let h = unsafe { std::slice::from_raw_parts(h_ptr, h_len) };
    let out = out_addr as *mut u64;
    let mut written: u64 = 0;
    let mut start = 0usize;
    let mut i = 0usize;
    while i + s_len <= h_len {
        if &h[i..i + s_len] == s {
            if written >= max_parts {
                eprintln!("$split: too many parts (max {max_parts})");
                return u64::MAX;
            }
            let piece = alloc_string_copy_no_retry(
                unsafe { h_ptr.add(start) },
                i - start,
            );
            if piece == u64::MAX {
                eprintln!("$split: out of GC heap mid-split");
                return u64::MAX;
            }
            unsafe { *out.add(written as usize) = piece; }
            written += 1;
            start = i + s_len;
            i = start;
        } else {
            i += 1;
        }
    }
    if written >= max_parts {
        eprintln!("$split: too many parts (max {max_parts})");
        return u64::MAX;
    }
    let piece = alloc_string_copy_no_retry(
        unsafe { h_ptr.add(start) },
        h_len - start,
    );
    if piece == u64::MAX {
        eprintln!("$split: out of GC heap on final piece");
        return u64::MAX;
    }
    unsafe { *out.add(written as usize) = piece; }
    written += 1;
    written
}

/// Parse `tagged` as a signed decimal integer.  Returns
/// `(value, true)` on the data stack — wait, we can't return two
/// values from a single C function easily.  Use a different shape:
/// returns `value` on success or stashes a flag via the kernel's
/// out-pointer.
///
/// Simpler convention: returns `u64::MAX` on parse failure (caller
/// surfaces as "false" with no value pushed), otherwise the
/// parsed value (and the kernel pushes both value + true).
#[no_mangle]
pub extern "C" fn rt_string_to_int(tagged: u64, out_value: u64) -> u64 {
    let (ptr, len) = unsafe { string_payload(tagged) };
    let bytes = unsafe { std::slice::from_raw_parts(ptr, len) };
    let s = match std::str::from_utf8(bytes) {
        Ok(s) => s.trim(),
        Err(_) => return 0,
    };
    match s.parse::<i64>() {
        Ok(n) => {
            unsafe { *(out_value as *mut i64) = n; }
            u64::MAX
        }
        Err(_) => 0,
    }
}

// ── iGui bridge (used by wf64-ui; no-op headless) ────────────────────

/// Clear the wf64-ui console scrollback.  No-op in the headless
/// `wf64` binary (where the fconsole module isn't compiled in)
/// and on non-Windows.
#[no_mangle]
pub extern "C" fn rt_igui_page() -> u64 {
    #[cfg(windows)]
    {
        crate::igui::fconsole::clear_screen();
    }
    0
}

/// Atomic flag set by `rt_bug_rust_panic`; consumed by the
/// wf64-ui worker loop after each eval.  The deferred-panic
/// pattern avoids unwinding across the extern "C" boundary
/// back into JIT'd asm (which is undefined behaviour).
pub static BUG_PANIC_PENDING: std::sync::atomic::AtomicBool =
    std::sync::atomic::AtomicBool::new(false);

/// Set the "panic on return from eval" flag.  Used by the Forth
/// word `bug-rust-panic` to manually exercise the wf64-ui crash
/// handler — the actual panic happens in pure-Rust context
/// (run_drain_loop after the JIT returns), where unwinding is
/// well-defined.  Never panics from inside the extern "C" body.
#[no_mangle]
pub extern "C" fn rt_bug_rust_panic() -> u64 {
    BUG_PANIC_PENDING.store(true, std::sync::atomic::Ordering::SeqCst);
    0
}

/// Deliberately read from a NULL pointer to trigger a real
/// Windows ACCESS_VIOLATION inside the worker thread.  Exists
/// purely to test the Phase 3b SEH-recovery path.  The VEH in
/// `crash_handler.rs` should catch it, capture register state,
/// rewrite RIP to the thunk, and the supervisor should respawn
/// the worker.  Never call from production code.
///
/// Implemented in Rust (not in Forth code calling `0 @`) so the
/// AV happens at a known RIP we can recognise in the dump.
#[no_mangle]
pub extern "C" fn rt_bug_seh_av() -> u64 {
    let p: *const u64 = std::ptr::null();
    unsafe { std::ptr::read_volatile(p) }
}

/// Record a cursor position for the next emit.  V1 limitation
/// (see the at-xy doc in `kernel/igui.masm`): not yet routed
/// through emit's write path.  Lands in `IGUI_PENDING_AT_XY` so
/// future streaming-IO work can pick it up.
#[no_mangle]
pub extern "C" fn rt_igui_at_xy(col: u64, row: u64) -> u64 {
    #[cfg(windows)]
    {
        IGUI_PENDING_AT_XY.with(|c| c.set(Some((col as usize, row as usize))));
    }
    let _ = (col, row);
    0
}

#[cfg(windows)]
thread_local! {
    /// Last-requested cursor position from `AT-XY`.  Read by the
    /// future streaming-emit path (not yet wired); cleared after
    /// each consumption.
    pub(crate) static IGUI_PENDING_AT_XY:
        std::cell::Cell<Option<(usize, usize)>> = const { std::cell::Cell::new(None) };
}

// ─── Forth-callable graphical pane API ───────────────────────────────
//
// Thin layer over `window::open_child` + `batch::push/finish/submit`.
// Forth opens a graphical MDI child, calls `gpane-begin id`, then any
// number of draw primitives, then `gpane-present`.  Colours are packed
// as 0xRRGGBB into a single Forth cell; coordinates are signed cells
// (pixels).
//
// All draw primitives operate on the worker thread's current batch
// (`batch::push` is thread-local).  The actual paint happens on the
// GUI thread after `submit` posts a fresh PaneBatch + invalidates.

/// Open a graphical MDI child sized `width x height` with the given
/// UTF-8 title (read from `title_addr..title_addr+title_len`).
/// Returns the child_id (positive i64) on success, 0 on failure.
#[no_mangle]
pub extern "C" fn rt_gpane_open(
    width: u64,
    height: u64,
    title_addr: u64,
    title_len: u64,
) -> u64 {
    #[cfg(windows)]
    {
        let title = unsafe {
            std::slice::from_raw_parts(title_addr as *const u8, title_len as usize)
        };
        let title = std::str::from_utf8(title).unwrap_or("∴ gpane");
        match crate::igui::window::open_child_sized(
            title,
            width as i32,
            height as i32,
        ) {
            Some(id) => id as u64,
            None => 0,
        }
    }
    #[cfg(not(windows))]
    {
        let _ = (width, height, title_addr, title_len);
        0
    }
}

/// Begin a draw batch targeting `child_id`.  Replaces any in-progress
/// batch on this thread.  Pair with `rt_gpane_present`.
#[no_mangle]
pub extern "C" fn rt_gpane_begin(child_id: u64) -> u64 {
    #[cfg(windows)]
    {
        crate::igui::batch::begin(child_id as i64);
    }
    let _ = child_id;
    0
}

/// Submit the current batch to the GUI thread; paints next frame.
/// No-op if no batch is in progress.
#[no_mangle]
pub extern "C" fn rt_gpane_present() -> u64 {
    #[cfg(windows)]
    {
        if let Some(batch) = crate::igui::batch::finish() {
            crate::igui::batch::submit(batch);
        }
    }
    0
}

/// Decode a 24-bit packed RGB cell (0xRRGGBB) into the float RGBA the
/// surface batch expects.  Alpha is always 1.0.
#[cfg(windows)]
fn rgb_to_rgba(packed: u64) -> crate::igui::batch::Rgba {
    let r = ((packed >> 16) & 0xFF) as f32 / 255.0;
    let g = ((packed >> 8) & 0xFF) as f32 / 255.0;
    let b = (packed & 0xFF) as f32 / 255.0;
    crate::igui::batch::Rgba { r, g, b, a: 1.0 }
}

/// Clear the pane with `rgb` (packed 0xRRGGBB).
#[no_mangle]
pub extern "C" fn rt_gpane_clear(rgb: u64) -> u64 {
    #[cfg(windows)]
    {
        crate::igui::batch::push(
            crate::igui::batch::SurfaceCmd::Clear {
                color: rgb_to_rgba(rgb),
            },
        );
    }
    let _ = rgb;
    0
}

/// Fill a rectangle at (x, y) with size (w × h).
#[no_mangle]
pub extern "C" fn rt_gpane_fill_rect(
    x: u64, y: u64, w: u64, h: u64, rgb: u64,
) -> u64 {
    #[cfg(windows)]
    {
        let (x, y, w, h) = (x as i64 as f32, y as i64 as f32,
                            w as i64 as f32, h as i64 as f32);
        crate::igui::batch::push(
            crate::igui::batch::SurfaceCmd::FillRect {
                rect: crate::igui::batch::Rect {
                    x0: x, y0: y, x1: x + w, y1: y + h,
                },
                corner_radius: 0.0,
                color: rgb_to_rgba(rgb),
            },
        );
    }
    let _ = (x, y, w, h, rgb);
    0
}

/// Stroke a rectangle.  `thick` is the line thickness in pixels.
#[no_mangle]
pub extern "C" fn rt_gpane_stroke_rect(
    x: u64, y: u64, w: u64, h: u64, thick: u64, rgb: u64,
) -> u64 {
    #[cfg(windows)]
    {
        let (x, y, w, h) = (x as i64 as f32, y as i64 as f32,
                            w as i64 as f32, h as i64 as f32);
        let thick = thick as i64 as f32;
        crate::igui::batch::push(
            crate::igui::batch::SurfaceCmd::StrokeRect {
                rect: crate::igui::batch::Rect {
                    x0: x, y0: y, x1: x + w, y1: y + h,
                },
                corner_radius: 0.0,
                half_thickness: thick / 2.0,
                color: rgb_to_rgba(rgb),
            },
        );
    }
    let _ = (x, y, w, h, thick, rgb);
    0
}

/// Draw a line from (x0,y0) to (x1,y1) with thickness `thick`.
#[no_mangle]
pub extern "C" fn rt_gpane_line(
    x0: u64, y0: u64, x1: u64, y1: u64, thick: u64, rgb: u64,
) -> u64 {
    #[cfg(windows)]
    {
        let x0 = x0 as i64 as f32; let y0 = y0 as i64 as f32;
        let x1 = x1 as i64 as f32; let y1 = y1 as i64 as f32;
        let thick = thick as i64 as f32;
        crate::igui::batch::push(
            crate::igui::batch::SurfaceCmd::DrawLine {
                p0: crate::igui::batch::Point { x: x0, y: y0 },
                p1: crate::igui::batch::Point { x: x1, y: y1 },
                half_thickness: thick / 2.0,
                color: rgb_to_rgba(rgb),
            },
        );
    }
    let _ = (x0, y0, x1, y1, thick, rgb);
    0
}

/// Fill a circle centered at (cx,cy) with radius `r`.
#[no_mangle]
pub extern "C" fn rt_gpane_fill_circle(
    cx: u64, cy: u64, r: u64, rgb: u64,
) -> u64 {
    #[cfg(windows)]
    {
        let cx = cx as i64 as f32; let cy = cy as i64 as f32;
        let r = r as i64 as f32;
        crate::igui::batch::push(
            crate::igui::batch::SurfaceCmd::FillCircle {
                center: crate::igui::batch::Point { x: cx, y: cy },
                radius: r,
                color: rgb_to_rgba(rgb),
            },
        );
    }
    let _ = (cx, cy, r, rgb);
    0
}

// ─── Canvas FFI (Forth-owned pixel framebuffer, the bulk fast path) ──
//
// The `gpane-*` primitives above are immediate-mode: each shape is one
// batch command, so a per-pixel image would mean one command per pixel
// — death by boundary crossing.  The canvas inverts that.  Forth owns a
// `w×h` BGRA byte-array, fills it with *native* stores (no FFI per
// pixel), then ships the whole frame across in ONE call: O(1) boundary
// crossings per frame instead of O(pixels).

/// Blit a Forth-owned `w×h` BGRA framebuffer to graphical pane
/// `child_id` and present it as a single Direct2D bitmap upload.
///
/// `src_addr` points at `w*h` packed `0xAARRGGBB` words (native BGRA,
/// little-endian).  We copy them into an owned `Arc<Vec<u32>>` the GUI
/// thread can still read when it paints a later frame — essential
/// because NewGC may move or reclaim the source byte-array after this
/// call returns.  Hands the copy to the batch as one `Blit` command.
/// Returns 0 (command-style); a no-op off Windows.
#[no_mangle]
pub extern "C" fn rt_canvas_blit(child_id: u64, src_addr: u64, w: u64, h: u64) -> u64 {
    #[cfg(windows)]
    {
        let (w, h) = (w as usize, h as usize);
        let n = w.saturating_mul(h);
        if n == 0 || src_addr == 0 {
            return 0;
        }
        // Copy out of the (possibly movable) Forth byte-array into an
        // owned buffer the GUI thread reads on a later frame.
        let src = unsafe { std::slice::from_raw_parts(src_addr as *const u32, n) };
        let pixels = std::sync::Arc::new(src.to_vec());
        crate::igui::batch::present_pixels(child_id as i64, w as u32, h as u32, pixels);
    }
    #[cfg(not(windows))]
    {
        let _ = (child_id, src_addr, w, h);
    }
    0
}

// ─── Doc-pane FFI (Forth-writable Markdown) ──────────────────────────
//
// A doc-pane is the read-only `help_pane`'s plain sibling: a single
// Markdown document whose source a Forth program supplies.  `doc-open`
// makes an empty pane and returns its child id; `doc-set` replaces the
// Markdown; `doc-append` streams more onto the end.  The pane re-parses
// and repaints itself on the GUI thread after each edit.

/// Open an empty Markdown doc-pane with the UTF-8 title at
/// `title_addr..title_addr+title_len`.  Returns the child id (>0) on
/// success, 0 on failure.
#[no_mangle]
pub extern "C" fn rt_doc_open(title_addr: u64, title_len: u64) -> u64 {
    #[cfg(windows)]
    {
        let title = unsafe {
            std::slice::from_raw_parts(title_addr as *const u8, title_len as usize)
        };
        let title = std::str::from_utf8(title).unwrap_or("∴ doc");
        match crate::igui::doc_pane::open(title) {
            Some(id) => id as u64,
            None => 0,
        }
    }
    #[cfg(not(windows))]
    {
        let _ = (title_addr, title_len);
        0
    }
}

/// Replace doc-pane `child_id`'s Markdown with the UTF-8 text at
/// `md_addr..md_addr+md_len`.  Returns 1 on success, 0 if the pane is
/// gone or the bytes aren't valid UTF-8.
#[no_mangle]
pub extern "C" fn rt_doc_set(child_id: u64, md_addr: u64, md_len: u64) -> u64 {
    #[cfg(windows)]
    {
        let bytes = unsafe {
            std::slice::from_raw_parts(md_addr as *const u8, md_len as usize)
        };
        let Ok(md) = std::str::from_utf8(bytes) else { return 0 };
        if crate::igui::doc_pane::set_markdown(child_id as i64, md) { 1 } else { 0 }
    }
    #[cfg(not(windows))]
    {
        let _ = (child_id, md_addr, md_len);
        0
    }
}

/// Append the UTF-8 text at `md_addr..md_addr+md_len` to doc-pane
/// `child_id`'s Markdown.  Returns 1 on success, 0 otherwise.
#[no_mangle]
pub extern "C" fn rt_doc_append(child_id: u64, md_addr: u64, md_len: u64) -> u64 {
    #[cfg(windows)]
    {
        let bytes = unsafe {
            std::slice::from_raw_parts(md_addr as *const u8, md_len as usize)
        };
        let Ok(md) = std::str::from_utf8(bytes) else { return 0 };
        if crate::igui::doc_pane::append_markdown(child_id as i64, md) { 1 } else { 0 }
    }
    #[cfg(not(windows))]
    {
        let _ = (child_id, md_addr, md_len);
        0
    }
}

// ─── Forth-side event API ────────────────────────────────────────────
//
// `gpane-next-event ( child_id timeout-ms -- p4 p3 p2 p1 kind )`
//
// Pulls the next event matching `child_id` (or a global, e.g.
// FrameClose) from the iGui mailbox.  Non-matching events stash in
// EVENT_STASH and are picked up by the worker's normal drain loop
// when this returns, so infrastructure events (EvalBuffer,
// ReplSubmit, etc.) survive even while Forth is in its event loop.
//
// `timeout-ms < 0` blocks indefinitely.  On timeout / no event the
// kind is `EV_NONE = 0` and all params are 0 — same shape as a real
// event so Forth's stack effect stays predictable.
//
// Event-kind tags (mirror IGuiEvent variants); the Forth-side
// constants in `lib/core.f` use these values.
pub const EV_NONE:        i64 = 0;
pub const EV_KEY:         i64 = 1;
pub const EV_CHAR:        i64 = 2;
pub const EV_MOUSE:       i64 = 3;
pub const EV_FOCUS:       i64 = 4;
pub const EV_RESIZE:      i64 = 5;
pub const EV_CLOSE:       i64 = 6;
pub const EV_FRAME_CLOSE: i64 = 7;
pub const EV_TICK:        i64 = 13;

/// Decode an `IGuiEvent` into (kind, p1, p2, p3, p4).  Mirrors the
/// CP `write_event` packing in `cp_exports::write_event`.  Returns
/// `EV_NONE` for variants Forth doesn't care about (e.g.
/// `ForthRestart`).
#[cfg(windows)]
fn decode_event(
    ev: &crate::igui::channels::IGuiEvent,
) -> (i64, i64, i64, i64, i64) {
    use crate::igui::channels::IGuiEvent;
    match ev {
        IGuiEvent::Key { vkey, mods, repeat, down, .. } => (
            EV_KEY,
            *vkey,
            *mods,
            if *down { 1 } else { 0 },
            *repeat,
        ),
        IGuiEvent::Char { codepoint, mods, .. } => (EV_CHAR, *codepoint, *mods, 0, 0),
        IGuiEvent::Mouse { x, y, op, button, mods, .. } => (
            EV_MOUSE,
            *x,
            *y,
            *op,
            *mods | (*button << 8),
        ),
        IGuiEvent::Focus { gained, .. } => {
            (EV_FOCUS, if *gained { 1 } else { 0 }, 0, 0, 0)
        }
        IGuiEvent::Resize { width, height, .. } => (EV_RESIZE, *width, *height, 0, 0),
        IGuiEvent::Close { .. } => (EV_CLOSE, 0, 0, 0, 0),
        IGuiEvent::FrameClose => (EV_FRAME_CLOSE, 0, 0, 0, 0),
        IGuiEvent::Tick { time_ms, .. } => (EV_TICK, *time_ms, 0, 0, 0),
        // Infrastructure events Forth never receives via this path.
        IGuiEvent::DpiChange { .. }
        | IGuiEvent::ThemeChange
        | IGuiEvent::Menu { .. }
        | IGuiEvent::EvalBuffer { .. }
        | IGuiEvent::ForthRestart
        | IGuiEvent::ForthInterrupt
        | IGuiEvent::ReplSubmit { .. } => (EV_NONE, 0, 0, 0, 0),
    }
}

/// Block up to `timeout_ms` for the next event whose `child_id`
/// equals `child_id` (or which is a global like FrameClose).
/// Writes the decoded event into the five `out_*` slots.  Returns 1
/// if an event was returned, 0 on timeout.
///
/// On 0 return the slots are zeroed so Forth's stack effect stays
/// predictable: callers always pop the same five cells.
#[no_mangle]
#[allow(clippy::too_many_arguments)]
pub extern "C" fn rt_gpane_next_event_for(
    child_id: u64,
    timeout_ms: u64,
    out_kind: *mut i64,
    out_p1: *mut i64,
    out_p2: *mut i64,
    out_p3: *mut i64,
    out_p4: *mut i64,
) -> u64 {
    // Zero outputs up front so the timeout path doesn't need to.
    unsafe {
        if !out_kind.is_null() { *out_kind = 0; }
        if !out_p1.is_null()   { *out_p1   = 0; }
        if !out_p2.is_null()   { *out_p2   = 0; }
        if !out_p3.is_null()   { *out_p3   = 0; }
        if !out_p4.is_null()   { *out_p4   = 0; }
    }

    #[cfg(windows)]
    {
        let id = child_id as i64;
        let timeout = timeout_ms as i64;
        // Loop on EV_NONE: an event variant we don't surface to
        // Forth (e.g. DpiChange) gets stashed back for the main
        // drain, then we retry — instead of collapsing the
        // caller's timeout to 0 by returning prematurely.  With a
        // finite timeout this can extend the total wait slightly
        // if many infrastructure events arrive in a burst; that's
        // acceptable and rare in practice.
        loop {
            let Some(ev) = crate::igui::channels::next_event_for(id, timeout) else {
                return 0;
            };
            let (kind, p1, p2, p3, p4) = decode_event(&ev);
            if kind == EV_NONE {
                crate::igui::channels::stash_event(ev);
                continue;
            }
            unsafe {
                if !out_kind.is_null() { *out_kind = kind; }
                if !out_p1.is_null()   { *out_p1   = p1;   }
                if !out_p2.is_null()   { *out_p2   = p2;   }
                if !out_p3.is_null()   { *out_p3   = p3;   }
                if !out_p4.is_null()   { *out_p4   = p4;   }
            }
            return 1;
        }
    }
    #[cfg(not(windows))]
    {
        let _ = (child_id, timeout_ms, out_kind, out_p1, out_p2, out_p3, out_p4);
        0
    }
}

/// Compile-mode helper for `S$"`.  Allocates a LITERAL slot,
/// allocates a fresh `String` GC object copying `len` bytes from
/// `src_addr`, stores the tagged pointer into the literal slot,
/// and emits a 22-byte stub at HERE that pushes `[slot_addr]` at
/// runtime.  HERE is advanced past the stub.
///
/// Returns 0 on success; `u64::MAX` on allocation failure or
/// LITERAL region overflow.  Errors are surfaced to the caller as
/// throws (caller picks the throw code: -2058 for region overflow,
/// -2059 for alloc failure).  Stderr-logged for the developer.
///
/// # Safety
/// `up` must be a valid Forth user-area pointer; `src_addr` ..
/// `src_addr + len` must be readable.
#[no_mangle]
pub extern "C" fn rt_s_literal_compile_at_here(
    up: u64,
    src_addr: u64,
    len: u64,
) -> u64 {
    // Bounds-check the LITERAL region.
    let literal_base = up + crate::USER_LITERAL_BASE;
    let literal_limit = literal_base + crate::LITERAL_REGION_SIZE;
    let next_addr_ptr = (up + crate::USER_LITERAL_NEXT) as *mut u64;
    let cur_next = unsafe { *next_addr_ptr };
    if cur_next + 8 > literal_limit {
        eprintln!(r#"S$": LITERAL region full (next=0x{cur_next:x}, limit=0x{literal_limit:x})"#);
        return u64::MAX;
    }

    // Allocate a String and copy bytes.
    let tagged = rt_string_from_bytes(up, src_addr, len);
    if tagged == u64::MAX {
        return u64::MAX;
    }

    // Reserve the slot, store the tagged ptr, bump LITERAL_NEXT.
    let slot_addr = cur_next;
    unsafe {
        *(slot_addr as *mut u64) = tagged;
        *next_addr_ptr = cur_next + 8;
    }

    // Emit the runtime stub at HERE.  Stub pushes [slot_addr]
    // and falls through to the next compiled word in the colon
    // body — no `ret`, because the colon body is a sequence of
    // inline calls/operations, not a series of leaf words.
    //   mov [rbp-8], rax         (4)  48 89 45 F8   - spill TOS
    //   mov rax, imm64=slot_addr (10) 48 B8 + 8     - rax = slot addr
    //   mov rax, [rax]           (3)  48 8B 00      - rax = *slot
    //   sub rbp, 8                (4)  48 83 ED 08  - lower DSP
    // Total = 21 bytes.  Control falls through.
    let here = unsafe { *((up + RT_USER_HERE) as *const u64) };
    let dst = here as *mut u8;
    unsafe {
        *dst.add(0)  = 0x48; *dst.add(1)  = 0x89;
        *dst.add(2)  = 0x45; *dst.add(3)  = 0xF8;
        *dst.add(4)  = 0x48; *dst.add(5)  = 0xB8;
        write_u64_le(dst.add(6), slot_addr);
        *dst.add(14) = 0x48; *dst.add(15) = 0x8B; *dst.add(16) = 0x00;
        *dst.add(17) = 0x48; *dst.add(18) = 0x83;
        *dst.add(19) = 0xED; *dst.add(20) = 0x08;
        *((up + RT_USER_HERE) as *mut u64) = here + 21;
    }
    0
}

/// Byte-compare two `String` payloads.  Returns `u64::MAX` (-1, i.e.
/// Forth true) if the bytes match exactly, `0` otherwise.
///
/// The kernel-side `$=` is responsible for tag-checking both inputs
/// before calling — this function trusts that both pointers carry
/// `TAG_STRING` and that the headers' length fields are
/// authoritative.
///
/// # Safety
/// Both `tagged_a` and `tagged_b` must be valid `String` tagged
/// pointers.  Nil or wrong-typed inputs will deref invalid memory.
#[no_mangle]
pub extern "C" fn rt_string_bytes_equal(tagged_a: u64, tagged_b: u64) -> u64 {
    // Fast path: same object.
    if tagged_a == tagged_b {
        return u64::MAX;
    }
    let base_a = (tagged_a & !7) as *const u64;
    let base_b = (tagged_b & !7) as *const u64;
    // Header layout: bits[0..5]=type, [5..29]=length, [29..]=GC.
    // Compare lengths first.
    let len_a = unsafe { (*base_a >> 5) & 0xFF_FFFF };
    let len_b = unsafe { (*base_b >> 5) & 0xFF_FFFF };
    if len_a != len_b {
        return 0;
    }
    if len_a == 0 {
        return u64::MAX;
    }
    let payload_a = unsafe { (base_a as *const u8).add(8) };
    let payload_b = unsafe { (base_b as *const u8).add(8) };
    let slice_a = unsafe { std::slice::from_raw_parts(payload_a, len_a as usize) };
    let slice_b = unsafe { std::slice::from_raw_parts(payload_b, len_a as usize) };
    if slice_a == slice_b { u64::MAX } else { 0 }
}

// ── LET DSL compilation ──────────────────────────────────────────────
//
// `rt_let_compile(up)` is called by the kernel's immediate `LET` word.
// It reads the LET source from the current input buffer up to the next
// `END` token, compiles it via [`crate::let_lang`], JITs the result in
// a fresh module (kept alive in `LET_JITS`), and emits a Win64
// trampoline at HERE that loads inputs from the Forth FP stack,
// invokes the compiled function, and adjusts FSP.
//
// Returns 0 on success or `u64::MAX` (= -1 as i64) on any error;
// error details are printed to stderr.

use std::sync::atomic::{AtomicUsize, Ordering};
use wfasm::{CodeArena, Jit};

use crate::let_lang;

// User-area offsets — keep in sync with macros.masm.
const RT_USER_SOURCE_ADDR: u64 = 0x30;
const RT_USER_SOURCE_LEN:  u64 = 0x38;
const RT_USER_TO_IN:       u64 = 0x40;
const RT_USER_HERE:        u64 = 0x18;
const RT_USER_FSP:         u64 = 0x1218;
const RT_USER_JIT_ARENA_BASE: u64 = 0x1810; // near RWX arena base (set at boot)
const RT_USER_JIT_ARENA_SIZE: u64 = 0x1818; // near RWX arena size

/// One process-wide near JIT code arena, lazily built from the user-area
/// cells the session published at boot. Leaked so it lives for the whole
/// session; its bump offset accumulates across every CODE:/LET word, all of
/// which land rel32-reachable from the kernel/dict.
thread_local! {
    static JIT_ARENA: std::cell::Cell<*mut CodeArena> =
        const { std::cell::Cell::new(std::ptr::null_mut()) };
}

/// Get (or lazily create) the near code arena. Returns null only if the
/// session never published one (e.g. a unit test with a bare user area).
unsafe fn jit_code_arena(up: u64) -> *mut CodeArena {
    JIT_ARENA.with(|cell| {
        let p = cell.get();
        if !p.is_null() { return p; }
        let base = unsafe { read_u64(up + RT_USER_JIT_ARENA_BASE) };
        let size = unsafe { read_u64(up + RT_USER_JIT_ARENA_SIZE) };
        if base == 0 || size == 0 { return std::ptr::null_mut(); }
        // 8-byte code header so the xt back-offset cell lands at
        // [fn_addr - cell], exactly like a boot-time primitive.
        let arena = Box::new(CodeArena::with_code_header(base as *mut u8, size as usize, 8));
        let raw = Box::into_raw(arena);
        cell.set(raw);
        raw
    })
}

thread_local! {
    /// Compiled LET functions live in their own JIT modules.  We keep
    /// every Jit alive for the duration of the session so the executable
    /// pages don't get freed under us when a colon definition still
    /// holds a CALL to the compiled function pointer.
    static LET_JITS: RefCell<Vec<Jit>> = RefCell::new(Vec::new());
}

/// Counter for generating unique LET function names. Persists for the
/// process lifetime; we don't reuse names because old Jits may still hold
/// the old name (and that's fine, but a fresh counter avoids confusion).
static LET_COUNTER: AtomicUsize = AtomicUsize::new(0);

/// Drop every LET-compiled Jit. Called by session reset between tests.
pub fn reset_let_session() {
    LET_JITS.with(|j| j.borrow_mut().clear());
}

/// libm functions the LET codegen may reference, with their declared
/// arity.  Resolved via GetProcAddress on ucrtbase.dll (the Windows
/// C runtime); missing ones are simply not registered, so a LET that
/// doesn't use them still compiles fine.
const LIBM_FUNCTIONS: &[(&str, usize)] = &[
    ("sin", 1), ("cos", 1), ("tan", 1),
    ("asin", 1), ("acos", 1), ("atan", 1),
    ("exp", 1), ("log", 1), ("log2", 1), ("log10", 1),
    ("atan2", 2), ("pow", 2), ("hypot", 2), ("fmod", 2),
];

#[cfg(windows)]
mod win_libm {
    use std::ffi::CString;
    use std::os::raw::{c_char, c_void};

    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn LoadLibraryW(name: *const u16) -> *mut c_void;
        fn GetProcAddress(module: *mut c_void, name: *const c_char) -> *mut c_void;
    }

    /// Get a handle to ucrtbase.dll. It's a process-global module — the
    /// C runtime is already loaded — so LoadLibraryW just bumps the
    /// refcount.  Cached per-thread to avoid hammering LoadLibrary on
    /// every LET.
    pub fn ucrtbase_handle() -> *mut c_void {
        thread_local! {
            static HANDLE: std::cell::Cell<*mut c_void> = const { std::cell::Cell::new(std::ptr::null_mut()) };
        }
        HANDLE.with(|h| {
            let cur = h.get();
            if !cur.is_null() {
                return cur;
            }
            // UTF-16 "ucrtbase.dll\0"
            let wname: Vec<u16> = "ucrtbase.dll".encode_utf16().chain(std::iter::once(0)).collect();
            let new = unsafe { LoadLibraryW(wname.as_ptr()) };
            h.set(new);
            new
        })
    }

    /// GetProcAddress wrapper. Returns None on missing symbol.
    pub fn proc_addr(module: *mut c_void, name: &str) -> Option<*mut c_void> {
        if module.is_null() {
            return None;
        }
        let cname = CString::new(name).ok()?;
        let addr = unsafe { GetProcAddress(module, cname.as_ptr()) };
        if addr.is_null() { None } else { Some(addr) }
    }
}

/// Resolve every libm function in LIBM_FUNCTIONS via GetProcAddress.
/// Returns the (name → host address) table used by the LET codegen to
/// bake direct `mov rax, addr; call rax` sequences into compiled LETs.
/// Missing symbols are simply omitted; the codegen produces an error
/// only if the user's LET references one that's absent.
#[cfg(windows)]
pub fn libm_address_table() -> let_lang::LibmTable {
    let mut t = let_lang::LibmTable::new();
    let h = win_libm::ucrtbase_handle();
    if h.is_null() { return t; }
    for &(name, _arity) in LIBM_FUNCTIONS {
        if let Some(addr) = win_libm::proc_addr(h, name) {
            t.insert(name.to_string(), addr as u64);
        }
    }
    t
}

#[cfg(not(windows))]
pub fn libm_address_table() -> let_lang::LibmTable {
    let_lang::LibmTable::new()
}

unsafe fn read_u64(addr: u64) -> u64 { unsafe { *(addr as *const u64) } }
unsafe fn write_u64(addr: u64, val: u64) { unsafe { *(addr as *mut u64) = val } }

/// Compile a LET form from the current input buffer.
///
/// # Safety
/// `up` must point to a valid Forth user area whose SOURCE_ADDR /
/// SOURCE_LEN / TO_IN / HERE fields are correctly maintained by the
/// kernel.
#[no_mangle]
pub extern "C" fn rt_let_compile(up: u64) -> u64 {
    match unsafe { try_compile_let(up) } {
        Ok(()) => 0,
        Err(msg) => {
            eprintln!("LET compile error: {msg}");
            u64::MAX
        }
    }
}

unsafe fn try_compile_let(up: u64) -> Result<(), String> {
    let src_base = unsafe { read_u64(up + RT_USER_SOURCE_ADDR) };
    let src_len  = unsafe { read_u64(up + RT_USER_SOURCE_LEN)  };
    let to_in    = unsafe { read_u64(up + RT_USER_TO_IN)        };

    if to_in > src_len {
        return Err(format!("TO_IN ({to_in}) past SOURCE_LEN ({src_len})"));
    }

    let remaining = unsafe {
        std::slice::from_raw_parts(
            (src_base + to_in) as *const u8,
            (src_len - to_in) as usize,
        )
    };

    let (body_bytes, consumed) = find_end_token(remaining)
        .ok_or_else(|| "no closing 'END' token in LET body".to_string())?;
    let body_str = std::str::from_utf8(body_bytes)
        .map_err(|_| "LET body is not UTF-8".to_string())?;

    // Our parser starts at `LET`; the keyword was already consumed by
    // the Forth interpreter before dispatching to our word.  Prepend
    // it back, plus the closing END, so parser sees a complete form.
    let source = format!("LET{body_str}END");

    let counter = LET_COUNTER.fetch_add(1, Ordering::SeqCst);
    let fn_name = format!("let_user_{counter:04}");

    let libm_table = libm_address_table();
    let compiled = let_lang::compile(&source, &fn_name, &libm_table)
        .map_err(|e| e.to_string())?;

    // Compile into a fresh JIT module so the main kernel module stays
    // frozen and we don't fight MCJIT's whole-module finalization rule.
    let mut jit = Jit::new(&format!("let_mod_{counter:04}"))
        .map_err(|e| format!("Jit::new: {e:?}"))?;
    jit.add_asm(&compiled.asm_text)
        .map_err(|e| format!("add_asm: {e:?}\nasm was:\n{}", compiled.asm_text))?;
    jit.declare_fn(&compiled.fn_name, 0)
        .map_err(|e| format!("declare_fn({}): {e:?}", compiled.fn_name))?;
    let fn_addr = jit.lookup_addr(&compiled.fn_name)
        .map_err(|e| format!("lookup_addr({}): {e:?}", compiled.fn_name))?;

    LET_JITS.with(|j| j.borrow_mut().push(jit));

    let here = unsafe { read_u64(up + RT_USER_HERE) };
    let trampoline_len = unsafe {
        emit_let_trampoline(here, fn_addr, compiled.n_inputs, compiled.n_outputs)
    };
    unsafe { write_u64(up + RT_USER_HERE, here + trampoline_len as u64); }
    unsafe { write_u64(up + RT_USER_TO_IN, to_in + consumed as u64); }
    Ok(())
}

/// Find the next "END" token in `src` (whitespace-delimited).
/// Returns (body-before-END, total-bytes-consumed-including-END).
fn find_end_token(src: &[u8]) -> Option<(&[u8], usize)> {
    let mut i = 0;
    while i + 3 <= src.len() {
        if &src[i..i + 3] == b"END" {
            let prev_ok = i == 0 || !is_ident_byte(src[i - 1]);
            let next_ok = i + 3 == src.len() || !is_ident_byte(src[i + 3]);
            if prev_ok && next_ok {
                return Some((&src[..i], i + 3));
            }
        }
        i += 1;
    }
    None
}

fn is_ident_byte(b: u8) -> bool { b.is_ascii_alphanumeric() || b == b'_' }

/// Emit Win64 trampoline at `here` calling fn_addr with rcx = FSP and
/// rdx = FSP + delta, then bumping FSP by delta where delta = (n_in - n_out)*8.
/// Returns the number of bytes emitted.
unsafe fn emit_let_trampoline(here: u64, fn_addr: u64, n_in: usize, n_out: usize) -> usize {
    let delta: i64 = (n_in as i64 - n_out as i64) * 8;
    let delta_i32: i32 = delta as i32;
    let dst = here as *mut u8;
    let mut p: usize = 0;

    // Preserve the Forth machine registers this trampoline would
    // otherwise destroy.  RAX is the cached TOS; r12 is callee-saved and
    // we reuse it below to stash RSP across the call's stack alignment.
    // Neither is restored without this — a SINGLE LET call hides it (the
    // data stack is usually empty at the call), but calling a LET word
    // inside a Forth loop corrupts TOS every iteration and leaves r12
    // clobbered.  Save both on the return stack; they survive the callee
    // (Win64 callee-saved) and the `and rsp,-16` below still aligns the
    // call correctly regardless of these two extra pushes.
    unsafe {
        // push rax :: 50   — save Forth TOS
        *dst.add(p) = 0x50; p += 1;
        // push r12 :: 41 54 — save callee-saved r12
        *dst.add(p) = 0x41; p += 1;
        *dst.add(p) = 0x54; p += 1;
    }

    // mov rcx, qword ptr [rbx + USER_FSP] :: 48 8B 8B disp32
    unsafe {
        *dst.add(p) = 0x48; p += 1;
        *dst.add(p) = 0x8B; p += 1;
        *dst.add(p) = 0x8B; p += 1;
        write_i32(dst.add(p), RT_USER_FSP as i32); p += 4;
    }

    // rdx = rcx + delta
    if delta == 0 {
        unsafe {
            // mov rdx, rcx :: 48 89 CA
            *dst.add(p) = 0x48; p += 1;
            *dst.add(p) = 0x89; p += 1;
            *dst.add(p) = 0xCA; p += 1;
        }
    } else if (-128..=127).contains(&delta) {
        unsafe {
            // lea rdx, [rcx + imm8] :: 48 8D 51 imm8
            *dst.add(p) = 0x48; p += 1;
            *dst.add(p) = 0x8D; p += 1;
            *dst.add(p) = 0x51; p += 1;
            *dst.add(p) = (delta as i8) as u8; p += 1;
        }
    } else {
        unsafe {
            // lea rdx, [rcx + imm32] :: 48 8D 91 imm32
            *dst.add(p) = 0x48; p += 1;
            *dst.add(p) = 0x8D; p += 1;
            *dst.add(p) = 0x91; p += 1;
            write_i32(dst.add(p), delta_i32); p += 4;
        }
    }

    // mov r12, rsp :: 49 89 E4
    unsafe {
        *dst.add(p) = 0x49; p += 1;
        *dst.add(p) = 0x89; p += 1;
        *dst.add(p) = 0xE4; p += 1;
        // and rsp, -16 :: 48 83 E4 F0
        *dst.add(p) = 0x48; p += 1;
        *dst.add(p) = 0x83; p += 1;
        *dst.add(p) = 0xE4; p += 1;
        *dst.add(p) = 0xF0; p += 1;
        // sub rsp, 32 :: 48 83 EC 20
        *dst.add(p) = 0x48; p += 1;
        *dst.add(p) = 0x83; p += 1;
        *dst.add(p) = 0xEC; p += 1;
        *dst.add(p) = 0x20; p += 1;
        // mov rax, imm64 :: 48 B8 [8 bytes]
        *dst.add(p) = 0x48; p += 1;
        *dst.add(p) = 0xB8; p += 1;
        write_u64_le(dst.add(p), fn_addr); p += 8;
        // call rax :: FF D0
        *dst.add(p) = 0xFF; p += 1;
        *dst.add(p) = 0xD0; p += 1;
        // mov rsp, r12 :: 4C 89 E4
        *dst.add(p) = 0x4C; p += 1;
        *dst.add(p) = 0x89; p += 1;
        *dst.add(p) = 0xE4; p += 1;
        // pop r12 :: 41 5C — restore the original callee-saved r12
        *dst.add(p) = 0x41; p += 1;
        *dst.add(p) = 0x5C; p += 1;
        // pop rax :: 58 — restore the Forth TOS
        *dst.add(p) = 0x58; p += 1;
    }

    // Adjust FSP by delta.
    if delta == 0 {
        // nothing to emit
    } else if (-128..=127).contains(&delta) {
        unsafe {
            // add qword ptr [rbx + USER_FSP], imm8 :: 48 83 83 disp32 imm8
            *dst.add(p) = 0x48; p += 1;
            *dst.add(p) = 0x83; p += 1;
            *dst.add(p) = 0x83; p += 1;
            write_i32(dst.add(p), RT_USER_FSP as i32); p += 4;
            *dst.add(p) = (delta as i8) as u8; p += 1;
        }
    } else {
        unsafe {
            // add qword ptr [rbx + USER_FSP], imm32 :: 48 81 83 disp32 imm32
            *dst.add(p) = 0x48; p += 1;
            *dst.add(p) = 0x81; p += 1;
            *dst.add(p) = 0x83; p += 1;
            write_i32(dst.add(p), RT_USER_FSP as i32); p += 4;
            write_i32(dst.add(p), delta_i32); p += 4;
        }
    }

    p
}

unsafe fn write_i32(dst: *mut u8, val: i32) {
    let bytes = val.to_le_bytes();
    unsafe { std::ptr::copy_nonoverlapping(bytes.as_ptr(), dst, 4); }
}

unsafe fn write_u64_le(dst: *mut u8, val: u64) {
    let bytes = val.to_le_bytes();
    unsafe { std::ptr::copy_nonoverlapping(bytes.as_ptr(), dst, 8); }
}

// ── CODE: DSL compilation ────────────────────────────────────────────
//
// `rt_code_compile_body(up)` is the worker behind the `CODE:` immediate
// word.  It reads the assembly source from the current input buffer up
// to the next `;CODE` token, wraps it in a `proc(...)` / `endp()` pair,
// hands it to a thread-local JASM `Assembler` (preloaded with
// `macros.masm`, so the user's source can use `proc/endp/next/pushd/stk`
// etc. naturally), JIT-compiles into a fresh module, and returns the
// resulting function address.  The kernel's `CODE:` word then builds
// the dict header and emits a 12-byte JMP trampoline at HERE that
// transfers control to the compiled function.
//
// Returns the function address on success, 0 on any error (details
// printed to stderr).

const MACROS_SOURCE: &str = include_str!("../kernel/macros.masm");

thread_local! {
    /// Each compiled CODE: word lives in its own JIT module.  We keep
    /// the Jit alive for the session lifetime so its executable memory
    /// stays mapped while colon definitions still reference the
    /// function via the trampoline.
    static CODE_JITS: RefCell<Vec<Jit>> = RefCell::new(Vec::new());

    /// Shared JASM Assembler pre-loaded with `macros.masm`. Stored as
    /// `Option` so we lazily initialise on first use — at that point the
    /// kernel layout is already established and macros.masm parses cleanly.
    static CODE_ASSEMBLER: RefCell<Option<wfasm::Assembler>> = const { RefCell::new(None) };
}

static CODE_COUNTER: AtomicUsize = AtomicUsize::new(0);

pub fn reset_code_session() {
    CODE_JITS.with(|j| j.borrow_mut().clear());
    // CODE_ASSEMBLER intentionally kept — re-bootstrapping macros.masm
    // for every reset would be wasteful, and its expansion-time state
    // (defines, assigns, macros) doesn't accumulate per call.
}

/// Compile the next CODE: body in the input buffer.
///
/// Returns the address of the JIT-compiled function on success, or 0
/// on any failure (the kernel surfaces this as a THROW).
#[no_mangle]
pub extern "C" fn rt_code_compile_body(up: u64) -> u64 {
    match unsafe { try_compile_code(up) } {
        Ok(addr) => addr,
        Err(msg) => {
            eprintln!("CODE: compile error: {msg}");
            0
        }
    }
}

unsafe fn try_compile_code(up: u64) -> Result<u64, String> {
    let src_base = unsafe { read_u64(up + RT_USER_SOURCE_ADDR) };
    let src_len  = unsafe { read_u64(up + RT_USER_SOURCE_LEN)  };
    let to_in    = unsafe { read_u64(up + RT_USER_TO_IN)        };

    if to_in > src_len {
        return Err(format!("TO_IN ({to_in}) past SOURCE_LEN ({src_len})"));
    }

    // Assemble the full CODE: body, which may span multiple input lines
    // when the user types it across several REPL lines.  We scan first
    // the current SOURCE buffer (rest of THIS line) and then, if no
    // `;CODE` is found there, the Io's input buffer past `in_cursor`.
    let current_tail = unsafe {
        std::slice::from_raw_parts(
            (src_base + to_in) as *const u8,
            (src_len - to_in) as usize,
        )
    };

    let body_string: String;
    let consumed_in_current: usize;
    let consumed_from_buffer: usize;

    if let Some((body, n)) = find_code_terminator(current_tail) {
        // Body fits on the current line.
        body_string = std::str::from_utf8(body)
            .map_err(|_| "CODE: body is not UTF-8".to_string())?
            .to_string();
        consumed_in_current = n;
        consumed_from_buffer = 0;
    } else {
        // Need to peek into the Io input buffer for additional lines.
        let current_tail_str = std::str::from_utf8(current_tail)
            .map_err(|_| "CODE: source is not UTF-8".to_string())?;
        let (extra, n_from_buf) = peek_until_code_terminator()
            .ok_or_else(|| "no closing ';CODE' token found before EOF".to_string())?;
        // Combine: current line tail + newline + extra
        body_string = format!("{current_tail_str}\n{extra}");
        consumed_in_current = current_tail.len();
        consumed_from_buffer = n_from_buf;
    }
    let body_str = &body_string;

    let counter = CODE_COUNTER.fetch_add(1, Ordering::SeqCst);
    let fn_label = format!("code_user_{counter:04}");

    // Wrap in proc/endp so the user can write idiomatic kernel asm with
    // `next()` / `pushd` / `popd` / `stk(in,out)` / etc.  We auto-emit a
    // trailing `next()` (= ret) so the user doesn't have to remember it,
    // but if they wrote their own that just becomes dead bytes.
    let asm_source = format!(
        ".intel_syntax noprefix\n\
         .text\n\
         proc({fn_label})\n\
         {body_str}\n\
         next()\n\
         endp()\n",
    );

    let mc_text = with_code_assembler(|asm| -> Result<String, String> {
        asm.assemble(&format!("code_body_{counter:04}"), &asm_source)
            .map_err(|e| format!("{e}"))
    })?;

    // Assemble straight into the near code arena, so the function lands
    // rel32-reachable from the kernel/dict — the word's xt points right at
    // it (no far-segment trampoline, no byte copy). The arena is host-owned,
    // so we drop the engine immediately; the emitted code lives on.
    let arena = unsafe { jit_code_arena(up) };
    if arena.is_null() {
        return Err("JIT code arena not initialised (session published no arena)".to_string());
    }
    let mut jit = Jit::new_in_arena(&format!("code_mod_{counter:04}"), arena)
        .map_err(|e| format!("Jit::new_in_arena: {e:?}"))?;
    jit.add_asm(&mc_text)
        .map_err(|e| format!("add_asm: {e:?}\nasm was:\n{mc_text}"))?;
    jit.declare_fn(&fn_label, 0)
        .map_err(|e| format!("declare_fn({fn_label}): {e:?}"))?;
    let fn_addr = jit.lookup_addr(&fn_label)
        .map_err(|e| format!("lookup_addr({fn_label}): {e:?}"))?;
    drop(jit); // code persists in the arena; engine no longer needed

    // Advance TO_IN past the consumed portion of the current line.
    unsafe {
        write_u64(up + RT_USER_TO_IN, to_in + consumed_in_current as u64);
    }
    // If we consumed lines from the Io buffer too, advance in_cursor.
    if consumed_from_buffer > 0 {
        advance_io_cursor(consumed_from_buffer);
    }
    Ok(fn_addr)
}

/// Peek into the current session's input past the kernel's `TO_IN`,
/// scanning for `;CODE`.  Returns (body_bytes_before_terminator,
/// bytes_consumed_from_buffered_input).
///
/// * Buffered mode: scans `input[in_cursor..]`, returns the consumed
///   byte count so the caller can advance `in_cursor`.
/// * Live mode: reads lines from stdin via `BufRead::read_line` until
///   it sees `;CODE`.  Stdin has been advanced past those lines as a
///   side effect, so the kernel's next refill picks up after `;CODE`.
///   `consumed` is irrelevant in this mode — returned as 0.
///
/// Returns None on EOF or read error.
fn peek_until_code_terminator() -> Option<(String, usize)> {
    // First snapshot: is the current Io Buffered or Live?  We can't
    // hold the borrow across stdin reads (would deadlock on Live mode's
    // re-entry into with_current_io somewhere downstream), so check
    // first, release, then act.
    let mode = CURRENT_IO.with(|cell| {
        cell.borrow().as_ref().map(|io| matches!(io, Io::Buffered { .. }))
    })?;

    if mode {
        // Buffered: scan the input vec directly.
        CURRENT_IO.with(|cell| {
            let borrow = cell.borrow();
            let io = borrow.as_ref()?;
            if let Io::Buffered { input, in_cursor, .. } = io {
                let rest = &input[*in_cursor..];
                let (body, consumed) = find_code_terminator(rest)?;
                let body_str = std::str::from_utf8(body).ok()?.to_string();
                Some((body_str, consumed))
            } else {
                None
            }
        })
    } else {
        // Live: read lines from stdin until we see ;CODE.  Each
        // BufRead::read_line consumes from stdin, so the kernel's
        // next refill picks up after our last consumed line.
        use std::io::{self, BufRead};
        let stdin = io::stdin();
        let mut handle = stdin.lock();
        let mut accumulator = String::new();
        loop {
            let mut line = String::new();
            match handle.read_line(&mut line) {
                Ok(0) => return None,             // EOF
                Err(_) => return None,
                Ok(_) => {
                    // Normalise line endings.
                    while line.ends_with('\n') || line.ends_with('\r') {
                        line.pop();
                    }
                    if let Some((body, _)) = find_code_terminator(line.as_bytes()) {
                        let body_str = std::str::from_utf8(body).ok()?;
                        accumulator.push_str(body_str);
                        return Some((accumulator, 0));
                    }
                    accumulator.push_str(&line);
                    accumulator.push('\n');
                }
            }
        }
    }
}

/// Advance the Io::Buffered in_cursor by `n` bytes. Only meaningful in
/// Buffered mode; no-op in Live.
fn advance_io_cursor(n: usize) {
    CURRENT_IO.with(|cell| {
        let mut borrow = cell.borrow_mut();
        if let Some(Io::Buffered { in_cursor, .. }) = borrow.as_mut() {
            *in_cursor += n;
        }
    });
}

fn find_code_terminator(src: &[u8]) -> Option<(&[u8], usize)> {
    const TAG: &[u8] = b";CODE";
    let mut i = 0;
    while i + TAG.len() <= src.len() {
        if &src[i..i + TAG.len()] == TAG {
            let prev_ok = i == 0 || src[i - 1].is_ascii_whitespace();
            let next_ok = i + TAG.len() == src.len() || src[i + TAG.len()].is_ascii_whitespace();
            if prev_ok && next_ok {
                // Consume the trailing newline (and any preceding CR), so
                // the next refill picks up at the START of the line after
                // ;CODE instead of seeing an empty line.
                let mut consumed = i + TAG.len();
                if consumed < src.len() && src[consumed] == b'\r' { consumed += 1; }
                if consumed < src.len() && src[consumed] == b'\n' { consumed += 1; }
                return Some((&src[..i], consumed));
            }
        }
        i += 1;
    }
    None
}

fn with_code_assembler<R>(
    f: impl FnOnce(&mut wfasm::Assembler) -> Result<R, String>,
) -> Result<R, String> {
    CODE_ASSEMBLER.with(|cell| {
        let mut borrowed = cell.borrow_mut();
        if borrowed.is_none() {
            let mut asm = wfasm::Assembler::new();
            asm.register_macro("stk", wfasm::asm::macros::stk);
            // Preload kernel macros (proc, endp, next, pushd, popd, stk,
            // win64_call, brk, plus the @assigns for cell / user-area
            // offsets / tfa constants).
            asm.assemble("macros.masm", MACROS_SOURCE)
                .map_err(|e| format!("preload macros.masm: {e}"))?;
            *borrowed = Some(asm);
        }
        f(borrowed.as_mut().unwrap())
    })
}

/// Write to current output. Buffered: append to vec. Live: stdout + flush.
fn write_bytes(bytes: &[u8]) {
    with_current_io(|io| match io {
        Io::Buffered { output, .. } => output.extend_from_slice(bytes),
        Io::Live { .. } => {
            let mut out = std::io::stdout();
            let _ = out.write_all(bytes);
            let _ = out.flush();
        }
    });
}
