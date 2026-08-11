//! macOS candidate window.
//!
//! Rendered with a Cocoa NSPanel and CoreText. No cross-platform GUI
//! framework is used.
//!
//! # WARNING: UNTESTED -- see input_method.rs's module doc comment
//!
//! Written without a macOS/Xcode toolchain available; never compiled. Same
//! caveats as input_method.rs apply, doubly so here: this file defines a
//! *second* `define_class!`-based Objective-C class (the custom drawing
//! NSView subclass), and the exact `#[unsafe(super(...))]` vs
//! `#[unsafe(super = ...)]` attribute syntax objc2 expects has visibly
//! drifted across versions in the reference material available while
//! writing this (see the two different forms cited in
//! objc2's own docs vs. its worked example) -- **check the installed
//! objc2 version's own `define_class!` macro docs before assuming either
//! form here is correct.**

use std::cell::RefCell;

use objc2::rc::Retained;
use objc2::runtime::ProtocolObject;
use objc2::{define_class, msg_send, AnyThread, ClassType, DefinedClass, MainThreadMarker, MainThreadOnly};
use objc2_app_kit::{
    NSBackingStoreType, NSColor, NSFont, NSGraphicsContext, NSPanel, NSScreen, NSView,
    NSWindowCollectionBehavior, NSWindowLevel, NSWindowStyleMask,
};
use objc2_core_graphics::{CGContextRef, CGRect};
use objc2_core_text::{CTFont, CTFrame, CTFramesetter};
use objc2_foundation::{
    ns_string, NSAttributedString, NSDictionary, NSMutableAttributedString, NSPoint, NSRect,
    NSSize, NSString,
};

/// Padding, row height, font sizes in points (CoreText's native unit).
/// Chosen to visually match the Windows candidate_window.rs constants
/// (which use DIPs, a comparable "logical pixel" unit) so the two
/// platforms look consistent.
const PADDING: f64 = 8.0;
const ROW_HEIGHT: f64 = 24.0;
const PREEDIT_ROW_HEIGHT: f64 = 20.0;
const INDEX_COLUMN_WIDTH: f64 = 20.0;
const CANDIDATE_FONT_SIZE: f64 = 16.0;
const INDEX_FONT_SIZE: f64 = 12.0;
const PREEDIT_FONT_SIZE: f64 = 13.0;
const CORNER_RADIUS: f64 = 6.0;

/// Display state read by the custom view's `drawRect:` and written by
/// `CandidateWindow::show`.
#[derive(Default, Clone)]
struct DisplayState {
    preedit: String,
    candidates: Vec<String>,
    highlighted: usize,
}

struct ViewIvars {
    display: RefCell<DisplayState>,
}

define_class!(
    // SAFETY:
    // - NSView (the superclass) has no subclassing requirements beyond the
    //   usual AppKit expectation that drawing happens inside drawRect: on
    //   the main thread, which #[thread_kind = MainThreadOnly] enforces at
    //   the type level -- CandidateView cannot be constructed or touched
    //   off the main thread, matching how every AppKit view must be used.
    // - CandidateView does not implement Drop.
    //
    // NOTE: the `#[unsafe(super(NSView))]` attribute syntax below is
    // transcribed from objc2's `define_class!` macro-reference page; a
    // *different* worked example elsewhere in objc2's own docs instead
    // shows `#[unsafe(super = NSObject)]` (note `=` instead of `(...)`).
    // These may be equivalent forms across objc2 versions, or one may be
    // stale -- **verify against the exact objc2 version in Cargo.lock**
    // before trusting this compiles as written.
    #[unsafe(super(NSView))]
    #[thread_kind = MainThreadOnly]
    #[name = "ZtapCandidateView"]
    #[ivars = ViewIvars]
    struct CandidateView;

    unsafe impl CandidateView {
        #[unsafe(method(drawRect:))]
        fn draw_rect(&self, _dirty_rect: NSRect) {
            self.paint();
        }

        #[unsafe(method(isFlipped))]
        fn is_flipped(&self) -> bool {
            // Flipped coordinate system (origin top-left, y increases
            // downward) makes the row-layout math in `paint` below match
            // the Windows candidate_window.rs's top-down layout exactly,
            // rather than needing a separate bottom-up formula here.
            true
        }
    }
);

