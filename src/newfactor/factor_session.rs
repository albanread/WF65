//! Headless Factor VM session — runs `factor.com` as a child process.
//!
//! ## Why a subprocess, not in-process embedding?
//!
//! Earlier iterations of this module loaded `factor.dll` into our address
//! space and called `start_standalone_factor_in_new_thread`.  That approach
//! ran into three independent showstoppers:
//!
//!   1. `factor_vm::init_ffi()` calls `GetModuleHandle(NULL)`, which returns
//!      the **host EXE's** HMODULE rather than `factor.dll`'s.  Every later
//!      `GetProcAddress(NULL_dll, "primitive_xyz")` returns NULL → first
//!      primitive call dereferences a NULL function pointer.
//!
//!   2. Factor's C++ runtime captures CRT `stdin`/`stdout` FILE* pointers
//!      directly.  In a GUI-subsystem process those FILE*s have
//!      `_fileno == -2` (NO_ASSOCIATED_STREAM), so Factor falls back to
//!      `fopen("nul", …)` regardless of any `_dup2` we do.
//!
//!   3. Even past those, the stock `factor.image` launches `ui.tools`
//!      (Factor's full IDE) when no `-run=` is supplied — exactly the
//!      "Factor IDE next to our window" surprise we want to avoid.
//!
//! All three are someone else's choice, baked deep into a binary we don't
//! build.  Fighting them in-process is expensive AND fragile.  Running
//! `factor.com` as a child process side-steps every one of them:
//!
//!   * The child gets its own CRT, its own stdio FILE* objects connected
//!     to our pipes by the OS — no `VALID_HANDLE` fallback, no _dup2 dance.
//!   * `factor.com` is the **console-subsystem** Factor binary; with
//!     `CREATE_NO_WINDOW` it spawns no visible window.
//!   * Crashes in the Factor VM kill the child only; the IDE survives and
//!     can respawn a fresh session.
//!
//! `factor.com` (~780 KB) plus `factor.dll` (~660 KB) plus the image
//! (currently 128 MB — a follow-up task is building a slimmer image
//! without UI/OpenGL/fonts) is our "headless Factor VM."
//!
//! ## Architecture
//!
//! ```text
//! Worker thread (owns FactorSession)
//! │   transpiler: Forth → Factor source
//! │
//! │   stdin_writer  ──────────►  factor.com (child process)
//! │                                  │ -run=listener
//! │   stdout_reader ◄──────────      │ stdin/stdout/stderr piped
//! ```
//!
//! ## Listener Suppression
//!
//! Factor's `listener-step` normally prints the data-stack and a vocab
//! prompt (`IN: scratchpad`) at the START of every step.  During bootstrap
//! we suppress both:
//!
//! ```factor
//! display-stacks? off           ! stop printing the data-stack between steps
//! M: object prompt. 2drop ;     ! redefine prompt method to do nothing
//! ```
//!
//! After that the stdout pipe contains only output explicitly produced by
//! user code (plus our sentinel lines, which we strip).
//!
//! ## Eval Protocol
//!
//! For each evaluation we send TWO listener-steps:
//!
//! ```factor
//! <transpiled-factor-code>
//! "%%NF-DONE%%\n" write flush
//! ```
//!
//! Line 1 is the user's expression.  If it throws, Factor's `call-error-hook`
//! prints a formatted error and `recover` keeps the listener alive.  Either
//! way, Factor reads Line 2 next and writes our sentinel — so the read side
//! always synchronizes.
//!
//! ## Stack Query
//!
//! ```factor
//! get-datastack [ dup integer? [ . ] [ drop ] if ] each
//! "%%NF-STACK%%\n" write flush
//! ```

use std::io::{self, BufRead, Write};
use std::path::Path;
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};
use std::time::{Duration, Instant};

use anyhow::{Context, Result};

use crate::newfactor::transpiler::Transpiler;

