//! Recursive-descent parser for the C subset.
//!
//! Mirrors `tests/differential/parse.cpp` routine-for-routine: same
//! accepted language, same tree shapes (see `tests/differential/SPEC.md`),
//! same error messages and positions.

use std::collections::{HashMap, HashSet};

use crate::ast::*;
use crate::token::{Kind, Tok, TokenError, tokenize};

/// Parse failure: 1-based position plus the reference message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParseError {
    pub line: usize,
    pub col: usize,
    pub msg: String,
}

impl std::fmt::Display for ParseError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}:{}: error: {}", self.line, self.col, self.msg)
    }
}

impl std::error::Error for ParseError {}

/// Tokenize and parse `src` into a translation unit.
pub fn parse(src: &[u8]) -> Result<TranslationUnit, ParseError> {
    let toks = tokenize(src).map_err(|e: TokenError| ParseError {
        line: e.line,
        col: e.col,
        msg: e.msg,
    })?;
    let mut p = Parser::new(&toks);
    let tu = p.parse_translation_unit()?;
    p.check_end()?;
    Ok(tu)
}

const TYPE_KWS: [&[u8]; 4] = [b"int", b"char", b"short", b"void"];
/// Words that may extend a type specifier (`unsigned int`, `const
/// char`, `long long`). They stay identifiers elsewhere — `long`
/// can still be a variable name — so they are recognized only
/// inside specifier position (see `parse_spec`).
const QUALIFIER_WORDS: [&[u8]; 4] = [b"long", b"unsigned", b"signed", b"const"];

fn is_qualifier(text: &[u8]) -> bool {
    is_op_in(text, &QUALIFIER_WORDS)
}
const ASSIGN_OPS: [&[u8]; 11] = [
    b"=", b"+=", b"-=", b"*=", b"/=", b"%=", b"&=", b"|=", b"^=", b"<<=", b">>=",
];
const EQUALITY_OPS: [&[u8]; 2] = [b"==", b"!="];
const RELATIONAL_OPS: [&[u8]; 4] = [b"<", b">", b"<=", b">="];
const SHIFT_OPS: [&[u8]; 2] = [b"<<", b">>"];
const BITAND_OPS: [&[u8]; 1] = [b"&"];
const BITXOR_OPS: [&[u8]; 1] = [b"^"];
const BITOR_OPS: [&[u8]; 1] = [b"|"];
const ADDITIVE_OPS: [&[u8]; 2] = [b"+", b"-"];
const MULTIPLICATIVE_OPS: [&[u8]; 3] = [b"*", b"/", b"%"];
const UNARY_OPS: [&[u8]; 8] = [b"!", b"&", b"*", b"++", b"--", b"-", b"+", b"~"];

fn is_op_in(text: &[u8], ops: &[&[u8]]) -> bool {
    ops.iter().any(|op| *op == text)
}

/// Value of an integer-literal token, mirroring the twin in
/// `compile.rs` (`int_literal_value`): suffixes stripped, radix
/// from prefix, saturating. Only used for typedef array sizes,
/// which must be integer literals.
fn int_text_value(text: &[u8]) -> i32 {
    let mut s = text;
    while matches!(s.last(), Some(b'u' | b'U' | b'l' | b'L')) {
        s = &s[..s.len() - 1];
    }
    let (radix, digits): (i64, &[u8]) = if s.starts_with(b"0x") || s.starts_with(b"0X") {
        (16, &s[2..])
    } else if s.starts_with(b"0b") || s.starts_with(b"0B") {
        (2, &s[2..])
    } else if s.len() > 1 && s[0] == b'0' {
        (8, s)
    } else {
        (10, s)
    };
    let mut v: i64 = 0;
    for &c in digits {
        let d = match c {
            b'0'..=b'9' => (c - b'0') as i64,
            b'a'..=b'f' => (c - b'a' + 10) as i64,
            b'A'..=b'F' => (c - b'A' + 10) as i64,
            _ => 0,
        };
        v = v.saturating_mul(radix).saturating_add(d);
    }
    v.clamp(i32::MIN as i64, i32::MAX as i64) as i32
}

/// Value of a character literal's raw text (including quotes),
/// for enumerator values: one plain character, or a simple escape
/// (`\n`, `\t`, `\r`, `\\`, `\'`, `\"`, `\0`, octal, `\x` hex).
/// Returns `None` for an empty or malformed literal.
fn char_text_value(text: &[u8]) -> Option<i32> {
    if text.len() < 3 || text[0] != b'\'' || text[text.len() - 1] != b'\'' {
        return None;
    }
    let inner = &text[1..text.len() - 1];
    if inner.is_empty() {
        return None;
    }
    if inner[0] != b'\\' {
        return inner.first().map(|&c| c as i32);
    }
    if inner.len() < 2 {
        return None;
    }
    let value = match inner[1] {
        b'n' => b'\n',
        b't' => b'\t',
        b'r' => b'\r',
        b'0'..=b'7' => {
            // Octal escape: up to three digits.
            let mut v: i32 = 0;
            for &c in inner[1..].iter().take(3) {
                if !matches!(c, b'0'..=b'7') {
                    break;
                }
                v = v * 8 + (c - b'0') as i32;
            }
            return Some(v);
        }
        b'x' => {
            let mut v: i32 = 0;
            for &c in inner[2..].iter() {
                let d = match c {
                    b'0'..=b'9' => (c - b'0') as i32,
                    b'a'..=b'f' => (c - b'a' + 10) as i32,
                    b'A'..=b'F' => (c - b'A' + 10) as i32,
                    _ => break,
                };
                v = v * 16 + d;
            }
            return Some(v);
        }
        b'a' => 7,
        b'b' => 8,
        b'f' => 12,
        b'v' => 11,
        c => c,
    };
    Some(value as i32)
}

/// Value of a constant expression, for enumerator initializers:
/// integer/character literals, enumerator names, parentheses,
/// casts, and arithmetic/bitwise/comparison operators over
/// constant operands, wrapping like the byte model's `i32`
/// arithmetic. Anything else (calls, variables, assignments,
/// division or modulo by zero) is not constant and yields `None`.
fn eval_const(e: &Expr, consts: &HashMap<Vec<u8>, i32>) -> Option<i32> {
    match e {
        Expr::Int(bytes) => Some(int_text_value(bytes)),
        Expr::Char(bytes) => char_text_value(bytes),
        Expr::Ident(name) => consts.get(name).copied(),
        Expr::Paren(inner) => eval_const(inner, consts),
        Expr::Cast { e, .. } => eval_const(e, consts),
        Expr::Ternary { cond, then, els } => {
            let c = eval_const(cond, consts)?;
            eval_const(if c != 0 { then } else { els }, consts)
        }
        Expr::Comma(exprs) => exprs.last().and_then(|e| eval_const(e, consts)),
        Expr::Unary { op, e } => {
            let v = eval_const(e, consts)?;
            match op.as_slice() {
                b"-" => Some(v.wrapping_neg()),
                b"+" => Some(v),
                b"~" => Some(!v),
                b"!" => Some(i32::from(v == 0)),
                _ => None,
            }
        }
        Expr::BinOp { op, l, r } => {
            let a = eval_const(l, consts)?;
            let b = eval_const(r, consts)?;
            Some(match op.as_slice() {
                b"+" => a.wrapping_add(b),
                b"-" => a.wrapping_sub(b),
                b"*" => a.wrapping_mul(b),
                b"/" => a.checked_div(b)?,
                b"%" => a.checked_rem(b)?,
                b"<<" => a.wrapping_shl(b as u32),
                b">>" => a.wrapping_shr(b as u32),
                b"&" => a & b,
                b"|" => a | b,
                b"^" => a ^ b,
                b"==" => i32::from(a == b),
                b"!=" => i32::from(a != b),
                b"<" => i32::from(a < b),
                b"<=" => i32::from(a <= b),
                b">" => i32::from(a > b),
                b">=" => i32::from(a >= b),
                b"&&" => i32::from(a != 0 && b != 0),
                b"||" => i32::from(a != 0 || b != 0),
                _ => return None,
            })
        }
        _ => None,
    }
}

/// A parsed type specifier: joined words plus declarator flags
/// contributed by a typedef name in first position (a pointer or
/// array alias). Plain specifiers carry no flags.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Spec {
    words: Vec<u8>,
    pointer: bool,
    array_len: Option<i32>,
    array_unsized: bool,
}

impl Spec {
    fn plain(words: Vec<u8>) -> Self {
        Spec {
            words,
            pointer: false,
            array_len: None,
            array_unsized: false,
        }
    }

    fn is_array(&self) -> bool {
        self.array_len.is_some() || self.array_unsized
    }
}

fn desc(t: &Tok) -> String {
    if t.kind == Kind::End {
        "end of file".to_string()
    } else {
        format!("'{}'", String::from_utf8_lossy(&t.text))
    }
}

fn is_lvalue(e: &Expr) -> bool {
    match e {
        Expr::Ident(_) => true,
        Expr::Index { .. } => true,
        Expr::Unary { op, .. } => op.as_slice() == b"*",
        Expr::Paren(inner) => is_lvalue(inner),
        _ => false,
    }
}

