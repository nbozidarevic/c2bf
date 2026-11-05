//! AST-to-Brainfuck compilation: statement/expression wiring over [`Emitter`].
//!
//! Mirrors the `BrainfuckCompiler` visitor methods: same emission order,
//! same allocation order, same error messages. Orders at multi-argument
//! call sites follow GCC right-to-left (last argument first), per the
//! order probe in `tests/differential/micro/`.
//!
//! Intentional strictness deltas vs the ANTLR frontend (all reject inputs
//! the reference accepts-with-garbage or crashes on; none occur in the
//! fixture corpus):
//! - Non-lvalue assignment targets are rejected at parse time (the C++
//!   silently compiles them into temporaries).
//! - String literals in value position are rejected (the C++ crashes with
//!   `bad_cast` when the value is used).
//! - Non-string `printf`/`scanf` formats are rejected with a message (the
//!   C++ crashes with `bad_cast`).
//! - Too few format arguments are rejected (the C++ reads out of bounds).

use crate::ast::{Block, BlockItem, Decl, Expr, External, FnParam, ForDecl, ForInit, FuncDecl, FuncDef, InitDecl, Stmt};
use crate::codegen::{CompileError, Emitter, ExpandedFunction, FuncSig};
use std::collections::{HashMap, HashSet};
use std::rc::Rc;
use crate::parser;

fn err(msg: impl Into<String>) -> CompileError {
    CompileError { msg: msg.into() }
}

/// Value of an integer literal token: decimal, octal (`0` prefix),
/// hex (`0x`/`0X`), or binary (`0b`/`0B`), with `u`/`l` suffixes
/// stripped. Saturates like the old `strtol` clamp; the caller
/// narrows to a byte (`as u8`), so oversized literals wrap.
fn int_literal_value(text: &[u8]) -> i32 {
    let mut s = text;
    while matches!(s.last(), Some(b'u' | b'U' | b'l' | b'L')) {
        s = &s[..s.len() - 1];
    }
    // A leading `-` only ever reaches here from parser-substituted
    // enumerator values (ordinary `-1` is a unary minus node); the
    // magnitude clamps to 2^31 so `-2147483648` is exact.
    let (negative, s) = if s.starts_with(b"-") {
        (true, &s[1..])
    } else {
        (false, s)
    };
    let (radix, digits): (i64, &[u8]) = if s.starts_with(b"0x") || s.starts_with(b"0X")
    {
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
    if negative {
        (-v.clamp(0, -(i32::MIN as i64))) as i32
    } else {
        v.clamp(i32::MIN as i64, i32::MAX as i64) as i32
    }
}

/// What a subscript base names: an array or a pointer variable.
enum SubBase {
    Array(i32, usize),
    Pointer(i32),
}

/// A store destination for an assignment target.
enum StoreTarget {
    /// Static cell: write directly.
    Cell(i32),
    /// Array range: `counter` holds the element offset (the index
    /// value itself, not an address), searching `cells` in order.
    Range { counter: i32, cells: Vec<i32> },
    /// Runtime address, searching every byte address (dereference
    /// or pointer subscript).
    Any { addr: i32 },
}

/// A `scanf` destination: a static cell, a runtime address to
/// store through, or — for `scanf %s` — a static array range
/// with a known bound, so over-long input truncates instead of
/// writing past the array.
#[derive(Clone, Copy)]
enum ScanfTarget {
    Cell(i32),
    Through(i32),
    Bounded { base: i32, len: usize },
}

/// Scoped renaming: rebuilds one function definition with every
/// declared name replaced by a fresh name, so that a declaration
/// shadowing an outer one gets its own cells and each expansion of
/// the function starts from a clean set of names.
///
/// Names are fresh strings that cannot collide with user code: they
/// start with the byte `0x01`, followed by the function name, `#`,
/// a counter, and the original name. The renaming is purely lexical
/// — it follows the parser's block structure, so a declaration is
/// visible from its own block to the end of that block, including
/// nested blocks, and goes out of scope when the block closes.
/// Names that are never declared inside the function (globals,
/// library calls) pass through unchanged.
///
/// The parameter frame is the outermost scope: parameters are bound
/// before the body is walked, so a declaration nested inside the
/// body shadows a parameter of the same name within its own block.
/// A declaration at the top level of the body is in the same scope
/// as the parameters and may not reuse one of their names; that
/// check lives in [`rename_function`].
struct Renamer<'a> {
    fn_name: &'a [u8],
    counter: usize,
    scopes: Vec<HashMap<Vec<u8>, Vec<u8>>>,
    /// Fresh names bound as arrays (by declarator, not by scope):
    /// redeclaration of such a name stays the emitter's business.
    array_names: HashSet<Vec<u8>>,
}

impl<'a> Renamer<'a> {
    fn fresh(&mut self, name: &[u8]) -> Vec<u8> {
        self.counter += 1;
        let mut fresh = vec![0x01];
        fresh.extend_from_slice(self.fn_name);
        fresh.push(b'#');
        fresh.extend_from_slice(self.counter.to_string().as_bytes());
        fresh.extend_from_slice(b"::");
        fresh.extend_from_slice(name);
        fresh
    }

    fn resolve(&self, name: &[u8]) -> Option<Vec<u8>> {
        self.scopes
            .iter()
            .rev()
            .find_map(|scope| scope.get(name).cloned())
    }

    fn rename_block(&mut self, block: &Block) -> Result<Block, CompileError> {
        self.scopes.push(HashMap::new());
        let mut items = Vec::with_capacity(block.0.len());
        for item in &block.0 {
            items.push(self.rename_item(item)?);
        }
        self.scopes.pop();
        Ok(Block(items))
    }

    fn rename_item(&mut self, item: &BlockItem) -> Result<BlockItem, CompileError> {
        match item {
            BlockItem::Decl(d) => Ok(BlockItem::Decl(self.rename_decl(d)?)),
            BlockItem::Stmt(s) => Ok(BlockItem::Stmt(self.rename_stmt(s)?)),
        }
    }

    fn rename_decl(&mut self, decl: &Decl) -> Result<Decl, CompileError> {
        let mut inits = Vec::with_capacity(decl.inits.len());
        for init in &decl.inits {
            // Each declarator is fresh-named and bound in the current
            // scope *before* its initializer is walked, matching the
            // parser's order: a later declarator in the same list
            // sees an earlier one. A name bound only in an outer
            // scope gets a fresh name (it is shadowed, not
            // redeclared). A name already bound in the *same* scope
            // is a redefinition and is rejected, unless an array is
            // involved on either side: array redeclarations reuse
            // the fresh name so the emitter's diagnostics still
            // decide (same-size re-clear, size mismatch, and
            // variable/array confusion).
            let new_is_array = init.array_len.is_some() || init.array_unsized;
            let existing = self
                .scopes
                .last()
                .and_then(|scope| scope.get(&init.name).cloned());
            let fresh = match existing {
                Some(existing) => {
                    if !new_is_array && !self.array_names.contains(&existing) {
                        return Err(err(format!(
                            "redefinition of '{}'",
                            String::from_utf8_lossy(&init.name)
                        )));
                    }
                    existing
                }
                None => {
                    let fresh = self.fresh(&init.name);
                    if new_is_array {
                        self.array_names.insert(fresh.clone());
                    }
                    self.scopes
                        .last_mut()
                        .expect("rename_decl runs inside a scope")
                        .insert(init.name.clone(), fresh.clone());
                    fresh
                }
            };
            inits.push(InitDecl {
                name: fresh,
                pointer: init.pointer,
                array_len: init
                    .array_len
                    .as_ref()
                    .map(|e| self.rename_expr(e))
                    .transpose()?,
                array_unsized: init.array_unsized,
                init: init.init.as_ref().map(|e| self.rename_expr(e)).transpose()?,
            });
        }
        Ok(Decl {
            spec: decl.spec.clone(),
            inits,
        })
    }

    fn rename_stmt(&mut self, stmt: &Stmt) -> Result<Stmt, CompileError> {
        match stmt {
            Stmt::Compound(block) => Ok(Stmt::Compound(self.rename_block(block)?)),
            Stmt::Expr(e) => Ok(Stmt::Expr(self.rename_expr(e)?)),
            Stmt::Empty => Ok(Stmt::Empty),
            Stmt::If { cond, then, els } => Ok(Stmt::If {
                cond: self.rename_expr(cond)?,
                then: Box::new(self.rename_stmt(then)?),
                els: els
                    .as_ref()
                    .map(|s| self.rename_stmt(s))
                    .transpose()?
                    .map(Box::new),
            }),
            Stmt::Switch { expr, body } => Ok(Stmt::Switch {
                expr: self.rename_expr(expr)?,
                body: Box::new(self.rename_stmt(body)?),
            }),
            Stmt::While { cond, body } => Ok(Stmt::While {
                cond: self.rename_expr(cond)?,
                body: Box::new(self.rename_stmt(body)?),
            }),
            Stmt::DoWhile { body, cond } => Ok(Stmt::DoWhile {
                body: Box::new(self.rename_stmt(body)?),
                cond: self.rename_expr(cond)?,
            }),
            Stmt::For {
                init,
                cond,
                incr,
                body,
            } => {
                // The `for` initializer's scope covers the condition,
                // the increment, and the body.
                self.scopes.push(HashMap::new());
                let init = match init {
                    ForInit::Decl(d) => {
                        // Bind the declared name before walking its
                        // initializer, as for block declarations;
                        // a bare specifier has no name to bind.
                        let name = match &d.name {
                            Some(n) => {
                                let fresh = self.fresh(n);
                                self.scopes
                                    .last_mut()
                                    .expect("for scope is open")
                                    .insert(n.clone(), fresh.clone());
                                Some(fresh)
                            }
                            None => None,
                        };
                        ForInit::Decl(ForDecl {
                            spec: d.spec.clone(),
                            name,
                            init: d
                                .init
                                .as_ref()
                                .map(|e| self.rename_expr(e))
                                .transpose()?,
                        })
                    }
                    ForInit::Empty => ForInit::Empty,
                    ForInit::Expr(e) => ForInit::Expr(self.rename_expr(e)?),
                };
                let cond = cond.as_ref().map(|e| self.rename_expr(e)).transpose()?;
                let incr = incr.as_ref().map(|e| self.rename_expr(e)).transpose()?;
                let body = Box::new(self.rename_stmt(body)?);
                self.scopes.pop();
                Ok(Stmt::For {
                    init,
                    cond,
                    incr,
                    body,
                })
            }
            Stmt::Return(e) => Ok(Stmt::Return(
                e.as_ref().map(|e| self.rename_expr(e)).transpose()?,
            )),
            Stmt::Break => Ok(Stmt::Break),
            Stmt::Continue => Ok(Stmt::Continue),
            Stmt::Case { label, body } => Ok(Stmt::Case {
                label: self.rename_expr(label)?,
                body: Box::new(self.rename_stmt(body)?),
            }),
            Stmt::Default { body } => Ok(Stmt::Default {
                body: Box::new(self.rename_stmt(body)?),
            }),
            Stmt::GotoLabel { name, body } => Ok(Stmt::GotoLabel {
                name: name.clone(),
                body: Box::new(self.rename_stmt(body)?),
            }),
        }
    }

    fn rename_expr(&mut self, expr: &Expr) -> Result<Expr, CompileError> {
        match expr {
            Expr::Comma(es) => {
                let mut out = Vec::with_capacity(es.len());
                for e in es {
                    out.push(self.rename_expr(e)?);
                }
                Ok(Expr::Comma(out))
            }
            Expr::Assign { op, lhs, rhs } => Ok(Expr::Assign {
                op: op.clone(),
                lhs: Box::new(self.rename_expr(lhs)?),
                rhs: Box::new(self.rename_expr(rhs)?),
            }),
            Expr::Ternary { cond, then, els } => Ok(Expr::Ternary {
                cond: Box::new(self.rename_expr(cond)?),
                then: Box::new(self.rename_expr(then)?),
                els: Box::new(self.rename_expr(els)?),
            }),
            Expr::BinOp { op, l, r } => Ok(Expr::BinOp {
                op: op.clone(),
                l: Box::new(self.rename_expr(l)?),
                r: Box::new(self.rename_expr(r)?),
            }),
            Expr::Unary { op, e } => Ok(Expr::Unary {
                op: op.clone(),
                e: Box::new(self.rename_expr(e)?),
            }),
            Expr::Cast { spec, e } => Ok(Expr::Cast {
                spec: spec.clone(),
                e: Box::new(self.rename_expr(e)?),
            }),
            Expr::Sizeof(e) => Ok(Expr::Sizeof(Box::new(self.rename_expr(e)?))),
            Expr::SizeofType(spec) => Ok(Expr::SizeofType(spec.clone())),
            Expr::Postfix { op, e } => Ok(Expr::Postfix {
                op: op.clone(),
                e: Box::new(self.rename_expr(e)?),
            }),
            Expr::Call { callee, args } => {
                let mut out = Vec::with_capacity(args.len());
                for arg in args {
                    out.push(self.rename_expr(arg)?);
                }
                Ok(Expr::Call {
                    callee: Box::new(self.rename_expr(callee)?),
                    args: out,
                })
            }
            Expr::Index { base, index } => Ok(Expr::Index {
                base: Box::new(self.rename_expr(base)?),
                index: Box::new(self.rename_expr(index)?),
            }),
            Expr::Paren(e) => Ok(Expr::Paren(Box::new(self.rename_expr(e)?))),
            Expr::Int(s) => Ok(Expr::Int(s.clone())),
            Expr::Char(s) => Ok(Expr::Char(s.clone())),
            Expr::StringLit { cooked, raw } => Ok(Expr::StringLit {
                cooked: cooked.clone(),
                raw: raw.clone(),
            }),
            Expr::Ident(name) => Ok(Expr::Ident(
                self.resolve(name).unwrap_or_else(|| name.clone()),
            )),
        }
    }
}

/// Scope-rename one function definition: bind fresh names for its
/// parameters, then rename its body. A duplicate parameter name is a
/// compile error, as is a declaration at the top level of the body
/// that reuses a parameter's name: parameters share the body's
/// outermost scope, so both compilers reject it (g++: "shadows a
/// parameter"; gcc: "redeclared as different kind of symbol").
/// Shadowing a parameter inside a nested block or a `for`
/// initializer stays legal and keeps its own cells.
/// Check a prototype's signature: duplicate parameter names are
/// an error (unnamed parameters carry no name and cannot collide),
/// and it must agree with any definition or earlier prototype of
/// the same name. Parameter names may differ between a prototype
/// and its definition; only the signature is compared.
fn check_prototype(
    em: &Emitter,
    d: &FuncDecl,
) -> Result<FuncSig, CompileError> {
    let mut seen: Vec<&[u8]> = Vec::with_capacity(d.params.len());
    for p in &d.params {
        if p.name.is_empty() {
            continue;
        }
        if seen.contains(&p.name.as_slice()) {
            return Err(err(format!(
                "duplicate parameter name '{}'",
                String::from_utf8_lossy(&p.name)
            )));
        }
        seen.push(&p.name);
    }
    let sig = FuncSig {
        is_void: d.is_void,
        returns_pointer: d.returns_pointer,
        params: d.params.clone(),
    };
    if let Some(def) = em.functions.get(&d.name) {
        let def_sig = FuncSig {
            is_void: def.is_void,
            returns_pointer: def.returns_pointer,
            params: def.params.clone(),
        };
        if !sig.matches(&def_sig) {
            return Err(err(format!(
                "conflicting prototype for function '{}'",
                String::from_utf8_lossy(&d.name)
            )));
        }
    }
    if let Some(old) = em.prototypes.get(&d.name) {
        if !sig.matches(old) {
            return Err(err(format!(
                "conflicting prototype for function '{}'",
                String::from_utf8_lossy(&d.name)
            )));
        }
    }
    Ok(sig)
}