// ── Configuration ─────────────────────────────────────────────────────────

/// Headless console-subsystem Factor launcher.  Same VM as `factor.exe`;
/// links subsystem 3 (Windows console) instead of 2 (Windows GUI), so with
/// `CREATE_NO_WINDOW` it spawns no visible window.
pub const FACTOR_COM: &str = "E:\\factor\\factor.com";

/// Path to the Factor image.
pub const FACTOR_IMAGE: &str = "E:\\NewFactor\\factor.image";

/// Root of the NewFactor repo (vocab root for `forth.all`).
pub const NEWFACTOR_ROOT: &str = "E:\\NewFactor";

/// Sentinel written by Factor after each eval (or error recovery).
const SENTINEL_DONE:  &str = "%%NF-DONE%%";
/// Sentinel for the startup readiness probe.
const SENTINEL_READY: &str = "%%NF-READY%%";
/// Sentinel ending a stack dump.
const SENTINEL_STACK: &str = "%%NF-STACK%%";

/// Maximum time to wait for any sentinel response.
const SENTINEL_TIMEOUT: Duration = Duration::from_secs(60);

// ── FactorSession ─────────────────────────────────────────────────────────

/// A headless Factor VM session backed by a `factor.com` child process.
pub struct FactorSession {
    transpiler: Transpiler,
    /// `factor.com` child process.  Killed in `Drop`.
    child: Child,
    /// BufWriter around the child's stdin.  `Option` so `Drop` can `.take()`
    /// it and half-close stdin before waiting/killing the child.
    stdin_writer: Option<io::BufWriter<ChildStdin>>,
    /// BufReader around the child's stdout.
    stdout_reader: io::BufReader<ChildStdout>,
    /// Last known data stack (integers only).
    data_stack: Vec<i64>,
}

impl FactorSession {
    // ── Construction ──────────────────────────────────────────────────

    /// Spawn `factor.com`, wait for the listener to be ready, then bootstrap
    /// `forth.all` and silence listener noise.
    pub fn new() -> Result<Self> {
        eprintln!("[NF] spawning {FACTOR_COM}");

        // CREATE_NO_WINDOW = 0x08000000 — suppresses the brief console flash
        // that would otherwise appear when a GUI-subsystem process spawns
        // a console-subsystem child.
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;

        let mut child = {
            use std::os::windows::process::CommandExt;
            Command::new(FACTOR_COM)
                .arg(format!("-i={FACTOR_IMAGE}"))
                .arg("-run=listener")  // override main-vocab-hook → no IDE
                .arg("-no-user-init")  // skip ~/.factor-rc
                .arg("-no-signals")    // we own the signal handlers
                .arg("-q")             // suppress version banner
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .creation_flags(CREATE_NO_WINDOW)
                .spawn()
                .with_context(|| format!("spawn {FACTOR_COM}"))?
        };

        eprintln!("[NF] spawned PID={}", child.id());

        // Take ownership of the piped stdio.  These `Option`s are `Some`
        // because we configured `Stdio::piped()` for all three streams.
        let stdin  = child.stdin .take().context("child stdin missing")?;
        let stdout = child.stdout.take().context("child stdout missing")?;

        // Drain stderr in a background thread so it never blocks the child
        // by filling the pipe buffer.  We just log it (could surface in
        // the UI later if useful).
        if let Some(stderr) = child.stderr.take() {
            std::thread::Builder::new()
                .name("nf-stderr-drain".into())
                .spawn(move || drain_child_stderr(stderr))
                .ok();
        }

        let mut session = FactorSession {
            transpiler:    Transpiler::new(),
            child,
            stdin_writer:  Some(io::BufWriter::new(stdin)),
            stdout_reader: io::BufReader::new(stdout),
            data_stack:    Vec::new(),
        };

        eprintln!("[NF] probing for listener readiness");
        session.wait_for_ready().context("Factor startup probe")?;
        eprintln!("[NF] listener responsive — bootstrapping");
        session.bootstrap().context("forth.all bootstrap")?;
        eprintln!("[NF] bootstrap complete");

        Ok(session)
    }

