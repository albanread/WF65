//! fconsole — interactive Forth REPL pane.
//!
//! Acts like a real terminal: one continuous text flow.  Output
//! lines accumulate at the top; the prompt + current input line
//! sits at the bottom of the visible area.  No separate input
//! strip — the cursor lives within the input string itself.
//!
//! Process-wide state mirrors `log_view`: a `Mutex<ConsoleState>`
//! that holds the scrollback ring and the input/history, callable
//! from any thread (the worker thread `append`s; the UI thread
//! reads via `paint`).  Per-window state is just rendering
//! resources + scroll offset.
//!
//! Keys handled directly:
//!   Enter             submit current input line → worker
//!   Backspace         delete codepoint left of cursor
//!   Delete            delete codepoint right of cursor
//!   Left / Right      move cursor within input
//!   Home / End        jump to start / end of input
//!   Up / Down         recall previous / next history entry
//!   Ctrl+L            clear scrollback
//!   Ctrl+U            clear input line (line-kill)
//!   Wheel             scroll scrollback
//!
//! History is process-wide so it survives a Forth restart.

#![cfg(windows)]

use std::sync::Mutex;

use windows::core::{w, PCWSTR};
use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, RECT, WPARAM};
use windows::Win32::Graphics::Direct2D::Common::{
    D2D1_ALPHA_MODE_IGNORE, D2D1_COLOR_F, D2D1_PIXEL_FORMAT, D2D_RECT_F, D2D_SIZE_U,
};
use windows::Win32::Graphics::Direct2D::{
    ID2D1HwndRenderTarget, ID2D1SolidColorBrush, D2D1_BRUSH_PROPERTIES,
    D2D1_DRAW_TEXT_OPTIONS_CLIP, D2D1_FEATURE_LEVEL_DEFAULT,
    D2D1_HWND_RENDER_TARGET_PROPERTIES, D2D1_PRESENT_OPTIONS_NONE,
    D2D1_RENDER_TARGET_PROPERTIES, D2D1_RENDER_TARGET_TYPE_DEFAULT,
    D2D1_RENDER_TARGET_USAGE_NONE,
};
use windows::Win32::Graphics::DirectWrite::{
    IDWriteTextFormat, IDWriteTextLayout, DWRITE_FONT_STRETCH_NORMAL, DWRITE_FONT_STYLE_NORMAL,
    DWRITE_FONT_WEIGHT, DWRITE_TEXT_METRICS, DWRITE_WORD_WRAPPING_NO_WRAP,
};
use windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT_B8G8R8A8_UNORM;
use windows::Win32::Graphics::Gdi::InvalidateRect;
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::HiDpi::GetDpiForWindow;
use windows::Win32::UI::Input::KeyboardAndMouse::{
    GetKeyState, SetFocus, VK_BACK, VK_CONTROL, VK_DELETE, VK_DOWN, VK_END, VK_HOME, VK_LEFT,
    VK_NEXT, VK_PRIOR, VK_RETURN, VK_RIGHT, VK_SHIFT, VK_UP,
};
use windows::Win32::UI::WindowsAndMessaging::{
    BringWindowToTop, DefMDIChildProcW, GetClientRect, GetWindowLongPtrW, IsWindow, LoadCursorW,
    RegisterClassExW, SendMessageW, SetWindowLongPtrW, CW_USEDEFAULT, GWLP_USERDATA, IDC_ARROW,
    MDICREATESTRUCTW, WHEEL_DELTA, WM_CHAR, WM_DPICHANGED_AFTERPARENT, WM_KEYDOWN, WM_LBUTTONDOWN,
    WM_MDIACTIVATE, WM_MDICREATE, WM_MOUSEWHEEL, WM_NCCREATE, WM_NCDESTROY, WM_PAINT,
    WM_SETFOCUS, WM_SIZE, WNDCLASSEXW, WNDCLASS_STYLES, WS_OVERLAPPEDWINDOW, WS_VISIBLE,
};

use super::renderer;

/// Set `WF64_UI_TRACE=1` in the environment before launching to dump
/// µs-precision timings to stderr for every keystroke, paint, append,
/// and worker event.  Tells us where the latency is when the console
/// feels slow.
fn trace_enabled() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| std::env::var_os("WF64_UI_TRACE").is_some())
}

#[inline]
pub fn trace(tag: &str, body: std::fmt::Arguments<'_>) {
    if !trace_enabled() { return; }
    use std::time::{SystemTime, UNIX_EPOCH};
    let now = SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default();
    let secs = now.as_secs() % 1000;
    let us = now.subsec_micros();
    let tid = unsafe { windows::Win32::System::Threading::GetCurrentThreadId() };
    eprintln!("[{secs:03}.{us:06} t{tid:>5}] {tag}: {body}");
}

macro_rules! trace_now {
    ($tag:expr, $($t:tt)*) => { trace($tag, format_args!($($t)*)); };
}

pub const MENU_CMD_ID: u16 = 0x3002;

const SCROLLBACK_CAP: usize = 8192;
const LINE_MAX: usize = 4096;
const HISTORY_CAP: usize = 1024;
const PROMPT: &str = "> ";
/// Continuation prompt for multi-line input (e.g. mid colon-def).
/// Same width as `PROMPT` so the input column stays aligned.
const CONT_PROMPT: &str = ". ";

const CLASS_NAME: PCWSTR = w!("WF64.iGui.Fconsole");
const TITLE: PCWSTR = w!("\u{2234} console");

static FCONSOLE_HWND: Mutex<Option<isize>> = Mutex::new(None);

// ─── Process-wide state ────────────────────────────────────────────
//
// Worker thread and UI thread don't share a single Mutex anymore.
// Worker pushes to `PENDING` (a separate Mutex) and PostMessages
// the frame; the UI thread, in its WndProc, drains PENDING onto
// `CONSOLE` and invalidates.  The UI thread also owns paint
// snapshotting of CONSOLE.  This mirrors text_view's flush-on-
// gui-thread model so all visible-state mutation lives on the GUI
// thread — the worker's append is a non-blocking enqueue + post.

