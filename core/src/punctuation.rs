pub struct PunctuationState {

    double_open: bool,

    single_open: bool,
}

impl PunctuationState {
    pub fn new() -> Self {
        PunctuationState {
            double_open: false,
            single_open: false,
        }
    }

    pub fn reset(&mut self) {
        self.double_open = false;
        self.single_open = false;
    }

    pub fn map(&mut self, ascii: char) -> Option<&'static str> {
        match ascii {

            ','  => Some("，"),
            '.'  => Some("。"),
            '!'  => Some("！"),
            '?'  => Some("？"),
            ';'  => Some("；"),
            ':'  => Some("："),
            '\\' => Some("、"),
            '('  => Some("（"),
            ')'  => Some("）"),
            '<'  => Some("《"),
            '>'  => Some("》"),
            '`'  => Some("·"),
            '-'  => Some("——"),
            '^'  => Some("……"),

            '"' => {
                // U+201C LEFT DOUBLE QUOTATION MARK "

                let s = if self.double_open { "\u{201D}" } else { "\u{201C}" };
                self.double_open = !self.double_open;
                Some(s)
            }
            '\'' => {

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
        assert_eq!(s.map('"'), Some("\u{201C}"));
        assert_eq!(s.map('"'), Some("\u{201D}")); // "
        assert_eq!(s.map('"'), Some("\u{201C}")); // " again
    }

    #[test]
    fn test_single_quote_alternation() {
        let mut s = PunctuationState::new();
        assert_eq!(s.map('\''), Some("\u{2018}"));
        assert_eq!(s.map('\''), Some("\u{2019}"));
    }

    #[test]
    fn test_reset_restores_left_quote() {
        let mut s = PunctuationState::new();
        s.map('"'); // advances to right-quote state
        s.reset();
        assert_eq!(s.map('"'), Some("\u{201C}"));
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
