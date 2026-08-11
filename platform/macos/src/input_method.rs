//! macOS InputMethodKit integration.
//!
//! Responsibilities:
//!   - Implement the IMKInputController subclass (the IME's main controller)
//!   - Route keyboard events to the ztap-core input engine
//!   - Manage the marked text (preedit underline) lifecycle
//!   - Coordinate with the candidate window
//!
//! # WARNING: UNTESTED -- no macOS/Xcode toolchain available while writing this
//!
//! This file was written without access to a Mac, Xcode, or a working
//! rustc/cargo targeting aarch64-apple-darwin/x86_64-apple-darwin in the
//! authoring environment. It has never been compiled. The objc2 /
//! objc2-input-method-kit API shapes below are based on
//! https://docs.rs/objc2-input-method-kit/latest/ (crate version 0.3.2) and
//! https://docs.rs/objc2/latest/objc2/macro.define_class.html, not a
//! verified build. Before relying on this:
//!
//! 1. Run `cargo build -p ztap-macos --target aarch64-apple-darwin` on a Mac
//!    with Xcode installed and fix whatever the compiler disagrees with.
//!    `define_class!`'s exact macro syntax (attribute names, ivar
//!    declaration shape) is the single most likely thing to have drifted
//!    from what's written here -- it's a relatively fast-moving macro
//!    across objc2 versions.
//! 2. Test by building an actual `.app` bundle, installing it under
//!    `~/Library/Input Methods/`, logging out/in (or running
//!    `killall imklaunchagent`), and selecting Ztap from the input source
//!    menu. IMKit's activation lifecycle (one controller instance per
//!    client connection, server registration via Info.plist) has failure
//!    modes that only surface this way.
//! 3. Treat every `msg_send!` call as suspect until it's been run and
//!    confirmed against a live client app (TextEdit is the classic smoke
//!    test) -- see the WARNING on `client_set_marked_text`/
//!    `client_insert_text` below for why these specifically are risky.
//!
//! # Why subclass IMKInputController directly
//!
//! Apple's docs for `IMKInputController` describe two integration styles:
//! either subclass `IMKInputController` and override its methods directly,
//! or leave it unsubclassed and provide a separate delegate object.
//! Ztap subclasses directly (`ZtapInputController : IMKInputController`) --
//! this is what the vast majority of real-world open-source IMKit-based
//! IMEs (e.g. Rime's Squirrel) do, and it keeps all of Ztap's state (the
//! `ztap_core::InputSession`, the candidate window handle) as ordinary
//! Objective-C instance variables on one class rather than split across a
//! controller/delegate pair.
//!
//! # Key handling approach
//!
//! Of IMKServerInput's three key-delivery styles (keybinding-table,
//! `inputText:key:modifiers:client:`, or raw `NSEvent`s via
//! `handleEvent:client:`), Ztap implements
//! **`inputText:key:modifiers:client:`** -- it hands over the decoded key
//! code and modifier flags directly, without needing a keybinding
//! dictionary in Info.plist, and maps almost one-to-one onto the same
//! vkey-based routing `tsf.rs::on_key_down` already uses on Windows.

use std::cell::RefCell;

use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2::{define_class, msg_send, AnyThread, ClassType, DefinedClass};
use objc2_foundation::{NSArray, NSInteger, NSRange, NSRect, NSString, NSUInteger};
use objc2_input_method_kit::{IMKInputController, NSObjectIMKServerInput};

use ztap_core::{Dictionary, Entry, InputSession, LearningStore};

use crate::candidate_window::CandidateWindow;

/// macOS virtual key codes Ztap cares about (from `<Carbon/Events.h>` /
/// `<HIToolbox/Events.h>`'s well-known constants -- these are stable ABI,
/// not something objc2 wraps, so they're spelled out directly).
mod vk {
    pub const RETURN: NSInteger = 0x24;
    pub const DELETE: NSInteger = 0x33; // Backspace
    pub const ESCAPE: NSInteger = 0x35;
    pub const SPACE: NSInteger = 0x31;
    pub const PAGE_UP: NSInteger = 0x74;
    pub const PAGE_DOWN: NSInteger = 0x79;
}