struct ConsoleState {
    /// Output lines, oldest first.  Submitted input is pushed in
    /// here too, prefixed with `> `, so the transcript reads
    /// continuously.
    lines: Vec<String>,
    /// Live input, codepoints.  Vec<char> (not String) so cursor
    /// motion is constant-time and Unicode-safe.
    input: Vec<char>,
    /// Cursor offset within `input`, in codepoints.  `0 ..=
    /// input.len()`.
    cursor: usize,
    /// Submitted lines, newest at the end.  Up/Down walks an
    /// index into this; `None` = at the live draft.
    history: Vec<String>,
    /// `None` = editing the live draft.  `Some(i)` = displaying
    /// history[i].  Up decrements (older), Down increments
    /// (newer).
    history_idx: Option<usize>,
    /// Saved live-draft when the user starts walking history,
    /// so Down past the newest restores what they were typing.
    history_draft: Option<(Vec<char>, usize)>,
}

/// Applied console state — only the UI thread mutates it (during
/// `flush_on_gui_thread` and `handle_char`/`handle_key`).  Paint
/// snapshots it briefly.
static CONSOLE: Mutex<Option<ConsoleState>> = Mutex::new(None);

/// Pending lines from the worker thread, awaiting drain by the UI
/// thread on WM_IGUI_FCONSOLE_FLUSH.  Separate Mutex so the worker
/// never contends with paint or UI-thread state mutation.
static PENDING: Mutex<Vec<String>> = Mutex::new(Vec::new());

fn with_console<R>(f: impl FnOnce(&mut ConsoleState) -> R) -> R {
    let mut guard = CONSOLE.lock().expect("CONSOLE poisoned");
    let state = guard.get_or_insert_with(|| ConsoleState {
        lines: Vec::new(),
        input: Vec::new(),
        cursor: 0,
        history: Vec::new(),
        history_idx: None,
        history_draft: None,
    });
    f(state)
}

/// Append one line to the scrollback.  Safe from any thread.
///
/// Worker path: pushes onto the pending queue (briefest possible
/// lock — just one `Vec::push`), then PostMessages the frame to
/// drain on the GUI thread.  The actual scrollback mutation
/// happens inside `flush_on_gui_thread`.
///
/// UI-thread callers (echoes from `submit_input`, etc.) take the
/// same path for uniformity — the PostMessage will be processed
/// in the same pump cycle and serializes correctly with paints.
pub fn append(line: &str) {
    let t0 = std::time::Instant::now();
    let trimmed_owned: String = if line.len() > LINE_MAX {
        let mut end = LINE_MAX;
        while end > 0 && !line.is_char_boundary(end) {
            end -= 1;
        }
        line[..end].to_string()
    } else {
        line.to_string()
    };
    let trimmed_len = trimmed_owned.len();
    {
        let mut q = PENDING.lock().expect("PENDING poisoned");
        q.push(trimmed_owned);
    }
    let t1 = std::time::Instant::now();
    super::window::post_fconsole_flush();
    let t2 = std::time::Instant::now();
    trace_now!("append",
        "len={} enqueue={}us post={}us",
        trimmed_len,
        (t1 - t0).as_micros(),
        (t2 - t1).as_micros());
}

/// UI-thread drain: pull every pending line onto the applied
/// scrollback and invalidate.  Called from the frame WndProc on
/// `WM_IGUI_FCONSOLE_FLUSH`.  Returns the number of lines drained
/// (mostly for tracing).
pub(super) fn flush_on_gui_thread() -> usize {
    let t0 = std::time::Instant::now();
    let drained: Vec<String> = {
        let mut q = PENDING.lock().expect("PENDING poisoned");
        std::mem::take(&mut *q)
    };
    if drained.is_empty() {
        return 0;
    }
    let n = drained.len();
    with_console(|state| {
        for line in drained {
            if state.lines.len() >= SCROLLBACK_CAP {
                state.lines.remove(0);
            }
            state.lines.push(line);
        }
    });
    let t1 = std::time::Instant::now();
    request_repaint();
    let t2 = std::time::Instant::now();
    trace_now!("flush",
        "n={} drain+apply={}us invalidate={}us",
        n,
        (t1 - t0).as_micros(),
        (t2 - t1).as_micros());
    n
}

/// Clear everything (scrollback + input + history).  Called by
/// Forth-Restart so a fresh session gets a fresh screen.
pub fn reset_for_restart() {
    with_console(|state| {
        state.lines.clear();
        state.input.clear();
        state.cursor = 0;
        state.history_idx = None;
        state.history_draft = None;
        // keep state.history — useful to repeat what you'd been
        // doing on the previous session.
    });
    request_repaint();
}

/// Clear the scrollback only (input and history preserved).
/// Backs the Forth word `PAGE`.
pub fn clear_screen() {
    with_console(|state| {
        state.lines.clear();
    });
    request_repaint();
}

/// Overlay-write a string at `(col, row)` in the scrollback.
/// Backs the Forth word `AT-XY` followed by text emission.
///
/// V1 semantics: pad the scrollback with empty lines until it has
/// at least `row + 1` lines, then overwrite characters starting at
/// column `col` in `lines[row]` with `text`.  If the call lands
/// past the current width of that line, the line is space-padded
/// first.  Subsequent `append` calls continue to push at the end
/// of the scrollback — at-xy is one-shot per call.
pub fn write_at(col: usize, row: usize, text: &str) {
    with_console(|state| {
        while state.lines.len() <= row {
            state.lines.push(String::new());
        }
        let line = &mut state.lines[row];
        // Convert to chars for column-correct overlay.
        let mut chars: Vec<char> = line.chars().collect();
        while chars.len() < col {
            chars.push(' ');
        }
        for (i, c) in text.chars().enumerate() {
            let pos = col + i;
            if pos < chars.len() {
                chars[pos] = c;
            } else {
                chars.push(c);
            }
        }
        *line = chars.into_iter().collect();
    });
    request_repaint();
}

fn request_repaint() {
    if let Some(raw) = *FCONSOLE_HWND.lock().expect("FCONSOLE_HWND poisoned") {
        let hwnd = HWND(raw as *mut _);
        if unsafe { IsWindow(Some(hwnd)) }.as_bool() {
            let _ = unsafe { InvalidateRect(Some(hwnd), None, false) };
        }
    }
}

// ─── Class registration / open ────────────────────────────────────

