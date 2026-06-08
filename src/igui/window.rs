//! MDI frame window, MDI client child, message pump, and the
//! cross-thread helpers used by `iGui.OpenChild` / `CloseChild` /
//! `SetTitle`.
//!
//! Window-creation operations issued by the language thread are
//! marshalled to the GUI thread via private `WM_USER` messages and
//! `SendMessageW`, which blocks until the WndProc returns. This
//! preserves the iGui rule that all HWND ownership lives on the GUI
//! thread without forcing a typed RPC between the two.

#![cfg(windows)]

use std::ptr;
use std::sync::OnceLock;
use std::sync::Mutex;

use windows::core::{w, PCWSTR};
use windows::Win32::Foundation::{COLORREF, HINSTANCE, HWND, LPARAM, LRESULT, RECT, WPARAM};
use windows::Win32::UI::WindowsAndMessaging::HICON;
use windows::Win32::Graphics::Gdi::{
    CreateCompatibleBitmap, CreateCompatibleDC, CreateFontW, CreatePatternBrush,
    CreateSolidBrush, DeleteDC, DeleteObject, FillRect as GdiFillRect, GetDC, ReleaseDC,
    SelectObject, SetBkMode, SetTextColor, TextOutW,
    BACKGROUND_MODE, FONT_CHARSET, FONT_CLIP_PRECISION, FONT_OUTPUT_PRECISION,
    FONT_QUALITY, HBITMAP, HBRUSH, HDC, HFONT, HGDIOBJ,
    TRANSPARENT,
};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::Threading::GetCurrentThreadId;
// (SetWindowSubclass not used — we subclass via SetWindowLongPtrW instead)
use windows::Win32::UI::HiDpi::{
    SetProcessDpiAwarenessContext, DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2,
};
use windows::Win32::UI::Input::KeyboardAndMouse::{
    GetKeyState, VK_CAPITAL, VK_CONTROL, VK_LWIN, VK_MENU, VK_RWIN, VK_SHIFT,
};
use windows::Win32::UI::WindowsAndMessaging::{
    CallWindowProcW, CreateWindowExW, DefFrameProcW, DispatchMessageW, GetClientRect,
    GetMessageTime, GetMessageW, GetWindowLongPtrW, LoadCursorW, PostMessageW, PostQuitMessage,
    RegisterClassExW, SendMessageW, SetWindowLongPtrW, ShowWindow,
    TranslateAcceleratorW, TranslateMessage, CLIENTCREATESTRUCT, CW_USEDEFAULT, GWLP_WNDPROC,
    HACCEL, IDC_ARROW, MDICREATESTRUCTW, MSG, SW_SHOW, WHEEL_DELTA, WM_CHAR, WM_CLOSE,
    WM_COMMAND, WM_DESTROY, WM_ERASEBKGND, WM_KEYDOWN, WM_KEYUP, WM_KILLFOCUS, WM_LBUTTONDOWN,
    WM_LBUTTONUP, WM_MBUTTONDOWN, WM_MBUTTONUP, WM_MDICREATE, WM_MOUSEMOVE, WM_MOUSEWHEEL,
    WM_RBUTTONDOWN, WM_RBUTTONUP, WM_SETFOCUS, WM_SIZE, WM_SYSCOLORCHANGE, WM_SYSKEYDOWN,
    WM_SYSKEYUP, WM_THEMECHANGED, WM_USER, WNDCLASSEXW, WNDCLASS_STYLES, WS_CHILD,
    WS_CLIPCHILDREN, WS_EX_APPWINDOW, WS_HSCROLL, WS_OVERLAPPEDWINDOW, WS_VISIBLE, WS_VSCROLL,
};

use super::channels::{self, modifier, mouse_op, IGuiEvent};
use super::child::{self, MdiBootstrap, MDI_CHILD_CLASS};
use super::cp_exports::FRAME_HWND;
use super::registry;
use super::renderer;
use super::IGuiError;

const FRAME_CHILD_ID: i64 = 1;
const FRAME_CLASS: PCWSTR = w!("WF64.iGui.Frame");

// Private messages used to marshal language-thread calls onto the GUI
// thread. lparam is the address of the corresponding *Request struct,
// which the WndProc reads, mutates, and returns 0; the SendMessageW
// caller reads its own request struct on return.
const WM_IGUI_OPEN_CHILD: u32 = WM_USER + 1;
const WM_IGUI_CLOSE_CHILD: u32 = WM_USER + 2;
const WM_IGUI_SET_TITLE: u32 = WM_USER + 3;
const WM_IGUI_SET_MENU: u32 = WM_USER + 4;
const WM_IGUI_MDI_VERB: u32 = WM_USER + 5;
/// Open a built-in text-view MDI child. Like WM_IGUI_OPEN_CHILD but
/// the child class is `text_view`'s, with its own WndProc + grid
/// state. Routed through the frame so the WM_MDICREATE call lands
/// on the GUI thread.
const WM_IGUI_OPEN_TEXT: u32 = WM_USER + 7;
/// Drain the pending text-command queue for a text-view child onto
/// its grid, then invalidate. wparam carries the child_id. Both
/// queue-drain and InvalidateRect run on the GUI thread inside the
/// frame's WndProc — the language thread sees nothing past `child_id`
/// as an opaque token. Posted (not sent) so a tight write loop
/// doesn't block on the GUI thread.
const WM_IGUI_TEXT_FLUSH: u32 = WM_USER + 8;
/// Drain the pending-line queue for the singleton fconsole pane
/// onto the applied scrollback, then invalidate.  wparam/lparam
/// unused (fconsole is a singleton; no child_id needed).  Same
/// rationale as WM_IGUI_TEXT_FLUSH: keep all state mutation on
/// the GUI thread so the worker side never holds a lock long.
pub(crate) const WM_IGUI_FCONSOLE_FLUSH: u32 = WM_USER + 9;
/// Worker thread caught a Rust panic (or SEH dump arrived from
/// a future VEH path); flush the captured dumps into the crash
/// view, opening it if it isn't already.  wparam/lparam unused.
pub(crate) const WM_IGUI_CRASH_FLUSH: u32 = WM_USER + 10;
/// Open the built-in help-pane MDI child — the DocCrate-style folder
/// browser.  Same marshaling rationale as WM_IGUI_OPEN_TEXT.
const WM_IGUI_OPEN_HELP: u32 = WM_USER + 11;
/// Open a generic, Forth-writable Markdown doc-pane MDI child.  Same
/// marshaling rationale as WM_IGUI_OPEN_TEXT.
const WM_IGUI_OPEN_DOC: u32 = WM_USER + 12;
/// Repaint a doc-pane after its Markdown source changed.  Posted (not
/// sent) — same rationale as WM_IGUI_TEXT_FLUSH.
const WM_IGUI_DOC_FLUSH: u32 = WM_USER + 13;
/// Sent from the language thread to a render-host HWND to install
/// or clear a Win32 timer driving `EvTick` events.
/// `wparam` carries the interval in ms (0 = clear), `lparam` is unused.
pub(crate) const WM_IGUI_SET_TIMER: u32 = WM_USER + 6;
/// Win32 timer id used by the redraw-rate ticker. One timer per
/// render host; reusing the same id replaces the previous one.
pub(crate) const TICK_TIMER_ID: usize = 0xA1;

/// HWND of the MDI client. Set by `run` after `CreateWindowExW`.
static MDI_CLIENT: Mutex<Option<isize>> = Mutex::new(None);
static GUI_THREAD_ID: OnceLock<u32> = OnceLock::new();

/// Original WNDPROC of the MDICLIENT, saved before we replace it so
/// our subclass can forward unhandled messages correctly.
static MDICLIENT_ORIG_PROC: OnceLock<isize> = OnceLock::new();
/// ∴ logo brush handle (raw isize) kept alive for the process lifetime.
/// Was LAMBDA_BRUSH_RAW (lisp wallpaper) before the Forth port.
static LOGO_BRUSH_RAW: OnceLock<isize> = OnceLock::new();

/// Background + glyph colours for the frame's tiled wallpaper.
/// Set by the parent binary before `igui::run` (which calls
/// `make_logo_brush` at MDICLIENT-create time).  Colours are
/// packed 0xRRGGBB.
#[derive(Clone, Copy, Debug)]
pub struct FramePalette {
    pub bg: u32,
    pub fg: u32,
}

