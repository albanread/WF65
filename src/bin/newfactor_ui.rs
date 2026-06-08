//! `newfactor-ui` — the NewFactor IDE.
//!
//! Reuses WF64's iGui MDI front-end (Direct2D / DirectWrite) unchanged.
//! Replaces the WF64 Forth session backend with [`FactorSession`]:
//!
//!   - The user types ANS Forth syntax in the REPL or editor pane.
//!   - The Rust transpiler converts Forth → Factor source.
//!   - The Factor VM (running in-process via `factor.dll`) evaluates it.
//!   - Output and the data stack are displayed exactly as in WF64.
//!
//! ## Architecture
//!
//! ```text
//! newfactor-ui.exe  (one Windows process)
//! ├── GUI thread     Direct2D MDI, Win32 message pump (wf64::igui)
//! │     ↕ IGuiEvent MPSC channel (wf64::igui::channels)
//! └── Worker thread  owns FactorSession
//!       transpiler   Forth source → Factor source
//!       factor_session  pipes to/from the Factor listener thread
//!             └── Factor thread  start_standalone_factor_in_new_thread
//!                     reads stdin pipe, writes stdout pipe
//! ```
//!
//! ## Differences from wf64-ui
//!
//! | | wf64-ui | newfactor-ui |
//! |---|---|---|
//! | Backend | WF64 JIT Forth (`Wf64Session`) | Factor VM (`FactorSession`) |
//! | Session boot | Assemble kernel + bootstrap dict | Load factor.dll + image |
//! | Core library | `lib/core.f` loaded at boot | `forth.all` loaded at boot |
//! | Stack cushion | 8 dummy cells pre-pushed | n/a (Factor GC'd stack) |
//! | Window symbol | `∴` (therefore) | `∿` (sine wave, ≈ "Factor") |
//!
//! Run with:
//!   cargo run --bin newfactor-ui

#[cfg(windows)]
fn main() -> Result<(), Box<dyn std::error::Error>> {
    wf64::igui::crash_handler::install();

    let worker = || {
        wait_for_frame();
        auto_open_console();
        run_supervisor();
    };
    let exit_code = wf64::igui::run(Some(worker))?;
    std::process::exit(exit_code);
}

// ── Supervisor ────────────────────────────────────────────────────────────

/// Supervisor loop: wraps the worker so that SEH crashes can be reported
/// and a fresh session rebooted.
#[cfg(windows)]
fn run_supervisor() {
    use wf64::igui::{crash_handler, crash_view};

    loop {
        let join = std::thread::Builder::new()
            .name("nf-worker".into())
            .spawn(|| {
                crash_handler::register_worker_thread();
                run_factor_worker();
                crash_handler::unregister_worker_thread();
            });
        let join = match join {
            Ok(j) => j,
            Err(e) => {
                eprintln!("[supervisor] could not spawn worker: {e}");
                return;
            }
        };
        // Swallow the std-lib panic that occurs when a VEH-redirected
        // SEH thread exits abnormally.
        let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _ = join.join();
        }));
        match crash_handler::take_dump() {
            Some(dump) => {
                let text = crash_handler::format_dump(&dump);
                crash_view::push(text);
                wf64::igui::fconsole::append("∿ Factor thread crashed (SEH) — rebooting.");
                wf64::igui::fconsole::append("");
                // loop: respawn
            }
            None => return,
        }
    }
}

// ── Worker ────────────────────────────────────────────────────────────────

#[cfg(windows)]
fn run_factor_worker() {
    use std::panic::{catch_unwind, AssertUnwindSafe};
    use wf64::igui::fconsole;

    loop {
        let mut session = match boot_session(true) {
            Some(s) => s,
            None => {
                eprintln!("[newfactor-ui] session boot failed; worker exiting");
                return;
            }
        };
        wf64::igui::stack_view::publish(session.stack());

        let result = catch_unwind(AssertUnwindSafe(move || run_drain_loop(session)));
        match result {
            Ok(()) => return,
            Err(payload) => {
                report_panic(payload);
                fconsole::reset_for_restart();
                fconsole::append("∿ Factor session crashed — rebooting.");
                fconsole::append("");
            }
        }
    }
}

