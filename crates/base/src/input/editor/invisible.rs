//! Characters that draw as nothing, or as a plain space, and so hide in text
//! that looks correct: a no-break space pasted into code, a zero width space,
//! a stray control character. The editor boxes them when
//! `highlight_invisible_characters` is on, and an application can use the
//! same rule to mark them elsewhere.

/// How an invisible character is best shown.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InvisibleCharacter {
    /// A space with a width, such as a no-break space: it can be shown as
    /// itself, marked.
    Space,
    /// A character without width, shown by its usual abbreviation.
    Named(&'static str),
    /// A control character or direction mark, shown by its code point.
    CodePoint,
}

/// Whether `character` is invisible, and how to show it. Tabs and line
/// breaks are not: editors show them their own way.
pub fn invisible_character(character: char) -> Option<InvisibleCharacter> {
    Some(match character {
        '\t' | '\n' | '\r' => return None,
        '\u{00A0}'
        | '\u{1680}'
        | '\u{2000}'..='\u{200A}'
        | '\u{202F}'
        | '\u{205F}'
        | '\u{3000}' => InvisibleCharacter::Space,
        '\u{00AD}' => InvisibleCharacter::Named("SHY"),
        '\u{200B}' => InvisibleCharacter::Named("ZWSP"),
        '\u{200C}' => InvisibleCharacter::Named("ZWNJ"),
        '\u{200D}' => InvisibleCharacter::Named("ZWJ"),
        '\u{2060}' => InvisibleCharacter::Named("WJ"),
        '\u{FEFF}' => InvisibleCharacter::Named("BOM"),
        // Direction marks and embeddings reorder text without showing.
        '\u{061C}'
        | '\u{200E}'
        | '\u{200F}'
        | '\u{202A}'..='\u{202E}'
        | '\u{2066}'..='\u{2069}' => InvisibleCharacter::CodePoint,
        // The rest of C0 and C1.
        character if character.is_control() => InvisibleCharacter::CodePoint,
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::{InvisibleCharacter, invisible_character};

    #[test]
    fn classifies_invisible_characters() {
        assert_eq!(invisible_character('a'), None);
        assert_eq!(invisible_character(' '), None);
        assert_eq!(invisible_character('\t'), None);
        assert_eq!(
            invisible_character('\u{00A0}'),
            Some(InvisibleCharacter::Space)
        );
        assert_eq!(
            invisible_character('\u{200B}'),
            Some(InvisibleCharacter::Named("ZWSP"))
        );
        assert_eq!(
            invisible_character('\u{FEFF}'),
            Some(InvisibleCharacter::Named("BOM"))
        );
        assert_eq!(
            invisible_character('\u{0007}'),
            Some(InvisibleCharacter::CodePoint)
        );
        assert_eq!(
            invisible_character('\u{009B}'),
            Some(InvisibleCharacter::CodePoint)
        );
        assert_eq!(
            invisible_character('\u{202E}'),
            Some(InvisibleCharacter::CodePoint)
        );
    }
}