pub fn default_frame_palette() -> FramePalette {
    FramePalette {
        bg: 0x1C2834,  // deep navy-slate  (WF64's classic)
        fg: 0x3A5068,  // dot glyph: ~+30 per channel
    }
}

static FRAME_PALETTE: Mutex<FramePalette> = Mutex::new(FramePalette {
    bg: 0x1C2834,
    fg: 0x3A5068,
});

/// Override the frame wallpaper palette.  Must be called before
/// `igui::run` so make_logo_brush picks it up.  Idempotent;
/// most recent call wins.  WF64's own binary doesn't call this
/// — its default ships; FactorForth calls it to warm the tint.
pub fn set_frame_palette(p: FramePalette) {
    if let Ok(mut g) = FRAME_PALETTE.lock() {
        *g = p;
    }
}

/// Cached app icon.  First call resolves; subsequent calls reuse.
static APP_ICON_RAW: OnceLock<isize> = OnceLock::new();

/// Return the HICON every WF64 window class should advertise.
/// Tries to load an embedded RT_ICON at id 1 first (parent
/// binaries that ship their own `.ico` via embed-resource get
/// their branding automatically); falls back to the
/// procedurally-drawn ∴ on a navy disk if no resource is present.
///
/// Safe to call from any thread; uses LR_SHARED so the handle
/// is process-owned and doesn't need cleanup.
///
/// # Safety
/// Must be called after a Win32 module handle is available
/// (i.e. inside or after `igui::run`).  Calling from a static
/// initializer would crash.
pub unsafe fn app_icon() -> windows::Win32::UI::WindowsAndMessaging::HICON {
    use windows::Win32::UI::WindowsAndMessaging::HICON;
    let raw = *APP_ICON_RAW.get_or_init(|| {
        use windows::Win32::UI::WindowsAndMessaging::{
            LoadImageW, IMAGE_ICON, LR_DEFAULTSIZE, LR_SHARED,
        };
        let h_instance: HINSTANCE = unsafe {
            GetModuleHandleW(None)
        }.unwrap_or_default().into();
        // Resource id 1 matches the convention in
        // tools/<host>-ui.rc:  `1 ICON "host.ico"`.
        let loaded = unsafe {
            LoadImageW(
                Some(h_instance), PCWSTR(1 as *const u16),
                IMAGE_ICON, 0, 0, LR_DEFAULTSIZE | LR_SHARED,
            )
        };
        match loaded {
            Ok(handle) if !handle.is_invalid() => handle.0 as isize,
            _ => unsafe { make_app_icon() }.0 as isize,
        }
    });
    HICON(raw as *mut _)
}

/// Discovered demo files: (menu_id, display_name, absolute_path).
/// Populated by `discover_demos` at frame creation; the WM_COMMAND
/// handler looks up entries here when the user clicks a Demos item.
static DEMO_FILES: OnceLock<Vec<(u16, String, std::path::PathBuf)>> = OnceLock::new();

/// Snapshot of the demos table for menu rebuilds.  Returns an
/// empty Vec if discovery hasn't run yet (e.g. before
/// `run()` populated DEMO_FILES).
pub(crate) fn demo_files_snapshot() -> Vec<(u16, String, std::path::PathBuf)> {
    DEMO_FILES.get().cloned().unwrap_or_default()
}

// ── Help / documentation launcher ─────────────────────────────────────────

/// Open the bundled user guide **in-window** as a help-pane (the
/// DocCrate-style folder browser), rendered by the embedded `docpane`
/// core.  No external viewer is launched.
///
/// `docs/` search order:
///   1. `<exe_dir>/docs/`         — production installation
///   2. `<exe_dir>/../../docs/`   — dev build (exe is under target/debug/)
///   3. `CARGO_MANIFEST_DIR/docs/` — `cargo run` from anywhere
pub(crate) fn open_docs() {
    // Open the manual *in-window* as a help-pane (the DocCrate-style
    // folder browser), rendered through the shared `docpane` core.  We're
    // already on the GUI thread here (called from the frame's WM_COMMAND),
    // so we create the child directly rather than marshaling through
    // `open_help_child`.
    let exe_dir = std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(|d| d.to_path_buf()));

    // ── locate docs/ directory ────────────────────────────────────
    let docs_dir: Option<std::path::PathBuf> = exe_dir
        .as_ref()
        .map(|d| d.join("docs"))
        .filter(|p| p.is_dir())
        .or_else(|| {
            // dev: exe is in target/debug/ or target/release/
            exe_dir.as_ref()
                .and_then(|d| d.ancestors().nth(2))
                .map(|root| root.join("docs"))
                .filter(|p| p.is_dir())
        })
        .or_else(|| {
            // cargo run from any directory
            let manifest = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
            let p = manifest.join("docs");
            p.is_dir().then_some(p)
        });

    let Some(docs_dir) = docs_dir else {
        eprintln!(
            "[docs] docs/ directory not found next to this executable.\n\
             Expected a docs/ folder alongside wf64-ui.exe."
        );
        return;
    };

    let Some(mdi) = mdi_client_hwnd() else {
        eprintln!("[docs] MDI client not available");
        return;
    };
    let title: Vec<u16> = "Manual\u{0}".encode_utf16().collect();
    super::help_pane::create_on_gui_thread(mdi, &title, &docs_dir.to_string_lossy());
}

/// Scan for `demos/*.f` files and return `(menu_id, display_name, path)`
/// triples sorted by filename.
///
/// Search order (first non-empty hit wins):
///   1. `<exe>/demos/`            — what we'd ship in an installer
///   2. `<exe>/../../demos/`      — `target/release/demos/` → repo root
///   3. `CARGO_MANIFEST_DIR/demos/` — fallback for `cargo run` from anywhere
///
/// Display name = filename stem with `-`/`_` turned into spaces and each
/// word title-cased.  `stack-tour.f` → `"Stack Tour"`.
fn discover_demos() -> Vec<(u16, String, std::path::PathBuf)> {
    use super::tools_menu::{DEMO_CMD_BASE, DEMO_CMD_END};

    let mut candidates: Vec<std::path::PathBuf> = Vec::new();
    if let Ok(exe) = std::env::current_exe() {
        if let Some(exe_dir) = exe.parent() {
            candidates.push(exe_dir.join("demos"));
            if let Some(repo) = exe.ancestors().nth(3) {
                candidates.push(repo.join("demos"));
            }
        }
    }
    candidates.push(std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("demos"));

    for dir in &candidates {
        if !dir.is_dir() {
            continue;
        }
        let Ok(entries) = std::fs::read_dir(dir) else { continue };
        let mut files: Vec<std::path::PathBuf> = entries
            .flatten()
            .map(|e| e.path())
            .filter(|p| p.extension().and_then(|e| e.to_str()) == Some("f"))
            .collect();
        files.sort();
        if files.is_empty() {
            continue;
        }
        return files
            .into_iter()
            .enumerate()
            .take((DEMO_CMD_END - DEMO_CMD_BASE + 1) as usize)
            .map(|(i, path)| {
                let stem = path
                    .file_stem()
                    .and_then(|s| s.to_str())
                    .unwrap_or("unknown");
                let pretty = stem
                    .split(|c: char| c == '-' || c == '_')
                    .map(|w| {
                        let mut ch = w.chars();
                        match ch.next() {
                            Some(f) => f.to_uppercase().collect::<String>() + ch.as_str(),
                            None => String::new(),
                        }
                    })
                    .collect::<Vec<_>>()
                    .join(" ");
                (DEMO_CMD_BASE + i as u16, pretty, path)
            })
            .collect();
    }
    Vec::new()
}

// ── ∴ logo wallpaper brush ─────────────────────────────────────────────────

/// Color helpers: COLORREF = R | (G<<8) | (B<<16).
const fn rgb(r: u8, g: u8, b: u8) -> COLORREF {
    COLORREF((r as u32) | ((g as u32) << 8) | ((b as u32) << 16))
}