pub fn register_class() -> Result<(), super::IGuiError> {
    let h_instance = unsafe { GetModuleHandleW(None) }
        .map_err(|e| super::IGuiError::Win32(format!("GetModuleHandleW (fconsole): {e}")))?
        .into();
    let cursor = unsafe { LoadCursorW(None, IDC_ARROW) }
        .map_err(|e| super::IGuiError::Win32(format!("LoadCursorW (fconsole): {e}")))?;
    let cls = WNDCLASSEXW {
        cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
        style: WNDCLASS_STYLES(0),
        lpfnWndProc: Some(fconsole_wnd_proc),
        cbClsExtra: 0,
        cbWndExtra: 0,
        hInstance: h_instance,
        hIcon: unsafe { super::window::app_icon() },
        hCursor: cursor,
        hbrBackground: Default::default(),
        lpszMenuName: PCWSTR::null(),
        lpszClassName: CLASS_NAME,
        hIconSm: unsafe { super::window::app_icon() },
    };
    let _ = unsafe { RegisterClassExW(&cls) };
    Ok(())
}

pub fn open(_frame: HWND, mdi_client: HWND) {
    if let Some(raw) = *FCONSOLE_HWND.lock().expect("FCONSOLE_HWND poisoned") {
        let hwnd = HWND(raw as *mut _);
        if unsafe { IsWindow(Some(hwnd)) }.as_bool() {
            unsafe {
                SendMessageW(
                    mdi_client,
                    WM_MDIACTIVATE,
                    Some(WPARAM(hwnd.0 as usize)),
                    Some(LPARAM(0)),
                )
            };
            let _ = unsafe { BringWindowToTop(hwnd) };
            return;
        }
    }

    let h_instance = match unsafe { GetModuleHandleW(None) } {
        Ok(h) => windows::Win32::Foundation::HANDLE(h.0),
        Err(e) => {
            eprintln!("[fconsole] GetModuleHandleW: {e}");
            return;
        }
    };

    let mut client_rect = RECT::default();
    let _ = unsafe { GetClientRect(mdi_client, &mut client_rect) };
    let w_full = (client_rect.right - client_rect.left).max(400);
    let h_full = (client_rect.bottom - client_rect.top).max(200);
    let width = (w_full * 6 / 10).max(360);
    let height = h_full;
    let x = (w_full - width).max(0);

    let create = MDICREATESTRUCTW {
        szClass: CLASS_NAME,
        szTitle: TITLE,
        hOwner: h_instance,
        x,
        y: 0,
        cx: width,
        cy: height,
        style: WS_VISIBLE | WS_OVERLAPPEDWINDOW,
        lParam: LPARAM(0),
    };
    let result = unsafe {
        SendMessageW(
            mdi_client,
            WM_MDICREATE,
            Some(WPARAM(0)),
            Some(LPARAM(&create as *const _ as isize)),
        )
    };
    if result.0 == 0 {
        eprintln!("[fconsole] WM_MDICREATE returned 0");
        let _ = CW_USEDEFAULT;
    }
}

// ─── Per-window state ─────────────────────────────────────────────

struct ConsoleWindowState {
    hwnd: HWND,
    target: Option<ID2D1HwndRenderTarget>,
    text_format: Option<IDWriteTextFormat>,
    cell_w: f32,
    cell_h: f32,
    /// Number of lines scrolled past the bottom.  0 = the prompt
    /// is at the bottom of the visible area.
    scroll_offset: usize,
    client_w: u32,
    client_h: u32,
    dpi: u32,
}

impl ConsoleWindowState {
    fn new(hwnd: HWND) -> Self {
        let dpi = unsafe { GetDpiForWindow(hwnd) };
        let dpi = if dpi == 0 { 96 } else { dpi };
        Self {
            hwnd,
            target: None,
            text_format: None,
            cell_w: 8.0,
            cell_h: 16.0,
            scroll_offset: 0,
            client_w: 0,
            client_h: 0,
            dpi,
        }
    }

    fn ensure_target(&mut self) {
        if self.target.is_some() {
            return;
        }
        let factory = &renderer::ctx().d2d.factory;
        let rect = unsafe {
            let mut r = RECT::default();
            let _ = GetClientRect(self.hwnd, &mut r);
            r
        };
        let size = D2D_SIZE_U {
            width: (rect.right - rect.left).max(1) as u32,
            height: (rect.bottom - rect.top).max(1) as u32,
        };
        self.client_w = size.width;
        self.client_h = size.height;
        let rt_props = D2D1_RENDER_TARGET_PROPERTIES {
            r#type: D2D1_RENDER_TARGET_TYPE_DEFAULT,
            pixelFormat: D2D1_PIXEL_FORMAT {
                format: DXGI_FORMAT_B8G8R8A8_UNORM,
                alphaMode: D2D1_ALPHA_MODE_IGNORE,
            },
            dpiX: self.dpi as f32,
            dpiY: self.dpi as f32,
            usage: D2D1_RENDER_TARGET_USAGE_NONE,
            minLevel: D2D1_FEATURE_LEVEL_DEFAULT,
        };
        let hwnd_props = D2D1_HWND_RENDER_TARGET_PROPERTIES {
            hwnd: self.hwnd,
            pixelSize: size,
            presentOptions: D2D1_PRESENT_OPTIONS_NONE,
        };
        match unsafe { factory.CreateHwndRenderTarget(&rt_props, &hwnd_props) } {
            Ok(t) => self.target = Some(t),
            Err(e) => eprintln!("[fconsole] CreateHwndRenderTarget: {e}"),
        }
    }

