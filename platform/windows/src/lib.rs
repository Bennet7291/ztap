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
use windows_core::{BOOL, Interface, Ref};

use ztap_core::{Dictionary, Entry, InputSession, LearningStore};

pub const CLSID_ZTAP_TEXT_SERVICE: GUID =
    GUID::from_u128(0x5a746170_0001_4000_8000_000000005a54);

pub const GUID_ZTAP_PROFILE: GUID = GUID::from_u128(0x5a746170_0002_4000_8000_000000005a54);

pub const LANGID_ZH_CN: u16 = 0x0804;

struct CompositionState {
    composition: ITfComposition,
}

#[implement(ITfTextInputProcessor, ITfKeyEventSink, ITfCompositionSink)]
pub struct ZtapTextService {
    state: RefCell<ServiceState>,
}

struct ServiceState {
    thread_mgr: Option<ITfThreadMgr>,
    client_id: u32,
    key_event_sink_advised: bool,
    session: Option<InputSession>,
    composition: Option<CompositionState>,
    self_as_sink: Option<ITfKeyEventSink>,
}

impl ZtapTextService {
    pub fn new() -> ZtapTextService {
        ZtapTextService {
            state: RefCell::new(ServiceState {
                thread_mgr: None,
                client_id: 0,
                key_event_sink_advised: false,
                session: None,
                composition: None,
                self_as_sink: None,
            }),
        }
    }

    pub fn create() -> (ITfTextInputProcessor, ITfKeyEventSink) {
        let svc = ZtapTextService::new();
        let processor: ITfTextInputProcessor = svc.into();
        let sink: ITfKeyEventSink = processor.cast().expect("ZtapTextService implements ITfKeyEventSink");
        (processor, sink)
    }

