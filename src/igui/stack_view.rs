//! stack_view — live data-stack viewer.
//!
//! View → Stack (Ctrl+Shift+K) opens a singleton MDI child that
//! shows the current Forth data stack, top-of-stack first.  Each
//! row carries:
//!
//!   index  | decimal              | hex                | ASCII / interp
//!   T      |              42      | 0x000000000000002A | '*'
//!   T-1    |        -1            | 0xFFFFFFFFFFFFFFFF | -1
//!   T-2    |       1234567890     | 0x00000000499602D2
//!   ...
//!
//! The worker thread publishes a fresh snapshot after every eval
//! by calling `publish(session.stack())`; we PostMessage the GUI
//! thread to repaint.  Read-only, scrollable, no input.

#![cfg(windows)]

use std::sync::Mutex;

use windows::core::{w, PCWSTR};
use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, RECT, WPARAM};
use windows::Win32::Graphics::Direct2D::Common::{
    D2D1_ALPHA_MODE_IGNORE, D2D1_COLOR_F, D2D1_PIXEL_FORMAT, D2D_SIZE_U,
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
use windows::Win32::UI::WindowsAndMessaging::{
    BringWindowToTop, DefMDIChildProcW, GetClientRect, GetWindowLongPtrW, IsWindow, LoadCursorW,
    PostMessageW, RegisterClassExW, SendMessageW, SetWindowLongPtrW, CW_USEDEFAULT,
    GWLP_USERDATA, IDC_ARROW, MDICREATESTRUCTW, WHEEL_DELTA, WM_DPICHANGED_AFTERPARENT,
    WM_MDIACTIVATE, WM_MDICREATE, WM_MOUSEWHEEL, WM_NCCREATE, WM_NCDESTROY, WM_PAINT, WM_SIZE,
    WNDCLASSEXW, WNDCLASS_STYLES, WS_OVERLAPPEDWINDOW, WS_VISIBLE,
};

use super::renderer;

pub const MENU_CMD_ID: u16 = 0x3005;

const CLASS_NAME: PCWSTR = w!("WF64.iGui.StackView");
const TITLE: PCWSTR = w!("\u{2234} stack");

/// Posted by `publish` after the worker stores a fresh snapshot.
/// Handled in the window's WndProc with InvalidateRect.
const WM_STACK_FLUSH: u32 = windows::Win32::UI::WindowsAndMessaging::WM_USER + 21;

static STACK_HWND: Mutex<Option<isize>> = Mutex::new(None);
static STACK_SNAPSHOT: Mutex<Vec<i64>> = Mutex::new(Vec::new());

/// Replace the stack snapshot.  Called by the worker after every
/// eval.  `cells` must be top-of-stack first (matches
/// `Wf64Session::stack()`).  Posts WM_STACK_FLUSH so the GUI
/// thread repaints (no-op if the window isn't open).
pub fn publish(cells: Vec<i64>) {
    if let Ok(mut s) = STACK_SNAPSHOT.lock() {
        *s = cells;
    }
    if let Some(raw) = STACK_HWND.lock().ok().and_then(|g| *g) {
        let hwnd = HWND(raw as *mut _);
        if unsafe { IsWindow(Some(hwnd)) }.as_bool() {
            let _ = unsafe {
                PostMessageW(Some(hwnd), WM_STACK_FLUSH, WPARAM(0), LPARAM(0))
            };
        }
    }
}

fn snapshot() -> Vec<i64> {
    STACK_SNAPSHOT.lock().map(|s| s.clone()).unwrap_or_default()
}

pub fn register_class() -> Result<(), super::IGuiError> {
    let h_instance = unsafe { GetModuleHandleW(None) }
        .map_err(|e| super::IGuiError::Win32(format!("GetModuleHandleW (stack): {e}")))?
        .into();
    let cursor = unsafe { LoadCursorW(None, IDC_ARROW) }
        .map_err(|e| super::IGuiError::Win32(format!("LoadCursorW (stack): {e}")))?;
    let cls = WNDCLASSEXW {
        cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
        style: WNDCLASS_STYLES(0),
        lpfnWndProc: Some(stack_wnd_proc),
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
    if let Some(raw) = *STACK_HWND.lock().expect("STACK_HWND poisoned") {
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
            eprintln!("[stack-view] GetModuleHandleW: {e}");
            return;
        }
    };
    let mut client_rect = RECT::default();
    let _ = unsafe { GetClientRect(mdi_client, &mut client_rect) };
    let w_full = (client_rect.right - client_rect.left).max(400);
    let h_full = (client_rect.bottom - client_rect.top).max(200);
    // Narrow + tall — the stack is a column, so the natural shape
    // mirrors that.  Pin to the right edge of the MDI client by
    // default; the user can drag it wherever.
    let width = 480_i32.min(w_full * 6 / 10);
    let height = (h_full * 7 / 10).max(280);
    let x = (w_full - width - 16).max(0);
    let y = 16;
    let create = MDICREATESTRUCTW {
        szClass: CLASS_NAME,
        szTitle: TITLE,
        hOwner: h_instance,
        x,
        y,
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
        eprintln!("[stack-view] WM_MDICREATE returned 0");
        let _ = CW_USEDEFAULT;
    }
}

// ─── Per-window state ─────────────────────────────────────────────

struct StackWindowState {
    hwnd: HWND,
    target: Option<ID2D1HwndRenderTarget>,
    text_format: Option<IDWriteTextFormat>,
    text_format_bold: Option<IDWriteTextFormat>,
    cell_w: f32,
    cell_h: f32,
    scroll_offset: usize,
    client_w: u32,
    client_h: u32,
    dpi: u32,
}

impl StackWindowState {
    fn new(hwnd: HWND) -> Self {
        let dpi = unsafe { GetDpiForWindow(hwnd) };
        let dpi = if dpi == 0 { 96 } else { dpi };
        Self {
            hwnd,
            target: None,
            text_format: None,
            text_format_bold: None,
            cell_w: 8.0,
            cell_h: 18.0,
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
            Err(e) => eprintln!("[stack-view] CreateHwndRenderTarget: {e}"),
        }
    }

    fn ensure_text_format(&mut self) {
        if self.text_format.is_some() && self.text_format_bold.is_some() {
            return;
        }
        let dw_factory = &renderer::ctx().dwrite.factory;
        let scale = self.dpi as f32 / 96.0;
        let font_size = 13.0_f32 * scale;
        let make = |weight: u32| unsafe {
            dw_factory.CreateTextFormat(
                w!("Cascadia Mono"),
                None,
                DWRITE_FONT_WEIGHT(weight as i32),
                DWRITE_FONT_STYLE_NORMAL,
                DWRITE_FONT_STRETCH_NORMAL,
                font_size,
                w!("en-us"),
            )
        };
        if self.text_format.is_none() {
            match make(400) {
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
                Err(e) => eprintln!("[stack-view] CreateTextFormat (regular): {e}"),
            }
        }
        if self.text_format_bold.is_none() {
            match make(700) {
                Ok(fmt) => {
                    let _ = unsafe { fmt.SetWordWrapping(DWRITE_WORD_WRAPPING_NO_WRAP) };
                    self.text_format_bold = Some(fmt);
                }
                Err(e) => eprintln!("[stack-view] CreateTextFormat (bold): {e}"),
            }
        }
    }

    fn paint(&mut self) {
        self.ensure_target();
        self.ensure_text_format();
        let Some(target) = self.target.clone() else { return; };
        let Some(format) = self.text_format.clone() else { return; };
        let Some(format_bold) = self.text_format_bold.clone() else { return; };
        let dw_factory = &renderer::ctx().dwrite.factory;

        unsafe { target.BeginDraw() };
        unsafe {
            target.Clear(Some(&D2D1_COLOR_F {
                r: 0.060, g: 0.075, b: 0.110, a: 1.0, // deep navy, like fconsole
            }));
        }

        // Palette.
        let label    = make_brush(&target, 0.55, 0.62, 0.72, 1.0); // dim slate
        let tos      = make_brush(&target, 0.95, 0.70, 0.30, 1.0); // amber (TOS)
        let val      = make_brush(&target, 0.85, 0.90, 0.95, 1.0); // bright
        let hex      = make_brush(&target, 0.50, 0.78, 0.62, 1.0); // green
        let ascii    = make_brush(&target, 0.65, 0.55, 0.85, 1.0); // violet
        let depth_bg = make_brush(&target, 0.130, 0.150, 0.190, 1.0);
        let _ = depth_bg;

        let pad_x = 12.0_f32;
        let pad_y = 10.0_f32;
        let w = self.client_w as f32;
        let h = self.client_h as f32;
        let cell_h = self.cell_h;

        let cells = snapshot();

        // Header row: "depth: N".
        let header_text = if cells.is_empty() {
            "depth: 0  (stack empty)".to_string()
        } else {
            format!("depth: {}", cells.len())
        };
        draw_text(
            dw_factory, &target, &format_bold,
            &header_text,
            pad_x, pad_y,
            w - pad_x * 2.0, cell_h,
            label.as_ref(),
        );
        let body_top = pad_y + cell_h * 1.4;
        let visible_rows = (((h - body_top - pad_y) / cell_h).floor().max(0.0)) as usize;

        let total = cells.len();
        let bottom_idx = total.saturating_sub(self.scroll_offset);
        let top_idx = bottom_idx.saturating_sub(visible_rows);

        // Column layout: tag | decimal | hex | ascii
        // Tag is short: "T" for TOS, "-1", "-2", … for the rest
        // (i.e. T-1, T-2 with the T elided after the head row).
        let col_tag = pad_x;
        let col_dec = pad_x + self.cell_w * 4.0;
        let col_hex = pad_x + self.cell_w * 26.0;
        let col_asc = pad_x + self.cell_w * 47.0;

        for (row_screen, idx) in (top_idx..bottom_idx).enumerate() {
            let y = body_top + row_screen as f32 * cell_h;
            let v = cells[idx];
            let tag = if idx == 0 {
                "T".to_string()
            } else {
                format!("-{}", idx)
            };
            let dec = format!("{:>20}", v);
            let hex_s = format!("0x{:016X}", v as u64);
            let asc = ascii_preview(v);
            let (val_brush, tag_brush) = if idx == 0 {
                (tos.as_ref(), tos.as_ref())
            } else {
                (val.as_ref(), label.as_ref())
            };
            draw_text(dw_factory, &target, &format, &tag, col_tag, y,
                col_dec - col_tag, cell_h, tag_brush);
            draw_text(dw_factory, &target, &format, &dec, col_dec, y,
                col_hex - col_dec, cell_h, val_brush);
            draw_text(dw_factory, &target, &format, &hex_s, col_hex, y,
                col_asc - col_hex, cell_h, hex.as_ref());
            if !asc.is_empty() {
                draw_text(dw_factory, &target, &format, &asc, col_asc, y,
                    w - col_asc - pad_x, cell_h, ascii.as_ref());
            }
        }

        // Hint if scrolled past the deepest cell.
        if total > visible_rows && top_idx > 0 {
            let hint = format!("⋮  {} more below", top_idx);
            let y = body_top + (visible_rows.saturating_sub(1)) as f32 * cell_h;
            draw_text(
                dw_factory, &target, &format,
                &hint, col_tag, y + cell_h, w - pad_x * 2.0, cell_h,
                label.as_ref(),
            );
        }

        let _ = unsafe { target.EndDraw(None, None) };
    }

    fn handle_wheel(&mut self, delta: i16) {
        let steps = (delta as i32 / WHEEL_DELTA as i32).abs().max(1) as usize;
        if delta < 0 {
            self.scroll_offset = self.scroll_offset.saturating_add(steps * 3);
        } else {
            self.scroll_offset = self.scroll_offset.saturating_sub(steps * 3);
        }
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
                t.Resize(&D2D_SIZE_U { width: new_w, height: new_h })
            };
        }
    }
}