struct Parser<'a> {
    t: &'a [Tok],
    p: usize,
    /// File-scope typedefs visible from their definition onward
    /// (source order). Block-scoped typedefs are rejected, so no
    /// scope stack is needed; see `docs/semantics.md`.
    typedefs: HashMap<Vec<u8>, TypedefDecl>,
    /// Enumerator constants visible from their definition onward
    /// (source order). File-scope only, like typedefs.
    enum_consts: HashMap<Vec<u8>, i32>,
    /// Declared names per enclosing scope, innermost last: the
    /// file scope, then each compound, parameter, and `for` scope.
    /// Used only to decide whether an identifier in expression
    /// position is a declared name (kept as an identifier) or an
    /// enumerator (substituted with its value).
    scopes: Vec<HashSet<Vec<u8>>>,
}

impl<'a> Parser<'a> {
    fn new(t: &'a [Tok]) -> Self {
        Parser {
            t,
            p: 0,
            typedefs: HashMap::new(),
            enum_consts: HashMap::new(),
            scopes: vec![HashSet::new()],
        }
    }

    /// Whether `name` is declared in any enclosing scope.
    fn is_declared(&self, name: &[u8]) -> bool {
        self.scopes.iter().rev().any(|s| s.contains(name))
    }

    /// Record `name` in the innermost scope.
    fn declare_name(&mut self, name: &[u8]) {
        if let Some(scope) = self.scopes.last_mut() {
            scope.insert(name.to_vec());
        }
    }

    fn peek_at(&self, k: isize) -> &Tok {
        let j = (self.p as isize + k).max(0) as usize;
        self.t.get(j).unwrap_or_else(|| self.t.last().unwrap())
    }

    fn peek(&self) -> &Tok {
        self.peek_at(0)
    }

    fn at_end(&self) -> bool {
        self.peek().kind == Kind::End
    }

    fn at_kw(&self, w: &[u8]) -> bool {
        let t = self.peek();
        t.kind == Kind::Kw && t.text.as_slice() == w
    }

    fn at_punct(&self, w: &[u8]) -> bool {
        let t = self.peek();
        t.kind == Kind::Punct && t.text.as_slice() == w
    }

    fn at_ident(&self) -> bool {
        self.peek().kind == Kind::Ident
    }

    fn at_punct_op(&self, ops: &[&[u8]]) -> bool {
        let t = self.peek();
        t.kind == Kind::Punct && is_op_in(&t.text, ops)
    }

    fn is_type_tok(&self) -> bool {
        let t = self.peek();
        t.kind == Kind::Kw && is_op_in(&t.text, &TYPE_KWS)
    }

    /// Whether the next tokens can start a declaration specifier:
    /// a type keyword, a typedef name (even alone, as an unnamed
    /// parameter's type), or an identifier followed by a type
    /// keyword or another identifier (a one-word specifier, or the
    /// first word of a multi-word one).
    fn at_spec_start(&self) -> bool {
        if self.is_type_tok() {
            return true;
        }
        if !self.at_ident() {
            return false;
        }
        if self.typedefs.contains_key(&self.peek().text) {
            return true;
        }
        let n = self.peek_at(1);
        (n.kind == Kind::Kw && is_op_in(&n.text, &TYPE_KWS)) || n.kind == Kind::Ident
    }

    /// Read a type specifier: one or more words joined by a single
    /// space (`int`, `unsigned int`, `long long`). A following type
    /// keyword or qualifier is consumed only when yet another word
    /// follows it, so a declarator named like a qualifier
    /// (`int long = 1;`) keeps its name: the qualifier before `=`
    /// is not swallowed into the specifier.
    ///
    /// A typedef name in the specifier expands to its aliased
    /// specifier plus declarator flags. Qualifiers may precede or
    /// follow the name (`const T`, `T const`); any other word
    /// beside the name is an error, as is a typedef name anywhere
    /// but the specifier's head.
    fn parse_spec(&mut self) -> Result<Spec, ParseError> {
        let mut words = vec![self.next_tok().text];
        while (self.is_type_tok()
            || (self.at_ident()
                && (is_qualifier(&self.peek().text)
                    || self.typedefs.contains_key(&self.peek().text))))
            && {
                let n = self.peek_at(1);
                (n.kind == Kind::Kw && is_op_in(&n.text, &TYPE_KWS))
                    || n.kind == Kind::Ident
            }
        {
            words.push(self.next_tok().text);
        }
        // An `enum` specifier takes a tag: `enum Color x`. The tag
        // is a plain identifier, so the joiner above leaves it;
        // consume it here unless a `{` follows (an enum definition,
        // handled by `parse_enum_declaration` at file scope and
        // rejected in blocks by the caller).
        if words.len() == 1
            && words[0].as_slice() == b"enum"
            && self.at_ident()
        {
            let after = self.peek_at(1);
            if !(after.kind == Kind::Punct && after.text == b"{") {
                words.push(self.next_tok().text);
            }
        }
        self.resolve_spec(words)
    }

    /// Expand a typedef name in the collected specifier words.
    /// `words` holds the raw words `parse_spec` collected; at most
    /// one may be a typedef name, and every other word must be a
    /// qualifier.
    fn resolve_spec(&self, words: Vec<Vec<u8>>) -> Result<Spec, ParseError> {
        let at = words
            .iter()
            .position(|w| self.typedefs.contains_key(w));
        let Some(at) = at else {
            return Ok(Spec::plain(words.join(&b' ')));
        };
        if words
            .iter()
            .filter(|w| self.typedefs.contains_key(w.as_slice()))
            .count()
            > 1
        {
            return self.fail("multiple typedef names in one specifier".to_string());
        }
        for (i, w) in words.iter().enumerate() {
            if i != at && !is_qualifier(w) {
                return self.fail(
                    "unexpected specifier beside a typedef name".to_string(),
                );
            }
        }
        let alias = &self.typedefs[&words[at]];
        let mut spec_words: Vec<Vec<u8>> = alias.spec.split(|&b| b == b' ').map(|w| w.to_vec()).collect();
        for (i, w) in words.iter().enumerate() {
            if i != at {
                spec_words.push(w.clone());
            }
        }
        Ok(Spec {
            words: spec_words.join(&b' '),
            pointer: alias.pointer,
            array_len: alias.array_len,
            array_unsized: alias.array_unsized,
        })
    }

    fn next_tok(&mut self) -> Tok {
        let t = self.peek().clone();
        self.p += 1;
        t
    }

    fn err(&self, line: usize, col: usize, msg: String) -> ParseError {
        ParseError { line, col, msg }
    }

    fn fail<T>(&self, msg: String) -> Result<T, ParseError> {
        let t = self.peek().clone();
        Err(self.err(t.line, t.col, msg))
    }

    fn expect_punct(&mut self, w: &[u8], what: &str) -> Result<Tok, ParseError> {
        if self.at_punct(w) {
            Ok(self.next_tok())
        } else {
            let t = self.peek().clone();
            Err(self.err(
                t.line,
                t.col,
                format!("expected {}, found {}", what, desc(&t)),
            ))
        }
    }

    fn expect_kw(&mut self, w: &[u8], what: &str) -> Result<Tok, ParseError> {
        if self.at_kw(w) {
            Ok(self.next_tok())
        } else {
            let t = self.peek().clone();
            Err(self.err(
                t.line,
                t.col,
                format!("expected {}, found {}", what, desc(&t)),
            ))
        }
    }

    fn expect_ident(&mut self, what: &str) -> Result<Tok, ParseError> {
        if self.at_ident() {
            Ok(self.next_tok())
        } else {
            let t = self.peek().clone();
            Err(self.err(
                t.line,
                t.col,
                format!("expected {}, found {}", what, desc(&t)),
            ))
        }
    }

    fn check_end(&self) -> Result<(), ParseError> {
        if !self.at_end() {
            let t = self.peek().clone();
            return Err(self.err(
                t.line,
                t.col,
                format!("unexpected trailing {}", desc(&t)),
            ));
        }
        Ok(())
    }

    fn parse_translation_unit(&mut self) -> Result<TranslationUnit, ParseError> {
        let mut items = Vec::new();
        while !self.at_end() {
            if self.at_punct(b";") {
                self.next_tok();
                continue;
            }
            items.push(self.parse_external()?);
        }
        Ok(TranslationUnit(items))
    }

    fn parse_external(&mut self) -> Result<External, ParseError> {
        if self.at_ident() && self.peek().text.as_slice() == b"typedef" {
            return self.parse_typedef_declaration();
        }
        if self.at_ident() && self.peek().text.as_slice() == b"enum" {
            return self.parse_enum_declaration();
        }
        if self.at_spec_start() {
            let mark = self.p;
            let spec = self.parse_spec()?;
            if self.at_ident() {
                let name = self.next_tok().text;
                if self.at_punct(b"(") {
                    if self.after_param_list_is_semicolon() {
                        return Ok(External::FuncDecl(self.parse_funcdecl(spec, name)?));
                    }
                    return Ok(External::FuncDef(self.parse_funcdef(spec, name)?));
                }
            }
            self.p = mark;
            return Ok(External::Decl(self.parse_declaration()?));
        }
        self.fail("expected declaration or function definition".to_string())
    }

