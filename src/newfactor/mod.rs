//! NewFactor IDE support.
//!
//! Two components:
//!
//! - [`transpiler`] — stateful Forth→Factor source transpiler (Rust
//!   reimplementation of `forth.preparser`).
//!
//! - [`factor_session`] — in-process Factor VM session.  Starts Factor's
//!   listener in a dedicated thread via `start_standalone_factor_in_new_thread`
//!   from `factor.dll`, then communicates with it through anonymous Windows
//!   pipes using a simple sentinel-based protocol.

pub mod transpiler;
pub mod factor_session;
