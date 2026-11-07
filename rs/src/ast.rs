//! Typed AST for the C subset, with S-expression serialization.
//!
//! [`dump_tu`] renders exactly the `tests/differential/SPEC.md` v1 format:
//! single-line S-expression, tokenizer-dump escaping, no positions.

use crate::token::escape_into;

#[derive(Debug)]
pub struct TranslationUnit(pub Vec<External>);

#[derive(Debug)]
pub enum External {
    FuncDef(FuncDef),
    FuncDecl(FuncDecl),
    Decl(Decl),
    Typedef(TypedefDecl),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FnParam {
    pub name: Vec<u8>,
    /// Pointer depth: `0` for a scalar, `1` for `*`, `2` for
    /// `**`. Depths above 2 are parse errors.
    pub ptr_depth: u8,
    /// Array dimensions (`name[N]`, `name[]`): one entry per
    /// bracket, `None` for an unsized (empty) bracket. Array
    /// parameters decay to pointers; the outermost size is
    /// discarded, the inner sizes are the stride.
    pub dims: Vec<Option<i32>>,
}

impl FnParam {
    /// Whether this parameter is an array parameter.
    pub fn is_array(&self) -> bool {
        !self.dims.is_empty()
    }
}

#[derive(Debug)]
pub struct FuncDef {
    pub name: Vec<u8>,
    /// `true` when the return type is exactly `void`; kept (unlike
    /// other return types) because a `void` call used as a value
    /// must yield zero rather than a returned value.
    pub is_void: bool,
    /// Pointer depth of the return type: `0` for a scalar, `1`
    /// for a `*` return declarator, `2` for `**`. Set by a
    /// `*` return declarator or by a pointer typedef as the
    /// return type.
    pub returns_ptr_depth: u8,
    pub params: Vec<FnParam>,
    pub body: Block,
}

/// A function prototype (forward declaration): a signature with
/// no body. Parameter names are optional; an unnamed parameter
/// keeps an empty name. Prototypes are file-scope only.
#[derive(Debug)]
pub struct FuncDecl {
    pub name: Vec<u8>,
    pub is_void: bool,
    pub returns_ptr_depth: u8,
    pub params: Vec<FnParam>,
}

/// A file-scope type alias (`typedef`), resolved at parse time:
/// uses expand to the aliased specifier plus declarator flags.
/// See `docs/semantics.md` for scope and redefinition rules.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TypedefDecl {
    pub name: Vec<u8>,
    /// Space-joined aliased specifier words (`int`,
    /// `unsigned int`).
    pub spec: Vec<u8>,
    /// Pointer depth of the alias (`0`, `1`, `2`).
    pub ptr_depth: u8,
    /// Array dimensions of the alias, one entry per bracket:
    /// a sized dimension holds the element count, evaluated
    /// at definition (array sizes must be integer literals,
    /// as for arrays), `None` is an unsized dimension.
    pub dims: Vec<Option<i32>>,
}

#[derive(Debug)]
pub struct Decl {
    pub spec: Vec<u8>,
    /// Declarators in order; empty for a bare specifier (`int;`).
    pub inits: Vec<InitDecl>,
}

#[derive(Debug)]
pub struct InitDecl {
    pub name: Vec<u8>,
    /// Pointer depth of the declarator (`0`, `1`, `2`).
    pub ptr_depth: u8,
    /// Array dimensions, one entry per bracket: `Some` holds
    /// the size expression for `name[N]`, `None` an unsized
    /// declarator (`name[]`); empty for scalars. Only the
    /// outermost dimension may be unsized, and only a string
    /// or list initializer may size it.
    pub dims: Vec<Option<Expr>>,
    pub init: Option<Expr>,
}

#[derive(Debug)]
pub struct Block(pub Vec<BlockItem>);

#[derive(Debug)]
pub enum BlockItem {
    Decl(Decl),
    Stmt(Stmt),
}