fn rename_function(f: &FuncDef) -> Result<ExpandedFunction, CompileError> {
    // Snapshot the parameter names before the body is walked: a
    // later top-level declaration may reuse an earlier one's name
    // (same-scope redeclaration), but never a parameter's.
    let param_names: Vec<Vec<u8>> = f.params.iter().map(|p| p.name.clone()).collect();
    for item in &f.body.0 {
        if let BlockItem::Decl(d) = item {
            for init in &d.inits {
                if param_names.iter().any(|n| n == &init.name) {
                    return Err(err(format!(
                        "redefinition of parameter '{}'",
                        String::from_utf8_lossy(&init.name)
                    )));
                }
            }
        }
    }
    let mut renamer = Renamer {
        fn_name: &f.name,
        counter: 0,
        scopes: vec![HashMap::new()],
        array_names: HashSet::new(),
    };
    let mut params = Vec::with_capacity(f.params.len());
    for p in &f.params {
        let fresh = renamer.fresh(&p.name);
        if renamer.scopes[0]
            .insert(p.name.clone(), fresh.clone())
            .is_some()
        {
            return Err(err(format!(
                "duplicate parameter name '{}'",
                String::from_utf8_lossy(&p.name)
            )));
        }
        params.push(FnParam {
            name: fresh,
            pointer: p.pointer,
            array: p.array,
        });
    }
    let body = renamer.rename_block(&f.body)?;
    Ok(ExpandedFunction {
        is_void: f.is_void,
        returns_pointer: f.returns_pointer,
        params,
        body,
    })
}

/// A compiled program: Brainfuck code plus the number of tape cells
/// the compiler allocated for it. Allocation never reuses cells, so
/// `cells` is the static tape footprint of the program.
pub struct CompiledProgram {
    /// Brainfuck code plus trailing newline, as from [`compile`].
    pub code: Vec<u8>,
    /// Tape cells allocated during emission.
    pub cells: usize,
}

/// Parse and compile `src` to Brainfuck plus trailing newline, and
/// report the allocated cell count.
///
/// Mirrors `BrainfuckCompiler::run()`: externals compile in source order
/// and a final newline is appended. Parse failures surface as
/// `CompileError` carrying the parse diagnostic.
pub fn compile_program(src: &[u8]) -> Result<CompiledProgram, CompileError> {
    let tu = parser::parse(src).map_err(|e| err(e.to_string()))?;
    let mut em = Emitter::new();
    // Scope-rename and register every function definition first, so
    // calls may refer to functions defined later in the file and
    // duplicate definitions are rejected before any code is emitted.
    // Prototypes register in the same pass and are checked against
    // definitions and earlier prototypes; a prototype alone emits
    // nothing.
    // Non-`main` definitions are never emitted directly; they expand
    // inline at their call sites. File-scope definitions are
    // checked in the same pass: a duplicate initialized definition
    // is a redefinition (both compilers reject it), unless an array
    // is involved, in which case the emitter decides during the
    // second pass. A bare declarator without an initializer is not
    // a definition here — it is rejected as unsupported during
    // emission — so it is neither checked nor recorded, which is
    // also why the tentative-definition question does not arise:
    // every duplicate this pass can express is initialized, and
    // both compilers reject those.
    let mut globals: HashMap<Vec<u8>, bool> = HashMap::new();
    for item in &tu.0 {
        match item {
            External::FuncDef(f) => {
                let expanded = rename_function(f)?;
                let def_sig = FuncSig {
                    is_void: expanded.is_void,
                    returns_pointer: expanded.returns_pointer,
                    params: expanded.params.clone(),
                };
                if let Some(proto) = em.prototypes.get(&f.name) {
                    if !def_sig.matches(proto) {
                        return Err(err(format!(
                            "conflicting prototype for function '{}'",
                            String::from_utf8_lossy(&f.name)
                        )));
                    }
                }
                if em
                    .functions
                    .insert(f.name.clone(), Rc::new(expanded))
                    .is_some()
                {
                    return Err(err("duplicate function definition"));
                }
            }
            External::FuncDecl(d) => {
                let sig = check_prototype(&em, d)?;
                em.prototypes.insert(d.name.clone(), Rc::new(sig));
            }
            // Typedefs are resolved at parse time; a typedef item
            // emits nothing.
            External::Typedef(_) => {}
            External::Decl(d) => {
                for init in &d.inits {
                    let is_array = init.array_len.is_some() || init.array_unsized;
                    if init.init.is_none() && !is_array {
                        continue;
                    }
                    if let Some(&old_is_array) = globals.get(&init.name) {
                        if !is_array && !old_is_array {
                            return Err(err(format!(
                                "redefinition of '{}'",
                                String::from_utf8_lossy(&init.name)
                            )));
                        }
                    }
                    globals.insert(init.name.clone(), is_array);
                }
            }
        }
    }
    for item in &tu.0 {
        match item {
            External::FuncDef(f) => {
                if f.name.as_slice() == b"main" {
                    let main_def = Rc::clone(
                        em.functions
                            .get(&f.name)
                            .expect("main is registered above"),
                    );
                    em.compile_block(&main_def.body)?;
                }
            }
            // Prototypes and typedefs emit nothing; prototypes
            // only register a signature, checked in the first pass
            // above.
            External::FuncDecl(_) => {}
            External::Typedef(_) => {}
            External::Decl(d) => {
                em.compile_decl(d)?;
            }
        }
    }
    let cells = em.allocated_cells();
    let mut code = em.output().to_vec();
    code.push(b'\n');
    Ok(CompiledProgram { code, cells })
}

/// Parse and compile `src` to Brainfuck plus trailing newline.
///
/// See [`compile_program`] for the allocated cell count.
pub fn compile(src: &[u8]) -> Result<Vec<u8>, CompileError> {
    compile_program(src).map(|program| program.code)
}

struct SwitchCase<'a> {
    is_default: bool,
    label_cell: i32,
    body: Vec<SwitchBodyItem<'a>>,
}

enum SwitchBodyItem<'a> {
    Decl(&'a Decl),
    Stmt(&'a Stmt),
}

fn push_switch_body<'a>(
    item: SwitchBodyItem<'a>,
    cases: &mut Vec<SwitchCase<'a>>,
) -> Result<(), CompileError> {
    match cases.last_mut() {
        None => Err(err("Unsupported statements before the first case label")),
        Some(c) => {
            c.body.push(item);
            Ok(())
        }
    }
}

impl Emitter {
    /// Compile a declaration: declarators in order, cell first, then
    /// the value each. A declarator without an initializer is still an
    /// error, as before (`int x;` pins it) — now per declarator.
    fn compile_decl(&mut self, d: &Decl) -> Result<(), CompileError> {
        // `int;` — no declarators, the base visitor emits nothing.
        for init in &d.inits {
            if init.array_len.is_some() || init.array_unsized {
                self.compile_array_decl(init)?;
                continue;
            }
            if self.memory.is_array(&init.name) {
                return Err(err("cannot redeclare an array as a variable"));
            }
            let e = match &init.init {
                None => return Err(err("Unsupported init declarator")),
                Some(e) => e,
            };
            let dest = self.memory.var_cell(&init.name);
            if init.pointer {
                self.memory.mark_pointer(&init.name);
            }
            let src = self.compile_expr(e)?;
            self.copy_value(src, dest);
        }
        Ok(())
    }

    /// Compile an array declaration: reserve consecutive cells and
    /// clear them, so elements start at zero (unlike C's indeterminate
    /// values; the tape starts zeroed, but a reclaimed index could
    /// otherwise carry a stale temporary). Arrays take no
    /// initializer; the size must be a positive integer literal.
    /// Redeclaring the same array with the same size re-clears it.
    fn compile_array_decl(&mut self, init: &InitDecl) -> Result<(), CompileError> {
        if self.memory.has_var(&init.name) {
            return Err(err("cannot redeclare a variable as an array"));
        }
        // A string initializer fills the elements (plus a NUL when
        // it fits); any other initializer is rejected, as before.
        let string_init = match &init.init {
            Some(Expr::StringLit { cooked, .. }) => Some(cooked.clone()),
            Some(_) if init.array_unsized => {
                return Err(err("unsized array requires a string initializer"));
            }
            Some(_) => return Err(err("array initializers are not supported")),
            None => None,
        };
        let len = if init.array_unsized {
            match &string_init {
                Some(cooked) => cooked.len() + 1,
                None => {
                    return Err(err("unsized array requires a string initializer"));
                }
            }
        } else {
            let len_expr = init.array_len.as_ref().expect("array declarator");
            let len = match len_expr {
                Expr::Int(bytes) => int_literal_value(bytes),
                _ => return Err(err("array size must be an integer literal")),
            };
            if len <= 0 {
                return Err(err("array size must be positive"));
            }
            let len = len as usize;
            if let Some(cooked) = &string_init {
                if cooked.len() > len {
                    return Err(err("string initializer is too long"));
                }
            }
            len
        };
        // Fill the elements: string bytes plus zero fill (a NUL
        // terminator when it fits), or plain clearing. Setting every
        // element covers clearing, so the string path needs no
        // separate pass.
        let fill = |em: &mut Self, base: i32| {
            for cell in base..base + len as i32 {
                let offset = (cell - base) as usize;
                let value = string_init
                    .as_ref()
                    .and_then(|cooked| cooked.get(offset).copied())
                    .unwrap_or(0);
                em.set_value(cell, value);
            }
        };
        if let Some((base, old_len)) = self.memory.array_info(&init.name) {
            if old_len != len {
                return Err(err("array size mismatch on redeclaration"));
            }
            fill(self, base);
            return Ok(());
        }
        let base = self.memory.alloc_array(&init.name, len);
        fill(self, base);
        Ok(())
    }

    /// Base cell and length of the array named by a subscript base
    /// (parentheses unwrap; anything else is rejected).
    fn array_base(&self, base: &Expr) -> Result<(i32, usize), CompileError> {
        match base {
            Expr::Ident(name) => match self.memory.array_info(name) {
                Some(info) => Ok(info),
                None => {
                    if self.memory.has_var(name) || self.memory.is_pointer(name) {
                        Err(err("subscript of non-array"))
                    } else {
                        Err(err("subscript of undeclared array"))
                    }
                }
            },
            Expr::Paren(inner) => self.array_base(inner),
            Expr::Index { .. } => Err(err("multi-dimensional arrays are not supported")),
            _ => Err(err("unsupported subscript base")),
        }
    }

    /// Constant subscript index, if the index expression is a compile
    /// time constant (literal, character, parenthesized, or signed
    /// constant). Anything else needs the variable-index support
    /// that follows this stage.
    fn const_index(e: &Expr) -> Option<i32> {
        match e {
            Expr::Int(bytes) => Some(int_literal_value(bytes)),
            Expr::Char(bytes) => Some(bytes.get(1).copied().unwrap_or(0) as i32),
            Expr::Paren(inner) => Self::const_index(inner),
            Expr::Unary { op, e } if op.as_slice() == b"-" => {
                Self::const_index(e).map(|v| v.wrapping_neg())
            }
            Expr::Unary { op, e } if op.as_slice() == b"+" => Self::const_index(e),
            _ => None,
        }
    }

    /// Element count of the array named by a `sizeof` operand, if
    /// it names one (parentheses unwrap). Element subscripts size
    /// 1, as does everything else.
    fn array_size_of(&self, e: &Expr) -> Option<usize> {
        match e {
            Expr::Ident(name) => self.memory.array_info(name).map(|(_, len)| len),
            Expr::Paren(inner) => self.array_size_of(inner),
            // A string literal sizes as its bytes plus the NUL.
            Expr::StringLit { cooked, .. } => Some(cooked.len() + 1),
            _ => None,
        }
    }

    /// Cell of a constant-index element; variable indices go
    /// through dispatch instead of this path.
    fn static_index_cell(&self, base: &Expr, index: &Expr) -> Result<i32, CompileError> {
        let (cell_base, len) = self.array_base(base)?;
        let k = match Self::const_index(index) {
            Some(k) => k,
            None => return Err(err("only constant array indices are supported")),
        };
        if k < 0 || k as usize >= len {
            return Err(err("array index out of bounds"));
        }
        Ok(cell_base + k)
    }

    /// A cell holding a tape index as a byte value, for a statically
    /// known index. Indexes of 256 and above cannot be held in a
    /// byte; materializing such an address is an error (pointer
    /// arithmetic that wraps there is ordinary byte arithmetic and
    /// stays allowed).
    fn address_const(&mut self, idx: i32) -> Result<i32, CompileError> {
        if !(0..=255).contains(&idx) {
            return Err(err("address is outside the addressable range"));
        }
        Ok(self.const_value(idx as u8))
    }

    /// What a subscript base names: an array (base cell, length) or
    /// a pointer variable (its cell). Anything else is an error
    /// with the same messages as the static subscript path.
    fn subscript_base(&self, base: &Expr) -> Result<SubBase, CompileError> {
        match base {
            Expr::Ident(name) => {
                if let Some((cell_base, len)) = self.memory.array_info(name) {
                    Ok(SubBase::Array(cell_base, len))
                } else if self.memory.is_pointer(name) {
                    match self.memory.var_index(name) {
                        Some(cell) => Ok(SubBase::Pointer(cell)),
                        None => Err(err("pointer is not declared")),
                    }
                } else if self.memory.has_var(name) {
                    Err(err("subscript of non-array"))
                } else {
                    Err(err("subscript of undeclared array"))
                }
            }
            Expr::Paren(inner) => self.subscript_base(inner),
            Expr::Index { .. } => Err(err("multi-dimensional arrays are not supported")),
            _ => Err(err("unsupported subscript base")),
        }
    }

    /// Cell holding the tape index of an element: `base + index` as
    /// byte arithmetic. A constant array index is bounds-checked;
    /// anything else wraps like all byte arithmetic.
    fn index_address(&mut self, base: &Expr, index: &Expr) -> Result<i32, CompileError> {
        match self.subscript_base(base)? {
            SubBase::Array(cell_base, len) => {
                if let Some(k) = Self::const_index(index) {
                    if k < 0 || k as usize >= len {
                        return Err(err("array index out of bounds"));
                    }
                    self.address_const(cell_base + k)
                } else {
                    let base_cell = self.address_const(cell_base)?;
                    let idx = self.compile_expr(index)?;
                    Ok(self.add_values(base_cell, idx))
                }
            }
            SubBase::Pointer(ptr_cell) => {
                if let Some(k) = Self::const_index(index) {
                    if k == 0 {
                        Ok(ptr_cell)
                    } else {
                        let k_cell = self.const_value(k as u8);
                        Ok(self.add_values(ptr_cell, k_cell))
                    }
                } else {
                    let idx = self.compile_expr(index)?;
                    Ok(self.add_values(ptr_cell, idx))
                }
            }
        }
    }

    /// Cell holding the tape index of the storage named by `e`:
    /// a variable, an array (its base), an element (its address),
    /// or a dereference (the address its operand evaluates to).
    fn address_value(&mut self, e: &Expr) -> Result<i32, CompileError> {
        match e {
            Expr::Ident(name) => {
                if let Some((base, _)) = self.memory.array_info(name) {
                    self.address_const(base)
                } else {
                    let idx = self.memory.var_cell(name);
                    self.address_const(idx)
                }
            }
            Expr::Paren(inner) => self.address_value(inner),
            Expr::Index { base, index } => self.index_address(base, index),
            Expr::Unary { op, e: inner } if op.as_slice() == b"*" => self.compile_expr(inner),
            _ => Err(err("unsupported address expression")),
        }
    }