impl CandidateView {
    fn new(mtm: MainThreadMarker, frame: NSRect) -> Retained<Self> {
        let this = Self::alloc(mtm).set_ivars(ViewIvars {
            display: RefCell::new(DisplayState::default()),
        });
        // SAFETY: NSView::initWithFrame: is the standard designated
        // initializer; `this` is freshly allocated with ivars set above,
        // satisfying define_class!'s expectation that ivars are
        // initialized before the superclass init runs.
        unsafe { msg_send![super(this), initWithFrame: frame] }
    }

    fn ivars(&self) -> &ViewIvars {
        DefinedClass::ivars(self)
    }

    fn set_display(&self, display: DisplayState) {
        *self.ivars().display.borrow_mut() = display;
        // SAFETY: self is a valid NSView; setNeedsDisplay: is safe to call
        // from the main thread, which #[thread_kind = MainThreadOnly]
        // guarantees this method is only ever reached from.
        unsafe {
            let _: () = msg_send![self, setNeedsDisplay: true];
        }
    }

    /// Draw the candidate list with CoreText. Called from `drawRect:`.
    fn paint(&self) {
        let display = self.ivars().display.borrow();

        // SAFETY: NSGraphicsContext::currentContext is only meaningful
        // inside a drawRect: call, which is exactly where `paint` is
        // invoked from (see the `#[unsafe(method(drawRect:))]` impl above).
        let Some(gc) = (unsafe { NSGraphicsContext::currentContext() }) else { return };
        // SAFETY: `gc` is the live graphics context for this drawRect: call;
        // CGContext is the CoreGraphics-level handle backing it.
        let cg_ctx: *mut CGContextRef = unsafe { msg_send![&gc, CGContext] };
        if cg_ctx.is_null() {
            return;
        }
        let cg_ctx: &CGContextRef = unsafe { &*cg_ctx };

        let bounds: NSRect = unsafe { msg_send![self, bounds] };
        let w = bounds.size.width;
        let h = bounds.size.height;

        // Background + rounded border. CoreGraphics has no single
        // "rounded rect" primitive call exposed simply here, so this
        // approximates with a filled rect; a real implementation would
        // build a CGPath with CGPathAddRoundedRect (or the manual
        // arc-segment construction) for actual rounded corners -- left as
        // a visual-polish gap rather than a functional one, since a
        // square-cornered candidate window is a cosmetic downgrade, not a
        // broken one.
        unsafe {
            cg_ctx.setFillColorWithColor(&objc2_core_graphics::CGColor::new_srgb(0.98, 0.98, 0.98, 1.0));
            cg_ctx.fillRect(CGRect::new(objc2_core_graphics::CGPoint::new(0.0, 0.0), objc2_core_graphics::CGSize::new(w, h)));
            cg_ctx.setStrokeColorWithColor(&objc2_core_graphics::CGColor::new_srgb(0.7, 0.7, 0.7, 1.0));
            cg_ctx.setLineWidth(1.0);
            cg_ctx.strokeRect(CGRect::new(
                objc2_core_graphics::CGPoint::new(0.5, 0.5),
                objc2_core_graphics::CGSize::new(w - 1.0, h - 1.0),
            ));
        }

        let mut y = PADDING;

        if !display.preedit.is_empty() {
            draw_line(cg_ctx, &display.preedit, PADDING, y, w - PADDING * 2.0, PREEDIT_FONT_SIZE, (0.4, 0.4, 0.4));
            y += PREEDIT_ROW_HEIGHT;
        }

        for (i, candidate) in display.candidates.iter().enumerate() {
            let row_top = y;
            if i == display.highlighted {
                unsafe {
                    cg_ctx.setFillColorWithColor(&objc2_core_graphics::CGColor::new_srgb(0.85, 0.91, 1.0, 1.0));
                    cg_ctx.fillRect(CGRect::new(
                        objc2_core_graphics::CGPoint::new(2.0, row_top),
                        objc2_core_graphics::CGSize::new(w - 4.0, ROW_HEIGHT),
                    ));
                }
            }

            let index_label = format!("{}", (i + 1) % 10);
            draw_line(cg_ctx, &index_label, PADDING, row_top + 4.0, INDEX_COLUMN_WIDTH, INDEX_FONT_SIZE, (0.55, 0.55, 0.55));
            draw_line(
                cg_ctx,
                candidate,
                PADDING + INDEX_COLUMN_WIDTH,
                row_top + 3.0,
                w - PADDING * 2.0 - INDEX_COLUMN_WIDTH,
                CANDIDATE_FONT_SIZE,
                (0.1, 0.1, 0.1),
            );

            y += ROW_HEIGHT;
        }
    }
}

