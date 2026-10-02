#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Token {
    Let,
    If,
    Else,
    In,
    Ident(String),
    StringLit(String),
    TemplateLit(String),
    NumberLit(f64),
    BoolLit(bool),
    NullLit,
    LParen,
    RParen,
    LBrace,
    RBrace,
    LBracket,
    RBracket,
    HashLBrace, // #{
    Dot,
    Comma,
    Semi,
    Colon,
    Question,
    QuestionDot,      // ?.
    QuestionQuestion, // ??
    Plus,
    Minus,
    Pipe, // |
    Or,   // ||
    And,  // &&
    Not,  // !
    Equal,
    NotEqual,
    Less,
    LessEqual,
    Greater,
    GreaterEqual,
    Assign,
    FatArrow,
}

pub(crate) fn tokenize(source: &str) -> Result<Vec<Token>, String> {
    let mut tokens = Vec::new();
    let mut chars = source.char_indices().peekable();

    while let Some(&(i, ch)) = chars.peek() {
        match ch {
            ' ' | '\t' | '\r' | '\n' => {
                chars.next();
            }
            '(' => {
                chars.next();
                tokens.push(Token::LParen);
            }
            ')' => {
                chars.next();
                tokens.push(Token::RParen);
            }
            '{' => {
                chars.next();
                tokens.push(Token::LBrace);
            }
            '}' => {
                chars.next();
                tokens.push(Token::RBrace);
            }
            '[' => {
                chars.next();
                tokens.push(Token::LBracket);
            }
            ']' => {
                chars.next();
                tokens.push(Token::RBracket);
            }
            '.' => {
                chars.next();
                tokens.push(Token::Dot);
            }
            ',' => {
                chars.next();
                tokens.push(Token::Comma);
            }
            ';' => {
                chars.next();
                tokens.push(Token::Semi);
            }
            ':' => {
                chars.next();
                tokens.push(Token::Colon);
            }
            '?' => {
                chars.next();
                if let Some(&(_, '.')) = chars.peek() {
                    chars.next();
                    tokens.push(Token::QuestionDot);
                } else if let Some(&(_, '?')) = chars.peek() {
                    chars.next();
                    tokens.push(Token::QuestionQuestion);
                } else {
                    tokens.push(Token::Question);
                }
            }
            '+' => {
                chars.next();
                tokens.push(Token::Plus);
            }
            '-' => {
                chars.next();
                tokens.push(Token::Minus);
            }
            '|' => {
                chars.next();
                if let Some(&(_, '|')) = chars.peek() {
                    chars.next();
                    tokens.push(Token::Or);
                } else {
                    tokens.push(Token::Pipe);
                }
            }
            '&' => {
                chars.next();
                if let Some(&(_, '&')) = chars.peek() {
                    chars.next();
                    tokens.push(Token::And);
                } else {
                    return Err(format!(
                        "unexpected character '&' at byte {i}; expected '&&'"
                    ));
                }
            }
            '!' => {
                chars.next();
                if let Some(&(_, '=')) = chars.peek() {
                    chars.next();
                    tokens.push(Token::NotEqual);
                } else {
                    tokens.push(Token::Not);
                }
            }
            '=' => {
                chars.next();
                if let Some(&(_, '=')) = chars.peek() {
                    chars.next();
                    tokens.push(Token::Equal);
                } else if let Some(&(_, '>')) = chars.peek() {
                    chars.next();
                    tokens.push(Token::FatArrow);
                } else {
                    tokens.push(Token::Assign);
                }
            }
            '<' => {
                chars.next();
                if let Some(&(_, '=')) = chars.peek() {
                    chars.next();
                    tokens.push(Token::LessEqual);
                } else {
                    tokens.push(Token::Less);
                }
            }
            '>' => {
                chars.next();
                if let Some(&(_, '=')) = chars.peek() {
                    chars.next();
                    tokens.push(Token::GreaterEqual);
                } else {
                    tokens.push(Token::Greater);
                }
            }
            '#' => {
                chars.next();
                if let Some(&(_, '{')) = chars.peek() {
                    chars.next();
                    tokens.push(Token::HashLBrace);
                } else {
                    return Err(format!("unexpected character '#' at byte {i}"));
                }
            }
            '`' => {
                chars.next();
                let mut s = String::new();
                let mut closed = false;
                while let Some((_, c)) = chars.next() {
                    if c == '\\' {
                        if let Some((_, escaped)) = chars.next() {
                            match escaped {
                                'n' => s.push('\n'),
                                'r' => s.push('\r'),
                                't' => s.push('\t'),
                                '\\' => s.push('\\'),
                                '`' => s.push('`'),
                                '{' => s.push_str("\\{"),
                                '}' => s.push_str("\\}"),
                                other => {
                                    s.push('\\');
                                    s.push(other);
                                }
                            }
                        }
                    } else if c == '`' {
                        closed = true;
                        break;
                    } else {
                        s.push(c);
                    }
                }
                if !closed {
                    return Err("unterminated template literal (`)".into());
                }
                tokens.push(Token::TemplateLit(s));
            }
            '"' => {
                chars.next();
                let mut s = String::new();
                let mut closed = false;
                while let Some((_, c)) = chars.next() {
                    if c == '\\' {
                        if let Some((_, escaped)) = chars.next() {
                            match escaped {
                                'n' => s.push('\n'),
                                'r' => s.push('\r'),
                                't' => s.push('\t'),
                                '\\' => s.push('\\'),
                                '"' => s.push('"'),
                                other => {
                                    s.push('\\');
                                    s.push(other);
                                }
                            }
                        }
                    } else if c == '"' {
                        closed = true;
                        break;
                    } else {
                        s.push(c);
                    }
                }
                if !closed {
                    return Err("unterminated string literal".into());
                }
                tokens.push(Token::StringLit(s));
            }
            '0'..='9' => {
                let start = i;
                let mut end = i;
                while let Some(&(j, c)) = chars.peek() {
                    if c.is_ascii_digit() || c == '.' {
                        end = j + c.len_utf8();
                        chars.next();
                    } else {
                        break;
                    }
                }
                let s = &source[start..end];
                let num: f64 = s.parse().map_err(|e| format!("invalid number: {e}"))?;
                tokens.push(Token::NumberLit(num));
            }
            'a'..='z' | 'A'..='Z' | '_' => {
                let start = i;
                let mut end = i;
                while let Some(&(j, c)) = chars.peek() {
                    if c.is_alphanumeric() || c == '_' {
                        end = j + c.len_utf8();
                        chars.next();
                    } else {
                        break;
                    }
                }
                let s = &source[start..end];
                match s {
                    "let" => tokens.push(Token::Let),
                    "if" => tokens.push(Token::If),
                    "else" => tokens.push(Token::Else),
                    "in" => tokens.push(Token::In),
                    "not" => tokens.push(Token::Not),
                    "true" => tokens.push(Token::BoolLit(true)),
                    "false" => tokens.push(Token::BoolLit(false)),
                    "null" => tokens.push(Token::NullLit),
                    _ => tokens.push(Token::Ident(s.to_string())),
                }
            }
            other => {
                return Err(format!("unexpected character {other:?} at byte {i}"));
            }
        }
    }
    Ok(tokens)
}