    fn ensure_text_format(&mut self) {
        if self.text_format.is_some() {
            return;
        }
        let dw_factory = &renderer::ctx().dwrite.factory;
        let scale = self.dpi as f32 / 96.0;
        let font_size = 14.0_f32 * scale;
        match unsafe {
            dw_factory.CreateTextFormat(
                w!("Cascadia Mono"),
                None,
                DWRITE_FONT_WEIGHT(400),
                DWRITE_FONT_STYLE_NORMAL,
                DWRITE_FONT_STRETCH_NORMAL,
                font_size,
                w!("en-us"),
            )
        } {
            Ok(fmt) => {
                let _ = unsafe { fmt.SetWordWrapping(DWRITE_WORD_WRAPPING_NO_WRAP) };
                if let Ok(layout) = unsafe {
                    dw_factory.CreateTextLayout(
                        &"M".encode_utf16().collect::<Vec<u16>>(),
                        &fmt,
                        4096.0,
                        4096.0,
                    )
                } {
                    let mut metrics = DWRITE_TEXT_METRICS::default();
                    let _ = unsafe { layout.GetMetrics(&mut metrics) };
                    if metrics.width > 0.0 {
                        self.cell_w = metrics.width;
                    }
                    if metrics.height > 0.0 {
                        self.cell_h = metrics.height;
                    }
                }
                self.text_format = Some(fmt);
            }
            Err(e) => eprintln!("[fconsole] CreateTextFormat: {e}"),
        }
    }

    fn paint(&mut self) {
        let t0 = std::time::Instant::now();
        self.ensure_target();
        self.ensure_text_format();
        let Some(target) = self.target.clone() else { return; };
        let Some(format) = self.text_format.clone() else { return; };
        let dw_factory = &renderer::ctx().dwrite.factory;

        unsafe { target.BeginDraw() };
        unsafe {
            target.Clear(Some(&D2D1_COLOR_F {
                r: 0.06,
                g: 0.08,
                b: 0.12,
                a: 1.0,
            }));
        }

        let fg = make_brush(&target, 0.85, 0.90, 0.95, 1.0);
        let prompt_brush = make_brush(&target, 0.95, 0.70, 0.30, 1.0); // amber

        let pad_x = 8.0_f32;
        let pad_y = 6.0_f32;
        let w = self.client_w as f32;
        let h = self.client_h as f32;
        let cell_h = self.cell_h;
        let visible_rows =
            ((h - pad_y * 2.0) / cell_h).floor().max(1.0) as usize;
        if visible_rows < 1 {
            let _ = unsafe { target.EndDraw(None, None) };
            return;
        }

        // Snapshot under the lock.
        let (lines, input_str, cursor_idx) = with_console(|state| {
            let input_str: String = state.input.iter().collect();
            (state.lines.clone(), input_str, state.cursor)
        });

        let cols_per_row = ((w - pad_x * 2.0) / self.cell_w)
            .floor()
            .max(1.0) as usize;
        let prompt_chars = PROMPT.chars().count();

        // Build the prompt rows separately and pin them to the
        // bottom of the client area — they don't scroll.  The
        // scrollback area above scrolls independently via
        // `scroll_offset`, which is now correctly clamped to the
        // post-wrap visual row count.
        let mut prompt_rows: Vec<VRowOpaque> = Vec::with_capacity(4);
        wrap_prompt_block(
            &input_str,
            cursor_idx,
            prompt_chars,
            cols_per_row,
            &mut prompt_rows,
        );
        let prompt_row_count = prompt_rows.len().max(1);
        // Reserve `prompt_row_count` rows at the bottom for the
        // prompt + a 1-row gap separator above it.
        let reserved_rows = prompt_row_count + 1;
        let scrollback_rows = visible_rows.saturating_sub(reserved_rows);

        // Wrap scrollback into visual rows.
        let mut rows: Vec<VRowOpaque> = Vec::with_capacity(lines.len() + 4);
        for line in &lines {
            wrap_into_rows(line, cols_per_row, None, None, &mut rows);
        }

        // Clamp scroll_offset against the post-wrap row count, NOT
        // against the logical-line count.  Pre-fix, the clamp used
        // `lines.len() + 1`, which meant a console full of
        // wrap-heavy demo output (long lines that span 3–5 visual
        // rows each) capped you well short of the real top.
        let max_scroll = rows.len().saturating_sub(scrollback_rows);
        if self.scroll_offset > max_scroll {
            self.scroll_offset = max_scroll;
        }

        let bottom_idx = rows.len().saturating_sub(self.scroll_offset);
        let top_idx = bottom_idx.saturating_sub(scrollback_rows);
        for (row_screen, idx) in (top_idx..bottom_idx).enumerate() {
            let y = pad_y + row_screen as f32 * cell_h;
            let row = &rows[idx];
            // Scrollback rows never have a prompt head (those are
            // the live input rows, painted separately below).
            let text_x = pad_x;
            if !row.text.is_empty() {
                draw_text(
                    dw_factory,
                    &target,
                    &format,
                    &row.text,
                    text_x,
                    y,
                    w - text_x - pad_x,
                    cell_h,
                    fg.as_ref(),
                );
            }
        }

        // "Scrolled-up" hint: when not anchored to the bottom,
        // show a faint marker on the separator row so the user
        // knows they're not seeing live output.
        let sep_row = scrollback_rows;
        let sep_y = pad_y + sep_row as f32 * cell_h;
        if self.scroll_offset > 0 {
            let hint = format!(
                "── scrolled up: {} row(s) below — End / wheel-down to return ──",
                self.scroll_offset,
            );
            draw_text(
                dw_factory, &target, &format,
                &hint, pad_x, sep_y, w - pad_x * 2.0, cell_h,
                prompt_brush.as_ref(),
            );
        }

        // Prompt rows — pinned to the bottom of the visible area.
        let prompt_top_row = scrollback_rows + 1;
        for (i, row) in prompt_rows.iter().enumerate() {
            let y = pad_y + (prompt_top_row + i) as f32 * cell_h;
            let text_x = if let Some(prompt) = row.prompt_head {
                draw_text(
                    dw_factory,
                    &target,
                    &format,
                    prompt,
                    pad_x,
                    y,
                    self.cell_w * prompt_chars as f32 + 4.0,
                    cell_h,
                    prompt_brush.as_ref(),
                );
                pad_x + prompt_chars as f32 * self.cell_w
            } else {
                pad_x
            };
            if !row.text.is_empty() {
                draw_text(
                    dw_factory,
                    &target,
                    &format,
                    &row.text,
                    text_x,
                    y,
                    w - text_x - pad_x,
                    cell_h,
                    fg.as_ref(),
                );
            }
            if let Some(col) = row.caret_col {
                let caret_x = pad_x + col as f32 * self.cell_w;
                paint_caret(&target, prompt_brush.as_ref(), caret_x, y, cell_h);
            }
        }

        let t1 = std::time::Instant::now();
        let _ = unsafe { target.EndDraw(None, None) };
        let t2 = std::time::Instant::now();
        let line_count = with_console(|s| s.lines.len());
        trace_now!("paint",
            "lines={} draw={}us enddraw={}us total={}us",
            line_count,
            (t1 - t0).as_micros(),
            (t2 - t1).as_micros(),
            (t2 - t0).as_micros());
    }