/// Render a small ASCII interpretation hint for a stack cell.
/// Empty if nothing readable.
fn ascii_preview(v: i64) -> String {
    // Treat low 8 bytes as little-endian characters; if all
    // printable ASCII, show them quoted.  Otherwise, single
    // character preview when v fits a printable byte.
    let bytes = (v as u64).to_le_bytes();
    let printable_run: String = bytes
        .iter()
        .take_while(|&&b| b != 0 && (0x20..0x7F).contains(&b))
        .map(|&b| b as char)
        .collect();
    if printable_run.len() >= 2 {
        return format!("\"{}\"", printable_run);
    }
    if (0x20..0x7F).contains(&(v as u64 as u8))
        && (v as u64) < 0x100
    {
        let ch = v as u8 as char;
        return format!("'{}'", ch);
    }
    String::new()
}

fn make_brush(
    target: &ID2D1HwndRenderTarget,
    r: f32, g: f32, b: f32, a: f32,
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
    text: &str, x: f32, y: f32, w: f32, h: f32,
    brush: Option<&ID2D1SolidColorBrush>,
) {
    if text.is_empty() { return; }
    let Some(brush) = brush else { return; };
    let wide: Vec<u16> = text.encode_utf16().collect();
    let layout: Result<IDWriteTextLayout, _> = unsafe {
        dw_factory.CreateTextLayout(&wide, format, w.max(1.0), h.max(1.0))
    };
    let Ok(layout) = layout else { return; };
    let origin = windows_numerics::Vector2 { X: x, Y: y };
    unsafe { target.DrawTextLayout(origin, &layout, brush, D2D1_DRAW_TEXT_OPTIONS_CLIP) };
}