    /// Parse a file-scope `typedef` declaration (the `typedef`
    /// keyword itself is still ahead): one declarator, no
    /// initializer, no comma list. The alias is registered for
    /// uses later in the file; redefining a name to a different
    /// expansion is an error, an identical redefinition is allowed.
    fn parse_typedef_declaration(&mut self) -> Result<External, ParseError> {
        self.next_tok(); // `typedef`
        let spec = self.parse_spec()?;
        let mut pointer = false;
        if self.at_punct(b"*") {
            self.next_tok();
            pointer = true;
        }
        if spec.pointer && pointer {
            return self.fail("pointer to pointer is not supported".to_string());
        }
        pointer |= spec.pointer;
        let name = self.expect_ident("declarator name")?.text;
        let (mut array_len, mut array_unsized) = (spec.array_len, spec.array_unsized);
        if self.at_punct(b"[") {
            if array_len.is_some() || array_unsized {
                return self.fail(
                    "multi-dimensional arrays are not supported".to_string(),
                );
            }
            self.next_tok();
            if self.at_punct(b"]") {
                array_unsized = true;
            } else {
                let len_expr = self.parse_assignment()?;
                let Expr::Int(bytes) = len_expr else {
                    return self.fail(
                        "array size must be an integer literal".to_string(),
                    );
                };
                let len = int_text_value(&bytes);
                if len <= 0 {
                    return self.fail("array size must be positive".to_string());
                }
                array_len = Some(len);
            }
            self.expect_punct(b"]", "']'")?;
        }
        if self.at_punct(b"=") {
            return self.fail("typedef declarators take no initializer".to_string());
        }
        if self.at_punct(b",") {
            return self.fail(
                "multiple typedef declarators are not supported".to_string(),
            );
        }
        self.expect_punct(b";", "';'")?;
        let decl = TypedefDecl {
            name: name.clone(),
            spec: spec.words,
            pointer,
            array_len,
            array_unsized,
        };
        if let Some(old) = self.typedefs.get(&name) {
            if *old != decl {
                return self.fail(format!(
                    "conflicting typedef for '{}'",
                    String::from_utf8_lossy(&name)
                ));
            }
        }
        self.typedefs.insert(name, decl.clone());
        Ok(External::Typedef(decl))
    }

    /// Parse a file-scope `enum` declaration (the `enum` keyword
    /// itself is still ahead): an optional tag, an optional
    /// enumerator list, and optional declarators. Enumerator
    /// values register in `enum_consts` for later uses; the
    /// declaration itself becomes an ordinary `Decl` (with no
    /// initializers when there are no declarators), which emits
    /// nothing. A definition with no enumerators is rejected, as
    /// is a duplicate enumerator name.
    fn parse_enum_declaration(&mut self) -> Result<External, ParseError> {
        self.next_tok(); // `enum`
        let mut spec_words = vec![b"enum".to_vec()];
        if self.at_ident() {
            spec_words.push(self.next_tok().text);
        }
        if self.at_punct(b"{") {
            self.next_tok();
            let mut next = 0i32;
            loop {
                let name = self.expect_ident("enumerator name")?.text;
                let value = if self.at_punct(b"=") {
                    self.next_tok();
                    let e = self.parse_assignment()?;
                    match eval_const(&e, &self.enum_consts) {
                        Some(v) => v,
                        None => {
                            return self.fail(
                                "enumerator value must be a constant expression"
                                    .to_string(),
                            );
                        }
                    }
                } else {
                    next
                };
                if self.enum_consts.contains_key(&name) {
                    return self.fail(format!(
                        "duplicate enumerator '{}'",
                        String::from_utf8_lossy(&name)
                    ));
                }
                self.enum_consts.insert(name, value);
                next = value.wrapping_add(1);
                if self.at_punct(b",") {
                    self.next_tok();
                    if self.at_punct(b"}") {
                        break;
                    }
                    continue;
                }
                break;
            }
            self.expect_punct(b"}", "'}'")?;
        }
        let spec = Spec::plain(spec_words.join(&b' '));
        if self.at_ident()
            && self.peek_at(1).kind == Kind::Punct
            && self.peek_at(1).text == b"("
        {
            let name = self.next_tok().text;
            if self.after_param_list_is_semicolon() {
                let d = self.parse_funcdecl(spec, name)?;
                return Ok(External::FuncDecl(d));
            }
            let f = self.parse_funcdef(spec, name)?;
            return Ok(External::FuncDef(f));
        }
        if self.at_punct(b";") {
            self.next_tok();
            return Ok(External::Decl(Decl {
                spec: spec.words,
                inits: Vec::new(),
            }));
        }
        let mut inits = Vec::new();
        inits.push(self.parse_init_declarator(&spec)?);
        while self.at_punct(b",") {
            self.next_tok();
            inits.push(self.parse_init_declarator(&spec)?);
        }
        self.expect_punct(b";", "';'")?;
        Ok(External::Decl(Decl {
            spec: spec.words,
            inits,
        }))
    }

    /// Whether the token after the parameter list starting at the
    /// current `(` is a `;`: the list closes a prototype rather
    /// than opening a definition. The scan counts parentheses, so
    /// array sizes in parentheses do not confuse it; the cursor
    /// does not move.
    fn after_param_list_is_semicolon(&self) -> bool {
        let mut depth = 0i32;
        let mut k = 0isize;
        loop {
            let t = self.peek_at(k);
            if t.kind == Kind::End {
                return false;
            }
            if t.kind == Kind::Punct {
                if t.text.as_slice() == b"(" {
                    depth += 1;
                } else if t.text.as_slice() == b")" {
                    depth -= 1;
                    if depth == 0 {
                        let after = self.peek_at(k + 1);
                        return after.kind == Kind::Punct && after.text.as_slice() == b";";
                    }
                }
            }
            k += 1;
        }
    }

    /// Parse a parameter list, parentheses included. Definitions
    /// require a name per parameter; prototypes allow unnamed
    /// parameters (empty name) but still reject a second `*` and
    /// anything that is not a type, a name, or a list punctuator.
    fn parse_params(&mut self, allow_unnamed: bool) -> Result<Vec<FnParam>, ParseError> {
        self.expect_punct(b"(", "'('")?;
        let mut params = Vec::new();
        if !self.at_punct(b")") {
            if self.at_kw(b"void") {
                self.next_tok();
            } else {
                loop {
                    if !self.at_spec_start() {
                        return self.fail("expected parameter type".to_string());
                    }
                    let spec = self.parse_spec()?;
                    // An unnamed parameter ends the specifier at
                    // `,`/`)`, but the specifier joiner only swallows
                    // a following type word when another word follows
                    // it (so `int long = 1` keeps `long` as the
                    // declarator name), leaving a bare qualifier
                    // (`unsigned`) when the type word itself is
                    // last. A lone qualifier followed by a type
                    // keyword is the whole specifier here.
                    if is_qualifier(&spec.words) && self.is_type_tok() {
                        self.next_tok();
                    }
                    // One `*` makes a pointer parameter; a second
                    // `*` fails at the name. A pointer typedef may
                    // not combine with `*`.
                    let mut pointer = spec.pointer;
                    if self.at_punct(b"*") {
                        if pointer {
                            return self.fail(
                                "pointer to pointer is not supported".to_string(),
                            );
                        }
                        self.next_tok();
                        pointer = true;
                    }
                    let name = if allow_unnamed
                        && (self.at_punct(b",")
                            || self.at_punct(b")")
                            || self.at_punct(b"["))
                    {
                        Vec::new()
                    } else {
                        self.expect_ident("parameter name")?.text
                    };
                    // An array parameter decays to a pointer; the
                    // size, when present, is parsed and discarded.
                    // An array typedef may not combine with `[]`.
                    let mut array = spec.is_array();
                    if self.at_punct(b"[") {
                        if array {
                            return self.fail(
                                "multi-dimensional arrays are not supported".to_string(),
                            );
                        }
                        self.next_tok();
                        if !self.at_punct(b"]") {
                            let _ = self.parse_assignment()?;
                        }
                        self.expect_punct(b"]", "']'")?;
                        array = true;
                    }
                    params.push(FnParam {
                        name,
                        pointer,
                        array,
                    });
                    if !self.at_punct(b",") {
                        break;
                    }
                    self.next_tok();
                }
            }
        }
        self.expect_punct(b")", "')'")?;
        Ok(params)
    }

    fn parse_funcdef(&mut self, spec: Spec, name: Vec<u8>) -> Result<FuncDef, ParseError> {
        let params = self.parse_params(false)?;
        // The function name lives in the file scope (so a later
        // call to it is not mistaken for an enumerator), and the
        // parameters live in their own scope enclosing the body.
        if let Some(file) = self.scopes.first_mut() {
            file.insert(name.clone());
        }
        let mut param_scope = HashSet::new();
        for p in &params {
            if !p.name.is_empty() {
                param_scope.insert(p.name.clone());
            }
        }
        self.scopes.push(param_scope);
        let body = self.parse_compound()?;
        self.scopes.pop();
        Ok(FuncDef {
            name,
            is_void: spec.words.as_slice() == b"void" && !spec.pointer,
            returns_pointer: spec.pointer,
            params,
            body,
        })
    }

    fn parse_funcdecl(&mut self, spec: Spec, name: Vec<u8>) -> Result<FuncDecl, ParseError> {
        if let Some(file) = self.scopes.first_mut() {
            file.insert(name.clone());
        }
        let params = self.parse_params(true)?;
        self.expect_punct(b";", "';'")?;
        Ok(FuncDecl {
            name,
            is_void: spec.words.as_slice() == b"void" && !spec.pointer,
            returns_pointer: spec.pointer,
            params,
        })
    }