    /// Whether `e` is the address of a cell whose index is only
    /// known at run time: a variable-index subscript or a
    /// dereference. Such targets go through dispatch instead of
    /// the static-cell paths.
    fn needs_address_path(&self, e: &Expr) -> bool {
        match e {
            Expr::Paren(inner) => self.needs_address_path(inner),
            Expr::Index { base, index } => match self.subscript_base(base) {
                Ok(SubBase::Pointer(_)) => true,
                Ok(SubBase::Array(..)) => Self::const_index(index).is_none(),
                Err(_) => false,
            },
            Expr::Unary { op, .. } => op.as_slice() == b"*",
            _ => false,
        }
    }

    /// Store destination for an assignment target: a static cell,
    /// or a runtime address plus the cells a dispatch searches
    /// (an array range, or every byte address when absent).
    fn store_target(&mut self, e: &Expr) -> Result<StoreTarget, CompileError> {
        match e {
            Expr::Ident(name) => {
                if self.memory.is_array(name) {
                    Err(err("cannot assign to an array"))
                } else {
                    Ok(StoreTarget::Cell(self.memory.var_cell(name)))
                }
            }
            Expr::Paren(inner) => self.store_target(inner),
            Expr::Index { base, index } => match self.subscript_base(base)? {
                SubBase::Array(cell_base, len) => {
                    if Self::const_index(index).is_some() {
                        Ok(StoreTarget::Cell(self.static_index_cell(base, index)?))
                    } else {
                        // The dispatch counts the offset down, so the
                        // counter is the index value itself; no
                        // address is materialized, and the array may
                        // live anywhere on the tape.
                        let counter = self.compile_expr(index)?;
                        let n = len.min(256) as i32;
                        Ok(StoreTarget::Range {
                            counter,
                            cells: (cell_base..cell_base + n).collect(),
                        })
                    }
                }
                SubBase::Pointer(_) => {
                    let addr = self.index_address(base, index)?;
                    Ok(StoreTarget::Any { addr })
                }
            },
            Expr::Unary { op, e: inner } if op.as_slice() == b"*" => {
                let addr = self.compile_expr(inner)?;
                Ok(StoreTarget::Any { addr })
            }
            _ => Err(err("invalid lvalue in assignment")),
        }
    }

    /// Write `value` into a store destination.
    fn store_value(&mut self, target: &StoreTarget, value: i32) -> Result<i32, CompileError> {
        match target {
            StoreTarget::Cell(cell) => Ok(self.copy_value(value, *cell)),
            StoreTarget::Range { counter, cells } => {
                self.store_to_cells(*counter, cells, value)?;
                Ok(value)
            }
            StoreTarget::Any { addr } => {
                self.store_through(*addr, value)?;
                Ok(value)
            }
        }
    }

    /// Value held by an assignment target, plus its store
    /// destination for a read-modify-write: static cells are their
    /// own value cell; address targets load through dispatch.
    fn load_target(&mut self, e: &Expr) -> Result<(i32, StoreTarget), CompileError> {
        let target = self.store_target(e)?;
        let value = match &target {
            StoreTarget::Cell(cell) => *cell,
            StoreTarget::Range { counter, cells } => {
                self.load_from_cells(*counter, cells)?
            }
            StoreTarget::Any { addr } => self.load_through(*addr)?,
        };
        Ok((value, target))
    }

    /// Static cell holding the storage of an address-of operand, if
    /// the address is statically known: a variable, an array base,
    /// or a constant-index element. `None` means the address must
    /// be computed into a cell at run time.
    fn static_address_cell(&self, e: &Expr) -> Option<i32> {
        match e {
            Expr::Ident(name) => {
                if let Some((base, _)) = self.memory.array_info(name) {
                    Some(base)
                } else {
                    self.memory.var_index(name)
                }
            }
            Expr::Paren(inner) => self.static_address_cell(inner),
            Expr::Index { base, index } => match self.subscript_base(base).ok()? {
                SubBase::Array(cell_base, len) => {
                    let k = Self::const_index(index)?;
                    if k < 0 || k as usize >= len {
                        return None;
                    }
                    Some(cell_base + k)
                }
                SubBase::Pointer(_) => None,
            },
            _ => None,
        }
    }

    /// Whether `e`, through parentheses, names an array (arrays are
    /// not modifiable: increment and decrement must reject them
    /// rather than modify a decayed address temporary).
    fn names_array(&self, e: &Expr) -> bool {
        match e {
            Expr::Ident(name) => self.memory.is_array(name),
            Expr::Paren(inner) => self.names_array(inner),
            _ => false,
        }
    }

    /// Reject dereferencing a plain scalar variable (or an
    /// undeclared name). Arrays and pointers, and any other
    /// expression (whose value is an address), pass.
    fn check_deref_operand(&self, e: &Expr) -> Result<(), CompileError> {
        match e {
            Expr::Paren(inner) => self.check_deref_operand(inner),
            Expr::Ident(name) => {
                if self.memory.is_array(name) || self.memory.is_pointer(name) {
                    Ok(())
                } else if self.memory.has_var(name) {
                    Err(err("dereference of non-pointer"))
                } else {
                    Err(err("dereference of undeclared identifier"))
                }
            }
            _ => Ok(()),
        }
    }

    fn compile_block(&mut self, b: &Block) -> Result<(), CompileError> {
        // Once a sibling may have executed `break`, `continue`, or
        // (inside a function expansion) `return`, the rest of the
        // block must be skipped at runtime (a bare flag checked only
        // by the loop condition would let them run once more).
        let mut after_break = false;
        let mut after_continue = false;
        let mut after_return = false;
        for item in &b.0 {
            match item {
                BlockItem::Decl(d) => {
                    if after_return {
                        self.compile_decl_return_guarded(d)?
                    } else if after_break || after_continue {
                        self.compile_decl_guarded(d)?
                    } else {
                        self.compile_decl(d)?
                    }
                }
                BlockItem::Stmt(s) => {
                    if after_return {
                        self.compile_stmt_return_guarded(s)?
                    } else if after_break || after_continue {
                        self.compile_stmt_guarded(s)?
                    } else {
                        self.compile_stmt(s)?
                    }
                    if stmt_may_break(s) {
                        after_break = true;
                    }
                    if stmt_may_continue(s) {
                        after_continue = true;
                    }
                    // `return` in `main` is a no-op, so it gates
                    // nothing; inside an expansion it gates the rest
                    // of the block.
                    if self.return_context.is_some() && stmt_may_return(s) {
                        after_return = true;
                    }
                }
            }
        }
        Ok(())
    }

    /// Compile `s`, skipping it at runtime if the innermost break flag is
    /// set (statements after a potential `break` in the same block).
    /// The continue guard layers inside, so a statement after both a
    /// possible `break` and a possible `continue` needs neither flag set.
    fn compile_stmt_guarded(&mut self, s: &Stmt) -> Result<(), CompileError> {
        if let Some(&flag) = self.break_stack.last() {
            if flag >= 0 {
                let nb = self.negate(flag)?;
                return self.perform_if(nb, |em| em.compile_stmt_guarded_continue(s));
            }
        }
        self.compile_stmt_guarded_continue(s)
    }

    /// Continue half of `compile_stmt_guarded`: skip `s` at runtime if
    /// the innermost continue flag is set.
    fn compile_stmt_guarded_continue(&mut self, s: &Stmt) -> Result<(), CompileError> {
        if let Some(&flag) = self.continue_stack.last() {
            if flag >= 0 {
                let nc = self.negate(flag)?;
                return self.perform_if(nc, |em| em.compile_stmt(s));
            }
        }
        self.compile_stmt(s)
    }

    /// Like `compile_stmt_guarded` for declarations.
    fn compile_decl_guarded(&mut self, d: &Decl) -> Result<(), CompileError> {
        if let Some(&flag) = self.break_stack.last() {
            if flag >= 0 {
                let nb = self.negate(flag)?;
                return self.perform_if(nb, |em| em.compile_decl_guarded_continue(d));
            }
        }
        self.compile_decl_guarded_continue(d)
    }

    /// Continue half of `compile_decl_guarded`.
    fn compile_decl_guarded_continue(&mut self, d: &Decl) -> Result<(), CompileError> {
        if let Some(&flag) = self.continue_stack.last() {
            if flag >= 0 {
                let nc = self.negate(flag)?;
                return self.perform_if(nc, |em| em.compile_decl(d));
            }
        }
        self.compile_decl(d)
    }

    /// Allocate the `broken` flag for a loop whose body may break, and
    /// push it (or the `-1` sentinel when the body cannot break, keeping
    /// the old error for any missed case). Returns the flag cell, if any.
    fn loop_break_flag(&mut self, body: &Stmt) -> Result<Option<i32>, CompileError> {
        if stmt_may_break(body) {
            let broken = self.const_value(0);
            self.break_stack.push(broken);
            Ok(Some(broken))
        } else {
            self.break_stack.push(-1);
            Ok(None)
        }
    }

    /// Condition `c` additionally gated on the `broken` flag being clear
    /// (identity when the loop has no flag).
    fn and_not_broken(&mut self, c: i32, broken: Option<i32>) -> Result<i32, CompileError> {
        match broken {
            Some(b) => {
                let nb = self.negate(b)?;
                self.logical_and(c, nb)
            }
            None => Ok(c),
        }
    }

    /// Allocate the `continued` flag for a loop whose body may continue,
    /// and push it (or the `-1` sentinel when the body cannot continue,
    /// keeping the old error for any missed case). Returns the flag cell,
    /// if any. Unlike `broken`, this flag does not gate the loop
    /// condition — `continue` re-tests it — but it must be cleared at
    /// the start of each body execution (see the loop arms).
    fn loop_continue_flag(&mut self, body: &Stmt) -> Result<Option<i32>, CompileError> {
        if stmt_may_continue(body) {
            let continued = self.const_value(0);
            self.continue_stack.push(continued);
            Ok(Some(continued))
        } else {
            self.continue_stack.push(-1);
            Ok(None)
        }
    }

    /// Condition `c` additionally gated on the `continued` flag being
    /// clear (identity when no flag is active). Used for `switch` case
    /// guards: a `continue` inside a case must stop later cases running.
    fn and_not_continued(&mut self, c: i32) -> Result<i32, CompileError> {
        match self.continue_stack.last().copied() {
            Some(f) if f >= 0 => {
                let nc = self.negate(f)?;
                self.logical_and(c, nc)
            }
            _ => Ok(c),
        }
    }

    /// Condition `c` additionally gated on the innermost function
    /// not having returned (identity outside an expansion, and in
    /// `main`, which has no return context).
    fn and_not_returned(&mut self, c: i32) -> Result<i32, CompileError> {
        match self.return_context {
            Some((flag, _)) => {
                let nr = self.negate(flag)?;
                self.logical_and(c, nr)
            }
            None => Ok(c),
        }
    }

    /// Run `f` at runtime only if the innermost function (if any) has
    /// not returned. Outside an expansion `f` runs unconditionally.
    fn unless_returned(
        &mut self,
        mut f: impl FnMut(&mut Self) -> Result<(), CompileError>,
    ) -> Result<(), CompileError> {
        match self.return_context {
            Some((flag, _)) => {
                let nr = self.negate(flag)?;
                self.perform_if(nr, |em| f(em))
            }
            None => f(self),
        }
    }

    /// Like `compile_stmt_guarded`, additionally skipped at runtime
    /// once the innermost function has returned (statements after a
    /// potential `return` in the same block). The return guard is the
    /// outer layer, so a returned function skips the statement
    /// outright instead of consulting the loop flags.
    fn compile_stmt_return_guarded(&mut self, s: &Stmt) -> Result<(), CompileError> {
        self.unless_returned(|em| em.compile_stmt_guarded(s))
    }

    /// Like `compile_decl_guarded`, additionally return-guarded.
    fn compile_decl_return_guarded(&mut self, d: &Decl) -> Result<(), CompileError> {
        self.unless_returned(|em| em.compile_decl_guarded(d))
    }

    /// Clear the `continued` flag at the start of a loop-body execution,
    /// then compile the body. Clearing here (rather than after the body)
    /// is what makes `continue` skip the rest of the body but still run
    /// the `for` update and re-test the condition on the next pass.
    fn compile_loop_body(
        &mut self,
        body: &Stmt,
        continued: Option<i32>,
    ) -> Result<(), CompileError> {
        if let Some(flag) = continued {
            self.set_value(flag, 0);
        }
        self.compile_stmt(body)
    }

    fn compile_stmt(&mut self, s: &Stmt) -> Result<(), CompileError> {
        match s {
            Stmt::Compound(b) => self.compile_block(b),
            Stmt::Expr(e) => {
                self.compile_expr(e)?;
                Ok(())
            }
            Stmt::Empty => Ok(()),
            Stmt::If { cond, then, els } => {
                let c = self.compile_expr(cond)?;
                self.perform_if_else(
                    c,
                    |em| em.compile_stmt(then),
                    |em| match els {
                        Some(e) => em.compile_stmt(e),
                        None => Ok(()),
                    },
                )
            }
            Stmt::Switch { expr, body } => self.compile_switch(expr, body),
            Stmt::While { cond, body } => {
                let broken = self.loop_break_flag(body)?;
                let continued = self.loop_continue_flag(body)?;
                let r = self.perform_while(
                    |em| {
                        let c = em.compile_expr(cond)?;
                        let c = em.and_not_broken(c, broken)?;
                        em.and_not_returned(c)
                    },
                    |em| em.compile_loop_body(body, continued),
                );
                self.continue_stack.pop();
                self.break_stack.pop();
                r
            }
            Stmt::DoWhile { body, cond } => {
                let broken = self.loop_break_flag(body)?;
                let continued = self.loop_continue_flag(body)?;
                let r = self.perform_do_while(
                    |em| {
                        let c = em.compile_expr(cond)?;
                        let c = em.and_not_broken(c, broken)?;
                        em.and_not_returned(c)
                    },
                    |em| em.compile_loop_body(body, continued),
                );
                self.continue_stack.pop();
                self.break_stack.pop();
                r
            }
            Stmt::For { init, cond, incr, body } => {
                let broken = self.loop_break_flag(body)?;
                let continued = self.loop_continue_flag(body)?;
                let r = self.perform_for(
                    |em| em.compile_for_init(init),
                    |em| {
                        let c = match cond {
                            Some(e) => em.compile_expr(e),
                            None => Ok(em.const_value(1)),
                        }?;
                        let c = em.and_not_broken(c, broken)?;
                        em.and_not_returned(c)
                    },
                    |em| {
                        // `return` skips the update, as `break` does:
                        // run it only if the function has not returned.
                        em.unless_returned(|em| {
                            // `break` skips the update: run it only if not broken.
                            if let Some(b) = broken {
                                let nb = em.negate(b)?;
                                em.perform_if_else(
                                    nb,
                                    |em| match incr {
                                        Some(e) => em.compile_expr(e).map(|_| ()),
                                        None => Ok(()),
                                    },
                                    |_| Ok(()),
                                )
                            } else {
                                match incr {
                                    Some(e) => em.compile_expr(e).map(|_| ()),
                                    None => Ok(()),
                                }
                            }
                        })
                    },
                    |em| em.compile_loop_body(body, continued),
                );
                self.continue_stack.pop();
                self.break_stack.pop();
                r
            }
            // In `main` return values are ignored: the C++ never
            // compiles the operand. Inside a function expansion the
            // operand is the result and the returned flag skips the
            // rest of the body at runtime (see `compile_block`).
            Stmt::Return(e) => {
                let Some((returned, result)) = self.return_context else {
                    return Ok(());
                };
                if let Some(e) = e {
                    let v = self.compile_expr(e)?;
                    self.copy_value(v, result);
                }
                self.set_value(returned, 1);
                Ok(())
            }
            Stmt::Break => {
                let Some(&top) = self.break_stack.last() else {
                    return Err(err("Unsupported break outside of any loop or switch"));
                };
                if top < 0 {
                    return Err(err("Unsupported break inside loops (only switch is supported)"));
                }
                self.set_value(top, 1);
                Ok(())
            }
            Stmt::Continue => {
                // Binds the innermost loop (through a `switch`, unlike
                // `break`): set its `continued` flag. The rest of the
                // body is skipped by the sibling guards, the `for`
                // update still runs, and the flag is cleared at the
                // next body execution.
                let Some(&top) = self.continue_stack.last() else {
                    return Err(err("Unsupported continue outside of any loop"));
                };
                if top < 0 {
                    return Err(err("Unsupported continue inside loops"));
                }
                self.set_value(top, 1);
                Ok(())
            }
            // Labels outside `switch`: the label (if any) compiles and is
            // dropped, the body compiles normally — the base visitor does
            // the same with unhandled labeled statements.
            Stmt::Case { label, body } => {
                self.compile_expr(label)?;
                self.compile_stmt(body)
            }
            Stmt::Default { body } => self.compile_stmt(body),
            Stmt::GotoLabel { body, .. } => self.compile_stmt(body),
        }
    }

