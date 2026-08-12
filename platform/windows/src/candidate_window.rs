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

const WNDCLASS_NAME: PCWSTR = windows::core::w!("ZtapCandidateWindowClass");

const PADDING: f32 = 8.0;
const ROW_HEIGHT: f32 = 24.0;
const PREEDIT_ROW_HEIGHT: f32 = 20.0;
const INDEX_COLUMN_WIDTH: f32 = 20.0;
const CANDIDATE_FONT_SIZE: f32 = 16.0;
const INDEX_FONT_SIZE: f32 = 12.0;
const PREEDIT_FONT_SIZE: f32 = 13.0;
const CORNER_RADIUS: f32 = 6.0;

pub struct CandidateWindow {
    hwnd: HWND,
    d2d_factory: ID2D1Factory,
    dwrite_factory: IDWriteFactory,
    render_target: RefCell<Option<ID2D1HwndRenderTarget>>,
    candidate_format: IDWriteTextFormat,
    index_format: IDWriteTextFormat,
    preedit_format: IDWriteTextFormat,

    display: RefCell<DisplayState>,
}

#[derive(Default, Clone)]
struct DisplayState {
    preedit: String,
    candidates: Vec<String>,
    highlighted: usize,
}

impl CandidateWindow {

    pub fn new() -> Result<Self> {

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
                windows::core::w!(""),
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

    fn create_window() -> Result<HWND> {
        use windows::Win32::System::LibraryLoader::GetModuleHandleW;

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

        unsafe {
            let _ = RegisterClassExW(&wc);
        }

        let hwnd = unsafe {
            CreateWindowExW(
                WS_EX_TOOLWINDOW | WS_EX_TOPMOST | WS_EX_NOACTIVATE,
                WNDCLASS_NAME,
                windows::core::w!("Ztap Candidates"),
                WS_POPUP | WS_BORDER,
                0,
                0,
                100,
                100,
                None,
                None,
                Some(hinstance.into()),
                None,
            )?
        };

        Ok(hwnd)
    }

    pub fn show(&self, preedit: &str, candidates: &[String], highlighted: usize, screen_x: i32, screen_y: i32) -> Result<()> {
        {
            let mut display = self.display.borrow_mut();
            display.preedit = preedit.to_string();
            display.candidates = candidates.to_vec();
            display.highlighted = highlighted;
        }

        let (width, height) = self.measure()?;

        let x = screen_x.max(0);
        let y = screen_y.max(0);

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

    pub fn hide(&self) {

        unsafe {
            let _ = ShowWindow(self.hwnd, SW_HIDE);
        }
    }

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

    fn ensure_render_target(&self) -> Result<ID2D1HwndRenderTarget> {
        if let Some(rt) = self.render_target.borrow().as_ref() {
            return Ok(rt.clone());
        }

        let mut client_rect = RECT::default();

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
            dpiX: 0.0,
            dpiY: 0.0,
            usage: Default::default(),
            minLevel: Default::default(),
        };
        let hwnd_props = D2D1_HWND_RENDER_TARGET_PROPERTIES {
            hwnd: self.hwnd,
            pixelSize: size,
            presentOptions: Default::default(),
        };

        let rt = unsafe { self.d2d_factory.CreateHwndRenderTarget(&rt_props, &hwnd_props)? };
        *self.render_target.borrow_mut() = Some(rt.clone());
        Ok(rt)
    }

    fn on_paint(&self) -> Result<()> {
        let rt = self.ensure_render_target()?;
        let display = self.display.borrow();

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

                let index_label = format!("{}", (i + 1) % 10);
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

            if let Err(e) = rt.EndDraw(None, None) {
                *self.render_target.borrow_mut() = None;
                return Err(e);
            }
        }

        Ok(())
    }
}

unsafe fn solid_brush(rt: &ID2D1HwndRenderTarget, r: f32, g: f32, b: f32, a: f32) -> Result<ID2D1SolidColorBrush> {
    let color = D2D1_COLOR_F { r, g, b, a };
    let props = D2D1_BRUSH_PROPERTIES {
        opacity: 1.0,
        transform: identity_matrix(),
    };
    rt.CreateSolidColorBrush(&color, Some(&props))
}

fn identity_matrix() -> D2D1_MATRIX_3X2_F {
    D2D1_MATRIX_3X2_F {
        matrix: [[1.0, 0.0], [0.0, 1.0], [0.0, 0.0]],
    }
}

extern "system" fn wndproc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    match msg {
        WM_PAINT => {

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

#[allow(dead_code)]
const _: fn(COLORREF) = |_| {};
