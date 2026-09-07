//! A small, **safe** textual formula parser.
//!
//! Higher-level systems (e.g. Ferris CMS) can store formulas as strings such as
//! `quantity * unit_price - COALESCE(discount, 0)` and translate them into typed
//! [`Node`]s at runtime. This parser is deliberately restricted:
//!
//! * identifiers are resolved **only** against the columns of the entity being
//!   queried (discovered through the generated `Column` types — no reflection,
//!   no table names, no raw SQL text is ever emitted from user input),
//! * the supported grammar is arithmetic, comparisons, boolean `AND`/`OR`/`NOT`,
//!   parentheses, numeric/string literals, and a whitelist of scalar functions
//!   (`COALESCE`, `NULLIF`, `ABS`, `LOWER`, `UPPER`, `LENGTH`).
//!
//! Relationship aggregates (`SUM(sales.total)`) are *not* accepted by this
//! scalar parser; use the typed [`crate::formula::Node`] builder for those so
//! the relationship is validated against real SeaORM metadata.
#![allow(
    clippy::collapsible_if,
    clippy::unnecessary_map_or,
    clippy::while_let_loop
)]

use crate::{
    EntityTrait, IdenStatic, Iterable,
    formula::{Node, error::FormulaError},
};
use std::str::FromStr;

/// Tokenizer and parser producing a [`Node`] bound to the columns of `E`.
pub fn parse<E>(input: &str) -> crate::formula::Result<Node>
where
    E: EntityTrait + 'static,
{
    let tokens = Tokenizer::new(input).tokenize()?;
    let mut p = Parser {
        tokens,
        pos: 0,
        _e: std::marker::PhantomData::<E>,
    };
    let node = p.parse_or()?;
    if p.pos != p.tokens.len() {
        return Err(FormulaError::new(format!(
            "unexpected trailing input: `{}`",
            p.tokens[p.pos..]
                .iter()
                .map(|t| t.text.clone())
                .collect::<Vec<_>>()
                .join(" ")
        )));
    }
    Ok(node)
}

/// Resolve a bare identifier against the columns of `E`.
fn resolve_column<E>(name: &str) -> crate::formula::Result<Node>
where
    E: EntityTrait + 'static,
{
    for col in E::Column::iter() {
        if col.as_str() == name {
            return Ok(Node::column(col));
        }
    }
    Err(FormulaError::new(format!(
        "unknown column `{name}` for entity `{}`",
        std::any::type_name::<E>()
    )))
}

#[derive(Debug, Clone, PartialEq)]
enum TokKind {
    Num,
    Str,
    Ident,
    // operators / punctuation
    Plus,
    Minus,
    Star,
    Slash,
    LParen,
    RParen,
    Comma,
    Eq,
    Ne,
    Lt,
    Le,
    Gt,
    Ge,
}

#[derive(Debug, Clone)]
struct Tok {
    kind: TokKind,
    text: String,
}

struct Tokenizer<'a> {
    chars: std::iter::Peekable<std::str::Chars<'a>>,
}

impl<'a> Tokenizer<'a> {
    fn new(s: &'a str) -> Self {
        Self {
            chars: s.chars().peekable(),
        }
    }

    fn tokenize(mut self) -> crate::formula::Result<Vec<Tok>> {
        let mut out = Vec::new();
        while let Some(&c) = self.chars.peek() {
            if c.is_whitespace() {
                self.chars.next();
            } else if c.is_ascii_digit()
                || (c == '.'
                    && self
                        .chars
                        .clone()
                        .nth(1)
                        .map_or(false, |d| d.is_ascii_digit()))
            {
                out.push(self.number());
            } else if c == '\'' {
                out.push(self.string()?);
            } else if c.is_ascii_alphabetic() || c == '_' {
                out.push(self.ident());
            } else {
                self.chars.next();
                let kind = match c {
                    '+' => TokKind::Plus,
                    '-' => TokKind::Minus,
                    '*' => TokKind::Star,
                    '/' => TokKind::Slash,
                    '(' => TokKind::LParen,
                    ')' => TokKind::RParen,
                    ',' => TokKind::Comma,
                    '=' => TokKind::Eq,
                    '<' => {
                        if self.chars.peek() == Some(&'=') {
                            self.chars.next();
                            TokKind::Le
                        } else if self.chars.peek() == Some(&'>') {
                            self.chars.next();
                            TokKind::Ne
                        } else {
                            TokKind::Lt
                        }
                    }
                    '>' => {
                        if self.chars.peek() == Some(&'=') {
                            self.chars.next();
                            TokKind::Ge
                        } else {
                            TokKind::Gt
                        }
                    }
                    other => {
                        return Err(FormulaError::new(format!(
                            "unexpected character `{other}` in formula"
                        )));
                    }
                };
                out.push(Tok {
                    kind,
                    text: c.to_string(),
                });
            }
        }
        Ok(out)
    }

