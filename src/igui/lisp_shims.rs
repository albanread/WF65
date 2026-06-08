//! Forth-callable shims for iGui (placeholder — Phase 1).
//!
//! Originally `lisp_shims.rs` in NewCormanLisp; it bridged the iGui
//! mailbox to native Lisp functions.  WF64's equivalent will be a
//! set of `@extern` runtime functions called from Forth `CODE:`
//! words.  In Phase 1 (scaffolding) the bridge isn't wired up yet —
//! the only consumer is the eventual `wf64-ui` binary which starts
//! the GUI thread directly.
//!
//! Kept in this file so the iGui module imports continue to
//! resolve.  When the Forth-side surface lands (`igui-start`,
//! `igui-open-child`, etc.) it will move here behind a clean
//! `extern "C" fn rt_igui_*` API.

#![cfg(windows)]
#![allow(dead_code)]

use std::sync::OnceLock;
use std::thread::JoinHandle;

/// JoinHandle for the GUI thread, parked here so a Forth-side
/// `igui-wait` (when it lands) can block on the message pump
/// exiting.
pub(crate) static GUI_THREAD: OnceLock<std::sync::Mutex<Option<JoinHandle<()>>>> =
    OnceLock::new();
