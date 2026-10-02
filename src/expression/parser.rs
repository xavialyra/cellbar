use super::ast::{Ast, BinaryOp, Expr, Stmt, TemplateSegment};
use super::lexer::{Token, tokenize};
use super::value::DynValue;

fn parse_template_segments(raw: &str) -> Result<Vec<TemplateSegment>, String> {
    let mut segments = Vec::new();
    let mut current_lit = String::new();
    let chars: Vec<char> = raw.chars().collect();
    let len = chars.len();
    let mut i = 0;

    while i < len {
        if chars[i] == '\\' && i + 1 < len && (chars[i + 1] == '{' || chars[i + 1] == '}') {
            current_lit.push(chars[i + 1]);
            i += 2;
            continue;
        }

        // ${expr} explicit interpolation
        if chars[i] == '$' && i + 1 < len && chars[i + 1] == '{' {
            let start = i + 2;
            let mut end = start;
            let mut depth = 1;
            while end < len {
                if chars[end] == '{' {
                    depth += 1;
                } else if chars[end] == '}' {
                    depth -= 1;
                    if depth == 0 {
                        break;
                    }
                }
                end += 1;
            }
            if depth != 0 {
                return Err("unclosed '${' in template literal".into());
            }
            if !current_lit.is_empty() {
                segments.push(TemplateSegment::Literal(std::mem::take(&mut current_lit)));
            }
            let inner: String = chars[start..end].iter().collect();
            let expr = compile_single_expr(inner.trim())?;
            segments.push(TemplateSegment::Expr(expr));
            i = end + 1;
            continue;
        }

        // {expr} interpolation (if contains no '{', '[', ']', or whitespace, and compiles as expr)
        if chars[i] == '{' {
            let start = i + 1;
            let mut end = start;
            let mut has_bracket_or_space = false;
            while end < len && chars[end] != '}' && chars[end] != '{' {
                if chars[end] == '[' || chars[end] == ']' || chars[end].is_whitespace() {
                    has_bracket_or_space = true;
                    break;
                }
                end += 1;
            }
            if !has_bracket_or_space && end < len && chars[end] == '}' && end > start {
                let inner: String = chars[start..end].iter().collect();
                if let Ok(expr) = compile_single_expr(inner.trim()) {
                    if !current_lit.is_empty() {
                        segments.push(TemplateSegment::Literal(std::mem::take(&mut current_lit)));
                    }
                    segments.push(TemplateSegment::Expr(expr));
                    i = end + 1;
                    continue;
                }
            }
        }

        current_lit.push(chars[i]);
        i += 1;
    }

    if !current_lit.is_empty() {
        segments.push(TemplateSegment::Literal(current_lit));
    }
    Ok(segments)
}

fn compile_single_expr(source: &str) -> Result<Expr, String> {
    let tokens = tokenize(source)?;
    let mut parser = Parser::new(tokens);
    let expr = parser.parse_expr()?;
    if parser.peek().is_some() {
        return Err(format!(
            "unexpected extra token after expression in {source:?}"
        ));
    }
    Ok(expr)
}

pub(crate) struct Parser {
    tokens: Vec<Token>,
    pos: usize,
}

impl Parser {
    pub(crate) fn new(tokens: Vec<Token>) -> Self {
        Self { tokens, pos: 0 }
    }

    pub(crate) fn peek(&self) -> Option<&Token> {
        self.tokens.get(self.pos)
    }

    fn next_token(&mut self) -> Option<Token> {
        if self.pos < self.tokens.len() {
            let t = self.tokens[self.pos].clone();
            self.pos += 1;
            Some(t)
        } else {
            None
        }
    }

    fn expect(&mut self, expected: Token) -> Result<(), String> {
        let got = self.next_token();
        if got == Some(expected.clone()) {
            Ok(())
        } else {
            Err(format!("expected {expected:?}, got {got:?}"))
        }
    }

    pub(crate) fn parse_program(&mut self) -> Result<Ast, String> {
        let mut statements = Vec::new();
        while self.peek().is_some() {
            statements.push(self.parse_statement()?);
        }
        if statements.is_empty() {
            return Err("expression must not be empty".to_owned());
        }
        Ok(Ast::new(statements))
    }

    fn parse_statement(&mut self) -> Result<Stmt, String> {
        if let Some(Token::Let) = self.peek() {
            self.next_token();
            let name = match self.next_token() {
                Some(Token::Ident(id)) => id,
                other => return Err(format!("expected variable name, got {other:?}")),
            };
            self.expect(Token::Assign)?;
            let expr = self.parse_expr()?;
            if let Some(Token::Semi) = self.peek() {
                self.next_token();
            }
            Ok(Stmt::Let(name, expr))
        } else {
            let expr = self.parse_expr()?;
            if let Some(Token::Semi) = self.peek() {
                self.next_token();
            }
            Ok(Stmt::Expr(expr))
        }
    }

