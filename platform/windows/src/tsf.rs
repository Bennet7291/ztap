//! Windows TSF (Text Services Framework) integration.
//!
//! Responsibilities:
//!   - Implement the ITfTextInputProcessor and related TSF COM interfaces
//!   - Route keyboard events to the ztap-core input engine
//!   - Manage the composition (preedit) lifecycle: start / update / commit / cancel
//!   - Coordinate with the candidate window for display
//!
//! # WARNING: UNTESTED -- no Windows toolchain available while writing this
//!
//! This file was written without access to a Windows machine, the Windows
//! SDK, or a working rustc/cargo in the authoring environment. It has
//! never been compiled. The windows-rs interface names, method
//! signatures, and #[implement] macro usage below are based on
//! windows-rs 0.62's published API documentation
//! (https://microsoft.github.io/windows-docs-rs/doc/windows/Win32/UI/TextServices/),
//! not a verified build. Before relying on this:
//!
//! 1. Run `cargo build -p ztap-windows` on a Windows machine with the
//!    Windows SDK installed and fix whatever the compiler disagrees with --
//!    COM vtable signatures are easy to get subtly wrong (Option<&T> vs
//!    &T, *mut vs *const, a wrapped vs. unwrapped return type) and
//!    only the compiler can catch that here.
//! 2. Test against a real TSF-enabled application (Notepad is the classic
//!    smoke test) -- TSF's activation/threading lifecycle has failure modes
//!    (edit-session re-entrancy, cicero deadlocks) that only show up at
//!    runtime.
//! 3. Treat every `unsafe` block as suspect until it's been run under a
//!    debugger at least once.
//!
//! # Threading & lifetime model
//!
//! TSF creates one ITfTextInputProcessor instance per thread manager
//! (ITfThreadMgr), and Windows may host several thread managers in the
//! same process (one per UI thread). ZtapTextService is deliberately
//! **not** Send/Sync -- it's used from a single STA thread, matching how
//! TSF text services are documented to run. ztap-core's InputSession
//! has no interior mutability or locking of its own because of this: a
//! ZtapTextService on thread A never touches the InputSession created
//! for thread B.
//!
//! # Edit sessions
//!
//! All document mutation (composition start/update/commit) in TSF must
//! happen inside an ITfEditSession::DoEditSession callback -- you cannot
//! call ITfInsertAtSelection::InsertTextAtSelection or touch a
//! composition directly from a key event handler. EditSessionImpl below
//! wraps a one-shot closure and implements ITfEditSession to satisfy this
//! requirement. Every closure passed to it is 'static and captures only
//! owned/cloned COM interface pointers (never a borrow of &self) -- COM
//! interface types are cheap AddRef'd handles, so cloning one before
//! building the closure is the correct pattern here, not a workaround.
//! Getting this wrong is the single most common source of E_FAIL /
//! deadlocks in hand-written TSF text services, so it's worth restating:
//! **no ITfRange/ITfComposition mutation outside an edit session, ever,
//! and no closure that reaches back into &self across the callback
//! boundary.**

use std::cell::RefCell;

use windows::core::{implement, Result, GUID, HRESULT};
use windows::Win32::Foundation::{E_FAIL, LPARAM, WPARAM};
use windows::Win32::UI::Input::KeyboardAndMouse::{
    VK_BACK, VK_ESCAPE, VK_NEXT, VK_PRIOR, VK_RETURN, VK_SPACE,
};
use windows::Win32::UI::TextServices::{
    ITfComposition, ITfCompositionSink, ITfCompositionSink_Impl, ITfContext,
    ITfContextComposition, ITfEditSession, ITfEditSession_Impl, ITfInsertAtSelection,
    ITfKeyEventSink, ITfKeyEventSink_Impl, ITfKeystrokeMgr, ITfRange, ITfTextInputProcessor,
    ITfTextInputProcessor_Impl, ITfThreadMgr, TF_ANCHOR_END, TF_ES_READWRITE, TF_ES_SYNC,
    INSERT_TEXT_AT_SELECTION_FLAGS,
};
// windows-core 0.62 moved BOOL here (it is no longer re-exported at
// windows::Win32::Foundation::BOOL) -- see Cargo.toml's fix-history note 2
// for why windows_core is a direct dependency of this crate at all.
// `Interface` (the trait providing `.cast::<T>()` on every COM interface
// type) also needs to be imported explicitly -- CI confirmed `.cast()`
// calls throughout this file don't resolve without it in scope, even
// though the method exists on the type (it's a trait method, and Rust
// requires the trait itself to be imported, not just implemented).
use windows_core::{Interface, BOOL, Ref};

