//! Build script for WF64.
//!
//! Embeds the Windows application manifest (`tools/wf64-ui.exe.manifest`)
//! into the `wf64-ui` binary so the IDE gets:
//!   - Per-monitor-v2 DPI awareness (crisp Direct2D on hi-DPI).
//!   - Common Controls v6 visual styles for dialogs.
//!   - UTF-8 active code page (∴ + non-ASCII filenames).
//!   - supportedOS GUIDs through Windows 11.
//!
//! On non-Windows builds the embed is skipped — `wf64-ui` is a
//! `cfg(windows)` binary anyway.

fn main() {
    println!("cargo:rerun-if-changed=tools/wf64-ui.rc");
    println!("cargo:rerun-if-changed=tools/wf64-ui.exe.manifest");

    #[cfg(target_os = "windows")]
    {
        embed_resource::compile("tools/wf64-ui.rc", embed_resource::NONE)
            .manifest_required()
            .unwrap();
    }
}
