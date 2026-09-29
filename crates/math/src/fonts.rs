// Copyright (c) 2026 Joel Baumert. All Rights Reserved.

//! Math alphabets (`\mathbf`, `\mathbb`, `\mathcal`, …). MathML Core only
//! honours `mathvariant="normal"`, so styled letters are emitted as the
//! Unicode Mathematical Alphanumeric Symbols (U+1D400–U+1D7FF) instead;
//! the handful of letters that live in Letterlike Symbols are mapped
//! individually.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Font {
    /// Default math style: single letters italic, digits upright.
    Normal,
    /// `\mathrm`, `\rm`: upright letters.
    Upright,
    Bold,
    Italic,
    BoldItalic,
    DoubleStruck,
    Script,
    Fraktur,
    SansSerif,
    Monospace,
}

impl Font {
    /// The styled form of `c`, or `c` itself when the alphabet has no
    /// such letter (Greek, punctuation, …).
    pub(crate) fn map(self, c: char) -> char {
        let (upper, lower, digit): (u32, u32, Option<u32>) = match self {
            Font::Normal | Font::Upright => return c,
            Font::Bold => (0x1D400, 0x1D41A, Some(0x1D7CE)),
            Font::Italic => (0x1D434, 0x1D44E, None),
            Font::BoldItalic => (0x1D468, 0x1D482, Some(0x1D7CE)),
            Font::DoubleStruck => (0x1D538, 0x1D552, Some(0x1D7D8)),
            Font::Script => (0x1D49C, 0x1D4B6, None),
            Font::Fraktur => (0x1D504, 0x1D51E, None),
            Font::SansSerif => (0x1D5A0, 0x1D5BA, Some(0x1D7E2)),
            Font::Monospace => (0x1D670, 0x1D68A, Some(0x1D7F6)),
        };
        if let Some(hole) = self.letterlike(c) {
            return hole;
        }
        let code = match c {
            'A'..='Z' => upper + (c as u32 - 'A' as u32),
            'a'..='z' => lower + (c as u32 - 'a' as u32),
            '0'..='9' => match digit {
                Some(d) => d + (c as u32 - '0' as u32),
                None => return c,
            },
            _ => return c,
        };
        char::from_u32(code).unwrap_or(c)
    }

    /// Letters whose styled form predates the math block and sits in
    /// Letterlike Symbols; the math block leaves those code points empty.
    fn letterlike(self, c: char) -> Option<char> {
        Some(match (self, c) {
            (Font::Italic, 'h') => 'ℎ',
            (Font::Script, 'B') => 'ℬ',
            (Font::Script, 'E') => 'ℰ',
            (Font::Script, 'F') => 'ℱ',
            (Font::Script, 'H') => 'ℋ',
            (Font::Script, 'I') => 'ℐ',
            (Font::Script, 'L') => 'ℒ',
            (Font::Script, 'M') => 'ℳ',
            (Font::Script, 'R') => 'ℛ',
            (Font::Script, 'e') => 'ℯ',
            (Font::Script, 'g') => 'ℊ',
            (Font::Script, 'o') => 'ℴ',
            (Font::Fraktur, 'C') => 'ℭ',
            (Font::Fraktur, 'H') => 'ℌ',
            (Font::Fraktur, 'I') => 'ℑ',
            (Font::Fraktur, 'R') => 'ℜ',
            (Font::Fraktur, 'Z') => 'ℨ',
            (Font::DoubleStruck, 'C') => 'ℂ',
            (Font::DoubleStruck, 'H') => 'ℍ',
            (Font::DoubleStruck, 'N') => 'ℕ',
            (Font::DoubleStruck, 'P') => 'ℙ',
            (Font::DoubleStruck, 'Q') => 'ℚ',
            (Font::DoubleStruck, 'R') => 'ℝ',
            (Font::DoubleStruck, 'Z') => 'ℤ',
            _ => return None,
        })
    }

    pub(crate) fn map_str(self, s: &str) -> String {
        s.chars().map(|c| self.map(c)).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_alphabets_and_their_holes() {
        assert_eq!(Font::Bold.map('A'), '𝐀');
        assert_eq!(Font::Bold.map('z'), '𝐳');
        assert_eq!(Font::Bold.map('7'), '𝟕');
        assert_eq!(Font::DoubleStruck.map('R'), 'ℝ');
        assert_eq!(Font::DoubleStruck.map('A'), '𝔸');
        assert_eq!(Font::DoubleStruck.map('1'), '𝟙');
        assert_eq!(Font::Script.map('L'), 'ℒ');
        assert_eq!(Font::Script.map('A'), '𝒜');
        assert_eq!(Font::Fraktur.map('g'), '𝔤');
        assert_eq!(Font::Italic.map('h'), 'ℎ');
        assert_eq!(Font::SansSerif.map('x'), '𝗑');
        assert_eq!(Font::Monospace.map('0'), '𝟶');
        // Outside the alphabet: unchanged.
        assert_eq!(Font::Bold.map('α'), 'α');
        assert_eq!(Font::Script.map('3'), '3');
        assert_eq!(Font::Upright.map('x'), 'x');
    }
}