#[derive(Debug)]
pub enum Stmt {
    Compound(Block),
    Expr(Expr),
    Empty,
    If {
        cond: Expr,
        then: Box<Stmt>,
        els: Option<Box<Stmt>>,
    },
    Switch {
        expr: Expr,
        body: Box<Stmt>,
    },
    While {
        cond: Expr,
        body: Box<Stmt>,
    },
    DoWhile {
        body: Box<Stmt>,
        cond: Expr,
    },
    For {
        init: ForInit,
        cond: Option<Expr>,
        incr: Option<Expr>,
        body: Box<Stmt>,
    },
    Return(Option<Expr>),
    Break,
    Continue,
    Case {
        label: Expr,
        body: Box<Stmt>,
    },
    Default {
        body: Box<Stmt>,
    },
    GotoLabel {
        name: Vec<u8>,
        body: Box<Stmt>,
    },
}

#[derive(Debug)]
pub enum ForInit {
    Decl(ForDecl),
    Empty,
    Expr(Expr),
}

#[derive(Debug)]
pub struct ForDecl {
    pub spec: Vec<u8>,
    pub name: Option<Vec<u8>>,
    pub init: Option<Expr>,
}

#[derive(Debug)]
pub enum Expr {
    Comma(Vec<Expr>),
    Assign {
        op: Vec<u8>,
        lhs: Box<Expr>,
        rhs: Box<Expr>,
    },
    Ternary {
        cond: Box<Expr>,
        then: Box<Expr>,
        els: Box<Expr>,
    },
    BinOp {
        op: Vec<u8>,
        l: Box<Expr>,
        r: Box<Expr>,
    },
    Unary {
        op: Vec<u8>,
        e: Box<Expr>,
    },
    /// Cast to `spec` (`(int) x`); a no-op in the byte model.
    Cast {
        spec: Vec<u8>,
        e: Box<Expr>,
    },
    /// `sizeof e`; unevaluated, always 1.
    Sizeof(Box<Expr>),
    /// `sizeof (spec)`; always 1.
    SizeofType(Vec<u8>),
    Postfix {
        op: Vec<u8>,
        e: Box<Expr>,
    },
    Call {
        callee: Box<Expr>,
        args: Vec<Expr>,
    },
    /// Subscript `base[index]`; the base must name an array or a
    /// pointer, and multi-dimensional subscripts are rejected in
    /// code generation.
    Index {
        base: Box<Expr>,
        index: Box<Expr>,
    },
    Paren(Box<Expr>),
    /// Braced initializer list `{1, 2}`. Only a declarator
    /// initializer (or a `for` scalar declarator) may hold one;
    /// any other position is a compile error.
    InitList(Vec<Expr>),
    Int(Vec<u8>),
    Char(Vec<u8>),
    StringLit {
        /// Concatenated cooked contents (escapes processed).
        cooked: Vec<u8>,
        /// First literal's raw inner text (escapes unprocessed).
        raw: Vec<u8>,
    },
    Ident(Vec<u8>),
}

fn open(out: &mut Vec<u8>, kind: &str) {
    out.push(b'(');
    out.extend_from_slice(kind.as_bytes());
}

fn open_text(out: &mut Vec<u8>, kind: &str, text: &[u8]) {
    open(out, kind);
    out.push(b' ');
    escape_into(out, text);
}

fn close(out: &mut Vec<u8>) {
    out.push(b')');
}

/// Serialize `tu` in the SPEC.md v1 format (no trailing newline).
pub fn dump_tu(tu: &TranslationUnit, out: &mut Vec<u8>) {
    open(out, "tu");
    for e in &tu.0 {
        out.push(b' ');
        dump_external(e, out);
    }
    close(out);
}

/// Serialize a parameter list: `(params (param <name> ...) ...)`.
/// An unnamed prototype parameter dumps as `(param ...)` with no
/// name, so `(param)` alone is an unnamed scalar parameter.
fn dump_params(params: &[FnParam], out: &mut Vec<u8>) {
    open(out, "params");
    for p in params {
        out.push(b' ');
        if p.name.is_empty() {
            open(out, "param");
        } else {
            open_text(out, "param", &p.name);
        }
        for _ in 0..p.ptr_depth {
            out.push(b' ');
            open(out, "pointer");
            close(out);
        }
        for _ in &p.dims {
            out.push(b' ');
            open(out, "array");
            close(out);
        }
        close(out);
    }
    close(out);
}