    // ── Startup / bootstrap ───────────────────────────────────────────

    /// Send a probe through the listener and wait for it to echo back.
    /// This is how we know the image is fully loaded and the listener is
    /// reading from its stdin.
    fn wait_for_ready(&mut self) -> Result<()> {
        self.send_raw("\"%%NF-READY%%\\n\" write flush\n")
            .context("send ready probe")?;
        self.read_until_sentinel(SENTINEL_READY, SENTINEL_TIMEOUT)
            .context("wait for Factor ready sentinel")?;
        Ok(())
    }

    /// Load `forth.all` and silence the listener's between-step noise.
    fn bootstrap(&mut self) -> Result<()> {
        let root_escaped = NEWFACTOR_ROOT.replace('\\', "\\\\");

        // Each `\n` is one listener-step.
        //
        //   display-stacks? off
        //       Stops the listener printing the data-stack at the START of
        //       every step.
        //
        //   M: object prompt. 2drop ;
        //       Stops the listener printing `IN: scratchpad\n` at the start
        //       of every step.  Redefining the catch-all method is permanent.
        let boot = format!(concat!(
            "USE: vocabs.loader\n",
            "\"{root}\" add-vocab-root\n",
            "USE: forth.all\n",
            "display-stacks? off\n",
            "M: object prompt. 2drop ;\n",
            "\"%%NF-READY%%\\n\" write flush\n",
        ), root = root_escaped);

        self.send_raw(&boot).context("send bootstrap")?;
        self.read_until_sentinel(SENTINEL_READY, SENTINEL_TIMEOUT)
            .context("wait for bootstrap ready sentinel")?;
        Ok(())
    }

    // ── Low-level I/O ─────────────────────────────────────────────────

    fn send_raw(&mut self, code: &str) -> Result<()> {
        let stdin = self
            .stdin_writer
            .as_mut()
            .context("Factor stdin already closed")?;
        stdin
            .write_all(code.as_bytes())
            .context("write to Factor stdin pipe")?;
        stdin
            .flush()
            .context("flush Factor stdin pipe")?;
        Ok(())
    }

    /// Read from Factor's stdout until a line consists solely of `sentinel`.
    ///
    /// Returns all output lines that appeared before the sentinel
    /// (with their trailing newlines preserved).
    fn read_until_sentinel(&mut self, sentinel: &str, timeout: Duration) -> Result<String> {
        let deadline = Instant::now() + timeout;
        let mut output = String::new();
        let mut line = String::new();

        loop {
            if Instant::now() > deadline {
                anyhow::bail!(
                    "timed out ({timeout:?}) waiting for Factor sentinel {sentinel:?}"
                );
            }
            line.clear();
            let n = self
                .stdout_reader
                .read_line(&mut line)
                .context("read from Factor stdout pipe")?;
            if n == 0 {
                anyhow::bail!(
                    "Factor stdout EOF while waiting for sentinel {sentinel:?} \
                     (child likely exited)"
                );
            }
            let trimmed = line.trim_end_matches(['\r', '\n']);
            if trimmed == sentinel {
                return Ok(output);
            }
            output.push_str(&line);
        }
    }

    // ── Public API ────────────────────────────────────────────────────

    /// Evaluate Forth source code.
    ///
    /// The code is transpiled to Factor, sent to the listener, and the
    /// captured output is returned.  On error, Factor's `call-error-hook`
    /// prints a formatted error and `recover` keeps the listener alive;
    /// the error text appears in the returned string.
    pub fn eval(&mut self, forth_source: &str) -> Result<String> {
        let factor_code = self.transpiler.transpile(forth_source);

        // Two listener-steps:
        //   Step 1 — transpiled Factor code (may produce output / error)
        //   Step 2 — write the done-sentinel; runs after any step-1 error
        let msg = format!(
            "{factor_code}\n\
             \"{SENTINEL_DONE}\\n\" write flush\n"
        );

        self.send_raw(&msg).context("send eval to Factor")?;
        self.read_until_sentinel(SENTINEL_DONE, SENTINEL_TIMEOUT)
            .context("receive eval output from Factor")
    }

