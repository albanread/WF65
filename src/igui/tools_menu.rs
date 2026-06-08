//! Frame-level menu bar and keyboard accelerators.
//!
//! Layout (standard Windows MDI app convention):
//!
//!   File   New, Open…, Save, Save As…, Exit
//!   Edit   Undo/Redo, Cut/Copy/Paste, Select All, word nav
//!   View   Console, REPL, Log, Crash dump
//!   Forth  Restart, Run Buffer
//!
//! File commands route to the active fedit child via the
//! EDIT_CMD forwarding range (so opening from inside fedit Just
//! Works); File→New / File→Exit are frame-level.
//!
//! View commands open or focus a built-in pane (singleton per
//! pane).  Forth commands fire IGuiEvents the worker drains.

#![cfg(windows)]

use windows::core::PCWSTR;
use windows::Win32::UI::WindowsAndMessaging::{
    AppendMenuW, CreateAcceleratorTableW, CreateMenu, CreatePopupMenu, ACCEL, FCONTROL, FSHIFT,
    FVIRTKEY, HACCEL, HMENU, MF_POPUP, MF_SEPARATOR, MF_STRING,
};

use super::crash_view;
use super::fconsole;
use super::fedit;
use super::log_view;
use super::repl_pane;
use super::stack_view;

/// Frame-level WM_COMMAND id for File → Exit.  Frame-handled.
pub const FILE_CMD_EXIT: u16 = 0x3050;

/// Frame-level WM_COMMAND id for the Forth-Restart menu item.
/// Living here so all menu IDs sit together.
pub const FORTH_RESTART_CMD_ID: u16 = 0x3200;

/// Frame-level WM_COMMAND id for Forth → Break (interrupt the
/// running eval at the next safepoint).  Unlike Restart this
/// doesn't tear down the session — just aborts the in-flight
/// eval via the VM's safepoint-interrupt mechanism.
pub const FORTH_INTERRUPT_CMD_ID: u16 = 0x3201;

/// Range reserved for auto-assigned Demos menu items.
/// Up to 4096 demos before we overflow — well past any reasonable
/// directory size.
pub const DEMO_CMD_BASE: u16 = 0x4000;
pub const DEMO_CMD_END:  u16 = 0x4FFF;

/// Help → Documentation: open the bundled docs/ in-window as a help-pane.
pub const HELP_CMD_DOCS: u16 = 0x5000;

// ─── Menu builders ────────────────────────────────────────────────────

/// Append items to a popup.  `id = 0` with label `"SEP"` inserts a
/// separator; everything else is a normal MF_STRING item.
fn append_items(popup: HMENU, ctx: &str, items: &[(u16, &str)]) {
    for &(id, label) in items {
        if label == "SEP" {
            let _ = unsafe { AppendMenuW(popup, MF_SEPARATOR, 0, PCWSTR::null()) };
            continue;
        }
        let mut w: Vec<u16> = label.encode_utf16().collect();
        w.push(0);
        if let Err(e) = unsafe {
            AppendMenuW(popup, MF_STRING, id as usize, PCWSTR(w.as_ptr()))
        } {
            eprintln!("[{ctx}] append {label:?}: {e}");
        }
    }
}

fn append_popup(bar: HMENU, ctx: &str, title: &str, popup: HMENU) {
    let mut t: Vec<u16> = title.encode_utf16().collect();
    t.push(0);
    if let Err(e) = unsafe {
        AppendMenuW(bar, MF_POPUP, popup.0 as usize, PCWSTR(t.as_ptr()))
    } {
        eprintln!("[{ctx}] append popup: {e}");
    }
}

/// File menu — New, Open, Save, Save As, Exit.
pub fn append_file_menu(bar: HMENU) {
    let Ok(popup) = (unsafe { CreatePopupMenu() }) else {
        eprintln!("[file-menu] CreatePopupMenu failed");
        return;
    };
    append_items(popup, "file-menu", &[
        (fedit::MENU_CMD_ID,       "&New\tCtrl+N"),
        (fedit::EDIT_CMD_OPEN,     "&Open…\tCtrl+O"),
        (0,                        "SEP"),
        (fedit::EDIT_CMD_SAVE,     "&Save\tCtrl+S"),
        (fedit::EDIT_CMD_SAVE_AS,  "Save &As…\tCtrl+Shift+S"),
        (0,                        "SEP"),
        (FILE_CMD_EXIT,            "E&xit\tAlt+F4"),
    ]);
    append_popup(bar, "file-menu", "&File", popup);
}