    fn parse_declaration(&mut self) -> Result<Decl, ParseError> {
        if self.at_ident() && self.peek().text.as_slice() == b"typedef" {
            return self.fail(
                "typedef declarations are only allowed at file scope".to_string(),
            );
        }
        let spec = if self.is_type_tok() || self.at_ident() {
            self.parse_spec()?
        } else {
            return self.fail("expected type name".to_string());
        };
        if spec.words.as_slice() == b"enum"
            && ((self.at_ident()
                && self.peek_at(1).kind == Kind::Punct
                && self.peek_at(1).text == b"{")
                || self.at_punct(b"{"))
        {
            return self.fail(
                "enum definitions are only allowed at file scope".to_string(),
            );
        }
        let mut inits = Vec::new();
        if !self.at_punct(b";") {
            inits.push(self.parse_init_declarator(&spec)?);
            while self.at_punct(b",") {
                self.next_tok();
                inits.push(self.parse_init_declarator(&spec)?);
            }
        }
        self.expect_punct(b";", "';'")?;
        Ok(Decl {
            spec: spec.words,
            inits,
        })
    }

    fn parse_init_declarator(&mut self, base: &Spec) -> Result<InitDecl, ParseError> {
        // One `*` makes a pointer declarator (`char *p`); a second
        // `*` fails at the name, so `char **p` is rejected. A
        // pointer typedef may not combine with `*`, and an array
        // typedef may not combine with `[]`.
        let mut pointer = base.pointer;
        if self.at_punct(b"*") {
            if pointer {
                return self.fail("pointer to pointer is not supported".to_string());
            }
            self.next_tok();
            pointer = true;
        }
        let name = self.expect_ident("declarator name")?.text;
        // The name is in scope from here, including in its own
        // initializer, so a use of the name there is the variable,
        // not an enumerator of the same name.
        self.declare_name(&name);
        let mut array_len = base
            .array_len
            .map(|len| Expr::Int(len.to_string().into_bytes()));
        let mut array_unsized = base.array_unsized;
        if self.at_punct(b"[") {
            if array_len.is_some() || array_unsized {
                return self.fail(
                    "multi-dimensional arrays are not supported".to_string(),
                );
            }
            self.next_tok();
            if self.at_punct(b"]") {
                // `name[]`: sized only by a string initializer.
                array_unsized = true;
            } else {
                array_len = Some(self.parse_assignment()?);
            }
            self.expect_punct(b"]", "']'")?;
        }
        let is_array = array_len.is_some() || array_unsized;
        let init = if self.at_punct(b"=") {
            self.next_tok();
            if is_array && self.at_punct(b"{") {
                return self.fail("array initializers are not supported".to_string());
            }
            Some(self.parse_assignment()?)
        } else {
            None
        };
        Ok(InitDecl {
            name,
            pointer,
            array_len,
            array_unsized,
            init,
        })
    }

    fn parse_statement(&mut self) -> Result<Stmt, ParseError> {
        if self.at_punct(b"{") {
            return Ok(Stmt::Compound(self.parse_compound()?));
        }
        if self.at_kw(b"if") || self.at_kw(b"switch") {
            return self.parse_selection();
        }
        if self.at_kw(b"while") || self.at_kw(b"do") || self.at_kw(b"for") {
            return self.parse_iteration();
        }
        if self.at_kw(b"break") || self.at_kw(b"continue") || self.at_kw(b"return") {
            return self.parse_jump();
        }
        if self.at_kw(b"case")
            || self.at_kw(b"default")
            || (self.at_ident() && self.peek_at(1).kind == Kind::Punct && self.peek_at(1).text == b":")
        {
            return self.parse_labeled();
        }
        if self.at_punct(b";") {
            self.next_tok();
            return Ok(Stmt::Empty);
        }
        let e = self.parse_expression()?;
        self.expect_punct(b";", "';'")?;
        Ok(Stmt::Expr(e))
    }

    fn parse_compound(&mut self) -> Result<Block, ParseError> {
        self.expect_punct(b"{", "'{'")?;
        self.scopes.push(HashSet::new());
        let mut items = Vec::new();
        while !self.at_punct(b"}") {
            if self.at_end() {
                return self.fail("expected '}'".to_string());
            }
            items.push(self.parse_block_item()?);
        }
        self.next_tok();
        self.scopes.pop();
        Ok(Block(items))
    }

    fn parse_block_item(&mut self) -> Result<BlockItem, ParseError> {
        if self.at_spec_start() {
            Ok(BlockItem::Decl(self.parse_declaration()?))
        } else {
            Ok(BlockItem::Stmt(self.parse_statement()?))
        }
    }

    fn parse_selection(&mut self) -> Result<Stmt, ParseError> {
        if self.at_kw(b"if") {
            self.next_tok();
            self.expect_punct(b"(", "'('")?;
            let c = self.parse_expression()?;
            self.expect_punct(b")", "')'")?;
            let t = self.parse_statement()?;
            if self.at_kw(b"else") {
                self.next_tok();
                let e = self.parse_statement()?;
                return Ok(Stmt::If {
                    cond: c,
                    then: Box::new(t),
                    els: Some(Box::new(e)),
                });
            }
            return Ok(Stmt::If {
                cond: c,
                then: Box::new(t),
                els: None,
            });
        }
        self.expect_kw(b"switch", "'switch'")?;
        self.expect_punct(b"(", "'('")?;
        let c = self.parse_expression()?;
        self.expect_punct(b")", "')'")?;
        let b = self.parse_statement()?;
        Ok(Stmt::Switch {
            expr: c,
            body: Box::new(b),
        })
    }

    fn parse_iteration(&mut self) -> Result<Stmt, ParseError> {
        if self.at_kw(b"do") {
            self.next_tok();
            let b = self.parse_statement()?;
            self.expect_kw(b"while", "'while'")?;
            self.expect_punct(b"(", "'('")?;
            let c = self.parse_expression()?;
            self.expect_punct(b")", "')'")?;
            self.expect_punct(b";", "';'")?;
            return Ok(Stmt::DoWhile {
                body: Box::new(b),
                cond: c,
            });
        }
        if self.at_kw(b"while") {
            self.next_tok();
            self.expect_punct(b"(", "'('")?;
            let c = self.parse_expression()?;
            self.expect_punct(b")", "')'")?;
            let b = self.parse_statement()?;
            return Ok(Stmt::While {
                cond: c,
                body: Box::new(b),
            });
        }
        self.expect_kw(b"for", "'for'")?;
        self.expect_punct(b"(", "'('")?;
        // A `for` declaration's scope covers the condition,
        // update, and body.
        self.scopes.push(HashSet::new());
        let init = if self.at_spec_start() {
            ForInit::Decl(self.parse_for_declaration()?)
        } else if self.at_punct(b";") {
            ForInit::Empty
        } else {
            ForInit::Expr(self.parse_expression()?)
        };
        self.expect_punct(b";", "';'")?;
        let cond = if !self.at_punct(b";") {
            Some(self.parse_expression()?)
        } else {
            None
        };
        self.expect_punct(b";", "';'")?;
        let incr = if !self.at_punct(b")") {
            Some(self.parse_expression()?)
        } else {
            None
        };
        self.expect_punct(b")", "')'")?;
        let body = self.parse_statement()?;
        self.scopes.pop();
        Ok(Stmt::For {
            init,
            cond,
            incr,
            body: Box::new(body),
        })
    }

    fn parse_for_declaration(&mut self) -> Result<ForDecl, ParseError> {
        let spec = self.parse_spec()?;
        if spec.pointer || spec.is_array() {
            return self.fail(
                "pointer and array typedefs are not supported in for declarations"
                    .to_string(),
            );
        }
        let spec = spec.words;
        if self.at_ident() {
            let name = self.next_tok().text;
            self.declare_name(&name);
            if self.at_punct(b"=") {
                self.next_tok();
                let init = self.parse_assignment()?;
                return Ok(ForDecl {
                    spec,
                    name: Some(name),
                    init: Some(init),
                });
            }
            return Ok(ForDecl {
                spec,
                name: Some(name),
                init: None,
            });
        }
        Ok(ForDecl {
            spec,
            name: None,
            init: None,
        })
    }

    fn parse_jump(&mut self) -> Result<Stmt, ParseError> {
        let kw = self.next_tok().text;
        if kw == b"return" && !self.at_punct(b";") {
            let e = self.parse_expression()?;
            self.expect_punct(b";", "';'")?;
            return Ok(Stmt::Return(Some(e)));
        }
        self.expect_punct(b";", "';'")?;
        if kw == b"return" {
            Ok(Stmt::Return(None))
        } else if kw == b"break" {
            Ok(Stmt::Break)
        } else {
            Ok(Stmt::Continue)
        }
    }

    fn parse_labeled(&mut self) -> Result<Stmt, ParseError> {
        if self.at_kw(b"case") {
            self.next_tok();
            let label = self.parse_assignment()?;
            self.expect_punct(b":", "':'")?;
            let b = self.parse_statement()?;
            return Ok(Stmt::Case {
                label,
                body: Box::new(b),
            });
        }
        if self.at_kw(b"default") {
            self.next_tok();
            self.expect_punct(b":", "':'")?;
            let b = self.parse_statement()?;
            return Ok(Stmt::Default {
                body: Box::new(b),
            });
        }
        let name = self.expect_ident("label name")?.text;
        self.expect_punct(b":", "':'")?;
        let b = self.parse_statement()?;
        Ok(Stmt::GotoLabel {
            name,
            body: Box::new(b),
        })
    }
}

impl<'a> Parser<'a> {
    fn parse_expression(&mut self) -> Result<Expr, ParseError> {
        let first = self.parse_assignment()?;
        if !self.at_punct(b",") {
            return Ok(first);
        }
        let mut kids = vec![first];
        while self.at_punct(b",") {
            self.next_tok();
            kids.push(self.parse_assignment()?);
        }
        Ok(Expr::Comma(kids))
    }

