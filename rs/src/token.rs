//! Tokenizer for the C subset.
//!
//! Mirrors `tests/differential/tokdump.cpp` token-for-token: same token
//! boundaries, same 1-based line/col tracking (including the `//` and `#`
//! "columns go stale until newline" quirk), same cooked string contents,
//! same error messages.
//!
//! One extension beyond the reference: object-like `#define` (and
//! `#undef`) are expanded at tokenize time. Function-like macros are
//! an error; every other `#` line is skipped, as before.

use std::collections::{HashMap, HashSet};

/// Token kinds. [`Kind::dump_name`] gives the differential-dump label.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Kw,
    Ident,
    Int,
    Char,
    String,
    Punct,
    End,
}

impl Kind {
    /// Differential-dump label (`kw`, `ident`, `int`, `char`, `string`,
    /// `punct`, `end`).
    pub fn dump_name(self) -> &'static str {
        match self {
            Kind::Kw => "kw",
            Kind::Ident => "ident",
            Kind::Int => "int",
            Kind::Char => "char",
            Kind::String => "string",
            Kind::Punct => "punct",
            Kind::End => "end",
        }
    }
}

/// A lexical token: kind, text bytes, 1-based start position.
///
/// Text is the raw source slice, except strings, which are cooked
/// (escapes processed, quotes stripped) — exactly like the C++ reference.
/// `raw` preserves the string's inner source text (escapes unprocessed,
/// quotes stripped) for `printf`/`scanf` fidelity; it is `None` for every
/// other token kind.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Tok {
    pub kind: Kind,
    pub text: Vec<u8>,
    pub raw: Option<Vec<u8>>,
    pub line: usize,
    pub col: usize,
}

/// Tokenizer failure: 1-based position plus the reference message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TokenError {
    pub line: usize,
    pub col: usize,
    pub msg: String,
}

impl std::fmt::Display for TokenError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}:{}: error: {}", self.line, self.col, self.msg)
    }
}

impl std::error::Error for TokenError {}

/// Tokenize `src` into the subset token stream, ending with [`Kind::End`].
pub fn tokenize(src: &[u8]) -> Result<Vec<Tok>, TokenError> {
    Cursor::new(src).run()
}

/// Escape token text for the `LINE:COL KIND TEXT` dump format.
pub fn escape_into(out: &mut Vec<u8>, text: &[u8]) {
    for &b in text {
        match b {
            b'\\' => out.extend_from_slice(b"\\\\"),
            b'\n' => out.extend_from_slice(b"\\n"),
            b'\t' => out.extend_from_slice(b"\\t"),
            b'\r' => out.extend_from_slice(b"\\r"),
            0 => out.extend_from_slice(b"\\0"),
            _ => out.push(b),
        }
    }
}

fn escape_val(c: u8) -> Option<u8> {
    match c {
        b'n' => Some(b'\n'),
        b't' => Some(b'\t'),
        b'r' => Some(b'\r'),
        b'0' => Some(0),
        b'\\' => Some(b'\\'),
        b'\'' => Some(b'\''),
        b'"' => Some(b'"'),
        b'a' => Some(7),
        b'b' => Some(8),
        _ => None,
    }
}

const KEYWORDS: [&[u8]; 17] = [
    b"int", b"char", b"short", b"void", b"if", b"else", b"switch", b"case",
    b"default", b"while", b"do", b"for", b"break", b"continue", b"return",
    b"sizeof", b"goto",
];

fn is_keyword(word: &[u8]) -> bool {
    KEYWORDS.iter().any(|k| *k == word)
}

fn is_two_char(a: u8, b: u8) -> bool {
    matches!(
        (a, b),
        (b'=', b'=')
            | (b'!', b'=')
            | (b'<', b'=')
            | (b'>', b'=')
            | (b'+', b'+')
            | (b'-', b'-')
            | (b'&', b'&')
            | (b'|', b'|')
            | (b'+', b'=')
            | (b'-', b'=')
            | (b'*', b'=')
            | (b'/', b'=')
            | (b'%', b'=')
            | (b'<', b'<')
            | (b'>', b'>')
            | (b'-', b'>')
            | (b'&', b'=')
            | (b'|', b'=')
            | (b'^', b'=')
    )
}