/// Edit menu — Undo, Redo, Cut, Copy, Paste, Select All, word nav.
pub fn append_edit_menu(bar: HMENU) {
    let Ok(popup) = (unsafe { CreatePopupMenu() }) else {
        eprintln!("[edit-menu] CreatePopupMenu failed");
        return;
    };
    append_items(popup, "edit-menu", &[
        (fedit::EDIT_CMD_UNDO,       "&Undo\tCtrl+Z"),
        (fedit::EDIT_CMD_REDO,       "&Redo\tCtrl+Y"),
        (0,                          "SEP"),
        (fedit::EDIT_CMD_CUT,        "Cu&t\tCtrl+X"),
        (fedit::EDIT_CMD_COPY,       "&Copy\tCtrl+C"),
        (fedit::EDIT_CMD_PASTE,      "&Paste\tCtrl+V"),
        (fedit::EDIT_CMD_SELECT_ALL, "Select &All\tCtrl+A"),
        (0,                          "SEP"),
        (fedit::EDIT_CMD_NEXT_WORD,  "Next &Word\tCtrl+\u{2192}"),
        (fedit::EDIT_CMD_PREV_WORD,  "Pre&v Word\tCtrl+\u{2190}"),
    ]);
    append_popup(bar, "edit-menu", "&Edit", popup);
}

/// View menu — the built-in panes.  Each entry focuses an
/// existing singleton or creates a new one.
pub fn append_view_menu(bar: HMENU) {
    let Ok(popup) = (unsafe { CreatePopupMenu() }) else {
        eprintln!("[view-menu] CreatePopupMenu failed");
        return;
    };
    append_items(popup, "view-menu", &[
        (fconsole::MENU_CMD_ID,    "&Console\tCtrl+Shift+R"),
        (repl_pane::MENU_CMD_ID,   "&REPL\tCtrl+Shift+P"),
        (stack_view::MENU_CMD_ID,  "&Stack\tCtrl+Shift+K"),
        (log_view::MENU_CMD_ID,    "&Log\tCtrl+Shift+L"),
        (crash_view::MENU_CMD_ID,  "Crash &Dump\tCtrl+Shift+X"),
    ]);
    append_popup(bar, "view-menu", "&View", popup);
}

/// Demos menu — one entry per discovered `.f` file in `demos/`.
/// `demos` is a slice of `(menu_id, display_name)` pairs.  Silently
/// skipped when empty (no menu shown), so the bar stays clean when
/// the user installed wf64 without the demos directory.
pub fn append_demos_menu(bar: HMENU, demos: &[(u16, String)]) {
    if demos.is_empty() {
        return;
    }
    let Ok(popup) = (unsafe { CreatePopupMenu() }) else {
        eprintln!("[demos-menu] CreatePopupMenu failed");
        return;
    };
    for (id, name) in demos {
        let mut w: Vec<u16> = name.encode_utf16().collect();
        w.push(0);
        if let Err(e) = unsafe {
            AppendMenuW(popup, MF_STRING, *id as usize, PCWSTR(w.as_ptr()))
        } {
            eprintln!("[demos-menu] append {name:?}: {e}");
        }
    }
    append_popup(bar, "demos-menu", "&Demos", popup);
}

/// Forth menu — language-thread lifecycle and buffer evaluation.
pub fn append_forth_menu(bar: HMENU) {
    let Ok(popup) = (unsafe { CreatePopupMenu() }) else {
        eprintln!("[forth-menu] CreatePopupMenu failed");
        return;
    };
    append_items(popup, "forth-menu", &[
        (FORTH_INTERRUPT_CMD_ID,      "&Break\tCtrl+B"),
        (FORTH_RESTART_CMD_ID,        "&Restart\tCtrl+Shift+F5"),
        (0,                           "SEP"),
        (fedit::EDIT_CMD_RUN_BUFFER,  "R&un Buffer\tF5"),
    ]);
    append_popup(bar, "forth-menu", "Fo&rth", popup);
}

