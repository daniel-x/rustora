// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The rustora authors. Ported from OpenRA, Copyright (c) The OpenRA Developers and Contributors.

//! Port of `OpenRA.Game/Support/VariableExpression.cs`: condition expressions such as
//! `!disabled && (veteran || rank > 2)` evaluated against integer variables.

use std::collections::HashMap;
use std::rc::Rc;

use indexmap::IndexSet;

use crate::fieldloader::{ArrayAware, FieldValue, LoadError, invalid};
use crate::miniyaml::MiniYaml;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum CharClass {
    Whitespace,
    Operator,
    Mixed,
    Id,
    Digit,
}

fn char_class(c: char) -> CharClass {
    match c {
        '~' | '!' | '%' | '^' | '&' | '*' | '(' | ')' | '+' | '=' | '[' | ']' | '{' | '}' | '|' | ':' | ';' | '\'' | '"' | '<' | '>'
        | '?' | ',' | '/' => CharClass::Operator,
        '.' | '$' | '-' | '@' => CharClass::Mixed,
        '0'..='9' => CharClass::Digit,
        ' ' | '\t' | '\n' | '\r' => CharClass::Whitespace,
        c if c.is_whitespace() => CharClass::Whitespace,
        _ => CharClass::Id,
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Tok {
    False,
    True,
    Number,
    Variable,
    OpenParen,
    CloseParen,
    Not,
    Negate,
    OnesComplement,
    And,
    Or,
    Equals,
    NotEquals,
    LessThan,
    LessThanOrEqual,
    GreaterThan,
    GreaterThanOrEqual,
    Add,
    Subtract,
    Multiply,
    Divide,
    Modulo,
    Invalid,
}

const LEFT: u8 = 1;
const RIGHT: u8 = 2;
const BOTH: u8 = 3;

struct TokInfo {
    symbol: &'static str,
    precedence: i32,
    operand_sides: u8,
    whitespace_sides: u8,
    right_assoc: bool,
    opens: bool,
    closes: bool,
}

fn info(t: Tok) -> TokInfo {
    let v = |symbol, precedence| TokInfo {
        symbol,
        precedence,
        operand_sides: 0,
        whitespace_sides: 0,
        right_assoc: false,
        opens: false,
        closes: false,
    };
    let unary = |symbol| TokInfo {
        symbol,
        precedence: 16,
        operand_sides: RIGHT,
        whitespace_sides: 0,
        right_assoc: true,
        opens: false,
        closes: false,
    };
    let bin = |symbol, precedence| TokInfo {
        symbol,
        precedence,
        operand_sides: BOTH,
        whitespace_sides: BOTH,
        right_assoc: false,
        opens: false,
        closes: false,
    };
    match t {
        Tok::Invalid => v("(<INVALID>)", !0),
        Tok::False => v("false", 0),
        Tok::True => v("true", 0),
        Tok::Number => v("(<number>)", 0),
        Tok::Variable => v("(<variable>)", 0),
        Tok::OpenParen => TokInfo {
            operand_sides: RIGHT,
            opens: true,
            ..v("(", -1)
        },
        Tok::CloseParen => TokInfo {
            operand_sides: LEFT,
            closes: true,
            ..v(")", -1)
        },
        Tok::Not => unary("!"),
        Tok::OnesComplement => unary("~"),
        Tok::Negate => unary("-"),
        Tok::And => bin("&&", 4),
        Tok::Or => bin("||", 3),
        Tok::Equals => bin("==", 8),
        Tok::NotEquals => bin("!=", 8),
        Tok::LessThan => bin("<", 9),
        Tok::LessThanOrEqual => bin("<=", 9),
        Tok::GreaterThan => bin(">", 9),
        Tok::GreaterThanOrEqual => bin(">=", 9),
        Tok::Add => bin("+", 11),
        Tok::Subtract => bin("-", 11),
        Tok::Multiply => bin("*", 12),
        Tok::Divide => bin("/", 12),
        Tok::Modulo => bin("%", 12),
    }
}

fn has_right_operand(t: Tok) -> bool {
    info(t).operand_sides & RIGHT != 0
}

#[derive(Clone, Debug)]
struct Token {
    ty: Tok,
    index: usize,
    text: String,
}

impl Token {
    fn left_operand(&self) -> bool {
        info(self.ty).operand_sides & LEFT != 0
    }

    fn right_operand(&self) -> bool {
        info(self.ty).operand_sides & RIGHT != 0
    }

    fn symbol(&self) -> &str {
        match self.ty {
            Tok::Number | Tok::Variable => &self.text,
            t => info(t).symbol,
        }
    }
}

fn scan_is_number(e: &[char], start: usize, i: &mut usize) -> Result<bool, String> {
    if char_class(e[*i]) != CharClass::Digit {
        return Ok(false);
    }
    *i += 1;
    while *i < e.len() {
        let cc = char_class(e[*i]);
        if cc != CharClass::Digit {
            if cc != CharClass::Whitespace && cc != CharClass::Operator && cc != CharClass::Mixed {
                let n: String = e[start..*i].iter().collect();
                return Err(format!("Number {n} and variable merged at index {start}"));
            }
            return Ok(true);
        }
        *i += 1;
    }
    Ok(true)
}

fn variable_or_keyword(e: &[char], start: usize, i: usize) -> Result<Tok, String> {
    if char_class(e[i - 1]) == CharClass::Mixed {
        let s: String = e[start..i].iter().collect();
        return Err(format!("Invalid identifier end character at index {} for `{s}`", i - 1));
    }
    let s: String = e[start..i].iter().collect();
    Ok(match s.as_str() {
        "true" => Tok::True,
        "false" => Tok::False,
        _ => Tok::Variable,
    })
}

fn next_type(e: &[char], i: &mut usize, last: Tok) -> Result<Tok, String> {
    let start = *i;
    let two = |i: &mut usize, c: char, yes: Tok, no: Tok| {
        *i += 1;
        if *i < e.len() && e[*i] == c {
            *i += 1;
            yes
        } else {
            no
        }
    };
    match e[*i] {
        '!' => return Ok(two(i, '=', Tok::NotEquals, Tok::Not)),
        '<' => return Ok(two(i, '=', Tok::LessThanOrEqual, Tok::LessThan)),
        '>' => return Ok(two(i, '=', Tok::GreaterThanOrEqual, Tok::GreaterThan)),
        '=' => {
            let t = two(i, '=', Tok::Equals, Tok::Invalid);
            if t == Tok::Invalid {
                return Err(format!("Unexpected character '=' at index {start} - should it be `==`?"));
            }
            return Ok(t);
        }
        '&' => {
            let t = two(i, '&', Tok::And, Tok::Invalid);
            if t == Tok::Invalid {
                return Err(format!("Unexpected character '&' at index {start} - should it be `&&`?"));
            }
            return Ok(t);
        }
        '|' => {
            let t = two(i, '|', Tok::Or, Tok::Invalid);
            if t == Tok::Invalid {
                return Err(format!("Unexpected character '|' at index {start} - should it be `||`?"));
            }
            return Ok(t);
        }
        '(' => {
            *i += 1;
            return Ok(Tok::OpenParen);
        }
        ')' => {
            *i += 1;
            return Ok(Tok::CloseParen);
        }
        '~' => {
            *i += 1;
            return Ok(Tok::OnesComplement);
        }
        '+' => {
            *i += 1;
            return Ok(Tok::Add);
        }
        '-' => {
            *i += 1;
            if *i < e.len() && scan_is_number(e, start, i)? {
                return Ok(Tok::Number);
            }
            *i = start + 1;
            return Ok(if last == Tok::Invalid || has_right_operand(last) {
                Tok::Negate
            } else {
                Tok::Subtract
            });
        }
        '*' => {
            *i += 1;
            return Ok(Tok::Multiply);
        }
        '/' => {
            *i += 1;
            return Ok(Tok::Divide);
        }
        '%' => {
            *i += 1;
            return Ok(Tok::Modulo);
        }
        _ => {}
    }
    if scan_is_number(e, start, i)? {
        return Ok(Tok::Number);
    }
    if char_class(e[start]) != CharClass::Id {
        return Err(format!("Invalid character '{}' at index {start}", e[*i]));
    }
    *i = start;
    while *i < e.len() {
        let cc = char_class(e[*i]);
        if cc == CharClass::Whitespace || cc == CharClass::Operator {
            return variable_or_keyword(e, start, *i);
        }
        *i += 1;
    }
    variable_or_keyword(e, start, *i)
}

fn next_token(e: &[char], i: &mut usize, last: Tok) -> Result<Option<Token>, String> {
    if *i == e.len() {
        return Ok(None);
    }
    let mut ws_before = false;
    if char_class(e[*i]) == CharClass::Whitespace {
        ws_before = true;
        while char_class(e[*i]) == CharClass::Whitespace {
            *i += 1;
            if *i == e.len() {
                return Ok(None);
            }
        }
    } else if last == Tok::Invalid {
        ws_before = true;
    } else if info(last).whitespace_sides & RIGHT != 0 {
        return Err(format!(
            "Missing whitespace at index {}, after `{}` operator.",
            *i,
            info(last).symbol
        ));
    }
    let start = *i;
    let ty = next_type(e, i, last)?;
    if !ws_before && info(ty).whitespace_sides & LEFT != 0 {
        return Err(format!(
            "Missing whitespace at index {}, before `{}` operator.",
            *i,
            info(ty).symbol
        ));
    }
    Ok(Some(Token {
        ty,
        index: start,
        text: e[start..*i].iter().collect(),
    }))
}

/// Compiled expression tree.
#[derive(Debug)]
enum Node {
    Const(i32),
    Bool(bool),
    Var(String),
    Unary(Tok, Box<Node>),
    Binary(Tok, Box<Node>, Box<Node>),
    /// Converts an int to bool (`!= 0`) or a bool to int (1/0).
    ToBool(Box<Node>),
    ToInt(Box<Node>),
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Ty {
    Int,
    Bool,
}

impl Node {
    fn eval_int(&self, v: &dyn Variables) -> i32 {
        match self {
            Node::Const(c) => *c,
            Node::Var(n) => v.get(n),
            Node::ToInt(b) => i32::from(b.eval_bool(v)),
            Node::Unary(Tok::Negate, x) => x.eval_int(v).wrapping_neg(),
            Node::Unary(Tok::OnesComplement, x) => !x.eval_int(v),
            Node::Binary(op, x, y) => {
                let (a, b) = (x.eval_int(v), y.eval_int(v));
                match op {
                    Tok::Add => a.wrapping_add(b),
                    Tok::Subtract => a.wrapping_sub(b),
                    Tok::Multiply => a.wrapping_mul(b),
                    Tok::Divide => {
                        if b != 0 {
                            a.wrapping_div(b)
                        } else {
                            0
                        }
                    }
                    Tok::Modulo => {
                        if b != 0 {
                            a.wrapping_rem(b)
                        } else {
                            0
                        }
                    }
                    _ => unreachable!(),
                }
            }
            _ => unreachable!(),
        }
    }

    fn eval_bool(&self, v: &dyn Variables) -> bool {
        match self {
            Node::Bool(b) => *b,
            Node::ToBool(x) => x.eval_int(v) != 0,
            Node::Unary(Tok::Not, x) => !x.eval_bool(v),
            Node::Binary(op, x, y) => match op {
                Tok::And => x.eval_bool(v) & y.eval_bool(v),
                Tok::Or => x.eval_bool(v) | y.eval_bool(v),
                Tok::Equals => x.eval_int(v) == y.eval_int(v),
                Tok::NotEquals => x.eval_int(v) != y.eval_int(v),
                Tok::LessThan => x.eval_int(v) < y.eval_int(v),
                Tok::LessThanOrEqual => x.eval_int(v) <= y.eval_int(v),
                Tok::GreaterThan => x.eval_int(v) > y.eval_int(v),
                Tok::GreaterThanOrEqual => x.eval_int(v) >= y.eval_int(v),
                _ => unreachable!(),
            },
            _ => unreachable!(),
        }
    }
}

/// Variable values; missing variables read as 0.
pub trait Variables {
    fn get(&self, name: &str) -> i32;
}

impl Variables for HashMap<String, i32> {
    fn get(&self, name: &str) -> i32 {
        HashMap::get(self, name).copied().unwrap_or(0)
    }
}

impl Variables for indexmap::IndexMap<String, i32> {
    fn get(&self, name: &str) -> i32 {
        indexmap::IndexMap::get(self, name).copied().unwrap_or(0)
    }
}

/// No variables at all.
pub struct NoVariables;

impl Variables for NoVariables {
    fn get(&self, _name: &str) -> i32 {
        0
    }
}

struct Parsed {
    root: Node,
    variables: IndexSet<String>,
}

fn build(expression: &str, result: Ty) -> Result<Parsed, String> {
    let e: Vec<char> = expression.chars().collect();
    let mut tokens: Vec<Token> = Vec::new();
    let mut openers: Vec<usize> = Vec::new();
    let mut variables = IndexSet::new();
    let mut i = 0;
    loop {
        let last_ty = tokens.last().map_or(Tok::Invalid, |t| t.ty);
        let Some(token) = next_token(&e, &mut i, last_ty)? else {
            match tokens.last() {
                None => return Err("Empty expression".into()),
                Some(l) if l.right_operand() => {
                    return Err(format!("Missing value or sub-expression at end for `{}` operator", l.symbol()));
                }
                _ => break,
            }
        };
        let ti = info(token.ty);
        if ti.closes {
            if openers.pop().is_none() {
                return Err(format!("Unmatched closing parenthesis at index {}", token.index));
            }
        }
        if ti.opens {
            openers.push(token.index);
        }
        match tokens.last() {
            None => {
                if token.left_operand() {
                    return Err(format!(
                        "Missing value or sub-expression at beginning for `{}` operator",
                        token.symbol()
                    ));
                }
            }
            Some(last) => {
                if info(last.ty).opens && ti.closes {
                    return Err(format!("Empty parenthesis at index {}", last.index));
                }
                if last.right_operand() == token.left_operand() {
                    if last.right_operand() {
                        return Err(format!(
                            "Missing value or sub-expression or there is an extra operator `{}` at index {} or `{}` at index {}",
                            last.symbol(),
                            last.index,
                            token.symbol(),
                            token.index
                        ));
                    }
                    return Err(format!(
                        "Missing binary operation before `{}` at index {}",
                        token.symbol(),
                        token.index
                    ));
                }
            }
        }
        if token.ty == Tok::Variable {
            variables.insert(token.text.clone());
        }
        tokens.push(token);
    }
    if let Some(&idx) = openers.last() {
        return Err(format!("Unclosed opening parenthesis at index {idx}"));
    }

    let mut postfix = Vec::new();
    let mut stack: Vec<Token> = Vec::new();
    for t in tokens {
        let ti = info(t.ty);
        if ti.opens {
            stack.push(t);
        } else if ti.closes {
            while let Some(top) = stack.pop() {
                if info(top.ty).opens {
                    break;
                }
                postfix.push(top);
            }
        } else if ti.operand_sides == 0 {
            postfix.push(t);
        } else {
            while let Some(top) = stack.last() {
                let tp = info(top.ty).precedence;
                if (ti.right_assoc && ti.precedence < tp) || (!ti.right_assoc && ti.precedence <= tp) {
                    postfix.push(stack.pop().unwrap());
                } else {
                    break;
                }
            }
            stack.push(t);
        }
    }
    while let Some(t) = stack.pop() {
        postfix.push(t);
    }

    let mut ast: Vec<(Node, Ty)> = Vec::new();
    let pop = |ast: &mut Vec<(Node, Ty)>, want: Ty| -> Node {
        let (n, ty) = ast.pop().expect("validated expression");
        match (ty, want) {
            (a, b) if a == b => n,
            (Ty::Int, Ty::Bool) => Node::ToBool(Box::new(n)),
            (Ty::Bool, Ty::Int) => Node::ToInt(Box::new(n)),
            _ => unreachable!(),
        }
    };
    for t in postfix {
        match t.ty {
            Tok::And | Tok::Or => {
                let y = pop(&mut ast, Ty::Bool);
                let x = pop(&mut ast, Ty::Bool);
                ast.push((Node::Binary(t.ty, Box::new(x), Box::new(y)), Ty::Bool));
            }
            Tok::Equals | Tok::NotEquals | Tok::LessThan | Tok::LessThanOrEqual | Tok::GreaterThan | Tok::GreaterThanOrEqual => {
                let y = pop(&mut ast, Ty::Int);
                let x = pop(&mut ast, Ty::Int);
                ast.push((Node::Binary(t.ty, Box::new(x), Box::new(y)), Ty::Bool));
            }
            Tok::Add | Tok::Subtract | Tok::Multiply | Tok::Divide | Tok::Modulo => {
                let y = pop(&mut ast, Ty::Int);
                let x = pop(&mut ast, Ty::Int);
                ast.push((Node::Binary(t.ty, Box::new(x), Box::new(y)), Ty::Int));
            }
            Tok::Not => {
                let x = pop(&mut ast, Ty::Bool);
                ast.push((Node::Unary(Tok::Not, Box::new(x)), Ty::Bool));
            }
            Tok::Negate | Tok::OnesComplement => {
                let x = pop(&mut ast, Ty::Int);
                ast.push((Node::Unary(t.ty, Box::new(x)), Ty::Int));
            }
            Tok::False => ast.push((Node::Bool(false), Ty::Bool)),
            Tok::True => ast.push((Node::Bool(true), Ty::Bool)),
            Tok::Number => {
                let v: i32 = t.text.parse().map_err(|_| format!("Invalid number {}", t.text))?;
                ast.push((Node::Const(v), Ty::Int));
            }
            Tok::Variable => ast.push((Node::Var(t.text), Ty::Int)),
            _ => return Err(format!("Unexpected token {}", t.symbol())),
        }
    }
    let root = pop(&mut ast, result);
    Ok(Parsed { root, variables })
}

/// A boolean condition expression (`BooleanExpression`).
#[derive(Clone, Debug)]
pub struct BooleanExpression(Rc<(String, Node, IndexSet<String>)>);

impl BooleanExpression {
    pub fn new(expression: &str) -> Result<Self, String> {
        let p = build(expression, Ty::Bool)?;
        Ok(BooleanExpression(Rc::new((expression.to_string(), p.root, p.variables))))
    }

    pub fn expression(&self) -> &str {
        &self.0.0
    }

    /// Variable names in order of first appearance.
    pub fn variables(&self) -> &IndexSet<String> {
        &self.0.2
    }

    pub fn evaluate(&self, symbols: &dyn Variables) -> bool {
        self.0.1.eval_bool(symbols)
    }
}

/// An integer expression (`IntegerExpression`).
#[derive(Clone, Debug)]
pub struct IntegerExpression(Rc<(String, Node, IndexSet<String>)>);

impl IntegerExpression {
    pub fn new(expression: &str) -> Result<Self, String> {
        let p = build(expression, Ty::Int)?;
        Ok(IntegerExpression(Rc::new((expression.to_string(), p.root, p.variables))))
    }

    pub fn expression(&self) -> &str {
        &self.0.0
    }

    pub fn variables(&self) -> &IndexSet<String> {
        &self.0.2
    }

    pub fn evaluate(&self, symbols: &dyn Variables) -> i32 {
        self.0.1.eval_int(symbols)
    }
}

impl FieldValue for BooleanExpression {
    const TYPE_NAME: &'static str = "BooleanExpression";
    fn parse(field: &str, value: Option<&str>, _yaml: Option<&MiniYaml>) -> Result<Self, LoadError> {
        match value {
            Some(v) => BooleanExpression::new(v).map_err(LoadError),
            None => invalid(value, "BooleanExpression", field),
        }
    }
}
impl ArrayAware for BooleanExpression {}

impl FieldValue for IntegerExpression {
    const TYPE_NAME: &'static str = "IntegerExpression";
    fn parse(field: &str, value: Option<&str>, _yaml: Option<&MiniYaml>) -> Result<Self, LoadError> {
        match value {
            Some(v) => IntegerExpression::new(v).map_err(LoadError),
            None => invalid(value, "IntegerExpression", field),
        }
    }
}
impl ArrayAware for IntegerExpression {}

#[cfg(test)]
mod tests {
    use super::*;

    fn vars(v: &[(&str, i32)]) -> HashMap<String, i32> {
        v.iter().map(|(k, v)| (k.to_string(), *v)).collect()
    }

    #[test]
    fn boolean_conditions() {
        let e = BooleanExpression::new("!disabled && (veteran || rank > 2)").unwrap();
        assert!(e.evaluate(&vars(&[("veteran", 1)])));
        assert!(!e.evaluate(&vars(&[("veteran", 1), ("disabled", 1)])));
        assert!(e.evaluate(&vars(&[("rank", 3)])));
        assert_eq!(e.variables().iter().collect::<Vec<_>>(), ["disabled", "veteran", "rank"]);
    }

    #[test]
    fn integer_arithmetic_and_precedence() {
        let e = IntegerExpression::new("a + b * 2 - -3 / 0").unwrap();
        assert_eq!(e.evaluate(&vars(&[("a", 1), ("b", 4)])), 9);
        let e = IntegerExpression::new("(a > 1) * 5").unwrap();
        assert_eq!(e.evaluate(&vars(&[("a", 2)])), 5);
    }

    #[test]
    fn syntax_errors() {
        assert!(BooleanExpression::new("a &&b").is_err());
        assert!(BooleanExpression::new("a = b").is_err());
        assert!(BooleanExpression::new("(a").is_err());
        assert!(BooleanExpression::new("").is_err());
    }
}