    pub(crate) fn parse_expr(&mut self) -> Result<Expr, String> {
        self.parse_ternary_or_if()
    }

    fn parse_ternary_or_if(&mut self) -> Result<Expr, String> {
        if let Some(Token::If) = self.peek() {
            self.next_token();
            let condition = self.parse_or()?;
            self.expect(Token::LBrace)?;
            let then_branch = self.parse_block_or_expr_until_rbrace()?;
            let else_branch = if let Some(Token::Else) = self.peek() {
                self.next_token();
                if let Some(Token::If) = self.peek() {
                    Some(self.parse_ternary_or_if()?)
                } else {
                    self.expect(Token::LBrace)?;
                    Some(self.parse_block_or_expr_until_rbrace()?)
                }
            } else {
                None
            };
            return Ok(Expr::If {
                condition: Box::new(condition),
                then_branch: Box::new(then_branch),
                else_branch: else_branch.map(Box::new),
            });
        }

        let expr = self.parse_coalesce()?;
        if let Some(Token::Question) = self.peek() {
            self.next_token();
            let then_branch = self.parse_expr()?;
            self.expect(Token::Colon)?;
            let else_branch = self.parse_expr()?;
            return Ok(Expr::If {
                condition: Box::new(expr),
                then_branch: Box::new(then_branch),
                else_branch: Some(Box::new(else_branch)),
            });
        }
        Ok(expr)
    }

    fn parse_block_or_expr_until_rbrace(&mut self) -> Result<Expr, String> {
        let mut statements = Vec::new();
        while let Some(token) = self.peek() {
            if token == &Token::RBrace {
                self.next_token();
                break;
            }
            statements.push(self.parse_statement()?);
        }
        match statements.as_slice() {
            [] => Ok(Expr::Literal(DynValue::Null)),
            [Stmt::Expr(e)] => Ok(e.clone()),
            _ => Ok(Expr::Block(statements)),
        }
    }

    fn parse_map_or_block(&mut self) -> Result<Expr, String> {
        if let Some(Token::RBrace) = self.peek() {
            self.next_token();
            return Ok(Expr::Map(Vec::new()));
        }
        if let Some(Token::Let) = self.peek() {
            return self.parse_block_or_expr_until_rbrace();
        }
        let looks_like_map = matches!(
            (self.peek(), self.tokens.get(self.pos + 1)),
            (
                Some(Token::Ident(_)),
                Some(Token::Colon | Token::Comma | Token::RBrace)
            ) | (Some(Token::StringLit(_)), Some(Token::Colon))
        );
        if looks_like_map {
            self.parse_map_entries()
        } else {
            self.parse_block_or_expr_until_rbrace()
        }
    }

    fn parse_map_entries(&mut self) -> Result<Expr, String> {
        let mut entries = Vec::new();
        loop {
            let (key, is_ident) = match self.next_token() {
                Some(Token::Ident(id)) => (id, true),
                Some(Token::StringLit(s)) => (s, false),
                other => {
                    return Err(format!(
                        "expected map key (identifier or string), got {other:?}"
                    ));
                }
            };

            let val = if is_ident && matches!(self.peek(), Some(Token::Comma) | Some(Token::RBrace))
            {
                Expr::Var(key.clone())
            } else {
                self.expect(Token::Colon)?;
                self.parse_expr()?
            };

            entries.push((key, val));
            match self.peek() {
                Some(Token::Comma) => {
                    self.next_token();
                    if let Some(Token::RBrace) = self.peek() {
                        self.next_token();
                        break;
                    }
                }
                Some(Token::RBrace) => {
                    self.next_token();
                    break;
                }
                other => return Err(format!("expected ',' or '}}' in map, got {other:?}")),
            }
        }
        Ok(Expr::Map(entries))
    }

    fn parse_coalesce(&mut self) -> Result<Expr, String> {
        let mut left = self.parse_or()?;
        while let Some(Token::QuestionQuestion) = self.peek() {
            self.next_token();
            let right = self.parse_or()?;
            left = Expr::Binary {
                op: BinaryOp::Coalesce,
                left: Box::new(left),
                right: Box::new(right),
            };
        }
        Ok(left)
    }

    fn parse_or(&mut self) -> Result<Expr, String> {
        let mut left = self.parse_and()?;
        while let Some(Token::Or) = self.peek() {
            self.next_token();
            let right = self.parse_and()?;
            left = Expr::Binary {
                op: BinaryOp::Or,
                left: Box::new(left),
                right: Box::new(right),
            };
        }
        Ok(left)
    }

