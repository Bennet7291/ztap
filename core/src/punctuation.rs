//! Chinese punctuation mapping module.
//!
//! While Chinese input mode is active, ASCII punctuation keystrokes are mapped
//! to their full-width Chinese equivalents.
//!
//! # Paired symbols
//!
//! Quotation marks (`"` and `'`) require left/right variants. [`PunctuationState`]
//! tracks the open/close parity for each pair and alternates between them on each
//! keystroke. All other mappings are stateless.
//!
//! # Example
//!
//! ```
//! let mut state = PunctuationState::new();
//! assert_eq!(state.map(','), Some("，"));
//! assert_eq!(state.map('.'), Some("。"));
//! assert_eq!(state.map('"'), Some("\u{201C}")); // left double quote "
//! assert_eq!(state.map('"'), Some("\u{201D}")); // right double quote "
//! assert_eq!(state.map('a'), None);             // not a punctuation key
//! ```

/// Stateful punctuation mapper.
///
/// Tracks whether the next double-quote or single-quote keystroke should
/// produce an opening or closing quotation mark.
pub struct PunctuationState {
    /// `false` → next `"` produces left mark `"`, `true` → right mark `"`
    double_open: bool,
    /// `false` → next `'` produces left mark `'`, `true` → right mark `'`
    single_open: bool,
}

impl PunctuationState {
    pub fn new() -> Self {
        PunctuationState {
            double_open: false,
            single_open: false,
        }
    }

    /// Reset quotation-mark parity.
    ///
    /// Call after committing or cancelling a composition so the next session
    /// starts fresh with a left (opening) quote.
    pub fn reset(&mut self) {
        self.double_open = false;
        self.single_open = false;
    }

    /// Map an ASCII character to its Chinese punctuation equivalent.
    ///
    /// Returns `None` if the character has no Chinese mapping; the caller
    /// should pass the character through unchanged.
    pub fn map(&mut self, ascii: char) -> Option<&'static str> {
        match ascii {
            // ── Stateless mappings ────────────────────────────────────────────
            ','  => Some("，"),
            '.'  => Some("。"),
            '!'  => Some("！"),
            '?'  => Some("？"),
            ';'  => Some("；"),
            ':'  => Some("："),
            '\\' => Some("、"),   // enumeration comma (顿号); backslash key
            '('  => Some("（"),
            ')'  => Some("）"),
            '<'  => Some("《"),   // book title mark open
            '>'  => Some("》"),   // book title mark close
            '`'  => Some("·"),    // middle dot (间隔号)
            '-'  => Some("——"),   // em dash (破折号)
            '^'  => Some("……"),   // ellipsis (省略号)

            // ── Stateful paired mappings ──────────────────────────────────────
            '"' => {
                // U+201C LEFT DOUBLE QUOTATION MARK "
                // U+201D RIGHT DOUBLE QUOTATION MARK "
                let s = if self.double_open { "\u{201D}" } else { "\u{201C}" };
                self.double_open = !self.double_open;
                Some(s)
            }
            '\'' => {
                // U+2018 LEFT SINGLE QUOTATION MARK '
                // U+2019 RIGHT SINGLE QUOTATION MARK '
                let s = if self.single_open { "\u{2019}" } else { "\u{2018}" };
                self.single_open = !self.single_open;
                Some(s)
            }

            _ => None,
        }
    }
}

impl Default for PunctuationState {
    fn default() -> Self {
        Self::new()
    }
}

/// Return `true` if the given ASCII character has a Chinese punctuation mapping.
///
/// Stateless utility — does not distinguish left/right quotation marks.
pub fn is_punctuation_key(ascii: char) -> bool {
    matches!(
        ascii,
        ',' | '.' | '!' | '?' | ';' | ':' | '\\'
        | '(' | ')' | '<' | '>' | '`' | '-' | '^'
        | '"' | '\''
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_basic_mappings() {
        let mut s = PunctuationState::new();
        assert_eq!(s.map(','), Some("，"));
        assert_eq!(s.map('.'), Some("。"));
        assert_eq!(s.map('!'), Some("！"));
        assert_eq!(s.map('?'), Some("？"));
        assert_eq!(s.map(';'), Some("；"));
        assert_eq!(s.map(':'), Some("："));
    }

    #[test]
    fn test_no_mapping_for_letters_and_digits() {
        let mut s = PunctuationState::new();
        assert_eq!(s.map('a'), None);
        assert_eq!(s.map('1'), None);
        assert_eq!(s.map(' '), None);
    }

    #[test]
    fn test_double_quote_alternation() {
        let mut s = PunctuationState::new();
        assert_eq!(s.map('"'), Some("\u{201C}")); // "
        assert_eq!(s.map('"'), Some("\u{201D}")); // "
        assert_eq!(s.map('"'), Some("\u{201C}")); // " again
    }

    #[test]
    fn test_single_quote_alternation() {
        let mut s = PunctuationState::new();
        assert_eq!(s.map('\''), Some("\u{2018}")); // '
        assert_eq!(s.map('\''), Some("\u{2019}")); // '
    }

    #[test]
    fn test_reset_restores_left_quote() {
        let mut s = PunctuationState::new();
        s.map('"'); // advances to right-quote state
        s.reset();
        assert_eq!(s.map('"'), Some("\u{201C}")); // back to left
    }

    #[test]
    fn test_brackets_and_title_marks() {
        let mut s = PunctuationState::new();
        assert_eq!(s.map('('), Some("（"));
        assert_eq!(s.map(')'), Some("）"));
        assert_eq!(s.map('<'), Some("《"));
        assert_eq!(s.map('>'), Some("》"));
    }

    #[test]
    fn test_is_punctuation_key() {
        assert!(is_punctuation_key(','));
        assert!(is_punctuation_key('"'));
        assert!(!is_punctuation_key('a'));
        assert!(!is_punctuation_key('5'));
    }
}