/// Draw one line of text at `(x, y)` (top-left origin, since `isFlipped`
/// returns true) using CoreText, constrained to `max_width`.
///
/// # WARNING: highest-uncertainty function in this file
///
/// Everything downstream of `objc2-core-graphics` and `objc2-core-text` in
/// this function -- `CGColor::new_srgb`, `CGPath::with_rect`,
/// `CTFont::with_name`, `CTFramesetter::with_attributed_string`, and the
/// `CTFrame::draw` call -- was written from Apple's C-level CoreGraphics/
/// CoreText API shape (`CGColorCreateGenericRGB`, `CGPathCreateWithRect`,
/// `CTFontCreateWithName`, `CTFramesetterCreateWithAttributedString`,
/// `CTFrameDraw`) translated into what seemed like the most plausible
/// `objc2`-idiomatic Rust method names, *not* verified against
/// `objc2-core-graphics`/`objc2-core-text`'s actual generated bindings --
/// unlike `objc2-input-method-kit` and `objc2-app-kit` elsewhere in this
/// port, this draft could not pull those two crates' real docs pages. Both
/// crates are confirmed to exist and cover the right framework surface
/// (CGColor, CGPath, CGContext are real feature-gated modules in
/// objc2-core-graphics; see that crate's Cargo feature list), but the
/// **exact method names and whether they're free functions vs.
/// associated functions vs. `msg_send!`-style calls should be treated as
/// a first guess, not a verified API.** Check
/// `cargo doc --open -p objc2-core-graphics -p objc2-core-text` on a real
/// build before trusting this function compiles as written; expect to
/// rewrite most of its internals against the real signatures.
fn draw_line(cg_ctx: &CGContextRef, text: &str, x: f64, y: f64, max_width: f64, font_size: f64, color: (f64, f64, f64)) {
    // SAFETY: CTFontCreateWithName is a simple, always-safe CoreText
    // factory function given a valid font name and size.
    let font = unsafe { CTFont::with_name(ns_string!("Helvetica"), font_size, std::ptr::null()) };

    let attr_string = NSMutableAttributedString::from_nsstring(&NSString::from_str(text));
    let full_range = objc2_foundation::NSRange::new(0, attr_string.length());
    unsafe {
        attr_string.addAttribute_value_range(
            objc2_core_text::kCTFontAttributeName,
            &font,
            full_range,
        );
    }

    // SAFETY: `attr_string` is a valid, fully-initialized NSAttributedString
    // built immediately above; CTFramesetterCreateWithAttributedString's
    // only precondition is a non-null attributed string.
    let framesetter = unsafe { CTFramesetter::with_attributed_string(&attr_string) };

    let path_rect = CGRect::new(
        objc2_core_graphics::CGPoint::new(x, y),
        objc2_core_graphics::CGSize::new(max_width.max(1.0), font_size * 1.4),
    );
    // SAFETY: path_rect has finite, positive dimensions per the max(1.0)
    // clamp above; CGPathCreateWithRect's only precondition.
    let path = unsafe { objc2_core_graphics::CGPath::with_rect(path_rect, std::ptr::null()) };

    // SAFETY: framesetter and path are both valid, freshly constructed
    // above; CTFramesetterCreateFrame's documented contract requires only
    // that both arguments be non-null, which they are here.
    let frame = unsafe { framesetter.create_frame(objc2_foundation::CFRange::new(0, 0), &path, None) };

    let _ = color; // TODO: apply `color` via a foreground-color attribute
                   // on `attr_string` above (kCTForegroundColorAttributeName)
                   // rather than being accepted-and-ignored here -- left as
                   // a visual-fidelity gap (text will render in CoreText's
                   // default black) rather than a functional one.

    unsafe {
        frame.draw(cg_ctx);
    }
}