use ztap_core::{Dictionary, Entry, InputSession, LearningStore};

/// The Class ID (CLSID) for the Ztap TSF text service.
///
/// Must exactly match the CLSID DllRegisterServer writes to the registry
/// (see lib.rs) and whatever CLSID any installer/.reg file references.
/// **Do not regenerate this value once shipped** -- TSF, Windows Update
/// servicing, and any registration script identify this text service by
/// this GUID for the product's lifetime. Generated once for this project;
/// it has no other prior meaning, treat it as opaque fixed data.
pub const CLSID_ZTAP_TEXT_SERVICE: GUID =
    GUID::from_u128(0x5a746170_0001_4000_8000_000000005a54);

/// Language profile GUID for Simplified Chinese pinyin input.
///
/// Distinct from the CLSID above -- TSF distinguishes "what code implements
/// this" (CLSID) from "which language profile did the user enable" (this
/// GUID); one text service DLL can expose several profiles, though Ztap
/// only exposes one. Also frozen once assigned, for the same reason as
/// CLSID_ZTAP_TEXT_SERVICE.
pub const GUID_ZTAP_PROFILE: GUID = GUID::from_u128(0x5a746170_0002_4000_8000_000000005a54);

/// LANGID for Chinese (Simplified, PRC) --
/// MAKELANGID(LANG_CHINESE, SUBLANG_CHINESE_SIMPLIFIED). Used when
/// registering the language profile so TSF offers Ztap only for zh-CN
/// input, not e.g. zh-TW.
pub const LANGID_ZH_CN: u16 = 0x0804;

/// State for one active composition.
struct CompositionState {
    composition: ITfComposition,
}

/// The main TSF text service object.
///
/// One instance is created per thread manager; it holds the core engine and
/// forwards system events to it.
///
/// # #[implement]
///
/// This attribute (from windows-rs's `implement` feature) generates the
/// COM vtables and IUnknown plumbing for every interface listed; method
/// bodies live in the `impl Xyz_Impl for ZtapTextService_Impl` blocks below
/// (windows-rs generates that _Impl wrapper type alongside
/// ZtapTextService itself -- see the crate's implement macro docs if
/// that naming looks unfamiliar).
///
/// ITfTextInputProcessor is the entry point TSF activates/deactivates.
/// ITfKeyEventSink receives keystrokes once registered via
/// ITfKeystrokeMgr::AdviseKeyEventSink. ITfCompositionSink is notified
/// if TSF force-terminates our composition (e.g. focus loss) -- we need to
/// know so we don't try to mutate a composition that no longer exists.
#[implement(ITfTextInputProcessor, ITfKeyEventSink, ITfCompositionSink)]
pub struct ZtapTextService {
    /// Interior-mutable engine state. RefCell, not a lock, because this
    /// object is only ever touched from the single STA thread TSF created
    /// it on -- see the module doc comment's threading section.
    state: RefCell<ServiceState>,
}

struct ServiceState {
    /// ITfThreadMgr this service is registered against; kept so
    /// Deactivate can unadvise the key event sink it advised in Activate.
    thread_mgr: Option<ITfThreadMgr>,
    /// Client ID assigned by ITfThreadMgr::Activate (passed to us as
    /// Activate's `tid` argument); required as the first argument to
    /// essentially every other TSF call this service makes.
    client_id: u32,
    /// Whether AdviseKeyEventSink succeeded, so Deactivate knows
    /// whether there's anything to unadvise.
    key_event_sink_advised: bool,
    /// The ztap-core engine. None until Dictionary/LearningStore
    /// finish loading in Activate -- see that method for why this isn't
    /// eagerly initialized in ZtapTextService::new.
    session: Option<InputSession>,
    /// The currently active composition, if the user has typed anything
    /// since the last commit/cancel.
    composition: Option<CompositionState>,
}

