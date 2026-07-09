use crate::{Token, TokenKind};

pub(crate) fn html(src: &str) -> Vec<Token<'_>> {
    if src.is_empty() {
        Vec::new()
    } else {
        vec![Token {
            text: src,
            kind: TokenKind::Plain,
        }]
    }
}

pub(crate) fn yaml(src: &str) -> Vec<Token<'_>> {
    html(src)
}
