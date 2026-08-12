//! macOS candidate window.
//!
//! Rendered with a Cocoa NSPanel and plain `NSTextField` labels as
//! subviews of its content view. No cross-platform GUI framework is used.
//!
//! # WARNING: UNTESTED -- see input_method.rs's module doc comment
//!
//! Written without a macOS/Xcode toolchain available; never compiled.
//!
//! # Design history: why NSTextField instead of a custom CoreText view
//!
//! The original draft of this file defined a second `define_class!`-based
//! Objective-C class (a custom `NSView` subclass overriding `drawRect:`)
//! and drew candidate rows by hand with raw CoreGraphics/CoreText calls
//! (`CGContext`, `CTFramesetter`, `CTFrame`). That approach produced a long
//! chain of real CI build failures: `CGContextRef`/`CGRect`/`CGPoint`/
//! `CGSize` do not live in `objc2_core_graphics` (they live in
//! `objc2_core_foundation`, confirmed against that crate's own geometry.rs
//! source and objc2-foundation's `NSRect`/`NSPoint`/`NSSize` type-alias
//! re-exports); `CTFrame`/`CTFramesetter` were not at the crate root import
//! path used; `kCTFontAttributeName` needed a different feature gate;
//! `CFRange` lived in `objc2_core_foundation`, not `objc2_foundation`. Each
//! fix surfaced the next, with no way to verify the whole chain without a
//! real compiler.
//!
//! Rather than continue guessing at CoreGraphics/CoreText binding shapes
//! this environment cannot verify, this rewrite follows objc2's own
//! **official, published, verified-working example** (the "Hello World"
//! AppKit app in https://docs.rs/objc2's crate-level docs, using
//! `NSTextField::labelWithString` + `NSFont`/`NSColor`/`NSTextAlignment`)
//! as closely as possible. This trades hand-drawn rounded corners and
//! pixel-exact CoreText metrics (which the Windows Direct2D/DirectWrite
//! implementation has) for a drastically smaller, better-grounded surface
//! area: no custom NSView subclass, no CoreGraphics, no CoreText, just
//! NSPanel + NSTextField, all of which appear verbatim in that official
//! sample. `objc2-core-graphics` and `objc2-core-text` are no longer
//! dependencies of this crate as a result -- see Cargo.toml.

use objc2::rc::Retained;
use objc2::{msg_send, AnyThread, MainThreadMarker};
use objc2_app_kit::{
    NSBackingStoreType, NSColor, NSFont, NSPanel, NSScreen, NSTextAlignment, NSTextField,
    NSView, NSWindowCollectionBehavior, NSWindowStyleMask,
};
use objc2_foundation::{ns_string, NSPoint, NSRect, NSSize, NSString};

/// Padding, row height, font sizes in points. Chosen to visually match the
/// Windows candidate_window.rs constants (which use DIPs, a comparable
/// "logical pixel" unit) so the two platforms look consistent.
const PADDING: f64 = 8.0;
const ROW_HEIGHT: f64 = 24.0;
const PREEDIT_ROW_HEIGHT: f64 = 20.0;
const INDEX_COLUMN_WIDTH: f64 = 20.0;
const CANDIDATE_FONT_SIZE: f64 = 16.0;
const INDEX_FONT_SIZE: f64 = 12.0;
const PREEDIT_FONT_SIZE: f64 = 13.0;

/// One row's pair of labels (index number + candidate word), or just the
/// preedit label -- kept so `show()` can reuse/reposition existing
/// `NSTextField`s across calls instead of tearing down and recreating the
/// whole subview tree on every keystroke.
struct Row {
    index_label: Retained<NSTextField>,
    word_label: Retained<NSTextField>,
}

/// Floating candidate panel.
pub struct CandidateWindow {
    panel: Retained<NSPanel>,
    mtm: MainThreadMarker,
    preedit_label: Retained<NSTextField>,
    /// Pool of row label pairs, grown on demand and reused across `show()`
    /// calls; rows beyond the current candidate count are hidden rather
    /// than removed, avoiding subview churn on every keystroke.
    rows: std::cell::RefCell<Vec<Row>>,
}