fn dump_external(e: &External, out: &mut Vec<u8>) {
    match e {
        External::FuncDef(f) => {
            open_text(out, "funcdef", &f.name);
            if !f.params.is_empty() {
                out.push(b' ');
                dump_params(&f.params, out);
            }
            out.push(b' ');
            dump_block(&f.body, out);
            close(out);
        }
        External::FuncDecl(d) => {
            open_text(out, "funcdecl", &d.name);
            if !d.params.is_empty() {
                out.push(b' ');
                dump_params(&d.params, out);
            }
            close(out);
        }
        External::Decl(d) => dump_decl(d, out),
        External::Typedef(t) => {
            open_text(out, "typedef", &t.name);
            out.push(b' ');
            escape_into(out, &t.spec);
            for _ in 0..t.ptr_depth {
                out.push(b' ');
                open(out, "pointer");
                close(out);
            }
            for dim in &t.dims {
                out.push(b' ');
                match dim {
                    Some(len) => open_text(out, "int", len.to_string().as_bytes()),
                    None => open(out, "unsized"),
                }
                close(out);
            }
            close(out);
        }
    }
}

fn dump_decl(d: &Decl, out: &mut Vec<u8>) {
    open_text(out, "decl", &d.spec);
    for init in &d.inits {
        out.push(b' ');
        open_text(out, "initdecl", &init.name);
        for _ in 0..init.ptr_depth {
            out.push(b' ');
            open(out, "pointer");
            close(out);
        }
        for dim in &init.dims {
            out.push(b' ');
            match dim {
                Some(len) => dump_expr(len, out),
                None => {
                    open(out, "unsized");
                    close(out);
                }
            }
        }
        if let Some(e) = &init.init {
            out.push(b' ');
            dump_expr(e, out);
        }
        close(out);
    }
    close(out);
}

fn dump_block(b: &Block, out: &mut Vec<u8>) {
    open(out, "compound");
    for item in &b.0 {
        out.push(b' ');
        match item {
            BlockItem::Decl(d) => dump_decl(d, out),
            BlockItem::Stmt(s) => dump_stmt(s, out),
        }
    }
    close(out);
}

fn dump_stmt(s: &Stmt, out: &mut Vec<u8>) {
    match s {
        Stmt::Compound(b) => dump_block(b, out),
        Stmt::Expr(e) => {
            open(out, "exprstmt");
            out.push(b' ');
            dump_expr(e, out);
            close(out);
        }
        Stmt::Empty => {
            open(out, "empty");
            close(out);
        }
        Stmt::If { cond, then, els } => {
            open(out, if els.is_some() { "ifelse" } else { "if" });
            out.push(b' ');
            dump_expr(cond, out);
            out.push(b' ');
            dump_stmt(then, out);
            if let Some(e) = els {
                out.push(b' ');
                dump_stmt(e, out);
            }
            close(out);
        }
        Stmt::Switch { expr, body } => {
            open(out, "switch");
            out.push(b' ');
            dump_expr(expr, out);
            out.push(b' ');
            dump_stmt(body, out);
            close(out);
        }
        Stmt::While { cond, body } => {
            open(out, "while");
            out.push(b' ');
            dump_expr(cond, out);
            out.push(b' ');
            dump_stmt(body, out);
            close(out);
        }
        Stmt::DoWhile { body, cond } => {
            open(out, "dowhile");
            out.push(b' ');
            dump_stmt(body, out);
            out.push(b' ');
            dump_expr(cond, out);
            close(out);
        }
        Stmt::For {
            init,
            cond,
            incr,
            body,
        } => {
            open(out, "for");
            out.push(b' ');
            match init {
                ForInit::Decl(f) => dump_fordecl(f, out),
                ForInit::Empty => {
                    open(out, "empty-init");
                    close(out);
                }
                ForInit::Expr(e) => dump_expr(e, out),
            }
            if let Some(c) = cond {
                out.push(b' ');
                dump_expr(c, out);
            }
            if let Some(i) = incr {
                out.push(b' ');
                dump_expr(i, out);
            }
            out.push(b' ');
            dump_stmt(body, out);
            close(out);
        }
        Stmt::Return(e) => {
            open(out, "return");
            if let Some(x) = e {
                out.push(b' ');
                dump_expr(x, out);
            }
            close(out);
        }
        Stmt::Break => {
            open(out, "break");
            close(out);
        }
        Stmt::Continue => {
            open(out, "continue");
            close(out);
        }
        Stmt::Case { label, body } => {
            open(out, "case");
            out.push(b' ');
            dump_expr(label, out);
            out.push(b' ');
            dump_stmt(body, out);
            close(out);
        }
        Stmt::Default { body } => {
            open(out, "default");
            out.push(b' ');
            dump_stmt(body, out);
            close(out);
        }
        Stmt::GotoLabel { name, body } => {
            open_text(out, "goto-label", name);
            out.push(b' ');
            dump_stmt(body, out);
            close(out);
        }
    }
}