    fn parse_assignment(&mut self) -> Result<Expr, ParseError> {
        let lhs = self.parse_conditional()?;
        let t = self.peek().clone();
        if t.kind == Kind::Punct && is_op_in(&t.text, &ASSIGN_OPS) {
            self.next_tok();
            if !is_lvalue(&lhs) {
                return Err(self.err(t.line, t.col, "invalid lvalue in assignment".to_string()));
            }
            let rhs = self.parse_assignment()?;
            return Ok(Expr::Assign {
                op: t.text,
                lhs: Box::new(lhs),
                rhs: Box::new(rhs),
            });
        }
        Ok(lhs)
    }

    /// Conditional (`?:`): between assignment and `||`, right-associative.
    /// The middle operand is a full expression (commas allowed); the
    /// else-branch is a conditional.
    fn parse_conditional(&mut self) -> Result<Expr, ParseError> {
        let cond = self.parse_logical_or()?;
        if !self.at_punct(b"?") {
            return Ok(cond);
        }
        self.next_tok();
        let then = self.parse_expression()?;
        self.expect_punct(b":", "':'")?;
        let els = self.parse_conditional()?;
        Ok(Expr::Ternary {
            cond: Box::new(cond),
            then: Box::new(then),
            els: Box::new(els),
        })
    }

    fn parse_logical_or(&mut self) -> Result<Expr, ParseError> {
        let mut n = self.parse_logical_and()?;
        while self.at_punct(b"||") {
            let op = self.next_tok().text;
            let rhs = self.parse_logical_and()?;
            n = Expr::BinOp {
                op,
                l: Box::new(n),
                r: Box::new(rhs),
            };
        }
        Ok(n)
    }

    fn parse_logical_and(&mut self) -> Result<Expr, ParseError> {
        let mut n = self.parse_bitwise_or()?;
        while self.at_punct(b"&&") {
            let op = self.next_tok().text;
            let rhs = self.parse_bitwise_or()?;
            n = Expr::BinOp {
                op,
                l: Box::new(n),
                r: Box::new(rhs),
            };
        }
        Ok(n)
    }

    fn parse_bitwise_or(&mut self) -> Result<Expr, ParseError> {
        let mut n = self.parse_bitwise_xor()?;
        while self.at_punct_op(&BITOR_OPS) {
            let op = self.next_tok().text;
            let rhs = self.parse_bitwise_xor()?;
            n = Expr::BinOp {
                op,
                l: Box::new(n),
                r: Box::new(rhs),
            };
        }
        Ok(n)
    }

    fn parse_bitwise_xor(&mut self) -> Result<Expr, ParseError> {
        let mut n = self.parse_bitwise_and()?;
        while self.at_punct_op(&BITXOR_OPS) {
            let op = self.next_tok().text;
            let rhs = self.parse_bitwise_and()?;
            n = Expr::BinOp {
                op,
                l: Box::new(n),
                r: Box::new(rhs),
            };
        }
        Ok(n)
    }

    fn parse_bitwise_and(&mut self) -> Result<Expr, ParseError> {
        let mut n = self.parse_equality()?;
        while self.at_punct_op(&BITAND_OPS) {
            let op = self.next_tok().text;
            let rhs = self.parse_equality()?;
            n = Expr::BinOp {
                op,
                l: Box::new(n),
                r: Box::new(rhs),
            };
        }
        Ok(n)
    }

    fn parse_equality(&mut self) -> Result<Expr, ParseError> {
        let mut n = self.parse_relational()?;
        while self.at_punct_op(&EQUALITY_OPS) {
            let op = self.next_tok().text;
            let rhs = self.parse_relational()?;
            n = Expr::BinOp {
                op,
                l: Box::new(n),
                r: Box::new(rhs),
            };
        }
        Ok(n)
    }

    fn parse_relational(&mut self) -> Result<Expr, ParseError> {
        let mut n = self.parse_shift()?;
        while self.at_punct_op(&RELATIONAL_OPS) {
            let op = self.next_tok().text;
            let rhs = self.parse_shift()?;
            n = Expr::BinOp {
                op,
                l: Box::new(n),
                r: Box::new(rhs),
            };
        }
        Ok(n)
    }

    fn parse_shift(&mut self) -> Result<Expr, ParseError> {
        let mut n = self.parse_additive()?;
        while self.at_punct_op(&SHIFT_OPS) {
            let op = self.next_tok().text;
            let rhs = self.parse_additive()?;
            n = Expr::BinOp {
                op,
                l: Box::new(n),
                r: Box::new(rhs),
            };
        }
        Ok(n)
    }

    fn parse_additive(&mut self) -> Result<Expr, ParseError> {
        let mut n = self.parse_multiplicative()?;
        while self.at_punct_op(&ADDITIVE_OPS) {
            let op = self.next_tok().text;
            let rhs = self.parse_multiplicative()?;
            n = Expr::BinOp {
                op,
                l: Box::new(n),
                r: Box::new(rhs),
            };
        }
        Ok(n)
    }

    fn parse_multiplicative(&mut self) -> Result<Expr, ParseError> {
        let mut n = self.parse_unary()?;
        while self.at_punct_op(&MULTIPLICATIVE_OPS) {
            let op = self.next_tok().text;
            let rhs = self.parse_unary()?;
            n = Expr::BinOp {
                op,
                l: Box::new(n),
                r: Box::new(rhs),
            };
        }
        Ok(n)
    }

    /// If the next tokens are a cast prefix `(spec)`, return the
    /// space-joined spec plus the number of tokens it spans
    /// (including both parentheses). A cast needs at least one
    /// specifier word (type keyword or qualifier) and a unary
    /// operand after the closing parenthesis, so `(x)` stays a
    /// parenthesized expression and `(unsigned)` alone stays a
    /// parenthesized variable.
    fn cast_prefix(&self) -> Option<(Vec<u8>, usize)> {
        if !self.at_punct(b"(") {
            return None;
        }
        let mut words: Vec<Vec<u8>> = Vec::new();
        let mut k = 1isize;
        loop {
            let t = self.peek_at(k);
            let is_word = (t.kind == Kind::Kw && is_op_in(&t.text, &TYPE_KWS))
                || (t.kind == Kind::Ident && is_qualifier(&t.text));
            if !is_word {
                break;
            }
            words.push(t.text.clone());
            k += 1;
        }
        if words.is_empty() {
            return None;
        }
        let close = self.peek_at(k);
        if !(close.kind == Kind::Punct && close.text == b")") {
            return None;
        }
        let after = self.peek_at(k + 1);
        let starts_unary = matches!(after.kind, Kind::Int | Kind::Char | Kind::String | Kind::Ident)
            || (after.kind == Kind::Punct
                && matches!(
                    after.text.as_slice(),
                    b"(" | b"!" | b"&" | b"++" | b"--" | b"-" | b"+" | b"~"
                ))
            || (after.kind == Kind::Kw && after.text == b"sizeof");
        if !starts_unary {
            return None;
        }
        Some((words.join(&b' '), (k + 1) as usize))
    }

    /// Parse the operand of `sizeof` (already consumed): the type
    /// form `sizeof (spec)`, or any unary expression. Either way the
    /// operand is unevaluated, so side effects in it never compile.
    fn parse_sizeof(&mut self) -> Result<Expr, ParseError> {
        if self.at_punct(b"(") {
            let mut words: Vec<Vec<u8>> = Vec::new();
            let mut k = 1isize;
            loop {
                let t = self.peek_at(k);
                let is_word = (t.kind == Kind::Kw && is_op_in(&t.text, &TYPE_KWS))
                    || (t.kind == Kind::Ident && is_qualifier(&t.text));
                if !is_word {
                    break;
                }
                words.push(t.text.clone());
                k += 1;
            }
            let close = self.peek_at(k);
            if !words.is_empty()
                && close.kind == Kind::Punct
                && close.text == b")"
            {
                for _ in 0..=k {
                    self.next_tok();
                }
                return Ok(Expr::SizeofType(words.join(&b' ')));
            }
        }
        Ok(Expr::Sizeof(Box::new(self.parse_unary()?)))
    }

    fn parse_unary(&mut self) -> Result<Expr, ParseError> {
        if self.at_kw(b"sizeof") {
            self.next_tok();
            return self.parse_sizeof();
        }
        if let Some((spec, n)) = self.cast_prefix() {
            for _ in 0..n {
                self.next_tok();
            }
            let e = self.parse_unary()?;
            return Ok(Expr::Cast {
                spec,
                e: Box::new(e),
            });
        }
        let t = self.peek().clone();
        if t.kind == Kind::Punct && is_op_in(&t.text, &UNARY_OPS) {
            self.next_tok();
            let e = self.parse_unary()?;
            return Ok(Expr::Unary {
                op: t.text,
                e: Box::new(e),
            });
        }
        self.parse_postfix()
    }

    fn parse_postfix(&mut self) -> Result<Expr, ParseError> {
        let mut n = self.parse_primary()?;
        loop {
            if self.at_punct(b"++") || self.at_punct(b"--") {
                let op = self.next_tok().text;
                n = Expr::Postfix {
                    op,
                    e: Box::new(n),
                };
            } else if self.at_punct(b"(") {
                self.next_tok();
                let mut args = Vec::new();
                if !self.at_punct(b")") {
                    args.push(self.parse_assignment()?);
                    while self.at_punct(b",") {
                        self.next_tok();
                        args.push(self.parse_assignment()?);
                    }
                }
                self.expect_punct(b")", "')'")?;
                n = Expr::Call {
                    callee: Box::new(n),
                    args,
                };
            } else if self.at_punct(b"[") {
                self.next_tok();
                let index = self.parse_assignment()?;
                self.expect_punct(b"]", "']'")?;
                n = Expr::Index {
                    base: Box::new(n),
                    index: Box::new(index),
                };
            } else {
                return Ok(n);
            }
        }
    }