    fn parse_and(&mut self) -> Result<Expr, String> {
        let mut left = self.parse_equality()?;
        while let Some(Token::And) = self.peek() {
            self.next_token();
            let right = self.parse_equality()?;
            left = Expr::Binary {
                op: BinaryOp::And,
                left: Box::new(left),
                right: Box::new(right),
            };
        }
        Ok(left)
    }

    fn parse_equality(&mut self) -> Result<Expr, String> {
        let mut left = self.parse_comparison()?;
        while let Some(token) = self.peek() {
            match token {
                Token::Equal => {
                    self.next_token();
                    let right = self.parse_comparison()?;
                    left = Expr::Binary {
                        op: BinaryOp::Equal,
                        left: Box::new(left),
                        right: Box::new(right),
                    };
                }
                Token::NotEqual => {
                    self.next_token();
                    let right = self.parse_comparison()?;
                    left = Expr::Binary {
                        op: BinaryOp::NotEqual,
                        left: Box::new(left),
                        right: Box::new(right),
                    };
                }
                _ => break,
            }
        }
        Ok(left)
    }

    fn parse_comparison(&mut self) -> Result<Expr, String> {
        let mut left = self.parse_add_sub()?;
        while let Some(token) = self.peek() {
            let op = match token {
                Token::Less => BinaryOp::Less,
                Token::LessEqual => BinaryOp::LessEqual,
                Token::Greater => BinaryOp::Greater,
                Token::GreaterEqual => BinaryOp::GreaterEqual,
                Token::In => BinaryOp::In,
                Token::Not => {
                    if self.tokens.get(self.pos + 1) == Some(&Token::In) {
                        self.next_token();
                        self.next_token();
                        let right = self.parse_add_sub()?;
                        left = Expr::Binary {
                            op: BinaryOp::NotIn,
                            left: Box::new(left),
                            right: Box::new(right),
                        };
                        continue;
                    } else {
                        break;
                    }
                }
                _ => break,
            };
            self.next_token();
            let right = self.parse_add_sub()?;
            left = Expr::Binary {
                op,
                left: Box::new(left),
                right: Box::new(right),
            };
        }
        Ok(left)
    }

    fn parse_add_sub(&mut self) -> Result<Expr, String> {
        let mut left = self.parse_unary()?;
        while let Some(token) = self.peek() {
            match token {
                Token::Plus => {
                    self.next_token();
                    let right = self.parse_unary()?;
                    left = Expr::Binary {
                        op: BinaryOp::Add,
                        left: Box::new(left),
                        right: Box::new(right),
                    };
                }
                Token::Minus => {
                    self.next_token();
                    let right = self.parse_unary()?;
                    left = Expr::Binary {
                        op: BinaryOp::Sub,
                        left: Box::new(left),
                        right: Box::new(right),
                    };
                }
                _ => break,
            }
        }
        Ok(left)
    }

    fn parse_unary(&mut self) -> Result<Expr, String> {
        if let Some(Token::Not) = self.peek() {
            self.next_token();
            let inner = self.parse_unary()?;
            return Ok(Expr::UnaryNot(Box::new(inner)));
        }
        if let Some(Token::Minus) = self.peek() {
            self.next_token();
            let inner = self.parse_unary()?;
            return Ok(Expr::UnaryNeg(Box::new(inner)));
        }
        self.parse_postfix()
    }

    fn parse_postfix(&mut self) -> Result<Expr, String> {
        let mut expr = self.parse_primary()?;
        loop {
            match self.peek() {
                Some(Token::Dot) => {
                    self.next_token();
                    let field = match self.next_token() {
                        Some(Token::Ident(id)) => id,
                        other => {
                            return Err(format!("expected field name after '.', got {other:?}"));
                        }
                    };
                    if let Some(Token::LParen) = self.peek() {
                        self.next_token();
                        let args = self.parse_args()?;
                        expr = Expr::MethodCall {
                            target: Box::new(expr),
                            method: field,
                            args,
                            optional: false,
                        };
                    } else {
                        expr = Expr::FieldAccess {
                            target: Box::new(expr),
                            field,
                            optional: false,
                        };
                    }
                }
                Some(Token::QuestionDot) => {
                    self.next_token();
                    if let Some(Token::LBracket) = self.peek() {
                        self.next_token();
                        let index = self.parse_expr()?;
                        self.expect(Token::RBracket)?;
                        expr = Expr::IndexAccess {
                            target: Box::new(expr),
                            index: Box::new(index),
                            optional: true,
                        };
                    } else {
                        let field = match self.next_token() {
                            Some(Token::Ident(id)) => id,
                            other => {
                                return Err(format!(
                                    "expected field name after '?.', got {other:?}"
                                ));
                            }
                        };
                        if let Some(Token::LParen) = self.peek() {
                            self.next_token();
                            let args = self.parse_args()?;
                            expr = Expr::MethodCall {
                                target: Box::new(expr),
                                method: field,
                                args,
                                optional: true,
                            };
                        } else {
                            expr = Expr::FieldAccess {
                                target: Box::new(expr),
                                field,
                                optional: true,
                            };
                        }
                    }
                }
                Some(Token::LBracket) => {
                    self.next_token();
                    let index = self.parse_expr()?;
                    self.expect(Token::RBracket)?;
                    expr = Expr::IndexAccess {
                        target: Box::new(expr),
                        index: Box::new(index),
                        optional: false,
                    };
                }
                _ => break,
            }
        }
        Ok(expr)
    }