/// Build an 80×80 GDI pattern brush: deep navy-slate background with a
/// barely-lighter ∴ (U+2234) tiled at two diagonal offsets per cell.
/// The half-brick offset creates a continuous diagonal lattice.
///
/// ∴ ("therefore") replaces the λ that the original NewCormanLisp UI
/// used — same role, Forth-flavoured glyph.  Three dots arranged like
/// a stack diagram, mathematical inference glyph; the right symbol for
/// a postfix-RPN language.
///
/// Called once on the GUI thread immediately after MDICLIENT is created.
/// The returned HBRUSH lives for the process lifetime.
unsafe fn make_logo_brush() -> HBRUSH {
    const TILE: i32 = 80;

    // Background + glyph palette.  Defaults are WF64's deep
    // navy-slate; parent binaries can override before the frame
    // is created via `set_frame_palette` (FactorForth uses this
    // to shift to a warm-charcoal tint that pairs with its
    // amber brand colour).  See `FRAME_PALETTE` below.
    let palette = FRAME_PALETTE.lock().map(|g| *g)
        .unwrap_or(default_frame_palette());
    let bg_rgb = palette.bg;
    let fg_rgb = palette.fg;
    let BG: COLORREF = rgb(
        (bg_rgb >> 16) as u8, (bg_rgb >> 8) as u8, bg_rgb as u8);
    let FG: COLORREF = rgb(
        (fg_rgb >> 16) as u8, (fg_rgb >> 8) as u8, fg_rgb as u8);

    unsafe {
        let screen_dc = GetDC(None);
        let mem_dc = CreateCompatibleDC(Some(screen_dc));
        let bmp: HBITMAP = CreateCompatibleBitmap(screen_dc, TILE, TILE);
        let old_bmp: HGDIOBJ = SelectObject(mem_dc, HGDIOBJ(bmp.0));

        // Fill solid background.
        let bg_brush: HBRUSH = CreateSolidBrush(BG);
        let tile_rect = RECT { left: 0, top: 0, right: TILE, bottom: TILE };
        GdiFillRect(mem_dc, &tile_rect, bg_brush);
        DeleteObject(HGDIOBJ(bg_brush.0));

        // ∴ has less ink than λ at the same point size — the three
        // dots occupy a tiny fraction of the glyph cell.  Pump the
        // height up so the pattern reads as a wallpaper, not as
        // accidental specks.  Bold weight also helps the dots show.
        // Italic doesn't change anything visually on dot glyphs;
        // dropped.
        SetBkMode(mem_dc, BACKGROUND_MODE(TRANSPARENT.0));
        SetTextColor(mem_dc, FG);

        let font: HFONT = CreateFontW(
            44, 0,                        // height ↑ from 28 — dots need air
            0, 0,
            700,                          // FW_BOLD (was FW_THIN)
            0, 0, 0,                      // italic off
            FONT_CHARSET(1),              // DEFAULT_CHARSET
            FONT_OUTPUT_PRECISION(0),
            FONT_CLIP_PRECISION(0),
            FONT_QUALITY(5),              // CLEARTYPE_QUALITY
            32u32,                        // FF_SWISS
            w!("Segoe UI"),
        );
        let old_font: HGDIOBJ = SelectObject(mem_dc, HGDIOBJ(font.0));

        // U+2234 ∴ — one UTF-16 codepoint (BMP).
        let glyph: &[u16] = &[0x2234_u16];
        // Stamps centred-ish in each half of the tile.  Tweaked to
        // give a pleasing diagonal repeat for the wider/taller font.
        let _ = TextOutW(mem_dc,  6,  4, glyph); // top-left
        let _ = TextOutW(mem_dc, 46, 36, glyph); // bottom-right (half-brick)

        SelectObject(mem_dc, old_font);
        DeleteObject(HGDIOBJ(font.0));
        SelectObject(mem_dc, old_bmp);

        let brush: HBRUSH = CreatePatternBrush(bmp);

        DeleteObject(HGDIOBJ(bmp.0));
        let _ = DeleteDC(mem_dc);
        let _ = ReleaseDC(None, screen_dc);

        brush
    }
}

/// Build a 32×32 HICON with the ∴ logo on a deep navy disk.  Used
/// as the frame's window icon (title-bar, Alt+Tab, taskbar).  No
/// .ico asset; generated procedurally from GDI primitives so the
/// build stays self-contained.
unsafe fn make_app_icon() -> HICON {
    use windows::Win32::Graphics::Gdi::{
        BI_RGB, BITMAPINFO, BITMAPINFOHEADER, DIB_RGB_COLORS,
        CreateDIBSection,
    };
    use windows::Win32::UI::WindowsAndMessaging::{CreateIconIndirect, HICON, ICONINFO};

    const SIZE: i32 = 32;
    const BG: COLORREF = rgb(20, 28, 40);    // even darker than wallpaper
    const FG: COLORREF = rgb(255, 184, 96);  // warm amber, readable at 16×16

    unsafe {
        let screen_dc = GetDC(None);
        let mem_dc = CreateCompatibleDC(Some(screen_dc));

        // 32-bit DIB so the icon supports the alpha channel for
        // smooth edges and modern shell rendering.
        let mut bmi = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: SIZE,
                biHeight: SIZE,  // positive = bottom-up
                biPlanes: 1,
                biBitCount: 32,
                biCompression: BI_RGB.0,
                ..Default::default()
            },
            ..Default::default()
        };
        let mut bits_ptr: *mut std::ffi::c_void = std::ptr::null_mut();
        let color_bmp: HBITMAP = CreateDIBSection(
            Some(mem_dc),
            &bmi,
            DIB_RGB_COLORS,
            &mut bits_ptr,
            None,
            0,
        ).unwrap_or_default();
        if color_bmp.is_invalid() {
            let _ = DeleteDC(mem_dc);
            let _ = ReleaseDC(None, screen_dc);
            return HICON::default();
        }
        let old_color = SelectObject(mem_dc, HGDIOBJ(color_bmp.0));

        // Pre-fill all pixels with opaque BG (alpha=0xFF + BG rgb).
        // bits_ptr is BGRA, bottom-up.
        let _ = bmi;
        let pixels = bits_ptr as *mut u32;
        let bg_argb = 0xFF000000
            | (BG.0 & 0xFF) << 16          // R → high byte (BGRA → ARGB)
            | (BG.0 & 0xFF00)              // G stays
            | (BG.0 & 0xFF0000) >> 16;     // B
        let total_pixels = (SIZE * SIZE) as isize;
        for i in 0..total_pixels {
            *pixels.offset(i) = bg_argb;
        }

        // Now blit the ∴ glyph on top.  GDI text writes 24-bit RGB
        // and clobbers our pre-set alpha to 0 — but the icon mask
        // (below) will treat alpha=0 as transparent, so we re-set
        // alpha to 0xFF on every text pixel afterwards (cheaper to
        // re-fill all 32×32 pixels' alpha at the end).
        SetBkMode(mem_dc, BACKGROUND_MODE(TRANSPARENT.0));
        SetTextColor(mem_dc, FG);
        let font: HFONT = CreateFontW(
            28, 0,
            0, 0,
            900,                          // FW_BLACK — heaviest available
            0, 0, 0,
            FONT_CHARSET(1),
            FONT_OUTPUT_PRECISION(0),
            FONT_CLIP_PRECISION(0),
            FONT_QUALITY(5),
            32u32,
            w!("Segoe UI"),
        );
        let old_font = SelectObject(mem_dc, HGDIOBJ(font.0));
        let glyph: &[u16] = &[0x2234_u16];
        let _ = TextOutW(mem_dc, 4, 0, glyph);
        SelectObject(mem_dc, old_font);
        DeleteObject(HGDIOBJ(font.0));

        // Re-set alpha = 0xFF on every pixel so the icon is fully
        // opaque (we don't want a Windows-default transparency
        // path; the disk is solid and the glyph reads on it).
        for i in 0..total_pixels {
            let p = pixels.offset(i);
            *p |= 0xFF000000;
        }

        SelectObject(mem_dc, old_color);

        // Build the AND mask: all-zero = fully opaque per pixel.
        // 32×32 / 8 = 128 bytes.  Required by ICONINFO even for
        // 32-bit colour icons.
        let mask_bmp: HBITMAP = CreateCompatibleBitmap(screen_dc, SIZE, SIZE);
        // (Default-initialised CompatibleBitmap content is undefined;
        // explicitly zero it via FillRect with a black brush so AND
        // mask is all-zero = opaque.)
        let old_mask = SelectObject(mem_dc, HGDIOBJ(mask_bmp.0));
        let black: HBRUSH = CreateSolidBrush(rgb(0, 0, 0));
        let r = RECT { left: 0, top: 0, right: SIZE, bottom: SIZE };
        GdiFillRect(mem_dc, &r, black);
        DeleteObject(HGDIOBJ(black.0));
        SelectObject(mem_dc, old_mask);

        let icon_info = ICONINFO {
            fIcon: true.into(),
            xHotspot: 0,
            yHotspot: 0,
            hbmMask: mask_bmp,
            hbmColor: color_bmp,
        };
        let icon = CreateIconIndirect(&icon_info).unwrap_or_default();

        DeleteObject(HGDIOBJ(mask_bmp.0));
        DeleteObject(HGDIOBJ(color_bmp.0));
        let _ = DeleteDC(mem_dc);
        let _ = ReleaseDC(None, screen_dc);

        icon
    }
}