impl ZtapTextService {
    /// Create the (not-yet-activated) TSF text service.
    ///
    /// Called from the class factory's CreateInstance (see lib.rs's
    /// DllGetClassObject). Deliberately does *not* touch the dictionary,
    /// disk, or any TSF interface yet -- ITfTextInputProcessor::Activate
    /// is where real setup happens, matching how TSF expects text services
    /// to behave (a service can be instantiated without ever being
    /// activated, e.g. registered but not the user's selected profile).
    pub fn new() -> ZtapTextService {
        ZtapTextService {
            state: RefCell::new(ServiceState {
                thread_mgr: None,
                client_id: 0,
                key_event_sink_advised: false,
                session: None,
                composition: None,
            }),
        }
    }

    /// Resolve `%APPDATA%\Ztap\user.dict` for the learning store.
    ///
    /// Falls back to a relative path (not reliably persisted, but not a
    /// crash) if the AppData folder can't be resolved, rather than failing
    /// activation entirely -- a Ztap that forgets learned words across
    /// restarts is a degraded experience, not a broken one, and a hard
    /// failure here would take down IME input for the whole session.
    fn learning_store_path() -> std::path::PathBuf {
        use windows::Win32::Foundation::MAX_PATH;
        use windows::Win32::UI::Shell::{SHGetFolderPathW, CSIDL_APPDATA};

        let mut buf = [0u16; MAX_PATH as usize];
        // SAFETY: `buf` is sized to MAX_PATH per SHGetFolderPathW's
        // contract; NULL hwnd/hToken and flags=0 request the current user's
        // roaming AppData folder without prompting for creation.
        let result = unsafe { SHGetFolderPathW(None, CSIDL_APPDATA as i32, None, 0, &mut buf) };
        if result.is_err() {
            return std::path::PathBuf::from("ztap_user.dict");
        }
        let len = buf.iter().position(|&c| c == 0).unwrap_or(0);
        let appdata = String::from_utf16_lossy(&buf[..len]);
        std::path::PathBuf::from(appdata).join("Ztap").join("user.dict")
    }