/// Three-char punctuators: the shift-assignment operators. Checked
/// before the two-char table so `<<=` lexes as one token, not `<<`
/// plus `=`.
fn is_three_char(a: u8, b: u8, c: u8) -> bool {
    matches!((a, b, c), (b'<', b'<', b'=') | (b'>', b'>', b'='))
}

fn is_one_char(c: u8) -> bool {
    matches!(
        c,
        b'(' | b')'
            | b'{'
            | b'}'
            | b'['
            | b']'
            | b';'
            | b','
            | b':'
            | b'?'
            | b'+'
            | b'-'
            | b'*'
            | b'/'
            | b'%'
            | b'<'
            | b'>'
            | b'!'
            | b'&'
            | b'|'
            | b'^'
            | b'~'
            | b'='
            | b'.'
    )
}

struct Cursor<'a> {
    src: &'a [u8],
    i: usize,
    line: usize,
    col: usize,
    /// Object-like macros defined so far (`#define`).
    defines: HashMap<Vec<u8>, Vec<Tok>>,
    /// Macros currently being expanded (self-reference guard).
    expanding: HashSet<Vec<u8>>,
}

impl<'a> Cursor<'a> {
    fn new(src: &'a [u8]) -> Self {
        Cursor {
            src,
            i: 0,
            line: 1,
            col: 1,
            defines: HashMap::new(),
            expanding: HashSet::new(),
        }
    }

    fn err(&self, line: usize, col: usize, msg: String) -> TokenError {
        TokenError { line, col, msg }
    }

    /// Expand identifier `name` (a defined macro) into `out`, with
    /// expanded tokens positioned at the use site (`line`, `col`).
    /// Chained macros expand recursively; a macro expanding to
    /// itself yields one plain identifier instead of looping.
    fn expand_into(&mut self, name: &[u8], line: usize, col: usize, out: &mut Vec<Tok>) {
        if !self.expanding.insert(name.to_vec()) {
            out.push(Tok {
                kind: Kind::Ident,
                text: name.to_vec(),
                raw: None,
                line,
                col,
            });
            return;
        }
        let repl = self.defines.get(name).cloned().unwrap_or_default();
        for mut t in repl {
            t.line = line;
            t.col = col;
            if t.kind == Kind::Ident && self.defines.contains_key(&t.text) {
                let inner = t.text.clone();
                self.expand_into(&inner, line, col, out);
            } else {
                out.push(t);
            }
        }
        self.expanding.remove(name);
    }

    /// Handle one `#` line (slice includes the `#`; `hash_col` is the
    /// column of the `#`). `#define NAME ...` registers an object-like
    /// macro (redefinition replaces it), `#undef NAME` removes one,
    /// and every other directive is ignored, as before. A `(` right
    /// after the macro name is a function-like macro: an error.
    fn handle_directive(
        &mut self,
        line: &[u8],
        line_no: usize,
        hash_col: usize,
    ) -> Result<(), TokenError> {
        let err_at = |col: usize, msg: &str| TokenError {
            line: line_no,
            col,
            msg: msg.to_string(),
        };
        let mut j = 1usize;
        while j < line.len() && (line[j] == b' ' || line[j] == b'\t') {
            j += 1;
        }
        let rest = &line[j..];
        let directive = |word: &[u8]| -> bool {
            rest.starts_with(word)
                && (rest.len() == word.len()
                    || rest[word.len()] == b' '
                    || rest[word.len()] == b'\t')
        };
        let read_name = |rest: &[u8], mut k: usize| -> Option<(Vec<u8>, usize)> {
            let start = k;
            if k < rest.len() && (rest[k].is_ascii_alphabetic() || rest[k] == b'_') {
                k += 1;
                while k < rest.len()
                    && (rest[k].is_ascii_alphanumeric() || rest[k] == b'_')
                {
                    k += 1;
                }
                Some((rest[start..k].to_vec(), k))
            } else {
                None
            }
        };
        if directive(b"define") {
            let mut k = "define".len();
            while k < rest.len() && (rest[k] == b' ' || rest[k] == b'\t') {
                k += 1;
            }
            let Some((name, used)) = read_name(rest, k) else {
                return Err(err_at(hash_col, "expected macro name in #define"));
            };
            // `used` is the end index of the name in `rest`, not a length.
            k = used;
            if k < rest.len() && rest[k] == b'(' {
                return Err(err_at(
                    hash_col + j + k,
                    "function-like macros are not supported",
                ));
            }
            while k < rest.len() && (rest[k] == b' ' || rest[k] == b'\t') {
                k += 1;
            }
            let repl_start = k;
            let mut repl = tokenize(&rest[repl_start..]).map_err(|e| TokenError {
                line: line_no,
                col: hash_col + j + repl_start + e.col - 1,
                msg: e.msg,
            })?;
            // `tokenize` always appends the End token; drop it.
            repl.pop();
            self.defines.insert(name, repl);
            return Ok(());
        }
        if directive(b"undef") {
            let mut k = "undef".len();
            while k < rest.len() && (rest[k] == b' ' || rest[k] == b'\t') {
                k += 1;
            }
            let Some((name, _)) = read_name(rest, k) else {
                return Err(err_at(hash_col, "expected macro name in #undef"));
            };
            self.defines.remove(&name);
            return Ok(());
        }
        Ok(())
    }