/// `NSEventModifierFlagOption`-equivalent bit we care about, so a pinyin
/// letter typed with a modifier held (e.g. Cmd-A for select-all) is *not*
/// intercepted as IME input. Value matches `NSEventModifierFlagCommand`.
const NS_EVENT_MODIFIER_FLAG_COMMAND: NSUInteger = 1 << 20;
/// `NSEventModifierFlagControl`, same rationale as above.
const NS_EVENT_MODIFIER_FLAG_CONTROL: NSUInteger = 1 << 18;

/// Per-controller mutable state. Kept behind a `RefCell` ivar (see
/// `Ivars` below) rather than plain fields, matching every other
/// `define_class!`-based objc2 example -- Objective-C's `-init` pattern
/// doesn't give Rust a way to build `Self` in one shot the way a normal
/// struct literal would, so interior mutability initialized post-`alloc`
/// is the standard shape.
struct ControllerState {
    session: Option<InputSession>,
    candidate_window: Option<CandidateWindow>,
}

struct Ivars {
    state: RefCell<ControllerState>,
}

define_class!(
    // SAFETY:
    // - IMKInputController (the superclass) has no documented subclassing
    //   requirements beyond what IMKServer's initWithServer:delegate:client:
    //   expects at construction, which ZtapInputController::new below
    //   satisfies by delegating to the inherited initializer.
    // - ZtapInputController does not implement Drop; its RefCell<ControllerState>
    //   cleans up via ordinary Rust drop glue when the Objective-C object is
    //   deallocated, which is sound for the same reason any Rust struct's
    //   fields drop normally.
    #[unsafe(super(IMKInputController))]
    #[name = "ZtapInputController"]
    #[ivars = Ivars]
    struct ZtapInputController;

    // NSObjectIMKServerInput is an "informal protocol" (a category on
    // NSObject providing default no-op implementations) rather than a
    // formal `@protocol` -- see that trait's doc comment on
    // NSObjectIMKServerInput in objc2-input-method-kit. Overriding
    // `inputText:key:modifiers:client:` here is what actually replaces the
    // default no-op with Ztap's real key handling.
    unsafe impl NSObjectIMKServerInput for ZtapInputController {
        #[unsafe(method(inputText:key:modifiers:client:))]
        unsafe fn inputText_key_modifiers_client(
            &self,
            string: Option<&NSString>,
            key_code: NSInteger,
            flags: NSUInteger,
            sender: Option<&AnyObject>,
        ) -> bool {
            self.handle_key_event(string, key_code, flags, sender)
        }

        #[unsafe(method(commitComposition:))]
        unsafe fn commitComposition(&self, sender: Option<&AnyObject>) {
            // The client wants the composition ended immediately (e.g. the
            // user clicked elsewhere, or is switching input methods). Commit
            // whatever's pending rather than silently discarding it -- an
            // IME that drops in-progress input on focus loss is a poor
            // experience, and every reference IMKit sample commits here
            // rather than cancelling.
            let raw = {
                let mut state = self.ivars().state.borrow_mut();
                let Some(session) = state.session.as_mut() else { return };
                if session.preedit.is_empty() {
                    return;
                }
                let raw = session.preedit.clone();
                session.cancel();
                raw
            };
            self.commit_text(&raw, sender);
        }

        #[unsafe(method(candidates:))]
        unsafe fn candidates(&self, _sender: Option<&AnyObject>) -> Option<Retained<NSArray>> {
            // Only meaningful if Ztap used IMKCandidates (Apple's built-in
            // candidate-window class) instead of the custom CandidateWindow
            // in candidate_window.rs. Ztap draws its own candidate panel
            // (matching the "no cross-platform GUI framework, roll our own"
            // rule that governs candidate_window.rs on Windows too), so this
            // is never actually queried by IMKit in practice -- left as a
            // correctly-typed no-op rather than omitted, since
              // NSObjectIMKServerInput's default already covers "not used."
            None
        }
    }
);