    fn submit_input(&mut self) {
        let (line_to_send, echo_lines) = with_console(|state| {
            let source: String = state.input.iter().collect();
            state.input.clear();
            state.cursor = 0;
            state.history_idx = None;
            state.history_draft = None;
            // Echo: first logical line gets PROMPT, continuation
            // lines get CONT_PROMPT so it reads as a single block.
            let echo_lines: Vec<String> = if source.is_empty() {
                vec![PROMPT.to_string()]
            } else {
                source
                    .split('\n')
                    .enumerate()
                    .map(|(i, line)| {
                        format!(
                            "{}{}",
                            if i == 0 { PROMPT } else { CONT_PROMPT },
                            line,
                        )
                    })
                    .collect()
            };
            if !source.is_empty() {
                if state.history.last().map(|s| s.as_str()) != Some(source.as_str()) {
                    if state.history.len() >= HISTORY_CAP {
                        state.history.remove(0);
                    }
                    state.history.push(source.clone());
                }
            }
            (source, echo_lines)
        });
        // Push each echoed logical line.  Empty input still echoes
        // (matches REPL convention — blank line shows the prompt
        // again) but is not sent to the worker.
        for line in &echo_lines {
            append(line);
        }
        if line_to_send.is_empty() {
            return;
        }
        super::channels::push(super::channels::IGuiEvent::EvalBuffer {
            source: line_to_send,
        });
        self.scroll_offset = 0;
    }

    /// Try to move the cursor up (`dir = -1`) or down (`dir = +1`)
    /// one logical line within the input buffer, preserving the
    /// column.  Returns `true` if a vertical move was possible (i.e.
    /// there is another logical line in that direction); `false`
    /// otherwise, in which case the caller falls back to a history
    /// walk so simple single-line input keeps its old behaviour.
    fn move_cursor_vertical(&mut self, dir: i32) -> bool {
        let moved = with_console(|state| {
            let line_start = logical_line_start(&state.input, state.cursor);
            let col = state.cursor - line_start;
            if dir < 0 {
                if line_start == 0 {
                    return false;
                }
                // Cursor moves to col within the previous logical
                // line; clamp to its length.
                let prev_end = line_start - 1; // index of the '\n'
                let prev_start = logical_line_start(&state.input, prev_end);
                let prev_len = prev_end - prev_start;
                state.cursor = prev_start + col.min(prev_len);
                true
            } else {
                // Down: find the '\n' after current line.
                let cur_end = logical_line_end(&state.input, state.cursor);
                if cur_end == state.input.len() {
                    return false;
                }
                let next_start = cur_end + 1;
                let next_end = logical_line_end(&state.input, next_start);
                let next_len = next_end - next_start;
                state.cursor = next_start + col.min(next_len);
                true
            }
        });
        moved
    }

    fn insert_newline(&mut self) {
        with_console(|state| {
            state.history_idx = None;
            state.history_draft = None;
            if state.input.len() < LINE_MAX {
                state.input.insert(state.cursor, '\n');
                state.cursor += 1;
            }
        });
        self.scroll_offset = 0;
    }

    fn handle_char(&mut self, c: char) {
        if (c as u32) < 0x20 {
            return; // control char — handled in WM_KEYDOWN
        }
        with_console(|state| {
            // If user edits while looking at history, that becomes
            // their new draft (history_idx clears).
            state.history_idx = None;
            state.history_draft = None;
            if state.input.len() < LINE_MAX {
                state.input.insert(state.cursor, c);
                state.cursor += 1;
            }
        });
        self.scroll_offset = 0;
        let _ = unsafe { InvalidateRect(Some(self.hwnd), None, false) };
    }

    fn handle_key(&mut self, vk: u32) {
        let vk16 = vk as u16;
        let ctrl = (unsafe { GetKeyState(VK_CONTROL.0 as i32) } as i16) < 0;

        let shift = (unsafe { GetKeyState(VK_SHIFT.0 as i32) } as i16) < 0;
        let mut needs_repaint = true;
        if vk16 == VK_RETURN.0 {
            // Shift+Enter always inserts a newline so users can
            // type multi-line input even when the form scanner
            // would otherwise see it as complete.  Plain Enter
            // submits when the input is a complete Forth form
            // (balanced colon-def, no unclosed paren-comment or
            // string) and inserts a newline otherwise.
            if shift {
                self.insert_newline();
            } else {
                let snapshot: String = with_console(|s| s.input.iter().collect());
                match super::repl_pane::form_complete(&snapshot) {
                    super::repl_pane::FormStatus::Complete => self.submit_input(),
                    _ => self.insert_newline(),
                }
            }
        } else if vk16 == VK_BACK.0 {
            with_console(|state| {
                state.history_idx = None;
                state.history_draft = None;
                if state.cursor > 0 {
                    state.cursor -= 1;
                    state.input.remove(state.cursor);
                }
            });
            self.scroll_offset = 0;
        } else if vk16 == VK_DELETE.0 {
            with_console(|state| {
                state.history_idx = None;
                state.history_draft = None;
                if state.cursor < state.input.len() {
                    state.input.remove(state.cursor);
                }
            });
            self.scroll_offset = 0;
        } else if vk16 == VK_LEFT.0 {
            with_console(|state| {
                if state.cursor > 0 {
                    state.cursor -= 1;
                }
            });
        } else if vk16 == VK_RIGHT.0 {
            with_console(|state| {
                if state.cursor < state.input.len() {
                    state.cursor += 1;
                }
            });
        } else if vk16 == VK_HOME.0 {
            // Ctrl+Home → start of full input.  Plain Home → start
            // of the current logical line (after the prompt).
            if ctrl {
                with_console(|state| state.cursor = 0);
            } else {
                with_console(|state| {
                    state.cursor = logical_line_start(&state.input, state.cursor);
                });
            }
        } else if vk16 == VK_END.0 {
            if ctrl {
                // Ctrl+End — jump to the live tail of the
                // transcript AND to the end of the input buffer.
                // Reads naturally: "go to the end of everything."
                with_console(|state| state.cursor = state.input.len());
                self.scroll_offset = 0;
            } else {
                with_console(|state| {
                    state.cursor = logical_line_end(&state.input, state.cursor);
                });
            }
        } else if vk16 == VK_UP.0 {
            // Move up one logical line if there is one; otherwise
            // walk history backwards.  Keeps simple single-line
            // input feeling identical, while multi-line input
            // behaves like an editor.
            if !self.move_cursor_vertical(-1) {
                self.history_walk(-1);
            }
            self.scroll_offset = 0;
        } else if vk16 == VK_DOWN.0 {
            if !self.move_cursor_vertical(1) {
                self.history_walk(1);
            }
            self.scroll_offset = 0;
        } else if vk16 == VK_PRIOR.0 {
            // PageUp — scroll back through the transcript.  Does
            // NOT touch the input buffer or cursor; the prompt
            // stays pinned to the bottom.
            self.page_scroll(1);
            needs_repaint = false; // page_scroll invalidates itself
        } else if vk16 == VK_NEXT.0 {
            // PageDown — scroll forward (towards the live tail).
            self.page_scroll(-1);
            needs_repaint = false;
        } else if ctrl && vk == 'L' as u32 {
            with_console(|state| state.lines.clear());
            self.scroll_offset = 0;
        } else if ctrl && vk == 'U' as u32 {
            with_console(|state| {
                state.input.clear();
                state.cursor = 0;
                state.history_idx = None;
                state.history_draft = None;
            });
            self.scroll_offset = 0;
        } else {
            needs_repaint = false;
        }
        if needs_repaint {
            let _ = unsafe { InvalidateRect(Some(self.hwnd), None, false) };
        }
    }