    /// Query the current Factor data stack.  Returns integers only, TOS last.
    pub fn stack(&mut self) -> Vec<i64> {
        let probe = format!(
            "get-datastack \
             [ dup integer? [ . ] [ drop ] if ] each\n\
             \"{SENTINEL_STACK}\\n\" write flush\n"
        );
        if self.send_raw(&probe).is_err() {
            return self.data_stack.clone();
        }
        match self.read_until_sentinel(SENTINEL_STACK, Duration::from_secs(5)) {
            Ok(raw) => {
                let parsed: Vec<i64> = raw
                    .lines()
                    .filter_map(|l| l.trim().parse().ok())
                    .collect();
                self.data_stack = parsed;
            }
            Err(e) => eprintln!("[FactorSession::stack] {e}"),
        }
        self.data_stack.clone()
    }

    /// Reset the transpiler ctrl-stack.  Does NOT restart the Factor VM.
    pub fn reset(&mut self) {
        self.transpiler.reset();
    }

    /// Load a source file.
    /// `.fth` files go through `forth-load`; `.factor` files use `load-file`.
    pub fn load_source_file(&mut self, path: &Path) -> Result<()> {
        let path_str = path.to_string_lossy();
        let escaped  = path_str.replace('\\', "\\\\");

        let code = if path.extension().and_then(|e| e.to_str()) == Some("fth") {
            format!("\"{escaped}\" forth-load")
        } else {
            format!("\"{escaped}\" load-file")
        };

        self.eval(&code)
            .with_context(|| format!("load {}", path.display()))?;
        Ok(())
    }
}

impl Drop for FactorSession {
    /// Best-effort: half-close stdin (so the listener sees EOF and exits
    /// cleanly via `quit`), then wait briefly, then kill if needed.
    fn drop(&mut self) {
        let pid = self.child.id();

        // Drop the BufWriter; this closes the write half of the stdin pipe,
        // which signals EOF to the listener loop in the child.
        drop(self.stdin_writer.take());

        let deadline = Instant::now() + Duration::from_millis(500);
        loop {
            match self.child.try_wait() {
                Ok(Some(_)) => return, // child exited on its own — good
                Ok(None) => {
                    if Instant::now() >= deadline {
                        let _ = self.child.kill();
                        let _ = self.child.wait();
                        eprintln!("[NF] killed factor.com (PID {pid})");
                        return;
                    }
                    std::thread::sleep(Duration::from_millis(20));
                }
                Err(e) => {
                    eprintln!("[NF] try_wait on child PID {pid}: {e}");
                    let _ = self.child.kill();
                    return;
                }
            }
        }
    }
}

// ── Helpers ───────────────────────────────────────────────────────────────

/// Background-thread loop that drains the child's stderr so it can't
/// block by filling the pipe buffer.  Each line is prefixed and printed
/// to our own stderr for diagnostics.
fn drain_child_stderr(stderr: std::process::ChildStderr) {
    use std::io::Read;
    let mut reader = io::BufReader::new(stderr);
    let mut buf = [0u8; 4096];
    loop {
        match reader.read(&mut buf) {
            Ok(0) => return, // EOF — child exited
            Ok(n) => {
                let s = String::from_utf8_lossy(&buf[..n]);
                for line in s.lines() {
                    if !line.is_empty() {
                        eprintln!("[factor stderr] {line}");
                    }
                }
            }
            Err(_) => return,
        }
    }
}
