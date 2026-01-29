use std::{char, ops::Range};

use gpui::{Context, Window};
use ropey::Rope;
use sum_tree::Bias;

use crate::{
    RopeExt as _,
    input::{InputState, Selection},
};

/// Unique identifier for a cursor/selection.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, PartialOrd, Ord)]
pub struct CursorId(usize);

impl CursorId {
    pub fn new(id: usize) -> Self {
        Self(id)
    }

    pub fn as_usize(&self) -> usize {
        self.0
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CharType {
    /// a-z, A-Z, 0-9, _
    Word,
    /// '\t', ' ', '\u{00A0}' etc.
    Whitespace,
    /// \n, \r
    Newline,
    /// . , ; : ( ) [ ] { } ... or CJK characters: `汉`, `🎉` etc.
    Other,
}

/// Implementation based on <https://github.com/zed-industries/zed/blob/main/crates/gpui/src/text_system/line_wrapper.rs>
fn is_word_char(c: char) -> bool {
    matches!(c, '_' ) ||
    // ASCII alphanumeric characters, for English, numbers: `Hello123`, etc.
    c.is_ascii_alphanumeric() ||
    // Latin script in Unicode for French, German, Spanish, etc.
    // Latin-1 Supplement
    // https://en.wikipedia.org/wiki/Latin-1_Supplement
    matches!(c, '\u{00C0}'..='\u{00FF}') ||
    // Latin Extended-A
    // https://en.wikipedia.org/wiki/Latin_Extended-A
    matches!(c, '\u{0100}'..='\u{017F}') ||
    // Latin Extended-B
    // https://en.wikipedia.org/wiki/Latin_Extended-B
    matches!(c, '\u{0180}'..='\u{024F}') ||
    // Cyrillic for Russian, Ukrainian, etc.
    // https://en.wikipedia.org/wiki/Cyrillic_script_in_Unicode
    matches!(c, '\u{0400}'..='\u{04FF}') ||

    // Vietnamese (https://vietunicode.sourceforge.net/charset/)
    matches!(c, '\u{1E00}'..='\u{1EFF}') || // Latin Extended Additional
    matches!(c, '\u{0300}'..='\u{036F}') // Combining Diacritical Marks
}

impl From<char> for CharType {
    fn from(c: char) -> Self {
        match c {
            c if is_word_char(c) => CharType::Word,
            c if c == '\n' || c == '\r' => CharType::Newline,
            c if c.is_whitespace() => CharType::Whitespace,
            _ => CharType::Other,
        }
    }
}

impl CharType {
    /// Check if two CharTypes are connectable
    pub(crate) fn is_connectable(self, c: char) -> bool {
        let other = CharType::from(c);
        match (self, other) {
            (CharType::Word, CharType::Word) => true,
            (CharType::Whitespace, CharType::Whitespace) => true,
            _ => false,
        }
    }
}

impl InputState {
    /// Select the word at the given offset on double-click.
    ///
    /// The offset is the UTF-8 offset.
    pub(super) fn select_word(&mut self, offset: usize, _: &mut Window, cx: &mut Context<Self>) {
        let Some(range) = TextSelector::word_range(&self.text, offset) else {
            return;
        };

        self.set_selection(range.start, range.end);
        self.selected_word_range =
            Some(Selection::new(CursorId::default(), range.start, range.end));
        cx.notify()
    }

    /// Select the line at the given offset on triple-click.
    ///
    /// The offset is the UTF-8 offset.
    pub(super) fn select_line(&mut self, offset: usize, _: &mut Window, cx: &mut Context<Self>) {
        let range = TextSelector::line_range(&self.text, offset);
        self.set_selection(range.start, range.end);
        self.selected_word_range = None;
        cx.notify()
    }
}

pub(crate) struct TextSelector;
impl TextSelector {
    /// Select a line in the given text at the specified offset.
    ///
    /// The offset is the UTF-8 offset.
    ///
    /// Returns the start and end offsets of the selected line.
    pub fn line_range(text: &Rope, offset: usize) -> Range<usize> {
        let offset = text.clip_offset(offset, Bias::Left);
        let row = text.offset_to_point(offset).row;
        let start = text.line_start_offset(row);
        let end = text.line_end_offset(row);

        start..end
    }

    /// Select a word in the given text at the specified offset.
    ///
    /// The offset is the UTF-8 offset.
    ///
    /// Returns the start and end offsets of the selected word.
    pub fn word_range(text: &Rope, offset: usize) -> Option<Range<usize>> {
        let offset = text.clip_offset(offset, Bias::Left);
        let Some(char) = text.char_at(offset) else {
            return None;
        };

        let char_type = CharType::from(char);
        let mut start = offset;
        let mut end = offset + char.len_utf8();
        let prev_chars = text.chars_at(start).reversed().take(128);
        let next_chars = text.chars_at(end).take(128);

        for ch in prev_chars {
            if char_type.is_connectable(ch) {
                start -= ch.len_utf8();
            } else {
                break;
            }
        }

        for ch in next_chars {
            if char_type.is_connectable(ch) {
                end += ch.len_utf8();
            } else {
                break;
            }
        }

        Some(start..end)
    }

    /// Calculate the start of the previous word from the given offset.
    ///
    /// This function works from any offset, moving backward to find the start
    /// of the previous word boundary.
    pub fn previous_word_start_at(text: &Rope, offset: usize) -> usize {
        if offset == 0 {
            return 0;
        }

        // Convert to character-safe offset
        let offset = text.clip_offset(offset, Bias::Left);
        let Some(char) = text.char_at(offset) else {
            return 0;
        };

        let char_type = CharType::from(char);

        // Move backward to find word boundary (end of current word/whitespace region)
        let mut current = offset;
        let mut found_boundary = false;
        let prev_chars = text.chars_at(current).reversed().take(256);

        for ch in prev_chars {
            if char_type.is_connectable(ch) {
                // Still in the same word/whitespace region
                current -= ch.len_utf8();
            } else {
                // Found a boundary
                found_boundary = true;
                break;
            }
        }

        if !found_boundary {
            return 0;
        }

        // Now handle two cases:
        // 1. If we started from a word, return the start of the current word
        // 2. If we started from whitespace, skip whitespace to find the previous word
        let mut search_start = current;
        let prev_chars = text.chars_at(current).reversed().take(256);

        for ch in prev_chars {
            let ch_type = CharType::from(ch);
            if ch_type != CharType::Word {
                // Skip non-word characters (whitespace, punctuation, etc.)
                search_start -= ch.len_utf8();
            } else {
                // Found the start of a word
                break;
            }
        }

        // Now accumulate word characters to find the start of the word
        let mut word_start = search_start;
        let prev_chars = text.chars_at(search_start).reversed().take(256);

        for ch in prev_chars {
            let ch_type = CharType::from(ch);
            if ch_type == CharType::Word {
                word_start -= ch.len_utf8();
            } else {
                break;
            }
        }

        word_start
    }

    /// Calculate the start of the next word from the given offset.
    ///
    /// This function works from any offset, moving forward to find the start
    /// of the next word boundary (after skipping whitespace).
    pub fn next_word_start_at(text: &Rope, offset: usize) -> usize {
        let text_len = text.len();
        if offset >= text_len {
            return text_len;
        }

        // Convert to character-safe offset
        let offset = text.clip_offset(offset, Bias::Left);
        let Some(char) = text.char_at(offset) else {
            return text_len;
        };

        let char_type = CharType::from(char);
        let mut end = offset + char.len_utf8();

        // Move forward to find word boundary (end of current word/whitespace region)
        let next_chars = text.chars_at(end).take(256);

        for ch in next_chars {
            if char_type.is_connectable(ch) {
                // Still in the same word/whitespace region
                end += ch.len_utf8();
            } else {
                // Found a boundary
                break;
            }
        }

        // Now skip past any non-word characters to find the start of the next word
        let mut start_of_next_word = end;
        let next_chars = text.chars_at(end).take(256);

        for ch in next_chars {
            let ch_type = CharType::from(ch);
            if ch_type != CharType::Word {
                // Skip non-word characters (whitespace, punctuation, etc.)
                start_of_next_word += ch.len_utf8();
            } else {
                // Found the start of a word
                break;
            }
        }

        start_of_next_word
    }

    /// Calculate the end of the next word from the given offset.
    ///
    /// This function works from any offset, moving forward to find the end
    /// of the next word (after the word itself, not the start).
    pub fn next_word_end_at(text: &Rope, offset: usize) -> usize {
        let text_len = text.len();
        if offset >= text_len {
            return text_len;
        }

        // Convert to character-safe offset
        let offset = text.clip_offset(offset, Bias::Left);
        let Some(char) = text.char_at(offset) else {
            return text_len;
        };

        let char_type = CharType::from(char);
        let mut end = offset + char.len_utf8();

        // Move forward to find word boundary (end of current word/whitespace region)
        let next_chars = text.chars_at(end).take(256);

        for ch in next_chars {
            if char_type.is_connectable(ch) {
                // Still in the same word/whitespace region
                end += ch.len_utf8();
            } else {
                // Found a boundary
                break;
            }
        }

        end
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ropey::Rope;

    #[test]
    fn test_char_type_from_char() {
        assert_eq!(CharType::from('a'), CharType::Word);
        assert_eq!(CharType::from('Z'), CharType::Word);
        assert_eq!(CharType::from('0'), CharType::Word);
        assert_eq!(CharType::from('_'), CharType::Word);
        assert_eq!(CharType::from('.'), CharType::Other);
        assert_eq!(CharType::from(','), CharType::Other);
        assert_eq!(CharType::from(';'), CharType::Other);
        assert_eq!(CharType::from('!'), CharType::Other);
        assert_eq!(CharType::from('?'), CharType::Other);
        assert_eq!(CharType::from('['), CharType::Other);
        assert_eq!(CharType::from('{'), CharType::Other);
        assert_eq!(CharType::from(' '), CharType::Whitespace);
        assert_eq!(CharType::from('\t'), CharType::Whitespace);
        assert_eq!(CharType::from('\u{00A0}'), CharType::Whitespace);
        assert_eq!(CharType::from('\n'), CharType::Newline);
        assert_eq!(CharType::from('\r'), CharType::Newline);
        assert_eq!(CharType::from('汉'), CharType::Other);
        // European letters
        assert_eq!(CharType::from('é'), CharType::Word);
        assert_eq!(CharType::from('ä'), CharType::Word);
        assert_eq!(CharType::from('ö'), CharType::Word);
        assert_eq!(CharType::from('ü'), CharType::Word);
        //Cyrillic letters
        assert_eq!(CharType::from('д'), CharType::Word);
    }

    #[test]
    fn test_word_range() {
        use indoc::indoc;

        let rope = Rope::from(indoc! {
            r#"
            test text:
            abcde 中文🎉 test
            hello[()]
            test_connector ____
            Rope
            rök
            grande île
            "#
        });

        let tests = vec![
            (0, 0, Some("test")),
            (0, 4, Some(" ")),
            (1, 0, Some("abcde")),
            (1, 4, Some("abcde")),
            (1, 5, Some(" ")),
            (1, 6, Some("中")),
            (1, 9, Some("文")),
            (1, 13, Some("🎉")),
            (1, 20, Some("test")),
            (2, 5, Some("[")),
            (2, 6, Some("(")),
            (2, 7, Some(")")),
            (2, 8, Some("]")),
            (3, 5, Some("test_connector")),
            (3, 14, Some(" ")),
            (3, 16, Some("____")),
            (4, 0, Some("Rope")),
            (5, 0, Some("rök")),
            (6, 8, Some("île")),
        ];

        for (line, column, expected) in tests {
            let line_start_offset = rope.line_start_offset(line);
            let offset = line_start_offset + column;
            let range = TextSelector::word_range(&rope, offset);

            let actual = range.map(|r| rope.slice(r).to_string());
            let expect = expected.map(|s| s.to_string());
            assert_eq!(actual, expect, "line {}, column {}", line, column);
        }
    }

    #[test]
    fn test_line_range() {
        let rope = Rope::from("first line\nsecond line\nthird");
        let tests = vec![
            (0, 0, "first line"),
            (0, 5, "first line"),
            (1, 3, "second line"),
            (2, 1, "third"),
        ];

        for (line, column, expected) in tests {
            let line_start_offset = rope.line_start_offset(line);
            let offset = line_start_offset + column;
            let range = TextSelector::line_range(&rope, offset);

            let actual = rope.slice(range).to_string();
            assert_eq!(actual, expected, "line {}, column {}", line, column);
        }
    }
}
