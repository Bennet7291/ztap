//! ztap-core — platform-agnostic input engine.
//!
//! # Module overview
//!
//! | Module | Responsibility |
//! |--------|---------------|
//! | [`pinyin`]      | Segment a continuous pinyin string into syllable paths |
//! | [`dictionary`]  | Load and query the word list (built-in + user) |
//! | [`ranking`]     | Score and sort candidates (corpus freq + user pref + length) |
//! | [`learning`]    | Record user selections and persist to disk |
//! | [`punctuation`] | Map ASCII punctuation to full-width Chinese equivalents |
//!
//! # Typical call flow
//!
//! ```text
//! user types "nihao"
//!        │
//!        ▼
//! pinyin::segment("nihao")
//!        → [["ni","hao"], ...]
//!        │
//!        ▼
//! dictionary::lookup(&["ni","hao"])
//!        → [Entry{你好,99000}, Entry{你号,1000}, ...]
//!        │
//!        ▼
//! ranking::rank(&mut entries, &store)
//!        → [你好, 你号, 泥好, ...]
//!        │
//!        ▼
//! display candidates; user picks one
//!        │
//!        ▼
//! learning::record_selection("你好")
//! ```

pub mod dictionary;
pub mod learning;
pub mod pinyin;
pub mod punctuation;
pub mod ranking;

// ── Convenience re-exports ────────────────────────────────────────────────────

pub use dictionary::{Dictionary, Entry};
pub use learning::LearningStore;
pub use pinyin::{segment, Syllable};
pub use punctuation::PunctuationState;
pub use ranking::rank;

// ── High-level session façade ─────────────────────────────────────────────────

/// An active input session.
///
/// The platform layer creates one `InputSession` per composition cycle and
/// drives it through the methods below. The session owns the live pinyin
/// buffer, the dictionary handle, the user learning store, and the
/// punctuation state machine.
pub struct InputSession {
    /// The word database (built-in corpus + user dictionary merged).
    pub dict: Dictionary,
    /// User selection history, used by the ranker and flushed at session end.
    pub store: LearningStore,
    /// Punctuation state (tracks open/close quote parity).
    pub punct: PunctuationState,
    /// The raw pinyin characters typed so far (letters only, no tones).
    pub preedit: String,
}

impl InputSession {
    /// Create a new session. The platform layer calls this once on IME activation.
    pub fn new(dict: Dictionary, store: LearningStore) -> Self {
        InputSession {
            dict,
            store,
            punct: PunctuationState::new(),
            preedit: String::new(),
        }
    }

    /// Append a pinyin letter to the buffer and return the updated candidate list.
    pub fn push_char(&mut self, c: char) -> Vec<Entry> {
        self.preedit.push(c);
        self.candidates()
    }

    /// Remove the last character from the buffer and return the updated candidate list.
    pub fn pop_char(&mut self) -> Vec<Entry> {
        self.preedit.pop();
        self.candidates()
    }

    /// Cancel the current composition (Escape key): clear the buffer.
    pub fn cancel(&mut self) {
        self.preedit.clear();
        self.punct.reset();
    }

    /// Confirm the candidate at zero-based `idx` and return the word to commit.
    ///
    /// Records the selection in the learning store and clears the preedit buffer.
    /// Returns `None` if `idx` is out of range.
    pub fn select(&mut self, idx: usize) -> Option<String> {
        let mut cands = self.candidates();
        if idx >= cands.len() {
            return None;
        }
        let word = cands.remove(idx).word;
        self.store.record_selection(&word);
        self.preedit.clear();
        self.punct.reset();
        Some(word)
    }

    /// Compute the ranked candidate list for the current preedit buffer.
    ///
    /// Returns at most 9 entries (one per digit key on the candidate bar).
    /// Returns an empty list when the buffer is empty.
    pub fn candidates(&mut self) -> Vec<Entry> {
        if self.preedit.is_empty() {
            return vec![];
        }
        // Use the best-scoring segmentation path (first after sorting by syllable count).
        let paths = segment(&self.preedit);
        let syllables: Vec<&str> = match paths.first() {
            Some(p) => p.iter().map(|s| s.0.as_str()).collect(),
            None => return vec![],
        };
        let mut entries = self.dict.lookup(&syllables);
        ranking::rank_top(&mut entries, &self.store, 9);
        entries
    }
}