    /// `direction = -1` for Up (older), `+1` for Down (newer).
    fn history_walk(&mut self, direction: i32) {
        with_console(|state| {
            let h_len = state.history.len();
            if h_len == 0 {
                return;
            }
            let next_idx: Option<usize> = match (state.history_idx, direction) {
                (None, -1) => {
                    // Save the live draft, start at newest.
                    state.history_draft = Some((state.input.clone(), state.cursor));
                    Some(h_len - 1)
                }
                (None, _) => None, // Down on draft: no-op
                (Some(i), -1) => Some(i.saturating_sub(1)),
                (Some(i), 1) => {
                    if i + 1 >= h_len {
                        // Past the newest — restore draft.
                        None
                    } else {
                        Some(i + 1)
                    }
                }
                _ => state.history_idx,
            };
            state.history_idx = next_idx;
            match next_idx {
                Some(i) => {
                    state.input = state.history[i].chars().collect();
                    state.cursor = state.input.len();
                }
                None => {
                    if let Some((draft, cur)) = state.history_draft.take() {
                        state.input = draft;
                        state.cursor = cur;
                    } else {
                        state.input.clear();
                        state.cursor = 0;
                    }
                }
            }
        });
    }

    fn handle_wheel(&mut self, delta: i16) {
        let steps = (delta as i32 / WHEEL_DELTA as i32).abs().max(1) as usize;
        if delta > 0 {
            // Wheel-up = scroll back (older content).  Final clamp
            // against the post-wrap row count happens in paint(),
            // since only paint knows the wrap width.
            self.scroll_offset = self.scroll_offset.saturating_add(steps * 3);
        } else {
            self.scroll_offset = self.scroll_offset.saturating_sub(steps * 3);
        }
        let _ = unsafe { InvalidateRect(Some(self.hwnd), None, false) };
    }

    /// Page up by roughly half a screen.  Pure scroll adjustment;
    /// final clamp against the wrap-row count happens in paint().
    fn page_scroll(&mut self, dir: i32) {
        // Estimate "half a screen" from the current client height.
        let half = ((self.client_h as f32) / (self.cell_h * 2.0))
            .floor()
            .max(1.0) as usize;
        if dir > 0 {
            self.scroll_offset = self.scroll_offset.saturating_add(half);
        } else {
            self.scroll_offset = self.scroll_offset.saturating_sub(half);
        }
        let _ = unsafe { InvalidateRect(Some(self.hwnd), None, false) };
    }

    fn scroll_to_bottom(&mut self) {
        if self.scroll_offset == 0 {
            return;
        }
        self.scroll_offset = 0;
        let _ = unsafe { InvalidateRect(Some(self.hwnd), None, false) };
    }

    fn handle_resize(&mut self) {
        let mut r = RECT::default();
        let _ = unsafe { GetClientRect(self.hwnd, &mut r) };
        let new_w = (r.right - r.left).max(1) as u32;
        let new_h = (r.bottom - r.top).max(1) as u32;
        if new_w == self.client_w && new_h == self.client_h {
            return;
        }
        self.client_w = new_w;
        self.client_h = new_h;
        if let Some(t) = self.target.as_ref() {
            let _ = unsafe {
                t.Resize(&D2D_SIZE_U {
                    width: new_w,
                    height: new_h,
                })
            };
        }
    }
}

/// Codepoint index of the first char on the logical line that
/// contains `cursor` within the multi-line `input`.  Equivalent to
/// "skip backwards over non-newline chars, stop just after the
/// previous `\n` or at index 0".
fn logical_line_start(input: &[char], cursor: usize) -> usize {
    let mut i = cursor.min(input.len());
    while i > 0 && input[i - 1] != '\n' {
        i -= 1;
    }
    i
}

/// Codepoint index of the last char on the logical line that
/// contains `cursor` (i.e. position of the next `\n`, or
/// `input.len()` if none).
fn logical_line_end(input: &[char], cursor: usize) -> usize {
    let mut i = cursor.min(input.len());
    while i < input.len() && input[i] != '\n' {
        i += 1;
    }
    i
}

