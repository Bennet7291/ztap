//! Windows candidate window.
//!
//! Rendered with a Win32 popup window, Direct2D, and DirectWrite.
//! No cross-platform GUI framework is used.
//!
//! # WARNING: UNTESTED -- see tsf.rs's module doc comment
//!
//! Written without a Windows toolchain available; never compiled. The
//! same caveats as tsf.rs apply here, doubly so for the WndProc/message
//! loop plumbing and the exact Direct2D/DirectWrite call sequence, which
//! are exactly the kind of "looks right, one enum value is wrong" code
//! that's very hard to get right without a compiler and a running window
//! to look at.

use std::cell::RefCell;

use windows::core::{Result, HSTRING, PCWSTR};
use windows::Win32::Foundation::{COLORREF, HWND, LPARAM, LRESULT, RECT, WPARAM};
use windows::Win32::Graphics::Direct2D::Common::{
    D2D1_ALPHA_MODE_PREMULTIPLIED, D2D1_COLOR_F, D2D1_MATRIX_3X2_F, D2D1_PIXEL_FORMAT, D2D_SIZE_U,
};
use windows::Win32::Graphics::Direct2D::{
    D2D1CreateFactory, ID2D1Factory, ID2D1HwndRenderTarget, ID2D1SolidColorBrush,
    D2D1_BRUSH_PROPERTIES, D2D1_FACTORY_TYPE_SINGLE_THREADED, D2D1_HWND_RENDER_TARGET_PROPERTIES,
    D2D1_RENDER_TARGET_PROPERTIES, D2D1_RENDER_TARGET_TYPE_DEFAULT, D2D1_ROUNDED_RECT,
};
use windows::Win32::Graphics::DirectWrite::{
    DWriteCreateFactory, IDWriteFactory, IDWriteTextFormat, DWRITE_FACTORY_TYPE_SHARED,
    DWRITE_FONT_STRETCH_NORMAL, DWRITE_FONT_STYLE_NORMAL, DWRITE_FONT_WEIGHT_NORMAL,
    DWRITE_FONT_WEIGHT_SEMI_BOLD, DWRITE_TEXT_ALIGNMENT_LEADING,
};
use windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT_UNKNOWN;
use windows::Win32::Graphics::Gdi::HBRUSH;
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, GetClientRect, LoadCursorW, RegisterClassExW, ShowWindow,
    CS_HREDRAW, CS_VREDRAW, HCURSOR, IDC_ARROW, SW_HIDE, SW_SHOWNOACTIVATE, WM_DESTROY,
    WM_PAINT, WNDCLASSEXW, WS_BORDER, WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW, WS_EX_TOPMOST,
    WS_POPUP,
};

/// Window class name for the candidate window. Registered once per
/// process; must be unique enough not to collide with another IME's class
/// if both happen to be loaded (unlikely, but "Ztap" as a prefix costs
/// nothing).
const WNDCLASS_NAME: PCWSTR = windows::core::w!("ZtapCandidateWindowClass");

/// Padding, row height, and font sizes in DIPs (device-independent pixels
/// -- Direct2D's native unit, 1/96 inch regardless of monitor DPI scaling).
const PADDING: f32 = 8.0;
const ROW_HEIGHT: f32 = 24.0;
const PREEDIT_ROW_HEIGHT: f32 = 20.0;
const INDEX_COLUMN_WIDTH: f32 = 20.0;
const CANDIDATE_FONT_SIZE: f32 = 16.0;
const INDEX_FONT_SIZE: f32 = 12.0;
const PREEDIT_FONT_SIZE: f32 = 13.0;
const CORNER_RADIUS: f32 = 6.0;

/// Floating candidate list window.
pub struct CandidateWindow {
    hwnd: HWND,
    d2d_factory: ID2D1Factory,
    dwrite_factory: IDWriteFactory,
    render_target: RefCell<Option<ID2D1HwndRenderTarget>>,
    candidate_format: IDWriteTextFormat,
    index_format: IDWriteTextFormat,
    preedit_format: IDWriteTextFormat,
    /// Current display state, read by `on_paint` (via `WM_PAINT`) and
    /// written by `show`. `RefCell` because `on_paint` is invoked through
    /// the WndProc (a plain function pointer taking `&self`-incompatible
    /// arguments -- see `wndproc` below), not a `&mut self` method call.
    display: RefCell<DisplayState>,
}