impl ZtapInputController {
    /// Construct via the inherited `initWithServer:delegate:client:`
    /// (`IMKInputController`'s own initializer -- Ztap does not override
    /// `init`/`initWithServer:delegate:client:` itself, since there is
    /// nothing Ztap needs to customize about *construction*; all of
    /// Ztap's setup happens lazily on first keystroke, mirroring
    /// `tsf::ZtapTextService::new`'s "don't do real setup until activation"
    /// design on Windows -- see that type's doc comment for the shared
    /// rationale).
    ///
    /// IMKServer calls this (indirectly, via the Objective-C runtime's
    /// `+alloc` / `-initWithServer:delegate:client:` dispatch driven by the
    /// `IMKInputControllerClass` key in Info.plist -- see `lib.rs`) once
    /// per client connection.
    fn ivars(&self) -> &Ivars {
        DefinedClass::ivars(self)
    }

    /// Lazily create the `InputSession` (and its `Dictionary`/
    /// `LearningStore`) on first use, exactly mirroring
    /// `tsf::ZtapTextService::Activate`'s reasoning on Windows: real
    /// dictionary/disk work shouldn't happen at object-construction time
    /// (`init`), only once the controller is actually about to process
    /// input.
    fn ensure_session(&self) {
        let mut state = self.ivars().state.borrow_mut();
        if state.session.is_none() {
            let dict = Dictionary::load_builtin();
            let store_path = Self::learning_store_path();
            let store = LearningStore::load(store_path);
            state.session = Some(InputSession::new(dict, store));
        }
        if state.candidate_window.is_none() {
            // CandidateWindow::new() can fail if AppKit/NSPanel setup goes
            // wrong; degrade to "no visible candidate window" rather than
            // panicking the whole input controller, since typing (and
              // committing raw pinyin via Enter) still works without it.
            state.candidate_window = CandidateWindow::new().ok();
        }
    }

    /// Resolve `~/Library/Application Support/Ztap/user.dict` for the
    /// learning store.
    fn learning_store_path() -> std::path::PathBuf {
        // SAFETY: NSHomeDirectory() is a simple, always-safe Foundation
        // C function with no preconditions.
        let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".to_string());
        std::path::PathBuf::from(home)
            .join("Library")
            .join("Application Support")
            .join("Ztap")
            .join("user.dict")
    }