/// Replacement WNDPROC for the MDICLIENT window.  Intercepts WM_ERASEBKGND
/// to paint the λ-tiled background; all other messages are forwarded to
/// the original MDICLIENT WndProc saved in MDICLIENT_ORIG_PROC.
unsafe extern "system" fn mdi_bg_proc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    if msg == WM_ERASEBKGND {
        if let Some(&raw) = LOGO_BRUSH_RAW.get() {
            let hdc = HDC(wparam.0 as *mut _);
            let brush = HBRUSH(raw as *mut _);
            let mut rect = RECT::default();
            unsafe { let _ = GetClientRect(hwnd, &mut rect); }
            unsafe { GdiFillRect(hdc, &rect, brush); }
            return LRESULT(1); // background erased — suppress default erase
        }
    }
    // Forward everything else (and WM_ERASEBKGND if brush not ready) to
    // the original MDICLIENT WndProc.
    let orig_raw = MDICLIENT_ORIG_PROC.get().copied().unwrap_or(0);
    if orig_raw != 0 {
        // SAFETY: orig_raw was obtained from GetWindowLongPtrW(GWLP_WNDPROC)
        // immediately before installation and is a valid WNDPROC pointer.
        unsafe {
            let f: unsafe extern "system" fn(HWND, u32, WPARAM, LPARAM) -> LRESULT =
                std::mem::transmute(orig_raw);
            CallWindowProcW(Some(f), hwnd, msg, wparam, lparam)
        }
    } else {
        unsafe { windows::Win32::UI::WindowsAndMessaging::DefWindowProcW(hwnd, msg, wparam, lparam) }
    }
}

pub(crate) fn mdi_client_hwnd() -> Option<HWND> {
    let raw = MDI_CLIENT.lock().ok()?;
    raw.map(|r| HWND(r as *mut _))
}

pub(crate) fn gui_thread_id() -> Option<u32> {
    GUI_THREAD_ID.get().copied()
}

