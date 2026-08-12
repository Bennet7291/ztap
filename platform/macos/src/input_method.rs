use std::cell::RefCell;

use objc2::runtime::{AnyObject, NSObjectProtocol};
use objc2::{define_class, msg_send, DefinedClass};
use objc2_foundation::{NSInteger, NSRange, NSRect, NSString, NSUInteger};
use objc2_input_method_kit::IMKInputController;

use ztap_core::{Dictionary, Entry, InputSession, LearningStore};

use crate::candidate_window::CandidateWindow;

mod vk {
    use super::*;

    pub const RETURN: NSInteger = 0x24;
    pub const DELETE: NSInteger = 0x33;
    pub const ESCAPE: NSInteger = 0x35;
    pub const SPACE: NSInteger = 0x31;
    pub const PAGE_UP: NSInteger = 0x74;
    pub const PAGE_DOWN: NSInteger = 0x79;
}

const NS_EVENT_MODIFIER_FLAG_COMMAND: NSUInteger = 1 << 20;
const NS_EVENT_MODIFIER_FLAG_CONTROL: NSUInteger = 1 << 18;

struct ControllerState {
    session: Option<InputSession>,
    candidate_window: Option<CandidateWindow>,
}

struct Ivars {
    state: RefCell<ControllerState>,
}

define_class!(
    #[unsafe(super = IMKInputController)]
    #[name = "ZtapInputController"]
    #[ivars = Ivars]
    struct ZtapInputController;

    unsafe impl NSObjectProtocol for ZtapInputController {}
);

impl ZtapInputController {
    fn ivars(&self) -> &Ivars {
        DefinedClass::ivars(self)
    }

    fn ensure_session(&self) {
        let mut state = self.ivars().state.borrow_mut();
        if state.session.is_none() {
            let dict = Dictionary::load_builtin();
            let store_path = Self::learning_store_path();
            let store = LearningStore::load(store_path);
            state.session = Some(InputSession::new(dict, store));
        }
        if state.candidate_window.is_none() {
            state.candidate_window = CandidateWindow::new().ok();
        }
    }

    fn learning_store_path() -> std::path::PathBuf {
        let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".to_string());
        std::path::PathBuf::from(home)
            .join("Library")
            .join("Application Support")
            .join("Ztap")
            .join("user.dict")
    }

    pub fn handle_input_text(
        &self,
        string: Option<&NSString>,
        key_code: NSInteger,
        flags: NSUInteger,
        sender: Option<&AnyObject>,
    ) -> bool {
        unsafe { self.handle_key_event(string, key_code, flags, sender) }
    }

    pub fn handle_commit_composition(&self, sender: Option<&AnyObject>) {
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

    unsafe fn handle_key_event(
        &self,
        string: Option<&NSString>,
        key_code: NSInteger,
        flags: NSUInteger,
        sender: Option<&AnyObject>,
    ) -> bool {
        self.ensure_session();

        if flags & (NS_EVENT_MODIFIER_FLAG_COMMAND | NS_EVENT_MODIFIER_FLAG_CONTROL) != 0 {
            return false;
        }

        let has_composition = {
            let state = self.ivars().state.borrow();
            state.session.as_ref().map(|s| !s.preedit.is_empty()).unwrap_or(false)
        };

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
            return false;
        }

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

    fn refresh_composition(&self, candidates: &[Entry], sender: Option<&AnyObject>) {
        let preedit = {
            let state = self.ivars().state.borrow();
            state.session.as_ref().map(|s| s.preedit.clone()).unwrap_or_default()
        };
        self.client_set_marked_text(&preedit, sender);

        let rect = self.client_first_rect(sender);
        let words: Vec<String> = candidates.iter().map(|e| e.word.clone()).collect();
        let state = self.ivars().state.borrow();
        if let Some(cw) = state.candidate_window.as_ref() {
            cw.show(&preedit, &words, 0, rect);
        }
    }

    fn clear_marked_text(&self, sender: Option<&AnyObject>) {
        self.client_set_marked_text("", sender);
        let state = self.ivars().state.borrow();
        if let Some(cw) = state.candidate_window.as_ref() {
            cw.hide();
        }
    }

    fn commit_text(&self, text: &str, sender: Option<&AnyObject>) {
        self.client_insert_text(text, sender);
        let state = self.ivars().state.borrow();
        if let Some(cw) = state.candidate_window.as_ref() {
            cw.hide();
        }
    }

    fn client_set_marked_text(&self, text: &str, sender: Option<&AnyObject>) {
        let Some(client) = sender else { return };
        let ns_text = NSString::from_str(text);
        let len = ns_text.len();
        let selection_range = NSRange::new(len, 0);
        let replacement_range = NSRange::new(objc2_foundation::NSNotFound as NSUInteger, 0);
        unsafe {
            let _: () = msg_send![
                client,
                setMarkedText: &*ns_text,
                selectionRange: selection_range,
                replacementRange: replacement_range,
            ];
        }
    }

    fn client_insert_text(&self, text: &str, sender: Option<&AnyObject>) {
        let Some(client) = sender else { return };
        let ns_text = NSString::from_str(text);
        let replacement_range = NSRange::new(objc2_foundation::NSNotFound as NSUInteger, 0);
        unsafe {
            let _: () = msg_send![
                client,
                insertText: &*ns_text,
                replacementRange: replacement_range,
            ];
        }
    }

    fn client_first_rect(&self, sender: Option<&AnyObject>) -> NSRect {
        let Some(client) = sender else { return NSRect::ZERO };
        let range = NSRange::new(objc2_foundation::NSNotFound as NSUInteger, 0);
        let mut actual_range = NSRange::new(0, 0);
        unsafe {
            msg_send![
                client,
                firstRectForCharacterRange: range,
                actualRange: &mut actual_range,
            ]
        }
    }
}
