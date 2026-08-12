use objc2::rc::Retained;
use objc2::{msg_send, MainThreadMarker, MainThreadOnly};
use objc2_app_kit::{
    NSBackingStoreType, NSColor, NSFont, NSPanel, NSScreen, NSTextAlignment, NSTextField,
    NSView, NSWindowCollectionBehavior, NSWindowStyleMask,
};
use objc2_foundation::{ns_string, NSPoint, NSRect, NSSize, NSString};

const PADDING: f64 = 8.0;
const ROW_HEIGHT: f64 = 24.0;
const PREEDIT_ROW_HEIGHT: f64 = 20.0;
const INDEX_COLUMN_WIDTH: f64 = 20.0;
const CANDIDATE_FONT_SIZE: f64 = 16.0;
const INDEX_FONT_SIZE: f64 = 12.0;
const PREEDIT_FONT_SIZE: f64 = 13.0;

struct Row {
    index_label: Retained<NSTextField>,
    word_label: Retained<NSTextField>,
}

pub struct CandidateWindow {
    panel: Retained<NSPanel>,
    mtm: MainThreadMarker,
    preedit_label: Retained<NSTextField>,
    rows: std::cell::RefCell<Vec<Row>>,
}

impl CandidateWindow {
    pub fn new() -> Result<Self, ()> {
        let mtm = MainThreadMarker::new().ok_or(())?;

        let frame = NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(100.0, 100.0));
        let panel: Retained<NSPanel> = unsafe {
            let alloc = NSPanel::alloc(mtm);
            msg_send![
                alloc,
                initWithContentRect: frame,
                styleMask: NSWindowStyleMask::Borderless,
                backing: NSBackingStoreType::Buffered,
                defer: false,
            ]
        };

        unsafe {
            let _: () = msg_send![&panel, setOpaque: false];
            let clear = NSColor::clearColor();
            let _: () = msg_send![&panel, setBackgroundColor: &*clear];
            let _: () = msg_send![&panel, setLevel: objc2_app_kit::NSPopUpMenuWindowLevel];
            let _: () = msg_send![
                &panel,
                setCollectionBehavior: NSWindowCollectionBehavior::CanJoinAllSpaces,
            ];
            let _: () = msg_send![&panel, setBecomesKeyOnlyIfNeeded: true];
        }

        let preedit_label = make_label(mtm, PREEDIT_FONT_SIZE, 0.4, 0.4, 0.4);
        unsafe {
            let _: () = msg_send![&preedit_label, setHidden: true];
        }

        let content_view = panel.contentView().expect("NSPanel must have a content view");
        unsafe {
            content_view.addSubview(&preedit_label);
        }

