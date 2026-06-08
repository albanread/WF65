//! Phase 3b — Vectored-Exception-Handler-based recovery for
//! Windows SEH exceptions (access violations, illegal instructions,
//! divide-by-zero, privileged-instruction).  When the worker
//! thread takes a SEH while running JIT'd Forth code, our VEH:
//!
//!   1. Snapshots the register state + RIP + 16 stack qwords into
//!      a pre-allocated static buffer.  No allocation, no Mutex —
//!      this runs in the exception context where almost anything
//!      can deadlock.
//!   2. Rewrites the CONTEXT record's `Rip` to point at a tiny
//!      `crash_recovery_thunk` function whose body is `ExitThread(2)`.
//!   3. Returns `EXCEPTION_CONTINUE_EXECUTION`.
//!
//! The OS resumes the worker thread at the thunk's RIP; the thread
//! exits cleanly.  The supervisor thread (spawned alongside the
//! worker by `wf64-ui`) is parked on the worker's `JoinHandle`;
//! when it returns, the supervisor checks the buffer.  If populated,
//! it formats a dump, posts to the crash view, and spawns a fresh
//! worker.  If empty, the worker exited cleanly and the supervisor
//! exits too.
//!
//! We only intercept SEH from the registered worker thread —
//! exceptions from the UI thread or anywhere else pass through to
//! JASM's VEH and ultimately the OS unhandled-filter, as before.

#![cfg(windows)]

use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};

use windows::Win32::Foundation::{
    EXCEPTION_ACCESS_VIOLATION, EXCEPTION_ILLEGAL_INSTRUCTION,
    EXCEPTION_INT_DIVIDE_BY_ZERO, EXCEPTION_PRIV_INSTRUCTION,
    EXCEPTION_STACK_OVERFLOW,
};
use windows::Win32::System::Diagnostics::Debug::{
    AddVectoredExceptionHandler, CONTEXT, EXCEPTION_POINTERS, EXCEPTION_RECORD,
    EXCEPTION_CONTINUE_EXECUTION, EXCEPTION_CONTINUE_SEARCH,
};
use windows::Win32::System::Threading::{ExitThread, GetCurrentThreadId};

/// Thread ID of the registered worker thread.  Set by
/// `register_worker_thread` from inside the worker right before
/// it starts running Forth code.  VEH compares against this to
/// decide whether to intercept an exception (we don't want to
/// recover SEH on the UI thread — UI-thread bugs should surface,
/// not get silently retried).
static WORKER_TID: AtomicU32 = AtomicU32::new(0);

/// One-shot flag: `true` iff the VEH has filled CAPTURED.  Cleared
/// by `take_dump`.  Use SeqCst for clarity; the path isn't hot.
static CAPTURED: AtomicBool = AtomicBool::new(false);

/// Lock-free capture buffer.  Written exactly once per crash
/// (by the VEH on the worker thread), read exactly once (by the
/// supervisor thread after the worker exits).  Field layout
/// chosen for direct display — no need to mirror the full
/// Windows CONTEXT struct.
#[repr(C)]
pub struct CapturedDump {
    pub code: u32,
    pub flags: u32,
    pub rip: u64,
    pub rsp: u64,
    pub rbp: u64,
    pub rax: u64,
    pub rbx: u64,
    pub rcx: u64,
    pub rdx: u64,
    pub rsi: u64,
    pub rdi: u64,
    pub r8: u64,
    pub r9: u64,
    pub r10: u64,
    pub r11: u64,
    pub r12: u64,
    pub r13: u64,
    pub r14: u64,
    pub r15: u64,
    pub stack: [u64; 16],
    pub access_addr: u64,
    pub access_kind: u32,
    pub thread_id: u32,
}