    /// Core key-routing logic, mirroring `tsf::ZtapTextService::on_key_down`
    /// on Windows almost one-to-one -- see that function's doc comment for
    /// the shared key-routing table this implements.
    unsafe fn handle_key_event(
        &self,
        string: Option<&NSString>,
        key_code: NSInteger,
        flags: NSUInteger,
        sender: Option<&AnyObject>,
    ) -> bool {
        self.ensure_session();

        // Let Cmd/Ctrl-modified keystrokes pass straight through -- Ztap
        // has no use for e.g. Cmd-A while composing, and intercepting it
        // would break basic app functionality (select-all, copy/paste,
        // etc.) while the IME happens to be active.
        if flags & (NS_EVENT_MODIFIER_FLAG_COMMAND | NS_EVENT_MODIFIER_FLAG_CONTROL) != 0 {
            return false;
        }

        let has_composition = {
            let state = self.ivars().state.borrow();
            state.session.as_ref().map(|s| !s.preedit.is_empty()).unwrap_or(false)
        };

        // a-z: the NSString `string` parameter carries the actual typed
        // character (already accounting for the current keyboard layout),
        // which is more correct than deriving a letter from `key_code`
        // (whose numeric values are keyboard-layout-dependent for letter
        // keys on some non-US layouts) -- this mirrors why Windows'
        // on_key_down instead uses the WM_KEYDOWN virtual-key code (which
        // *is* layout-independent on Windows): each platform's "give me the
        // logical key" primitive is different, and this uses whichever one
        // is actually correct for that platform.
        if let Some(s) = string {
            let rust_str = s.to_string();
            if rust_str.len() == 1 {
                if let Some(ch) = rust_str.chars().next() {
                    if ch.is_ascii_lowercase() {
                        let candidates = {
                            let mut state = self.ivars().state.borrow_mut();
                            let Some(session) = state.session.as_mut() else { return false };
                            session.push_char(ch)
                        };
                        self.refresh_composition(&candidates, sender);
                        return true;
                    }
                    if has_composition && ch.is_ascii_digit() && ch != '0' {
                        let idx = (ch as u8 - b'1') as usize;
                        let word = {
                            let mut state = self.ivars().state.borrow_mut();
                            let Some(session) = state.session.as_mut() else { return false };
                            session.select(idx)
                        };
                        if let Some(word) = word {
                            self.commit_text(&word, sender);
                        }
                        return true;
                    }
                }
            }
        }

        if key_code == vk::DELETE && has_composition {
            let (candidates, now_empty) = {
                let mut state = self.ivars().state.borrow_mut();
                let Some(session) = state.session.as_mut() else { return false };
                let candidates = session.pop_char();
                (candidates, session.preedit.is_empty())
            };
            if now_empty {
                self.clear_marked_text(sender);
            } else {
                self.refresh_composition(&candidates, sender);
            }
            return true;
        }

        if key_code == vk::SPACE && has_composition {
            let word = {
                let mut state = self.ivars().state.borrow_mut();
                let Some(session) = state.session.as_mut() else { return false };
                session.select(0)
            };
            if let Some(word) = word {
                self.commit_text(&word, sender);
            }
            return true;
        }

        if key_code == vk::RETURN && has_composition {
            let raw = {
                let mut state = self.ivars().state.borrow_mut();
                let Some(session) = state.session.as_mut() else { return false };
                let raw = session.preedit.clone();
                session.cancel();
                raw
            };
            self.commit_text(&raw, sender);
            return true;
        }

        if key_code == vk::ESCAPE && has_composition {
            {
                let mut state = self.ivars().state.borrow_mut();
                if let Some(session) = state.session.as_mut() {
                    session.cancel();
                }
            }
            self.clear_marked_text(sender);
            return true;
        }

        if (key_code == vk::PAGE_UP || key_code == vk::PAGE_DOWN) && has_composition {
            // TODO(candidate paging): see the identical TODO in
            // tsf::ZtapTextService::on_key_down on Windows -- ztap-core's
            // InputSession has no paging cursor yet. Left unconsumed here
            // for the same reason.
            return false;
        }

        // Punctuation: only with no active composition, same rule as Windows.
        if !has_composition {
            if let Some(s) = string {
                let rust_str = s.to_string();
                if rust_str.len() == 1 {
                    if let Some(ch) = rust_str.chars().next() {
                        if ch.is_ascii_punctuation() {
                            let mapped = {
                                let mut state = self.ivars().state.borrow_mut();
                                state.session.as_mut().and_then(|sess| sess.punct.map(ch))
                            };
                            if let Some(mapped) = mapped {
                                self.commit_text(&mapped, sender);
                                return true;
                            }
                        }
                    }
                }
            }
        }

        false
    }

    /// Update marked (preedit) text and refresh the candidate window.
    fn refresh_composition(&self, candidates: &[Entry], sender: Option<&AnyObject>) {
        let preedit = {
            let state = self.ivars().state.borrow();
            state.session.as_ref().map(|s| s.preedit.clone()).unwrap_or_default()
        };
        self.client_set_marked_text(&preedit, sender);

        // Show the candidate window near the composition's insertion point.
        // firstRectForCharacterRange:actualRange: (queried via
        // client_first_rect below) gives screen coordinates for this.
        let rect = self.client_first_rect(sender);
        let words: Vec<String> = candidates.iter().map(|e| e.word.clone()).collect();
        let state = self.ivars().state.borrow();
        if let Some(cw) = state.candidate_window.as_ref() {
            cw.show(&preedit, &words, 0, rect);
        }
    }

    /// Clear marked text and hide the candidate window (composition
    /// cancelled or emptied via backspace).
    fn clear_marked_text(&self, sender: Option<&AnyObject>) {
        self.client_set_marked_text("", sender);
        let state = self.ivars().state.borrow();
        if let Some(cw) = state.candidate_window.as_ref() {
            cw.hide();
        }
    }

    /// Commit `text` to the client and end the composition.
    fn commit_text(&self, text: &str, sender: Option<&AnyObject>) {
        self.client_insert_text(text, sender);
        let state = self.ivars().state.borrow();
        if let Some(cw) = state.candidate_window.as_ref() {
            cw.hide();
        }
    }