    /// Handle a WM_KEYDOWN event; return true if the key was consumed by the IME.
    ///
    /// Key routing:
    /// - a-z          -> push_char, refresh candidates
    /// - Backspace    -> pop_char, refresh candidates
    /// - 1-9          -> select candidate at index (digit - 1)
    /// - Space        -> select candidate 0 (top pick)
    /// - Enter        -> commit raw preedit text as-is
    /// - Escape       -> cancel composition
    /// - Page Up/Down -> scroll candidate page (see the TODO below -- not
    ///                  wired up yet, ztap-core has no paging cursor)
    /// - Punctuation  -> map via PunctuationState and commit
    ///
    /// Takes the owning ITfContext because committing text or updating a
    /// composition both need it, and TSF's key event sink hands us the
    /// context anyway (see OnKeyDown below) -- no reason to re-fetch it
    /// from the thread manager mid-keystroke.
    pub fn on_key_down(&self, context: &ITfContext, vkey: u32) -> Result<bool> {
        let vk = vkey as u16;

        let has_composition = {
            let state = self.state.borrow();
            state.session.as_ref().map(|s| !s.preedit.is_empty()).unwrap_or(false)
        };

        if (b'a' as u16..=b'z' as u16).contains(&vk) {
            let candidates = {
                let mut state = self.state.borrow_mut();
                let Some(session) = state.session.as_mut() else { return Ok(false) };
                let ch = (vk as u8) as char;
                session.push_char(ch)
            };
            self.refresh_composition(context, &candidates)?;
            return Ok(true);
        }

        if vk == VK_BACK.0 && has_composition {
            let (candidates, now_empty) = {
                let mut state = self.state.borrow_mut();
                let Some(session) = state.session.as_mut() else { return Ok(false) };
                let candidates = session.pop_char();
                (candidates, session.preedit.is_empty())
            };
            if now_empty {
                self.end_composition(context)?;
            } else {
                self.refresh_composition(context, &candidates)?;
            }
            return Ok(true);
        }

        if (b'1' as u16..=b'9' as u16).contains(&vk) && has_composition {
            let idx = (vk - b'1' as u16) as usize;
            let word = {
                let mut state = self.state.borrow_mut();
                let Some(session) = state.session.as_mut() else { return Ok(false) };
                session.select(idx)
            };
            if let Some(word) = word {
                self.commit_text(context, &word)?;
            }
            return Ok(true);
        }

        if vk == VK_SPACE.0 && has_composition {
            let word = {
                let mut state = self.state.borrow_mut();
                let Some(session) = state.session.as_mut() else { return Ok(false) };
                session.select(0)
            };
            if let Some(word) = word {
                self.commit_text(context, &word)?;
            }
            return Ok(true);
        }

        if vk == VK_RETURN.0 && has_composition {
            // Commit the raw preedit as-is (typed pinyin, not converted to
            // characters) -- the standard Enter behavior for pinyin IMEs
            // when the user wants literal Latin text.
            let raw = {
                let mut state = self.state.borrow_mut();
                let Some(session) = state.session.as_mut() else { return Ok(false) };
                let raw = session.preedit.clone();
                session.cancel();
                raw
            };
            self.commit_text(context, &raw)?;
            return Ok(true);
        }

        if vk == VK_ESCAPE.0 && has_composition {
            {
                let mut state = self.state.borrow_mut();
                if let Some(session) = state.session.as_mut() {
                    session.cancel();
                }
            }
            self.end_composition(context)?;
            return Ok(true);
        }

        if (vk == VK_PRIOR.0 || vk == VK_NEXT.0) && has_composition {
            // TODO(candidate paging): ztap-core's InputSession::candidates()
            // currently returns only the top 9 ranked entries with no
            // paging cursor -- there is nothing here to page *into* yet.
            // Either extend InputSession with a page-offset parameter, or
            // have candidate_window.rs request a larger slice and paginate
            // client-side. Left unconsumed here (falls through to `false`)
            // rather than silently doing nothing, so Page Up/Down still
            // reaches the host app's normal scrolling when not mid-composition.
            return Ok(false);
        }

        // Punctuation mapping: only meaningful with no active pinyin
        // composition (the digit/space/enter/escape branches above already
        // handle all in-composition control keys).
        if !has_composition {
            if let Some(ch) = char::from_u32(vkey) {
                if ch.is_ascii_punctuation() {
                    let mapped = {
                        let mut state = self.state.borrow_mut();
                        state.session.as_mut().and_then(|s| s.punct.map(ch))
                    };
                    if let Some(mapped) = mapped {
                        self.commit_text(context, &mapped)?;
                        return Ok(true);
                    }
                }
            }
        }

        Ok(false)
    }

    /// Refresh the composition's underlined preedit text to match the
    /// current session buffer. `candidates` is accepted (and currently
    /// unused beyond documenting intent) so the eventual candidate-window
    /// refresh call has an obvious place to plug in -- see the TODO below.
    fn refresh_composition(&self, context: &ITfContext, candidates: &[Entry]) -> Result<()> {
        let preedit = {
            let state = self.state.borrow();
            state.session.as_ref().map(|s| s.preedit.clone()).unwrap_or_default()
        };
        self.update_composition(context, &preedit)?;

        // TODO(candidate window): forward `candidates` to
        // candidate_window::CandidateWindow::update(...) here once that
        // type has a concrete "attach to this text service instance" wiring
        // decided -- see candidate_window.rs's own TODOs. on_key_down's
        // candidate-producing branches (push_char/pop_char) funnel through
        // this one function specifically so there is a single place to add
        // that call.
        let _ = candidates;
        Ok(())
    }

    /// Commit `text` into the focused document via
    /// ITfInsertAtSelection::InsertTextAtSelection, then end any active
    /// composition.
    fn commit_text(&self, context: &ITfContext, text: &str) -> Result<()> {
        let client_id = self.state.borrow().client_id;
        let text_utf16: Vec<u16> = text.encode_utf16().collect();

        let session = EditSessionImpl::new(context.clone(), move |cookie, ctx| {
            // SAFETY: DoEditSession's contract (see the module doc comment)
            // guarantees this closure runs synchronously, on this thread,
            // with a valid write-locked document -- the precondition
            // InsertTextAtSelection requires.
            unsafe {
                let insert: ITfInsertAtSelection = ctx.cast()?;
                let _range: ITfRange = insert.InsertTextAtSelection(
                    cookie,
                    INSERT_TEXT_AT_SELECTION_FLAGS(0),
                    &text_utf16,
                )?;
            }
            Ok(())
        });
        run_edit_session(session, client_id)?;

        self.end_composition(context)?;
        Ok(())
    }