    fn learning_store_path() -> std::path::PathBuf {
        use windows::Win32::Foundation::MAX_PATH;
        use windows::Win32::UI::Shell::{SHGetFolderPathW, CSIDL_APPDATA};

        let mut buf = [0u16; MAX_PATH as usize];
        let result = unsafe { SHGetFolderPathW(None, CSIDL_APPDATA as i32, None, 0, &mut buf) };
        if result.is_err() {
            return std::path::PathBuf::from("ztap_user.dict");
        }
        let len = buf.iter().position(|&c| c == 0).unwrap_or(0);
        let appdata = String::from_utf16_lossy(&buf[..len]);
        std::path::PathBuf::from(appdata).join("Ztap").join("user.dict")
    }

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
            return Ok(false);
        }

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

    fn refresh_composition(&self, context: &ITfContext, candidates: &[Entry]) -> Result<()> {
        let preedit = {
            let state = self.state.borrow();
            state.session.as_ref().map(|s| s.preedit.clone()).unwrap_or_default()
        };
        self.update_composition(context, &preedit)?;
        let _ = candidates;
        Ok(())
    }

    fn commit_text(&self, context: &ITfContext, text: &str) -> Result<()> {
        let client_id = self.state.borrow().client_id;
        let text_utf16: Vec<u16> = text.encode_utf16().collect();

        let session = EditSessionImpl::new(context.clone(), move |cookie, ctx| {
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

    fn update_composition(&self, context: &ITfContext, preedit: &str) -> Result<()> {
        let client_id = self.state.borrow().client_id;
        let already_composing = self.state.borrow().composition.is_some();

        if !already_composing {
            self.start_composition(context, client_id)?;
        }

        let Some(composition) = self.state.borrow().composition.as_ref().map(|c| c.composition.clone()) else {
            return Err(E_FAIL.into());
        };

        let text_utf16: Vec<u16> = preedit.encode_utf16().collect();
        let session = EditSessionImpl::new(context.clone(), move |cookie, _ctx| {
            unsafe {
                let range: ITfRange = composition.GetRange()?;
                range.SetText(cookie, 0, &text_utf16)?;
                range.Collapse(cookie, TF_ANCHOR_END)?;
            }
            Ok(())
        });
        run_edit_session(session, client_id)
    }

    fn start_composition(&self, context: &ITfContext, client_id: u32) -> Result<()> {
        let created = std::rc::Rc::new(RefCell::new(None::<ITfComposition>));
        let created_for_closure = created.clone();

        let session = EditSessionImpl::new(context.clone(), move |cookie, ctx| {
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

    fn end_composition(&self, context: &ITfContext) -> Result<()> {
        let client_id = self.state.borrow().client_id;
        let Some(comp_state) = self.state.borrow_mut().composition.take() else {
            return Ok(());
        };

        let session = EditSessionImpl::new(context.clone(), move |cookie, _ctx| {
            unsafe { comp_state.composition.EndComposition(cookie) }
        });
        run_edit_session(session, client_id)
    }
}

fn run_edit_session(session: EditSessionImpl, client_id: u32) -> Result<()> {
    let context = session.context.clone();
    let iface: ITfEditSession = session.into();
    let hr: HRESULT = unsafe {
        context.RequestEditSession(client_id, &iface, TF_ES_SYNC | TF_ES_READWRITE)?
    };
    if hr.is_err() {
        return Err(hr.into());
    }
    Ok(())
}

impl ITfTextInputProcessor_Impl for ZtapTextService_Impl {
    fn Activate(&self, ptim: Ref<'_, ITfThreadMgr>, tid: u32) -> Result<()> {
        let Some(thread_mgr) = ptim.as_ref() else {
            return Err(E_FAIL.into());
        };

        let client_id = tid;

        let dict = Dictionary::load_builtin();
        let store_path = ZtapTextService::learning_store_path();
        let store = LearningStore::load(store_path);
        let session = InputSession::new(dict, store);

        let keystroke_mgr: ITfKeystrokeMgr = thread_mgr.cast()?;

        let sink = self.state.borrow_mut().self_as_sink.take();
        let this_as_sink = match sink {
            Some(s) => s,
            None => return Err(E_FAIL.into()),
        };

        unsafe {
            keystroke_mgr.AdviseKeyEventSink(client_id, &this_as_sink, true)?;
        }

        let mut state = self.state.borrow_mut();
        state.thread_mgr = Some(thread_mgr.clone());
        state.client_id = client_id;
        state.key_event_sink_advised = true;
        state.session = Some(session);
        state.self_as_sink = Some(this_as_sink);

        Ok(())
    }

    fn Deactivate(&self) -> Result<()> {
        let mut state = self.state.borrow_mut();

        if state.key_event_sink_advised {
            if let Some(thread_mgr) = state.thread_mgr.take() {
                if let Ok(keystroke_mgr) = thread_mgr.cast::<ITfKeystrokeMgr>() {
                    unsafe {
                        let _ = keystroke_mgr.UnadviseKeyEventSink(state.client_id);
                    }
                }
            }
            state.key_event_sink_advised = false;
        }

        if let Some(mut session) = state.session.take() {
            session.store.flush_if_dirty();
        }

        Ok(())
    }
}

impl ITfKeyEventSink_Impl for ZtapTextService_Impl {
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
        Ok(BOOL(0))
    }

    fn OnKeyUp(&self, _pic: Ref<'_, ITfContext>, _wparam: WPARAM, _lparam: LPARAM) -> Result<BOOL> {
        Ok(BOOL(0))
    }

    fn OnPreservedKey(&self, _pic: Ref<'_, ITfContext>, _rguid: *const GUID) -> Result<BOOL> {
        Ok(BOOL(0))
    }

    fn OnSetFocus(&self, _fforeground: BOOL) -> Result<()> {
        Ok(())
    }
}

impl ITfCompositionSink_Impl for ZtapTextService_Impl {
    fn OnCompositionTerminated(&self, _ecwrite: u32, _pcomposition: Ref<'_, ITfComposition>) -> Result<()> {
        let mut state = self.state.borrow_mut();
        state.composition = None;
        if let Some(session) = state.session.as_mut() {
            session.cancel();
        }
        Ok(())
    }
}

#[implement(ITfEditSession)]
pub struct EditSessionImpl {
    context: ITfContext,
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
            return Err(E_FAIL.into());
        };
        body(ec, &self.context)
    }
}
