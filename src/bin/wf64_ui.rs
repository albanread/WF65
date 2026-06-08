// Release builds ship as a Windows GUI-subsystem app: no console
// window pops up when the user launches the IDE from Explorer.
// Dev / debug builds keep the console attached so `eprintln!` traces
// are visible in the cargo-run terminal.  Controlled by the
// `gui_subsystem` Cargo feature (enabled by tools/build-release.ps1).
#![cfg_attr(
    all(windows, feature = "gui_subsystem"),
    windows_subsystem = "windows"
)]

//! `wf64-ui` — the WF64 Forth IDE front-end.
//!
//! Phase 2b: spawns a worker thread that owns a `Wf64Session`,
//! loads `lib/core.f`, and drains `IGuiEvent::EvalBuffer` events
//! from the iGui mailbox.  Each event's captured stdout is pushed
//! to the log overlay, followed by a ` ok` line — the standard
//! Forth REPL prompt convention.
//!
//! F5 in the editor pane sends the buffer's text as an
//! EvalBuffer event; this worker is what services it.
//!
//! Single-symbol convention: `∴` (U+2234, "therefore") prefixes
//! the frame and child titles.  Three dots stacked vertically —
//! visually a Forth data stack, mathematically the "therefore"
//! glyph that postfix proof-style reasoning earns.
//!
//! Run with:
//!   cargo run --bin wf64-ui

#[cfg(windows)]
fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Phase 3b: install the SEH crash handler BEFORE spawning
    // anything that could trigger it.  Worker thread spawning
    // happens inside igui::run; the supervisor below registers
    // itself with the handler before doing any Forth work.
    wf64::igui::crash_handler::install();

    let worker = || {
        wait_for_frame();
        auto_open_console();
        run_supervisor();
    };
    let exit_code = wf64::igui::run(Some(worker))?;
    std::process::exit(exit_code);
}

/// Supervisor — wraps the actual Forth worker so that SEH-caught
/// crashes (which exit the worker thread cleanly via our
/// VEH-redirect-to-ExitThread thunk) get detected, reported, and
/// recovered from by spawning a fresh worker thread.
///
/// Three exit paths from the worker thread:
///   - Clean return (FrameClose): `take_dump` is None, supervisor
///     also returns and the iGui shuts down.
///   - Rust panic: caught inside the worker by `catch_unwind`,
///     reported via `crash_view::push`, session rebooted within
///     the same thread — supervisor sees nothing.
///   - SEH exception: VEH rewrites RIP to ExitThread, thread
///     dies, supervisor's join returns Ok(()), `take_dump`
///     yields the captured state, we report and respawn.
#[cfg(windows)]
fn run_supervisor() {
    use wf64::igui::{crash_handler, crash_view};

    loop {
        let join = std::thread::Builder::new()
            .name("wf64-worker".into())
            .spawn(|| {
                crash_handler::register_worker_thread();
                run_forth_worker();
                crash_handler::unregister_worker_thread();
            });
        let join = match join {
            Ok(j) => j,
            Err(e) => {
                eprintln!("[supervisor] could not spawn worker: {e}");
                return;
            }
        };
        // join.join() blocks until the worker thread exits.  WHEN
        // our VEH catches an SEH and redirects RIP → ExitThread,
        // the worker exits "abnormally" from Rust's POV: std's
        // thread-lifecycle bookkeeping then panics from INSIDE
        // join() with "threads should not terminate unexpectedly".
        // That panic unwinds OUT of join, bypassing `let _`, so
        // we wrap the join itself in catch_unwind to swallow it
        // and continue to the dump-check / respawn step.
        let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _ = join.join();
        }));
        match crash_handler::take_dump() {
            Some(dump) => {
                let text = crash_handler::format_dump(&dump);
                crash_view::push(text);
                wf64::igui::fconsole::append("∴ Forth thread crashed (SEH) — rebooting.");
                wf64::igui::fconsole::append("");
                // loop: respawn the worker
            }
            None => {
                // Clean exit — no dump pending, nothing to report.
                return;
            }
        }
    }
}