    /// Start a new ITfComposition if none is active, then replace its
    /// range's text with `preedit` and collapse the selection to the end.
    ///
    /// TSF convention (matching every other CJK IME on Windows) is to show
    /// the *raw input* underlined in the composition while the candidate
    /// list is a separate floating window -- not to inline the converted
    /// word into the document until commit. This writes `preedit` (the raw
    /// pinyin buffer), never a candidate's converted word, into the
    /// composition range.
    fn update_composition(&self, context: &ITfContext, preedit: &str) -> Result<()> {
        let client_id = self.state.borrow().client_id;
        let already_composing = self.state.borrow().composition.is_some();

        if !already_composing {
            self.start_composition(context, client_id)?;
        }

        let Some(composition) = self.state.borrow().composition.as_ref().map(|c| c.composition.clone()) else {
            // start_composition above should have populated this; if it
            // didn't (e.g. StartComposition failed silently somehow), bail
            // rather than proceed against a nonexistent composition.
            return Err(E_FAIL.into());
        };

        let text_utf16: Vec<u16> = preedit.encode_utf16().collect();
        let session = EditSessionImpl::new(context.clone(), move |cookie, _ctx| {
            // SAFETY: `composition` was cloned (AddRef'd) before this
            // closure was constructed, so it remains a valid COM reference
            // independent of `self`'s lifetime -- see the module doc
            // comment's note on why closures never reach back into &self.
            unsafe {
                let range: ITfRange = composition.GetRange()?;
                range.SetText(cookie, 0, &text_utf16)?;
                range.Collapse(cookie, TF_ANCHOR_END)?;
            }
            // NOTE: this does not yet apply the underline "input" display
            // attribute (TF_ATTR_INPUT via GUID_PROP_ATTRIBUTE) -- doing so
            // requires an ITfDisplayAttributeInfo registered through
            // ITfCategoryMgr/ITfSource at Activate time, which isn't wired
            // up in this draft (see the TODO in Activate below).
            // Composition text updates correctly without it; only the
            // visual underline styling is missing until that registration
            // is added.
            Ok(())
        });
        run_edit_session(session, client_id)
    }

    /// Start a new composition anchored at the current selection. Populates
    /// `state.composition` on success. Split out from update_composition
    /// so the "reserve an insertion point, then StartComposition" sequence
    /// -- which itself must run inside its own edit session -- stays
    /// self-contained.
    fn start_composition(&self, context: &ITfContext, client_id: u32) -> Result<()> {
        // Rc<RefCell<Option<...>>> bridges the composition handle created
        // *inside* the edit-session closure back out to this function,
        // since the closure only returns Result<()> (it's ITfEditSession
        // shaped, not a general-purpose callback with an arbitrary return
        // type).
        let created = std::rc::Rc::new(RefCell::new(None::<ITfComposition>));
        let created_for_closure = created.clone();

        let session = EditSessionImpl::new(context.clone(), move |cookie, ctx| {
            // SAFETY: see commit_text's SAFETY note -- same DoEditSession
            // synchronous-callback guarantee applies here.
            unsafe {
                let insert_sel: ITfInsertAtSelection = ctx.cast()?;
                let anchor: ITfRange = insert_sel.InsertTextAtSelection(
                    cookie,
                    INSERT_TEXT_AT_SELECTION_FLAGS(0),
                    &[],
                )?;
                let comp_services: ITfContextComposition = ctx.cast()?;
                let composition: ITfComposition =
                    comp_services.StartComposition(cookie, &anchor, None)?;
                *created_for_closure.borrow_mut() = Some(composition);
            }
            Ok(())
        });
        run_edit_session(session, client_id)?;

        let Some(composition) = created.borrow_mut().take() else {
            return Err(E_FAIL.into());
        };
        self.state.borrow_mut().composition = Some(CompositionState { composition });
        Ok(())
    }

