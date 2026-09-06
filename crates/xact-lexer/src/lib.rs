//! Lexical analysis for Xact command lines (spec section 2: lexing is
//! native, deterministic Rust — never LLM inference).

use xact_ast::Span;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TokenKind {
    /// `£` — command sigil (spec section 7).
    Pound,
    /// `!` — policy sigil (spec section 8). Reserved past Phase 1.
    Bang,
    /// `@` — agent sigil (spec section 9). Reserved past Phase 1.
    At,
    /// Any bareword: verbs, ownership/reference keywords, `to`, `are`, and
    /// path-like operands. The parser resolves meaning from grammar
    /// position, not the lexer.
    Word(String),
    /// A `'...'` or `"..."` string literal, unquoted.
    StringLit(String),
    Eof,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Token {
    pub kind: TokenKind,
    pub span: Span,
}

pub fn tokenize(input: &str) -> Vec<Token> {
    let bytes = input.as_bytes();
    let len = bytes.len();
    let mut tokens = Vec::new();
    let mut chars = input.char_indices().peekable();

    while let Some(&(start, ch)) = chars.peek() {
        if ch.is_whitespace() {
            chars.next();
            continue;
        }

        match ch {
            '£' => {
                chars.next();
                let end = chars.peek().map(|&(i, _)| i).unwrap_or(len);
                tokens.push(Token {
                    kind: TokenKind::Pound,
                    span: Span::new(start, end),
                });
            }
            '!' => {
                chars.next();
                let end = chars.peek().map(|&(i, _)| i).unwrap_or(len);
                tokens.push(Token {
                    kind: TokenKind::Bang,
                    span: Span::new(start, end),
                });
            }
            '@' => {
                chars.next();
                let end = chars.peek().map(|&(i, _)| i).unwrap_or(len);
                tokens.push(Token {
                    kind: TokenKind::At,
                    span: Span::new(start, end),
                });
            }
            '\'' | '"' => {
                let quote = ch;
                chars.next();
                let content_start = chars.peek().map(|&(i, _)| i).unwrap_or(len);
                let content_end;
                loop {
                    match chars.peek() {
                        Some(&(i, c)) if c == quote => {
                            content_end = i;
                            chars.next();
                            break;
                        }
                        Some(_) => {
                            chars.next();
                        }
                        None => {
                            content_end = len;
                            break;
                        }
                    }
                }
                let end = chars.peek().map(|&(i, _)| i).unwrap_or(len);
                tokens.push(Token {
                    kind: TokenKind::StringLit(input[content_start..content_end].to_string()),
                    span: Span::new(start, end),
                });
            }
            _ => {
                let word_start = start;
                let mut word_end = start;
                while let Some(&(i, c)) = chars.peek() {
                    if c.is_whitespace() || matches!(c, '£' | '!' | '@' | '\'' | '"') {
                        break;
                    }
                    word_end = i + c.len_utf8();
                    chars.next();
                }
                tokens.push(Token {
                    kind: TokenKind::Word(input[word_start..word_end].to_string()),
                    span: Span::new(word_start, word_end),
                });
            }
        }
    }

    tokens.push(Token {
        kind: TokenKind::Eof,
        span: Span::new(len, len),
    });
    tokens
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tokenizes_simple_see_command() {
        let tokens = tokenize("£ SEE MY ~/Documents");
        let kinds: Vec<_> = tokens.into_iter().map(|t| t.kind).collect();
        assert_eq!(
            kinds,
            vec![
                TokenKind::Pound,
                TokenKind::Word("SEE".into()),
                TokenKind::Word("MY".into()),
                TokenKind::Word("~/Documents".into()),
                TokenKind::Eof,
            ]
        );
    }

    #[test]
    fn tokenizes_string_literal() {
        let tokens = tokenize("£ THEY are \"alice,bob\"");
        let kinds: Vec<_> = tokens.into_iter().map(|t| t.kind).collect();
        assert_eq!(
            kinds,
            vec![
                TokenKind::Pound,
                TokenKind::Word("THEY".into()),
                TokenKind::Word("are".into()),
                TokenKind::StringLit("alice,bob".into()),
                TokenKind::Eof,
            ]
        );
    }

    #[test]
    fn tokenizes_copy_with_destination() {
        let tokens = tokenize("£ COPY THAT to OUR ~/backup");
        let kinds: Vec<_> = tokens.into_iter().map(|t| t.kind).collect();
        assert_eq!(
            kinds,
            vec![
                TokenKind::Pound,
                TokenKind::Word("COPY".into()),
                TokenKind::Word("THAT".into()),
                TokenKind::Word("to".into()),
                TokenKind::Word("OUR".into()),
                TokenKind::Word("~/backup".into()),
                TokenKind::Eof,
            ]
        );
    }
}