/// Block until the frame HWND is published.  The frame is created
/// after the worker is spawned, so the worker has to wait before it
/// can post WM_COMMAND messages to it.
#[cfg(windows)]
fn wait_for_frame() {
    use std::time::Duration;
    for _ in 0..200 {  // up to 4s
        if wf64::igui::cp_exports::FRAME_HWND.get().is_some() {
            return;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    eprintln!("[wf64-ui] FRAME_HWND not published after 4 s; continuing anyway");
}

/// Post WM_COMMAND so the console pane opens on startup.  fedit
/// stays closed — open it via Ctrl+Shift+E when you want it.  The
/// log overlay (Ctrl+Shift+L) is also opt-in.
#[cfg(windows)]
fn auto_open_console() {
    use windows::Win32::Foundation::{HWND, LPARAM, WPARAM};
    use windows::Win32::UI::WindowsAndMessaging::{PostMessageW, WM_COMMAND};
    let Some(&hwnd_isize) = wf64::igui::cp_exports::FRAME_HWND.get() else {
        return;
    };
    let hwnd = HWND(hwnd_isize as *mut _);
    let cmd_id = wf64::igui::fconsole::MENU_CMD_ID;
    let _ = unsafe {
        PostMessageW(
            Some(hwnd),
            WM_COMMAND,
            WPARAM(cmd_id as usize),
            LPARAM(0),
        )
    };
}

/// Top-level worker loop with crash recovery.  Each iteration
/// runs the actual session-drain loop inside `catch_unwind`; on
/// any panic we capture the message, push to the crash-dump
/// view (which auto-opens), drop the panicked session, and boot
/// a fresh one.  The process keeps running.
///
/// Doesn't catch SEH exceptions yet (Windows access-violations
/// from JIT'd code) — those still take down the worker thread.
/// Phase 3b plans a VEH-based recovery that redirects RIP to a
/// thread-exit thunk so the process survives even those.
#[cfg(windows)]
fn run_forth_worker() {
    use std::panic::{catch_unwind, AssertUnwindSafe};
    use wf64::igui::fconsole;

    loop {
        let session = match boot_session(true) {
            Some(s) => s,
            None => {
                eprintln!("[wf64-ui] session boot failed; worker exiting");
                return;
            }
        };
        // Publish the initial (empty) stack so the stack viewer
        // shows "depth: 0" instead of stale data from a previous run.
        wf64::igui::stack_view::publish(session.stack());
        // catch_unwind takes ownership of session — on panic it
        // gets dropped here, freeing the Wf64Session's heap/kernel
        // arena.  On clean exit (Ok(())) we return from the worker.
        let result = catch_unwind(AssertUnwindSafe(|| {
            run_drain_loop(session)
        }));
        match result {
            Ok(()) => return,
            Err(payload) => {
                report_panic(payload);
                fconsole::reset_for_restart();
                fconsole::append("∴ session crashed — rebooting Forth.");
                fconsole::append("");
                // loop continues; boot_session brings up a new one
            }
        }
    }
}

/// The original drain loop — extracted so `run_forth_worker` can
/// wrap it in `catch_unwind`.  Takes the session by value so it
/// gets dropped on panic-unwind.
#[cfg(windows)]
fn run_drain_loop(mut session: wf64::Wf64Session) {
    use wf64::igui::channels::{self, IGuiEvent};
    use wf64::igui::fconsole;

    loop {
        let twait = std::time::Instant::now();
        let Some(ev) = channels::next_event(200) else {
            continue;
        };
        wf64::igui::fconsole::trace(
            "worker",
            format_args!("recv after {}us", twait.elapsed().as_micros()),
        );
        match ev {
            IGuiEvent::EvalBuffer { source } => {
                let teval = std::time::Instant::now();
                handle_eval(&mut session, &source);
                wf64::igui::fconsole::trace(
                    "worker",
                    format_args!("eval ({} bytes) took {}us",
                        source.len(), teval.elapsed().as_micros()),
                );
                wf64::igui::stack_view::publish(session.stack());
                // Deferred-panic check: `bug-rust-panic` from
                // Forth set this flag during the eval; panic NOW,
                // in pure-Rust context, so unwinding is sound
                // (panic from inside extern "C" → JIT'd asm is UB).
                if wf64::runtime::BUG_PANIC_PENDING.swap(
                    false, std::sync::atomic::Ordering::SeqCst,
                ) {
                    panic!("bug-rust-panic triggered from Forth — testing crash recovery");
                }
            }
            IGuiEvent::ForthRestart => {
                fconsole::reset_for_restart();
                drop(session);
                fconsole::append("∴ restart requested — fresh session below.");
                fconsole::append("");
                match boot_session(false) {
                    Some(s) => session = s,
                    None => return,
                }
                wf64::igui::stack_view::publish(session.stack());
            }
            IGuiEvent::ReplSubmit { child_id } => {
                use wf64::igui::repl_pane::{self, AppendKind};
                let Some(source) = repl_pane::pop_input(child_id) else {
                    continue;
                };
                match session.eval(&source) {
                    Ok(output) => {
                        let trimmed = output.trim_end_matches('\n');
                        if !trimmed.is_empty() {
                            repl_pane::append(
                                child_id,
                                trimmed.to_string(),
                                AppendKind::Output,
                            );
                        }
                    }
                    Err(e) => {
                        // Include any output captured before the throw
                        // (may be empty; the trim below handles that).
                        let msg = e.to_string();
                        repl_pane::append(child_id, msg, AppendKind::Error);
                    }
                }
                wf64::igui::stack_view::publish(session.stack());
                if wf64::runtime::BUG_PANIC_PENDING.swap(
                    false, std::sync::atomic::Ordering::SeqCst,
                ) {
                    panic!("bug-rust-panic triggered from Forth — testing crash recovery");
                }
            }
            IGuiEvent::FrameClose => {
                fconsole::append("∴ frame closing");
                return;
            }
            _ => {}
        }
    }
}

/// Format a panic payload into a multi-line dump and push it
/// to the crash view.  Captures: panic message, thread name,
/// captured backtrace if RUST_BACKTRACE=1 (else a hint).
#[cfg(windows)]
fn report_panic(payload: Box<dyn std::any::Any + Send>) {
    use wf64::igui::crash_view;
    let msg: String = if let Some(s) = payload.downcast_ref::<&'static str>() {
        s.to_string()
    } else if let Some(s) = payload.downcast_ref::<String>() {
        s.clone()
    } else {
        "<panic payload not a string>".to_string()
    };
    let thread = std::thread::current()
        .name()
        .map(|s| s.to_string())
        .unwrap_or_else(|| format!("{:?}", std::thread::current().id()));
    let ts = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| format!("{}.{:03}", d.as_secs(), d.subsec_millis()))
        .unwrap_or_else(|_| "<no time>".into());

    let mut dump = String::new();
    dump.push_str(&format!("when:    {ts}\n"));
    dump.push_str(&format!("thread:  {thread}\n"));
    dump.push_str(&format!("kind:    Rust panic\n"));
    dump.push_str(&format!("message: {msg}\n"));
    dump.push_str("\n");
    dump.push_str("Session has been dropped and a fresh one booted below.\n");
    dump.push_str("Any user definitions from before the crash are gone.\n");
    dump.push_str("\n");
    dump.push_str("(SEH exceptions — access-violations from JIT'd Forth code —\n");
    dump.push_str("are NOT yet recovered: those still take the worker thread\n");
    dump.push_str("down.  Phase 3b will add a VEH-based recovery path.)\n");

    crash_view::push(dump);
}