    /// End the active composition (if any), clearing composition state.
    /// Called both on successful commit and on cancel (Escape / empty buffer).
    fn end_composition(&self, context: &ITfContext) -> Result<()> {
        let client_id = self.state.borrow().client_id;
        let Some(comp_state) = self.state.borrow_mut().composition.take() else {
            return Ok(());
        };

        let session = EditSessionImpl::new(context.clone(), move |cookie, _ctx| {
            // SAFETY: `comp_state.composition` was cloned before this
            // closure was constructed (moved in via the outer `let`, itself
            // taken from self.state before the closure exists) -- same
            // "never borrow &self across the callback" rule as elsewhere.
            unsafe { comp_state.composition.EndComposition(cookie) }
        });
        run_edit_session(session, client_id)
    }
}

/// Run `session` synchronously with read-write access.
///
/// TF_ES_SYNC | TF_ES_READWRITE is correct for every call site in this
/// file -- Ztap never needs an async edit session, since all document
/// mutation happens directly in response to a key event already running on
/// the UI thread, and TF_ES_SYNC is what makes DoEditSession execute
/// before RequestEditSession returns (which every closure above relies on
/// for its captured state to still be meaningful when the call returns).
///
/// Takes `session` by value (not `&EditSessionImpl`) so the
/// `EditSessionImpl -> ITfEditSession` conversion (`#[implement]`'s
/// generated `From<EditSessionImpl> for ITfEditSession` impl) applies
/// directly via `.into()`, with no ambiguity between "clone the COM
/// object" and "clone the reference" for the compiler to get wrong --
/// CI's own error showed the previous `&EditSessionImpl`-based version
/// resolving `.clone()` against the reference itself rather than the
/// underlying object, since `EditSessionImpl` has no `#[derive(Clone)]`
/// of its own (COM refcount-clone semantics come from `#[implement]`,
/// which apparently doesn't extend to being reachable through a plain
/// `&` the way this draft assumed). Every call site already constructs a
/// fresh, single-use `EditSessionImpl` immediately before calling this
/// function, so taking ownership here costs nothing.
fn run_edit_session(session: EditSessionImpl, client_id: u32) -> Result<()> {
    let context = session.context.clone();
    let iface: ITfEditSession = session.into();
    // SAFETY: `iface` is a fully-constructed ITfEditSession wrapping the
    // session object; RequestEditSession's documented contract is that
    // with TF_ES_SYNC set it synchronously invokes DoEditSession before
    // returning.
    let hr: HRESULT = unsafe { context.RequestEditSession(client_id, &iface, TF_ES_SYNC | TF_ES_READWRITE)? };
    if hr.is_err() {
        return Err(hr.into());
    }
    Ok(())
}

// -- ITfTextInputProcessor ---------------------------------------------

impl ITfTextInputProcessor_Impl for ZtapTextService_Impl {
    /// Called by TSF when the user's language profile activates this
    /// service. This is where all real initialization happens -- see
    /// ZtapTextService::new's doc comment for why it's not done eagerly.
    fn Activate(&self, ptim: Ref<'_, ITfThreadMgr>, tid: u32) -> Result<()> {
        let Some(thread_mgr) = ptim.as_ref() else {
            return Err(E_FAIL.into());
        };

        let client_id = tid;

        // A dictionary load failure is fatal to this text service -- there
        // is no meaningful degraded mode for an IME with no dictionary --
        // so it propagates as an activation failure. TSF will then simply
        // not offer this text service to the user, which is the correct
        // outcome rather than activating into a silently broken state.
        let dict = Dictionary::load_builtin();
        let store_path = ZtapTextService::learning_store_path();
        let store = LearningStore::load(store_path);
        let session = InputSession::new(dict, store);

        let keystroke_mgr: ITfKeystrokeMgr = thread_mgr.cast()?;

        // WARNING: turning `&ZtapTextService_Impl` (i.e. `self` here) into
        // an owned ITfKeyEventSink COM reference is the single
        // most-likely-to-be-wrong line in this file. windows-rs's
        // #[implement]-generated types normally expose this via a method
        // on the outer wrapper (e.g. constructing ZtapTextService first as
        // a local, converting it to ITfTextInputProcessor via `.into()`,
        // then `.cast::<ITfKeyEventSink>()` on *that* -- not by reaching
        // for a COM interface from inside a method that only has `&self`
        // of the _Impl type). The realistic fix is almost certainly
        // restructuring so activation flows through the already-owned
        // outer interface handle the class factory produced (see lib.rs's
        // DllGetClassObject), rather than trying to conjure one here.
        // Left as an explicit, loud gap rather than a plausible-looking
        // cast that could compile by accident and be wrong at runtime.
        let this_as_sink: ITfKeyEventSink = self
            .cast()
            .expect("TODO: verify self.cast::<ITfKeyEventSink>() is valid inside an _Impl method on a real windows-rs build; see the WARNING comment above");

        unsafe {
            keystroke_mgr.AdviseKeyEventSink(client_id, &this_as_sink, BOOL(1))?;
        }

        let mut state = self.state.borrow_mut();
        state.thread_mgr = Some(thread_mgr.clone());
        state.client_id = client_id;
        state.key_event_sink_advised = true;
        state.session = Some(session);

        // TODO(display attributes): register an ITfDisplayAttributeInfo
        // (underline style for TF_ATTR_INPUT) via ITfCategoryMgr +
        // ITfSource::AdviseSink(IID_ITfDisplayAttributeProvider, ...), and
        // implement ITfDisplayAttributeProvider on ZtapTextService so the
        // composition underline in update_composition actually renders.
        // Composition text updates correctly without this; only the visual
        // underline is affected.

        Ok(())
    }