    fn parse_primary(&mut self) -> Result<Expr, ParseError> {
        let t = self.peek().clone();
        match t.kind {
            Kind::Int => {
                self.next_tok();
                Ok(Expr::Int(t.text))
            }
            Kind::Char => {
                self.next_tok();
                Ok(Expr::Char(t.text))
            }
            Kind::Ident => {
                self.next_tok();
                // An enumerator name substitutes its value, unless
                // a declared name shadows it in an enclosing scope.
                if !self.is_declared(&t.text) {
                    if let Some(&value) = self.enum_consts.get(&t.text) {
                        return Ok(Expr::Int(value.to_string().into_bytes()));
                    }
                }
                Ok(Expr::Ident(t.text))
            }
            Kind::String => {
                let mut parts = Vec::new();
                let mut raw: Vec<u8> = Vec::new();
                let mut first = true;
                while self.peek().kind == Kind::String {
                    let t = self.next_tok();
                    parts.extend_from_slice(&t.text);
                    if first {
                        // First literal's raw text only: mirrors the C++
                        // `StringLiteral()[0]` quirk in printf/scanf calls.
                        raw = t.raw.expect("string token must carry raw text");
                        first = false;
                    }
                }
                Ok(Expr::StringLit {
                    cooked: parts,
                    raw,
                })
            }
            _ => {
                if self.at_punct(b"(") {
                    self.next_tok();
                    let e = self.parse_expression()?;
                    self.expect_punct(b")", "')'")?;
                    return Ok(Expr::Paren(Box::new(e)));
                }
                let t = self.peek().clone();
                Err(self.err(
                    t.line,
                    t.col,
                    format!("expected expression, found {}", desc(&t)),
                ))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ast::dump_tu;

    fn sexp(src: &str) -> String {
        let tu = parse(src.as_bytes()).expect("parse failed");
        let mut out = Vec::new();
        dump_tu(&tu, &mut out);
        String::from_utf8(out).expect("non-utf8 dump")
    }

    fn assert_sexp(src: &str, want: &str) {
        assert_eq!(sexp(src), want);
    }

    fn assert_err(src: &str, line: usize, col: usize, msg: &str) {
        match parse(src.as_bytes()) {
            Err(e) => {
                assert_eq!(e.line, line);
                assert_eq!(e.col, col);
                assert_eq!(e.msg, msg);
            }
            Ok(_) => panic!("expected parse error for {:?}", src),
        }
    }

    #[test]
    fn minimal_main() {
        assert_sexp(
            "int main() { return 0; }",
            "(tu (funcdef main (compound (return (int 0)))))",
        );
    }

    #[test]
    fn declarator_init_has_no_assign_node() {
        assert_sexp(
            "int x = 1 + 2 * 3;",
            "(tu (decl int (initdecl x (binop + (int 1) (binop * (int 2) (int 3))))))",
        );
    }

    #[test]
    fn assignment_is_right_associative() {
        assert_sexp(
            "int f() { a = b = 1; }",
            "(tu (funcdef f (compound (exprstmt (assign = (ident a) (assign = (ident b) (int 1)))))))",
        );
    }

    #[test]
    fn dangling_else_binds_inner() {
        assert_sexp(
            "int f() { if (a) if (b) c(); else d(); }",
            "(tu (funcdef f (compound (if (ident a) (ifelse (ident b) (exprstmt (call (ident c))) (exprstmt (call (ident d))))))))",
        );
    }

    #[test]
    fn while_loop() {
        assert_sexp(
            "int f() { while (i) i = i + 1; }",
            "(tu (funcdef f (compound (while (ident i) (exprstmt (assign = (ident i) (binop + (ident i) (int 1))))))))",
        );
    }

    #[test]
    fn do_while_loop() {
        assert_sexp(
            "int f() { do i = i + 1; while (i < 3); }",
            "(tu (funcdef f (compound (dowhile (exprstmt (assign = (ident i) (binop + (ident i) (int 1)))) (binop < (ident i) (int 3))))))",
        );
    }

    #[test]
    fn do_while_requires_semicolon() {
        assert_err(
            "int f() { do { } while (1) }",
            1,
            28,
            "expected ';', found '}'",
        );
    }

    #[test]
    fn for_empty_clauses() {
        assert_sexp(
            "int f() { for (;;) ; }",
            "(tu (funcdef f (compound (for (empty-init) (empty)))))",
        );
    }

    #[test]
    fn for_full_clauses() {
        assert_sexp(
            "int f() { for (i = 0; i < 3; i++) x(); }",
            "(tu (funcdef f (compound (for (assign = (ident i) (int 0)) (binop < (ident i) (int 3)) (postfix ++ (ident i)) (exprstmt (call (ident x)))))))",
        );
    }

    #[test]
    fn for_declaration_init() {
        assert_sexp(
            "int f() { for (int i = 0; i < 1; i++) ; }",
            "(tu (funcdef f (compound (for (fordecl int i (int 0)) (binop < (ident i) (int 1)) (postfix ++ (ident i)) (empty)))))",
        );
    }

    #[test]
    fn switch_case_break_default() {
        assert_sexp(
            "int f() { switch (x) { case 1: y(); break; default: z(); } }",
            "(tu (funcdef f (compound (switch (ident x) (compound (case (int 1) (exprstmt (call (ident y)))) (break) (default (exprstmt (call (ident z)))))))))",
        );
    }

    #[test]
    fn call_arguments() {
        assert_sexp(
            "int f() { g(a, b + 1); }",
            "(tu (funcdef f (compound (exprstmt (call (ident g) (ident a) (binop + (ident b) (int 1)))))))",
        );
    }

    #[test]
    fn comma_expression() {
        assert_sexp(
            "int f() { a = 1, b = 2; }",
            "(tu (funcdef f (compound (exprstmt (comma (assign = (ident a) (int 1)) (assign = (ident b) (int 2)))))))",
        );
    }

    #[test]
    fn parens_are_kept() {
        assert_sexp(
            "int f() { x = (a + b) * c; }",
            "(tu (funcdef f (compound (exprstmt (assign = (ident x) (binop * (paren (binop + (ident a) (ident b))) (ident c)))))))",
        );
    }

    #[test]
    fn unary_and_postfix_operators() {
        assert_sexp(
            "int f() { x = !a; y = &b; i++; --j; }",
            "(tu (funcdef f (compound (exprstmt (assign = (ident x) (unary ! (ident a)))) (exprstmt (assign = (ident y) (unary & (ident b)))) (exprstmt (postfix ++ (ident i))) (exprstmt (unary -- (ident j))))))",
        );
    }

    #[test]
    fn adjacent_strings_paste() {
        assert_sexp(
            "int f() { s(\"a\" \"b\"); }",
            "(tu (funcdef f (compound (exprstmt (call (ident s) (string ab))))))",
        );
    }

    #[test]
    fn logic_precedence() {
        assert_sexp(
            "int f() { x = a || b && !c; }",
            "(tu (funcdef f (compound (exprstmt (assign = (ident x) (binop || (ident a) (binop && (ident b) (unary ! (ident c)))))))))",
        );
    }

    #[test]
    fn ternary_basic() {
        assert_sexp(
            "int f() { x = a ? b : c; }",
            "(tu (funcdef f (compound (exprstmt (assign = (ident x) (ternary (ident a) (ident b) (ident c)))))))",
        );
    }

    #[test]
    fn ternary_right_associative() {
        assert_sexp(
            "int f() { x = a ? b : c ? d : e; }",
            "(tu (funcdef f (compound (exprstmt (assign = (ident x) (ternary (ident a) (ident b) (ternary (ident c) (ident d) (ident e))))))))",
        );
    }

    #[test]
    fn ternary_condition_is_logical_or() {
        assert_sexp(
            "int f() { x = a || b ? c : d; }",
            "(tu (funcdef f (compound (exprstmt (assign = (ident x) (ternary (binop || (ident a) (ident b)) (ident c) (ident d)))))))",
        );
    }

    #[test]
    fn ternary_middle_allows_comma() {
        assert_sexp(
            "int f() { x = a ? b, c : d; }",
            "(tu (funcdef f (compound (exprstmt (assign = (ident x) (ternary (ident a) (comma (ident b) (ident c)) (ident d)))))))",
        );
    }

    #[test]
    fn error_ternary_missing_colon() {
        assert_err("int f() { x = a ? b; }", 1, 20, "expected ':', found ';'");
    }

    #[test]
    fn goto_label() {
        assert_sexp(
            "int f() { lab: x = 1; }",
            "(tu (funcdef f (compound (goto-label lab (exprstmt (assign = (ident x) (int 1)))))))",
        );
    }

    #[test]
    fn decls_with_and_without_init() {
        assert_sexp("int g;\nint;", "(tu (decl int (initdecl g)) (decl int))");
    }

    #[test]
    fn multi_word_type_specs() {
        assert_sexp(
            "int f() { unsigned int a = 1; const char b = 2; long c = 3; long long d = 4; }",
            "(tu (funcdef f (compound (decl unsigned int (initdecl a (int 1))) (decl const char (initdecl b (int 2))) (decl long (initdecl c (int 3))) (decl long long (initdecl d (int 4))))))",
        );
        assert_sexp(
            "unsigned int g = 1;",
            "(tu (decl unsigned int (initdecl g (int 1))))",
        );
        assert_sexp(
            "int f() { for (unsigned int i = 0; i < 3; i++) {} }",
            "(tu (funcdef f (compound (for (fordecl unsigned int i (int 0)) (binop < (ident i) (int 3)) (postfix ++ (ident i)) (compound)))))",
        );
        // A declarator named like a qualifier keeps its name.
        assert_sexp(
            "int f() { int long = 1; }",
            "(tu (funcdef f (compound (decl int (initdecl long (int 1))))))",
        );
        // Multi-word parameter types parse (parameters are kept,
        // the type is not).
        assert_sexp(
            "int f(unsigned int x) { return 0; }",
            "(tu (funcdef f (params (param x)) (compound (return (int 0)))))",
        );
    }

    #[test]
    fn multiple_declarators() {
        assert_sexp(
            "int f() { int a = 1, b = 2, c; }",
            "(tu (funcdef f (compound (decl int (initdecl a (int 1)) (initdecl b (int 2)) (initdecl c)))))",
        );
        assert_sexp(
            "int a = 1, b = 2;",
            "(tu (decl int (initdecl a (int 1)) (initdecl b (int 2))))",
        );
    }

    #[test]
    fn array_and_pointer_declarators() {
        assert_sexp(
            "int f() { char a[3]; }",
            "(tu (funcdef f (compound (decl char (initdecl a (int 3))))))",
        );
        assert_sexp(
            "int f() { char *p = &x; }",
            "(tu (funcdef f (compound (decl char (initdecl p (pointer) (unary & (ident x)))))))",
        );
        assert_sexp(
            "int f() { char *a[2]; int b = 1, *q = &b; }",
            "(tu (funcdef f (compound (decl char (initdecl a (pointer) (int 2))) (decl int (initdecl b (int 1)) (initdecl q (pointer) (unary & (ident b)))))))",
        );
        assert_sexp(
            "char g[4];",
            "(tu (decl char (initdecl g (int 4))))",
        );
        assert_sexp(
            "int f() { char t[] = \"hi\"; int a[]; }",
            "(tu (funcdef f (compound (decl char (initdecl t (unsized) (string hi))) (decl int (initdecl a (unsized))))))",
        );
    }

    #[test]
    fn subscript_and_dereference() {
        assert_sexp(
            "int f() { a[1] + a[i]; }",
            "(tu (funcdef f (compound (exprstmt (binop + (index (ident a) (int 1)) (index (ident a) (ident i)))))))",
        );
        assert_sexp(
            "int f() { *p = 1; a[0] = 2; a[i]++; p[1] = *q; }",
            "(tu (funcdef f (compound (exprstmt (assign = (unary * (ident p)) (int 1))) (exprstmt (assign = (index (ident a) (int 0)) (int 2))) (exprstmt (postfix ++ (index (ident a) (ident i)))) (exprstmt (assign = (index (ident p) (int 1)) (unary * (ident q)))))))",
        );
        assert_sexp(
            "int f() { x = a * *p; }",
            "(tu (funcdef f (compound (exprstmt (assign = (ident x) (binop * (ident a) (unary * (ident p))))))))",
        );
    }

    #[test]
    fn array_declarator_errors() {
        assert_err("int f() { int a[3][4]; }", 1, 19, "expected ';', found '['");
        assert_err(
            "int f() { char **p; }",
            1,
            17,
            "expected declarator name, found '*'",
        );
        assert_err(
            "int f() { int a[2] = {1, 2}; }",
            1,
            22,
            "array initializers are not supported",
        );
    }

    #[test]
    fn chars_dump_raw_with_escaping() {
        assert_sexp(
            "int f() { c = 'x'; d = '\\n'; }",
            "(tu (funcdef f (compound (exprstmt (assign = (ident c) (char 'x'))) (exprstmt (assign = (ident d) (char '\\\\n'))))))",
        );
    }

    #[test]
    fn string_escapes_dump_escaped() {
        assert_sexp(
            "int f() { s(\"a\\nb\"); }",
            "(tu (funcdef f (compound (exprstmt (call (ident s) (string a\\nb))))))",
        );
    }

    #[test]
    fn params_are_kept() {
        assert_sexp(
            "int add(int a, int b) { return a; }",
            "(tu (funcdef add (params (param a) (param b)) (compound (return (ident a)))))",
        );
        assert_sexp(
            "int f(void) { return 0; }",
            "(tu (funcdef f (compound (return (int 0)))))",
        );
        assert_sexp(
            "int f(int a, char *p, char s[], int t[4]) { }",
            "(tu (funcdef f (params (param a) (param p (pointer)) (param s (array)) (param t (array))) (compound)))",
        );
    }

    #[test]
    fn prototypes_parse() {
        assert_sexp(
            "int f(int a, char *p);",
            "(tu (funcdecl f (params (param a) (param p (pointer)))))",
        );
        assert_sexp(
            "int f(int, char *);",
            "(tu (funcdecl f (params (param) (param (pointer)))))",
        );
        assert_sexp("int f(void);", "(tu (funcdecl f))");
        assert_sexp("int f();", "(tu (funcdecl f))");
        assert_sexp(
            "int f(char s[], int t[4]);",
            "(tu (funcdecl f (params (param s (array)) (param t (array)))))",
        );
        // Parentheses in an array size do not confuse the
        // prototype/definition scan.
        assert_sexp(
            "int f(char s[(1)]); int main() { return 0; }",
            "(tu (funcdecl f (params (param s (array)))) (funcdef main (compound (return (int 0)))))",
        );
    }

    #[test]
    fn prototype_errors() {
        // A second `*` still fails at the parameter name.
        match parse(b"int f(int **p);") {
            Err(e) => assert!(
                e.msg.contains("parameter name"),
                "unexpected error: {}",
                e.msg
            ),
            Ok(_) => panic!("double-pointer prototype accepted"),
        }
        // A variadic prototype is rejected: `...` does not
        // start a parameter type.
        match parse(b"int f(int, ...);") {
            Err(e) => assert!(
                e.msg.contains("parameter type"),
                "unexpected error: {}",
                e.msg
            ),
            Ok(_) => panic!("variadic prototype accepted"),
        }
        // Prototypes are file-scope only: in a block this is a
        // declaration, and `(` cannot follow a declarator name.
        match parse(b"int main() { int f(int); return 0; }") {
            Err(e) => assert!(
                e.msg.contains("';'"),
                "unexpected error: {}",
                e.msg
            ),
            Ok(_) => panic!("block-scope prototype accepted"),
        }
    }

    #[test]
    fn param_requires_name() {
        match parse(b"int f(int) { }") {
            Err(e) => assert!(
                e.msg.contains("parameter name"),
                "unexpected error: {}",
                e.msg
            ),
            Ok(_) => panic!("unnamed parameter accepted"),
        }
        match parse(b"int f(int **p) { }") {
            Err(e) => assert!(
                e.msg.contains("parameter name"),
                "unexpected error: {}",
                e.msg
            ),
            Ok(_) => panic!("double-pointer parameter accepted"),
        }
    }

    #[test]
    fn break_continue_bare_return() {
        assert_sexp(
            "int f() { while (1) { break; continue; } return; }",
            "(tu (funcdef f (compound (while (int 1) (compound (break) (continue))) (return))))",
        );
    }

    #[test]
    fn bitwise_and_shift_assign_ops() {
        assert_sexp(
            "int f() { a &= b; a <<= 2; }",
            "(tu (funcdef f (compound (exprstmt (assign &= (ident a) (ident b))) (exprstmt (assign <<= (ident a) (int 2))))))",
        );
        assert_sexp(
            "int f() { a |= b ^= c; }",
            "(tu (funcdef f (compound (exprstmt (assign |= (ident a) (assign ^= (ident b) (ident c)))))))",
        );
    }

    #[test]
    fn typedefs_parse() {
        assert_sexp(
            "typedef int MyInt; int main() { MyInt a = 5; }",
            "(tu (typedef MyInt int) (funcdef main (compound (decl int (initdecl a (int 5))))))",
        );
        assert_sexp(
            "typedef int *PInt; typedef char Arr[3]; typedef PInt PP; int main() { PInt p = 0; Arr s = \"hi\"; }",
            "(tu (typedef PInt int (pointer)) (typedef Arr char (int 3)) (typedef PP int (pointer)) (funcdef main (compound (decl int (initdecl p (pointer) (int 0))) (decl char (initdecl s (int 3) (string hi))))))",
        );
        assert_sexp(
            "typedef unsigned int U; U f(U x) { return x; } int main() { return f(7); }",
            "(tu (typedef U unsigned int) (funcdef f (params (param x)) (compound (return (ident x)))) (funcdef main (compound (return (call (ident f) (int 7))))))",
        );
        assert_sexp(
            "typedef int *P; int f(P);",
            "(tu (typedef P int (pointer)) (funcdecl f (params (param (pointer)))))",
        );
        // An identical redefinition is allowed.
        assert_sexp(
            "typedef int T; typedef int T; int main() { T a = 1; }",
            "(tu (typedef T int) (typedef T int) (funcdef main (compound (decl int (initdecl a (int 1))))))",
        );
        // Qualifiers may precede or follow the typedef name.
        assert_sexp(
            "typedef int T; const T a = 1; T const b = 2;",
            "(tu (typedef T int) (decl int const (initdecl a (int 1))) (decl int const (initdecl b (int 2))))",
        );
    }

    #[test]
    fn typedef_errors() {
        let cases: &[(&str, &str)] = &[
            (
                "typedef int T; typedef char T;",
                "conflicting typedef for 'T'",
            ),
            (
                "int main() { typedef int T; return 0; }",
                "only allowed at file scope",
            ),
            ("typedef int T = 1;", "take no initializer"),
            ("typedef int A, B;", "multiple typedef declarators"),
            ("typedef int *P; P *q;", "pointer to pointer"),
            ("typedef char A[3]; A b[2];", "multi-dimensional arrays"),
            ("typedef char A[n];", "array size must be an integer literal"),
            ("typedef char A[0];", "array size must be positive"),
            ("typedef int T; int T x;", "beside a typedef name"),
            (
                "typedef int *P; int main() { for (P p = 0;;) { } return 0; }",
                "not supported in for declarations",
            ),
        ];
        for (src, want) in cases {
            match parse(src.as_bytes()) {
                Err(e) => assert!(
                    e.msg.contains(want),
                    "for {:?}: got error {:?}, want {:?}",
                    src,
                    e.msg,
                    want
                ),
                Ok(_) => panic!("accepted {:?}", src),
            }
        }
    }

    #[test]
    fn enums_parse() {
        assert_sexp(
            "enum Color { RED, GREEN = 5, BLUE }; int main() { printf(\"%d,%d,%d\", RED, GREEN, BLUE); }",
            "(tu (decl enum Color) (funcdef main (compound (exprstmt (call (ident printf) (string %d,%d,%d) (int 0) (int 5) (int 6))))))",
        );
        assert_sexp(
            "enum { A = 'a', B = A + 1, C, }; enum State state = C; int main() { return 0; }",
            "(tu (decl enum) (decl enum State (initdecl state (int 99))) (funcdef main (compound (return (int 0)))))",
        );
        // A declared name shadows an enumerator in its scope.
        assert_sexp(
            "enum { RED }; int main() { int RED = 5; printf(\"%d\", RED); }",
            "(tu (decl enum) (funcdef main (compound (decl int (initdecl RED (int 5))) (exprstmt (call (ident printf) (string %d) (ident RED))))))",
        );
        // An enum return type still parses as a function.
        assert_sexp(
            "enum Color f() { return 1; }",
            "(tu (funcdef f (compound (return (int 1)))))",
        );
    }

    #[test]
    fn enum_errors() {
        let cases: &[(&str, &str)] = &[
            ("enum { A }; enum { A };", "duplicate enumerator 'A'"),
            ("enum { A, A };", "duplicate enumerator 'A'"),
            (
                "int x = 1; enum { A = x };",
                "must be a constant expression",
            ),
            (
                "enum { A = f() };",
                "must be a constant expression",
            ),
            (
                "enum { A = 1 / 0 };",
                "must be a constant expression",
            ),
            ("enum { };", "enumerator name"),
            (
                "int main() { enum Color { RED }; return 0; }",
                "only allowed at file scope",
            ),
        ];
        for (src, want) in cases {
            match parse(src.as_bytes()) {
                Err(e) => assert!(
                    e.msg.contains(want),
                    "for {:?}: got error {:?}, want {:?}",
                    src,
                    e.msg,
                    want
                ),
                Ok(_) => panic!("accepted {:?}", src),
            }
        }
    }

    #[test]
    fn bitwise_and_shift_precedence() {
        assert_sexp(
            "int f() { a = b & c | d; }",
            "(tu (funcdef f (compound (exprstmt (assign = (ident a) (binop | (binop & (ident b) (ident c)) (ident d)))))))",
        );
        assert_sexp(
            "int f() { a = b ^ c & d; }",
            "(tu (funcdef f (compound (exprstmt (assign = (ident a) (binop ^ (ident b) (binop & (ident c) (ident d))))))))",
        );
        assert_sexp(
            "int f() { a = b << c + d; }",
            "(tu (funcdef f (compound (exprstmt (assign = (ident a) (binop << (ident b) (binop + (ident c) (ident d))))))))",
        );
        assert_sexp(
            "int f() { a = b & c == d; }",
            "(tu (funcdef f (compound (exprstmt (assign = (ident a) (binop & (ident b) (binop == (ident c) (ident d))))))))",
        );
        assert_sexp(
            "int f() { a = b << c << d; }",
            "(tu (funcdef f (compound (exprstmt (assign = (ident a) (binop << (binop << (ident b) (ident c)) (ident d)))))))",
        );
        assert_sexp(
            "int f() { a = b && c | d; }",
            "(tu (funcdef f (compound (exprstmt (assign = (ident a) (binop && (ident b) (binop | (ident c) (ident d))))))))",
        );
    }

    #[test]
    fn define_expands_before_parsing() {
        assert_sexp(
            "#define N 5\nint f() { return N; }",
            "(tu (funcdef f (compound (return (int 5)))))",
        );
        assert_sexp(
            "#define T int\nT x = 1;",
            "(tu (decl int (initdecl x (int 1))))",
        );
    }

    #[test]
    fn sizeof_forms() {
        assert_sexp(
            "int f() { return sizeof (char); }",
            "(tu (funcdef f (compound (return (sizeof-type char)))))",
        );
        assert_sexp(
            "int f() { return sizeof (unsigned int); }",
            "(tu (funcdef f (compound (return (sizeof-type unsigned int)))))",
        );
        assert_sexp(
            "int f() { return sizeof x; }",
            "(tu (funcdef f (compound (return (sizeof (ident x))))))",
        );
        assert_sexp(
            "int f() { return sizeof (x + 1); }",
            "(tu (funcdef f (compound (return (sizeof (paren (binop + (ident x) (int 1))))))))",
        );
        assert_sexp(
            "int f() { return sizeof (char) + 1; }",
            "(tu (funcdef f (compound (return (binop + (sizeof-type char) (int 1))))))",
        );
    }

    #[test]
    fn casts_are_noop_nodes() {
        assert_sexp(
            "int f() { return (int) x; }",
            "(tu (funcdef f (compound (return (cast int (ident x))))))",
        );
        assert_sexp(
            "int f() { return (unsigned int) (char) x; }",
            "(tu (funcdef f (compound (return (cast unsigned int (cast char (ident x)))))))",
        );
        assert_sexp(
            "int f() { a = (int) b + 1; }",
            "(tu (funcdef f (compound (exprstmt (assign = (ident a) (binop + (cast int (ident b)) (int 1)))))))",
        );
        assert_sexp(
            "int f() { a = (int) -b; }",
            "(tu (funcdef f (compound (exprstmt (assign = (ident a) (cast int (unary - (ident b))))))))",
        );
        // A lone parenthesized qualifier stays a variable, not a cast.
        assert_sexp(
            "int f() { (unsigned); }",
            "(tu (funcdef f (compound (exprstmt (paren (ident unsigned))))))",
        );
    }

    #[test]
    fn unary_minus_plus_tilde() {
        assert_sexp(
            "int f() { return -5; }",
            "(tu (funcdef f (compound (return (unary - (int 5))))))",
        );
        assert_sexp(
            "int f() { return +5; }",
            "(tu (funcdef f (compound (return (unary + (int 5))))))",
        );
        assert_sexp(
            "int f() { return ~a; }",
            "(tu (funcdef f (compound (return (unary ~ (ident a))))))",
        );
        assert_sexp(
            "int f() { return --a; }",
            "(tu (funcdef f (compound (return (unary -- (ident a))))))",
        );
        assert_sexp(
            "int f() { a = b - -c; }",
            "(tu (funcdef f (compound (exprstmt (assign = (ident a) (binop - (ident b) (unary - (ident c))))))))",
        );
    }

    #[test]
    fn nested_empty_compound() {
        assert_sexp("int f() { {} }", "(tu (funcdef f (compound (compound))))");
    }

    #[test]
    fn error_bad_lvalue_points_at_operator() {
        assert_err("int f() { 1 = 2; }", 1, 13, "invalid lvalue in assignment");
    }

    #[test]
    fn error_expected_declaration() {
        assert_err(
            "int x; }",
            1,
            8,
            "expected declaration or function definition",
        );
    }

    #[test]
    fn error_missing_semicolon() {
        assert_err("int f() { x = 1 }", 1, 17, "expected ';', found '}'");
    }

    #[test]
    fn error_expected_expression() {
        assert_err("int f() { x = ; }", 1, 15, "expected expression, found ';'");
    }

    #[test]
    fn pasted_string_cooks_all_keeps_first_raw() {
        use crate::ast::{BlockItem, Expr, External, Stmt};

        let tu = parse(b"int f() { s(\"a\" \"b\"); }").expect("parse failed");
        let stmt = match &tu.0[..] {
            [External::FuncDef(f)] => match &f.body.0[..] {
                [BlockItem::Stmt(s)] => s,
                _ => panic!("expected one statement"),
            },
            _ => panic!("expected one function"),
        };
        match stmt {
            Stmt::Expr(Expr::Call { args, .. }) => match &args[..] {
                [Expr::StringLit { cooked, raw }] => {
                    assert_eq!(cooked.as_slice(), b"ab");
                    assert_eq!(raw.as_slice(), b"a");
                }
                _ => panic!("expected one argument"),
            },
            _ => panic!("expected call"),
        }
    }
}