/// Public entry point. Opens the iGui frame, sets up the MDI client,
/// runs the Win32 message pump until `WM_QUIT`, and returns the quit
/// code. If `worker` is provided, it is spawned on a background
/// thread once the frame is up.
pub fn run<F>(worker: Option<F>) -> Result<i32, IGuiError>
where
    F: FnOnce() + Send + 'static,
{
    unsafe {
        let _ = SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
    }
    let _ = GUI_THREAD_ID.set(unsafe { GetCurrentThreadId() });

    let h_instance = unsafe { GetModuleHandleW(None) }
        .map_err(|e| IGuiError::Win32(format!("GetModuleHandleW failed: {e}")))?
        .into();
    let cursor = unsafe { LoadCursorW(None, IDC_ARROW) }
        .map_err(|e| IGuiError::Win32(format!("LoadCursorW failed: {e}")))?;

    // Window icon — used for the frame's title bar, taskbar,
    // Alt+Tab.  Child windows also use it via app_icon() below.
    let app_icon = unsafe { app_icon() };

    // Frame class.
    let frame_class = WNDCLASSEXW {
        cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
        style: WNDCLASS_STYLES(0),
        lpfnWndProc: Some(frame_wnd_proc),
        cbClsExtra: 0,
        cbWndExtra: 0,
        hInstance: h_instance,
        hIcon: app_icon,
        hCursor: cursor,
        hbrBackground: windows::Win32::Graphics::Gdi::HBRUSH(ptr::null_mut()),
        lpszMenuName: PCWSTR::null(),
        lpszClassName: FRAME_CLASS,
        hIconSm: app_icon,  // 16×16 — Windows downsamples our 32×32
    };
    if unsafe { RegisterClassExW(&frame_class) } == 0 {
        return Err(IGuiError::Win32("RegisterClassExW (frame) returned 0".into()));
    }
    child::register_classes()?;

    // Renderer comes up before any window so child WM_NCCREATE can build
    // its swap chain immediately.
    renderer::install()?;

    let hwnd = unsafe {
        CreateWindowExW(
            WS_EX_APPWINDOW,
            FRAME_CLASS,
            w!("\u{2234} WF64 \u{2014} Forth IDE"),
            WS_OVERLAPPEDWINDOW | WS_CLIPCHILDREN | WS_VISIBLE,
            CW_USEDEFAULT,
            CW_USEDEFAULT,
            1024,
            720,
            None,
            None,
            Some(h_instance),
            None,
        )
    }
    .map_err(|e| IGuiError::Win32(format!("CreateWindowExW (frame) failed: {e}")))?;
    let _ = FRAME_HWND.set(hwnd.0 as isize);

    // MDI client occupies the whole frame body for now (no toolbar /
    // status bar yet).
    let mut frame_rect = RECT::default();
    unsafe { GetClientRect(hwnd, &mut frame_rect) }
        .map_err(|e| IGuiError::Win32(format!("GetClientRect (frame) failed: {e}")))?;
    let mut create = CLIENTCREATESTRUCT {
        hWindowMenu: Default::default(),
        idFirstChild: 0xCC00,
    };
    let mdi = unsafe {
        CreateWindowExW(
            windows::Win32::UI::WindowsAndMessaging::WINDOW_EX_STYLE(0),
            w!("MDICLIENT"),
            PCWSTR::null(),
            WS_CHILD | WS_VISIBLE | WS_CLIPCHILDREN | WS_HSCROLL | WS_VSCROLL,
            0,
            0,
            frame_rect.right - frame_rect.left,
            frame_rect.bottom - frame_rect.top,
            Some(hwnd),
            None,
            Some(h_instance),
            Some(&mut create as *mut _ as *mut _),
        )
    }
    .map_err(|e| IGuiError::Win32(format!("CreateWindowExW (MDICLIENT) failed: {e}")))?;
    {
        let mut slot = MDI_CLIENT.lock().expect("MDI_CLIENT mutex poisoned");
        *slot = Some(mdi.0 as isize);
    }

    // Install the ∴-tiled background.  The brush lives for the process
    // lifetime; no explicit cleanup needed since we exit shortly after
    // the frame is destroyed.
    let logo_brush = unsafe { make_logo_brush() };
    let _ = LOGO_BRUSH_RAW.set(logo_brush.0 as isize);
    unsafe {
        // Save the original MDICLIENT WndProc then replace it with ours.
        let orig = GetWindowLongPtrW(mdi, GWLP_WNDPROC);
        let _ = MDICLIENT_ORIG_PROC.set(orig);
        SetWindowLongPtrW(mdi, GWLP_WNDPROC, mdi_bg_proc as *const () as isize);
    }

    channels::install();
    super::system_colors::sample();

    // Discover demo files (`demos/*.f`) so the Demos menu shows
    // one entry per file.  Populates the DEMO_FILES static; the
    // WM_COMMAND handler later uses it to read+run the right
    // file when the user clicks an entry.
    let demo_files = discover_demos();
    let demo_name_ids: Vec<(u16, String)> = demo_files
        .iter()
        .map(|(id, name, _)| (*id, name.clone()))
        .collect();
    let _ = DEMO_FILES.set(demo_files);

    // Install a default Tools menu so the built-in editor and log
    // view are reachable even before any language-thread code runs.
    // `iGui.SetMenu` from CP will replace this, but
    // `menu::install_for_frame` always re-appends the tools so they
    // stay available.
    if let Some(default_menu) = super::tools_menu::build_default_menu_bar(&demo_name_ids) {
        let _ = unsafe {
            windows::Win32::UI::WindowsAndMessaging::SetMenu(hwnd, Some(default_menu))
        };
        let _ = unsafe { windows::Win32::UI::WindowsAndMessaging::DrawMenuBar(hwnd) };
    }

    let _ = unsafe { ShowWindow(hwnd, SW_SHOW) };

    if let Some(worker) = worker {
        std::thread::Builder::new()
            .name("igui-language".into())
            .spawn(worker)
            .map_err(|e| IGuiError::Win32(format!("spawn language thread: {e}")))?;
    }

    // Frame-level accelerator table for the built-in tools:
    // Ctrl+Shift+E opens fedit, Ctrl+Shift+L opens the log view,
    // both regardless of which child has focus.
    let accel: Option<HACCEL> = super::tools_menu::build_accelerator_table();

    let mut msg = MSG::default();
    let exit_code = unsafe {
        loop {
            let r = GetMessageW(&mut msg, None, 0, 0);
            if r.0 == 0 {
                break msg.wParam.0 as i32;
            }
            if r.0 == -1 {
                break 1;
            }
            // Frame accelerators run before MDI accel and TranslateMessage:
            // they own the highest-priority shortcuts (Ctrl+Shift+E to
            // open fedit) regardless of which child has focus.
            if let Some(h) = accel {
                if TranslateAcceleratorW(hwnd, h, &mut msg) != 0 {
                    continue;
                }
            }
            // MDI requires TranslateMDISysAccel before TranslateMessage
            // for system MDI shortcuts (Ctrl+F4, Ctrl+F6, etc.).
            if windows::Win32::UI::WindowsAndMessaging::TranslateMDISysAccel(mdi, &msg).as_bool() {
                continue;
            }
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    };

    Ok(exit_code)
}

unsafe extern "system" fn frame_wnd_proc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    let mdi = mdi_client_hwnd().unwrap_or_default();

    match msg {
        WM_IGUI_OPEN_CHILD => {
            let req_ptr = lparam.0 as *mut OpenChildRequest;
            if !req_ptr.is_null() {
                let req = unsafe { &mut *req_ptr };
                req.out = handle_open_child(req);
            }
            LRESULT(0)
        }
        WM_IGUI_OPEN_TEXT => {
            let req_ptr = lparam.0 as *mut OpenTextRequest;
            if !req_ptr.is_null() {
                let req = unsafe { &mut *req_ptr };
                if let Some(mdi_client) = mdi_client_hwnd() {
                    req.out = super::text_view::create_on_gui_thread(mdi_client, &req.title);
                }
            }
            LRESULT(0)
        }
        WM_IGUI_TEXT_FLUSH => {
            let child_id = wparam.0 as i64;
            super::text_view::flush_on_gui_thread(child_id);
            LRESULT(0)
        }
        WM_IGUI_OPEN_HELP => {
            let req_ptr = lparam.0 as *mut OpenHelpRequest;
            if !req_ptr.is_null() {
                let req = unsafe { &mut *req_ptr };
                if let Some(mdi_client) = mdi_client_hwnd() {
                    req.out =
                        super::help_pane::create_on_gui_thread(mdi_client, &req.title, &req.path);
                }
            }
            LRESULT(0)
        }
        WM_IGUI_OPEN_DOC => {
            let req_ptr = lparam.0 as *mut OpenDocRequest;
            if !req_ptr.is_null() {
                let req = unsafe { &mut *req_ptr };
                if let Some(mdi_client) = mdi_client_hwnd() {
                    req.out = super::doc_pane::create_on_gui_thread(mdi_client, &req.title);
                }
            }
            LRESULT(0)
        }
        WM_IGUI_DOC_FLUSH => {
            let child_id = wparam.0 as i64;
            super::doc_pane::flush_on_gui_thread(child_id);
            LRESULT(0)
        }
        WM_IGUI_FCONSOLE_FLUSH => {
            super::fconsole::flush_on_gui_thread();
            LRESULT(0)
        }
        WM_IGUI_CRASH_FLUSH => {
            super::crash_view::flush_on_gui_thread(hwnd);
            LRESULT(0)
        }
        WM_IGUI_CLOSE_CHILD => {
            let req_ptr = lparam.0 as *mut CloseChildRequest;
            if !req_ptr.is_null() {
                let req = unsafe { &mut *req_ptr };
                if let Some(mdi_child) = registry::mdi_hwnd_of(req.child_id) {
                    if mdi.0 as isize != 0 {
                        child::close_via_mdi(mdi, mdi_child);
                        req.ok = true;
                    }
                }
            }
            LRESULT(0)
        }
        WM_IGUI_SET_TITLE => {
            let req_ptr = lparam.0 as *mut SetTitleRequest;
            if !req_ptr.is_null() {
                let req = unsafe { &*req_ptr };
                if let Some(mdi_child) = registry::mdi_hwnd_of(req.child_id) {
                    child::set_title(mdi_child, &req.title);
                }
            }
            LRESULT(0)
        }
        WM_IGUI_SET_MENU => {
            let req_ptr = lparam.0 as *mut SetMenuRequest;
            if !req_ptr.is_null() {
                let req = unsafe { &mut *req_ptr };
                req.ok = super::menu::install_for_frame(hwnd, mdi, &req.spec);
            }
            LRESULT(0)
        }
        WM_IGUI_MDI_VERB => {
            // wparam high byte = verb tag (avoid having to allocate
            // a request struct).
            let tag = wparam.0 as u8;
            if let Some(verb) = mdi_verb_from_tag(tag) {
                if mdi.0 as isize != 0 {
                    if matches!(verb, super::menu::MdiVerb::CloseAll) {
                        for (_id, mdi_child) in registry::snapshot() {
                            child::close_via_mdi(mdi, mdi_child);
                        }
                    } else {
                        super::menu::dispatch_mdi(mdi, verb);
                    }
                }
            }
            LRESULT(0)
        }
        WM_COMMAND => {
            let cmd_id = (wparam.0 & 0xFFFF) as u16;
            // Built-in tools (fedit, log view) are wired before the
            // user menu so they work even if no language-thread spec
            // has been installed.
            if cmd_id == super::fedit::MENU_CMD_ID {
                if mdi.0 as isize != 0 {
                    super::fedit::open(hwnd, mdi);
                }
                return LRESULT(0);
            }
            if cmd_id == super::log_view::MENU_CMD_ID {
                if mdi.0 as isize != 0 {
                    super::log_view::open(hwnd, mdi);
                }
                return LRESULT(0);
            }
            if cmd_id == super::fconsole::MENU_CMD_ID {
                if mdi.0 as isize != 0 {
                    super::fconsole::open(hwnd, mdi);
                }
                return LRESULT(0);
            }
            if cmd_id == super::crash_view::MENU_CMD_ID {
                if mdi.0 as isize != 0 {
                    super::crash_view::open(hwnd, mdi);
                }
                return LRESULT(0);
            }
            if cmd_id == super::repl_pane::MENU_CMD_ID {
                if mdi.0 as isize != 0 {
                    super::repl_pane::open(mdi);
                }
                return LRESULT(0);
            }
            if cmd_id == super::stack_view::MENU_CMD_ID {
                if mdi.0 as isize != 0 {
                    super::stack_view::open(hwnd, mdi);
                }
                return LRESULT(0);
            }
            if cmd_id == super::tools_menu::FORTH_RESTART_CMD_ID {
                // Pipe into the same mailbox the worker drains; it
                // tears down the session and brings up a fresh one.
                super::channels::push(super::channels::IGuiEvent::ForthRestart);
                return LRESULT(0);
            }
            if cmd_id == super::tools_menu::FORTH_INTERRUPT_CMD_ID {
                // Doesn't drop the session — just nudges the VM to
                // raise ERROR_INTERRUPT at the next safepoint.  The
                // listener's recover catches it, prints "ANS error
                // -28: Interrupt", and resumes the prompt.
                //
                // Crucial detail: we can't go through the IGuiEvent
                // queue because the IDE worker thread is blocked
                // inside session.eval() waiting for the dispatcher
                // — that's the whole point of needing an interrupt
                // in the first place.  Instead the parent binary
                // registers an `interrupt_hook` at startup, and we
                // call it synchronously from this thread (it's
                // designed for cross-thread use; just signals a
                // flag the VM polls at its next safepoint).
                if let Ok(hook) = super::channels::INTERRUPT_HOOK.lock() {
                    if let Some(f) = *hook { f(); }
                }
                return LRESULT(0);
            }
            // ── Demos menu ──────────────────────────────────────
            // Look up the file by id, push the source as an
            // EvalBuffer event with an appended `<stem>` call so
            // the worker both defines the words AND invokes the
            // entry point.  Errors come back through the normal
            // eval-output path into the console pane.
            if cmd_id >= super::tools_menu::DEMO_CMD_BASE
                && cmd_id <= super::tools_menu::DEMO_CMD_END
            {
                if let Some(demos) = DEMO_FILES.get() {
                    if let Some((_, name, path)) =
                        demos.iter().find(|(id, _, _)| *id == cmd_id)
                    {
                        match std::fs::read_to_string(path) {
                            Ok(text) => {
                                let stem = path
                                    .file_stem()
                                    .and_then(|s| s.to_str())
                                    .unwrap_or("");
                                // Convention: the entry-point word
                                // is the file stem.  The demo file
                                // defines it; we append a call so
                                // it runs immediately.
                                let banner = format!(
                                    "\\ === running demo: {} ===\n",
                                    name
                                );
                                let source =
                                    format!("{banner}{text}\n{stem}\n");
                                channels::push(IGuiEvent::EvalBuffer { source });
                            }
                            Err(e) => {
                                eprintln!(
                                    "[demos] cannot read {}: {}",
                                    path.display(), e,
                                );
                            }
                        }
                    }
                }
                return LRESULT(0);
            }
            if cmd_id == super::tools_menu::FILE_CMD_EXIT {
                // File → Exit: close the frame.  WM_CLOSE on the
                // frame fires the FrameClose event chain so the
                // worker shuts down cleanly.
                let _ = unsafe {
                    windows::Win32::UI::WindowsAndMessaging::PostMessageW(
                        Some(hwnd),
                        windows::Win32::UI::WindowsAndMessaging::WM_CLOSE,
                        WPARAM(0),
                        LPARAM(0),
                    )
                };
                return LRESULT(0);
            }
            if cmd_id == super::tools_menu::HELP_CMD_DOCS {
                open_docs();
                return LRESULT(0);
            }
            // File → Open: always create a new fedit pane and load the
            // chosen file into it, regardless of which MDI child is
            // currently active.  Never forward to the active child —
            // WM_MDIGETACTIVE returns any active child (console, log,
            // etc.), so forwarding silently drops the command when a
            // non-fedit pane has focus.
            if cmd_id == super::fedit::EDIT_CMD_OPEN {
                if mdi.0 as isize != 0 {
                    super::fedit::open_with_dialog(hwnd, mdi);
                }
                return LRESULT(0);
            }
            // Edit-menu commands: forward to the active MDI child.
            // fedit's WndProc recognises these IDs in its own
            // WM_COMMAND handler and dispatches to the right method.
            // If no child is active or the active child doesn't
            // care about Edit commands, the message is harmless.
            if cmd_id >= super::fedit::EDIT_CMD_BASE
                && cmd_id <= super::fedit::EDIT_CMD_END
            {
                if mdi.0 as isize != 0 {
                    let active_raw = unsafe {
                        windows::Win32::UI::WindowsAndMessaging::SendMessageW(
                            mdi,
                            windows::Win32::UI::WindowsAndMessaging::WM_MDIGETACTIVE,
                            Some(WPARAM(0)),
                            Some(LPARAM(0)),
                        )
                    };
                    let active = HWND(active_raw.0 as *mut _);
                    if active.0 as isize != 0 {
                        unsafe {
                            windows::Win32::UI::WindowsAndMessaging::SendMessageW(
                                active,
                                WM_COMMAND,
                                Some(wparam),
                                Some(lparam),
                            )
                        };
                    }
                }
                return LRESULT(0);
            }
            // MDI verbs auto-allocated in install_for_frame.
            if let Some(verb) = super::menu::lookup_mdi_verb(cmd_id) {
                if mdi.0 as isize != 0 {
                    if matches!(verb, super::menu::MdiVerb::CloseAll) {
                        for (_id, mdi_child) in registry::snapshot() {
                            child::close_via_mdi(mdi, mdi_child);
                        }
                    } else {
                        super::menu::dispatch_mdi(mdi, verb);
                    }
                }
                return LRESULT(0);
            }
            // User menu items: push EvMenu so the language thread can
            // dispatch on item_id.
            channels::push(IGuiEvent::Menu {
                menu_id: 0,
                item_id: cmd_id as i64,
            });
            // Then hand the command to DefFrameProcW. This is essential for a
            // *maximized* MDI child: the system moves its minimize/restore/
            // close buttons (the small window icons) into the frame's menu
            // bar, and clicking them arrives here as a WM_COMMAND that only
            // DefFrameProcW knows how to perform. Returning LRESULT(0) (as
            // before) swallowed them, so the controls were dead. DefFrameProcW
            // ignores genuine user-menu ids (well below the MDI reserved
            // range), so this is safe for both.
            unsafe { DefFrameProcW(hwnd, Some(mdi), msg, wparam, lparam) }
        }
        WM_SIZE => {
            // MDI client sizes itself via DefFrameProcW.
            channels::push(IGuiEvent::Resize {
                child_id: FRAME_CHILD_ID,
                width: (lparam.0 & 0xFFFF) as i64,
                height: ((lparam.0 >> 16) & 0xFFFF) as i64,
            });
            unsafe { DefFrameProcW(hwnd, Some(mdi), msg, wparam, lparam) }
        }
        WM_KEYDOWN | WM_SYSKEYDOWN => {
            push_key(FRAME_CHILD_ID, true, wparam, lparam);
            unsafe { DefFrameProcW(hwnd, Some(mdi), msg, wparam, lparam) }
        }
        WM_KEYUP | WM_SYSKEYUP => {
            push_key(FRAME_CHILD_ID, false, wparam, lparam);
            unsafe { DefFrameProcW(hwnd, Some(mdi), msg, wparam, lparam) }
        }
        WM_CHAR => {
            channels::push(IGuiEvent::Char {
                child_id: FRAME_CHILD_ID,
                codepoint: wparam.0 as i64,
                mods: current_modifiers(),
                time_ms: msg_time(),
            });
            unsafe { DefFrameProcW(hwnd, Some(mdi), msg, wparam, lparam) }
        }
        WM_MOUSEMOVE => {
            push_mouse(FRAME_CHILD_ID, mouse_op::MOVE, 0, lparam);
            unsafe { DefFrameProcW(hwnd, Some(mdi), msg, wparam, lparam) }
        }
        WM_LBUTTONDOWN => {
            push_mouse(FRAME_CHILD_ID, mouse_op::LEFT_DOWN, 1, lparam);
            unsafe { DefFrameProcW(hwnd, Some(mdi), msg, wparam, lparam) }
        }
        WM_LBUTTONUP => {
            push_mouse(FRAME_CHILD_ID, mouse_op::LEFT_UP, 1, lparam);
            unsafe { DefFrameProcW(hwnd, Some(mdi), msg, wparam, lparam) }
        }
        WM_RBUTTONDOWN => {
            push_mouse(FRAME_CHILD_ID, mouse_op::RIGHT_DOWN, 2, lparam);
            unsafe { DefFrameProcW(hwnd, Some(mdi), msg, wparam, lparam) }
        }
        WM_RBUTTONUP => {
            push_mouse(FRAME_CHILD_ID, mouse_op::RIGHT_UP, 2, lparam);
            unsafe { DefFrameProcW(hwnd, Some(mdi), msg, wparam, lparam) }
        }
        WM_MBUTTONDOWN => {
            push_mouse(FRAME_CHILD_ID, mouse_op::MIDDLE_DOWN, 3, lparam);
            unsafe { DefFrameProcW(hwnd, Some(mdi), msg, wparam, lparam) }
        }
        WM_MBUTTONUP => {
            push_mouse(FRAME_CHILD_ID, mouse_op::MIDDLE_UP, 3, lparam);
            unsafe { DefFrameProcW(hwnd, Some(mdi), msg, wparam, lparam) }
        }
        WM_MOUSEWHEEL => {
            let raw = ((wparam.0 >> 16) & 0xFFFF) as i16;
            let delta = raw as i64;
            let lines = if WHEEL_DELTA != 0 {
                delta / (WHEEL_DELTA as i64)
            } else {
                0
            };
            channels::push(IGuiEvent::Mouse {
                child_id: FRAME_CHILD_ID,
                x: (lparam.0 & 0xFFFF) as i16 as i64,
                y: ((lparam.0 >> 16) & 0xFFFF) as i16 as i64,
                op: mouse_op::WHEEL,
                button: 0,
                mods: current_modifiers(),
                wheel_delta: delta,
                wheel_lines: lines,
                time_ms: msg_time(),
            });
            unsafe { DefFrameProcW(hwnd, Some(mdi), msg, wparam, lparam) }
        }
        WM_SETFOCUS => {
            channels::push(IGuiEvent::Focus {
                child_id: FRAME_CHILD_ID,
                gained: true,
            });
            unsafe { DefFrameProcW(hwnd, Some(mdi), msg, wparam, lparam) }
        }
        WM_KILLFOCUS => {
            channels::push(IGuiEvent::Focus {
                child_id: FRAME_CHILD_ID,
                gained: false,
            });
            unsafe { DefFrameProcW(hwnd, Some(mdi), msg, wparam, lparam) }
        }
        WM_SYSCOLORCHANGE | WM_THEMECHANGED => {
            super::system_colors::refresh_and_notify();
            unsafe { DefFrameProcW(hwnd, Some(mdi), msg, wparam, lparam) }
        }
        WM_CLOSE => {
            channels::push(IGuiEvent::FrameClose);
            // Close every registered MDI child, then destroy the frame.
            if mdi.0 as isize != 0 {
                for (_id, child_hwnd) in registry::snapshot() {
                    child::close_via_mdi(mdi, child_hwnd);
                }
            }
            let _ = unsafe { windows::Win32::UI::WindowsAndMessaging::DestroyWindow(hwnd) };
            LRESULT(0)
        }
        WM_DESTROY => {
            unsafe { PostQuitMessage(0) };
            LRESULT(0)
        }
        _ => unsafe { DefFrameProcW(hwnd, Some(mdi), msg, wparam, lparam) },
    }
}