impl CapturedDump {
    const fn zero() -> Self {
        Self {
            code: 0, flags: 0, rip: 0, rsp: 0, rbp: 0,
            rax: 0, rbx: 0, rcx: 0, rdx: 0, rsi: 0, rdi: 0,
            r8: 0, r9: 0, r10: 0, r11: 0, r12: 0, r13: 0, r14: 0, r15: 0,
            stack: [0u64; 16],
            access_addr: 0,
            access_kind: 0,
            thread_id: 0,
        }
    }
}

/// Writes happen in the VEH (one writer at a time per
/// exception); reads happen in the supervisor after the worker
/// dies.  The `CAPTURED` AtomicBool serialises read vs write.
/// `static mut` is OK here because access is gated by the flag —
/// readers must check `CAPTURED` first.
static mut CAPTURED_DUMP: CapturedDump = CapturedDump::zero();

/// Install the VEH.  Call once at process startup, BEFORE the
/// worker thread starts.  Idempotent (subsequent calls no-op).
pub fn install() {
    static INSTALLED: AtomicBool = AtomicBool::new(false);
    if INSTALLED.swap(true, Ordering::SeqCst) {
        return;
    }
    // First parameter = 1 → install at the FRONT of the VEH chain.
    // This means we get first crack at exceptions before JASM's
    // dumper; if we don't recognise / don't own the thread, we
    // return CONTINUE_SEARCH and JASM still gets to dump.
    let handle = unsafe { AddVectoredExceptionHandler(1, Some(veh_callback)) };
    if handle.is_null() {
        eprintln!("[crash_handler] AddVectoredExceptionHandler failed");
    }
}

/// Worker thread calls this from its own context right after
/// spawn, so the VEH knows which thread to intercept.  Setting
/// to 0 clears the registration.
pub fn register_worker_thread() {
    let tid = unsafe { GetCurrentThreadId() };
    WORKER_TID.store(tid, Ordering::Release);
}

pub fn unregister_worker_thread() {
    WORKER_TID.store(0, Ordering::Release);
}

/// Returns a fresh copy of the captured dump and clears the
/// flag.  Call from the supervisor after the worker's
/// `JoinHandle::join` returns.  `Some` → the worker died via a
/// caught SEH; `None` → clean exit, no recovery needed.
pub fn take_dump() -> Option<CapturedDump> {
    if !CAPTURED.swap(false, Ordering::Acquire) {
        return None;
    }
    // SAFETY: writes are gated by VEH (single writer per crash);
    // the AtomicBool above ensures we read only after the writer
    // is done.  The supervisor is the only reader.
    let dump = unsafe { std::ptr::read(&raw const CAPTURED_DUMP) };
    Some(dump)
}

/// Format the dump as a multi-line text block matching JASM's
/// style.  Run on the supervisor thread; allocates freely.
pub fn format_dump(d: &CapturedDump) -> String {
    let mut s = String::with_capacity(2048);
    s.push_str(&format!("kind:           SEH exception (worker thread)\n"));
    s.push_str(&format!("exception code: {}  ({})\n", format_code(d.code), exception_name(d.code)));
    if d.code == EXCEPTION_ACCESS_VIOLATION.0 as u32 {
        let kind = match d.access_kind {
            0 => "read",
            1 => "write",
            8 => "execute",
            _ => "?",
        };
        s.push_str(&format!("access:         {kind} at {:#018x}\n", d.access_addr));
    }
    s.push_str(&format!("thread id:      {}\n", d.thread_id));
    s.push_str(&format!("\n"));
    s.push_str(&format!("rip = {:#018x}   flags = {:#010x}\n", d.rip, d.flags));
    s.push_str(&format!("rax = {:#018x}   rbx = {:#018x}\n", d.rax, d.rbx));
    s.push_str(&format!("rcx = {:#018x}   rdx = {:#018x}\n", d.rcx, d.rdx));
    s.push_str(&format!("rsi = {:#018x}   rdi = {:#018x}\n", d.rsi, d.rdi));
    s.push_str(&format!("rbp = {:#018x}   rsp = {:#018x}\n", d.rbp, d.rsp));
    s.push_str(&format!("r8  = {:#018x}   r9  = {:#018x}\n", d.r8, d.r9));
    s.push_str(&format!("r10 = {:#018x}   r11 = {:#018x}\n", d.r10, d.r11));
    s.push_str(&format!("r12 = {:#018x}   r13 = {:#018x}\n", d.r12, d.r13));
    s.push_str(&format!("r14 = {:#018x}   r15 = {:#018x}\n", d.r14, d.r15));
    s.push_str(&format!("\n"));
    s.push_str(&format!("stack (16 qwords from rsp):\n"));
    for (i, qw) in d.stack.iter().enumerate() {
        s.push_str(&format!("  [rsp+{:>3}] {:#018x}\n", i * 8, qw));
    }
    s.push_str(&format!("\n"));
    s.push_str(&format!("Worker thread has been terminated and a fresh session\n"));
    s.push_str(&format!("booted below.  Any user definitions from before the\n"));
    s.push_str(&format!("crash are gone.\n"));
    s
}