    fn run(mut self) -> Result<Vec<Tok>, TokenError> {
        let mut toks: Vec<Tok> = Vec::new();
        let n = self.src.len();
        while self.i < n {
            let c = self.src[self.i];
            if c == b' ' || c == b'\t' || c == b'\r' {
                self.i += 1;
                self.col += 1;
            } else if c == b'\n' {
                self.i += 1;
                self.line += 1;
                self.col = 1;
            } else if c == b'/' && self.i + 1 < n && self.src[self.i + 1] == b'/' {
                while self.i < n && self.src[self.i] != b'\n' {
                    self.i += 1;
                }
            } else if c == b'/' && self.i + 1 < n && self.src[self.i + 1] == b'*' {
                self.i += 2;
                self.col += 2;
                let mut closed = false;
                while self.i < n {
                    if self.src[self.i] == b'*'
                        && self.i + 1 < n
                        && self.src[self.i + 1] == b'/'
                    {
                        self.i += 2;
                        self.col += 2;
                        closed = true;
                        break;
                    }
                    if self.src[self.i] == b'\n' {
                        self.line += 1;
                        self.col = 1;
                    } else {
                        self.col += 1;
                    }
                    self.i += 1;
                }
                if !closed {
                    return Err(self.err(
                        self.line,
                        self.col,
                        "unterminated block comment".to_string(),
                    ));
                }
            } else if c == b'#' {
                let mut j = self.i;
                while j < n && self.src[j] != b'\n' {
                    j += 1;
                }
                // Copy the line: handling it needs `&mut self`.
                let line: Vec<u8> = self.src[self.i..j].to_vec();
                let (line_no, hash_col) = (self.line, self.col);
                self.i = j;
                self.handle_directive(&line, line_no, hash_col)?;
            } else if c.is_ascii_digit() {
                let j0 = self.i;
                // Radix prefixes: `0x`/`0X` hex, `0b`/`0B` binary
                // (the prefix only applies when a digit of that radix
                // follows; `0x` alone is `0` plus ident `x`).
                if self.src[self.i] == b'0'
                    && self.i + 2 < n
                    && (self.src[self.i + 1] == b'x' || self.src[self.i + 1] == b'X')
                    && self.src[self.i + 2].is_ascii_hexdigit()
                {
                    self.i += 2;
                    while self.i < n && self.src[self.i].is_ascii_hexdigit() {
                        self.i += 1;
                    }
                } else if self.src[self.i] == b'0'
                    && self.i + 2 < n
                    && (self.src[self.i + 1] == b'b' || self.src[self.i + 1] == b'B')
                    && (self.src[self.i + 2] == b'0' || self.src[self.i + 2] == b'1')
                {
                    self.i += 2;
                    while self.i < n
                        && (self.src[self.i] == b'0' || self.src[self.i] == b'1')
                    {
                        self.i += 1;
                    }
                } else {
                    while self.i < n && self.src[self.i].is_ascii_digit() {
                        self.i += 1;
                    }
                    // A leading `0` with more digits is octal; 8/9
                    // there are an error, as in g++.
                    if self.i - j0 > 1 && self.src[j0] == b'0' {
                        for &d in &self.src[j0..self.i] {
                            if d == b'8' || d == b'9' {
                                return Err(self.err(
                                    self.line,
                                    self.col,
                                    "invalid digit in octal constant".to_string(),
                                ));
                            }
                        }
                    }
                }
                // Optional integer suffixes (`u`/`U`/`l`/`L`, in any
                // combination); the value ignores them.
                while self.i < n
                    && matches!(self.src[self.i], b'u' | b'U' | b'l' | b'L')
                {
                    self.i += 1;
                }
                toks.push(Tok {
                    kind: Kind::Int,
                    text: self.src[j0..self.i].to_vec(),
                    raw: None,
                    line: self.line,
                    col: self.col,
                });
                self.col += self.i - j0;
            } else if c.is_ascii_alphabetic() || c == b'_' {
                let j0 = self.i;
                while self.i < n
                    && (self.src[self.i].is_ascii_alphanumeric() || self.src[self.i] == b'_')
                {
                    self.i += 1;
                }
                let word = self.src[j0..self.i].to_vec();
                let kind = if is_keyword(&word) {
                    Kind::Kw
                } else {
                    Kind::Ident
                };
                let (line, col) = (self.line, self.col);
                self.col += self.i - j0;
                if kind == Kind::Ident && self.defines.contains_key(&word) {
                    self.expand_into(&word, line, col, &mut toks);
                } else {
                    toks.push(Tok {
                        kind,
                        text: word,
                        raw: None,
                        line,
                        col,
                    });
                }
            } else if c == b'\'' {
                let mut j = self.i + 1;
                if j < n && self.src[j] == b'\\' {
                    if j + 1 >= n {
                        return Err(self.err(
                            self.line,
                            self.col,
                            "bad character literal".to_string(),
                        ));
                    }
                    let ec = self.col + (j - self.i);
                    match escape_val(self.src[j + 1]) {
                        Some(_) => {}
                        None => {
                            return Err(self.err(
                                self.line,
                                ec,
                                "bad escape in literal".to_string(),
                            ))
                        }
                    }
                    j += 2;
                } else if j < n && self.src[j] != b'\'' && self.src[j] != b'\n' {
                    j += 1;
                } else {
                    return Err(self.err(
                        self.line,
                        self.col,
                        "bad character literal".to_string(),
                    ));
                }
                if j >= n || self.src[j] != b'\'' {
                    return Err(self.err(
                        self.line,
                        self.col,
                        "unterminated character literal".to_string(),
                    ));
                }
                j += 1;
                toks.push(Tok {
                    kind: Kind::Char,
                    text: self.src[self.i..j].to_vec(),
                    raw: None,
                    line: self.line,
                    col: self.col,
                });
                self.col += j - self.i;
                self.i = j;
            } else if c == b'"' {
                let mut j = self.i + 1;
                let mut parts: Vec<u8> = Vec::new();
                loop {
                    if j >= n || self.src[j] == b'\n' {
                        return Err(self.err(
                            self.line,
                            self.col,
                            "unterminated string literal".to_string(),
                        ));
                    }
                    if self.src[j] == b'"' {
                        break;
                    }
                    if self.src[j] == b'\\' {
                        if j + 1 >= n {
                            return Err(self.err(
                                self.line,
                                self.col,
                                "bad escape in string literal".to_string(),
                            ));
                        }
                        let ec = self.col + (j - self.i);
                        match escape_val(self.src[j + 1]) {
                            Some(v) => parts.push(v),
                            None => {
                                return Err(self.err(
                                    self.line,
                                    ec,
                                    "bad escape in literal".to_string(),
                                ))
                            }
                        }
                        j += 2;
                    } else {
                        parts.push(self.src[j]);
                        j += 1;
                    }
                }
                j += 1;
                toks.push(Tok {
                    kind: Kind::String,
                    text: parts,
                    // Inner source text: quotes at i and j-1 are stripped.
                    raw: Some(self.src[self.i + 1..j - 1].to_vec()),
                    line: self.line,
                    col: self.col,
                });
                self.col += j - self.i;
                self.i = j;
            } else if self.i + 2 < n
                && is_three_char(self.src[self.i], self.src[self.i + 1], self.src[self.i + 2])
            {
                toks.push(Tok {
                    kind: Kind::Punct,
                    text: self.src[self.i..self.i + 3].to_vec(),
                    raw: None,
                    line: self.line,
                    col: self.col,
                });
                self.i += 3;
                self.col += 3;
            } else if self.i + 1 < n && is_two_char(self.src[self.i], self.src[self.i + 1]) {
                toks.push(Tok {
                    kind: Kind::Punct,
                    text: self.src[self.i..self.i + 2].to_vec(),
                    raw: None,
                    line: self.line,
                    col: self.col,
                });
                self.i += 2;
                self.col += 2;
            } else if is_one_char(c) {
                toks.push(Tok {
                    kind: Kind::Punct,
                    text: vec![c],
                    raw: None,
                    line: self.line,
                    col: self.col,
                });
                self.i += 1;
                self.col += 1;
            } else {
                return Err(self.err(
                    self.line,
                    self.col,
                    format!("unexpected character '{}'", c as char),
                ));
            }
        }
        toks.push(Tok {
            kind: Kind::End,
            text: Vec::new(),
            raw: None,
            line: self.line,
            col: self.col,
        });
        Ok(toks)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn toks(src: &[u8]) -> Vec<Tok> {
        tokenize(src).expect("tokenize failed")
    }

    fn kinds(src: &[u8]) -> Vec<Kind> {
        toks(src).iter().map(|t| t.kind).collect()
    }

    fn texts(src: &[u8]) -> Vec<String> {
        toks(src)
            .iter()
            .map(|t| String::from_utf8_lossy(&t.text).into_owned())
            .collect()
    }

    fn assert_kinds(src: &[u8], want: &[Kind]) {
        assert_eq!(kinds(src), want);
    }

    fn assert_texts(src: &[u8], want: &[&str]) {
        let got = texts(src);
        assert_eq!(got.len(), want.len(), "token count");
        for (g, w) in got.iter().zip(want.iter()) {
            assert_eq!(g, w);
        }
    }

    #[test]
    fn empty_yields_end_only() {
        let t = toks(b"");
        assert_eq!(t.len(), 1);
        assert_eq!(t[0].kind, Kind::End);
        assert_eq!((t[0].line, t[0].col), (1, 1));
    }

    #[test]
    fn declaration() {
        let t = toks(b"int x = 42;");
        assert_kinds(
            b"int x = 42;",
            &[
                Kind::Kw,
                Kind::Ident,
                Kind::Punct,
                Kind::Int,
                Kind::Punct,
                Kind::End,
            ],
        );
        assert_eq!(t[0].text, b"int");
        assert_eq!(t[1].text, b"x");
        assert_eq!(t[3].text, b"42");
        assert_eq!((t[3].line, t[3].col), (1, 9));
        assert_eq!((t[5].line, t[5].col), (1, 12));
    }

    #[test]
    fn define_object_like_expands_at_use_site() {
        let t = toks(b"#define N 5\nint x = N;");
        assert_texts(
            b"#define N 5\nint x = N;",
            &["int", "x", "=", "5", ";", ""],
        );
        // The expanded `5` carries the position of the use, line 2 col 9.
        assert_eq!((t[3].line, t[3].col), (2, 9));
        assert_eq!(t[3].kind, Kind::Int);
    }

    #[test]
    fn define_chained_redefined_and_empty() {
        assert_texts(
            b"#define A B\n#define B 7\nint x = A;",
            &["int", "x", "=", "7", ";", ""],
        );
        assert_texts(
            b"#define N 1\n#define N 2\nint x = N;",
            &["int", "x", "=", "2", ";", ""],
        );
        // An empty replacement expands to nothing.
        assert_texts(b"#define E\nint x = 1;", &["int", "x", "=", "1", ";", ""]);
        // A macro may stand for a keyword.
        assert_texts(
            b"#define T int\nT x = 1;",
            &["int", "x", "=", "1", ";", ""],
        );
        assert_eq!(toks(b"#define T int\nT x = 1;")[0].kind, Kind::Kw);
    }

    #[test]
    fn define_self_reference_and_undef() {
        // Self-reference yields one plain identifier, not a loop.
        assert_texts(
            b"#define A A\nint x = A;",
            &["int", "x", "=", "A", ";", ""],
        );
        // After #undef the name stays an identifier.
        let t = toks(b"#define N 5\n#undef N\nint x = N;");
        assert_texts(
            b"#define N 5\n#undef N\nint x = N;",
            &["int", "x", "=", "N", ";", ""],
        );
        assert_eq!(t[3].kind, Kind::Ident);
        // Uses before the definition do not expand.
        assert_texts(
            b"int x = N;\n#define N 5",
            &["int", "x", "=", "N", ";", ""],
        );
    }

    #[test]
    fn define_function_like_and_bad_name_error() {
        assert_eq!(
            tokenize(b"#define F(x) x"),
            Err(TokenError {
                line: 1,
                col: 10,
                msg: "function-like macros are not supported".to_string(),
            })
        );
        assert_eq!(
            tokenize(b"#define"),
            Err(TokenError {
                line: 1,
                col: 1,
                msg: "expected macro name in #define".to_string(),
            })
        );
        // Other directives are still skipped wholesale.
        assert_texts(b"#include <stdio.h>\nint x = 1;", &["int", "x", "=", "1", ";", ""]);
    }

    #[test]
    fn sizeof_is_keyword() {
        assert_kinds(b"sizeof x", &[Kind::Kw, Kind::Ident, Kind::End]);
        assert_kinds(b"sizeofx", &[Kind::Ident, Kind::End]);
    }

    #[test]
    fn keywords_vs_idents() {
        assert_kinds(
            b"if iffy while3 return do doing",
            &[
                Kind::Kw,
                Kind::Ident,
                Kind::Ident,
                Kind::Kw,
                Kind::Kw,
                Kind::Ident,
                Kind::End,
            ],
        );
        let t = toks(b"if iffy while3 return do doing");
        assert_eq!(t[1].text, b"iffy");
        assert_eq!(t[2].text, b"while3");
        assert_eq!(t[5].text, b"doing");
    }

    #[test]
    fn three_and_two_char_assign_operators() {
        assert_texts(
            b"a <<= 1; a >>= 2; a &= 3; a |= 4; a ^= 5;",
            &[
                "a", "<<=", "1", ";", "a", ">>=", "2", ";", "a", "&=", "3", ";", "a",
                "|=", "4", ";", "a", "^=", "5", ";", "",
            ],
        );
        // Spaced, `<<` and `=` stay separate tokens.
        assert_texts(b"a << = 1;", &["a", "<<", "=", "1", ";", ""]);
    }

    #[test]
    fn all_two_char_operators() {
        assert_texts(
            b"a==b!=c<=d>=e++f--g&&h||i+=j-=k*=l/=m%=n<<o>>p->q;",
            &[
                "a", "==", "b", "!=", "c", "<=", "d", ">=", "e", "++", "f", "--",
                "g", "&&", "h", "||", "i", "+=", "j", "-=", "k", "*=", "l",
                "/=", "m", "%=", "n", "<<", "o", ">>", "p", "->", "q", ";", "",
            ],
        );
    }

    #[test]
    fn int_literal_radixes_and_suffixes() {
        assert_texts(
            b"0x1F 0Xff 010 077 0b101 0B11 42U 7l 0 00 123",
            &[
                "0x1F", "0Xff", "010", "077", "0b101", "0B11", "42U", "7l", "0",
                "00", "123", "",
            ],
        );
        assert_kinds(
            b"0x1F 010 0b101 42U",
            &[Kind::Int, Kind::Int, Kind::Int, Kind::Int, Kind::End],
        );
        // `0x` with no hex digit is `0` plus an identifier, as before.
        assert_texts(b"0x", &["0", "x", ""]);
    }

    #[test]
    fn error_invalid_octal_digit() {
        assert_eq!(
            tokenize(b"09"),
            Err(TokenError {
                line: 1,
                col: 1,
                msg: "invalid digit in octal constant".to_string(),
            })
        );
    }

    #[test]
    fn strings_are_cooked() {
        let t = toks(br#""a\nb\t\"q\"\\""#);
        assert_eq!(t[0].kind, Kind::String);
        assert_eq!((t[0].line, t[0].col), (1, 1));
        assert_eq!(t[0].text, b"a\nb\t\"q\"\\");
    }

    #[test]
    fn strings_carry_raw_text() {
        let t = toks(br#""a\nb""#);
        assert_eq!(t[0].kind, Kind::String);
        assert_eq!(t[0].text, b"a\nb");
        assert_eq!(t[0].raw, Some(b"a\\nb".to_vec()));
        let t = toks(b"42");
        assert_eq!(t[0].raw, None);
    }

    #[test]
    fn chars_keep_raw_slice() {
        assert_texts(b"'x' '\\n'", &["'x'", "'\\n'", ""]);
        let t = toks(b"'x' '\\n'");
        assert_eq!(t[0].kind, Kind::Char);
        assert_eq!(t[1].kind, Kind::Char);
    }

    #[test]
    fn comments_and_hash_lines_skipped() {
        let src = b"int a; // trailing\n# define X\n/* multi\nline */ b=c;";
        assert_kinds(
            src,
            &[
                Kind::Kw,
                Kind::Ident,
                Kind::Punct,
                Kind::Ident,
                Kind::Punct,
                Kind::Ident,
                Kind::Punct,
                Kind::End,
            ],
        );
        let t = toks(src);
        assert_eq!((t[3].line, t[3].col), (4, 9));
        assert_eq!(t[3].text, b"b");
    }

    #[test]
    fn positions_across_lines() {
        let t = toks(b"a\nbb\n;");
        assert_eq!((t[0].line, t[0].col), (1, 1));
        assert_eq!((t[1].line, t[1].col), (2, 1));
        assert_eq!((t[2].line, t[2].col), (3, 1));
        assert_eq!((t[3].line, t[3].col), (3, 2));
    }

    #[test]
    fn no_floats_int_dot_int() {
        // Pins the reference quirk: `3.14` is int, dot, int.
        assert_texts(b"3.14", &["3", ".", "14", ""]);
        assert_kinds(b"3.14", &[Kind::Int, Kind::Punct, Kind::Int, Kind::End]);
    }

    #[test]
    fn escape_into_covers_controls() {
        let mut out = Vec::new();
        escape_into(&mut out, b"a\nb\\c\x00d\t\re");
        assert_eq!(out, b"a\\nb\\\\c\\0d\\t\\re");
    }

    #[test]
    fn error_unterminated_string() {
        assert_eq!(
            tokenize(b"\"abc"),
            Err(TokenError {
                line: 1,
                col: 1,
                msg: "unterminated string literal".to_string(),
            })
        );
    }

    #[test]
    fn error_bad_escape_points_at_backslash() {
        assert_eq!(
            tokenize(b"\"a\\qb\""),
            Err(TokenError {
                line: 1,
                col: 3,
                msg: "bad escape in literal".to_string(),
            })
        );
    }

    #[test]
    fn error_unexpected_character() {
        assert_eq!(
            tokenize(b"int $;"),
            Err(TokenError {
                line: 1,
                col: 5,
                msg: "unexpected character '$'".to_string(),
            })
        );
    }

    #[test]
    fn error_unterminated_block_comment() {
        assert_eq!(
            tokenize(b"/* x"),
            Err(TokenError {
                line: 1,
                col: 5,
                msg: "unterminated block comment".to_string(),
            })
        );
    }

    #[test]
    fn error_bad_character_literal() {
        assert_eq!(
            tokenize(b"''"),
            Err(TokenError {
                line: 1,
                col: 1,
                msg: "bad character literal".to_string(),
            })
        );
    }
}