    /// Called by TSF when this service is deactivated (profile switched
    /// away, or the thread is shutting down). Must undo everything
    /// Activate set up, and -- critically -- flush the learning store to
    /// disk, since this may be the last chance before the process exits.
    fn Deactivate(&self) -> Result<()> {
        let mut state = self.state.borrow_mut();

        if state.key_event_sink_advised {
            if let Some(thread_mgr) = state.thread_mgr.take() {
                if let Ok(keystroke_mgr) = thread_mgr.cast::<ITfKeystrokeMgr>() {
                    // SAFETY: unadvising a sink that was successfully
                    // advised in Activate with the same client_id; safe per
                    // UnadviseKeyEventSink's documented contract.
                    unsafe {
                        let _ = keystroke_mgr.UnadviseKeyEventSink(state.client_id);
                    }
                }
            }
            state.key_event_sink_advised = false;
        }

        if let Some(mut session) = state.session.take() {
            // LearningStore's own Drop impl also flushes (see learning.rs's
            // module docs), but flushing explicitly here means a failure is
            // at least observable via the Result (even though there's
            // nowhere meaningful to surface it from inside Deactivate)
            // rather than silently swallowed by a Drop that can't return
            // errors at all.
            session.store.flush_if_dirty();
        }

        Ok(())
    }
}

// -- ITfKeyEventSink -----------------------------------------------------

impl ITfKeyEventSink_Impl for ZtapTextService_Impl {
    /// TSF asks "would you consume this key" before delivering it for real
    /// via OnKeyDown. This performs the same *classification* as
    /// on_key_down without any of its mutation, so a key can be tested
    /// and then, separately, actually applied without double-applying it.
    ///
    /// NOTE: this duplicates on_key_down's routing conditions as read-only
    /// checks rather than sharing code with it. A cleaner design would
    /// split on_key_down into a pure "classify" step and a separate
    /// "apply" step reused by both methods; left as a follow-up rather than
    /// risk a double-apply bug in this untested first pass -- the current
    /// split means OnTestKeyDown can occasionally over-report "yes" (e.g.
    /// Page Up/Down, which on_key_down currently still declines to
    /// consume -- see its TODO), but never under-reports, which is the
    /// safer direction to be wrong in for a "should I intercept this key"
    /// check.
    fn OnTestKeyDown(&self, pic: Ref<'_, ITfContext>, wparam: WPARAM, _lparam: LPARAM) -> Result<BOOL> {
        let Some(_context) = pic.as_ref() else { return Ok(BOOL(0)) };
        let vk = wparam.0 as u16;
        let has_composition = self
            .state
            .borrow()
            .session
            .as_ref()
            .map(|s| !s.preedit.is_empty())
            .unwrap_or(false);

        let maybe_consumed = (b'a' as u16..=b'z' as u16).contains(&vk)
            || (has_composition
                && matches!(vk, v if v == VK_BACK.0 || v == VK_RETURN.0 || v == VK_ESCAPE.0 || v == VK_SPACE.0))
            || (has_composition && (b'1' as u16..=b'9' as u16).contains(&vk));

        Ok(BOOL(maybe_consumed as i32))
    }