fn format_code(c: u32) -> String {
    format!("0x{c:08x}")
}

fn exception_name(c: u32) -> &'static str {
    match c {
        x if x == EXCEPTION_ACCESS_VIOLATION.0 as u32     => "ACCESS_VIOLATION",
        x if x == EXCEPTION_ILLEGAL_INSTRUCTION.0 as u32  => "ILLEGAL_INSTRUCTION",
        x if x == EXCEPTION_INT_DIVIDE_BY_ZERO.0 as u32   => "INT_DIVIDE_BY_ZERO",
        x if x == EXCEPTION_PRIV_INSTRUCTION.0 as u32     => "PRIV_INSTRUCTION",
        x if x == EXCEPTION_STACK_OVERFLOW.0 as u32       => "STACK_OVERFLOW",
        _                                                  => "unknown SEH code",
    }
}

/// The VEH callback.  Runs in the context of the faulting
/// thread (on its stack — be MINIMAL here).  We do exactly
/// one mut-static write, two atomic stores, and one CONTEXT
/// field mutation — no allocation, no I/O, no Mutex.
unsafe extern "system" fn veh_callback(info: *mut EXCEPTION_POINTERS) -> i32 {
    if info.is_null() {
        return EXCEPTION_CONTINUE_SEARCH;
    }
    let ep: &EXCEPTION_POINTERS = unsafe { &*info };
    let er_ptr: *const EXCEPTION_RECORD = ep.ExceptionRecord;
    if er_ptr.is_null() {
        return EXCEPTION_CONTINUE_SEARCH;
    }
    let er: &EXCEPTION_RECORD = unsafe { &*er_ptr };
    let code = er.ExceptionCode.0 as u32;

    // Only handle fatal SEH codes we actually want to recover from.
    // Don't touch things like MS_VC_EXCEPTION (0x406D1388, thread
    // naming) or DBG_PRINTEXCEPTION_C (0x40010006, OutputDebugString).
    let recoverable = matches!(
        code,
        x if x == EXCEPTION_ACCESS_VIOLATION.0 as u32
          || x == EXCEPTION_ILLEGAL_INSTRUCTION.0 as u32
          || x == EXCEPTION_INT_DIVIDE_BY_ZERO.0 as u32
          || x == EXCEPTION_PRIV_INSTRUCTION.0 as u32
    );
    if !recoverable {
        return EXCEPTION_CONTINUE_SEARCH;
    }

    // Only intercept on the worker thread.  UI-thread SEH should
    // still take down the process so we notice and fix it.
    let current_tid = unsafe { GetCurrentThreadId() };
    let worker_tid = WORKER_TID.load(Ordering::Acquire);
    if worker_tid == 0 || current_tid != worker_tid {
        return EXCEPTION_CONTINUE_SEARCH;
    }

    let ctx_ptr: *mut CONTEXT = ep.ContextRecord;
    if ctx_ptr.is_null() {
        return EXCEPTION_CONTINUE_SEARCH;
    }
    let ctx: &mut CONTEXT = unsafe { &mut *ctx_ptr };

    // Capture into the static buffer.  SAFETY: VEH on the worker
    // is the only writer; supervisor reads only after the worker
    // exits (synchronised via the CAPTURED AtomicBool below).
    unsafe {
        let d = &mut *(&raw mut CAPTURED_DUMP);
        d.code  = code;
        d.flags = er.ExceptionFlags;
        d.rip   = ctx.Rip;
        d.rsp   = ctx.Rsp;
        d.rbp   = ctx.Rbp;
        d.rax   = ctx.Rax;
        d.rbx   = ctx.Rbx;
        d.rcx   = ctx.Rcx;
        d.rdx   = ctx.Rdx;
        d.rsi   = ctx.Rsi;
        d.rdi   = ctx.Rdi;
        d.r8    = ctx.R8;
        d.r9    = ctx.R9;
        d.r10   = ctx.R10;
        d.r11   = ctx.R11;
        d.r12   = ctx.R12;
        d.r13   = ctx.R13;
        d.r14   = ctx.R14;
        d.r15   = ctx.R15;
        d.thread_id = current_tid;
        if code == EXCEPTION_ACCESS_VIOLATION.0 as u32 && er.NumberParameters >= 2 {
            d.access_kind = er.ExceptionInformation[0] as u32;
            d.access_addr = er.ExceptionInformation[1] as u64;
        } else {
            d.access_kind = 0;
            d.access_addr = 0;
        }
        // 16 stack qwords from rsp.  Wrap in a try-style read —
        // if rsp is bogus (corrupt), reading would AV recursively,
        // which the VEH would catch.  Use a guarded copy via
        // ReadProcessMemory to avoid that.
        copy_stack_safely(ctx.Rsp as *const u64, &mut d.stack);
    }

    // Publish the dump.
    CAPTURED.store(true, Ordering::Release);

    // Redirect resumption to the thunk.  Don't touch rsp — the
    // thunk doesn't need a sane frame; ExitThread doesn't return.
    ctx.Rip = crash_recovery_thunk as usize as u64;

    EXCEPTION_CONTINUE_EXECUTION
}