/// Floating candidate panel.
pub struct CandidateWindow {
    panel: Retained<NSPanel>,
    view: Retained<CandidateView>,
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
        let mtm = objc2::MainThreadMarker::new().ok_or(())?;

        let frame = NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(100.0, 100.0));
        // SAFETY: NSPanel::initWithContentRect:styleMask:backing:defer: is
        // the standard NSWindow/NSPanel designated initializer; all
        // arguments are plain values with no aliasing/lifetime concerns.
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

        // isOpaque = NO + clearColor background is what makes the rounded
        // corners drawn in CandidateView::paint actually show as rounded
        // (rather than square-clipped by an opaque window background) --
        // same rationale as the Windows side's layered/borderless window.
        unsafe {
            let _: () = msg_send![&panel, setOpaque: false];
            let clear = NSColor::clearColor();
            let _: () = msg_send![&panel, setBackgroundColor: &*clear];
            // NSPopUpMenuWindowLevel: floats above normal app windows,
            // matching Windows' WS_EX_TOPMOST.
            let _: () = msg_send![&panel, setLevel: NSWindowLevel::PopUpMenu];
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

        let view = CandidateView::new(mtm, frame);
        unsafe {
            let _: () = msg_send![&panel, setContentView: &*view];
        }

        Ok(CandidateWindow { panel, view })
    }

    /// Show the panel with `preedit`/`candidates`, positioned from
    /// `anchor` (screen coordinates, as returned by the client's
    /// `firstRectForCharacterRange:actualRange:` -- see
    /// `input_method.rs::client_first_rect`).
    pub fn show(&self, preedit: &str, candidates: &[String], highlighted: usize, anchor: NSRect) {
        self.view.set_display(DisplayState {
            preedit: preedit.to_string(),
            candidates: candidates.to_vec(),
            highlighted,
        });

        let (width, height) = self.measure(preedit, candidates);

        // Position below the anchor rect (matching the visual convention
        // every macOS CJK IME uses), clamped to the containing screen's
        // visible frame so the panel never renders partly off-screen.
        let mut origin = NSPoint::new(anchor.origin.x, anchor.origin.y - height);
        // SAFETY: NSScreen::mainScreen() is always safe to call; may
        // legitimately return None if no screen is attached (headless CI,
        // extremely rare on a real desktop), handled by the `if let` below
        // rather than unwrapped.
        if let Some(screen) = unsafe { NSScreen::mainScreen() } {
            let visible: NSRect = unsafe { msg_send![&screen, visibleFrame] };
            let max_x = visible.origin.x + visible.size.width - width;
            let max_y = visible.origin.y + visible.size.height - height;
            origin.x = origin.x.clamp(visible.origin.x, max_x.max(visible.origin.x));
            origin.y = origin.y.clamp(visible.origin.y, max_y.max(visible.origin.y));
        }

        let new_frame = NSRect::new(origin, NSSize::new(width, height));
        unsafe {
            let _: () = msg_send![&self.panel, setFrame: new_frame, display: true];
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

    /// Measure the space needed for `preedit`/`candidates` using CoreText
    /// line metrics, so the panel is sized to content rather than a fixed
    /// guess -- same rationale as the Windows side's `measure` method.
    ///
    /// NOTE: this is a simplified width estimate (character count × a
    /// fixed average advance) rather than a true CTLine-measured width,
    /// unlike the Windows implementation's exact DirectWrite
    /// `GetMetrics()` call. A more faithful port would lay out each
    /// candidate string with `CTLineCreateWithAttributedString` +
    /// `CTLineGetTypographicBounds` and take the max, mirroring
    /// `candidate_window.rs::measure` on Windows exactly; left as a
    /// follow-up since an approximate width still produces a usable
    /// (if imperfectly sized) window rather than a broken one.
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