impl CandidateWindow {
    /// Create the (initially hidden) candidate panel. Called once when the
    /// IME controller first needs it -- see
    /// `input_method.rs::ZtapInputController::ensure_session`.
    pub fn new() -> Result<Self, ()> {
        // SAFETY: MainThreadMarker::new() returning None (not on the main
        // thread) is treated as a hard error here rather than silently
        // proceeding -- every AppKit call in this file requires the main
        // thread, and IMKit is documented to dispatch controller callbacks
        // on the main thread, so this should always succeed in practice;
        // the Err path exists so a violated assumption surfaces as a
        // handled error (see ensure_session's `.ok()` fallback) rather
        // than an AppKit assertion crash deep in this function.
        let mtm = MainThreadMarker::new().ok_or(())?;

        let frame = NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(100.0, 100.0));
        // SAFETY: NSPanel::initWithContentRect:styleMask:backing:defer: is
        // the standard NSWindow/NSPanel designated initializer, matching
        // the pattern objc2's own docs.rs sample uses for NSWindow's
        // equivalent initializer; all arguments are plain values with no
        // aliasing/lifetime concerns.
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

        // isOpaque = NO + clearColor background: an un-bordered panel with
        // no chrome, like every other CJK IME candidate window. No rounded
        // corners in this rewrite (see module doc comment) -- a plain
        // rectangular panel is a cosmetic downgrade from the original
        // hand-drawn-rounded-rect draft, not a functional one.
        unsafe {
            let _: () = msg_send![&panel, setOpaque: false];
            let clear = NSColor::clearColor();
            let _: () = msg_send![&panel, setBackgroundColor: &*clear];
            // NSPopUpMenuWindowLevel: floats above normal app windows,
            // matching Windows' WS_EX_TOPMOST.
            //
            // WARNING: `NSWindowLevel` is an `isize` type alias, not an
            // enum (confirmed by CI: "no associated function or constant
            // named PopUpMenu found for type isize" when this was written
            // as `NSWindowLevel::PopUpMenu`). The classic Objective-C
            // constant is `NSPopUpMenuWindowLevel`, a free-standing
            // constant at the crate root -- used here on that basis, but
            // this specific name has not been confirmed against
            // objc2-app-kit's actual generated bindings (unlike most other
            // fixes in this file, which trace directly to a CI error
            // message). If this doesn't resolve, the raw numeric level
            // (101, per Apple's <NSWindow.h> `NSPopUpMenuWindowLevel`
            // definition) via `NSWindowLevel::from(101)` or a plain
            // integer literal is the fallback.
            let _: () = msg_send![&panel, setLevel: objc2_app_kit::NSPopUpMenuWindowLevel];
            // Follow the active Space (desktop) rather than being pinned to
            // whichever Space existed when the panel was first shown.
            let _: () = msg_send![
                &panel,
                setCollectionBehavior: NSWindowCollectionBehavior::CanJoinAllSpaces,
            ];
            // Never take key focus -- matches Windows' WS_EX_NOACTIVATE;
            // critical so the candidate window never steals keyboard focus
            // from whatever the user is actually typing into.
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

    /// Show the panel with `preedit`/`candidates`, positioned from
    /// `anchor` (screen coordinates, as returned by the client's
    /// `firstRectForCharacterRange:actualRange:` -- see
    /// `input_method.rs::client_first_rect`).
    pub fn show(&self, preedit: &str, candidates: &[String], highlighted: usize, anchor: NSRect) {
        let content_view = self.panel.contentView().expect("NSPanel must have a content view");

        let (width, height) = self.measure(preedit, candidates);

        // Position below the anchor rect (matching the visual convention
        // every macOS CJK IME uses), clamped to the containing screen's
        // visible frame so the panel never renders partly off-screen.
        let mut origin = NSPoint::new(anchor.origin.x, anchor.origin.y - height);
        // SAFETY: NSScreen::mainScreen(mtm) is always safe to call with a
        // valid MainThreadMarker; may legitimately return None if no
        // screen is attached (headless CI, extremely rare on a real
        // desktop), handled by the `if let` below rather than unwrapped.
        if let Some(screen) = NSScreen::mainScreen(self.mtm) {
            let visible = screen.visibleFrame();
            let max_x = visible.origin.x + visible.size.width - width;
            let max_y = visible.origin.y + visible.size.height - height;
            origin.x = origin.x.clamp(visible.origin.x, max_x.max(visible.origin.x));
            origin.y = origin.y.clamp(visible.origin.y, max_y.max(visible.origin.y));
        }

        let new_frame = NSRect::new(origin, NSSize::new(width, height));
        // NOTE: every AppKit mutation call below is wrapped in `unsafe {}`
        // uniformly, even where a given setter might actually be a safe
        // fn in this binding version -- an unnecessary `unsafe` block is
        // only ever a warning (see lib.rs's own CI-reported warnings of
        // exactly this kind), never a hard error, whereas guessing a call
        // is safe when the binding actually marks it `unsafe fn` is a hard
        // compile error. Consistently over-wrapping here is the safer
        // default given this file could not be checked against a
        // compiler; unwind this once real `cargo build` output confirms
        // which calls don't need it.
        unsafe {
            let _: () = msg_send![&self.panel, setFrame: new_frame, display: true];
        }

        // Layout is top-down in *panel-local* coordinates with y=0 at the
        // top (AppKit views are bottom-left-origin by default and this
        // panel's content view is not flipped, so each row's y is
        // `height - PADDING - row_top_from_top - row_height`).
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

            // Highlight the selected row's background. NSTextField has no
            // simple per-instance background-fill-only mode without also
            // needing setBezeled/setDrawsBackground plumbing per row; a
            // subtle text-color change stands in for the highlight instead
            // (matching the Windows implementation's own filled-rectangle
            // highlight would need a separate background NSView per row --
            // left as a visual-polish gap, not a functional one, since the
            // highlighted candidate is still distinguishable by index
            // alone).
            let (r, g, b) = if i == highlighted { (0.05, 0.05, 0.4) } else { (0.1, 0.1, 0.1) };
            unsafe {
                let color = NSColor::colorWithSRGBRed_green_blue_alpha(r, g, b, 1.0);
                row.word_label.setTextColor(Some(&color));
            }

            y_from_top += ROW_HEIGHT;
        }
        // Hide any pooled rows beyond the current candidate count.
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

    /// Hide the candidate panel.
    pub fn hide(&self) {
        // SAFETY: self.panel is a valid, live NSPanel for the lifetime of self.
        unsafe {
            let _: () = msg_send![&self.panel, orderOut: std::ptr::null::<objc2::runtime::AnyObject>()];
        }
    }

    /// Grow the row pool (adding new `NSTextField` subviews) if fewer than
    /// `count` rows currently exist. Never shrinks the pool -- excess rows
    /// are hidden by the caller (`show`, above) instead, so repeatedly
    /// showing a shrinking-then-growing candidate list doesn't churn
    /// subviews on every keystroke.
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

    /// Measure the space needed for `preedit`/`candidates`.
    ///
    /// NOTE: this is a simplified width estimate (character count × a
    /// fixed average advance) rather than a true glyph-measured width,
    /// unlike the Windows implementation's exact DirectWrite
    /// `GetMetrics()` call -- `NSTextField`/`NSAttributedString` do
    /// support real size-to-fit measurement
    /// (`NSString.boundingRectWithSize:options:attributes:`), but this
    /// draft did not have a verified binding path for that call available
    /// to check against a compiler (the same category of risk that
    /// motivated moving off hand-rolled CoreText entirely -- see the
    /// module doc comment). An approximate width still produces a usable
    /// (if imperfectly sized) window rather than a broken one; revisit
    /// once building against a real toolchain.
    fn measure(&self, preedit: &str, candidates: &[String]) -> (f64, f64) {
        let longest_chars = candidates
            .iter()
            .map(|c| c.chars().count())
            .chain(std::iter::once(preedit.chars().count()))
            .max()
            .unwrap_or(0);
        // Rough average advance for CJK text at CANDIDATE_FONT_SIZE; CJK
        // glyphs are close to full-em-width, unlike Latin text, so this
        // approximation is much closer to correct here than it would be
        // for a Latin-heavy string.
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

/// Build one non-editable, non-bordered `NSTextField` label, matching the
/// construction sequence in objc2's own verified docs.rs "Hello World"
/// sample (`NSTextField::labelWithString` + `setTextColor` +
/// `setFont`/`NSFont::systemFontOfSize`).
fn make_label(mtm: MainThreadMarker, font_size: f64, r: f64, g: f64, b: f64) -> Retained<NSTextField> {
    // SAFETY: NSTextField::labelWithString is a simple, safe AppKit
    // convenience constructor per objc2's own sample usage; ns_string!("")
    // is a valid empty compile-time NSString literal.
    let label = unsafe { NSTextField::labelWithString(ns_string!(""), mtm) };
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