fn dump_fordecl(f: &ForDecl, out: &mut Vec<u8>) {
    // Text is "spec" or "spec name", byte-identical to the reference's
    // single-string text (spec/name never need escaping).
    open(out, "fordecl");
    out.push(b' ');
    escape_into(out, &f.spec);
    if let Some(name) = &f.name {
        out.push(b' ');
        escape_into(out, name);
    }
    if let Some(init) = &f.init {
        out.push(b' ');
        dump_expr(init, out);
    }
    close(out);
}

fn dump_expr(e: &Expr, out: &mut Vec<u8>) {
    match e {
        Expr::Comma(es) => {
            open(out, "comma");
            for x in es {
                out.push(b' ');
                dump_expr(x, out);
            }
            close(out);
        }
        Expr::Assign { op, lhs, rhs } => {
            open_text(out, "assign", op);
            out.push(b' ');
            dump_expr(lhs, out);
            out.push(b' ');
            dump_expr(rhs, out);
            close(out);
        }
        Expr::Ternary { cond, then, els } => {
            open(out, "ternary");
            out.push(b' ');
            dump_expr(cond, out);
            out.push(b' ');
            dump_expr(then, out);
            out.push(b' ');
            dump_expr(els, out);
            close(out);
        }
        Expr::BinOp { op, l, r } => {
            open_text(out, "binop", op);
            out.push(b' ');
            dump_expr(l, out);
            out.push(b' ');
            dump_expr(r, out);
            close(out);
        }
        Expr::Unary { op, e } => {
            open_text(out, "unary", op);
            out.push(b' ');
            dump_expr(e, out);
            close(out);
        }
        Expr::Cast { spec, e } => {
            open_text(out, "cast", spec);
            out.push(b' ');
            dump_expr(e, out);
            close(out);
        }
        Expr::Sizeof(e) => {
            open(out, "sizeof");
            out.push(b' ');
            dump_expr(e, out);
            close(out);
        }
        Expr::SizeofType(spec) => {
            open_text(out, "sizeof-type", spec);
            close(out);
        }
        Expr::Postfix { op, e } => {
            open_text(out, "postfix", op);
            out.push(b' ');
            dump_expr(e, out);
            close(out);
        }
        Expr::Call { callee, args } => {
            open(out, "call");
            out.push(b' ');
            dump_expr(callee, out);
            for a in args {
                out.push(b' ');
                dump_expr(a, out);
            }
            close(out);
        }
        Expr::Paren(e) => {
            open(out, "paren");
            out.push(b' ');
            dump_expr(e, out);
            close(out);
        }
        Expr::InitList(es) => {
            open(out, "initlist");
            for x in es {
                out.push(b' ');
                dump_expr(x, out);
            }
            close(out);
        }
        Expr::Int(t) => {
            open_text(out, "int", t);
            close(out);
        }
        Expr::Char(t) => {
            open_text(out, "char", t);
            close(out);
        }
        Expr::StringLit { cooked, .. } => {
            open_text(out, "string", cooked);
            close(out);
        }
        Expr::Ident(t) => {
            open_text(out, "ident", t);
            close(out);
        }
        Expr::Index { base, index } => {
            open(out, "index");
            out.push(b' ');
            dump_expr(base, out);
            out.push(b' ');
            dump_expr(index, out);
            close(out);
        }
    }
}