unsafe extern "system" fn stack_wnd_proc(
    hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM,
) -> LRESULT {
    if msg == WM_NCCREATE {
        let state = Box::new(StackWindowState::new(hwnd));
        let raw = Box::into_raw(state) as isize;
        unsafe { SetWindowLongPtrW(hwnd, GWLP_USERDATA, raw) };
        *STACK_HWND.lock().expect("STACK_HWND poisoned") = Some(hwnd.0 as isize);
        return unsafe { DefMDIChildProcW(hwnd, msg, wparam, lparam) };
    }
    let state_ptr = unsafe { GetWindowLongPtrW(hwnd, GWLP_USERDATA) } as *mut StackWindowState;
    if state_ptr.is_null() {
        return unsafe { DefMDIChildProcW(hwnd, msg, wparam, lparam) };
    }
    let state = unsafe { &mut *state_ptr };
    match msg {
        WM_NCDESTROY => {
            *STACK_HWND.lock().expect("STACK_HWND poisoned") = None;
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
        WM_MOUSEWHEEL => {
            let delta = ((wparam.0 >> 16) as i16) as i16;
            state.handle_wheel(delta);
            LRESULT(0)
        }
        m if m == WM_STACK_FLUSH => {
            let _ = unsafe { InvalidateRect(Some(hwnd), None, false) };
            LRESULT(0)
        }
        WM_DPICHANGED_AFTERPARENT => {
            state.dpi = unsafe { GetDpiForWindow(hwnd) };
            if state.dpi == 0 { state.dpi = 96; }
            state.text_format = None;
            state.text_format_bold = None;
            state.target = None;
            let _ = unsafe { InvalidateRect(Some(hwnd), None, false) };
            unsafe { DefMDIChildProcW(hwnd, msg, wparam, lparam) }
        }
        _ => unsafe { DefMDIChildProcW(hwnd, msg, wparam, lparam) },
    }
}
