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

/// A storage-class specifier word (`static`, `extern`, `auto`,
/// `register`). They stay identifiers elsewhere — all four can
/// still name variables — so they are recognized only inside
/// specifier position (see `parse_spec`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum StorageClass {
    Static,
    Extern,
    Auto,
    Register,
}

fn storage_word(text: &[u8]) -> Option<StorageClass> {
    match text {
        b"static" => Some(StorageClass::Static),
        b"extern" => Some(StorageClass::Extern),
        b"auto" => Some(StorageClass::Auto),
        b"register" => Some(StorageClass::Register),
        _ => None,
    }
}

/// A parsed type specifier: joined words plus declarator flags
/// contributed by a typedef name in first position (a pointer or
/// array alias). Plain specifiers carry no flags. A storage
/// class, when present, is stripped from the words: `static` and
/// `extern` are meaningful only at file scope, `auto` and
/// `register` never change anything in this model.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Spec {
    words: Vec<u8>,
    /// Pointer depth carried by a typedef alias (`0`, `1`, `2`).
    ptr_depth: u8,
    /// Array dimensions carried by a typedef alias, one entry
    /// per bracket (`None` for an unsized dimension).
    dims: Vec<Option<i32>>,
    /// Function-pointer signature carried by a typedef alias
    /// (`typedef int (*cb)(int);`), if any.
    fn_sig: Option<FuncSig>,
    storage: Option<StorageClass>,
}

impl Spec {
    fn plain(words: Vec<u8>) -> Self {
        Spec {
            words,
            ptr_depth: 0,
            dims: Vec::new(),
            fn_sig: None,
            storage: None,
        }
    }