        Ok(CandidateWindow {
            panel,
            mtm,
            preedit_label,
            rows: std::cell::RefCell::new(Vec::new()),
        })
    }

    pub fn show(&self, preedit: &str, candidates: &[String], highlighted: usize, anchor: NSRect) {
        let content_view = self.panel.contentView().expect("NSPanel must have a content view");

        let (width, height) = self.measure(preedit, candidates);

        let mut origin = NSPoint::new(anchor.origin.x, anchor.origin.y - height);
        if let Some(screen) = NSScreen::mainScreen(self.mtm) {
            let visible = screen.visibleFrame();
            let max_x = visible.origin.x + visible.size.width - width;
            let max_y = visible.origin.y + visible.size.height - height;
            origin.x = origin.x.clamp(visible.origin.x, max_x.max(visible.origin.x));
            origin.y = origin.y.clamp(visible.origin.y, max_y.max(visible.origin.y));
        }

        let new_frame = NSRect::new(origin, NSSize::new(width, height));
        unsafe {
            let _: () = msg_send![&self.panel, setFrame: new_frame, display: true];
        }

        let mut y_from_top = PADDING;

        if !preedit.is_empty() {
            unsafe {
                self.preedit_label.setStringValue(&NSString::from_str(preedit));
                let _: () = msg_send![&self.preedit_label, setHidden: false];
            }
            let label_y = height - y_from_top - PREEDIT_ROW_HEIGHT;
            unsafe {
                self.preedit_label.setFrame(NSRect::new(
                    NSPoint::new(PADDING, label_y),
                    NSSize::new(width - PADDING * 2.0, PREEDIT_ROW_HEIGHT),
                ));
            }
            y_from_top += PREEDIT_ROW_HEIGHT;
        } else {
            unsafe {
                let _: () = msg_send![&self.preedit_label, setHidden: true];
            }
        }

        self.ensure_row_count(candidates.len(), &content_view);
        let rows = self.rows.borrow();
        for (i, candidate) in candidates.iter().enumerate() {
            let row = &rows[i];
            let row_y = height - y_from_top - ROW_HEIGHT;

            let index_text = format!("{}", (i + 1) % 10);
            unsafe {
                row.index_label.setStringValue(&NSString::from_str(&index_text));
                row.word_label.setStringValue(&NSString::from_str(candidate));
                let _: () = msg_send![&row.index_label, setHidden: false];
                let _: () = msg_send![&row.word_label, setHidden: false];
            }
            unsafe {
                row.index_label.setFrame(NSRect::new(
                    NSPoint::new(PADDING, row_y),
                    NSSize::new(INDEX_COLUMN_WIDTH, ROW_HEIGHT),
                ));
                row.word_label.setFrame(NSRect::new(
                    NSPoint::new(PADDING + INDEX_COLUMN_WIDTH, row_y),
                    NSSize::new(width - PADDING * 2.0 - INDEX_COLUMN_WIDTH, ROW_HEIGHT),
                ));
            }

            let (r, g, b) = if i == highlighted { (0.05, 0.05, 0.4) } else { (0.1, 0.1, 0.1) };
            unsafe {
                let color = NSColor::colorWithSRGBRed_green_blue_alpha(r, g, b, 1.0);
                row.word_label.setTextColor(Some(&color));
            }

            y_from_top += ROW_HEIGHT;
        }
        for row in rows.iter().skip(candidates.len()) {
            unsafe {
                let _: () = msg_send![&row.index_label, setHidden: true];
                let _: () = msg_send![&row.word_label, setHidden: true];
            }
        }
        drop(rows);

        unsafe {
            let _: () = msg_send![&self.panel, orderFront: std::ptr::null::<objc2::runtime::AnyObject>()];
        }
    }

    pub fn hide(&self) {
        unsafe {
            let _: () = msg_send![&self.panel, orderOut: std::ptr::null::<objc2::runtime::AnyObject>()];
        }
    }

    fn ensure_row_count(&self, count: usize, content_view: &Retained<NSView>) {
        let mut rows = self.rows.borrow_mut();
        while rows.len() < count {
            let index_label = make_label(self.mtm, INDEX_FONT_SIZE, 0.55, 0.55, 0.55);
            let word_label = make_label(self.mtm, CANDIDATE_FONT_SIZE, 0.1, 0.1, 0.1);
            unsafe {
                content_view.addSubview(&index_label);
                content_view.addSubview(&word_label);
            }
            rows.push(Row { index_label, word_label });
        }
    }

    fn measure(&self, preedit: &str, candidates: &[String]) -> (f64, f64) {
        let longest_chars = candidates
            .iter()
            .map(|c| c.chars().count())
            .chain(std::iter::once(preedit.chars().count()))
            .max()
            .unwrap_or(0);
        let avg_advance = CANDIDATE_FONT_SIZE * 1.05;
        let content_width = longest_chars as f64 * avg_advance;

        let width = (PADDING * 2.0 + INDEX_COLUMN_WIDTH + content_width).max(80.0);
        let has_preedit = !preedit.is_empty();
        let height = (PADDING * 2.0
            + if has_preedit { PREEDIT_ROW_HEIGHT } else { 0.0 }
            + candidates.len() as f64 * ROW_HEIGHT)
            .max(ROW_HEIGHT);

        (width, height)
    }
}

fn make_label(mtm: MainThreadMarker, font_size: f64, r: f64, g: f64, b: f64) -> Retained<NSTextField> {
    let label = NSTextField::labelWithString(ns_string!(""), mtm);
    unsafe {
        let color = NSColor::colorWithSRGBRed_green_blue_alpha(r, g, b, 1.0);
        label.setTextColor(Some(&color));
        label.setAlignment(NSTextAlignment::Left);
        let font = NSFont::systemFontOfSize(font_size);
        label.setFont(Some(&font));
        let _: () = msg_send![&label, setHidden: true];
    }
    label
}