/// Visual-row builder for scrollback lines.  Split `line` into
/// chunks of at most `cols` chars, push each as a `VRow`.  The
/// VRow type is private to `paint`; we re-declare a parallel
/// structure here because Rust closures can't carry it.
fn wrap_into_rows(
    line: &str,
    cols: usize,
    prompt_head: Option<&'static str>,
    caret_at: Option<usize>,
    out: &mut Vec<VRowOpaque>,
) {
    if line.is_empty() {
        out.push(VRowOpaque {
            text: String::new(),
            prompt_head,
            caret_col: if prompt_head.is_some() { caret_at } else { None },
        });
        return;
    }
    let mut col = 0usize;
    let mut start = 0usize;
    let mut row_start_char = 0usize; // absolute char index of row's first char
    let mut char_count = 0usize;
    let mut head = prompt_head;
    for (i, _ch) in line.char_indices() {
        if col == cols {
            let caret_col = caret_at.and_then(|abs| {
                if abs >= row_start_char && abs < row_start_char + col {
                    Some(abs - row_start_char)
                } else {
                    None
                }
            });
            out.push(VRowOpaque {
                text: line[start..i].to_string(),
                prompt_head: head,
                caret_col,
            });
            head = None;
            row_start_char += col;
            start = i;
            col = 0;
        }
        col += 1;
        char_count += 1;
    }
    if start < line.len() || (col == 0 && line.is_empty()) {
        let row_end_char = row_start_char + col;
        let caret_col = caret_at.and_then(|abs| {
            if abs >= row_start_char && abs <= row_end_char {
                Some(abs - row_start_char)
            } else {
                None
            }
        });
        out.push(VRowOpaque {
            text: line[start..].to_string(),
            prompt_head: head,
            caret_col,
        });
    }
    let _ = char_count;
}

/// Wrap one logical prompt + input line into one or more visual
/// rows.  The first sub-row gets `prompt_head = Some(prompt_str)`
/// so the painter knows to draw the amber prompt on the left.
/// `caret_abs` is measured from the start of the LOGICAL line
/// (including the prompt chars), so 0 = before the prompt and
/// `prompt_chars` = at the start of input.  Pass `None` to mark
/// "caret not in this logical line" (e.g. when the cursor is on
/// a different logical line of the multi-line input).
fn wrap_prompt_row(
    input: &str,
    prompt_str: &'static str,
    prompt_chars: usize,
    caret_abs: Option<usize>,
    cols: usize,
    out: &mut Vec<VRowOpaque>,
) {
    // The very first visual row holds `cols - prompt_chars` chars
    // of input.  Subsequent rows hold `cols` chars each.  If the
    // prompt itself is wider than a single row, things degrade
    // sensibly (whole input on row 2+).
    if cols <= prompt_chars {
        out.push(VRowOpaque {
            text: String::new(),
            prompt_head: Some(prompt_str),
            caret_col: caret_abs.filter(|&abs| abs <= prompt_chars),
        });
        // The input then wraps on subsequent rows, no head flag.
        wrap_into_rows(
            input,
            cols,
            None,
            caret_abs.and_then(|abs| caret_at_for_remainder(abs, prompt_chars)),
            out,
        );
        return;
    }
    let first_cap = cols - prompt_chars;
    let chars: Vec<char> = input.chars().collect();
    if chars.is_empty() {
        out.push(VRowOpaque {
            text: String::new(),
            prompt_head: Some(prompt_str),
            caret_col: caret_abs.map(|abs| abs.min(cols)),
        });
        return;
    }
    // First visual row: prompt + first_cap input chars.
    let head_end = first_cap.min(chars.len());
    let head_text: String = chars[..head_end].iter().collect();
    let head_caret_col = caret_abs.filter(|&abs| abs <= prompt_chars + head_end);
    out.push(VRowOpaque {
        text: head_text,
        prompt_head: Some(prompt_str),
        caret_col: head_caret_col,
    });
    if head_end == chars.len() {
        return;
    }
    // Remaining input wraps at `cols` per row.
    let rest: String = chars[head_end..].iter().collect();
    // The caret's absolute position is `prompt_chars + cursor_idx`.
    // For wrapping the remainder, we want the absolute char offset
    // within the REMAINDER, plus its visual offset (which is the
    // remainder's char index, since we don't have a prompt prefix
    // on cont rows).
    let rest_caret = caret_abs
        .filter(|&abs| abs > prompt_chars + head_end)
        .map(|abs| abs - prompt_chars - head_end);
    wrap_into_rows(&rest, cols, None, rest_caret, out);
}

/// Wrap the entire multi-line input buffer (split on `\n`) into
/// visual rows.  First logical line gets `PROMPT` on its head row;
/// subsequent logical lines get `CONT_PROMPT`.  `cursor_idx` is
/// the codepoint offset into the raw input buffer (including its
/// embedded `\n`s); we resolve which logical line owns the caret
/// and pass the right `caret_abs` down to that single sub-call.
fn wrap_prompt_block(
    input: &str,
    cursor_idx: usize,
    prompt_chars: usize,
    cols: usize,
    out: &mut Vec<VRowOpaque>,
) {
    // Walk by chars, splitting on '\n', tracking each logical
    // line's starting char index so we can place the caret.
    let chars: Vec<char> = input.chars().collect();
    if chars.is_empty() {
        // Single empty prompt row with caret at column prompt_chars.
        wrap_prompt_row("", PROMPT, prompt_chars, Some(prompt_chars), cols, out);
        return;
    }
    let mut logical_idx = 0usize;
    let mut line_start = 0usize;
    let mut i = 0usize;
    while i <= chars.len() {
        if i == chars.len() || chars[i] == '\n' {
            let line: String = chars[line_start..i].iter().collect();
            let prompt = if logical_idx == 0 { PROMPT } else { CONT_PROMPT };
            // Caret belongs to this logical line iff
            // line_start <= cursor_idx <= i.  Equality with `i`
            // (i.e. just before the '\n') puts the caret at the
            // end of THIS line, not the start of the next.
            let caret_abs = if cursor_idx >= line_start && cursor_idx <= i {
                Some(prompt_chars + (cursor_idx - line_start))
            } else {
                None
            };
            wrap_prompt_row(&line, prompt, prompt_chars, caret_abs, cols, out);
            logical_idx += 1;
            line_start = i + 1;
        }
        i += 1;
    }
}