    fn compile_for_init(&mut self, init: &ForInit) -> Result<(), CompileError> {
        match init {
            ForInit::Decl(d) => self.compile_for_decl(d),
            ForInit::Empty => Ok(()),
            ForInit::Expr(e) => {
                self.compile_expr(e)?;
                Ok(())
            }
        }
    }

    fn compile_for_decl(&mut self, d: &ForDecl) -> Result<(), CompileError> {
        let name = match &d.name {
            // `for (int;;)` — specifier only, no code.
            None => return Ok(()),
            Some(name) => name,
        };
        let e = match &d.init {
            None => return Err(err("Unsupported init declarator")),
            Some(e) => e,
        };
        let dest = self.memory.var_cell(name);
        let src = self.compile_expr(e)?;
        self.copy_value(src, dest);
        Ok(())
    }

    fn compile_switch(&mut self, expr: &Expr, body: &Stmt) -> Result<(), CompileError> {
        let expr_cell = self.compile_expr(expr)?;
        let mut cases: Vec<SwitchCase<'_>> = Vec::new();
        self.collect_switch_stmt(body, &mut cases)?;
        let any_match = self.const_value(0);
        let mut matched: Vec<i32> = Vec::new();
        for c in &cases {
            if c.is_default {
                matched.push(-1);
                continue;
            }
            let m = self.is_equal(expr_cell, c.label_cell)?;
            matched.push(m);
            let o = self.logical_or(any_match, m)?;
            self.move_value(o, any_match);
        }
        let fall = self.const_value(0);
        let broken = self.const_value(0);
        self.break_stack.push(broken);
        for (i, c) in cases.iter().enumerate() {
            // GCC right-to-left within each nested call (see order probe).
            let run_cond = if c.is_default {
                let nb = self.negate(broken)?;
                let nm = self.negate(any_match)?;
                let o = self.logical_or(fall, nm)?;
                self.logical_and(o, nb)?
            } else {
                let nb = self.negate(broken)?;
                let o = self.logical_or(matched[i], fall)?;
                self.logical_and(o, nb)?
            };
            // A `continue` in a case binds the enclosing loop: later
            // cases must not run (fallthrough is cut) and the loop's
            // `continued` flag stays set until the next body execution.
            // A `return` in a case likewise cuts later cases.
            let run_cond = self.and_not_continued(run_cond)?;
            let run_cond = self.and_not_returned(run_cond)?;
            self.perform_if_else(
                run_cond,
                |em| {
                    em.set_value(fall, 1);
                    // Same sibling rule as blocks: statements after a
                    // potential `break`, `continue`, or `return` in
                    // this case are flag-guarded.
                    let mut after_break = false;
                    let mut after_continue = false;
                    let mut after_return = false;
                    for item in &c.body {
                        match item {
                            SwitchBodyItem::Decl(d) => {
                                if after_return {
                                    em.compile_decl_return_guarded(d)?
                                } else if after_break || after_continue {
                                    em.compile_decl_guarded(d)?
                                } else {
                                    em.compile_decl(d)?
                                }
                            }
                            SwitchBodyItem::Stmt(s) => {
                                if after_return {
                                    em.compile_stmt_return_guarded(s)?
                                } else if after_break || after_continue {
                                    em.compile_stmt_guarded(s)?
                                } else {
                                    em.compile_stmt(s)?
                                }
                                if stmt_may_break(s) {
                                    after_break = true;
                                }
                                if stmt_may_continue(s) {
                                    after_continue = true;
                                }
                                if em.return_context.is_some() && stmt_may_return(s) {
                                    after_return = true;
                                }
                            }
                        }
                    }
                    Ok(())
                },
                |_| Ok(()),
            )?;
        }
        self.break_stack.pop();
        Ok(())
    }