#[derive(Default, Clone)]
struct DisplayState {
    preedit: String,
    candidates: Vec<String>,
    highlighted: usize,
}

impl CandidateWindow {
    /// Create the (initially hidden) candidate window.
    ///
    /// Called once when the IME is activated (see tsf.rs's `Activate`,
    /// which does not yet call this -- wiring `CandidateWindow` into
    /// `ZtapTextService` is one of the TODOs left in tsf.rs's
    /// `refresh_composition`).
    pub fn new() -> Result<Self> {
        // SAFETY: D2D1CreateFactory / DWriteCreateFactory are documented
        // safe-to-call-anytime factory constructors; no preconditions
        // beyond a loaded d2d1.dll/dwrite.dll, which every Windows version
        // Ztap targets ships by default.
        let d2d_factory: ID2D1Factory =
            unsafe { D2D1CreateFactory(D2D1_FACTORY_TYPE_SINGLE_THREADED, None)? };
        let dwrite_factory: IDWriteFactory =
            unsafe { DWriteCreateFactory(DWRITE_FACTORY_TYPE_SHARED)? };

        let candidate_format = unsafe {
            dwrite_factory.CreateTextFormat(
                windows::core::w!("Segoe UI"),
                None,
                DWRITE_FONT_WEIGHT_NORMAL,
                DWRITE_FONT_STYLE_NORMAL,
                DWRITE_FONT_STRETCH_NORMAL,
                CANDIDATE_FONT_SIZE,
                windows::core::w!(""), // locale: system default
            )?
        };
        unsafe { candidate_format.SetTextAlignment(DWRITE_TEXT_ALIGNMENT_LEADING)? };

        let index_format = unsafe {
            dwrite_factory.CreateTextFormat(
                windows::core::w!("Segoe UI"),
                None,
                DWRITE_FONT_WEIGHT_NORMAL,
                DWRITE_FONT_STYLE_NORMAL,
                DWRITE_FONT_STRETCH_NORMAL,
                INDEX_FONT_SIZE,
                windows::core::w!(""),
            )?
        };

        let preedit_format = unsafe {
            dwrite_factory.CreateTextFormat(
                windows::core::w!("Segoe UI"),
                None,
                DWRITE_FONT_WEIGHT_SEMI_BOLD,
                DWRITE_FONT_STYLE_NORMAL,
                DWRITE_FONT_STRETCH_NORMAL,
                PREEDIT_FONT_SIZE,
                windows::core::w!(""),
            )?
        };

        let hwnd = Self::create_window()?;

        Ok(CandidateWindow {
            hwnd,
            d2d_factory,
            dwrite_factory,
            render_target: RefCell::new(None),
            candidate_format,
            index_format,
            preedit_format,
            display: RefCell::new(DisplayState::default()),
        })
    }