fn caret_at_for_remainder(caret_abs: usize, prompt_chars: usize) -> Option<usize> {
    if caret_abs > prompt_chars {
        Some(caret_abs - prompt_chars)
    } else {
        None
    }
}

/// Opaque visual-row mirror of the private `VRow` inside `paint`.
/// Public-to-the-module so the helpers above can push into the
/// same Vec the paint loop reads.
///
/// `prompt_head`:
///   - `None` — pure scrollback or input continuation (after
///     wrap) row.  No prompt prefix is drawn; text starts at the
///     left margin.
///   - `Some(s)` — head row for a logical input line.  The
///     painter draws `s` in amber at the left margin (currently
///     `PROMPT` for the first logical line of the input and
///     `CONT_PROMPT` for subsequent logical lines).
struct VRowOpaque {
    text: String,
    prompt_head: Option<&'static str>,
    caret_col: Option<usize>,
}

fn paint_caret(
    target: &ID2D1HwndRenderTarget,
    brush: Option<&ID2D1SolidColorBrush>,
    x: f32,
    y: f32,
    h: f32,
) {
    let Some(brush) = brush else { return; };
    let r = D2D_RECT_F {
        left: x,
        top: y + 1.0,
        right: x + 1.6,
        bottom: y + h - 1.0,
    };
    unsafe { target.FillRectangle(&r, brush) };
}

fn make_brush(
    target: &ID2D1HwndRenderTarget,
    r: f32,
    g: f32,
    b: f32,
    a: f32,
) -> Option<ID2D1SolidColorBrush> {
    let color = D2D1_COLOR_F { r, g, b, a };
    let props = D2D1_BRUSH_PROPERTIES {
        opacity: 1.0,
        transform: windows_numerics::Matrix3x2::identity(),
    };
    unsafe { target.CreateSolidColorBrush(&color, Some(&props)) }.ok()
}

fn draw_text(
    dw_factory: &windows::Win32::Graphics::DirectWrite::IDWriteFactory,
    target: &ID2D1HwndRenderTarget,
    format: &IDWriteTextFormat,
    text: &str,
    x: f32,
    y: f32,
    w: f32,
    h: f32,
    brush: Option<&ID2D1SolidColorBrush>,
) {
    if text.is_empty() {
        return;
    }
    let Some(brush) = brush else { return; };
    let wide: Vec<u16> = text.encode_utf16().collect();
    let layout: Result<IDWriteTextLayout, _> = unsafe {
        dw_factory.CreateTextLayout(&wide, format, w.max(1.0), h.max(1.0))
    };
    let Ok(layout) = layout else { return; };
    let origin = windows_numerics::Vector2 { X: x, Y: y };
    unsafe { target.DrawTextLayout(origin, &layout, brush, D2D1_DRAW_TEXT_OPTIONS_CLIP) };
}

// ─── WndProc ───────────────────────────────────────────────────────

unsafe extern "system" fn fconsole_wnd_proc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    if msg == WM_NCCREATE {
        let state = Box::new(ConsoleWindowState::new(hwnd));
        let raw = Box::into_raw(state) as isize;
        unsafe { SetWindowLongPtrW(hwnd, GWLP_USERDATA, raw) };
        *FCONSOLE_HWND.lock().expect("FCONSOLE_HWND poisoned") = Some(hwnd.0 as isize);
        return unsafe { DefMDIChildProcW(hwnd, msg, wparam, lparam) };
    }

    let state_ptr = unsafe { GetWindowLongPtrW(hwnd, GWLP_USERDATA) } as *mut ConsoleWindowState;
    if state_ptr.is_null() {
        return unsafe { DefMDIChildProcW(hwnd, msg, wparam, lparam) };
    }
    let state = unsafe { &mut *state_ptr };

    match msg {
        WM_NCDESTROY => {
            *FCONSOLE_HWND.lock().expect("FCONSOLE_HWND poisoned") = None;
            unsafe {
                SetWindowLongPtrW(hwnd, GWLP_USERDATA, 0);
                let _ = Box::from_raw(state_ptr);
            }
            unsafe { DefMDIChildProcW(hwnd, msg, wparam, lparam) }
        }
        WM_PAINT => {
            state.paint();
            unsafe { DefMDIChildProcW(hwnd, msg, wparam, lparam) }
        }
        WM_SIZE => {
            state.handle_resize();
            let _ = unsafe { InvalidateRect(Some(hwnd), None, false) };
            unsafe { DefMDIChildProcW(hwnd, msg, wparam, lparam) }
        }
        WM_CHAR => {
            let t = std::time::Instant::now();
            if let Some(c) = char::from_u32(wparam.0 as u32) {
                state.handle_char(c);
            }
            trace_now!("WM_CHAR", "ch={:#x} handled={}us",
                wparam.0 as u32, t.elapsed().as_micros());
            LRESULT(0)
        }
        WM_KEYDOWN => {
            let t = std::time::Instant::now();
            state.handle_key(wparam.0 as u32);
            trace_now!("WM_KEYDOWN", "vk={:#x} handled={}us",
                wparam.0 as u32, t.elapsed().as_micros());
            LRESULT(0)
        }
        WM_MOUSEWHEEL => {
            let delta = ((wparam.0 >> 16) as i16) as i16;
            state.handle_wheel(delta);
            LRESULT(0)
        }
        WM_LBUTTONDOWN => {
            let _ = unsafe { SetFocus(Some(hwnd)) };
            LRESULT(0)
        }
        WM_SETFOCUS => {
            let _ = unsafe { InvalidateRect(Some(hwnd), None, false) };
            unsafe { DefMDIChildProcW(hwnd, msg, wparam, lparam) }
        }
        WM_DPICHANGED_AFTERPARENT => {
            state.dpi = unsafe { GetDpiForWindow(hwnd) };
            if state.dpi == 0 {
                state.dpi = 96;
            }
            state.text_format = None;
            state.target = None;
            let _ = unsafe { InvalidateRect(Some(hwnd), None, false) };
            unsafe { DefMDIChildProcW(hwnd, msg, wparam, lparam) }
        }
        _ => unsafe { DefMDIChildProcW(hwnd, msg, wparam, lparam) },
    }
}