    fn number(&mut self) -> Tok {
        let mut s = String::new();
        while let Some(&c) = self.chars.peek() {
            if c.is_ascii_digit() || c == '.' {
                s.push(c);
                self.chars.next();
            } else {
                break;
            }
        }
        Tok {
            kind: TokKind::Num,
            text: s,
        }
    }

    fn string(&mut self) -> crate::formula::Result<Tok> {
        self.chars.next(); // opening quote
        let mut s = String::new();
        for c in self.chars.by_ref() {
            if c == '\'' {
                return Ok(Tok {
                    kind: TokKind::Str,
                    text: s,
                });
            }
            s.push(c);
        }
        Err(FormulaError::new("unterminated string literal"))
    }

    fn ident(&mut self) -> Tok {
        let mut s = String::new();
        while let Some(&c) = self.chars.peek() {
            if c.is_ascii_alphanumeric() || c == '_' {
                s.push(c);
                self.chars.next();
            } else {
                break;
            }
        }
        Tok {
            kind: TokKind::Ident,
            text: s,
        }
    }
}

struct Parser<E> {
    tokens: Vec<Tok>,
    pos: usize,
    _e: std::marker::PhantomData<E>,
}

impl<E> Parser<E>
where
    E: EntityTrait + 'static,
{
    fn peek(&self) -> Option<&Tok> {
        self.tokens.get(self.pos)
    }

    fn bump(&mut self) -> Option<Tok> {
        let t = self.tokens.get(self.pos).cloned();
        if t.is_some() {
            self.pos += 1;
        }
        t
    }

    fn expect(&mut self, kind: &TokKind, what: &str) -> crate::formula::Result<Tok> {
        match self.bump() {
            Some(t) if &t.kind == kind => Ok(t),
            other => Err(FormulaError::new(format!(
                "expected {what}, found `{}`",
                other.map(|t| t.text).unwrap_or_else(|| "<end>".into())
            ))),
        }
    }

    fn parse_or(&mut self) -> crate::formula::Result<Node> {
        let mut left = self.parse_and()?;
        while self.peek().map_or(false, |t| {
            t.kind == TokKind::Ident && t.text.eq_ignore_ascii_case("or")
        }) {
            self.bump();
            let right = self.parse_and()?;
            left = left.or(right);
        }
        Ok(left)
    }

    fn parse_and(&mut self) -> crate::formula::Result<Node> {
        let mut left = self.parse_cmp()?;
        while self.peek().map_or(false, |t| {
            t.kind == TokKind::Ident && t.text.eq_ignore_ascii_case("and")
        }) {
            self.bump();
            let right = self.parse_cmp()?;
            left = left.and(right);
        }
        Ok(left)
    }

    fn parse_cmp(&mut self) -> crate::formula::Result<Node> {
        let left = self.parse_add()?;
        let kind = match self.peek().map(|t| t.kind.clone()) {
            Some(
                k @ (TokKind::Eq
                | TokKind::Ne
                | TokKind::Lt
                | TokKind::Le
                | TokKind::Gt
                | TokKind::Ge),
            ) => {
                self.bump();
                k
            }
            _ => return Ok(left),
        };
        let right = self.parse_add()?;
        Ok(match kind {
            TokKind::Eq => left.eq(right),
            TokKind::Ne => left.neq(right),
            TokKind::Lt => left.lt(right),
            TokKind::Le => left.lte(right),
            TokKind::Gt => left.gt(right),
            TokKind::Ge => left.gte(right),
            _ => unreachable!(),
        })
    }

    fn parse_add(&mut self) -> crate::formula::Result<Node> {
        let mut left = self.parse_mul()?;
        loop {
            let kind = match self.peek().map(|t| t.kind.clone()) {
                Some(k @ (TokKind::Plus | TokKind::Minus)) => {
                    self.bump();
                    k
                }
                _ => break,
            };
            let right = self.parse_mul()?;
            left = if kind == TokKind::Plus {
                left.add(right)
            } else {
                left.sub(right)
            };
        }
        Ok(left)
    }

    fn parse_mul(&mut self) -> crate::formula::Result<Node> {
        let mut left = self.parse_unary()?;
        loop {
            let kind = match self.peek().map(|t| t.kind.clone()) {
                Some(k @ (TokKind::Star | TokKind::Slash)) => {
                    self.bump();
                    k
                }
                _ => break,
            };
            let right = self.parse_unary()?;
            left = if kind == TokKind::Star {
                left.mul(right)
            } else {
                left.div(right)
            };
        }
        Ok(left)
    }

    fn parse_unary(&mut self) -> crate::formula::Result<Node> {
        if let Some(kind) = self.peek().map(|t| t.kind.clone()) {
            if kind == TokKind::Minus {
                self.bump();
                return Ok(self.parse_unary()?.neg());
            }
        }
        if let Some(text) = self.peek().map(|t| t.text.clone()) {
            if text.eq_ignore_ascii_case("not") {
                self.bump();
                return Ok(self.parse_unary()?.not());
            }
        }
        self.parse_primary()
    }

    fn parse_primary(&mut self) -> crate::formula::Result<Node> {
        let tok = self
            .bump()
            .ok_or_else(|| FormulaError::new("unexpected end of formula"))?;
        match tok.kind {
            TokKind::Num => self.number_lit(&tok),
            TokKind::Str => Ok(Node::str(&tok.text)),
            TokKind::LParen => {
                let inner = self.parse_or()?;
                self.expect(&TokKind::RParen, "`)`")?;
                Ok(inner)
            }
            TokKind::Ident => {
                // function call?
                if self.peek().map_or(false, |t| t.kind == TokKind::LParen) {
                    self.bump(); // '('
                    let args = self.parse_args()?;
                    self.call(&tok.text, args)
                } else if tok.text.eq_ignore_ascii_case("true") {
                    Ok(Node::boolean(true))
                } else if tok.text.eq_ignore_ascii_case("false") {
                    Ok(Node::boolean(false))
                } else if tok.text.eq_ignore_ascii_case("today")
                    || tok.text.eq_ignore_ascii_case("current_date")
                {
                    Ok(Node::today())
                } else {
                    resolve_column::<E>(&tok.text)
                }
            }
            _ => Err(FormulaError::new(format!(
                "unexpected token `{}`",
                tok.text
            ))),
        }
    }

    fn number_lit(&self, tok: &Tok) -> crate::formula::Result<Node> {
        if tok.text.contains('.') {
            #[cfg(feature = "with-rust_decimal")]
            {
                Node::decimal_str(&tok.text)
            }
            #[cfg(not(feature = "with-rust_decimal"))]
            {
                Err(FormulaError::new(
                    "decimal literals require the `with-rust_decimal` feature",
                ))
            }
        } else {
            let v: i64 = i64::from_str(&tok.text)
                .map_err(|_| FormulaError::new(format!("invalid integer `{}`", tok.text)))?;
            Ok(Node::int(v))
        }
    }

    fn parse_args(&mut self) -> crate::formula::Result<Vec<Node>> {
        let mut args = Vec::new();
        if self.peek().map_or(false, |t| t.kind == TokKind::RParen) {
            self.bump();
            return Ok(args);
        }
        loop {
            args.push(self.parse_or()?);
            match self.peek().map(|t| t.kind.clone()) {
                Some(TokKind::Comma) => {
                    self.bump();
                }
                Some(TokKind::RParen) => {
                    self.bump();
                    break;
                }
                _ => return Err(FormulaError::new("expected `,` or `)` in function call")),
            }
        }
        Ok(args)
    }

    fn call(&self, name: &str, args: Vec<Node>) -> crate::formula::Result<Node> {
        let upper = name.to_uppercase();
        match upper.as_str() {
            "COALESCE" => Node::coalesce(args),
            "NULLIF" if args.len() == 2 => Ok(Node::nullif(args[0].clone(), args[1].clone())),
            "ABS" if args.len() == 1 => Ok(args[0].clone().abs()),
            "LOWER" if args.len() == 1 => Ok(args[0].clone().lower()),
            "UPPER" if args.len() == 1 => Ok(args[0].clone().upper()),
            "LENGTH" if args.len() == 1 => Ok(args[0].clone().length()),
            "NULLIF" | "ABS" | "LOWER" | "UPPER" | "LENGTH" => Err(FormulaError::new(format!(
                "{name} expects a different number of arguments"
            ))),
            _ => Err(FormulaError::new(format!(
                "unknown function `{name}` (whitelist: COALESCE, NULLIF, ABS, LOWER, UPPER, LENGTH)"
            ))),
        }
    }
}