    fn parse_args(&mut self) -> Result<Vec<Expr>, String> {
        let mut args = Vec::new();
        if let Some(Token::RParen) = self.peek() {
            self.next_token();
            return Ok(args);
        }
        loop {
            args.push(self.parse_expr()?);
            match self.peek() {
                Some(Token::Comma) => {
                    self.next_token();
                }
                Some(Token::RParen) => {
                    self.next_token();
                    break;
                }
                other => return Err(format!("expected ',' or ')', got {other:?}")),
            }
        }
        Ok(args)
    }

    fn parse_primary(&mut self) -> Result<Expr, String> {
        // Lambda syntax: |param1, param2| body or || body
        if let Some(Token::Or) = self.peek() {
            self.next_token(); // empty param closure || expr
            let body = self.parse_expr()?;
            return Ok(Expr::Lambda {
                params: Vec::new(),
                body: Box::new(body),
            });
        }

        if let Some(Token::Pipe) = self.peek() {
            self.next_token();
            let mut params = Vec::new();
            if let Some(Token::Pipe) = self.peek() {
                self.next_token();
            } else {
                loop {
                    let param = match self.next_token() {
                        Some(Token::Ident(id)) => id,
                        other => {
                            return Err(format!("expected lambda parameter name, got {other:?}"));
                        }
                    };
                    params.push(param);
                    match self.peek() {
                        Some(Token::Comma) => {
                            self.next_token();
                        }
                        Some(Token::Pipe) => {
                            self.next_token();
                            break;
                        }
                        other => {
                            return Err(format!("expected ',' or '|' in lambda, got {other:?}"));
                        }
                    }
                }
            }
            let body = self.parse_expr()?;
            return Ok(Expr::Lambda {
                params,
                body: Box::new(body),
            });
        }

        match self.next_token() {
            Some(Token::StringLit(s)) => Ok(Expr::Literal(DynValue::String(s))),
            Some(Token::TemplateLit(s)) => {
                let segments = parse_template_segments(&s)?;
                Ok(Expr::Template(segments))
            }
            Some(Token::NumberLit(n)) => Ok(Expr::Literal(DynValue::Number(n))),
            Some(Token::BoolLit(b)) => Ok(Expr::Literal(DynValue::Bool(b))),
            Some(Token::NullLit) => Ok(Expr::Literal(DynValue::Null)),
            Some(Token::LBracket) => {
                let mut items = Vec::new();
                if let Some(Token::RBracket) = self.peek() {
                    self.next_token();
                    return Ok(Expr::List(items));
                }
                loop {
                    items.push(self.parse_expr()?);
                    match self.peek() {
                        Some(Token::Comma) => {
                            self.next_token();
                            if let Some(Token::RBracket) = self.peek() {
                                self.next_token();
                                break;
                            }
                        }
                        Some(Token::RBracket) => {
                            self.next_token();
                            break;
                        }
                        other => return Err(format!("expected ',' or ']' in list, got {other:?}")),
                    }
                }
                Ok(Expr::List(items))
            }
            Some(Token::HashLBrace) | Some(Token::LBrace) => self.parse_map_or_block(),
            Some(Token::LParen) => {
                let expr = self.parse_expr()?;
                self.expect(Token::RParen)?;
                Ok(expr)
            }
            Some(Token::Ident(id)) => {
                if let Some(Token::LParen) = self.peek() {
                    self.next_token();
                    let args = self.parse_args()?;
                    Ok(Expr::Call { func: id, args })
                } else if let Some(Token::FatArrow) = self.peek() {
                    self.next_token(); // id => expr
                    let body = self.parse_expr()?;
                    Ok(Expr::Lambda {
                        params: vec![id],
                        body: Box::new(body),
                    })
                } else {
                    Ok(Expr::Var(id))
                }
            }
            other => Err(format!("unexpected token {other:?}")),
        }
    }
}

pub fn compile(source: &str) -> Result<Ast, String> {
    let trimmed = source.trim();
    if trimmed.is_empty() {
        return Err("expression must not be empty".to_owned());
    }
    let tokens = tokenize(trimmed)?;
    let mut parser = Parser::new(tokens);
    parser.parse_program()
}