    fn is_array(&self) -> bool {
        !self.dims.is_empty()
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
                    || storage_word(&self.peek().text).is_some()
                    || self.typedefs.contains_key(&self.peek().text))))
            && {
                let n = self.peek_at(1);
                (n.kind == Kind::Kw && is_op_in(&n.text, &TYPE_KWS))
                    || n.kind == Kind::Ident
                    // A `*` after the candidate means a pointer
                    // declarator follows (`unsigned char *f`), so
                    // the candidate is part of the specifier.
                    || (n.kind == Kind::Punct && n.text == b"*")
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

    /// Resolve the collected specifier words: strip storage
    /// classes (two are an error), then expand a typedef name in
    /// the remaining words. Every word beside a typedef name must
    /// be a qualifier.
    fn resolve_spec(&self, words: Vec<Vec<u8>>) -> Result<Spec, ParseError> {
        let mut storage = None;
        let mut kept: Vec<Vec<u8>> = Vec::new();
        for w in words {
            if let Some(s) = storage_word(&w) {
                if storage.is_some() {
                    return self.fail(
                        "multiple storage classes in one specifier".to_string(),
                    );
                }
                storage = Some(s);
            } else {
                kept.push(w);
            }
        }
        let words = kept;
        let at = words
            .iter()
            .position(|w| self.typedefs.contains_key(w));
        let Some(at) = at else {
            let mut spec = Spec::plain(words.join(&b' '));
            spec.storage = storage;
            return Ok(spec);
        };
        // A typedef name is hidden in any enclosing scope where
        // a variable, parameter, or `for` declarator of the same
        // name is declared (the same scope tracking that hides
        // enumerators): using it as a specifier there is a
        // parse error, as in C, rather than silently expanding
        // the alias. The name is visible again when the scope
        // ends, because `scopes` pops with the block.
        if self.is_declared(&words[at]) {
            return self.fail(format!(
                "typedef name '{}' is hidden by a declaration of the same name",
                String::from_utf8_lossy(&words[at])
            ));
        }
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
            ptr_depth: alias.ptr_depth,
            dims: alias.dims.clone(),
            fn_sig: alias.fn_sig.clone(),
            storage,
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
            let mut spec = self.parse_spec()?;
            if let Some(is_def) = self.scan_fnptr_return_def() {
                return self.parse_fnptr_return(spec, is_def);
            }
            // `*`s before the name and a `(` after it are a
            // pointer-returning function declarator (`char
            // *f()`, `int **f()`); `*`s anywhere else belong
            // to the declaration below, so only take them
            // when a name and a `(` follow the run of stars.
            // Depth above 2 (a third `*`, counting a pointer
            // typedef) is an error.
            let mut stars = 0isize;
            while self.peek_at(stars).kind == Kind::Punct
                && self.peek_at(stars).text == b"*"
            {
                stars += 1;
            }
            if stars > 0
                && self.peek_at(stars).kind == Kind::Ident
                && self.peek_at(stars + 1).kind == Kind::Punct
                && self.peek_at(stars + 1).text == b"("
            {
                if spec.fn_sig.is_some() {
                    return self.fail(
                        "pointer to function pointer is not supported".to_string(),
                    );
                }
                if spec.ptr_depth + stars as u8 > 2 {
                    return self.fail(
                        "pointer to pointer to pointer is not supported".to_string(),
                    );
                }
                for _ in 0..stars {
                    self.next_tok();
                }
                spec.ptr_depth += stars as u8;
            }
            if self.at_ident() {
                let name = self.next_tok().text;
                if self.at_punct(b"(") {
                    let returns_fn_sig = spec.fn_sig.clone();
                    let is_decl = self.after_param_list_is_semicolon();
                    let params = self.parse_params(!is_decl)?;
                    if is_decl {
                        return Ok(External::FuncDecl(
                            self.parse_funcdecl(spec, name, returns_fn_sig, params)?,
                        ));
                    }
                    return Ok(External::FuncDef(
                        self.parse_funcdef(spec, name, returns_fn_sig, params)?,
                    ));
                }
            }
            self.p = mark;
            return Ok(External::Decl(self.parse_declaration(true)?));
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
        if spec.storage.is_some() {
            return self.fail(
                "storage class specifiers are not allowed with typedef".to_string(),
            );
        }
        // Stars add to the aliased depth (a pointer alias plus
        // `*` is depth 2); a third level is an error.
        let mut ptr_depth = spec.ptr_depth;
        while self.at_punct(b"*") {
            if ptr_depth >= 2 {
                return self.fail(
                    "pointer to pointer to pointer is not supported".to_string(),
                );
            }
            self.next_tok();
            ptr_depth += 1;
        }
        if spec.fn_sig.is_some() && ptr_depth > 0 {
            return self.fail(
                "pointer to function pointer is not supported".to_string(),
            );
        }
        if self.at_punct(b"(") {
            // Parenthesized function-pointer declarator:
            // `typedef int (*cb)(int);`. The star inside the
            // parentheses is the function-pointer star; stars
            // before it are the pointed-to return type's.
            let (inner, name) = self.parse_paren_stars_name(false)?;
            let paren_dims = self.parse_paren_dims()?;
            if !self.at_punct(b"(") {
                return self.fail(
                    "parenthesized typedef declarators without a parameter list are not supported"
                        .to_string(),
                );
            }
            if inner != 1 {
                return self.fail(if inner == 0 {
                    "parenthesized function declarators are not supported"
                        .to_string()
                } else {
                    "pointer to function pointer is not supported".to_string()
                });
            }
            let sig = self.parse_fn_sig_params(&spec, ptr_depth)?;
            if self.at_punct(b"[") {
                return self.fail(
                    "arrays of function pointers are not supported".to_string(),
                );
            }
            let mut dims: Vec<Option<i32>> = Vec::new();
            for dim in paren_dims {
                match dim {
                    None => dims.push(None),
                    Some(Expr::Int(bytes)) => {
                        let len = int_text_value(&bytes);
                        if len <= 0 {
                            return self.fail(
                                "array size must be positive".to_string(),
                            );
                        }
                        dims.push(Some(len));
                    }
                    Some(_) => {
                        return self.fail(
                            "array size must be an integer literal".to_string(),
                        );
                    }
                }
            }
            if dims.len() > 1 {
                return self.fail(
                    "multi-dimensional function-pointer arrays are not supported"
                        .to_string(),
                );
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
                ptr_depth: 0,
                dims,
                fn_sig: Some(sig),
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
            return Ok(External::Typedef(decl));
        }
        let name = self.expect_ident("declarator name")?.text;
        // Brackets prepend dimensions before the aliased ones
        // (an array alias plus `[N]` is an outer dimension:
        // `A b[2]` with `A` an array of 3 is a 2x3 array).
        let mut declarator_dims = Vec::new();
        while self.at_punct(b"[") {
            self.next_tok();
            if self.at_punct(b"]") {
                declarator_dims.push(None);
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
                declarator_dims.push(Some(len));
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
        let mut dims = declarator_dims;
        dims.extend(spec.dims.iter().copied());
        if spec.fn_sig.is_some() && dims.len() > 1 {
            return self.fail(
                "multi-dimensional function-pointer arrays are not supported"
                    .to_string(),
            );
        }
        let decl = TypedefDecl {
            name: name.clone(),
            spec: spec.words,
            ptr_depth,
            dims,
            fn_sig: spec.fn_sig.clone(),
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
        let mut spec = Spec::plain(spec_words.join(&b' '));
        // As in `parse_external`: `enum Tag *f()` is a
        // pointer-returning function.
        if self.at_punct(b"*")
            && self.peek_at(1).kind == Kind::Ident
            && self.peek_at(2).kind == Kind::Punct
            && self.peek_at(2).text == b"("
        {
            self.next_tok();
            spec.ptr_depth = 1;
        }
        if self.at_ident()
            && self.peek_at(1).kind == Kind::Punct
            && self.peek_at(1).text == b"("
        {
            let name = self.next_tok().text;
            let is_decl = self.after_param_list_is_semicolon();
            let params = self.parse_params(!is_decl)?;
            if is_decl {
                let d = self.parse_funcdecl(spec, name, None, params)?;
                return Ok(External::FuncDecl(d));
            }
            let f = self.parse_funcdef(spec, name, None, params)?;
            return Ok(External::FuncDef(f));
        }
        if self.at_punct(b";") {
            self.next_tok();
            return Ok(External::Decl(Decl {
                spec: spec.words,
                inits: Vec::new(),
                is_extern: false,
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
            is_extern: false,
        }))
    }

    /// Whether the tokens from here spell a function returning
    /// a function pointer: `[ *s ] ( [*s] name ( params ) ) (
    /// params )` followed by `{` (a definition, returned as
    /// `Some(true)`) or `;` (a prototype, `Some(false)`).
    /// Anything else is `None`, and the normal external paths
    /// run. The scan only reads tokens; the cursor does not
    /// move. Leading stars are the pointed-to return type's
    /// pointer depth; the stars inside the parentheses must
    /// be exactly one (checked while parsing).
    fn scan_fnptr_return_def(&self) -> Option<bool> {
        let mut k: isize = 0;
        let tok = |k: isize| self.peek_at(k);
        while tok(k).kind == Kind::Punct && tok(k).text == b"*" {
            k += 1;
        }
        if !(tok(k).kind == Kind::Punct && tok(k).text == b"(") {
            return None;
        }
        k += 1;
        while tok(k).kind == Kind::Punct && tok(k).text == b"*" {
            k += 1;
        }
        if tok(k).kind != Kind::Ident {
            return None;
        }
        k += 1;
        if !(tok(k).kind == Kind::Punct && tok(k).text == b"(") {
            return None;
        }
        k = self.scan_balanced(k)?;
        if !(tok(k).kind == Kind::Punct && tok(k).text == b")") {
            return None;
        }
        k += 1;
        if !(tok(k).kind == Kind::Punct && tok(k).text == b"(") {
            return None;
        }
        k = self.scan_balanced(k)?;
        if !(tok(k).kind == Kind::Punct && tok(k).text == b")") {
            return None;
        }
        k += 1;
        if tok(k).kind == Kind::Punct && tok(k).text == b"{" {
            Some(true)
        } else if tok(k).kind == Kind::Punct && tok(k).text == b";" {
            Some(false)
        } else {
            None
        }
    }

    /// Offset just past the `)` matching the `(` at `start`
    /// (relative token offset), counting nested parentheses, or
    /// `None` when the end of input comes first. The cursor
    /// does not move.
    fn scan_balanced(&self, start: isize) -> Option<isize> {
        let mut depth = 0i32;
        let mut k = start;
        loop {
            let t = self.peek_at(k);
            if t.kind == Kind::End {
                return None;
            }
            if t.kind == Kind::Punct {
                if t.text == b"(" {
                    depth += 1;
                } else if t.text == b")" {
                    depth -= 1;
                    if depth == 0 {
                        return Some(k);
                    }
                }
            }
            k += 1;
        }
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
                    // Storage classes are not allowed in parameters,
                    // except `register`, which is ignored.
                    if let Some(s) = spec.storage {
                        if s != StorageClass::Register {
                            return self.fail(
                                "storage class specifiers are not allowed in parameters"
                                    .to_string(),
                            );
                        }
                    }
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
                    // Stars add to the aliased depth; a third
                    // level is an error.
                    let mut ptr_depth = spec.ptr_depth;
                    while self.at_punct(b"*") {
                        if ptr_depth >= 2 {
                            return self.fail(
                                "pointer to pointer to pointer is not supported"
                                    .to_string(),
                            );
                        }
                        self.next_tok();
                        ptr_depth += 1;
                    }
                    // Stars on top of a function-pointer typedef
                    // are a pointer to a function pointer.
                    if spec.fn_sig.is_some() && ptr_depth > 0 {
                        return self.fail(
                            "pointer to function pointer is not supported"
                                .to_string(),
                        );
                    }
                    if self.at_punct(b"(") {
                        // Parenthesized parameter declarator, as
                        // in init declarators: a parameter list
                        // after it makes a function-pointer
                        // parameter, otherwise a plain pointer
                        // parameter with redundant parentheses.
                        let (inner, paren_name) =
                            self.parse_paren_stars_name(allow_unnamed)?;
                        let paren_dims = self.parse_paren_dims()?;
                        if self.at_punct(b"(") {
                            // A function-pointer typedef does not
                            // compose with a parenthesized
                            // declarator (see parse_init_declarator).
                            if spec.fn_sig.is_some() {
                                return self.fail(
                                    "typedef function-pointer types do not support parenthesized declarators"
                                        .to_string(),
                                );
                            }
                            if inner != 1 {
                                return self.fail(if inner == 0 {
                                    "parenthesized function declarators are not supported"
                                        .to_string()
                                } else {
                                    "pointer to function pointer is not supported"
                                        .to_string()
                                });
                            }
                            let sig = self.parse_fn_sig_params(&spec, ptr_depth)?;
                            if self.at_punct(b"[") {
                                return self.fail(
                                    "arrays of function pointers are not supported"
                                        .to_string(),
                                );
                            }
                            // Dimensions inside the parentheses
                            // are the array dimensions, converted
                            // like declarator brackets: the first
                            // may be unsized or any expression
                            // (recorded when literal, discarded
                            // otherwise), later ones must be
                            // positive integer literals.
                            let mut paren_dims_i32: Vec<Option<i32>> = Vec::new();
                            for (k, dim) in paren_dims.iter().enumerate() {
                                match dim {
                                    None => {
                                        if k == 0 {
                                            paren_dims_i32.push(None);
                                        } else {
                                            return self.fail(
                                                "array size must be an integer literal"
                                                    .to_string(),
                                            );
                                        }
                                    }
                                    Some(Expr::Int(bytes)) => {
                                        let len = int_text_value(bytes);
                                        if k == 0 {
                                            paren_dims_i32.push(Some(len));
                                        } else if len <= 0 {
                                            return self.fail(
                                                "array size must be positive".to_string(),
                                            );
                                        } else {
                                            paren_dims_i32.push(Some(len));
                                        }
                                    }
                                    Some(_) => {
                                        if k == 0 {
                                            paren_dims_i32.push(None);
                                        } else {
                                            return self.fail(
                                                "array size must be an integer literal"
                                                    .to_string(),
                                            );
                                        }
                                    }
                                }
                            }
                            let mut dims = paren_dims_i32;
                            dims.extend(spec.dims.iter().copied());
                            if dims.len() > 1 {
                                return self.fail(
                                    "multi-dimensional function-pointer arrays are not supported"
                                        .to_string(),
                                );
                            }
                            params.push(FnParam {
                                name: paren_name,
                                ptr_depth: 0,
                                dims,
                                fn_sig: Some(Box::new(sig)),
                            });
                            if !self.at_punct(b",") {
                                break;
                            }
                            self.next_tok();
                            continue;
                        }
                        if spec.fn_sig.is_some() && inner > 1 {
                            return self.fail(
                                "pointer to function pointer is not supported"
                                    .to_string(),
                            );
                        }
                        let total_depth = ptr_depth + inner;
                        if total_depth > 2 {
                            return self.fail(
                                "pointer to pointer to pointer is not supported"
                                    .to_string(),
                            );
                        }
                        if !paren_dims.is_empty() || self.at_punct(b"[") {
                            return self.fail(
                                "pointer to array is not supported".to_string(),
                            );
                        }
                        params.push(FnParam {
                            name: paren_name,
                            ptr_depth: if spec.fn_sig.is_some() {
                                0
                            } else {
                                total_depth
                            },
                            dims: spec.dims.clone(),
                            fn_sig: spec.fn_sig.clone().map(Box::new),
                        });
                        if !self.at_punct(b",") {
                            break;
                        }
                        self.next_tok();
                        continue;
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
                    // Brackets prepend dimensions before the
                    // aliased ones. An array parameter decays to a
                    // pointer: the outermost size is discarded,
                    // the inner sizes are the stride. Every
                    // dimension after the first declarator bracket
                    // must be an integer literal; the first may be
                    // empty or any expression, which is discarded
                    // as before.
                    let mut declarator_dims = Vec::new();
                    let mut first_bracket = true;
                    while self.at_punct(b"[") {
                        self.next_tok();
                        if self.at_punct(b"]") {
                            if first_bracket {
                                declarator_dims.push(None);
                            } else {
                                return self.fail(
                                    "array size must be an integer literal".to_string(),
                                );
                            }
                        } else {
                            let size = self.parse_assignment()?;
                            match size {
                                Expr::Int(bytes) if !first_bracket => {
                                    let len = int_text_value(&bytes);
                                    if len <= 0 {
                                        return self.fail(
                                            "array size must be positive".to_string(),
                                        );
                                    }
                                    declarator_dims.push(Some(len));
                                }
                                Expr::Int(bytes) => {
                                    // Outermost dimension: recorded
                                    // when literal, discarded as
                                    // before when it is not.
                                    declarator_dims.push(Some(int_text_value(&bytes)));
                                }
                                _ => {
                                    if first_bracket {
                                        declarator_dims.push(None);
                                    } else {
                                        return self.fail(
                                            "array size must be an integer literal"
                                                .to_string(),
                                        );
                                    }
                                }
                            }
                        }
                        first_bracket = false;
                        self.expect_punct(b"]", "']'")?;
                    }
                    let mut dims = declarator_dims;
                    dims.extend(spec.dims.iter().copied());
                    if spec.fn_sig.is_some() && dims.len() > 1 {
                        return self.fail(
                            "multi-dimensional function-pointer arrays are not supported"
                                .to_string(),
                        );
                    }
                    params.push(FnParam {
                        name,
                        ptr_depth,
                        dims,
                        fn_sig: spec.fn_sig.clone().map(Box::new),
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

    /// Parse a function returning a function pointer in the
    /// direct spelling: `[ *s ] ( [*] name ( params ) ) (
    /// params )`, followed by a body (definition) or `;`
    /// (prototype, decided by the caller from the lookahead).
    /// Leading stars are the pointed-to return type's pointer
    /// depth; the inner star count must be exactly one.
    fn parse_fnptr_return(
        &mut self,
        spec: Spec,
        is_def: bool,
    ) -> Result<External, ParseError> {
        let mut ret_depth = 0u8;
        while self.at_punct(b"*") {
            if ret_depth >= 2 {
                return self.fail(
                    "pointer to pointer to pointer is not supported".to_string(),
                );
            }
            self.next_tok();
            ret_depth += 1;
        }
        self.expect_punct(b"(", "'('")?;
        let mut inner = 0u8;
        while self.at_punct(b"*") {
            inner += 1;
            self.next_tok();
        }
        let name = self.expect_ident("declarator name")?.text;
        if inner != 1 {
            return self.fail(if inner == 0 {
                "parenthesized function declarators are not supported".to_string()
            } else {
                "pointer to function pointer is not supported".to_string()
            });
        }
        let params = self.parse_params(!is_def)?;
        self.expect_punct(b")", "')'")?;
        let sig = self.parse_fn_sig_params(&spec, ret_depth)?;
        if is_def {
            Ok(External::FuncDef(self.parse_funcdef(
                spec,
                name,
                Some(sig),
                params,
            )?))
        } else {
            Ok(External::FuncDecl(self.parse_funcdecl(
                spec,
                name,
                Some(sig),
                params,
            )?))
        }
    }

    fn parse_funcdef(
        &mut self,
        spec: Spec,
        name: Vec<u8>,
        returns_fn_sig: Option<FuncSig>,
        params: Vec<FnParam>,
    ) -> Result<FuncDef, ParseError> {
        // A function pointer that itself returns a function
        // pointer is deferred: the returned signature may not
        // carry one of its own.
        if returns_fn_sig
            .as_ref()
            .is_some_and(|s| s.returns_fn_sig.is_some())
        {
            return self.fail(
                "nested function pointer returns are not supported".to_string(),
            );
        }
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
            is_void: spec.words.as_slice() == b"void"
                && spec.ptr_depth == 0
                && returns_fn_sig.is_none(),
            returns_ptr_depth: spec.ptr_depth,
            returns_fn_sig: returns_fn_sig.map(Box::new),
            params,
            body,
        })
    }

    fn parse_funcdecl(
        &mut self,
        spec: Spec,
        name: Vec<u8>,
        returns_fn_sig: Option<FuncSig>,
        params: Vec<FnParam>,
    ) -> Result<FuncDecl, ParseError> {
        if returns_fn_sig
            .as_ref()
            .is_some_and(|s| s.returns_fn_sig.is_some())
        {
            return self.fail(
                "nested function pointer returns are not supported".to_string(),
            );
        }
        if let Some(file) = self.scopes.first_mut() {
            file.insert(name.clone());
        }
        self.expect_punct(b";", "';'")?;
        Ok(FuncDecl {
            name,
            is_void: spec.words.as_slice() == b"void"
                && spec.ptr_depth == 0
                && returns_fn_sig.is_none(),
            returns_ptr_depth: spec.ptr_depth,
            returns_fn_sig: returns_fn_sig.map(Box::new),
            params,
        })
    }

    /// Parse a declaration; `file_scope` tells whether it sits
    /// at file scope or inside a function body. Storage classes
    /// are checked here: `static` and `extern` are file-scope
    /// only, `auto` and `register` are ignored everywhere. A
    /// file-scope `extern` declarator without an initializer is a
    /// non-defining declaration: it is kept, flagged
    /// `is_extern`, and emits nothing, so a later definition
    /// still registers normally and a use without one fails
    /// in code generation.
    fn parse_declaration(&mut self, file_scope: bool) -> Result<Decl, ParseError> {
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
        match spec.storage {
            Some(StorageClass::Static) if !file_scope => {
                return self.fail("static locals are not supported".to_string());
            }
            Some(StorageClass::Extern) if !file_scope => {
                return self.fail(
                    "extern declarations are only allowed at file scope".to_string(),
                );
            }
            _ => {}
        }
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
            is_extern: file_scope && spec.storage == Some(StorageClass::Extern),
        })
    }

    /// Parse a braced initializer list, with the `{` ahead:
    /// `{e, e, ...}`, with an optional trailing comma. Lists may
    /// nest (`{{1, 2}, {3, 4}}` for multi-dimensional arrays);
    /// an empty list and designated initializers (`[i] =` or
    /// `.name =`) are parse errors. Whether a nested list is
    /// meaningful depends on the declarator: code generation
    /// rejects nested lists for scalars and one-dimensional
    /// arrays.
    fn parse_init_list(&mut self) -> Result<Expr, ParseError> {
        self.expect_punct(b"{", "'{'")?;
        if self.at_punct(b"}") {
            return self.fail("empty initializer list is not supported".to_string());
        }
        let mut elems = Vec::new();
        loop {
            if self.at_punct(b"{") {
                elems.push(self.parse_init_list()?);
                if self.at_punct(b",") {
                    self.next_tok();
                    if self.at_punct(b"}") {
                        break;
                    }
                    continue;
                }
                break;
            }
            if self.at_punct(b"[") || self.at_punct(b".") {
                return self.fail(
                    "designated initializers are not supported".to_string(),
                );
            }
            elems.push(self.parse_assignment()?);
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
        Ok(Expr::InitList(elems))
    }

    /// Consume a parenthesized declarator head `( * * name )`
    /// and return the inner star count and the name (empty
    /// for an unnamed parameter when `allow_unnamed` and the
    /// next token is `)`).
    fn parse_paren_stars_name(
        &mut self,
        allow_unnamed: bool,
    ) -> Result<(u8, Vec<u8>), ParseError> {
        self.expect_punct(b"(", "'('")?;
        let mut inner = 0u8;
        while self.at_punct(b"*") {
            inner += 1;
            self.next_tok();
        }
        let name = if allow_unnamed && self.at_punct(b")") {
            Vec::new()
        } else {
            self.expect_ident("declarator name")?.text
        };
        self.expect_punct(b")", "')'")?;
        Ok((inner, name))
    }

    /// Consume bracket dimensions following a parenthesized
    /// declarator head (`(*arr[3])`, `(*arr[])`): one entry
    /// per bracket, `None` for an empty bracket. Callers
    /// convert the size expressions to their own dimension
    /// form (declarator expressions, parameter sizes, or
    /// typedef sizes).
    fn parse_paren_dims(&mut self) -> Result<Vec<Option<Expr>>, ParseError> {
        let mut dims = Vec::new();
        while self.at_punct(b"[") {
            self.next_tok();
            if self.at_punct(b"]") {
                dims.push(None);
            } else {
                dims.push(Some(self.parse_assignment()?));
            }
            self.expect_punct(b"]", "']'")?;
        }
        Ok(dims)
    }

    /// Parse a function-pointer parameter list after a
    /// parenthesized declarator head, and build its
    /// signature. Nested function-pointer parameters and
    /// array return types are rejected. When the base
    /// specifier is itself a function-pointer typedef, the
    /// pointed-to function returns a function pointer of
    /// that signature; returning a pointer to one, or a
    /// function pointer that itself returns one, is
    /// rejected.
    fn parse_fn_sig_params(
        &mut self,
        base: &Spec,
        returns_ptr_depth: u8,
    ) -> Result<FuncSig, ParseError> {
        if !base.dims.is_empty() {
            return self.fail("function return type cannot be an array".to_string());
        }
        if let Some(inner) = &base.fn_sig {
            if returns_ptr_depth > 0 {
                return self.fail(
                    "pointer to function pointer is not supported".to_string(),
                );
            }
            if inner.returns_fn_sig.is_some() {
                return self.fail(
                    "nested function pointer returns are not supported".to_string(),
                );
            }
        }
        let params = self.parse_params(true)?;
        if params.iter().any(|p| p.fn_sig.is_some()) {
            return self.fail(
                "nested function pointer parameters are not supported".to_string(),
            );
        }
        Ok(FuncSig {
            is_void: base.words.as_slice() == b"void"
                && returns_ptr_depth == 0
                && base.fn_sig.is_none(),
            returns_ptr_depth,
            params,
            returns_fn_sig: base.fn_sig.clone().map(Box::new),
        })
    }

    /// Parse an optional `= init` tail for a declarator.
    fn parse_opt_init(&mut self) -> Result<Option<Expr>, ParseError> {
        if self.at_punct(b"=") {
            self.next_tok();
            if self.at_punct(b"{") {
                Ok(Some(self.parse_init_list()?))
            } else {
                Ok(Some(self.parse_assignment()?))
            }
        } else {
            Ok(None)
        }
    }

    fn parse_init_declarator(&mut self, base: &Spec) -> Result<InitDecl, ParseError> {
        // Stars add to the aliased depth (`char **p` is depth 2);
        // a third level is an error.
        let mut ptr_depth = base.ptr_depth;
        while self.at_punct(b"*") {
            if ptr_depth >= 2 {
                return self.fail(
                    "pointer to pointer to pointer is not supported".to_string(),
                );
            }
            self.next_tok();
            ptr_depth += 1;
        }
        // Stars on top of a function-pointer typedef are a
        // pointer to a function pointer, which is deferred.
        if base.fn_sig.is_some() && ptr_depth > 0 {
            return self.fail(
                "pointer to function pointer is not supported".to_string(),
            );
        }
        if self.at_punct(b"(") {
            // Parenthesized declarator: `(*fp)(params)` is a
            // function pointer, `(*fp)` alone is a plain
            // pointer declarator with redundant parentheses.
            // The star inside the parentheses is the
            // function-pointer star; stars before it are the
            // return type's. A second inner star is a pointer
            // to a function pointer and is rejected, as is a
            // parameter list after a bare parenthesized name.
            let (inner, name) = self.parse_paren_stars_name(false)?;
            self.declare_name(&name);
            let paren_dims = self.parse_paren_dims()?;
            if self.at_punct(b"(") {
                // A function-pointer typedef does not compose
                // with a parenthesized declarator: the
                // parameter list here would make the pointed-to
                // function return another function pointer.
                if base.fn_sig.is_some() {
                    return self.fail(
                        "typedef function-pointer types do not support parenthesized declarators"
                            .to_string(),
                    );
                }
                if inner != 1 {
                    return self.fail(if inner == 0 {
                        "parenthesized function declarators are not supported"
                            .to_string()
                    } else {
                        "pointer to function pointer is not supported"
                            .to_string()
                    });
                }
                let sig = self.parse_fn_sig_params(base, ptr_depth)?;
                if self.at_punct(b"[") {
                    return self.fail(
                        "arrays of function pointers are not supported".to_string(),
                    );
                }
                // Dimensions inside the parentheses are the
                // array dimensions (`(*arr[3])(int)`); at most
                // one, and sized dimensions must be integer
                // literals (an empty bracket is sized by a
                // braced-list initializer, as for arrays).
                let mut dims = paren_dims;
                dims.extend(base.dims.iter().map(|d| {
                    d.as_ref()
                        .map(|len| Expr::Int(len.to_string().into_bytes()))
                }));
                if dims.len() > 1 {
                    return self.fail(
                        "multi-dimensional function-pointer arrays are not supported"
                            .to_string(),
                    );
                }
                for dim in &dims {
                    if let Some(e) = dim {
                        if !matches!(e, Expr::Int(_)) {
                            return self.fail(
                                "array size must be an integer literal".to_string(),
                            );
                        }
                    }
                }
                let init = self.parse_opt_init()?;
                return Ok(InitDecl {
                    name,
                    ptr_depth: 0,
                    dims,
                    fn_sig: Some(sig),
                    init,
                });
            }
            if !paren_dims.is_empty() {
                return self.fail("pointer to array is not supported".to_string());
            }
            if base.fn_sig.is_some() && inner > 1 {
                return self.fail(
                    "pointer to function pointer is not supported".to_string(),
                );
            }
            let total_depth = ptr_depth + inner;
            if total_depth > 2 {
                return self.fail(
                    "pointer to pointer to pointer is not supported".to_string(),
                );
            }
            if self.at_punct(b"[") {
                return self.fail("pointer to array is not supported".to_string());
            }
            let dims = base.dims.iter().map(|d| {
                d.as_ref()
                    .map(|len| Expr::Int(len.to_string().into_bytes()))
            }).collect();
            let init = self.parse_opt_init()?;
            return Ok(InitDecl {
                name,
                ptr_depth: if base.fn_sig.is_some() { 0 } else { total_depth },
                dims,
                fn_sig: base.fn_sig.clone(),
                init,
            });
        }
        let name = self.expect_ident("declarator name")?.text;
        // The name is in scope from here, including in its own
        // initializer, so a use of the name there is the variable,
        // not an enumerator of the same name.
        self.declare_name(&name);
        // Brackets prepend dimensions before the aliased ones.
        let mut declarator_dims = Vec::new();
        while self.at_punct(b"[") {
            self.next_tok();
            if self.at_punct(b"]") {
                // `name[]`: sized only by a string or list
                // initializer.
                declarator_dims.push(None);
            } else {
                declarator_dims.push(Some(self.parse_assignment()?));
            }
            self.expect_punct(b"]", "']'")?;
        }
        let mut dims = declarator_dims;
        dims.extend(base.dims.iter().map(|d| {
            d.as_ref()
                .map(|len| Expr::Int(len.to_string().into_bytes()))
        }));
        if base.fn_sig.is_some() && dims.len() > 1 {
            return self.fail(
                "multi-dimensional function-pointer arrays are not supported"
                    .to_string(),
            );
        }
        let init = if self.at_punct(b"=") {
            self.next_tok();
            if self.at_punct(b"{") {
                Some(self.parse_init_list()?)
            } else {
                Some(self.parse_assignment()?)
            }
        } else {
            None
        };
        Ok(InitDecl {
            name,
            ptr_depth,
            dims,
            fn_sig: base.fn_sig.clone(),
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
        if self.at_kw(b"break")
            || self.at_kw(b"continue")
            || self.at_kw(b"return")
            || self.at_kw(b"goto")
        {
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
            Ok(BlockItem::Decl(self.parse_declaration(false)?))
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
        match spec.storage {
            Some(StorageClass::Static) => {
                return self.fail("static locals are not supported".to_string());
            }
            Some(StorageClass::Extern) => {
                return self.fail(
                    "extern declarations are only allowed at file scope".to_string(),
                );
            }
            _ => {}
        }
        if spec.ptr_depth > 0 || spec.is_array() {
            return self.fail(
                "pointer and array typedefs are not supported in for declarations"
                    .to_string(),
            );
        }
        if spec.fn_sig.is_some() {
            return self.fail(
                "function pointer typedefs are not supported in for declarations"
                    .to_string(),
            );
        }
        let spec = spec.words;
        if self.at_ident() {
            let name = self.next_tok().text;
            self.declare_name(&name);
            if self.at_punct(b"=") {
                self.next_tok();
                let init = if self.at_punct(b"{") {
                    self.parse_init_list()?
                } else {
                    self.parse_assignment()?
                };
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
        if kw == b"goto" {
            let name = self.expect_ident("label name")?.text;
            self.expect_punct(b";", "';'")?;
            return Ok(Stmt::Goto { name });
        }
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
    fn braced_initializer_lists_parse() {
        assert_sexp(
            "int a[3] = {1, 2, 3};",
            "(tu (decl int (initdecl a (int 3) (initlist (int 1) (int 2) (int 3)))))",
        );
        assert_sexp(
            "int f() { char s[] = {'h', 'i', 0}; int x = {5}; int y = {1,}; }",
            "(tu (funcdef f (compound (decl char (initdecl s (unsized) (initlist (char 'h') (char 'i') (int 0)))) (decl int (initdecl x (initlist (int 5)))) (decl int (initdecl y (initlist (int 1)))))))",
        );
        // A nested list parses (it is meaningful for
        // multi-dimensional declarators); code generation
        // rejects it for one-dimensional ones.
        assert_sexp(
            "int a[2] = {{1}, 2};",
            "(tu (decl int (initdecl a (int 2) (initlist (initlist (int 1)) (int 2)))))",
        );
        // A braced list is an initializer, not an expression.
        for src in [
            "int a[2] = {};",
            "int a[3] = {[1] = 5};",
            "int main() { int x; x = {1}; return 0; }",
            "int main() { return {1}; }",
        ] {
            assert!(
                parse(src.as_bytes()).is_err(),
                "accepted {:?}",
                src
            );
        }
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
        assert_sexp(
            "int f() { int a[3][4]; }",
            "(tu (funcdef f (compound (decl int (initdecl a (int 3) (int 4))))))",
        );
        // A third level of indirection is still an error.
        match parse(b"int f() { char ***p; }") {
            Err(e) => assert!(
                e.msg.contains("pointer to pointer to pointer"),
                "unexpected error: {}",
                e.msg
            ),
            Ok(_) => panic!("triple-pointer declarator accepted"),
        }
        // `int a[2] = {1, 2};` used to be rejected here; braced
        // initializers are supported now, and the list-shape
        // errors live in `braced_initializer_lists_parse`.
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
    fn pointer_return_declarators_parse() {
        // The dump does not encode the return pointer, so these
        // pin acceptance; the signature itself is pinned by the
        // compile tests below.
        assert_sexp(
            "char *f() { return 0; }",
            "(tu (funcdef f (compound (return (int 0)))))",
        );
        assert_sexp(
            "char *f(void); int main() { return 0; }",
            "(tu (funcdecl f) (funcdef main (compound (return (int 0)))))",
        );
        assert_sexp(
            "unsigned char *f(int x) { return 0; }",
            "(tu (funcdef f (params (param x)) (compound (return (int 0)))))",
        );
        // A multi-word specifier joins before a pointer
        // declarator, while a declarator named like a type
        // word still keeps its name.
        assert_sexp(
            "unsigned int *p;",
            "(tu (decl unsigned int (initdecl p (pointer))))",
        );
        assert_sexp(
            "int long = 1;",
            "(tu (decl int (initdecl long (int 1))))",
        );
        // A `*` without a following `(` is still a variable
        // declaration, not a function.
        assert_sexp(
            "char *p;",
            "(tu (decl char (initdecl p (pointer))))",
        );
        // Pointer-to-pointer returns parse (the dump does not
        // encode the return depth); a third level is an error.
        assert_sexp(
            "char **f() { return 0; }",
            "(tu (funcdef f (compound (return (int 0)))))",
        );
        assert_sexp(
            "typedef char *CP; CP *f() { return 0; }",
            "(tu (typedef CP char (pointer)) (funcdef f (compound (return (int 0)))))",
        );
        for src in [
            "char ***f() { return 0; }",
            "typedef char *CP; CP **f() { return 0; }",
        ] {
            assert!(
                parse(src.as_bytes()).is_err(),
                "accepted triple-pointer return: {:?}",
                src
            );
        }
    }

    #[test]
    fn prototype_errors() {
        // A double-pointer parameter parses; a third `*`
        // is still an error.
        assert_sexp(
            "int f(int **p);",
            "(tu (funcdecl f (params (param p (pointer) (pointer)))))",
        );
        match parse(b"int f(int ***p);") {
            Err(e) => assert!(
                e.msg.contains("pointer to pointer to pointer"),
                "unexpected error: {}",
                e.msg
            ),
            Ok(_) => panic!("triple-pointer prototype accepted"),
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
        match parse(b"int f(int ***p) { }") {
            Err(e) => assert!(
                e.msg.contains("pointer to pointer to pointer"),
                "unexpected error: {}",
                e.msg
            ),
            Ok(_) => panic!("triple-pointer parameter accepted"),
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
    fn goto_and_label() {
        assert_sexp(
            "int f() { goto done; x = 1; done: x = 2; }",
            "(tu (funcdef f (compound (goto done) (exprstmt (assign = (ident x) (int 1))) (goto-label done (exprstmt (assign = (ident x) (int 2)))))))",
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
            (
                "typedef int *P; P **q;",
                "pointer to pointer to pointer",
            ),
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
    fn typedef_fnptr_parse() {
        assert_sexp(
            "typedef int (*cb)(int);",
            "(tu (typedef cb int (fnsig (params (param)))))",
        );
        assert_sexp(
            "typedef int (*cb)(int); int main() { cb fp = 0; }",
            "(tu (typedef cb int (fnsig (params (param)))) (funcdef main (compound (decl int (initdecl fp (fnsig (params (param))) (int 0))))))",
        );
        assert_sexp(
            "typedef int (*cb)(int); cb f(cb x) { return x; }",
            "(tu (typedef cb int (fnsig (params (param)))) (funcdef f (params (param x (fnsig (params (param))))) (compound (return (ident x)))))",
        );
        assert_sexp(
            "typedef char *(*scb)(int); int main() { scb gp = 0; }",
            "(tu (typedef scb char (fnsig (params (param)) (pointer))) (funcdef main (compound (decl char (initdecl gp (fnsig (params (param)) (pointer)) (int 0))))))",
        );
        assert_sexp(
            "typedef int (*cb)(int); typedef cb (*get)(void);",
            "(tu (typedef cb int (fnsig (params (param)))) (typedef get int (fnsig (fnsig (params (param))))))",
        );
        assert_sexp(
            "int (*f(int x))(int) { return 0; }",
            "(tu (funcdef f (params (param x)) (compound (return (int 0)))))",
        );
        assert_sexp(
            "int (*f(int))(int);",
            "(tu (funcdecl f (params (param))))",
        );
        assert_sexp(
            "int (*arr[3])(int);",
            "(tu (decl int (initdecl arr (int 3) (fnsig (params (param))))))",
        );
        assert_sexp(
            "typedef int (*cb[3])(int);",
            "(tu (typedef cb int (int 3) (fnsig (params (param)))))",
        );
        assert_sexp(
            "int f(int (*arr[3])(int));",
            "(tu (funcdecl f (params (param arr (array) (fnsig (params (param)))))))",
        );
        assert_err(
            "typedef int (*cb)(int);\ntypedef cb *p;",
            2,
            13,
            "pointer to function pointer is not supported",
        );
        for (src, want) in [
            (
                "typedef int (*cb)(int); int main() { cb (*fp)(int); }",
                "do not support parenthesized declarators",
            ),
            (
                "typedef int (*cb)(int); int main() { for (cb fp = 0;;) { } return 0; }",
                "not supported in for declarations",
            ),
        ] {
            match parse(src.as_bytes()) {
                Err(e) => assert!(
                    e.msg.contains(want),
                    "for {:?}: got error {:?}, want {:?}",
                    src, e.msg, want
                ),
                Ok(_) => panic!("accepted {:?}", src),
            }
        }
    }

    #[test]
    fn pointer_depth_and_dimensions_parse() {
        // One `(pointer)` marker per star, one size marker per
        // bracket, in declarator order.
        assert_sexp(
            "int **pp;",
            "(tu (decl int (initdecl pp (pointer) (pointer))))",
        );
        assert_sexp(
            "int a[2][3];",
            "(tu (decl int (initdecl a (int 2) (int 3))))",
        );
        assert_sexp(
            "int f(int **pp, int a[][3]) { return 0; }",
            "(tu (funcdef f (params (param pp (pointer) (pointer)) (param a (array) (array))) (compound (return (int 0)))))",
        );
        // Typedef composition: a pointer alias plus `*` is depth
        // 2; an array alias plus `[N]` prepends a dimension.
        assert_sexp(
            "typedef int *P; P *q;",
            "(tu (typedef P int (pointer)) (decl int (initdecl q (pointer) (pointer))))",
        );
        assert_sexp(
            "typedef char A[3]; A b[2];",
            "(tu (typedef A char (int 3)) (decl char (initdecl b (int 2) (int 3))))",
        );
        assert_sexp(
            "typedef int **PP; typedef char Row[3]; Row m[2];",
            "(tu (typedef PP int (pointer) (pointer)) (typedef Row char (int 3)) (decl char (initdecl m (int 2) (int 3))))",
        );
        // Nested initializer lists parse (code generation
        // decides whether the declarator can use one).
        assert_sexp(
            "int a[2][2] = {{1, 2}, {3, 4}};",
            "(tu (decl int (initdecl a (int 2) (int 2) (initlist (initlist (int 1) (int 2)) (initlist (int 3) (int 4))))))",
        );
        // A third level of indirection is still an error, as
        // is a non-literal inner parameter dimension.
        for (src, want) in [
            ("int ***p;", "pointer to pointer to pointer"),
            ("typedef int ***P;", "pointer to pointer to pointer"),
            ("int f(int a[2][n]);", "array size must be an integer literal"),
            ("int f(int a[2][0]);", "array size must be positive"),
        ] {
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
    fn typedef_shadowing() {
        // A typedef name hidden by a declaration of the same
        // name in an enclosing scope does not expand in
        // specifier position: block, parameter, `for`, and
        // file scope all hide it, as in C.
        for src in [
            "typedef int T; int main() { int T = 1; T x = 2; return 0; }",
            "typedef int T; int f(int T) { T x = 1; return 0; }",
            "typedef int T; int main() { for (int T = 0; T < 1; T++) { T x = 1; } return 0; }",
            "typedef int T; int T = 1; T x = 2;",
        ] {
            match parse(src.as_bytes()) {
                Err(e) => assert!(
                    e.msg.contains("is hidden by a declaration"),
                    "for {:?}: got error {:?}",
                    src,
                    e.msg
                ),
                Ok(_) => panic!("accepted shadowed typedef use: {:?}", src),
            }
        }
        // The alias is visible again when the shadowing scope
        // ends, and the shadowing name itself still works as
        // a value.
        assert_sexp(
            "typedef int T; int main() { { int T = 1; } T x = 2; return x; }",
            "(tu (typedef T int) (funcdef main (compound (compound (decl int (initdecl T (int 1)))) (decl int (initdecl x (int 2))) (return (ident x)))))",
        );
        assert_sexp(
            "typedef int T; int main() { int T = 1; return T; }",
            "(tu (typedef T int) (funcdef main (compound (decl int (initdecl T (int 1))) (return (ident T)))))",
        );
        assert_sexp(
            "typedef int T; int f(int T) { return T; } int main() { T x = 3; return x; }",
            "(tu (typedef T int) (funcdef f (params (param T)) (compound (return (ident T)))) (funcdef main (compound (decl int (initdecl x (int 3))) (return (ident x)))))",
        );
    }

    #[test]
    fn function_pointer_declarators_parse() {
        // One `(fnsig ...)` child per function-pointer
        // declarator, carrying the pointed function's
        // parameters, void-ness, and return pointer depth.
        assert_sexp(
            "int (*fp)(int);",
            "(tu (decl int (initdecl fp (fnsig (params (param))))))",
        );
        assert_sexp(
            "int (*fp)(int x, char *y);",
            "(tu (decl int (initdecl fp (fnsig (params (param x) (param y (pointer)))))))",
        );
        assert_sexp(
            "int *(*fp)(int);",
            "(tu (decl int (initdecl fp (fnsig (params (param)) (pointer)))))",
        );
        assert_sexp(
            "void (*cb)(void);",
            "(tu (decl void (initdecl cb (fnsig (void)))))",
        );
        assert_sexp(
            "int f(int (*cb)(int));",
            "(tu (funcdecl f (params (param cb (fnsig (params (param)))))))",
        );
        assert_sexp(
            "int (*fp)();",
            "(tu (decl int (initdecl fp (fnsig))))",
        );
        assert_sexp(
            "int f(int (*)(int));",
            "(tu (funcdecl f (params (param (fnsig (params (param)))))))",
        );
        // A return-pointer alias composes with the
        // parenthesized star as the return depth.
        assert_sexp(
            "typedef int *P; P (*fp)(int);",
            "(tu (typedef P int (pointer)) (decl int (initdecl fp (fnsig (params (param)) (pointer)))))",
        );
        // Redundant parentheses around a plain declarator
        // parse as the plain declarator.
        assert_sexp(
            "int (*p) = 0;",
            "(tu (decl int (initdecl p (pointer) (int 0))))",
        );
        assert_sexp(
            "int (x) = 1;",
            "(tu (decl int (initdecl x (int 1))))",
        );
        // Deferred forms are parse errors with their own
        // messages.
        for (src, want) in [
            ("int (**fp)(int);", "pointer to function pointer"),
            ("int (*fp)(int)[3];", "arrays of function pointers"),
            ("int (*fp)[3];", "pointer to array"),
            (
                "typedef char A[3]; A (*fp)(int);",
                "function return type cannot be an array",
            ),
            ("typedef int (*cb)(int);", "declarator name"),
            (
                "int f(int (*cb)(int (*g)(int)));",
                "nested function pointer parameters",
            ),
            (
                "int (fp)(int);",
                "parenthesized function declarators",
            ),
        ] {
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
    fn storage_classes_parse() {
        // Storage words are stripped from the specifier in dumps;
        // `static`/`extern` at file scope behave like plain
        // declarations, `register`/`auto` are ignored anywhere.
        assert_sexp(
            "static int x = 1; extern int y; int y = 2; int main() { register int r = 3; auto int a = 4; }",
            "(tu (decl int (initdecl x (int 1))) (decl int (initdecl y)) (decl int (initdecl y (int 2))) (funcdef main (compound (decl int (initdecl r (int 3))) (decl int (initdecl a (int 4))))))",
        );
        assert_sexp(
            "static int f() { return 1; }",
            "(tu (funcdef f (compound (return (int 1)))))",
        );
        // The storage words can still name variables.
        assert_sexp(
            "int static = 1; int register = 2;",
            "(tu (decl int (initdecl static (int 1))) (decl int (initdecl register (int 2))))",
        );
    }

    #[test]
    fn storage_class_errors() {
        let cases: &[(&str, &str)] = &[
            (
                "int main() { static int x = 1; return 0; }",
                "static locals are not supported",
            ),
            (
                "int main() { extern int x; return 0; }",
                "only allowed at file scope",
            ),
            (
                "int f(static int x) { return x; }",
                "not allowed in parameters",
            ),
            (
                "int main() { for (static int i = 0;;) { } return 0; }",
                "static locals are not supported",
            ),
            (
                "static extern int x = 1;",
                "multiple storage classes",
            ),
            (
                "typedef static int T;",
                "not allowed with typedef",
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