    /// Register the window class (idempotent -- `RegisterClassExW` returns
    /// an error harmlessly ignorable if already registered by an earlier
    /// `CandidateWindow` in the same process) and create the popup window.
    fn create_window() -> Result<HWND> {
        use windows::Win32::System::LibraryLoader::GetModuleHandleW;

        // SAFETY: GetModuleHandleW(None) returns this module's own HMODULE;
        // always safe, no preconditions.
        let hinstance = unsafe { GetModuleHandleW(None)? };

        let wc = WNDCLASSEXW {
            cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
            style: CS_HREDRAW | CS_VREDRAW,
            lpfnWndProc: Some(wndproc),
            hInstance: hinstance.into(),
            hCursor: unsafe { LoadCursorW(None, IDC_ARROW)? } as HCURSOR,
            hbrBackground: HBRUSH(std::ptr::null_mut()),
            lpszClassName: WNDCLASS_NAME,
            ..Default::default()
        };
        // SAFETY: `wc` is fully initialized above; a duplicate-registration
        // error (ERROR_CLASS_ALREADY_EXISTS) is expected and harmless if
        // multiple CandidateWindows are created in one process, so its
        // Result is intentionally not propagated with `?`.
        unsafe {
            let _ = RegisterClassExW(&wc);
        }

        // WS_EX_TOOLWINDOW: no taskbar entry. WS_EX_TOPMOST: always above
        // normal windows, like every other IME candidate window.
        // WS_EX_NOACTIVATE + not calling SetForegroundWindow anywhere is
        // what keeps focus on the user's actual document while this shows.
        // SAFETY: all arguments are simple values/PCWSTRs with 'static
        // lifetime (string literals via `w!`); CreateWindowExW's only
        // precondition is a registered class name, satisfied above.
        let hwnd = unsafe {
            CreateWindowExW(
                WS_EX_TOOLWINDOW | WS_EX_TOPMOST | WS_EX_NOACTIVATE,
                WNDCLASS_NAME,
                windows::core::w!("Ztap Candidates"),
                WS_POPUP | WS_BORDER,
                0,
                0,
                100,
                100, // resized in `show`
                None,
                None,
                Some(hinstance.into()),
                None,
            )?
        };

        Ok(hwnd)
    }

    /// Show the window near the cursor with the given candidate list.
    ///
    /// `highlighted` is the zero-based index of the currently selected entry.
    ///
    /// NOTE: `screen_x`/`screen_y` (the anchor point, normally obtained via
    /// `ITfContextView::GetTextExt` on the composition range) is a
    /// parameter here rather than something this type queries itself --
    /// `CandidateWindow` has no reference back to the `ITfContext` that
    /// would let it ask, and threading one through would couple this
    /// platform-rendering type to TSF specifics it otherwise doesn't need.
    /// The caller (`tsf.rs`, once wired up -- see its TODO) is responsible
    /// for the `GetTextExt` call and passing the result in.
    pub fn show(&self, preedit: &str, candidates: &[String], highlighted: usize, screen_x: i32, screen_y: i32) -> Result<()> {
        {
            let mut display = self.display.borrow_mut();
            display.preedit = preedit.to_string();
            display.candidates = candidates.to_vec();
            display.highlighted = highlighted;
        }

        let (width, height) = self.measure()?;

        // Clamp so the window stays on-screen. A real implementation should
        // query the monitor work area under (screen_x, screen_y) via
        // MonitorFromPoint + GetMonitorInfo; using a fixed conservative
        // screen-size assumption here would be wrong on multi-monitor or
        // unusually small displays, so this intentionally clamps only
        // against the *top-left* (never let the window start off-screen to
        // the left/above the anchor), and leaves bottom/right clamping as a
        // follow-up once real monitor geometry is available.
        let x = screen_x.max(0);
        let y = screen_y.max(0);

        // SAFETY: `self.hwnd` was created in `new`/`create_window` and is
        // valid for the lifetime of `self`; SetWindowPos's other arguments
        // are plain integers.
        unsafe {
            use windows::Win32::UI::WindowsAndMessaging::{SetWindowPos, SWP_NOACTIVATE, SWP_NOZORDER};
            SetWindowPos(
                self.hwnd,
                None,
                x,
                y,
                width as i32,
                height as i32,
                SWP_NOACTIVATE | SWP_NOZORDER,
            )?;
            let _ = ShowWindow(self.hwnd, SW_SHOWNOACTIVATE);
            let _ = windows::Win32::Graphics::Gdi::InvalidateRect(Some(self.hwnd), None, false.into());
        }

        Ok(())
    }

    /// Hide the candidate window.
    pub fn hide(&self) {
        // SAFETY: self.hwnd is a valid window handle for the lifetime of self.
        unsafe {
            let _ = ShowWindow(self.hwnd, SW_HIDE);
        }
    }