fn handle_open_child(req: &OpenChildRequest) -> Option<i64> {
    let mdi = mdi_client_hwnd()?;
    let child_id = registry::allocate_child_id();
    let bootstrap = Box::into_raw(Box::new(MdiBootstrap { child_id }));
    let h_module = unsafe { GetModuleHandleW(None) }.ok()?;
    let h_owner = windows::Win32::Foundation::HANDLE(h_module.0);

    // Width/height of 0 means "use the Windows default size";
    // otherwise honour what the caller asked for.
    let cx = if req.width  > 0 { req.width  } else { CW_USEDEFAULT };
    let cy = if req.height > 0 { req.height } else { CW_USEDEFAULT };
    let mdi_create = MDICREATESTRUCTW {
        szClass: MDI_CHILD_CLASS,
        szTitle: PCWSTR::from_raw(req.title.as_ptr()),
        hOwner: h_owner,
        x: CW_USEDEFAULT,
        y: CW_USEDEFAULT,
        cx,
        cy,
        style: WS_VISIBLE | WS_OVERLAPPEDWINDOW,
        lParam: LPARAM(bootstrap as isize),
    };
    let result = unsafe {
        SendMessageW(
            mdi,
            WM_MDICREATE,
            Some(WPARAM(0)),
            Some(LPARAM(&mdi_create as *const _ as isize)),
        )
    };
    let new_hwnd = HWND(result.0 as *mut _);
    if new_hwnd.0.is_null() {
        // WM_MDICREATE failed; reclaim the bootstrap to avoid leaking.
        let _ = unsafe { Box::from_raw(bootstrap) };
        return None;
    }
    Some(child_id)
}