    /// Send `setMarkedText:selectionRange:replacementRange:` to the client.
    ///
    /// # WARNING: raw `msg_send!`, unverified selector/argument shape
    ///
    /// `IMKTextInput` (the protocol the client object conforms to) has no
    /// typed binding in `objc2-input-method-kit` as of the version this was
    /// written against -- the crate's own docs describe the client only as
    /// "an object that conforms to the IMKInputText and NSObject
    /// protocols," with no generated Rust trait for its methods (unlike
    /// `NSObjectIMKServerInput`, which *is* generated, for the
    /// controller-side methods). This call is therefore hand-written
    /// `msg_send!` against the selector name and argument types from
    /// Apple's Objective-C header (see the `IMKTextInput-Protocol.h`
    /// reference this was checked against), not verified by the Rust
    /// compiler's normal type-checking against a real binding. Double-check
    /// the selector spelling and NSRange-by-value ABI on a real build.
    fn client_set_marked_text(&self, text: &str, sender: Option<&AnyObject>) {
        let Some(client) = sender else { return };
        let ns_text = NSString::from_str(text);
        let len = ns_text.len();
        // Selection at the end of the marked text; replacementRange
        // {NSNotFound, 0} means "at the current insertion point" (mirrors
        // TF_ANCHOR_END + no explicit replacement range on the Windows side).
        let selection_range = NSRange::new(len, 0);
        let replacement_range = NSRange::new(objc2_foundation::NSNotFound as NSUInteger, 0);
        // SAFETY: `client` is the AnyObject IMKit handed this controller as
        // the `sender`/client parameter of inputText:key:modifiers:client:,
        // which is documented to conform to IMKTextInput; the selector and
        // argument encoding below are transcribed from Apple's
        // IMKTextInput-Protocol.h (`- (void)setMarkedText:(id)string
        // selectionRange:(NSRange)r replacementRange:(NSRange)r2`).
        unsafe {
            let _: () = msg_send![
                client,
                setMarkedText: &*ns_text,
                selectionRange: selection_range,
                replacementRange: replacement_range,
            ];
        }
    }

    /// Send `insertText:replacementRange:` to the client (final commit).
    /// Same WARNING as `client_set_marked_text` above applies.
    fn client_insert_text(&self, text: &str, sender: Option<&AnyObject>) {
        let Some(client) = sender else { return };
        let ns_text = NSString::from_str(text);
        let replacement_range = NSRange::new(objc2_foundation::NSNotFound as NSUInteger, 0);
        // SAFETY: see client_set_marked_text's SAFETY note.
        unsafe {
            let _: () = msg_send![
                client,
                insertText: &*ns_text,
                replacementRange: replacement_range,
            ];
        }
    }

    /// Query `firstRectForCharacterRange:actualRange:` on the client to
    /// find where (in screen coordinates) the candidate window should
    /// appear. Returns a zero-sized rect at the origin on failure (the
    /// candidate window's own clamping logic -- see candidate_window.rs --
    /// then falls back to a reasonable position rather than drawing at a
    /// garbage location).
    fn client_first_rect(&self, sender: Option<&AnyObject>) -> NSRect {
        let Some(client) = sender else { return NSRect::ZERO };
        let range = NSRange::new(objc2_foundation::NSNotFound as NSUInteger, 0);
        let mut actual_range = NSRange::new(0, 0);
        // SAFETY: same rationale as client_set_marked_text -- selector and
        // struct-return ABI transcribed from IMKTextInput-Protocol.h
        // (`- (NSRect)firstRectForCharacterRange:(NSRange)r
        // actualRange:(NSRange*)r2`). Struct-returning ObjC messages
        // occasionally need `msg_send!`'s alternate calling convention on
        // some architectures (the classic "big struct return" ABI wrinkle)
        // -- this is exactly the kind of thing that's very easy to get
        // subtly wrong without a compiler to check the generated encoding
        // against, and is flagged here as a second highest-risk spot in
        // this file alongside the two client_* text calls above.
        unsafe {
            msg_send![
                client,
                firstRectForCharacterRange: range,
                actualRange: &mut actual_range,
            ]
        }
    }
}

// Silence an unused-import warning for `sel`, which is referenced only
// through define_class!'s macro-generated code paths (the method
// attributes above expand using it internally) rather than a directly
// named call site in this file's own source text.
#[allow(unused_imports)]
use objc2::sel as _sel_marker;