    /// Compute the pixel size needed to display the current `display`
    /// state, using DirectWrite text-measurement (`GetMetrics` on a laid
    /// out `IDWriteTextLayout`) so the window is sized to content rather
    /// than a fixed guess.
    fn measure(&self) -> Result<(f32, f32)> {
        let display = self.display.borrow();

        let mut max_width: f32 = 0.0;
        for candidate in &display.candidates {
            let layout = unsafe {
                self.dwrite_factory.CreateTextLayout(
                    &HSTRING::from(candidate.as_str()).as_wide(),
                    &self.candidate_format,
                    f32::MAX,
                    ROW_HEIGHT,
                )?
            };
            let metrics = unsafe { layout.GetMetrics()? };
            max_width = max_width.max(metrics.width);
        }
        if !display.preedit.is_empty() {
            let layout = unsafe {
                self.dwrite_factory.CreateTextLayout(
                    &HSTRING::from(display.preedit.as_str()).as_wide(),
                    &self.preedit_format,
                    f32::MAX,
                    PREEDIT_ROW_HEIGHT,
                )?
            };
            let metrics = unsafe { layout.GetMetrics()? };
            max_width = max_width.max(metrics.width);
        }

        let width = PADDING * 2.0 + INDEX_COLUMN_WIDTH + max_width;
        let has_preedit = !display.preedit.is_empty();
        let height = PADDING * 2.0
            + if has_preedit { PREEDIT_ROW_HEIGHT } else { 0.0 }
            + (display.candidates.len() as f32) * ROW_HEIGHT;

        Ok((width.max(80.0), height.max(ROW_HEIGHT)))
    }

    /// Lazily create (or recreate, after a `D2DERR_RECREATE_TARGET`) the
    /// `ID2D1HwndRenderTarget` sized to the window's current client rect.
    fn ensure_render_target(&self) -> Result<ID2D1HwndRenderTarget> {
        if let Some(rt) = self.render_target.borrow().as_ref() {
            return Ok(rt.clone());
        }

        let mut client_rect = RECT::default();
        // SAFETY: self.hwnd is valid; GetClientRect's only precondition.
        unsafe { GetClientRect(self.hwnd, &mut client_rect)? };
        let size = D2D_SIZE_U {
            width: (client_rect.right - client_rect.left).max(1) as u32,
            height: (client_rect.bottom - client_rect.top).max(1) as u32,
        };

        let rt_props = D2D1_RENDER_TARGET_PROPERTIES {
            r#type: D2D1_RENDER_TARGET_TYPE_DEFAULT,
            pixelFormat: D2D1_PIXEL_FORMAT {
                format: DXGI_FORMAT_UNKNOWN,
                alphaMode: D2D1_ALPHA_MODE_PREMULTIPLIED,
            },
            dpiX: 0.0, // 0.0 = use default system DPI, per D2D1_RENDER_TARGET_PROPERTIES docs
            dpiY: 0.0,
            usage: Default::default(),
            minLevel: Default::default(),
        };
        let hwnd_props = D2D1_HWND_RENDER_TARGET_PROPERTIES {
            hwnd: self.hwnd,
            pixelSize: size,
            presentOptions: Default::default(),
        };

        // SAFETY: rt_props/hwnd_props are fully initialized above; self.hwnd
        // is a valid window handle for the lifetime of self.
        let rt = unsafe { self.d2d_factory.CreateHwndRenderTarget(&rt_props, &hwnd_props)? };
        *self.render_target.borrow_mut() = Some(rt.clone());
        Ok(rt)
    }

