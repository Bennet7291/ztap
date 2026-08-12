pub mod dictionary;
pub mod learning;
pub mod pinyin;
pub mod punctuation;
pub mod ranking;

pub use dictionary::{Dictionary, Entry};
pub use learning::LearningStore;
pub use pinyin::{segment, Syllable};
pub use punctuation::PunctuationState;
pub use ranking::rank;

pub struct InputSession {

    pub dict: Dictionary,

    pub store: LearningStore,

    pub punct: PunctuationState,

    pub preedit: String,
}

impl InputSession {

    pub fn new(dict: Dictionary, store: LearningStore) -> Self {
        InputSession {
            dict,
            store,
            punct: PunctuationState::new(),
            preedit: String::new(),
        }
    }

    pub fn push_char(&mut self, c: char) -> Vec<Entry> {
        self.preedit.push(c);
        self.candidates()
    }

    pub fn pop_char(&mut self) -> Vec<Entry> {
        self.preedit.pop();
        self.candidates()
    }

    pub fn cancel(&mut self) {
        self.preedit.clear();
        self.punct.reset();
    }

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

    pub fn candidates(&mut self) -> Vec<Entry> {
        if self.preedit.is_empty() {
            return vec![];
        }

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