    fn collect_switch_stmt<'a>(
        &mut self,
        s: &'a Stmt,
        cases: &mut Vec<SwitchCase<'a>>,
    ) -> Result<(), CompileError> {
        match s {
            Stmt::Case { label, body } => {
                let cell = self.compile_expr(label)?;
                cases.push(SwitchCase {
                    is_default: false,
                    label_cell: cell,
                    body: Vec::new(),
                });
                self.collect_switch_stmt(body, cases)
            }
            Stmt::Default { body } => {
                cases.push(SwitchCase {
                    is_default: true,
                    label_cell: -1,
                    body: Vec::new(),
                });
                self.collect_switch_stmt(body, cases)
            }
            Stmt::Compound(b) => {
                for item in &b.0 {
                    match item {
                        BlockItem::Decl(d) => {
                            push_switch_body(SwitchBodyItem::Decl(d), cases)?
                        }
                        BlockItem::Stmt(s) => self.collect_switch_stmt(s, cases)?,
                    }
                }
                Ok(())
            }
            other => push_switch_body(SwitchBodyItem::Stmt(other), cases),
        }
    }

    fn compile_expr(&mut self, e: &Expr) -> Result<i32, CompileError> {
        match e {
            Expr::Int(bytes) => {
                // Radix/suffix handling lives in `int_literal_value`;
                // the byte narrowing matches the old `atoi`+clamp shape.
                Ok(self.const_value(int_literal_value(bytes) as u8))
            }
            Expr::Char(bytes) => {
                // Raw slice including quotes; byte 1 is the value — even for
                // escapes (`'\n'` yields '\\' = 92, exactly like the C++).
                Ok(self.const_value(bytes.get(1).copied().unwrap_or(0)))
            }
            Expr::Ident(name) => {
                // A bare array name decays to the address of its
                // first element; scalars and pointers keep the
                // variable cell itself.
                if let Some((base, _)) = self.memory.array_info(name) {
                    self.address_const(base)
                } else {
                    Ok(self.memory.var_cell(name))
                }
            }
            // A string literal in value position decays to the
            // address of its anonymous storage (the format
            // argument of printf/scanf never reaches here).
            Expr::StringLit { cooked, .. } => self.string_value(cooked),
            Expr::Paren(e) => self.compile_expr(e),
            Expr::Comma(es) => {
                debug_assert!(!es.is_empty());
                let mut last = 0;
                for e in es {
                    last = self.compile_expr(e)?;
                }
                Ok(last)
            }
            Expr::Assign { op, lhs, rhs } => {
                if op.as_slice() == b"=" {
                    // GCC right-to-left: the target (last argument) first,
                    // so a subscript's index expression compiles before
                    // the right-hand side.
                    let target = self.store_target(lhs)?;
                    let r = self.compile_expr(rhs)?;
                    self.store_value(&target, r)
                } else {
                    // Sequenced statements in C++: right-hand side first.
                    let r = self.compile_expr(rhs)?;
                    let (target_value, target) = self.load_target(lhs)?;
                    let res = match op.as_slice() {
                        b"+=" => self.add_values(target_value, r),
                        b"-=" => self.subtract_values(target_value, r),
                        b"*=" => self.multiply_values(target_value, r),
                        b"/=" => self.divide_values(target_value, r)?,
                        b"%=" => self.mod_values(target_value, r)?,
                        b"&=" => self.bitwise_and_values(target_value, r)?,
                        b"|=" => self.bitwise_or_values(target_value, r)?,
                        b"^=" => self.bitwise_xor_values(target_value, r)?,
                        b"<<=" => self.shift_left_values(target_value, r)?,
                        b">>=" => self.shift_right_values(target_value, r)?,
                        _ => return Err(err("Unsupported assignment expression")),
                    };
                    self.store_value(&target, res)
                }
            }
            Expr::Ternary { cond, then, els } => {
                // Condition once; exactly one branch runs, writing the
                // result cell (mirrors the `if`/`else` statement shape).
                let c = self.compile_expr(cond)?;
                let result = self.const_value(0);
                self.perform_if_else(
                    c,
                    |em| {
                        let t = em.compile_expr(then)?;
                        em.copy_value(t, result);
                        Ok(())
                    },
                    |em| {
                        let e = em.compile_expr(els)?;
                        em.copy_value(e, result);
                        Ok(())
                    },
                )?;
                Ok(result)
            }
            Expr::BinOp { op, l, r } => {
                // GCC evaluates arithmetic operands left-to-right but every
                // other binary operator right-to-left (measured per site:
                // the probe plus cmp.c/logic.c green runs pin comparisons
                // and logic; addition/div/mul/mod/order_arith pin arithmetic;
                // bitwise/shift stay on the default right-to-left arm, and
                // the bitwise fixture's double-side-effect lines pin it).
                let (ll, rr) = match op.as_slice() {
                    b"+" | b"-" | b"*" | b"/" | b"%" => {
                        let ll = self.compile_expr(l)?;
                        let rr = self.compile_expr(r)?;
                        (ll, rr)
                    }
                    _ => {
                        let rr = self.compile_expr(r)?;
                        let ll = self.compile_expr(l)?;
                        (ll, rr)
                    }
                };
                match op.as_slice() {
                    b"+" => Ok(self.add_values(ll, rr)),
                    b"-" => Ok(self.subtract_values(ll, rr)),
                    b"*" => Ok(self.multiply_values(ll, rr)),
                    b"/" => self.divide_values(ll, rr),
                    b"%" => self.mod_values(ll, rr),
                    b"==" => self.is_equal(ll, rr),
                    b"!=" => {
                        let e = self.is_equal(ll, rr)?;
                        self.negate(e)
                    }
                    b"&&" => self.logical_and(ll, rr),
                    b"||" => self.logical_or(ll, rr),
                    b"<" => self.less_than(ll, rr),
                    b"<=" => self.less_than_or_equal(ll, rr),
                    b">" => self.greater_than(ll, rr),
                    b">=" => self.greater_than_or_equal(ll, rr),
                    b"<<" => self.shift_left_values(ll, rr),
                    b">>" => self.shift_right_values(ll, rr),
                    b"&" => self.bitwise_and_values(ll, rr),
                    b"|" => self.bitwise_or_values(ll, rr),
                    b"^" => self.bitwise_xor_values(ll, rr),
                    _ => Err(err("Unsupported binary operator")),
                }
            }
            Expr::Unary { op, e } => match op.as_slice() {
                b"!" => {
                    let v = self.compile_expr(e)?;
                    self.negate(v)
                }
                // Wrapping arithmetic: `-x` is `0 - x`, `~x` is
                // `255 - x`, `+x` is `x` itself. Printed with `%d`
                // these differ from C `int` values for negatives;
                // only wrap-back uses agree with the g++ oracle
                // (see the `unary` differential fixture).
                b"-" => {
                    let v = self.compile_expr(e)?;
                    let zero = self.const_value(0);
                    Ok(self.subtract_values(zero, v))
                }
                b"+" => self.compile_expr(e),
                b"~" => {
                    let v = self.compile_expr(e)?;
                    let top = self.const_value(255);
                    Ok(self.subtract_values(top, v))
                }
                // Address-of: a cell holding the tape index of
                // the operand's storage (variables, array bases,
                // elements, and dereferences).
                b"&" => self.address_value(e),
                // Dereference: load the cell at the tape index the
                // operand evaluates to. A scalar variable is not a
                // pointer; anything else holds an address.
                b"*" => {
                    self.check_deref_operand(e)?;
                    let addr = self.compile_expr(e)?;
                    self.load_through(addr)
                }
                b"++" => {
                    if self.names_array(e) {
                        return Err(err("cannot modify an array"));
                    }
                    if self.needs_address_path(e) {
                        let (v, target) = self.load_target(e)?;
                        let one = self.const_value(1);
                        let sum = self.add_values(v, one);
                        self.store_value(&target, sum)?;
                        Ok(self.duplicate_value(sum))
                    } else {
                        let t = self.compile_expr(e)?;
                        let one = self.const_value(1);
                        let sum = self.add_values(t, one);
                        self.copy_value(sum, t);
                        Ok(self.duplicate_value(t))
                    }
                }
                b"--" => {
                    if self.names_array(e) {
                        return Err(err("cannot modify an array"));
                    }
                    if self.needs_address_path(e) {
                        let (v, target) = self.load_target(e)?;
                        let one = self.const_value(1);
                        let diff = self.subtract_values(v, one);
                        self.store_value(&target, diff)?;
                        Ok(self.duplicate_value(diff))
                    } else {
                        let t = self.compile_expr(e)?;
                        let one = self.const_value(1);
                        let diff = self.subtract_values(t, one);
                        self.copy_value(diff, t);
                        Ok(self.duplicate_value(t))
                    }
                }
                _ => Err(err("Unsupported unary expression")),
            },
            // Casts are no-ops: every value is already a byte, and
            // literals and operations already truncate.
            Expr::Cast { e, .. } => self.compile_expr(e),
            // sizeof is always 1: every type is one byte here. The
            // operand is never compiled (unevaluated, as in C) — except
            // that an array operand yields its element count.
            Expr::Sizeof(e) => {
                let size = self.array_size_of(e).unwrap_or(1);
                Ok(self.const_value(size as u8))
            }
            Expr::SizeofType(_) => Ok(self.const_value(1)),
            Expr::Postfix { op, e } => {
                if self.names_array(e) {
                    return Err(err("cannot modify an array"));
                }
                if self.needs_address_path(e) {
                    let (old, target) = self.load_target(e)?;
                    let one = self.const_value(1);
                    if op.as_slice() == b"++" {
                        let sum = self.add_values(old, one);
                        self.store_value(&target, sum)?;
                    } else if op.as_slice() == b"--" {
                        let diff = self.subtract_values(old, one);
                        self.store_value(&target, diff)?;
                    } else {
                        return Err(err("Unsupported postfix expression"));
                    }
                    return Ok(old);
                }
                let target = self.compile_expr(e)?;
                let old = self.duplicate_value(target);
                let one = self.const_value(1);
                if op.as_slice() == b"++" {
                    let sum = self.add_values(target, one);
                    self.copy_value(sum, target);
                } else if op.as_slice() == b"--" {
                    let diff = self.subtract_values(target, one);
                    self.copy_value(diff, target);
                } else {
                    return Err(err("Unsupported postfix expression"));
                }
                Ok(old)
            }
            Expr::Call { callee, args } => self.compile_call(callee, args),
            // Constant array indices resolve to a static cell,
            // like a variable; anything else loads through dispatch
            // over the array's range (or every address, for a
            // pointer base).
            Expr::Index { base, index } => match self.subscript_base(base)? {
                SubBase::Array(cell_base, len) => {
                    if Self::const_index(index).is_some() {
                        self.static_index_cell(base, index)
                    } else {
                        // As in `store_target`: the dispatch counts
                        // the offset itself down.
                        let counter = self.compile_expr(index)?;
                        let n = len.min(256) as i32;
                        let cells: Vec<i32> = (cell_base..cell_base + n).collect();
                        self.load_from_cells(counter, &cells)
                    }
                }
                SubBase::Pointer(_) => {
                    let addr = self.index_address(base, index)?;
                    self.load_through(addr)
                }
            },
        }
    }

    /// Static array range (base, remaining length) for an
    /// address-of operand naming an array or a constant-index
    /// element of one, through parentheses. `None` when the bound
    /// is not statically known.
    fn string_bound(&self, e: &Expr) -> Option<(i32, usize)> {
        match e {
            Expr::Paren(inner) => self.string_bound(inner),
            Expr::Ident(name) => self.memory.array_info(name),
            Expr::Index { base, index } => match self.subscript_base(base).ok()? {
                SubBase::Array(cell_base, len) => {
                    let k = Self::const_index(index)?;
                    if k < 0 || k as usize >= len {
                        return None;
                    }
                    Some((cell_base + k, len - k as usize))
                }
                SubBase::Pointer(_) => None,
            },
            _ => None,
        }
    }

    /// Compile a `scanf` destination: a static cell for a variable,
    /// a bounded array range, or a statically known element;
    /// otherwise a runtime address to store through. A bare pointer
    /// variable holds its address, so it stores through its own cell.
    fn compile_scanf_target(&mut self, a: &Expr) -> Result<ScanfTarget, CompileError> {
        match a {
            Expr::Ident(name) => {
                if let Some((base, len)) = self.memory.array_info(name) {
                    Ok(ScanfTarget::Bounded { base, len })
                } else if self.memory.is_pointer(name) {
                    Ok(ScanfTarget::Through(self.memory.var_cell(name)))
                } else {
                    Ok(ScanfTarget::Cell(self.memory.var_cell(name)))
                }
            }
            Expr::Paren(inner) => self.compile_scanf_target(inner),
            Expr::Unary { op, e: inner } if op.as_slice() == b"&" => {
                if let Some((base, len)) = self.string_bound(inner) {
                    Ok(ScanfTarget::Bounded { base, len })
                } else if let Some(cell) = self.static_address_cell(inner) {
                    Ok(ScanfTarget::Cell(cell))
                } else {
                    Ok(ScanfTarget::Through(self.address_value(inner)?))
                }
            }
            _ => Err(err("scanf argument must be an address")),
        }
    }

    /// Read one `scanf` value (`%c` or `%d`) into a destination:
    /// directly for a static cell (a bounded range writes its
    /// first cell, as before), through dispatch for a runtime
    /// address.
    fn read_into_target(&mut self, target: ScanfTarget, is_char: bool) -> Result<(), CompileError> {
        match target {
            ScanfTarget::Cell(cell) | ScanfTarget::Bounded { base: cell, .. } => {
                if is_char {
                    self.read_as_char(cell);
                } else {
                    self.read_as_number(cell)?;
                }
                Ok(())
            }
            ScanfTarget::Through(addr) => {
                let tmp = self.const_value(0);
                if is_char {
                    self.read_as_char(tmp);
                } else {
                    self.read_as_number(tmp)?;
                }
                self.store_through(addr, tmp)?;
                Ok(())
            }
        }
    }

    /// Compile a call: only `printf`/`scanf` with a string-literal format.
    fn compile_call(&mut self, callee: &Expr, args: &[Expr]) -> Result<i32, CompileError> {
        let name = match callee {
            Expr::Ident(name) => name.as_slice(),
            _ => return Err(err("Unsupported postfix expression")),
        };
        if self.functions.contains_key(name) {
            let def = Rc::clone(
                self.functions
                    .get(name)
                    .expect("registered function is present"),
            );
            return self.expand_call(name, &def, args);
        }
        // A prototype without a definition has no body to expand:
        // the arity is checked against the prototype first (same
        // message as for definitions), then the missing
        // definition is reported. Calls to names with neither a
        // definition nor a prototype keep the existing
        // "Unsupported postfix expression" error below.
        if let Some(proto) = self.prototypes.get(name) {
            if args.len() != proto.params.len() {
                return Err(err(format!(
                    "wrong number of arguments to function '{}'",
                    String::from_utf8_lossy(name)
                )));
            }
            return Err(err(format!(
                "call to undefined function '{}'",
                String::from_utf8_lossy(name)
            )));
        }
        let is_printf = match name {
            b"printf" => true,
            b"scanf" => false,
            _ => return Err(err("Unsupported postfix expression")),
        };
        // Arguments compile left-to-right (sequenced vector construction).
        // `printf` takes value cells; `scanf` takes destinations, so
        // its arguments compile through `compile_scanf_target`.
        let mut vals: Vec<i32> = Vec::new();
        let mut targets: Vec<ScanfTarget> = Vec::new();
        let mut format: &[u8] = b"";
        for (i, a) in args.iter().enumerate() {
            if i == 0 {
                match a {
                    Expr::StringLit { raw, .. } => format = raw,
                    _ => {
                        // Evaluate for emission parity, then fail: the C++
                        // crashes casting a non-string format argument.
                        self.compile_expr(a)?;
                        return Err(err(if is_printf {
                            "printf format must be a string literal"
                        } else {
                            "scanf format must be a string literal"
                        }));
                    }
                }
            } else if is_printf {
                vals.push(self.compile_expr(a)?);
            } else {
                targets.push(self.compile_scanf_target(a)?);
            }
        }
        let mut next_var = 0usize;
        let mut i = 0usize;
        while i < format.len() {
            let c = format[i];
            if is_printf {
                match c {
                    b'\\' => {
                        i += 1;
                        match format.get(i).copied() {
                            Some(b'n') => self.print_char(b'\n'),
                            Some(b't') => self.print_char(b'\t'),
                            _ => return Err(err("Incorrect backslash in printf")),
                        }
                    }
                    b'%' => {
                        i += 1;
                        // The specifier is checked before consuming an
                        // argument, so `%%` prints a literal `%` without
                        // consuming one. `%u` prints the same digits
                        // as `%d` (values are bytes, never negative);
                        // `%x` prints lowercase hex.
                        match format.get(i).copied() {
                            Some(b'%') => self.print_char(b'%'),
                            Some(spec @ (b'c' | b'd' | b'u' | b'x' | b's')) => {
                                let cell = next_format_arg(&vals, &mut next_var)?;
                                match spec {
                                    b'c' => self.print_as_char(cell),
                                    b'x' => self.print_as_hex(cell)?,
                                    b's' => self.print_as_string(cell)?,
                                    _ => self.print_as_number(cell)?,
                                }
                            }
                            _ => return Err(err("Unexpected printf format")),
                        }
                    }
                    _ => self.print_char(c),
                }
            } else {
                match c {
                    b'%' => {
                        i += 1;
                        let target = next_scanf_target(&targets, &mut next_var)?;
                        match format.get(i).copied() {
                            Some(b'c') => self.read_into_target(target, true)?,
                            Some(b'd') => self.read_into_target(target, false)?,
                            Some(b's') => match target {
                                ScanfTarget::Bounded { base, len } => {
                                    self.read_as_string_bounded(base, len)?
                                }
                                ScanfTarget::Cell(cell) => {
                                    let addr = self.address_const(cell)?;
                                    self.read_as_string_through(addr)?;
                                }
                                ScanfTarget::Through(addr) => {
                                    self.read_as_string_through(addr)?
                                }
                            },
                            _ => return Err(err("Unexpected scanf format")),
                        }
                    }
                    b' ' => {}
                    _ => return Err(err("Unexpected scanf format")),
                }
            }
            i += 1;
        }
        // Calls yield no usable value (the C++ returns null Any); statements
        // drop it. Zero is a dummy for the unreachable use-as-value spot.
        Ok(0)
    }

    /// Inline-expand a call to a user-defined function: evaluate the
    /// arguments left-to-right (as for `printf`/`scanf` here; GCC
    /// leaves the order unspecified), bind them to fresh cells for
    /// the scope-renamed parameters, and compile the renamed body
    /// under a fresh `returned` flag and result cell. Returns the
    /// result cell; a `void` call leaves it zero, and a bare
    /// `return;` leaves whatever value was last returned (zero for a
    /// fresh cell). Temporaries allocated by the expansion are
    /// reclaimed on exit; only the pre-checkpoint result cell (and
    /// variables, which the reclaim floor protects) flow out.
    ///
    /// There is no call stack and no return address: the body is
    /// emitted at the call site, so a function may not call itself,
    /// directly or through other functions. `break`/`continue`
    /// stacks and the return context are saved and restored around
    /// the expansion, so a loop or `return` inside the body binds
    /// to the callee, never to the caller.
    fn expand_call(
        &mut self,
        name: &[u8],
        def: &ExpandedFunction,
        args: &[Expr],
    ) -> Result<i32, CompileError> {
        if self.expanding.iter().any(|n| n.as_slice() == name) {
            return Err(err(format!(
                "recursive call to function '{}' is not supported",
                String::from_utf8_lossy(name)
            )));
        }
        if args.len() != def.params.len() {
            return Err(err(format!(
                "wrong number of arguments to function '{}'",
                String::from_utf8_lossy(name)
            )));
        }
        // The result cell is allocated *before* the reclaim
        // checkpoint below, so the value flows out through a
        // pre-region cell: without this, enough expansions in one
        // program would push later variables past the 256
        // addressable cells and `&` on them would fail. Everything
        // else the expansion allocates is reclaimed on exit;
        // parameters and body locals are variables, so the reclaim
        // floor keeps their cells.
        let result = self.const_value(0);
        let checkpoint = self.checkpoint();
        let returned = self.const_value(0);
        // Each argument value is copied to a fresh cell as soon as
        // it is evaluated: an argument expression may yield a live
        // variable cell (a bare identifier does), and a later
        // argument assigning that variable must not rewrite an
        // earlier argument's value before binding.
        let mut arg_cells = Vec::with_capacity(args.len());
        for arg in args {
            let v = self.compile_expr(arg)?;
            arg_cells.push(self.duplicate_value(v));
        }
        for (param, &arg) in def.params.iter().zip(arg_cells.iter()) {
            let cell = self.memory.var_cell(&param.name);
            if param.pointer || param.array {
                self.memory.mark_pointer(&param.name);
            }
            self.copy_value(arg, cell);
        }
        let saved_break = std::mem::take(&mut self.break_stack);
        let saved_continue = std::mem::take(&mut self.continue_stack);
        let saved_return = self.return_context.replace((returned, result));
        self.expanding.push(name.to_vec());
        let compiled = self.compile_block(&def.body);
        self.expanding.pop();
        self.break_stack = saved_break;
        self.continue_stack = saved_continue;
        self.return_context = saved_return;
        compiled?;
        self.release_temps(checkpoint);
        if def.is_void {
            // A value returned from a `void` function is ignored: a
            // `void` call used as a value yields zero.
            self.set_value(result, 0);
        }
        Ok(result)
    }
}

/// Whether `s` may execute a `break` bound to the *innermost enclosing*
/// loop or switch. Breaks inside nested loops or switches bind to those
/// constructs and do not count; anything else (blocks, `if`, labels)
/// is transparent.
fn stmt_may_break(s: &Stmt) -> bool {
    match s {
        Stmt::Break => true,
        Stmt::Compound(b) => b.0.iter().any(|it| match it {
            BlockItem::Stmt(s) => stmt_may_break(s),
            BlockItem::Decl(_) => false,
        }),
        Stmt::If { then, els, .. } => {
            stmt_may_break(then) || els.as_ref().is_some_and(|e| stmt_may_break(e))
        }
        Stmt::Case { body, .. } | Stmt::Default { body } | Stmt::GotoLabel { body, .. } => {
            stmt_may_break(body)
        }
        Stmt::While { .. }
        | Stmt::DoWhile { .. }
        | Stmt::For { .. }
        | Stmt::Switch { .. } => false,
        _ => false,
    }
}

/// Whether `s` may execute a `continue` bound to the *innermost
/// enclosing* loop. Unlike `break`, `continue` passes through a
/// `switch` (it binds the loop, not the switch); nested loops still
/// bind their own continues and do not count.
fn stmt_may_continue(s: &Stmt) -> bool {
    match s {
        Stmt::Continue => true,
        Stmt::Compound(b) => b.0.iter().any(|it| match it {
            BlockItem::Stmt(s) => stmt_may_continue(s),
            BlockItem::Decl(_) => false,
        }),
        Stmt::If { then, els, .. } => {
            stmt_may_continue(then) || els.as_ref().is_some_and(|e| stmt_may_continue(e))
        }
        Stmt::Switch { body, .. } => stmt_may_continue(body),
        Stmt::Case { body, .. } | Stmt::Default { body } | Stmt::GotoLabel { body, .. } => {
            stmt_may_continue(body)
        }
        Stmt::While { .. } | Stmt::DoWhile { .. } | Stmt::For { .. } => false,
        _ => false,
    }
}

/// Whether `s` may execute a `return`. Unlike `break`/`continue`,
/// a `return` is never bound by an enclosing loop or `switch`: it
/// always belongs to the innermost function, so bodies of loops and
/// switches propagate it.
fn stmt_may_return(s: &Stmt) -> bool {
    match s {
        Stmt::Return(_) => true,
        Stmt::Compound(b) => block_may_return(b),
        Stmt::If { then, els, .. } => {
            stmt_may_return(then) || els.as_ref().is_some_and(|e| stmt_may_return(e))
        }
        Stmt::Switch { body, .. }
        | Stmt::While { body, .. }
        | Stmt::DoWhile { body, .. }
        | Stmt::For { body, .. }
        | Stmt::Case { body, .. }
        | Stmt::Default { body }
        | Stmt::GotoLabel { body, .. } => stmt_may_return(body),
        _ => false,
    }
}

fn block_may_return(b: &Block) -> bool {
    b.0.iter().any(|it| match it {
        BlockItem::Stmt(s) => stmt_may_return(s),
        BlockItem::Decl(_) => false,
    })
}

fn next_format_arg(vals: &[i32], next: &mut usize) -> Result<i32, CompileError> {
    let v = *vals
        .get(*next)
        .ok_or_else(|| err("Too few arguments for format string"))?;
    *next += 1;
    Ok(v)
}