/// Create a session, load core.f, emit the startup banner.  Used
/// both for the initial boot and for Forth → Restart.  `intro`
/// controls whether the welcome lines print (we skip them after
/// a restart since the console already shows a "restart requested"
/// notice).
#[cfg(windows)]
fn boot_session(intro: bool) -> Option<wf64::Wf64Session> {
    use std::path::Path;
    use wf64::igui::fconsole;

    if intro {
        fconsole::append("∴ WF64 — Forth IDE");
        fconsole::append("");
        fconsole::append("Type at the prompt, press Enter.");
        fconsole::append("Editor: Ctrl+Shift+E   Console: Ctrl+Shift+R   Restart: Ctrl+Shift+F5");
        fconsole::append("");
    }

    let mut session = match wf64::Wf64Session::new() {
        Ok(s) => s,
        Err(e) => {
            fconsole::append(&format!("∴ session boot failed: {e}"));
            return None;
        }
    };

    // lib/core.f is now loaded automatically by Wf64Session::new()
    // before taking the boot snapshot — all standard words are already
    // present.  Resolve the same path the session would have used so
    // the status banner reports the actual file (release-packaged
    // binaries find it next to the exe; dev runs find it in the repo).
    let core_path = wf64::default_kernel_path()
        .parent()
        .and_then(|p| p.parent())
        .map(|p| p.join("lib").join("core.f"))
        .unwrap_or_else(|| Path::new("lib").join("core.f"));
    fconsole::append(&format!("∴ loaded {}", core_path.display()));
    // Preload a cushion of zeros so a couple of accidental
    // over-drops at the REPL don't immediately crash the worker.
    // Eight cells = one cache line; enough to recover from a
    // typo, not so many that `.s` looks cluttered.
    const STACK_CUSHION: usize = 8;
    session.push_stack_cushion(STACK_CUSHION);
    fconsole::append(&format!(
        "∴ {STACK_CUSHION} cushion cells preloaded (drop them or ignore)"
    ));
    // No manual " ok" — the include itself emits one through the
    // captured IO stream when it succeeds.
    fconsole::append("");
    Some(session)
}