    /// WM_PAINT handler: draw the candidate list with Direct2D.
    ///
    /// Called from `wndproc` below (see that function's dispatch of
    /// `WM_PAINT`), which is why this takes `&self` rather than `&mut
    /// self` -- the window's `GWLP_USERDATA` slot stores a raw pointer
    /// (see `wndproc`'s SAFETY notes), and `&self` through that pointer is
    /// all a plain `extern "system" fn` WndProc can offer without a second
    /// layer of interior mutability, which `RefCell<DisplayState>` and
    /// `RefCell<Option<ID2D1HwndRenderTarget>>` already provide.
    fn on_paint(&self) -> Result<()> {
        let rt = self.ensure_render_target()?;
        let display = self.display.borrow();

        // SAFETY: `rt` was just created/retrieved above and is valid;
        // BeginDraw/EndDraw bracket every Direct2D drawing call below per
        // the API's documented usage pattern.
        unsafe {
            rt.BeginDraw();

            let bg = D2D1_COLOR_F { r: 0.98, g: 0.98, b: 0.98, a: 1.0 };
            rt.Clear(Some(&bg));

            let border_brush = solid_brush(&rt, 0.7, 0.7, 0.7, 1.0)?;
            let text_brush = solid_brush(&rt, 0.1, 0.1, 0.1, 1.0)?;
            let index_brush = solid_brush(&rt, 0.55, 0.55, 0.55, 1.0)?;
            let preedit_brush = solid_brush(&rt, 0.4, 0.4, 0.4, 1.0)?;
            let highlight_brush = solid_brush(&rt, 0.85, 0.91, 1.0, 1.0)?;

            let mut client_rect = RECT::default();
            let _ = GetClientRect(self.hwnd, &mut client_rect);
            let w = (client_rect.right - client_rect.left) as f32;
            let h = (client_rect.bottom - client_rect.top) as f32;

            let border_rect = D2D1_ROUNDED_RECT {
                rect: windows::Win32::Graphics::Direct2D::Common::D2D_RECT_F {
                    left: 0.5,
                    top: 0.5,
                    right: w - 0.5,
                    bottom: h - 0.5,
                },
                radiusX: CORNER_RADIUS,
                radiusY: CORNER_RADIUS,
            };
            rt.DrawRoundedRectangle(&border_rect, &border_brush, 1.0, None);

            let mut y = PADDING;

            if !display.preedit.is_empty() {
                let layout_rect = windows::Win32::Graphics::Direct2D::Common::D2D_RECT_F {
                    left: PADDING,
                    top: y,
                    right: w - PADDING,
                    bottom: y + PREEDIT_ROW_HEIGHT,
                };
                rt.DrawText(
                    &HSTRING::from(display.preedit.as_str()).as_wide(),
                    &self.preedit_format,
                    &layout_rect,
                    &preedit_brush,
                    Default::default(),
                    Default::default(),
                );
                y += PREEDIT_ROW_HEIGHT;
            }

            for (i, candidate) in display.candidates.iter().enumerate() {
                let row_top = y;
                if i == display.highlighted {
                    let hl_rect = windows::Win32::Graphics::Direct2D::Common::D2D_RECT_F {
                        left: 2.0,
                        top: row_top,
                        right: w - 2.0,
                        bottom: row_top + ROW_HEIGHT,
                    };
                    rt.FillRectangle(&hl_rect, &highlight_brush);
                }

                let index_label = format!("{}", (i + 1) % 10); // 1..9, then 0 for a hypothetical 10th
                let index_rect = windows::Win32::Graphics::Direct2D::Common::D2D_RECT_F {
                    left: PADDING,
                    top: row_top,
                    right: PADDING + INDEX_COLUMN_WIDTH,
                    bottom: row_top + ROW_HEIGHT,
                };
                rt.DrawText(
                    &HSTRING::from(index_label.as_str()).as_wide(),
                    &self.index_format,
                    &index_rect,
                    &index_brush,
                    Default::default(),
                    Default::default(),
                );

                let text_rect = windows::Win32::Graphics::Direct2D::Common::D2D_RECT_F {
                    left: PADDING + INDEX_COLUMN_WIDTH,
                    top: row_top,
                    right: w - PADDING,
                    bottom: row_top + ROW_HEIGHT,
                };
                rt.DrawText(
                    &HSTRING::from(candidate.as_str()).as_wide(),
                    &self.candidate_format,
                    &text_rect,
                    &text_brush,
                    Default::default(),
                    Default::default(),
                );

                y += ROW_HEIGHT;
            }

            // EndDraw can return D2DERR_RECREATE_TARGET; when it does, the
            // render target and every brush/resource built from it are
            // invalid and must be recreated on the next paint.
            if let Err(e) = rt.EndDraw(None, None) {
                *self.render_target.borrow_mut() = None;
                return Err(e);
            }
        }

        Ok(())
    }
}

