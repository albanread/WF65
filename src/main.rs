//! Interactive WF64 REPL.
//!
//! All the heavy lifting lives in `wf64::Wf64Session`. This binary is
//! just the live-stdio shim around it: build a session, hook it to
//! stdin/stdout, run quit until the user types `bye` or stdin hits EOF.

use anyhow::Result;
use wf64::Wf64Session;

fn main() -> Result<()> {
    // new() assembles the kernel, bootstraps the dictionary, and loads
    // lib/core.f — the session is ready to use as returned.
    let mut session = Wf64Session::new()?;
    session.run_interactive()?;
    Ok(())
}