/// Help menu — Documentation (opens the bundled docs/ in-window as a help-pane).
pub fn append_help_menu(bar: HMENU) {
    let Ok(popup) = (unsafe { CreatePopupMenu() }) else {
        eprintln!("[help-menu] CreatePopupMenu failed");
        return;
    };
    append_items(popup, "help-menu", &[
        (HELP_CMD_DOCS, "&Documentation\tF1"),
    ]);
    append_popup(bar, "help-menu", "&Help", popup);
}

/// Build the default frame menu bar: File, Edit, View, Forth,
/// [Demos], Help.  `demos` carries `(id, display_name)` pairs from
/// the frame's demo-discovery pass; pass an empty slice for no Demos
/// menu.
pub fn build_default_menu_bar(demos: &[(u16, String)]) -> Option<HMENU> {
    let bar = unsafe { CreateMenu() }.ok()?;
    append_file_menu(bar);
    append_edit_menu(bar);
    append_view_menu(bar);
    append_forth_menu(bar);
    append_demos_menu(bar, demos);
    append_help_menu(bar);
    Some(bar)
}

/// Frame-level accelerator table.  Mirrors the visible menu
/// shortcuts so power-users get the same keystrokes regardless of
/// whether the menu is open.
pub fn build_accelerator_table() -> Option<HACCEL> {
    use windows::Win32::UI::Input::KeyboardAndMouse::VK_F5;
    let entries = [
        // File
        ACCEL { fVirt: FCONTROL | FVIRTKEY,          key: b'N' as u16, cmd: fedit::MENU_CMD_ID },
        ACCEL { fVirt: FCONTROL | FVIRTKEY,          key: b'O' as u16, cmd: fedit::EDIT_CMD_OPEN },
        ACCEL { fVirt: FCONTROL | FVIRTKEY,          key: b'S' as u16, cmd: fedit::EDIT_CMD_SAVE },
        ACCEL { fVirt: FCONTROL | FSHIFT | FVIRTKEY, key: b'S' as u16, cmd: fedit::EDIT_CMD_SAVE_AS },
        // View
        ACCEL { fVirt: FCONTROL | FSHIFT | FVIRTKEY, key: b'R' as u16, cmd: fconsole::MENU_CMD_ID },
        ACCEL { fVirt: FCONTROL | FSHIFT | FVIRTKEY, key: b'P' as u16, cmd: repl_pane::MENU_CMD_ID },
        ACCEL { fVirt: FCONTROL | FSHIFT | FVIRTKEY, key: b'K' as u16, cmd: stack_view::MENU_CMD_ID },
        ACCEL { fVirt: FCONTROL | FSHIFT | FVIRTKEY, key: b'L' as u16, cmd: log_view::MENU_CMD_ID },
        ACCEL { fVirt: FCONTROL | FSHIFT | FVIRTKEY, key: b'X' as u16, cmd: crash_view::MENU_CMD_ID },
        // Forth
        ACCEL { fVirt: FCONTROL | FVIRTKEY,          key: b'B' as u16, cmd: FORTH_INTERRUPT_CMD_ID },
        ACCEL { fVirt: FCONTROL | FSHIFT | FVIRTKEY, key: VK_F5.0,     cmd: FORTH_RESTART_CMD_ID },
        ACCEL { fVirt: FVIRTKEY,                     key: VK_F5.0,     cmd: fedit::EDIT_CMD_RUN_BUFFER },
        // Help
        ACCEL { fVirt: FVIRTKEY,                     key: 0x70_u16,    cmd: HELP_CMD_DOCS },
    ];
    unsafe { CreateAcceleratorTableW(&entries) }
        .ok()
        .filter(|h| !h.is_invalid())
}