pub(crate) fn msg_time() -> i64 {
    unsafe { GetMessageTime() as i64 }
}

pub(crate) fn current_modifiers() -> i64 {
    let mut m = 0i64;
    unsafe {
        if (GetKeyState(VK_SHIFT.0 as i32) as i16) < 0 {
            m |= modifier::SHIFT;
        }
        if (GetKeyState(VK_CONTROL.0 as i32) as i16) < 0 {
            m |= modifier::CONTROL;
        }
        if (GetKeyState(VK_MENU.0 as i32) as i16) < 0 {
            m |= modifier::ALT;
        }
        if (GetKeyState(VK_LWIN.0 as i32) as i16) < 0
            || (GetKeyState(VK_RWIN.0 as i32) as i16) < 0
        {
            m |= modifier::WIN;
        }
        if (GetKeyState(VK_CAPITAL.0 as i32) & 1) != 0 {
            m |= modifier::CAPS;
        }
    }
    m
}

pub(crate) fn push_key(child_id: i64, down: bool, wparam: WPARAM, lparam: LPARAM) {
    let scancode = ((lparam.0 >> 16) & 0xFF) as i64;
    let repeat = (lparam.0 & 0xFFFF) as i64;
    channels::push(IGuiEvent::Key {
        child_id,
        vkey: wparam.0 as i64,
        scancode,
        mods: current_modifiers(),
        repeat,
        down,
        time_ms: msg_time(),
    });
}

pub(crate) fn push_mouse(child_id: i64, op: i64, button: i64, lparam: LPARAM) {
    let x = (lparam.0 & 0xFFFF) as i16 as i64;
    let y = ((lparam.0 >> 16) & 0xFFFF) as i16 as i64;
    channels::push(IGuiEvent::Mouse {
        child_id,
        x,
        y,
        op,
        button,
        mods: current_modifiers(),
        wheel_delta: 0,
        wheel_lines: 0,
        time_ms: msg_time(),
    });
}

// ─── Cross-thread request structures ─────────────────────────────────

pub(crate) struct OpenChildRequest {
    pub title: Vec<u16>,
    /// Initial pixel size. (0, 0) means "let Windows pick" via
    /// CW_USEDEFAULT (the existing behaviour).
    pub width: i32,
    pub height: i32,
    pub out: Option<i64>,
}

pub(crate) struct OpenTextRequest {
    pub title: Vec<u16>,
    pub out: Option<i64>,
}

pub(crate) struct OpenDocRequest {
    pub title: Vec<u16>,
    pub out: Option<i64>,
}

pub(crate) struct OpenHelpRequest {
    pub title: Vec<u16>,
    /// A docs folder (→ sidebar) or a single `.md` file path.
    pub path: String,
    pub out: Option<i64>,
}

pub(crate) struct CloseChildRequest {
    pub child_id: i64,
    pub ok: bool,
}

pub(crate) struct SetTitleRequest {
    pub child_id: i64,
    pub title: Vec<u16>,
}

pub(crate) struct SetMenuRequest {
    pub spec: String,
    pub ok: bool,
}

fn mdi_verb_from_tag(tag: u8) -> Option<super::menu::MdiVerb> {
    use super::menu::MdiVerb;
    match tag {
        1 => Some(MdiVerb::Cascade),
        2 => Some(MdiVerb::TileH),
        3 => Some(MdiVerb::TileV),
        4 => Some(MdiVerb::CloseAll),
        5 => Some(MdiVerb::ArrangeIcons),
        _ => None,
    }
}

fn mdi_verb_to_tag(verb: super::menu::MdiVerb) -> u8 {
    use super::menu::MdiVerb;
    match verb {
        MdiVerb::Cascade => 1,
        MdiVerb::TileH => 2,
        MdiVerb::TileV => 3,
        MdiVerb::CloseAll => 4,
        MdiVerb::ArrangeIcons => 5,
    }
}

/// Called from the language thread. Marshals to the GUI thread via
/// SendMessageW; blocks until the child has been created.
pub fn open_child(title: &str) -> Option<i64> {
    open_child_sized(title, 0, 0)
}