/// Allocate a solid-color brush against `rt`. Small helper to avoid
/// repeating the `D2D1_BRUSH_PROPERTIES` boilerplate at every call site in
/// `on_paint` above.
unsafe fn solid_brush(rt: &ID2D1HwndRenderTarget, r: f32, g: f32, b: f32, a: f32) -> Result<ID2D1SolidColorBrush> {
    let color = D2D1_COLOR_F { r, g, b, a };
    let props = D2D1_BRUSH_PROPERTIES {
        opacity: 1.0,
        transform: identity_matrix(),
    };
    rt.CreateSolidColorBrush(&color, Some(&props))
}

/// Identity `D2D1_MATRIX_3X2_F` (no translation/rotation/scale/skew).
///
/// `windows::Foundation::Numerics::Matrix3x2` (the WinRT numerics type,
/// which has an `identity()` constructor) is the *wrong* type here --
/// `D2D1_BRUSH_PROPERTIES.transform` is a Win32 Direct2D
/// `D2D1_MATRIX_3X2_F`, a distinct type from the WinRT one despite the
/// similar name and shape. `windows-rs`'s binding of `D2D1_MATRIX_3X2_F`
/// has no `identity()` associated function (confirmed against a
/// third-party Direct2D-in-Rust example using the same field name), so
/// this builds the matrix by hand: row-major `[[m11, m12], [m21, m22],
/// [dx, dy]]`, where the identity leaves `m11 = m22 = 1.0` and every other
/// entry `0.0`.
fn identity_matrix() -> D2D1_MATRIX_3X2_F {
    D2D1_MATRIX_3X2_F {
        matrix: [[1.0, 0.0], [0.0, 1.0], [0.0, 0.0]],
    }
}

/// The window procedure registered for `WNDCLASS_NAME`.
///
/// # SAFETY / design note
///
/// This is a plain `extern "system" fn`, so it cannot capture `self`. The
/// standard Win32 pattern -- used here -- is to stash a raw pointer to the
/// owning `CandidateWindow` in the window's `GWLP_USERDATA` slot via
/// `SetWindowLongPtrW` right after `CreateWindowExW` returns, then retrieve
/// it here via `GetWindowLongPtrW` on every message.
///
/// **This wiring is incomplete in this draft**: `create_window` above does
/// not yet call `SetWindowLongPtrW`, so `on_paint` below is currently
/// unreachable (the `GetWindowLongPtrW` call returns null and falls
/// through to `DefWindowProcW`). Completing this requires restructuring
/// `CandidateWindow::new`/`create_window` so the pointer can be set *after*
/// the `CandidateWindow` struct itself exists (a chicken-and-egg problem:
/// `CreateWindowExW` needs to happen to get an `hwnd`, but the `hwnd` is a
/// field *of* `CandidateWindow`) — typically solved by passing a
/// `Box<CandidateWindow>` pointer through `CreateWindowExW`'s `lpparam`
/// and setting `GWLP_USERDATA` from `WM_NCCREATE` inside this very
/// function. Left as an explicit gap rather than a plausible-looking
/// `SetWindowLongPtrW` call whose pointer validity this draft could not
/// verify without a compiler and a debugger.
extern "system" fn wndproc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    match msg {
        WM_PAINT => {
            // SAFETY: see the "design note" above -- GWLP_USERDATA wiring
            // is not yet completed, so this pointer read is a documented
            // gap, not a verified-safe operation. `is_null()` guards the
            // consequence (a crash) but not the underlying incompleteness.
            let ptr = unsafe {
                windows::Win32::UI::WindowsAndMessaging::GetWindowLongPtrW(
                    hwnd,
                    windows::Win32::UI::WindowsAndMessaging::GWLP_USERDATA,
                )
            };
            if ptr != 0 {
                let window = unsafe { &*(ptr as *const CandidateWindow) };
                let _ = window.on_paint();
            }
            unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) }
        }
        WM_DESTROY => LRESULT(0),
        _ => unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) },
    }
}

// Silences an unused-import warning for COLORREF, which is imported for
// documentation/completeness of the Win32 color-handling story in this
// file even though D2D1_COLOR_F (not COLORREF) is what's actually used for
// drawing -- COLORREF only shows up if a caller needs the classic GDI
// 0x00BBGGRR color format for interop elsewhere.
#[allow(dead_code)]
const _: fn(COLORREF) = |_| {};