/// Copy 16 qwords from rsp using ReadProcessMemory so a bogus
/// rsp doesn't AV us recursively.
unsafe fn copy_stack_safely(src: *const u64, dst: &mut [u64; 16]) {
    use windows::Win32::System::Diagnostics::Debug::ReadProcessMemory;
    use windows::Win32::System::Threading::GetCurrentProcess;
    let proc = unsafe { GetCurrentProcess() };
    let mut bytes_read: usize = 0;
    let dst_bytes = unsafe {
        std::slice::from_raw_parts_mut(
            dst.as_mut_ptr() as *mut u8,
            std::mem::size_of_val(dst),
        )
    };
    let _ = unsafe {
        ReadProcessMemory(
            proc,
            src as *const _,
            dst_bytes.as_mut_ptr() as *mut _,
            dst_bytes.len(),
            Some(&mut bytes_read),
        )
    };
    // Anything not read stays at the previous static value
    // (probably zero); harmless.
}

/// CPU resumes here after VEH rewrites RIP.  Just exits the
/// thread cleanly.  The supervisor's `JoinHandle::join` will
/// unblock and detect the captured dump.
///
/// Marked `extern "system"` so Windows can call it with a sane
/// ABI even from the post-VEH resumption point.
unsafe extern "system" fn crash_recovery_thunk() -> ! {
    // ExitThread doesn't return.  Use exit code 2 to distinguish
    // SEH-caught crash from any other thread exit path (Rust
    // panic = code 1 via std's default abort, clean return = 0).
    unsafe { ExitThread(2) };
}