fn next_scanf_target(
    targets: &[ScanfTarget],
    next: &mut usize,
) -> Result<ScanfTarget, CompileError> {
    let t = *targets
        .get(*next)
        .ok_or_else(|| err("Too few arguments for format string"))?;
    *next += 1;
    Ok(t)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compile_program_reports_code_and_cells() {
        let empty = compile_program(b"int main() {}").expect("empty main compiles");
        assert_eq!(empty.code, compile(b"int main() {}").unwrap());
        let bigger = compile_program(
            b"int main() { int x = 0; int y = 0; printf(\"%d\", x + y); }",
        )
        .expect("bigger program compiles");
        assert_eq!(bigger.code, compile(
            b"int main() { int x = 0; int y = 0; printf(\"%d\", x + y); }",
        ).unwrap());
        assert!(bigger.cells > empty.cells);
        assert_eq!(
            compile_program(b"int main() {}").unwrap().cells,
            empty.cells,
            "allocation is deterministic"
        );
    }

    #[test]
    fn shadowing_uses_distinct_cells() {
        // A shadowed declaration gets its own cells: the nested `x`
        // does not overwrite the outer one, and the global `x` is a
        // third, untouched cell.
        let bf = compile(
            b"int x = 7; int main() { int x = 1; printf(\"%d,\", x); { int x = 2; printf(\"%d,\", x); } printf(\"%d\", x); }",
        )
        .expect("shadowing program compiles");
        let mut out = Vec::new();
        crate::runner::run(&bf, &mut std::io::empty(), &mut out).expect("bf runs");
        assert_eq!(out, b"1,2,1");
        let bf = compile(
            b"int main() { int a = 1; int b = 2; { int a = 3; printf(\"%d,\", a); } printf(\"%d,%d\", a, b); }",
        )
        .expect("shadowing program compiles");
        let mut out = Vec::new();
        crate::runner::run(&bf, &mut std::io::empty(), &mut out).expect("bf runs");
        assert_eq!(out, b"3,1,2");
    }

    #[test]
    fn for_decl_is_scope_renamed() {
        let bf = compile(
            b"int main() { int s = 0; for (int i = 0; i < 3; i++) { s = s + i; } printf(\"%d\", s); }",
        )
        .expect("for-decl program compiles");
        let mut out = Vec::new();
        crate::runner::run(&bf, &mut std::io::empty(), &mut out).expect("bf runs");
        assert_eq!(out, b"3");
    }

    #[test]
    fn uncalled_function_is_registered_not_emitted() {
        // The retired `non_main.c` fixture: a lone definition is
        // valid and emits nothing without `main`.
        let program =
            compile_program(b"int f() { return 0; }").expect("lone definition compiles");
        assert_eq!(program.code, b"\n");
        // With `main`, the uncalled body contributes no output.
        let bf = compile(
            b"int f() { printf(\"x\"); } int main() { printf(\"ok\"); }",
        )
        .expect("uncalled-function program compiles");
        let mut out = Vec::new();
        crate::runner::run(&bf, &mut std::io::empty(), &mut out).expect("bf runs");
        assert_eq!(out, b"ok");
    }

    #[test]
    fn duplicate_function_definition_rejected() {
        let err = compile(b"int f() {} int f() {} int main() {}").expect_err("duplicate def");
        assert!(
            err.msg.contains("duplicate function definition"),
            "unexpected error: {}",
            err.msg
        );
    }

    #[test]
    fn prototypes_allow_calls_and_ignore_names() {
        // A prototype before the definition, with a different
        // parameter name and an unnamed/void/spelling variants,
        // plus a redundant prototype after a definition. A
        // prototype alone, never called, emits nothing.
        let bf = compile(
            b"int dbl(int v); void greet(void); int add(int, int); int dbl(int x) { return x * 2; } int add(int a, int b) { return a + b; } int add(int, int); void greet(void) { printf(\"hi \"); } int main() { printf(\"%d,\", dbl(21)); greet(); printf(\"%d\", add(3, 4)); }",
        )
        .expect("prototype program compiles");
        let mut out = Vec::new();
        crate::runner::run(&bf, &mut std::io::empty(), &mut out).expect("bf runs");
        assert_eq!(out, b"42,hi 7");
        let bf = compile(
            b"int f(unsigned int); int f(int a) { return a + 1; } int main() { printf(\"%d\", f(1)); }",
        )
        .expect("spelling-variant prototype compiles");
        let mut out = Vec::new();
        crate::runner::run(&bf, &mut std::io::empty(), &mut out).expect("bf runs");
        assert_eq!(out, b"2");
        let bf = compile(
            b"int f(int); int main() { printf(\"ok\"); }",
        )
        .expect("uncalled prototype compiles");
        let mut out = Vec::new();
        crate::runner::run(&bf, &mut std::io::empty(), &mut out).expect("bf runs");
        assert_eq!(out, b"ok");
        let bf = compile(
            b"int main(); int main() { printf(\"ok\"); }",
        )
        .expect("main prototype compiles");
        let mut out = Vec::new();
        crate::runner::run(&bf, &mut std::io::empty(), &mut out).expect("bf runs");
        assert_eq!(out, b"ok");
    }

    #[test]
    fn prototype_signature_conflicts_rejected() {
        let cases: &[(&[u8], &str)] = &[
            (
                b"int f(int); int f(int a, int b) { return 0; } int main() {}",
                "conflicting prototype for function 'f'",
            ),
            (
                b"void f(); int f() { return 0; } int main() {}",
                "conflicting prototype for function 'f'",
            ),
            (
                b"int f(int); int f(char *p) { return 0; } int main() {}",
                "conflicting prototype for function 'f'",
            ),
            (
                b"int f(int); int f(char s[]) { return 0; } int main() {}",
                "conflicting prototype for function 'f'",
            ),
            (
                b"int f(int); int f(int, int); int main() {}",
                "conflicting prototype for function 'f'",
            ),
            (
                b"int f(int a) { return a; } int f(int, int); int main() {}",
                "conflicting prototype for function 'f'",
            ),
            (
                b"int f(int a, int a); int main() {}",
                "duplicate parameter name 'a'",
            ),
            (
                b"int f(int); int main() { printf(\"%d\", f(1)); }",
                "call to undefined function 'f'",
            ),
            (
                b"int f(int); int main() { f(1, 2); }",
                "wrong number of arguments to function 'f'",
            ),
        ];
        for (src, want) in cases {
            assert_eq!(compile(src).unwrap_err().msg, *want, "for {:?}", src);
        }
    }

    #[test]
    fn typedefs_compile_and_run() {
        let bf = compile(
            b"typedef int MyInt; typedef MyInt Alias; typedef unsigned int U; \
              typedef int *PInt; typedef char Arr[4]; \
              MyInt twice(MyInt x) { return x + x; } \
              int main() { \
                MyInt a = 21; Alias c = 1; U b = 3; \
                PInt p = &a; Arr s = \"hi\"; \
                *p = twice(a); \
                printf(\"%d,%d,%s,%d\", a + b, *p, s, c); \
              }",
        )
        .expect("typedef program compiles");
        let mut out = Vec::new();
        crate::runner::run(&bf, &mut std::io::empty(), &mut out).expect("bf runs");
        assert_eq!(out, b"45,42,hi,1");
    }

    #[test]
    fn typedef_prototype_signatures() {
        // A prototype spelled through aliases agrees with a
        // definition spelled directly, and vice versa.
        let bf = compile(
            b"typedef int MyInt; MyInt f(MyInt); \
              int f(int x) { return x; } \
              int main() { printf(\"%d\", f(3)); }",
        )
        .expect("alias prototype compiles");
        let mut out = Vec::new();
        crate::runner::run(&bf, &mut std::io::empty(), &mut out).expect("bf runs");
        assert_eq!(out, b"3");
        // A pointer return spelled through an alias conflicts
        // with a scalar definition.
        let err = compile(
            b"typedef int *P; P f(int); int f(int x) { return x; } int main() {}",
        )
        .expect_err("pointer-return conflict");
        assert_eq!(err.msg, "conflicting prototype for function 'f'");
    }

    #[test]
    fn typedef_pointer_return_runs() {
        // A pointer return spelled through an alias is usable:
        // the callee returns an address byte, the caller derefs it.
        let bf = compile(
            b"typedef int *P; P get(int *q) { return q; } \
              int main() { int x = 65; printf(\"%d\", *get(&x)); }",
        )
        .expect("pointer-return program compiles");
        let mut out = Vec::new();
        crate::runner::run(&bf, &mut std::io::empty(), &mut out).expect("bf runs");
        assert_eq!(out, b"65");
    }

    #[test]
    fn enums_compile_and_run() {
        let bf = compile(
            b"enum Color { RED, GREEN = 5, BLUE }; \
              enum { A = 'a', B = A + 1 }; \
              enum State { IDLE, RUNNING }; \
              enum State state = RUNNING; \
              int main() { \
                enum Color c = BLUE; \
                int x = A; \
                printf(\"%d,%d,%d,%d,%d\", RED, GREEN, BLUE, x, state); \
                switch (c) { \
                  case RED: printf(\"R\"); break; \
                  case BLUE: printf(\"B\"); break; \
                  default: printf(\"?\"); break; \
                } \
              }",
        )
        .expect("enum program compiles");
        let mut out = Vec::new();
        crate::runner::run(&bf, &mut std::io::empty(), &mut out).expect("bf runs");
        assert_eq!(out, b"0,5,6,97,1B");
    }

    #[test]
    fn enum_negative_and_overflow_wrap() {
        // Enumerator values are bytes like every value here: a
        // negative value wraps to 255 and 300 wraps to 44,
        // unlike the g++ oracle, so this is pinned by a unit
        // test, not a fixture.
        let bf = compile(b"enum { NEG = -1, BIG = 300 }; int main() { printf(\"%d,%d\", NEG, BIG); }")
            .expect("enum program compiles");
        let mut out = Vec::new();
        crate::runner::run(&bf, &mut std::io::empty(), &mut out).expect("bf runs");
        assert_eq!(out, b"255,44");
    }

    #[test]
    fn enum_shadowing_uses_declared_name() {
        // A nested block's RED shadows the enumerator while the
        // block lasts; after it, the enumerator is visible again.
        let bf = compile(
            b"enum { RED }; int main() { \
                { int RED = 9; printf(\"%d,\", RED); } \
                printf(\"%d\", RED); \
              }",
        )
        .expect("shadowing program compiles");
        let mut out = Vec::new();
        crate::runner::run(&bf, &mut std::io::empty(), &mut out).expect("bf runs");
        assert_eq!(out, b"9,0");
    }

    #[test]
    fn typedef_errors_rejected() {
        let cases: &[(&[u8], &str)] = &[
            (
                b"typedef int T; typedef char T; int main() {}",
                "conflicting typedef for 'T'",
            ),
            (
                b"int main() { typedef int T; return 0; }",
                "only allowed at file scope",
            ),
        ];
        for (src, want) in cases {
            let err = compile(src).expect_err("typedef error");
            assert!(
                err.msg.contains(want),
                "for {:?}: got {:?}, want {:?}",
                src,
                err.msg,
                want
            );
        }
    }

    #[test]
    fn duplicate_parameter_name_rejected() {
        let err = compile(b"int f(int x, int x) { return x; } int main() { return 0; }")
            .expect_err("duplicate param");
        assert!(
            err.msg.contains("duplicate parameter name"),
            "unexpected error: {}",
            err.msg
        );
    }

    #[test]
    fn inline_two_calls_identity() {
        // Sequential calls rebind the parameter cell; a call to a
        // function defined after `main` works (forward reference).
        let bf = compile(
            b"int main() { printf(\"%d,%d\", id(3), id(4)); } int id(int x) { return x; }",
        )
        .expect("forward-call program compiles");
        let mut out = Vec::new();
        crate::runner::run(&bf, &mut std::io::empty(), &mut out).expect("bf runs");
        assert_eq!(out, b"3,4");
    }

    #[test]
    fn inline_value_call_nested() {
        let bf = compile(
            b"int add(int a, int b) { return a + b; } int quad(int x) { int y = dbl(x); return dbl(y); } int dbl(int x) { return x * 2; } int main() { printf(\"%d,%d\", add(add(1, 2), add(3, 4)), quad(3)); }",
        )
        .expect("nested-call program compiles");
        let mut out = Vec::new();
        crate::runner::run(&bf, &mut std::io::empty(), &mut out).expect("bf runs");
        assert_eq!(out, b"10,12");
    }

    #[test]
    fn inline_invalid_args() {
        // Calls sit in compiled positions: a call in `main`'s
        // `return` operand is never compiled (`return` in `main` is
        // a no-op), so arity and recursion errors there would not
        // fire. Errors are lazy in the same way: an uncalled
        // recursive function compiles.
        for (src, want) in [
            (
                b"int f(int a) { return a; } int main() { int x = f(1, 2); printf(\"%d\", x); }".as_slice(),
                "wrong number of arguments to function 'f'",
            ),
            (
                b"int f(int a, int b) { return a; } int main() { int x = f(1); printf(\"%d\", x); }".as_slice(),
                "wrong number of arguments to function 'f'",
            ),
            (
                b"int f(int a) { return f(a); } int main() { int x = f(1); printf(\"%d\", x); }".as_slice(),
                "recursive call to function 'f' is not supported",
            ),
            (
                b"int f(int a) { return g(a); } int g(int a) { return f(a); } int main() { int x = f(1); printf(\"%d\", x); }".as_slice(),
                "recursive call to function 'f' is not supported",
            ),
        ] {
            assert_eq!(compile(src).unwrap_err().msg, want, "for {:?}", src);
        }
        // Uncalled, the recursive function compiles.
        compile(b"int f(int a) { return f(a); } int main() { printf(\"ok\"); }")
            .expect("uncalled recursive function compiles");
    }

    #[test]
    fn return_in_loop_exits_and_falls_through() {
        // Without the loop-condition gate the second program would
        // run the loop to completion and return 99 instead of 2.
        let bf = compile(
            b"int first(int n) { int i = 0; while (i < 10) { if (i == n) { return i * 2; } i++; } return 99; } int main() { printf(\"%d,%d\", first(3), first(20)); }",
        )
        .expect("return-in-loop program compiles");
        let mut out = Vec::new();
        crate::runner::run(&bf, &mut std::io::empty(), &mut out).expect("bf runs");
        assert_eq!(out, b"6,99");
        let bf = compile(
            b"int f() { int i = 0; int c = 0; while (i < 3) { c = c + 1; if (c == 2) { return c; } i = i + 1; } return 99; } int main() { printf(\"%d\", f()); }",
        )
        .expect("re-entry program compiles");
        let mut out = Vec::new();
        crate::runner::run(&bf, &mut std::io::empty(), &mut out).expect("bf runs");
        assert_eq!(out, b"2");
    }

    #[test]
    fn return_in_switch_cuts_later_cases() {
        let bf = compile(
            b"int pick(int s) { switch (s) { case 1: return 10; case 2: return 20; default: return 30; } } int main() { printf(\"%d,%d,%d\", pick(1), pick(2), pick(9)); }",
        )
        .expect("return-in-switch program compiles");
        let mut out = Vec::new();
        crate::runner::run(&bf, &mut std::io::empty(), &mut out).expect("bf runs");
        assert_eq!(out, b"10,20,30");
    }

    #[test]
    fn return_skips_following_statements() {
        let bf = compile(
            b"int f(int x) { if (x) { return 1; } printf(\"BAD\"); return 2; } int main() { printf(\"%d,\", f(1)); printf(\"%d\", f(0)); }",
        )
        .expect("return-guard program compiles");
        let mut out = Vec::new();
        crate::runner::run(&bf, &mut std::io::empty(), &mut out).expect("bf runs");
        assert_eq!(out, b"1,BAD2");
    }

    #[test]
    fn void_calls_and_bare_return() {
        let bf = compile(
            b"void greet() { printf(\"hi\"); return; } int main() { greet(); printf(\"!\"); }",
        )
        .expect("void program compiles");
        let mut out = Vec::new();
        crate::runner::run(&bf, &mut std::io::empty(), &mut out).expect("bf runs");
        assert_eq!(out, b"hi!");
        // A void call used as a value yields zero; a value returned
        // from a void function is ignored.
        let bf = compile(
            b"void greet() { printf(\"hi\"); return 5; } int main() { printf(\"%d,\", greet()); printf(\"done\"); }",
        )
        .expect("void-value program compiles");
        let mut out = Vec::new();
        crate::runner::run(&bf, &mut std::io::empty(), &mut out).expect("bf runs");
        assert_eq!(out, b"hi0,done");
    }

    #[test]
    fn pointer_and_array_params() {
        let bf = compile(
            b"int bump(int *p) { *p = *p + 1; return *p; } int main() { int x = 5; int b = bump(&x); printf(\"%d,%d\", b, x); }",
        )
        .expect("pointer-param program compiles");
        let mut out = Vec::new();
        crate::runner::run(&bf, &mut std::io::empty(), &mut out).expect("bf runs");
        assert_eq!(out, b"6,6");
        let bf = compile(
            b"int sum(int a[], int n) { int s = 0; int i = 0; while (i < n) { s = s + a[i]; i++; } return s; } int main() { int arr[4]; arr[0] = 1; arr[1] = 2; arr[2] = 3; arr[3] = 4; printf(\"%d\", sum(arr, 4)); }",
        )
        .expect("array-param program compiles");
        let mut out = Vec::new();
        crate::runner::run(&bf, &mut std::io::empty(), &mut out).expect("bf runs");
        assert_eq!(out, b"10");
    }

    #[test]
    fn arguments_evaluate_before_binding() {
        // Arguments evaluate left-to-right into temporaries before
        // any parameter is bound, so an argument that assigns a
        // variable cannot leak into an earlier parameter.
        let bf = compile(
            b"int f(int a, int b) { return a; } int main() { int x = 7; int r = f(x, x = 9); printf(\"%d,%d\", r, x); }",
        )
        .expect("aliasing program compiles");
        let mut out = Vec::new();
        crate::runner::run(&bf, &mut std::io::empty(), &mut out).expect("bf runs");
        assert_eq!(out, b"7,9");
    }

    #[test]
    fn body_top_level_may_not_redefine_parameter() {
        // Parameters share the body's outermost scope: a top-level
        // declaration reusing a parameter's name is rejected, matching
        // g++ ("shadows a parameter") and gcc ("redeclared as
        // different kind of symbol", both 15.2, probed via CI). The
        // check scans every top-level declaration, so using the
        // parameter first does not save the later redefinition, and
        // it fires whether or not the function is called.
        for src in [
            b"int f(int x) { int x = 5; return x; } int main() { int y = f(9); }".as_slice(),
            b"int f(int x) { int x; return x; } int main() { int y = f(9); }".as_slice(),
            b"int f(int x) { int x = 5, y = 1; return x + y; } int main() { int y = f(9); }".as_slice(),
            b"int f(int x) { int y = x; int x = 2; return y; } int main() { int y = f(9); }".as_slice(),
            b"int f(int x) { char x[2]; return 0; } int main() { int y = f(9); }".as_slice(),
            b"int main(int x) { int x = 5; }".as_slice(),
            b"int f(int x) { int x = 5; return x; } int main() {}".as_slice(),
        ] {
            assert_eq!(
                compile(src).unwrap_err().msg,
                "redefinition of parameter 'x'",
                "for {:?}",
                src
            );
        }
    }

    #[test]
    fn same_scope_redeclaration_rejected() {
        // Redeclaring a name in the same block is an error, matching
        // g++ ("redeclaration"/"conflicting declaration") and gcc
        // ("redefinition"/"conflicting types", both 15.2, probed via
        // CI) in every non-array combination, including within one
        // declarator list and in a function that is never called
        // (renaming runs for every definition).
        for (src, want) in [
            (
                b"int f() { int x = 1; int x = 2; return x; } int main() { printf(\"%d\", f()); }".as_slice(),
                "redefinition of 'x'",
            ),
            (
                b"int f() { int x = 1, x = 2; return x; } int main() { printf(\"%d\", f()); }".as_slice(),
                "redefinition of 'x'",
            ),
            (
                b"int f() { char *p = 0; char *p = 0; return 0; } int main() { printf(\"%d\", f()); }".as_slice(),
                "redefinition of 'p'",
            ),
            (
                b"int f() { int x = 1; char *x = 0; return 0; } int main() { printf(\"%d\", f()); }".as_slice(),
                "redefinition of 'x'",
            ),
            (
                b"int f() { char *p = 0; int p = 1; return 0; } int main() { printf(\"%d\", f()); }".as_slice(),
                "redefinition of 'p'",
            ),
            (
                b"int f() { int x = 1; int x = 2; return x; } int main() {}".as_slice(),
                "redefinition of 'x'",
            ),
        ] {
            assert_eq!(compile(src).unwrap_err().msg, want, "for {:?}", src);
        }
        // Array-involved redeclarations keep the emitter's messages.
        for (src, want) in [
            (
                b"int main() { char a[2]; char a[3]; }".as_slice(),
                "array size mismatch on redeclaration",
            ),
            (
                b"int main() { char x = 1; char x[2]; }".as_slice(),
                "cannot redeclare a variable as an array",
            ),
            (
                b"int main() { char a[2]; char a = 1; }".as_slice(),
                "cannot redeclare an array as a variable",
            ),
            (
                b"int main() { char *p = 0; char p[2]; }".as_slice(),
                "cannot redeclare a variable as an array",
            ),
            (
                b"int main() { char a[2]; char *a = 0; }".as_slice(),
                "cannot redeclare an array as a variable",
            ),
        ] {
            assert_eq!(compile(src).unwrap_err().msg, want, "for {:?}", src);
        }
    }

    #[test]
    fn global_redefinition_rejected() {
        // Duplicate initialized definitions at file scope are
        // redefinitions: both compilers reject them (the probe
        // cases g1–g4). A bare declarator without an initializer is
        // not a definition here and is skipped by the check, so its
        // existing "Unsupported init declarator" error is preserved.
        for (src, want) in [
            (
                b"int x = 1; int x = 2; int main() { printf(\"%d\", x); }".as_slice(),
                "redefinition of 'x'",
            ),
            (
                b"int x = 1, x = 2; int main() { printf(\"%d\", x); }".as_slice(),
                "redefinition of 'x'",
            ),
            (
                b"char *p = 0; char *p = 0; int main() { printf(\"%d\", *p); }".as_slice(),
                "redefinition of 'p'",
            ),
            (
                b"int x = 1; char *x = 0; int main() { return 0; }".as_slice(),
                "redefinition of 'x'",
            ),
            (
                b"char *p = 0; int p = 1; int main() { return 0; }".as_slice(),
                "redefinition of 'p'",
            ),
            (
                b"int x; int x = 1; int main() { printf(\"%d\", x); }".as_slice(),
                "Unsupported init declarator",
            ),
        ] {
            assert_eq!(compile(src).unwrap_err().msg, want, "for {:?}", src);
        }
        // Array-involved file-scope redeclarations keep the
        // emitter's behavior.
        let bf = compile(b"char g[2]; char g[2]; int main() { printf(\"%d\", g[0]); }")
            .expect("same-size global array redeclares");
        let mut out = Vec::new();
        crate::runner::run(&bf, &mut std::io::empty(), &mut out).expect("bf runs");
        assert_eq!(out, b"0");
        for (src, want) in [
            (
                b"char g[2]; char g[3]; int main() { return 0; }".as_slice(),
                "array size mismatch on redeclaration",
            ),
            (
                b"int x = 1; char x[2]; int main() { return 0; }".as_slice(),
                "cannot redeclare a variable as an array",
            ),
            (
                b"char a[2]; int a = 1; int main() { return 0; }".as_slice(),
                "cannot redeclare an array as a variable",
            ),
        ] {
            assert_eq!(compile(src).unwrap_err().msg, want, "for {:?}", src);
        }
    }

    #[test]
    fn nested_blocks_may_shadow() {
        // Shadowing in a nested block or a `for` initializer stays
        // legal: only the same block is closed to redeclaration.
        let bf = compile(
            b"int f() { int x = 1; { int x = 2; } return x; } int main() { printf(\"%d\", f()); }",
        )
        .expect("nested-shadow program compiles");
        let mut out = Vec::new();
        crate::runner::run(&bf, &mut std::io::empty(), &mut out).expect("bf runs");
        assert_eq!(out, b"1");
        let bf = compile(
            b"int f() { int x = 1; for (int x = 0; x < 1; x++) {} return x; } int main() { printf(\"%d\", f()); }",
        )
        .expect("for-shadow program compiles");
        let mut out = Vec::new();
        crate::runner::run(&bf, &mut std::io::empty(), &mut out).expect("bf runs");
        assert_eq!(out, b"1");
    }

    #[test]
    fn nested_shadowing_of_parameters_is_legal() {
        // A parameter may still be shadowed inside a nested block or
        // a `for` initializer, and a global may be shadowed at the
        // top level: only the parameter's own outermost scope is
        // closed to redeclaration.
        let bf = compile(
            b"int f(int x) { int y = 0; { int x = 5; y = x; } return y + x; } int main() { printf(\"%d\", f(9)); }",
        )
        .expect("nested-shadow program compiles");
        let mut out = Vec::new();
        crate::runner::run(&bf, &mut std::io::empty(), &mut out).expect("bf runs");
        assert_eq!(out, b"14");
        let bf = compile(
            b"int f(int x) { for (int x = 0; x < 2; x++) {} return x; } int main() { printf(\"%d\", f(9)); }",
        )
        .expect("for-shadow program compiles");
        let mut out = Vec::new();
        crate::runner::run(&bf, &mut std::io::empty(), &mut out).expect("bf runs");
        assert_eq!(out, b"9");
        let bf = compile(
            b"int x = 7; int f() { int x = 5; return x; } int main() { printf(\"%d,%d\", f(), x); }",
        )
        .expect("global-shadow program compiles");
        let mut out = Vec::new();
        crate::runner::run(&bf, &mut std::io::empty(), &mut out).expect("bf runs");
        assert_eq!(out, b"5,7");
    }

    #[test]
    fn expansions_reclaim_temporaries() {
        // Forty sequential expansions must not push a later variable
        // past the 256 addressable cells: the address-of below would
        // fail to compile if expansion temporaries were never freed.
        let mut src =
            b"int id(int x) { return x; } int bump(int *p) { *p = *p + 1; return *p; } int main() { int i = 0; "
                .to_vec();
        for _ in 0..40 {
            src.extend_from_slice(b"i = id(i + 1); ");
        }
        src.extend_from_slice(b"int x = 5; int b = bump(&x); printf(\"%d,%d,%d\", i, b, x); }");
        let bf = compile(&src).expect("many-call program compiles");
        let mut out = Vec::new();
        crate::runner::run(&bf, &mut std::io::empty(), &mut out).expect("bf runs");
        assert_eq!(out, b"40,6,6");
    }

    #[test]
    fn variables_survive_temp_reclaim() {
        // A variable declared inside an if branch keeps its cell
        // when later code allocates temporaries in the reclaimed
        // range around it.
        let bf = compile(
            b"int main() { int x = 0; if (1) { int y = 7; x = y; } printf(\"%d\", x); }",
        )
        .expect("branch-var program compiles");
        let mut out = Vec::new();
        crate::runner::run(&bf, &mut std::io::empty(), &mut out).expect("bf runs");
        assert_eq!(out, b"7");
        let bf = compile(
            b"int main() { if (1) { int y = 7; printf(\"%d\", y); } printf(\"%d\", 3); }",
        )
        .expect("branch-var program compiles");
        let mut out = Vec::new();
        crate::runner::run(&bf, &mut std::io::empty(), &mut out).expect("bf runs");
        assert_eq!(out, b"73");
    }

    #[test]
    fn sizeof_is_one_and_unevaluated() {
        // sizeof(x + 1) is 1 here although C says 4 (int type); the
        // operand is never compiled, so x++ has no side effect.
        let bf = compile(
            b"int main() { int x = 0; printf(\"%d\", sizeof(char) + sizeof x + sizeof(x + 1)); x++; printf(\"%d\", x); printf(\"%d\", sizeof(x++)); printf(\"%d\", x); }",
        )
        .expect("sizeof program compiles");
        let mut out = Vec::new();
        crate::runner::run(&bf, &mut std::io::empty(), &mut out).expect("bf runs");
        assert_eq!(out, b"3111");
    }

    #[test]
    fn constant_index_arrays_read_write() {
        let bf = compile(
            b"int main() { char a[4]; a[0] = 65; a[1] = 66; a[2] = a[0] + 1; a[1]++; a[2] += 2; a[(3)] = 68; printf(\"%c%c%d%c\", a[0], a[1], a[2], a[3]); printf(\"%d\", sizeof(a)); }",
        )
        .expect("array program compiles");
        let mut out = Vec::new();
        crate::runner::run(&bf, &mut std::io::empty(), &mut out).expect("bf runs");
        assert_eq!(out, b"AC68D4");
        // A character literal is a constant index too (same raw-slice
        // value rule as a character in value position).
        let bf = compile(
            b"int main() { char a[121]; a['x'] = 7; printf(\"%d\", a['x']); }",
        )
        .expect("char-index program compiles");
        let mut out = Vec::new();
        crate::runner::run(&bf, &mut std::io::empty(), &mut out).expect("bf runs");
        assert_eq!(out, b"7");
    }

    #[test]
    fn array_elements_start_zero() {
        let bf = compile(b"int main() { char a[3]; printf(\"%d%d%d\", a[0], a[1], a[2]); }")
            .expect("array program compiles");
        let mut out = Vec::new();
        crate::runner::run(&bf, &mut std::io::empty(), &mut out).expect("bf runs");
        assert_eq!(out, b"000");
        // Redeclaring the same array with the same size re-clears it.
        let bf = compile(
            b"int main() { char a[2]; a[0] = 9; char a[2]; printf(\"%d\", a[0]); }",
        )
        .expect("redeclared array compiles");
        let mut out = Vec::new();
        crate::runner::run(&bf, &mut std::io::empty(), &mut out).expect("bf runs");
        assert_eq!(out, b"0");
        // Elements declared after a reclaimed branch still start at
        // zero even if their cells reuse reclaimed indices.
        let bf = compile(
            b"int main() { if (1) { int y = 200; printf(\"%d\", y); } char a[2]; printf(\"%d\", a[0]); }",
        )
        .expect("array after branch compiles");
        let mut out = Vec::new();
        crate::runner::run(&bf, &mut std::io::empty(), &mut out).expect("bf runs");
        assert_eq!(out, b"2000");
    }

    #[test]
    fn array_errors() {
        let cases: &[(&[u8], &str)] = &[
            (
                b"int main() { char a[3]; a[3] = 1; }",
                "array index out of bounds",
            ),
            (
                b"int main() { char a[3]; a[-1] = 1; }",
                "array index out of bounds",
            ),
            (
                b"int main() { char x = 1; printf(\"%d\", x[0]); }",
                "subscript of non-array",
            ),
            (
                b"int main() { b[0] = 1; }",
                "subscript of undeclared array",
            ),
            (
                b"int main() { char a[2]; printf(\"%d\", a[0][1]); }",
                "multi-dimensional arrays are not supported",
            ),
            (b"int main() { char a[0]; }", "array size must be positive"),
            (
                b"int main() { int i = 2; char a[i]; }",
                "array size must be an integer literal",
            ),
            (
                b"int main() { char a[2]; char a[3]; }",
                "array size mismatch on redeclaration",
            ),
            (
                b"int main() { char x = 1; char x[2]; }",
                "cannot redeclare a variable as an array",
            ),
            (
                b"int main() { char a[2]; char a = 1; }",
                "cannot redeclare an array as a variable",
            ),
            (
                b"int main() { char a[2] = 5; }",
                "array initializers are not supported",
            ),
        ];
        for (src, want) in cases {
            assert_eq!(compile(src).unwrap_err().msg, *want, "for {:?}", src);
        }
    }

    #[test]
    fn pointers_load_store_and_arithmetic() {
        let bf = compile(
            b"int main() { char a[5]; char *p = 0; char *q = 0; int i = 0; a[0] = 10; a[1] = 20; a[2] = 30; a[3] = 40; a[4] = 50; p = &a[0]; printf(\"%d,\", *p); p++; printf(\"%d,\", *p); *p = 25; printf(\"%d,\", a[1]); p = p + 2; printf(\"%d,\", *p); printf(\"%d,\", p[0]); printf(\"%d,\", p[1]); i = 3; printf(\"%d,\", a[i]); a[i] = 44; printf(\"%d,\", a[3]); p = a; printf(\"%d,\", *p); p = &a[4]; printf(\"%d,\", *p); q = &a[2]; printf(\"%d,\", p == q); printf(\"%d,\", p > q); printf(\"%d,\", p - q); q = &a[1]; char *s2 = 0; s2 = &q; printf(\"%d,\", *(*s2)); }",
        )
        .expect("pointer program compiles");
        let mut out = Vec::new();
        crate::runner::run(&bf, &mut std::io::empty(), &mut out).expect("bf runs");
        assert_eq!(out, b"10,20,25,40,40,50,40,44,10,50,0,1,2,25,");
    }

    #[test]
    fn pointers_deref_store_forms() {
        // Prefix/postfix inc/dec and compound assignment through a
        // dereference, plus *(p++) with its defined order: the store
        // lands at the old address and p still advances.
        let bf = compile(
            b"int main() { char a[3]; char *p = 0; a[0] = 1; a[1] = 2; a[2] = 3; p = &a[0]; (*p)++; printf(\"%d,\", a[0]); ++*p; printf(\"%d,\", a[0]); *p += 10; printf(\"%d,\", a[0]); (*p)--; printf(\"%d,\", a[0]); *p++ = 99; printf(\"%d,%d,\", a[0], *p); printf(\"%d\", a[1]); }",
        )
        .expect("deref store program compiles");
        let mut out = Vec::new();
        crate::runner::run(&bf, &mut std::io::empty(), &mut out).expect("bf runs");
        assert_eq!(out, b"2,3,13,12,99,2,2");
    }

    #[test]
    fn scanf_through_pointers_and_element_addresses() {
        let bf = compile(
            b"int main() { char a[3]; char *p = 0; int i = 0; p = &a[0]; scanf(\"%d\", p); scanf(\"%d\", &a[2]); i = 1; scanf(\"%d\", &a[i]); scanf(\"%d\", &a[1]); printf(\"%d,%d,%d\", a[0], a[1], a[2]); }",
        )
        .expect("scanf pointer program compiles");
        let mut out = Vec::new();
        let mut input = std::io::Cursor::new(b"7 8 9 6\n");
        crate::runner::run(&bf, &mut input, &mut out).expect("bf runs");
        assert_eq!(out, b"7,6,8");
    }

    #[test]
    fn division_by_zero_is_defined() {
        // Previously `x / 0` spun forever: the loop tested `a >= 0`.
        // Now the quotient is 0 and the remainder keeps `a == q*b+r`,
        // i.e. `a % 0 == a`. C leaves both undefined and g++ traps,
        // so these are pinned by unit tests, not by the oracle.
        let bf = compile(
            b"int main() { printf(\"%d,%d,%d,%d,%d,%d\", 7 / 0, 7 % 0, 0 / 0, 0 % 0, 200 / 0, 200 % 0); }",
        )
        .expect("div-zero program compiles");
        let mut out = Vec::new();
        crate::runner::run(&bf, &mut std::io::empty(), &mut out).expect("bf runs");
        assert_eq!(out, b"0,7,0,0,0,200");
        let bf = compile(
            b"int main() { int d = 0; int a = 17; printf(\"%d,%d,\", a / d, a % d); d = 3; printf(\"%d,%d,\", a / d, a % d); a /= d; printf(\"%d\", a); }",
        )
        .expect("variable-divisor program compiles");
        let mut out = Vec::new();
        crate::runner::run(&bf, &mut std::io::empty(), &mut out).expect("bf runs");
        assert_eq!(out, b"0,17,5,2,5");
    }

    #[test]
    fn scanf_leading_sign_is_honored() {
        // A '-' before the first digit negates (wrapping, so "-5"
        // reads as 251); '+' and leading whitespace are ignored, as
        // before; the wrap-back value agrees with C int arithmetic.
        let bf = compile(
            b"int main() { int x = 0; scanf(\"%d\", &x); printf(\"%d,\", x); scanf(\"%d\", &x); printf(\"%d,\", x); scanf(\"%d\", &x); printf(\"%d,\", x + 5); scanf(\"%d\", &x); printf(\"%d\", x); }",
        )
        .expect("signed scanf program compiles");
        let mut out = Vec::new();
        let mut input = std::io::Cursor::new(b"-5 +7   -12 -0\n");
        crate::runner::run(&bf, &mut input, &mut out).expect("bf runs");
        assert_eq!(out, b"251,7,249,0");
        let bf = compile(
            b"int main() { int x = 0; char *p = 0; p = &x; scanf(\"%d\", p); printf(\"%d\", x); }",
        )
        .expect("signed scanf through pointer compiles");
        let mut out = Vec::new();
        let mut input = std::io::Cursor::new(b"-7\n");
        crate::runner::run(&bf, &mut input, &mut out).expect("bf runs");
        assert_eq!(out, b"249");
    }

    #[test]
    fn scanf_string_truncates_and_keeps_eof_contents() {
        // Over-long input truncates to len-1 characters plus NUL,
        // the rest of the token is discarded, and end of input
        // before any token character leaves the array untouched.
        let bf = compile(
            b"int main() { char s[4]; scanf(\"%s\", s); printf(\"[%s]\", s); scanf(\"%s\", s); printf(\"[%s]\", s); char u[6] = \"keep\"; scanf(\"%s\", u); printf(\"[%s]\", u); }",
        )
        .expect("scanf string program compiles");
        let mut out = Vec::new();
        let mut input = std::io::Cursor::new(b"hello\nbye\n");
        crate::runner::run(&bf, &mut input, &mut out).expect("bf runs");
        assert_eq!(out, b"[hel][bye][keep]");
        // A one-cell array holds only the NUL.
        let bf = compile(
            b"int main() { char w[1]; scanf(\"%s\", w); printf(\"[%s]\", w); }",
        )
        .expect("one-cell scanf string program compiles");
        let mut out = Vec::new();
        let mut input = std::io::Cursor::new(b"xyz\n");
        crate::runner::run(&bf, &mut input, &mut out).expect("bf runs");
        assert_eq!(out, b"[]");
    }

    #[test]
    fn scanf_string_through_pointers_and_element_addresses() {
        // Through a pointer, through a constant element address
        // (bounded by the remaining length), and through a
        // variable element address (unbounded, like a pointer).
        let bf = compile(
            b"int main() { char a[8]; char *p = 0; int i = 0; p = &a[0]; scanf(\"%s\", p); scanf(\"%s\", &a[2]); i = 1; scanf(\"%s\", &a[i]); printf(\"[%s|%s]\", a, &a[2]); }",
        )
        .expect("scanf string pointer program compiles");
        let mut out = Vec::new();
        let mut input = std::io::Cursor::new(b"hi\nworld\nyo\n");
        crate::runner::run(&bf, &mut input, &mut out).expect("bf runs");
        assert_eq!(out, b"[hyo|o]");
    }

    #[test]
    fn scanf_string_consumes_its_delimiter() {
        // Defined deviation from C (like `%d` here, whose
        // terminating character is also consumed, not pushed
        // back): `%c` after `%s` reads the character after the
        // delimiter, where C would read the delimiter itself.
        let bf = compile(
            b"int main() { char s[8]; char c = 0; scanf(\"%s%c\", s, &c); printf(\"[%s]%c\", s, c); }",
        )
        .expect("scanf string/char program compiles");
        let mut out = Vec::new();
        let mut input = std::io::Cursor::new(b"ab cd");
        crate::runner::run(&bf, &mut input, &mut out).expect("bf runs");
        assert_eq!(out, b"[ab]c");
    }

    #[test]
    fn scanf_string_errors() {
        let cases: &[(&[u8], &str)] = &[
            (
                b"int main() { scanf(\"%s\", 5); }",
                "scanf argument must be an address",
            ),
            (
                b"int main() { char a[3]; scanf(\"%s\", &a[9]); }",
                "array index out of bounds",
            ),
            (
                b"int main() { int x = 0; scanf(\"%y\", &x); }",
                "Unexpected scanf format",
            ),
            (
                b"int main() { char s[4]; scanf(\"%s\"); }",
                "Too few arguments for format string",
            ),
        ];
        for (src, want) in cases {
            assert_eq!(compile(src).unwrap_err().msg, *want, "for {:?}", src);
        }
    }

    #[test]
    fn variable_index_works_beyond_addressable_range() {
        // Array dispatch counts the offset itself down and never
        // materializes an address, so a high array (past cell 255)
        // supports variable indices even though its elements have
        // no addressable `&` form. The unit test pins both the
        // load and the store; the old base-0-only behavior hid
        // behind arrays allocated first.
        let bf = compile(
            b"int main() { char f[260]; char a[3]; int i = 0; a[0] = 5; a[1] = 6; a[2] = 7; i = 2; printf(\"%d,\", a[i]); a[i] = 9; printf(\"%d\", a[2]); }",
        )
        .expect("high array program compiles");
        let mut out = Vec::new();
        crate::runner::run(&bf, &mut std::io::empty(), &mut out).expect("bf runs");
        assert_eq!(out, b"7,9");
    }

    #[test]
    fn variable_index_out_of_range_is_noop() {
        // Out-of-range variable indices load zero and store
        // nowhere (defined fallback; C leaves it undefined, so
        // this is a unit test, not an oracle fixture).
        let bf = compile(
            b"int main() { char a[3]; int i = 0; a[0] = 5; i = 9; printf(\"%d,\", a[i]); a[i] = 7; printf(\"%d\", a[0]); }",
        )
        .expect("out-of-range program compiles");
        let mut out = Vec::new();
        crate::runner::run(&bf, &mut std::io::empty(), &mut out).expect("bf runs");
        assert_eq!(out, b"0,5");
    }

    #[test]
    fn pointer_errors() {
        let cases: &[(&[u8], &str)] = &[
            (
                b"int main() { char x = 1; printf(\"%d\", *x); }",
                "dereference of non-pointer",
            ),
            (
                b"int main() { printf(\"%d\", *b); }",
                "dereference of undeclared identifier",
            ),
            (
                b"int main() { char a[2]; char *p = 0; a = p; }",
                "cannot assign to an array",
            ),
            (
                b"int main() { char a[2]; a++; }",
                "cannot modify an array",
            ),
            (
                b"int main() { char a[2]; printf(\"%d\", &a[9]); }",
                "array index out of bounds",
            ),
            (
                b"int main() { scanf(\"%d\", 5); }",
                "scanf argument must be an address",
            ),
            (
                b"int main() { char a[300]; char x = 1; char *p = 0; p = &x; }",
                "address is outside the addressable range",
            ),
            (
                b"int main() { char f[260]; char a[3]; char *p = 0; p = &a[0]; }",
                "address is outside the addressable range",
            ),
        ];
        for (src, want) in cases {
            assert_eq!(compile(src).unwrap_err().msg, *want, "for {:?}", src);
        }
    }

    #[test]
    fn string_literals_and_percent_s() {
        let bf = compile(
            b"int main() { char s[6] = \"hello\"; char t[] = \"hi\"; char *p = \"bye\"; printf(\"[%s|%s|%s]\", s, t, p); printf(\"[%s]\", &s[1]); s[0] = 'H'; printf(\"[%s]\", s); printf(\"%d,%d,%d\", sizeof(s), sizeof(t), sizeof(\"bye\")); char e[] = \"\"; printf(\"[%s]\", e); char *q = \"a\\tb\"; printf(\"[%s]\", q); char *r = \"x\"; printf(\"%d\", *r); printf(\"[%s]\", \"lit\"); }",
        )
        .expect("string program compiles");
        let mut out = Vec::new();
        crate::runner::run(&bf, &mut std::io::empty(), &mut out).expect("bf runs");
        assert_eq!(
            out,
            b"[hello|hi|bye][ello][Hello]6,3,4[][a\tb]120[lit]"
        );
    }

    #[test]
    fn string_errors() {
        let cases: &[(&[u8], &str)] = &[
            (
                b"int main() { char s[2] = \"hello\"; }",
                "string initializer is too long",
            ),
            (
                b"int main() { char s[] = 5; }",
                "unsized array requires a string initializer",
            ),
            (
                b"int main() { char s[]; }",
                "unsized array requires a string initializer",
            ),
            (
                b"int main() { char s[3] = 5; }",
                "array initializers are not supported",
            ),
        ];
        for (src, want) in cases {
            assert_eq!(compile(src).unwrap_err().msg, *want, "for {:?}", src);
        }
    }

    #[test]
    fn casts_wrap_like_bytes() {
        let bf = compile(
            b"int main() { printf(\"%d\", (char)300); printf(\"%d\", (int)(200 + 100)); printf(\"%d\", (unsigned int)7); }",
        )
        .expect("cast program compiles");
        let mut out = Vec::new();
        crate::runner::run(&bf, &mut std::io::empty(), &mut out).expect("bf runs");
        assert_eq!(out, b"44447");
    }

    #[test]
    fn int_literal_bases() {
        assert_eq!(int_literal_value(b"42"), 42);
        assert_eq!(int_literal_value(b"0xFF"), 255);
        assert_eq!(int_literal_value(b"0xff"), 255);
        assert_eq!(int_literal_value(b"010"), 8);
        assert_eq!(int_literal_value(b"077"), 63);
        assert_eq!(int_literal_value(b"0b101"), 5);
        assert_eq!(int_literal_value(b"0B11"), 3);
        assert_eq!(int_literal_value(b"42U"), 42);
        assert_eq!(int_literal_value(b"7ll"), 7);
        assert_eq!(int_literal_value(b"0x1FUL"), 31);
        assert_eq!(int_literal_value(b"0x100"), 256);
        assert_eq!(int_literal_value(b"99999999999999999999"), i32::MAX);
        let bf = compile(b"int main() { printf(\"%d %d\", 0xff, 010); }")
            .expect("literal program compiles");
        let mut out = Vec::new();
        crate::runner::run(&bf, &mut std::io::empty(), &mut out).expect("bf runs");
        assert_eq!(out, b"255 8");
    }

    #[test]
    fn printf_percent_unsigned_hex() {
        let bf = compile(
            b"int main() { printf(\"%% %u %x %d\", 250, 255, 5); printf(\"%x\", 0); printf(\"%x\", 16); printf(\"%x\", 171); }",
        )
        .expect("printf program compiles");
        let mut out = Vec::new();
        crate::runner::run(&bf, &mut std::io::empty(), &mut out).expect("bf runs");
        assert_eq!(out, b"% 250 ff 5010ab");
    }

    #[test]
    fn unary_minus_plus_tilde_wrap() {
        let bf = compile(
            b"int main() { printf(\"%d\", -5 + 5); printf(\"%d\", ~5 + 6); printf(\"%d\", +5); printf(\"%c\", ~5); }",
        )
        .expect("unary program compiles");
        let mut out = Vec::new();
        crate::runner::run(&bf, &mut std::io::empty(), &mut out).expect("bf runs");
        assert_eq!(out, vec![b'0', b'0', b'5', 250]);
    }
}
