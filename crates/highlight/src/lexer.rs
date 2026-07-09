use crate::{Token, TokenKind};

pub(crate) struct LexerSpec;

pub(crate) fn tokenize<'a>(src: &'a str, _spec: &LexerSpec) -> Vec<Token<'a>> {
    if src.is_empty() {
        Vec::new()
    } else {
        vec![Token {
            text: src,
            kind: TokenKind::Plain,
        }]
    }
}