/// Open a child with an explicit initial pixel size. Pass 0 for
/// either dimension to fall back to Windows' CW_USEDEFAULT.
pub fn open_child_sized(title: &str, width: i32, height: i32) -> Option<i64> {
    let frame_raw = *FRAME_HWND.get()?;
    let frame = HWND(frame_raw as *mut _);
    let mut title_w: Vec<u16> = title.encode_utf16().collect();
    title_w.push(0);
    let mut req = OpenChildRequest {
        title: title_w,
        width,
        height,
        out: None,
    };
    unsafe {
        SendMessageW(
            frame,
            WM_IGUI_OPEN_CHILD,
            Some(WPARAM(0)),
            Some(LPARAM(&mut req as *mut _ as isize)),
        )
    };
    req.out
}

/// Called from the language thread. Same SendMessageW marshalling
/// as `open_child`, but routes to the text-view class on the GUI
/// thread (where state allocation + WM_MDICREATE happen safely).
pub fn open_text_child(title: &str) -> Option<i64> {
    let frame_raw = *FRAME_HWND.get()?;
    let frame = HWND(frame_raw as *mut _);
    let mut title_w: Vec<u16> = title.encode_utf16().collect();
    title_w.push(0);
    let mut req = OpenTextRequest {
        title: title_w,
        out: None,
    };
    unsafe {
        SendMessageW(
            frame,
            WM_IGUI_OPEN_TEXT,
            Some(WPARAM(0)),
            Some(LPARAM(&mut req as *mut _ as isize)),
        )
    };
    req.out
}

/// Open a generic Forth-writable Markdown pane.  Marshals to the GUI
/// thread like `open_text_child`; the returned child id is the token
/// Forth uses with `doc_pane::set_markdown` / `append_markdown`.
pub fn open_doc_child(title: &str) -> Option<i64> {
    let frame_raw = *FRAME_HWND.get()?;
    let frame = HWND(frame_raw as *mut _);
    let mut title_w: Vec<u16> = title.encode_utf16().collect();
    title_w.push(0);
    let mut req = OpenDocRequest {
        title: title_w,
        out: None,
    };
    unsafe {
        SendMessageW(
            frame,
            WM_IGUI_OPEN_DOC,
            Some(WPARAM(0)),
            Some(LPARAM(&mut req as *mut _ as isize)),
        )
    };
    req.out
}

/// Open a help-pane MDI child browsing `path` (a docs folder, or a single
/// `.md` file).  Marshals to the GUI thread like `open_text_child`.
pub fn open_help_child(title: &str, path: &str) -> Option<i64> {
    let frame_raw = *FRAME_HWND.get()?;
    let frame = HWND(frame_raw as *mut _);
    let mut title_w: Vec<u16> = title.encode_utf16().collect();
    title_w.push(0);
    let mut req = OpenHelpRequest {
        title: title_w,
        path: path.to_owned(),
        out: None,
    };
    unsafe {
        SendMessageW(
            frame,
            WM_IGUI_OPEN_HELP,
            Some(WPARAM(0)),
            Some(LPARAM(&mut req as *mut _ as isize)),
        )
    };
    req.out
}

pub fn close_child(child_id: i64) -> bool {
    let Some(frame_raw) = FRAME_HWND.get() else {
        return false;
    };
    let frame = HWND(*frame_raw as *mut _);
    let mut req = CloseChildRequest {
        child_id,
        ok: false,
    };
    unsafe {
        SendMessageW(
            frame,
            WM_IGUI_CLOSE_CHILD,
            Some(WPARAM(0)),
            Some(LPARAM(&mut req as *mut _ as isize)),
        )
    };
    req.ok
}

/// Marshal `spec` to the GUI thread, where it's parsed and installed
/// as the frame's menu bar. Returns true on success.
pub fn set_menu(spec: &str) -> bool {
    let Some(frame_raw) = FRAME_HWND.get() else {
        return false;
    };
    let frame = HWND(*frame_raw as *mut _);
    let mut req = SetMenuRequest {
        spec: spec.to_owned(),
        ok: false,
    };
    unsafe {
        SendMessageW(
            frame,
            WM_IGUI_SET_MENU,
            Some(WPARAM(0)),
            Some(LPARAM(&mut req as *mut _ as isize)),
        )
    };
    req.ok
}

/// Install or clear the per-child redraw timer. `interval_ms <= 0`
/// clears the timer; otherwise WM_TIMER fires every `interval_ms`
/// milliseconds and the render host pushes an `EvTick` event.
pub fn set_redraw_rate(child_id: i64, interval_ms: i64) -> bool {
    let Some(render_hwnd) = registry::render_hwnd_of(child_id) else {
        return false;
    };
    let interval = if interval_ms <= 0 { 0 } else { interval_ms as usize };
    unsafe {
        SendMessageW(
            render_hwnd,
            WM_IGUI_SET_TIMER,
            Some(WPARAM(interval)),
            Some(LPARAM(0)),
        )
    };
    true
}

/// Post a "drain the text-view command queue and repaint" message
/// at the frame. Frame WndProc dispatches to text_view's flush
/// handler on the GUI thread, which applies queued commands to the
/// child's grid and then InvalidateRects the child window. The
/// language thread never touches a child HWND. Posted (not sent)
/// so a tight write loop doesn't block on the GUI thread.
pub(crate) fn post_text_flush(child_id: i64) {
    let Some(frame_raw) = FRAME_HWND.get() else {
        return;
    };
    let frame = HWND(*frame_raw as *mut _);
    let _ = unsafe {
        PostMessageW(
            Some(frame),
            WM_IGUI_TEXT_FLUSH,
            WPARAM(child_id as usize),
            LPARAM(0),
        )
    };
}

/// Counterpart to `post_text_flush` for a Markdown doc-pane.  Posted
/// (not sent) so a streaming `append_markdown` loop doesn't block on
/// the GUI thread.
pub(crate) fn post_doc_flush(child_id: i64) {
    let Some(frame_raw) = FRAME_HWND.get() else {
        return;
    };
    let frame = HWND(*frame_raw as *mut _);
    let _ = unsafe {
        PostMessageW(
            Some(frame),
            WM_IGUI_DOC_FLUSH,
            WPARAM(child_id as usize),
            LPARAM(0),
        )
    };
}

/// Counterpart to `post_text_flush` for the singleton fconsole
/// pane.  Posted (not sent) so a tight worker write loop doesn't
/// block on the GUI thread.
pub(crate) fn post_fconsole_flush() {
    let Some(frame_raw) = FRAME_HWND.get() else {
        return;
    };
    let frame = HWND(*frame_raw as *mut _);
    let _ = unsafe {
        PostMessageW(
            Some(frame),
            WM_IGUI_FCONSOLE_FLUSH,
            WPARAM(0),
            LPARAM(0),
        )
    };
}

/// Worker thread (or any non-GUI thread) calls this after pushing
/// a new crash dump into `crash_view::DUMPS`, to ask the UI thread
/// to open / refresh the crash view.
pub(crate) fn post_crash_flush() {
    let Some(frame_raw) = FRAME_HWND.get() else {
        return;
    };
    let frame = HWND(*frame_raw as *mut _);
    let _ = unsafe {
        PostMessageW(
            Some(frame),
            WM_IGUI_CRASH_FLUSH,
            WPARAM(0),
            LPARAM(0),
        )
    };
}


/// Marshal an MDI verb to the GUI thread for execution.
pub fn dispatch_mdi_verb(verb: super::menu::MdiVerb) {
    let Some(frame_raw) = FRAME_HWND.get() else {
        return;
    };
    let frame = HWND(*frame_raw as *mut _);
    let tag = mdi_verb_to_tag(verb) as usize;
    unsafe {
        SendMessageW(
            frame,
            WM_IGUI_MDI_VERB,
            Some(WPARAM(tag)),
            Some(LPARAM(0)),
        )
    };
}

pub fn set_child_title(child_id: i64, title: &str) {
    let Some(frame_raw) = FRAME_HWND.get() else {
        return;
    };
    let frame = HWND(*frame_raw as *mut _);
    let mut title_w: Vec<u16> = title.encode_utf16().collect();
    title_w.push(0);
    let req = SetTitleRequest {
        child_id,
        title: title_w,
    };
    unsafe {
        SendMessageW(
            frame,
            WM_IGUI_SET_TITLE,
            Some(WPARAM(0)),
            Some(LPARAM(&req as *const _ as isize)),
        )
    };
}