#[cfg(windows)]
fn run_drain_loop(mut session: wf64::newfactor::factor_session::FactorSession) {
    use wf64::igui::channels::{self, IGuiEvent};
    use wf64::igui::fconsole;

    loop {
        let Some(ev) = channels::next_event(200) else { continue };

        match ev {
            IGuiEvent::EvalBuffer { source } => {
                handle_eval(&mut session, &source);
                wf64::igui::stack_view::publish(session.stack());
            }
            IGuiEvent::ForthRestart => {
                fconsole::reset_for_restart();
                drop(session);
                fconsole::append("∿ restart requested — fresh Factor session below.");
                fconsole::append("");
                match boot_session(false) {
                    Some(s) => session = s,
                    None => return,
                }
                wf64::igui::stack_view::publish(session.stack());
            }
            IGuiEvent::ReplSubmit { child_id } => {
                use wf64::igui::repl_pane::{self, AppendKind};
                let Some(source) = repl_pane::pop_input(child_id) else { continue };
                match session.eval(&source) {
                    Ok(output) => {
                        let body = output.trim_matches('\n');
                        if body.is_empty() {
                            repl_pane::append(child_id, "ok".into(), AppendKind::Output);
                        } else {
                            for line in body.lines() {
                                repl_pane::append(
                                    child_id,
                                    line.to_string(),
                                    AppendKind::Output,
                                );
                            }
                        }
                    }
                    Err(e) => {
                        repl_pane::append(child_id, e.to_string(), AppendKind::Error);
                    }
                }
                wf64::igui::stack_view::publish(session.stack());
            }
            IGuiEvent::FrameClose => {
                fconsole::append("∿ frame closing");
                return;
            }
            _ => {}
        }
    }
}

// ── Boot / eval helpers ───────────────────────────────────────────────────

#[cfg(windows)]
fn boot_session(
    intro: bool,
) -> Option<wf64::newfactor::factor_session::FactorSession> {
    use wf64::igui::fconsole;

    if intro {
        fconsole::append("∿ NewFactor IDE");
        fconsole::append("");
        fconsole::append("Factor VM starting (loading image + forth.all)…");
        fconsole::append("This takes a few seconds on first boot.");
        fconsole::append("");
        fconsole::append(
            "Type ANS Forth in the prompt, press Enter.  \
             Control structures (IF/ELSE/THEN, BEGIN/UNTIL…) are supported.",
        );
        fconsole::append(
            "Editor: Ctrl+Shift+E   Console: Ctrl+Shift+R   Restart: Ctrl+Shift+F5",
        );
        fconsole::append("");
    }

    match wf64::newfactor::factor_session::FactorSession::new() {
        Ok(s) => {
            fconsole::append("∿ Factor VM ready — forth.all loaded.");
            fconsole::append("");
            Some(s)
        }
        Err(e) => {
            fconsole::append(&format!("∿ session boot failed: {e}"));
            None
        }
    }
}

#[cfg(windows)]
fn handle_eval(
    session: &mut wf64::newfactor::factor_session::FactorSession,
    source: &str,
) {
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
            let body = output.trim_matches('\n');
            if body.is_empty() {
                fconsole::append("ok");
            } else {
                for line in body.lines() {
                    fconsole::append(line);
                }
            }
        }
        Err(e) => {
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
            // Show surviving stack.
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

// ── Startup helpers ───────────────────────────────────────────────────────

#[cfg(windows)]
fn wait_for_frame() {
    use std::time::Duration;
    for _ in 0..200 {
        if wf64::igui::cp_exports::FRAME_HWND.get().is_some() {
            return;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    eprintln!("[newfactor-ui] FRAME_HWND not published after 4 s; continuing anyway");
}

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

// ── Panic reporting ───────────────────────────────────────────────────────

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
    dump.push_str("kind:    Rust panic\n");
    dump.push_str(&format!("message: {msg}\n"));
    dump.push('\n');
    dump.push_str("The Factor session has been dropped.\n");
    dump.push_str("A fresh session will be booted below.\n");
    crash_view::push(dump);
}

#[cfg(not(windows))]
fn main() {
    eprintln!("newfactor-ui is Windows-only (iGui depends on Direct2D / DirectWrite).");
    std::process::exit(1);
}