    fn OnKeyDown(&self, pic: Ref<'_, ITfContext>, wparam: WPARAM, _lparam: LPARAM) -> Result<BOOL> {
        let Some(context) = pic.as_ref() else { return Ok(BOOL(0)) };
        let consumed = self.on_key_down(context, wparam.0 as u32)?;
        Ok(BOOL(consumed as i32))
    }

    fn OnTestKeyUp(&self, _pic: Ref<'_, ITfContext>, _wparam: WPARAM, _lparam: LPARAM) -> Result<BOOL> {
        // Ztap acts entirely on key-down; key-up is never consumed.
        Ok(BOOL(0))
    }

    fn OnKeyUp(&self, _pic: Ref<'_, ITfContext>, _wparam: WPARAM, _lparam: LPARAM) -> Result<BOOL> {
        Ok(BOOL(0))
    }

    fn OnPreservedKey(&self, _pic: Ref<'_, ITfContext>, _rguid: *const GUID) -> Result<BOOL> {
        // Ztap doesn't register any preserved (hotkey) key combinations.
        Ok(BOOL(0))
    }

    fn OnSetFocus(&self, _fforeground: BOOL) -> Result<()> {
        Ok(())
    }
}

// -- ITfCompositionSink ---------------------------------------------------

impl ITfCompositionSink_Impl for ZtapTextService_Impl {
    /// TSF calls this if it force-terminates our composition (e.g. the
    /// document lost focus, or another text service took over). We must
    /// forget our composition handle without calling EndComposition on
    /// it again -- it's already gone -- and reset the pinyin buffer so the
    /// next keystroke starts a fresh composition instead of silently
    /// continuing to append to now-orphaned state.
    fn OnCompositionTerminated(&self, _ecwrite: u32, _pcomposition: Ref<'_, ITfComposition>) -> Result<()> {
        let mut state = self.state.borrow_mut();
        state.composition = None;
        if let Some(session) = state.session.as_mut() {
            session.cancel();
        }
        Ok(())
    }
}

// -- Edit sessions --------------------------------------------------------

/// A one-shot ITfEditSession wrapping a boxed 'static closure.
///
/// TSF requires document mutation to happen inside a DoEditSession
/// callback (see the module doc comment). Rather than hand-writing a new
/// #[implement(ITfEditSession)] struct at every call site, this type
/// takes the closure once and is reused by commit_text,
/// update_composition, start_composition, and end_composition above.
///
/// Every closure passed in must be 'static and must not borrow &self
/// from the ZtapTextService that created it -- capture cloned COM
/// interface handles (cheap AddRefs) instead, exactly as every call site
/// above already does. See the module doc comment's "Edit sessions"
/// section.
#[implement(ITfEditSession)]
pub struct EditSessionImpl {
    context: ITfContext,
    /// RefCell<Option<...>> so the FnOnce can be taken and invoked
    /// exactly once from DoEditSession, even though COM methods take
    /// &self rather than self by value.
    #[allow(clippy::type_complexity)]
    body: RefCell<Option<Box<dyn FnOnce(u32, &ITfContext) -> Result<()>>>>,
}

impl EditSessionImpl {
    fn new(context: ITfContext, body: impl FnOnce(u32, &ITfContext) -> Result<()> + 'static) -> Self {
        EditSessionImpl {
            context,
            body: RefCell::new(Some(Box::new(body))),
        }
    }
}

impl ITfEditSession_Impl for EditSessionImpl_Impl {
    fn DoEditSession(&self, ec: u32) -> Result<()> {
        let Some(body) = self.body.borrow_mut().take() else {
            // A second call would mean this one-shot session was scheduled
            // twice, which shouldn't happen -- surfaced as E_FAIL rather
            // than silently succeeding, since it indicates a bug in how
            // callers of run_edit_session are using this type.
            return Err(E_FAIL.into());
        };
        body(ec, &self.context)
    }
}