/// Run one source chunk through the session and pipe the result
/// to the console.  Single-line submissions get the plain
/// "<output> ok" treatment a real REPL does; multi-line buffers
/// (F5 from fedit) get a header / footer so the transcript is
/// scannable.
#[cfg(windows)]
fn handle_eval(session: &mut wf64::Wf64Session, source: &str) {
    use wf64::igui::fconsole;
    let multiline = source.lines().count() > 1;
    if multiline {
        fconsole::append("─── eval ───");
        for line in source.lines().take(8) {
            fconsole::append(line);
        }
        let extra = source.lines().count().saturating_sub(8);
        if extra > 0 {
            fconsole::append(&format!("    … {extra} more line(s) elided"));
        }
        fconsole::append("─── result ───");
    }
    match session.eval(source) {
        Ok(output) => {
            // The kernel emits ` ok\n` after every successful eval.
            // Strip it: when the eval also produced user output, the
            // trailing "ok" is just noise (especially for the multi-
            // line demos that fill the screen).  On an empty success
            // we still show a faint "ok" so the user knows the eval
            // ran (e.g. ` : foo 1 2 + ; ` defines a word and prints
            // nothing else).
            let trimmed = output.trim_end_matches('\n');
            let body = trimmed
                .strip_suffix(" ok")
                .map(|s| s.trim_end_matches('\n'))
                .unwrap_or(trimmed);
            if body.is_empty() {
                fconsole::append("ok");
            } else {
                for line in body.lines() {
                    fconsole::append(line);
                }
            }
        }
        Err(e) => {
            // First line: ⚠ + human-readable error description.
            // Remaining lines: output captured before the throw fired
            // (e.g. interpreter's "FOO ? " marker, partial print output).
            let msg = e.to_string();
            let mut lines = msg.lines();
            if let Some(first) = lines.next() {
                fconsole::append(&format!("⚠ {first}"));
            }
            for ln in lines {
                if !ln.is_empty() {
                    fconsole::append(&format!("  {ln}"));
                }
            }
            // Show the surviving stack so the user can see what's left.
            let stk = session.stack();
            if !stk.is_empty() {
                let items: Vec<String> = stk.iter().map(|v| v.to_string()).collect();
                fconsole::append(&format!(
                    "  stack ({} item{}): {}  ← TOS",
                    stk.len(),
                    if stk.len() == 1 { "" } else { "s" },
                    items.join("  "),
                ));
            }
        }
    }
}

#[cfg(not(windows))]
fn main() {
    eprintln!("wf64-ui is Windows-only (iGui depends on Direct2D / DirectWrite).");
    std::process::exit(1);
}
