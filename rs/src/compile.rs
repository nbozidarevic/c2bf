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
use crate::codegen::{CompileError, Emitter, ExpandedFunction, FuncSig, NumericBase};
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

/// What a flattened subscript chain is rooted at: a named
/// array (with its dimensions), a multi-dimensional array
/// parameter (with the dimensions after the first, the
/// stride), or a pointer-valued expression whose value is an
/// address (a pointer variable, a dereference, a call, ...).
/// Planned subscript-chain offset: a static cell offset
/// when every index is constant, or the constant prefix sum
/// to fold into the emitted variable offset otherwise.
enum ChainOffsetPlan {
    Static(i32),
    Dynamic(i64),
}

enum ChainClass<'a> {
    Array {
        base: i32,
        dims: Vec<usize>,
        indices: Vec<&'a Expr>,
    },
    ParamArray {
        cell: i32,
        row_dims: Vec<usize>,
        indices: Vec<&'a Expr>,
    },
    Pointer {
        root: &'a Expr,
        indices: Vec<&'a Expr>,
    },
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

/// A `scanf` scalar conversion for `read_scanf_scalar`.
enum ScanfConv {
    Char,
    Decimal,
    Based(NumericBase),
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
            let new_is_array = !init.dims.is_empty();
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
            let mut dims = Vec::with_capacity(init.dims.len());
            for dim in &init.dims {
                dims.push(
                    dim.as_ref()
                        .map(|e| self.rename_expr(e))
                        .transpose()?,
                );
            }
            inits.push(InitDecl {
                name: fresh,
                ptr_depth: init.ptr_depth,
                dims,
                fn_sig: init.fn_sig.clone(),
                init: init.init.as_ref().map(|e| self.rename_expr(e)).transpose()?,
            });
        }
        Ok(Decl {
            spec: decl.spec.clone(),
            inits,
            is_extern: decl.is_extern,
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
            Stmt::Goto { name } => Ok(Stmt::Goto { name: name.clone() }),
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
            Expr::InitList(es) => {
                let mut out = Vec::with_capacity(es.len());
                for e in es {
                    out.push(self.rename_expr(e)?);
                }
                Ok(Expr::InitList(out))
            }
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
/// Array parameter dimensions after the first are the stride
/// and must be sized: an unsized dimension there can only come
/// from an unsized typedef alias (`typedef char S[]; f(S a[2])`),
/// since the parser rejects a bare `[]` after the first bracket.
fn check_param_dims(params: &[FnParam]) -> Result<(), CompileError> {
    for p in params {
        if p.dims.len() > 1 && p.dims[1..].iter().any(|d| d.is_none()) {
            return Err(err(
                "array parameter dimensions after the first must have a size",
            ));
        }
    }
    Ok(())
}

fn check_prototype(
    em: &Emitter,
    d: &FuncDecl,
) -> Result<FuncSig, CompileError> {
    check_param_dims(&d.params)?;
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
        returns_ptr_depth: d.returns_ptr_depth,
        params: d.params.clone(),
    };
    if let Some(def) = em.functions.get(&d.name) {
        let def_sig = FuncSig {
            is_void: def.is_void,
            returns_ptr_depth: def.returns_ptr_depth,
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
    check_param_dims(&f.params)?;
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
            ptr_depth: p.ptr_depth,
            dims: p.dims.clone(),
            fn_sig: p.fn_sig.clone(),
        });
    }
    let body = renamer.rename_block(&f.body)?;
    Ok(ExpandedFunction {
        is_void: f.is_void,
        returns_ptr_depth: f.returns_ptr_depth,
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
    // Names declared only as `extern`: candidates for the
    // undefined-reference set, minus any name defined
    // anywhere in the unit (order-independent, like a
    // linker: a definition after the use still satisfies it).
    let mut defined: HashSet<Vec<u8>> = HashSet::new();
    let mut extern_candidates: HashSet<Vec<u8>> = HashSet::new();
    for item in &tu.0 {
        match item {
            External::FuncDef(f) => {
                let expanded = rename_function(f)?;
                let def_sig = FuncSig {
                    is_void: expanded.is_void,
                    returns_ptr_depth: expanded.returns_ptr_depth,
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
                em.fn_order.push(f.name.clone());
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
                    let is_array = !init.dims.is_empty();
                    if d.is_extern && init.init.is_none() {
                        extern_candidates.insert(init.name.clone());
                        continue;
                    }
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
                    defined.insert(init.name.clone());
                }
            }
        }
    }
    em.extern_undefined = extern_candidates
        .difference(&defined)
        .cloned()
        .collect();
    for item in &tu.0 {
        match item {
            External::FuncDef(f) => {
                if f.name.as_slice() == b"main" {
                    let main_def = Rc::clone(
                        em.functions
                            .get(&f.name)
                            .expect("main is registered above"),
                    );
                    em.compile_function_body(&main_def.body)?;
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
    /// Error when `name` was declared only as `extern` and never
    /// defined in this translation unit: a use of it is an
    /// undefined reference, not a zero cell.
    fn check_extern_defined(&self, name: &[u8]) -> Result<(), CompileError> {
        if self.extern_undefined.contains(name) {
            return Err(err(format!(
                "undefined reference to '{}'",
                String::from_utf8_lossy(name)
            )));
        }
        Ok(())
    }

    fn compile_decl(&mut self, d: &Decl) -> Result<(), CompileError> {
        // `int;` — no declarators, the base visitor emits nothing.
        for init in &d.inits {
            // A function-pointer declarator holds a callee ID
            // in one cell: never a data pointer, never an
            // array, and its initializer is a designator, a
            // function pointer of the same signature, or 0.
            if let Some(sig) = &init.fn_sig {
                if self.memory.is_array(&init.name) {
                    return Err(err("cannot redeclare an array as a variable"));
                }
                let dest = self.memory.var_cell(&init.name);
                self.fnptrs.insert(init.name.clone());
                self.fnptr_sigs.insert(init.name.clone(), sig.clone());
                let e = match &init.init {
                    None => return Err(err("Unsupported init declarator")),
                    Some(e) => e,
                };
                let value = self.compile_fnptr_value(e, sig)?;
                self.copy_value(value, dest);
                continue;
            }
            // A non-defining `extern` declarator emits nothing
            // (scalar or array); a use without a definition
            // fails at the use site via `extern_undefined`.
            if d.is_extern && init.init.is_none() {
                continue;
            }
            if !init.dims.is_empty() {
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
            if init.ptr_depth > 0 {
                self.memory.mark_pointer(&init.name);
            }
            let src = self.compile_scalar_init(e)?;
            self.copy_value(src, dest);
        }
        Ok(())
    }

    /// Cell for a scalar declarator initializer (`int x = {5}`):
    /// a plain expression compiles directly; a braced list must
    /// hold exactly one element.
    fn compile_scalar_init(&mut self, e: &Expr) -> Result<i32, CompileError> {
        match e {
            Expr::InitList(elems) => {
                if elems.len() != 1 {
                    return Err(err(
                        "scalar initializer list must contain exactly one element",
                    ));
                }
                self.compile_expr(&elems[0])
            }
            _ => self.compile_expr(e),
        }
    }

    /// Compile an array declaration: reserve consecutive cells and
    /// clear them, so elements start at zero (unlike C's indeterminate
    /// values; the tape starts zeroed, but a reclaimed index could
    /// otherwise carry a stale temporary). Every size must be a
    /// positive integer literal; only the outermost dimension may
    /// be unsized, sized by its initializer. Redeclaring the same
    /// array with the same total size re-clears it.
    fn compile_array_decl(&mut self, init: &InitDecl) -> Result<(), CompileError> {
        if self.memory.has_var(&init.name) {
            return Err(err("cannot redeclare a variable as an array"));
        }
        if init.dims.is_empty() {
            return Err(err("array declarator has no dimensions"));
        }
        // Resolve the dimension list: every dimension after
        // the first must be a sized integer literal.
        let mut sized: Vec<Option<usize>> = Vec::with_capacity(init.dims.len());
        for (k, dim) in init.dims.iter().enumerate() {
            match dim {
                None => {
                    if k == 0 {
                        sized.push(None);
                    } else {
                        return Err(err(
                            "only the first array dimension may be unsized",
                        ));
                    }
                }
                Some(Expr::Int(bytes)) => {
                    let len = int_literal_value(bytes);
                    if len <= 0 {
                        return Err(err("array size must be positive"));
                    }
                    sized.push(Some(len as usize));
                }
                Some(_) => {
                    return Err(err("array size must be an integer literal"));
                }
            }
        }
        // A string initializer fills the elements (plus a NUL when
        // it fits); a braced list fills them in order, zero-filling
        // the rest; any other initializer is rejected, as before.
        // A list holding exactly one string literal is a string
        // initializer (`char a[] = {"hi"}`), sized with its NUL.
        // `None` is an unsized dimension.
        let array_unsized = sized[0].is_none();
        let mut list_init: Option<&Vec<Expr>> = None;
        let string_init = match &init.init {
            Some(Expr::StringLit { cooked, .. }) => Some(cooked.clone()),
            Some(Expr::InitList(elems)) => {
                match elems.as_slice() {
                    [Expr::StringLit { cooked, .. }] => Some(cooked.clone()),
                    _ => {
                        list_init = Some(elems);
                        None
                    }
                }
            }
            Some(_) if array_unsized => {
                return Err(err("unsized array requires a string initializer"));
            }
            Some(_) => return Err(err("array initializers are not supported")),
            None => None,
        };
        // String initializers size only one-dimensional
        // arrays; a list holding exactly one string literal is
        // a string initializer too, so both shapes are
        // rejected for more dimensions.
        if sized.len() > 1 && string_init.is_some() {
            return Err(err(
                "string initializer is not supported for multi-dimensional arrays",
            ));
        }
        let stride: usize = sized[1..]
            .iter()
            .map(|d| d.expect("sized dimension"))
            .product();
        let (dims, len) = if array_unsized {
            match &string_init {
                Some(cooked) => {
                    let len = cooked.len() + 1;
                    (vec![len], len)
                }
                None => match list_init {
                    Some(elems) => {
                        if elems.is_empty() {
                            return Err(err("array size must be positive"));
                        }
                        let outer =
                            if elems.iter().any(|e| matches!(e, Expr::InitList(_))) {
                                elems.len()
                            } else {
                                elems.len().div_ceil(stride)
                            };
                        let len = outer
                            .checked_mul(stride)
                            .ok_or_else(|| err("array is too large"))?;
                        let mut dims = vec![outer];
                        dims.extend(
                            sized[1..]
                                .iter()
                                .map(|d| d.expect("sized dimension")),
                        );
                        (dims, len)
                    }
                    None => {
                        return Err(err("unsized array requires a string initializer"));
                    }
                },
            }
        } else {
            let dims: Vec<usize> = sized
                .iter()
                .map(|d| d.expect("sized dimension"))
                .collect();
            let mut len: usize = 1;
            for d in &dims {
                len = len
                    .checked_mul(*d)
                    .ok_or_else(|| err("array is too large"))?;
            }
            if let Some(cooked) = &string_init {
                if cooked.len() > len {
                    return Err(err("string initializer is too long"));
                }
            }
            if let Some(elems) = list_init {
                if elems.len() > len {
                    return Err(err("initializer list is too long"));
                }
            }
            (dims, len)
        };
        self.memory.record_array_dims(&init.name, dims.clone());
        // Fill the elements: string bytes plus zero fill (a NUL
        // terminator when it fits), list elements in source order
        // plus zero fill, or plain clearing. Setting every element
        // covers clearing, so the string path needs no separate
        // pass. List elements evaluate left to right, as written.
        let fill = |em: &mut Self, base: i32| -> Result<(), CompileError> {
            if dims.len() > 1 {
                return match list_init {
                    Some(elems) => em.fill_list(base, &dims, elems),
                    None => {
                        for cell in base..base + len as i32 {
                            em.set_value(cell, 0);
                        }
                        Ok(())
                    }
                };
            }
            if let Some(elems) = list_init {
                for (offset, elem) in elems.iter().enumerate() {
                    if matches!(elem, Expr::InitList(_)) {
                        return Err(err(
                            "nested initializer lists are not supported",
                        ));
                    }
                    let src = em.compile_expr(elem)?;
                    em.copy_value(src, base + offset as i32);
                }
                for cell in base + elems.len() as i32..base + len as i32 {
                    em.set_value(cell, 0);
                }
                return Ok(());
            }
            for cell in base..base + len as i32 {
                let offset = (cell - base) as usize;
                let value = string_init
                    .as_ref()
                    .and_then(|cooked| cooked.get(offset).copied())
                    .unwrap_or(0);
                em.set_value(cell, value);
            }
            Ok(())
        };
        if let Some((base, old_len)) = self.memory.array_info(&init.name) {
            if old_len != len {
                return Err(err("array size mismatch on redeclaration"));
            }
            fill(self, base)?;
            return Ok(());
        }
        let base = self.memory.alloc_array(&init.name, len);
        fill(self, base)?;
        Ok(())
    }

    /// Fill a multi-dimensional array from a braced list: a list
    /// of lists fills row by row, zero-filling each row; a flat
    /// list fills row-major, zero-filling at the end. Mixing the
    /// two in one list, a string element, a list longer than
    /// the array, or nesting deeper than the dimensions is an
    /// error. Elements evaluate left to right, in source order.
    fn fill_list(
        &mut self,
        base: i32,
        dims: &[usize],
        elems: &[Expr],
    ) -> Result<(), CompileError> {
        debug_assert!(!dims.is_empty());
        if dims.len() == 1
            && elems
                .iter()
                .any(|e| matches!(e, Expr::InitList(_)))
        {
            return Err(err("nested initializer lists are not supported"));
        }
        if elems
            .iter()
            .any(|e| matches!(e, Expr::StringLit { .. }))
        {
            return Err(err(
                "string initializer is not supported for multi-dimensional arrays",
            ));
        }
        let stride: usize = dims[1..].iter().product();
        let total: usize = dims.iter().product();
        let all_nested = elems.iter().all(|e| matches!(e, Expr::InitList(_)));
        let any_nested = elems.iter().any(|e| matches!(e, Expr::InitList(_)));
        if any_nested && !all_nested {
            return Err(err(
                "mixed nested and flat initializer lists are not supported",
            ));
        }
        if all_nested {
            if elems.len() > dims[0] {
                return Err(err("initializer list is too long"));
            }
            for (row, elem) in elems.iter().enumerate() {
                let Expr::InitList(sub) = elem else {
                    return Err(err("initializer list is too long"));
                };
                self.fill_list(base + (row * stride) as i32, &dims[1..], sub)?;
            }
            for cell in base + (elems.len() * stride) as i32..base + total as i32 {
                self.set_value(cell, 0);
            }
            return Ok(());
        }
        if elems.len() > total {
            return Err(err("initializer list is too long"));
        }
        for (offset, elem) in elems.iter().enumerate() {
            let src = self.compile_expr(elem)?;
            self.copy_value(src, base + offset as i32);
        }
        for cell in base + elems.len() as i32..base + total as i32 {
            self.set_value(cell, 0);
        }
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
            // A subscript chain sizes as the product of the
            // dimensions it leaves: a row of a two-dimensional
            // array sizes as the row length, a full element
            // as 1. Chains longer than the dimensions have no
            // size here (the operand is unevaluated).
            Expr::Index { .. } => match self.classify_chain(e) {
                Ok(ChainClass::Array { dims, indices, .. }) => {
                    if indices.len() > dims.len() {
                        None
                    } else {
                        Some(dims[indices.len()..].iter().product())
                    }
                }
                Ok(ChainClass::ParamArray { row_dims, indices, .. }) => {
                    if indices.len() > row_dims.len() + 1 {
                        None
                    } else {
                        Some(row_dims[indices.len().saturating_sub(1)..].iter().product())
                    }
                }
                _ => None,
            },
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

    /// Flatten a subscript chain: `a[i][j]` is root `a` with
    /// indices `[i, j]` in source order; parentheses unwrap at
    /// every level.
    fn flatten_chain<'a>(e: &'a Expr) -> (&'a Expr, Vec<&'a Expr>) {
        let mut indices = Vec::new();
        let mut cur = e;
        loop {
            match cur {
                Expr::Paren(inner) => cur = inner.as_ref(),
                Expr::Index { base, index } => {
                    indices.push(index.as_ref());
                    cur = base.as_ref();
                }
                _ => break,
            }
        }
        indices.reverse();
        (cur, indices)
    }

    /// Classify a subscript chain by its root. A name that is
    /// neither an array, a recorded parameter, nor a pointer
    /// keeps the single-step subscript errors, so an unknown
    /// or scalar base is diagnosed the same way as before.
    fn classify_chain<'a>(&self, e: &'a Expr) -> Result<ChainClass<'a>, CompileError> {
        let (root, indices) = Self::flatten_chain(e);
        match root {
            Expr::Ident(name) => {
                self.check_extern_defined(name)?;
                if let Some((base, total)) = self.memory.array_info(name) {
                    let dims = self
                        .memory
                        .array_dims(name)
                        .unwrap_or_else(|| vec![total]);
                    Ok(ChainClass::Array {
                        base,
                        dims,
                        indices,
                    })
                } else if let Some(row_dims) = self.param_dims.get(name) {
                    match self.memory.var_index(name) {
                        Some(cell) => Ok(ChainClass::ParamArray {
                            cell,
                            row_dims: row_dims.clone(),
                            indices,
                        }),
                        None => Err(err("pointer is not declared")),
                    }
                } else if self.memory.is_pointer(name) {
                    match self.memory.var_index(name) {
                        Some(_) => Ok(ChainClass::Pointer { root, indices }),
                        None => Err(err("pointer is not declared")),
                    }
                } else if self.memory.has_var(name) {
                    Err(err("subscript of non-array"))
                } else {
                    Err(err("subscript of undeclared array"))
                }
            }
            _ => Ok(ChainClass::Pointer { root, indices }),
        }
    }

    /// Cell holding the base address for the first subscript of
    /// a pointer-valued chain root: a pointer variable's cell,
    /// or whatever address a dereference, call, or string
    /// literal evaluates to.
    fn address_holder(&mut self, e: &Expr) -> Result<i32, CompileError> {
        match e {
            Expr::Paren(inner) => self.address_holder(inner),
            Expr::Ident(name) => {
                if let Some((base, _)) = self.memory.array_info(name) {
                    self.address_const(base)
                } else {
                    Ok(self.memory.var_cell(name))
                }
            }
            _ => self.compile_expr(e),
        }
    }

    /// Cell holding the address of the last element of a
    /// pointer-valued subscript chain: every step but the last
    /// loads the pointer it names, then adds the next index in
    /// byte arithmetic (`pp[i][j]` loads the pointer at
    /// `pp + i`, adds `j`, and addresses through it). A
    /// constant zero index adds nothing, so `p[0]` keeps the
    /// existing emission.
    fn pointer_chain_address(
        &mut self,
        root: &Expr,
        indices: &[&Expr],
    ) -> Result<i32, CompileError> {
        let mut cur = self.address_holder(root)?;
        for (k, index) in indices.iter().enumerate() {
            let addr = match Self::const_index(index) {
                Some(0) => cur,
                _ => {
                    let idx = self.compile_expr(index)?;
                    self.add_values(cur, idx)
                }
            };
            if k + 1 == indices.len() {
                return Ok(addr);
            }
            cur = self.load_through(addr)?;
        }
        Err(err("unsupported subscript base"))
    }

    /// Strides for a dimension list: stride[k] is the product
    /// of the dimensions after k, so the linear offset is the
    /// sum of index[k] * stride[k].
    fn strides_for(dims: &[usize]) -> Vec<usize> {
        let mut out = vec![1usize; dims.len()];
        let mut acc = 1usize;
        for k in (0..dims.len()).rev() {
            out[k] = acc;
            acc = acc.saturating_mul(dims[k]);
        }
        out
    }

    /// Planned chain offset: `Static` when every index is
    /// constant (bounds-checked per dimension), `Dynamic`
    /// carrying the constant prefix sum when any index is
    /// variable. Pure: no cells are allocated and no code is
    /// emitted, so callers can allocate in the same order as
    /// the single-step paths did.
    fn plan_chain_offset(
        strides: &[usize],
        bounds: &[usize],
        indices: &[&Expr],
    ) -> Result<ChainOffsetPlan, CompileError> {
        // Strides/bounds cover every dimension; a row chain
        // supplies fewer indices, using only the leading ones.
        debug_assert!(strides.len() >= indices.len());
        debug_assert!(bounds.len() >= indices.len());
        let mut static_sum: i64 = 0;
        let mut dynamic = false;
        for (k, index) in indices.iter().enumerate() {
            match Self::const_index(index) {
                Some(v) => {
                    if v < 0 || v as usize >= bounds[k] {
                        return Err(err("array index out of bounds"));
                    }
                    static_sum += v as i64 * strides[k] as i64;
                }
                None => dynamic = true,
            }
        }
        if dynamic {
            Ok(ChainOffsetPlan::Dynamic(static_sum))
        } else {
            Ok(ChainOffsetPlan::Static(static_sum as i32))
        }
    }

    /// Emit the variable part of a planned offset: compile each
    /// variable index in order, multiply by its constant
    /// stride, add the terms, and fold the constant prefix in
    /// byte arithmetic (it wraps like every value here).
    fn emit_chain_offset(
        &mut self,
        strides: &[usize],
        indices: &[&Expr],
        static_prefix: i64,
    ) -> Result<i32, CompileError> {
        let mut acc: Option<i32> = None;
        for (k, index) in indices.iter().enumerate() {
            if Self::const_index(index).is_some() {
                continue;
            }
            let idx = self.compile_expr(index)?;
            let stride = strides[k];
            let term = if stride == 1 {
                idx
            } else {
                let cell = self.const_value(stride as u8);
                self.multiply_values(idx, cell)
            };
            acc = Some(match acc {
                None => term,
                Some(prev) => self.add_values(prev, term),
            });
        }
        let acc = acc.expect("a dynamic offset has a variable index");
        if static_prefix == 0 {
            Ok(acc)
        } else {
            let prefix = self.const_value(static_prefix as u8);
            Ok(self.add_values(prefix, acc))
        }
    }

    /// Cell holding the tape index of the storage named by a
    /// whole subscript expression: array chains address
    /// `base + linear offset` in byte arithmetic, parameter
    /// chains add the offset to the runtime base address, and
    /// pointer-valued chains step through loads.
    fn index_address(&mut self, e: &Expr) -> Result<i32, CompileError> {
        match self.classify_chain(e)? {
            ChainClass::Array { base, dims, indices } => {
                if indices.len() > dims.len() {
                    return Err(err("too many subscripts"));
                }
                let strides = Self::strides_for(&dims);
                match Self::plan_chain_offset(&strides, &dims, &indices)? {
                    ChainOffsetPlan::Static(off) => self.address_const(base + off),
                    ChainOffsetPlan::Dynamic(prefix) => {
                        let base_cell = self.address_const(base)?;
                        let off =
                            self.emit_chain_offset(&strides, &indices, prefix)?;
                        Ok(self.add_values(base_cell, off))
                    }
                }
            }
            ChainClass::ParamArray {
                cell,
                row_dims,
                indices,
            } => {
                if indices.len() > row_dims.len() + 1 {
                    return Err(err("too many subscripts"));
                }
                let mut full = vec![1usize];
                full.extend_from_slice(&row_dims);
                let strides = Self::strides_for(&full);
                let mut bounds = vec![usize::MAX];
                for k in 1..indices.len() {
                    bounds.push(row_dims[k - 1]);
                }
                match Self::plan_chain_offset(&strides, &bounds, &indices)? {
                    ChainOffsetPlan::Static(off) => {
                        if off == 0 {
                            Ok(cell)
                        } else {
                            let off_cell = self.const_value(off as u8);
                            Ok(self.add_values(cell, off_cell))
                        }
                    }
                    ChainOffsetPlan::Dynamic(prefix) => {
                        let off =
                            self.emit_chain_offset(&strides, &indices, prefix)?;
                        Ok(self.add_values(cell, off))
                    }
                }
            }
            ChainClass::Pointer { root, indices } => {
                self.pointer_chain_address(root, &indices)
            }
        }
    }

    /// Cell holding the tape index of the storage named by `e`:
    /// a variable, an array (its base), an element (its address),
    /// or a dereference (the address its operand evaluates to).
    fn address_value(&mut self, e: &Expr) -> Result<i32, CompileError> {
        match e {
            Expr::Ident(name) => {
                self.check_extern_defined(name)?;
                if self.functions.contains_key(name) {
                    return self.fn_id_cell(name);
                }
                if let Some((base, _)) = self.memory.array_info(name) {
                    self.address_const(base)
                } else {
                    let idx = self.memory.var_cell(name);
                    self.address_const(idx)
                }
            }
            Expr::Paren(inner) => self.address_value(inner),
            Expr::Index { .. } => self.index_address(e),
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
            Expr::Index { .. } => match self.classify_chain(e) {
                Ok(ChainClass::Pointer { .. }) | Ok(ChainClass::ParamArray { .. }) => true,
                Ok(ChainClass::Array { indices, .. }) => {
                    indices.iter().any(|i| Self::const_index(i).is_none())
                }
                Err(_) => false,
            },
            Expr::Unary { op, .. } => op.as_slice() == b"*",
            _ => false,
        }
    }

    /// Store destination for a multi-dimensional array element
    /// (chain length equals dimension count): a static cell for
    /// constant indices, a range dispatch over the array's
    /// cells for variable ones (an out-of-range variable
    /// offset stores nowhere, as for one dimension).
    fn array_element_target(
        &mut self,
        base: i32,
        dims: &[usize],
        indices: &[&Expr],
    ) -> Result<StoreTarget, CompileError> {
        let strides = Self::strides_for(dims);
        let total: usize = dims.iter().product();
        match Self::plan_chain_offset(&strides, dims, indices)? {
            ChainOffsetPlan::Static(off) => Ok(StoreTarget::Cell(base + off)),
            ChainOffsetPlan::Dynamic(prefix) => {
                let counter = self.emit_chain_offset(&strides, indices, prefix)?;
                let n = total.min(256) as i32;
                Ok(StoreTarget::Range {
                    counter,
                    cells: (base..base + n).collect(),
                })
            }
        }
    }

    /// Value cell for a multi-dimensional array element: the
    /// static cell itself for constant indices, a range load
    /// for variable ones.
    fn array_element_value(
        &mut self,
        base: i32,
        dims: &[usize],
        indices: &[&Expr],
    ) -> Result<i32, CompileError> {
        match self.array_element_target(base, dims, indices)? {
            StoreTarget::Cell(cell) => Ok(cell),
            StoreTarget::Range { counter, cells } => {
                self.load_from_cells(counter, &cells)
            }
            StoreTarget::Any { .. } => {
                unreachable!("array_element_target never builds an Any target")
            }
        }
    }

    /// Whether `e` names a function as a designator: a
    /// defined or prototyped function name, a function-pointer
    /// variable, or a dereference/address chain of one.
    /// Pure: no code is emitted.
    fn is_fn_designator(&self, e: &Expr) -> bool {
        match e {
            Expr::Paren(inner) => self.is_fn_designator(inner),
            Expr::Unary { op, e: inner }
                if op.as_slice() == b"*" || op.as_slice() == b"&" =>
            {
                self.is_fn_designator(inner)
            }
            Expr::Ident(name) => {
                self.functions.contains_key(name)
                    || self.prototypes.contains_key(name)
                    || self.fnptrs.contains(name)
            }
            _ => false,
        }
    }

    /// Declared signature of a function-pointer lvalue
    /// (`fp`, `(fp)`), or `None` for any other target.
    fn fnptr_lvalue_sig(&self, e: &Expr) -> Option<FuncSig> {
        let mut cur = e;
        while let Expr::Paren(inner) = cur {
            cur = inner;
        }
        let Expr::Ident(name) = cur else {
            return None;
        };
        self.fnptr_sigs.get(name).cloned()
    }

    /// Store destination for an assignment target: a static cell,
    /// or a runtime address plus the cells a dispatch searches
    /// (an array range, or every byte address when absent).
    fn store_target(&mut self, e: &Expr) -> Result<StoreTarget, CompileError> {
        match e {
            Expr::Ident(name) => {
                self.check_extern_defined(name)?;
                if self.functions.contains_key(name)
                    || self.prototypes.contains_key(name)
                {
                    return Err(err("cannot assign to a function"));
                }
                if self.memory.is_array(name) {
                    Err(err("cannot assign to an array"))
                } else {
                    Ok(StoreTarget::Cell(self.memory.var_cell(name)))
                }
            }
            Expr::Paren(inner) => self.store_target(inner),
            Expr::Index { base, index } => match self.classify_chain(e)? {
                ChainClass::Array {
                    base: cell_base,
                    dims,
                    indices,
                } => {
                    if indices.len() > dims.len() {
                        return Err(err("too many subscripts"));
                    }
                    if indices.len() < dims.len() {
                        return Err(err("cannot assign to an array"));
                    }
                    if dims.len() == 1 {
                        match self.subscript_base(base)? {
                            SubBase::Array(cell_base, len) => {
                                if Self::const_index(index).is_some() {
                                    Ok(StoreTarget::Cell(
                                        self.static_index_cell(base, index)?,
                                    ))
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
                                let addr = self.index_address(e)?;
                                Ok(StoreTarget::Any { addr })
                            }
                        }
                    } else {
                        self.array_element_target(cell_base, &dims, &indices)
                    }
                }
                ChainClass::ParamArray { row_dims, indices, .. } => {
                    if indices.len() > row_dims.len() + 1 {
                        return Err(err("too many subscripts"));
                    }
                    if indices.len() <= row_dims.len() {
                        return Err(err("cannot assign to an array"));
                    }
                    let addr = self.index_address(e)?;
                    Ok(StoreTarget::Any { addr })
                }
                ChainClass::Pointer { root, indices } => {
                    let addr = self.pointer_chain_address(root, &indices)?;
                    Ok(StoreTarget::Any { addr })
                }
            },
            Expr::Unary { op, e: inner } if op.as_slice() == b"*" => {
                if self.is_fn_designator(inner) {
                    return Err(err("cannot assign to a function"));
                }
                let addr = self.compile_expr(inner)?;
                Ok(StoreTarget::Any { addr })
            }
            _ => Err(err("invalid lvalue in assignment")),
        }
    }

    /// Write `value` into a store destination.
    fn store_value(&mut self, target: &StoreTarget, value: i32) -> Result<i32, CompileError> {
        // An assignment expression yields the value stored,
        // captured in a fresh cell — not the target cell,
        // whose later reassignment inside the same expression
        // must not rewrite it. The capture is taken before
        // the store so it reads the value as computed, even
        // if the store overwrites the value cell itself.
        let captured = self.duplicate_value(value);
        match target {
            StoreTarget::Cell(cell) => {
                self.copy_value(captured, *cell);
            }
            StoreTarget::Range { counter, cells } => {
                self.store_to_cells(*counter, cells, captured)?;
            }
            StoreTarget::Any { addr } => {
                self.store_through(*addr, captured)?;
            }
        }
        Ok(captured)
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
            Expr::Index { .. } => match self.classify_chain(e) {
                Ok(ChainClass::Array { base, dims, indices }) => {
                    if indices.len() > dims.len() {
                        return None;
                    }
                    let strides = Self::strides_for(&dims);
                    match Self::plan_chain_offset(&strides, &dims, &indices) {
                        Ok(ChainOffsetPlan::Static(off)) => Some(base + off),
                        _ => None,
                    }
                }
                _ => None,
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
            // A subscript chain names an array while indices
            // remain: `a[i]` of a two-dimensional array is a
            // row, not a modifiable element.
            Expr::Index { .. } => match self.classify_chain(e) {
                Ok(ChainClass::Array { dims, indices, .. }) => indices.len() < dims.len(),
                Ok(ChainClass::ParamArray { row_dims, indices, .. }) => {
                    indices.len() <= row_dims.len()
                }
                _ => false,
            },
            _ => false,
        }
    }

    /// Base cell of a multi-dimensional array named (through
    /// parentheses) by a dereference operand, if any: `*a`
    /// for such an array is its first row. One-dimensional
    /// arrays are unaffected (`*a` is the first element).
    fn deref_row_base(&self, e: &Expr) -> Option<i32> {
        let mut cur = e;
        while let Expr::Paren(inner) = cur {
            cur = inner.as_ref();
        }
        let Expr::Ident(name) = cur else {
            return None;
        };
        let dims = self.memory.array_dims(name)?;
        if dims.len() <= 1 {
            return None;
        }
        self.memory.array_info(name).map(|(base, _)| base)
    }

    /// Cell holding a callee ID when `e` dereferences a
    /// function designator or a function pointer: `*fp` is
    /// the ID `fp` holds, `*f` is `f`'s ID, and chains
    /// collapse the same way. Returns `None` for ordinary
    /// dereferences, which load through dispatch.
    fn fnptr_deref_value(&mut self, e: &Expr) -> Result<Option<i32>, CompileError> {
        match e {
            Expr::Paren(inner) => self.fnptr_deref_value(inner),
            Expr::Unary { op, e: inner } if op.as_slice() == b"*" => {
                self.fnptr_deref_value(inner)
            }
            Expr::Ident(name) => {
                if self.fnptrs.contains(name) {
                    Ok(Some(self.memory.var_cell(name)))
                } else if self.functions.contains_key(name) {
                    Ok(Some(self.fn_id_cell(name)?))
                } else if self.prototypes.contains_key(name) {
                    Err(err(format!(
                        "undefined function '{}'",
                        String::from_utf8_lossy(name)
                    )))
                } else {
                    Ok(None)
                }
            }
            _ => Ok(None),
        }
    }

    /// Reject dereferencing a plain scalar variable (or an
    /// undeclared name). Arrays and pointers, and any other
    /// expression (whose value is an address), pass.
    fn check_deref_operand(&self, e: &Expr) -> Result<(), CompileError> {
        match e {
            Expr::Paren(inner) => self.check_deref_operand(inner),
            Expr::Ident(name) => {
                self.check_extern_defined(name)?;
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
                        self.compile_decl_goto(d)?
                    }
                }
                BlockItem::Stmt(s) => {
                    // SPIKE (goto): a label statement for a pending
                    // `goto` must not be guarded by its own flag —
                    // the flag is cleared by the label itself, and
                    // a guard evaluated before the clear would skip
                    // the label forever. Drop the name from the
                    // pending set first; the remaining flags still
                    // guard the statement.
                    if let Stmt::GotoLabel { name, .. } = s {
                        self.goto_pending.remove(name);
                    }
                    if after_return {
                        self.compile_stmt_return_guarded(s)?
                    } else if after_break || after_continue {
                        self.compile_stmt_guarded(s)?
                    } else {
                        self.compile_stmt_goto(s)?
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
                return self.perform_if(nc, |em| em.compile_stmt_goto(s));
            }
        }
        self.compile_stmt_goto(s)
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
                return self.perform_if(nc, |em| em.compile_decl_goto(d));
            }
        }
        self.compile_decl_goto(d)
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

    /// SPIKE (goto): condition `c` additionally gated on every
    /// flag in `flags` being clear (identity with no flags).
    fn and_not_gotos(&mut self, mut c: i32, flags: &[i32]) -> Result<i32, CompileError> {
        for &f in flags {
            let nf = self.negate(f)?;
            c = self.logical_and(c, nf)?;
        }
        Ok(c)
    }

    /// SPIKE (goto): run `f` only when every flag in `flags` is
    /// clear, via one combined guard cell (identity with no
    /// flags). Used for `for` updates and `do...while` bodies,
    /// which a loop-condition gate alone cannot skip.
    fn perform_unless_gotos(
        &mut self,
        flags: &[i32],
        mut f: impl FnMut(&mut Self) -> Result<(), CompileError>,
    ) -> Result<(), CompileError> {
        if flags.is_empty() {
            return f(self);
        }
        let mut ok = self.const_value(1);
        for &flag in flags {
            let nf = self.negate(flag)?;
            ok = self.logical_and(ok, nf)?;
        }
        self.perform_if(ok, |em| f(em))
    }

    /// SPIKE (goto): flag cells for `goto`s pending at the start
    /// of the construct being compiled, sorted and deduplicated.
    fn pending_goto_flag_cells(&self) -> Vec<i32> {
        let mut cells: Vec<i32> = self
            .goto_pending
            .iter()
            .filter_map(|n| self.goto_flags.get(n).copied())
            .collect();
        cells.sort_unstable();
        cells.dedup();
        cells
    }

    /// SPIKE (goto): flag cells a loop condition (and a `for`
    /// update) must gate on: `goto`s inside the loop body whose
    /// label is defined outside the body (an exit jump), plus
    /// flags pending at the loop's entry. A `goto` whose label
    /// is inside the body is not gated: the next iteration
    /// re-enters the body, the pre-label statements are
    /// guarded by the pending flag, and the label clears it.
    fn loop_goto_flag_cells(&self, body: &Stmt) -> Vec<i32> {
        let mut names: HashSet<Vec<u8>> = HashSet::new();
        goto_targets_in(body, &mut names);
        let mut inside: HashSet<Vec<u8>> = HashSet::new();
        goto_labels_in(body, &mut inside);
        names.retain(|n| !inside.contains(n));
        for n in &self.goto_pending {
            names.insert(n.clone());
        }
        let mut cells: Vec<i32> = names
            .iter()
            .filter_map(|n| self.goto_flags.get(n).copied())
            .collect();
        cells.sort_unstable();
        cells.dedup();
        cells
    }

    /// SPIKE (goto): compile `s`, skipping it at runtime if any
    /// pending `goto` flag is set. Guards nest one per flag, in
    /// name order for deterministic emission.
    fn compile_stmt_goto(&mut self, s: &Stmt) -> Result<(), CompileError> {
        if self.goto_pending.is_empty() {
            return self.compile_stmt(s);
        }
        let mut names: Vec<Vec<u8>> = self.goto_pending.iter().cloned().collect();
        names.sort();
        self.compile_stmt_goto_chain(s, &names)
    }

    fn compile_stmt_goto_chain(
        &mut self,
        s: &Stmt,
        names: &[Vec<u8>],
    ) -> Result<(), CompileError> {
        match names.split_first() {
            None => self.compile_stmt(s),
            Some((name, rest)) => {
                let Some(flag) = self.goto_flags.get(name).copied() else {
                    return self.compile_stmt_goto_chain(s, rest);
                };
                let nc = self.negate(flag)?;
                self.perform_if(nc, |em| em.compile_stmt_goto_chain(s, rest))
            }
        }
    }

    /// SPIKE (goto): like [`Self::compile_stmt_goto`] for
    /// declarations.
    fn compile_decl_goto(&mut self, d: &Decl) -> Result<(), CompileError> {
        if self.goto_pending.is_empty() {
            return self.compile_decl(d);
        }
        let mut names: Vec<Vec<u8>> = self.goto_pending.iter().cloned().collect();
        names.sort();
        self.compile_decl_goto_chain(d, &names)
    }

    fn compile_decl_goto_chain(
        &mut self,
        d: &Decl,
        names: &[Vec<u8>],
    ) -> Result<(), CompileError> {
        match names.split_first() {
            None => self.compile_decl(d),
            Some((name, rest)) => {
                let Some(flag) = self.goto_flags.get(name).copied() else {
                    return self.compile_decl_goto_chain(d, rest);
                };
                let nc = self.negate(flag)?;
                self.perform_if(nc, |em| em.compile_decl_goto_chain(d, rest))
            }
        }
    }

    /// SPIKE (goto): validate a function body, allocate one flag
    /// cell per label (zero) in label order, compile the body,
    /// then restore the outer function's goto state. Flag cells
    /// allocated inside an expansion are reclaimed by the
    /// caller's checkpoint release, like other expansion temps.
    fn compile_function_body(&mut self, body: &Block) -> Result<(), CompileError> {
        let label_order = validate_gotos(body)?;
        let saved_flags = std::mem::take(&mut self.goto_flags);
        let saved_pending = std::mem::take(&mut self.goto_pending);
        for name in label_order {
            let flag = self.const_value(0);
            self.goto_flags.insert(name, flag);
        }
        let r = self.compile_block(body);
        self.goto_flags = saved_flags;
        self.goto_pending = saved_pending;
        r
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
                // SPIKE (goto): a `goto` out of the loop exits it,
                // like `break`; a `goto` into the body is rejected.
                let goto_flags = self.loop_goto_flag_cells(body);
                let r = self.perform_while(
                    |em| {
                        let c = em.compile_expr(cond)?;
                        let c = em.and_not_broken(c, broken)?;
                        let c = em.and_not_gotos(c, &goto_flags)?;
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
                let goto_flags = self.loop_goto_flag_cells(body);
                // The first body emission is unconditional, so a
                // `goto` pending at entry must guard the whole
                // loop, not just the re-test.
                let entry_flags = self.pending_goto_flag_cells();
                let r = self.perform_unless_gotos(&entry_flags, |em| {
                    em.perform_do_while(
                        |em| {
                            let c = em.compile_expr(cond)?;
                            let c = em.and_not_broken(c, broken)?;
                            let c = em.and_not_gotos(c, &goto_flags)?;
                            em.and_not_returned(c)
                        },
                        |em| em.compile_loop_body(body, continued),
                    )
                });
                self.continue_stack.pop();
                self.break_stack.pop();
                r
            }
            Stmt::For { init, cond, incr, body } => {
                let broken = self.loop_break_flag(body)?;
                let continued = self.loop_continue_flag(body)?;
                let goto_flags = self.loop_goto_flag_cells(body);
                let r = self.perform_for(
                    |em| em.compile_for_init(init),
                    |em| {
                        let c = match cond {
                            Some(e) => em.compile_expr(e),
                            None => Ok(em.const_value(1)),
                        }?;
                        let c = em.and_not_broken(c, broken)?;
                        let c = em.and_not_gotos(c, &goto_flags)?;
                        em.and_not_returned(c)
                    },
                    |em| {
                        // SPIKE (goto): the update is skipped after a
                        // `goto` out of the loop, as after `break`.
                        em.perform_unless_gotos(&goto_flags, |em| {
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
            // SPIKE (goto): set the label's flag; the rest of the
            // block is guarded until the label clears it. The
            // label records itself as pending in source order,
            // so statements compiled after a `goto` are guarded
            // at compile time exactly when they may be skipped.
            Stmt::Goto { name } => {
                let Some(flag) = self.goto_flags.get(name).copied() else {
                    return Err(err(format!(
                        "undefined label '{}'",
                        String::from_utf8_lossy(name)
                    )));
                };
                self.set_value(flag, 1);
                self.goto_pending.insert(name.clone());
                Ok(())
            }
            // SPIKE (goto): clear the label's flag (a no-op when
            // reached in normal flow) and compile the body; the
            // label also leaves the pending set, so the body is
            // not guarded by its own flag.
            Stmt::GotoLabel { name, body } => {
                if let Some(flag) = self.goto_flags.get(name).copied() {
                    self.set_value(flag, 0);
                }
                self.goto_pending.remove(name);
                self.compile_stmt(body)
            }
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
        let src = self.compile_scalar_init(e)?;
        self.copy_value(src, dest);
        Ok(())
    }

    fn compile_switch(&mut self, expr: &Expr, body: &Stmt) -> Result<(), CompileError> {
        let expr_cell = self.compile_expr(expr)?;
        let mut cases: Vec<SwitchCase<'_>> = Vec::new();
        self.collect_switch_stmt(body, &mut cases)?;
        let any_match = self.const_value(0);
        // When there is at most one `default` and it comes
        // last (the usual shape, and the only shape the fuzzer
        // emits), compute each case's match just in time and
        // reclaim it after the case, instead of holding every
        // match cell live across the whole switch: the
        // default's gate reads `any_match`, which is complete
        // by then, and `fall`/`broken`/expression/label cells
        // are allocated before the per-case checkpoint, so
        // they survive. Any other default placement keeps the
        // eager shape below, which computes all matches
        // upfront.
        let defaults = cases.iter().filter(|c| c.is_default).count();
        let lazy = defaults == 0
            || (defaults == 1 && cases.last().is_some_and(|c| c.is_default));
        let mut matched: Vec<i32> = Vec::new();
        if !lazy {
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
        }
        let fall = self.const_value(0);
        let broken = self.const_value(0);
        self.break_stack.push(broken);
        for (i, c) in cases.iter().enumerate() {
            let checkpoint = self.checkpoint();
            let match_cell = if lazy {
                if c.is_default {
                    -1
                } else {
                    let m = self.is_equal(expr_cell, c.label_cell)?;
                    let o = self.logical_or(any_match, m)?;
                    self.move_value(o, any_match);
                    m
                }
            } else {
                matched[i]
            };
            // GCC right-to-left within each nested call (see order probe).
            let run_cond = if c.is_default {
                let nb = self.negate(broken)?;
                let nm = self.negate(any_match)?;
                let o = self.logical_or(fall, nm)?;
                self.logical_and(o, nb)?
            } else {
                let nb = self.negate(broken)?;
                let o = self.logical_or(match_cell, fall)?;
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
            // Just-in-time path only: the match and gate temporaries
            // are dead once the case's guard has run (the body
            // flows out through `fall`/`broken`/variables and
            // the guard's own reclaim), so later cases reuse
            // their cells. The eager path keeps its layout
            // exactly.
            if lazy {
                self.release_temps(checkpoint);
            }
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
                // variable cell itself. A bare function name is
                // a designator: its callee ID.
                self.check_extern_defined(name)?;
                if self.functions.contains_key(name) {
                    return self.fn_id_cell(name);
                }
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
            // Lists only reach here outside a declarator (the
            // parser only builds them in initializer position).
            Expr::InitList(_) => Err(err(
                "initializer lists are only allowed in declarations",
            )),
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
                    // A function-pointer target takes only a
                    // designator, a function pointer of the same
                    // signature, or 0 — never a plain value.
                    let r = if let Some(sig) = self.fnptr_lvalue_sig(lhs) {
                        self.compile_fnptr_value(rhs, &sig)?
                    } else {
                        self.compile_expr(rhs)?
                    };
                    self.store_value(&target, r)
                } else {
                    // Arithmetic on function pointers is
                    // meaningless (callee IDs have no address
                    // arithmetic); the dereference form below
                    // is refused by `store_target`.
                    if self.fnptr_lvalue_sig(lhs).is_some() {
                        return Err(err(
                            "arithmetic on function pointers is not supported",
                        ));
                    }
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
                // Dereferencing a multi-dimensional array name
                // yields its first row, which decays to the base
                // address instead of loading the first element.
                b"*" => {
                    // Dereferencing a function designator or a
                    // function pointer yields the callee ID
                    // itself, never a load through dispatch.
                    if let Some(cell) = self.fnptr_deref_value(e)? {
                        return Ok(cell);
                    }
                    self.check_deref_operand(e)?;
                    if let Some(base) = self.deref_row_base(e) {
                        return self.address_const(base);
                    }
                    let addr = self.compile_expr(e)?;
                    self.load_through(addr)
                }
                b"++" => {
                    if self.fnptr_lvalue_sig(e).is_some() {
                        return Err(err(
                            "arithmetic on function pointers is not supported",
                        ));
                    }
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
                    if self.fnptr_lvalue_sig(e).is_some() {
                        return Err(err(
                            "arithmetic on function pointers is not supported",
                        ));
                    }
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
                if self.fnptr_lvalue_sig(e).is_some() {
                    return Err(err(
                        "arithmetic on function pointers is not supported",
                    ));
                }
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
            Expr::Index { base, index } => match self.classify_chain(e)? {
                ChainClass::Array {
                    base: cell_base,
                    dims,
                    indices,
                } => {
                    if indices.len() > dims.len() {
                        return Err(err("too many subscripts"));
                    }
                    if indices.len() < dims.len() {
                        // A short chain names a row, which decays
                        // to its address.
                        return self.index_address(e);
                    }
                    if dims.len() == 1 {
                        match self.subscript_base(base)? {
                            SubBase::Array(cell_base, len) => {
                                if Self::const_index(index).is_some() {
                                    self.static_index_cell(base, index)
                                } else {
                                    // As in `store_target`: the dispatch counts
                                    // the offset itself down.
                                    let counter = self.compile_expr(index)?;
                                    let n = len.min(256) as i32;
                                    let cells: Vec<i32> =
                                        (cell_base..cell_base + n).collect();
                                    self.load_from_cells(counter, &cells)
                                }
                            }
                            SubBase::Pointer(_) => {
                                let addr = self.index_address(e)?;
                                self.load_through(addr)
                            }
                        }
                    } else {
                        self.array_element_value(cell_base, &dims, &indices)
                    }
                }
                ChainClass::ParamArray { row_dims, indices, .. } => {
                    if indices.len() > row_dims.len() + 1 {
                        return Err(err("too many subscripts"));
                    }
                    if indices.len() <= row_dims.len() {
                        return self.index_address(e);
                    }
                    let addr = self.index_address(e)?;
                    self.load_through(addr)
                }
                ChainClass::Pointer { root, indices } => {
                    let addr = self.pointer_chain_address(root, &indices)?;
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
            Expr::Index { .. } => match self.classify_chain(e) {
                Ok(ChainClass::Array { base, dims, indices }) => {
                    if indices.len() > dims.len() {
                        return None;
                    }
                    let strides = Self::strides_for(&dims);
                    let off = match Self::plan_chain_offset(&strides, &dims, &indices) {
                        Ok(ChainOffsetPlan::Static(off)) => off,
                        _ => return None,
                    };
                    let total: usize = dims.iter().product();
                    Some((base + off, total - off as usize))
                }
                _ => None,
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
                self.check_extern_defined(name)?;
                if self.functions.contains_key(name)
                    || self.prototypes.contains_key(name)
                {
                    return Err(err("cannot assign to a function"));
                }
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
            // A dereference is itself an address-valued target:
            // `scanf("%d", *pp)` reads into the cell `*pp`
            // names, and `scanf("%d", *p)` into the cell `*p`
            // names. The loaded address is the destination.
            Expr::Unary { op, .. } if op.as_slice() == b"*" => {
                Ok(ScanfTarget::Through(self.compile_expr(a)?))
            }
            // A subscript naming a pointer value is likewise
            // address-valued (`scanf("%d", pp[0])` reads into
            // the cell the pointer at `pp[0]` names); an array
            // element subscript is not, and keeps the error.
            Expr::Index { .. }
                if matches!(
                    self.classify_chain(a),
                    Ok(ChainClass::Pointer { .. })
                ) =>
            {
                Ok(ScanfTarget::Through(self.compile_expr(a)?))
            }
            _ => Err(err("scanf argument must be an address")),
        }
    }

    /// Read one `scanf` scalar into a destination: directly for
    /// a static cell (a bounded range writes its first cell, as
    /// before), through dispatch for a runtime address. `None`
    /// reads a character (`%c`); `Some` reads a based number
    /// (`%o`/`%x`/`%i`). Decimal (`%d`/`%u`) uses
    /// `read_decimal_into_target` instead, keeping the decimal
    /// reader whose emission is pinned by the goldens.
    fn read_into_target(
        &mut self,
        target: ScanfTarget,
        base: Option<NumericBase>,
    ) -> Result<(), CompileError> {
        let read = |em: &mut Self, cell: i32| -> Result<(), CompileError> {
            match base {
                None => Ok(em.read_as_char(cell)),
                Some(b) => em.read_as_number_base(cell, b),
            }
        };
        match target {
            ScanfTarget::Cell(cell) | ScanfTarget::Bounded { base: cell, .. } => read(self, cell),
            ScanfTarget::Through(addr) => {
                let tmp = self.const_value(0);
                read(self, tmp)?;
                self.store_through(addr, tmp)?;
                Ok(())
            }
        }
    }

    /// Read one decimal `scanf` value (`%d`/`%u`) into a
    /// destination, on the decimal reader.
    fn read_decimal_into_target(&mut self, target: ScanfTarget) -> Result<(), CompileError> {
        match target {
            ScanfTarget::Cell(cell) | ScanfTarget::Bounded { base: cell, .. } => {
                self.read_as_number(cell)
            }
            ScanfTarget::Through(addr) => {
                let tmp = self.const_value(0);
                self.read_as_number(tmp)?;
                self.store_through(addr, tmp)?;
                Ok(())
            }
        }
    }

    /// Read one `scanf` scalar conversion into `target`, or into
    /// a scratch cell whose value is discarded when `target` is
    /// `None` (assignment suppression).
    fn read_scanf_scalar(
        &mut self,
        target: Option<ScanfTarget>,
        conv: ScanfConv,
    ) -> Result<(), CompileError> {
        let dest = match target {
            Some(t) => t,
            None => ScanfTarget::Cell(self.const_value(0)),
        };
        match conv {
            ScanfConv::Char => self.read_into_target(dest, None),
            ScanfConv::Decimal => self.read_decimal_into_target(dest),
            ScanfConv::Based(b) => self.read_into_target(dest, Some(b)),
        }
    }

    /// Compile a call: only `printf`/`scanf` with a string-literal format.
    /// Cell holding the callee ID of a defined function: its
    /// position in translation-unit order plus one (0 is
    /// null). More than 255 defined functions cannot be
    /// named through a pointer.
    fn fn_id_cell(&mut self, name: &[u8]) -> Result<i32, CompileError> {
        match self.fn_order.iter().position(|n| n.as_slice() == name) {
            Some(idx) if idx < 255 => Ok(self.const_value((idx + 1) as u8)),
            Some(_) => Err(err("too many functions for function pointer dispatch")),
            None => Err(err(format!(
                "undefined function '{}'",
                String::from_utf8_lossy(name)
            ))),
        }
    }

    /// (cell holding a callee ID, signature) for a function
    /// designator chain: `f`, `&f`, `*fp`, `**fp`, and
    /// parenthesized forms. A prototype-only name has no ID;
    /// anything else that is not a designator or a function
    /// pointer is an incompatible type.
    fn fn_designator_chain(&mut self, e: &Expr) -> Result<(i32, FuncSig), CompileError> {
        match e {
            Expr::Paren(inner) => self.fn_designator_chain(inner),
            Expr::Unary { op, e: inner }
                if op.as_slice() == b"*" || op.as_slice() == b"&" =>
            {
                self.fn_designator_chain(inner)
            }
            Expr::Ident(name) => {
                let sig = self.functions.get(name).map(|def| FuncSig {
                    is_void: def.is_void,
                    returns_ptr_depth: def.returns_ptr_depth,
                    params: def.params.clone(),
                });
                if let Some(sig) = sig {
                    let cell = self.fn_id_cell(name)?;
                    Ok((cell, sig))
                } else if self.prototypes.contains_key(name) {
                    Err(err(format!(
                        "undefined function '{}'",
                        String::from_utf8_lossy(name)
                    )))
                } else if let Some(sig) = self.fnptr_sigs.get(name).cloned() {
                    Ok((self.memory.var_cell(name), sig))
                } else {
                    self.check_extern_defined(name)?;
                    Err(err("incompatible function pointer type"))
                }
            }
            _ => Err(err("incompatible function pointer type")),
        }
    }

    /// Cell holding a function-pointer value for `e` in a
    /// position whose expected signature is `expected`: a
    /// designator, a function pointer of a compatible
    /// signature, a null constant, a ternary of two such
    /// values, or a cast, parenthesized, or comma form
    /// around one. Anything else is an incompatible type.
    fn compile_fnptr_value(&mut self, e: &Expr, expected: &FuncSig) -> Result<i32, CompileError> {
        let expected = expected.clone();
        match e {
            Expr::Paren(inner) => self.compile_fnptr_value(inner, &expected),
            Expr::Cast { e: inner, .. } => self.compile_fnptr_value(inner, &expected),
            Expr::Comma(es) => {
                for prefix in &es[..es.len().saturating_sub(1)] {
                    self.compile_expr(prefix)?;
                }
                match es.last() {
                    Some(last) => self.compile_fnptr_value(last, &expected),
                    None => Err(err("incompatible function pointer type")),
                }
            }
            Expr::Int(bytes) => {
                if int_literal_value(bytes) == 0 {
                    Ok(self.const_value(0))
                } else {
                    Err(err("incompatible function pointer type"))
                }
            }
            Expr::Ternary { cond, then, els } => {
                let c = self.compile_expr(cond)?;
                let result = self.const_value(0);
                let expected_then = expected.clone();
                let expected_els = expected.clone();
                self.perform_if_else(
                    c,
                    move |em| {
                        let t = em.compile_fnptr_value(then, &expected_then)?;
                        em.copy_value(t, result);
                        Ok(())
                    },
                    move |em| {
                        let v = em.compile_fnptr_value(els, &expected_els)?;
                        em.copy_value(v, result);
                        Ok(())
                    },
                )?;
                Ok(result)
            }
            _ => {
                let (cell, sig) = self.fn_designator_chain(e)?;
                if !sig.matches(&expected) {
                    return Err(err("incompatible function pointer type"));
                }
                Ok(cell)
            }
        }
    }

    fn compile_call(&mut self, callee: &Expr, args: &[Expr]) -> Result<i32, CompileError> {
        let name = match callee {
            Expr::Ident(name) => name.as_slice(),
            _ => {
                // Parenthesized builtin names still mean the
                // builtin (`(printf)(...)`); anything else
                // goes through designator dispatch.
                let mut cur = callee;
                while let Expr::Paren(inner) = cur {
                    cur = inner;
                }
                if let Expr::Ident(bname) = cur {
                    if bname.as_slice() == b"printf" || bname.as_slice() == b"scanf" {
                        bname.as_slice()
                    } else if let Some((id, sig)) = self.indirect_callee(callee)? {
                        return self.compile_indirect_call(id, &sig, args);
                    } else {
                        return Err(err("Unsupported postfix expression"));
                    }
                } else if let Some((id, sig)) = self.indirect_callee(callee)? {
                    return self.compile_indirect_call(id, &sig, args);
                } else {
                    return Err(err("Unsupported postfix expression"));
                }
            }
        };
        if self.functions.contains_key(name) {
            let def = Rc::clone(
                self.functions
                    .get(name)
                    .expect("registered function is present"),
            );
            return self.expand_call(name, &def, args);
        }
        // A function-pointer variable as callee dispatches on
        // the ID it holds, against its declared signature.
        if let Some(sig) = self.fnptr_sigs.get(name).cloned() {
            let id = self.memory.var_cell(name);
            return self.compile_indirect_call(id, &sig, args);
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
                        // `%x`/`%X` print hex, `%o` octal. Length
                        // modifiers are skipped (every value is one
                        // byte); flags, widths, and precisions still
                        // fail below.
                        skip_length_modifiers(format, &mut i);
                        match format.get(i).copied() {
                            Some(b'%') => self.print_char(b'%'),
                            Some(
                                spec @ (b'c' | b'd' | b'u' | b'x' | b'X' | b'o' | b's'),
                            ) => {
                                let cell = next_format_arg(&vals, &mut next_var)?;
                                match spec {
                                    b'c' => self.print_as_char(cell),
                                    b'x' => self.print_as_hex(cell)?,
                                    b'X' => self.print_as_hex_upper(cell)?,
                                    b'o' => self.print_as_octal(cell)?,
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
                        // Assignment suppression (`%*d`): convert and
                        // discard, consuming no target. Length
                        // modifiers are skipped, as for `printf`.
                        let suppressed = if format.get(i).copied() == Some(b'*') {
                            i += 1;
                            true
                        } else {
                            false
                        };
                        skip_length_modifiers(format, &mut i);
                        let target = if suppressed {
                            None
                        } else {
                            Some(next_scanf_target(&targets, &mut next_var)?)
                        };
                        match format.get(i).copied() {
                            Some(b'c') => {
                                self.read_scanf_scalar(target, ScanfConv::Char)?
                            }
                            Some(b'd') | Some(b'u') => {
                                self.read_scanf_scalar(target, ScanfConv::Decimal)?
                            }
                            Some(b'o') => self.read_scanf_scalar(
                                target,
                                ScanfConv::Based(NumericBase::Oct),
                            )?,
                            Some(b'x') => self.read_scanf_scalar(
                                target,
                                ScanfConv::Based(NumericBase::Hex),
                            )?,
                            Some(b'i') => self.read_scanf_scalar(
                                target,
                                ScanfConv::Based(NumericBase::Auto),
                            )?,
                            Some(b's') => match target {
                                Some(ScanfTarget::Bounded { base, len }) => {
                                    self.read_as_string_bounded(base, len)?
                                }
                                Some(ScanfTarget::Cell(cell)) => {
                                    let addr = self.address_const(cell)?;
                                    self.read_as_string_through(addr)?;
                                }
                                Some(ScanfTarget::Through(addr)) => {
                                    self.read_as_string_through(addr)?;
                                }
                                // Discard into anonymous scratch: the
                                // bounded read truncates and drops
                                // the rest of the token, as for an
                                // array, and the cells are never
                                // reclaimed.
                                None => {
                                    let base = self.memory.alloc_anon_array(256);
                                    self.read_as_string_bounded(base, 256)?;
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
        // earlier argument's value before binding. A
        // function-pointer argument is checked against the
        // parameter's signature as it is compiled.
        let mut arg_cells = Vec::with_capacity(args.len());
        for (param, arg) in def.params.iter().zip(args.iter()) {
            let v = if let Some(sig) = &param.fn_sig {
                self.compile_fnptr_value(arg, sig)?
            } else {
                self.compile_expr(arg)?
            };
            arg_cells.push(self.duplicate_value(v));
        }
        self.expand_call_body(name, def, &arg_cells, result, returned, checkpoint)?;
        Ok(result)
    }

    /// Bind already-evaluated arguments and compile one
    /// function body under a fresh return context: shared by
    /// direct calls and by each guarded arm of an indirect
    /// call. The caller owns the reclaim checkpoint, so
    /// cells allocated before it (the result, the returned
    /// flag, the argument cells) survive; only the body's
    /// own temporaries are reclaimed here.
    fn expand_call_body(
        &mut self,
        name: &[u8],
        def: &ExpandedFunction,
        arg_cells: &[i32],
        result: i32,
        returned: i32,
        checkpoint: i32,
    ) -> Result<(), CompileError> {
        if self.expanding.iter().any(|n| n.as_slice() == name) {
            return Err(err(format!(
                "recursive call to function '{}' is not supported",
                String::from_utf8_lossy(name)
            )));
        }
        for (param, &arg) in def.params.iter().zip(arg_cells.iter()) {
            let cell = self.memory.var_cell(&param.name);
            if let Some(sig) = &param.fn_sig {
                self.fnptrs.insert(param.name.clone());
                self.fnptr_sigs
                    .insert(param.name.clone(), (**sig).clone());
            } else {
                if param.ptr_depth > 0 || param.is_array() {
                    self.memory.mark_pointer(&param.name);
                }
                // Multi-dimensional array parameters record their
                // stride (the dimensions after the first) for
                // subscripting at call sites.
                if param.dims.len() > 1 {
                    let row_dims: Vec<usize> = param.dims[1..]
                        .iter()
                        .copied()
                        .map(|d| d.expect("parameter stride is sized") as usize)
                        .collect();
                    self.param_dims.insert(param.name.clone(), row_dims);
                }
            }
            self.copy_value(arg, cell);
        }
        let saved_break = std::mem::take(&mut self.break_stack);
        let saved_continue = std::mem::take(&mut self.continue_stack);
        let saved_return = self.return_context.replace((returned, result));
        self.expanding.push(name.to_vec());
        let compiled = self.compile_function_body(&def.body);
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
        Ok(())
    }

    /// Indirect call through a callee ID: evaluate the
    /// arguments once, then dispatch on the ID over every
    /// defined function whose signature matches. Each
    /// matching arm expands under an ID-equality guard, so
    /// only the pointed-to body runs; a null ID (or a
    /// signature with no defined function) runs no arm and
    /// leaves the result zero. The result and returned flag
    /// are shared by all arms, so a matching arm's return
    /// flows out exactly as for a direct call.
    fn compile_indirect_call(
        &mut self,
        id_cell: i32,
        sig: &FuncSig,
        args: &[Expr],
    ) -> Result<i32, CompileError> {
        if args.len() != sig.params.len() {
            return Err(err("wrong number of arguments in indirect call"));
        }
        let result = self.const_value(0);
        let returned = self.const_value(0);
        let mut arg_cells = Vec::with_capacity(args.len());
        for (param, arg) in sig.params.iter().zip(args.iter()) {
            let v = if let Some(psig) = &param.fn_sig {
                self.compile_fnptr_value(arg, psig)?
            } else {
                self.compile_expr(arg)?
            };
            arg_cells.push(self.duplicate_value(v));
        }
        // Candidate list first: borrowing `self.functions`
        // while expanding would conflict with `&mut self`.
        // Functions past the 255 ID cap are directly callable
        // only; their IDs wrap, so they never match here.
        let mut candidates: Vec<(usize, Vec<u8>, Rc<ExpandedFunction>)> = Vec::new();
        for (idx, fname) in self.fn_order.clone().into_iter().enumerate() {
            if idx >= 255 {
                continue;
            }
            if let Some(def) = self.functions.get(&fname) {
                let def_sig = FuncSig {
                    is_void: def.is_void,
                    returns_ptr_depth: def.returns_ptr_depth,
                    params: def.params.clone(),
                };
                if def_sig.matches(sig) {
                    candidates.push((idx, fname, Rc::clone(def)));
                }
            }
        }
        for (idx, fname, def) in candidates {
            let id_const = self.const_value((idx + 1) as u8);
            let guard = self.is_equal(id_cell, id_const)?;
            let arg_cells = arg_cells.clone();
            self.perform_if(guard, move |em| {
                let checkpoint = em.checkpoint();
                em.expand_call_body(&fname, &def, &arg_cells, result, returned, checkpoint)
            })?;
        }
        Ok(result)
    }

    /// (ID cell, signature) when `callee` is a dereference /
    /// address / parenthesis chain over a function designator
    /// or function pointer, or `None` when it is some other
    /// expression (the caller reports it unsupported).
    fn indirect_callee(
        &mut self,
        callee: &Expr,
    ) -> Result<Option<(i32, FuncSig)>, CompileError> {
        let mut cur = callee;
        loop {
            match cur {
                Expr::Paren(inner) => cur = inner.as_ref(),
                Expr::Unary { op, e: inner }
                    if op.as_slice() == b"*" || op.as_slice() == b"&" =>
                {
                    cur = inner.as_ref()
                }
                _ => break,
            }
        }
        match cur {
            Expr::Ident(_) => Ok(Some(self.fn_designator_chain(callee)?)),
            _ => Ok(None),
        }
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

/// SPIKE (goto): where one label or `goto` sits in a function body:
/// the enclosing loops and `if` arms (as node ids), whether it is
/// inside a `switch`, and its source order among labels/gotos.
#[derive(Clone, Default)]
struct GotoSite {
    order: usize,
    loops: HashSet<usize>,
    arms: HashSet<usize>,
    in_switch: bool,
}

#[allow(clippy::too_many_arguments)]
fn collect_goto_sites_stmt(
    s: &Stmt,
    loops: &mut Vec<usize>,
    arms: &mut Vec<usize>,
    in_switch: bool,
    next_id: &mut usize,
    order: &mut usize,
    labels: &mut HashMap<Vec<u8>, GotoSite>,
    label_order: &mut Vec<Vec<u8>>,
    gotos: &mut Vec<(Vec<u8>, GotoSite)>,
) -> Result<(), CompileError> {
    let site = |loops: &[usize], arms: &[usize], in_switch: bool, order: usize| GotoSite {
        order,
        loops: loops.iter().copied().collect(),
        arms: arms.iter().copied().collect(),
        in_switch,
    };
    match s {
        Stmt::Goto { name } => {
            gotos.push((name.clone(), site(loops, arms, in_switch, *order)));
            *order += 1;
            Ok(())
        }
        Stmt::GotoLabel { name, body } => {
            if labels.contains_key(name) {
                return Err(err(format!(
                    "duplicate label '{}'",
                    String::from_utf8_lossy(name)
                )));
            }
            labels.insert(name.clone(), site(loops, arms, in_switch, *order));
            label_order.push(name.clone());
            *order += 1;
            collect_goto_sites_stmt(body, loops, arms, in_switch, next_id, order, labels, label_order, gotos)
        }
        Stmt::Compound(b) => collect_goto_sites_block(
            b, loops, arms, in_switch, next_id, order, labels, label_order, gotos,
        ),
        Stmt::If { then, els, .. } => {
            let arm = *next_id;
            *next_id += 1;
            arms.push(arm);
            collect_goto_sites_stmt(then, loops, arms, in_switch, next_id, order, labels, label_order, gotos)?;
            arms.pop();
            if let Some(e) = els {
                let arm = *next_id;
                *next_id += 1;
                arms.push(arm);
                collect_goto_sites_stmt(e, loops, arms, in_switch, next_id, order, labels, label_order, gotos)?;
                arms.pop();
            }
            Ok(())
        }
        Stmt::Switch { body, .. } => {
            collect_goto_sites_stmt(body, loops, arms, true, next_id, order, labels, label_order, gotos)
        }
        Stmt::While { body, .. } | Stmt::DoWhile { body, .. } => {
            let id = *next_id;
            *next_id += 1;
            loops.push(id);
            let r = collect_goto_sites_stmt(body, loops, arms, in_switch, next_id, order, labels, label_order, gotos);
            loops.pop();
            r
        }
        Stmt::For { body, .. } => {
            let id = *next_id;
            *next_id += 1;
            loops.push(id);
            let r = collect_goto_sites_stmt(body, loops, arms, in_switch, next_id, order, labels, label_order, gotos);
            loops.pop();
            r
        }
        Stmt::Case { body, .. } | Stmt::Default { body } => {
            collect_goto_sites_stmt(body, loops, arms, in_switch, next_id, order, labels, label_order, gotos)
        }
        _ => Ok(()),
    }
}

fn collect_goto_sites_block(
    b: &Block,
    loops: &mut Vec<usize>,
    arms: &mut Vec<usize>,
    in_switch: bool,
    next_id: &mut usize,
    order: &mut usize,
    labels: &mut HashMap<Vec<u8>, GotoSite>,
    label_order: &mut Vec<Vec<u8>>,
    gotos: &mut Vec<(Vec<u8>, GotoSite)>,
) -> Result<(), CompileError> {
    for item in &b.0 {
        if let BlockItem::Stmt(s) = item {
            collect_goto_sites_stmt(s, loops, arms, in_switch, next_id, order, labels, label_order, gotos)?;
        }
    }
    Ok(())
}

/// SPIKE (goto): the forward-only subset this spike measures.
/// Labels are function-scoped. A `goto` is accepted only when its
/// label is defined later in the same function, is not inside a
/// `switch`, is not inside a loop or `if` arm the `goto` is
/// outside of (jumping in would bypass the loop/branch guard),
/// and the `goto` itself is not inside a `switch`. Everything
/// else is a compile error with its own message. Returns label
/// names in first-seen order, for flag allocation.
fn validate_gotos(body: &Block) -> Result<Vec<Vec<u8>>, CompileError> {
    let mut labels: HashMap<Vec<u8>, GotoSite> = HashMap::new();
    let mut label_order: Vec<Vec<u8>> = Vec::new();
    let mut gotos: Vec<(Vec<u8>, GotoSite)> = Vec::new();
    let mut order = 0usize;
    let mut next_id = 0usize;
    collect_goto_sites_block(
        body,
        &mut Vec::new(),
        &mut Vec::new(),
        false,
        &mut next_id,
        &mut order,
        &mut labels,
        &mut label_order,
        &mut gotos,
    )?;
    for (name, site) in &gotos {
        let Some(label) = labels.get(name) else {
            return Err(err(format!(
                "undefined label '{}'",
                String::from_utf8_lossy(name)
            )));
        };
        if site.in_switch || label.in_switch {
            return Err(err("goto in switch is not supported".to_string()));
        }
        if !label.loops.is_subset(&site.loops) {
            return Err(err(format!(
                "goto into a loop to '{}' is not supported",
                String::from_utf8_lossy(name)
            )));
        }
        if !label.arms.is_subset(&site.arms) {
            return Err(err(format!(
                "goto into an if branch to '{}' is not supported",
                String::from_utf8_lossy(name)
            )));
        }
        if label.order <= site.order {
            return Err(err(format!(
                "backward goto to '{}' is not supported",
                String::from_utf8_lossy(name)
            )));
        }
    }
    Ok(label_order)
}

/// `goto` target names anywhere under `s`.
fn goto_targets_in(s: &Stmt, out: &mut HashSet<Vec<u8>>) {
    match s {
        Stmt::Goto { name } => {
            out.insert(name.clone());
        }
        Stmt::Compound(b) => {
            for item in &b.0 {
                if let BlockItem::Stmt(s) = item {
                    goto_targets_in(s, out);
                }
            }
        }
        Stmt::If { then, els, .. } => {
            goto_targets_in(then, out);
            if let Some(e) = els {
                goto_targets_in(e, out);
            }
        }
        Stmt::Switch { body, .. }
        | Stmt::While { body, .. }
        | Stmt::DoWhile { body, .. }
        | Stmt::For { body, .. }
        | Stmt::Case { body, .. }
        | Stmt::Default { body }
        | Stmt::GotoLabel { body, .. } => goto_targets_in(body, out),
        _ => {}
    }
}

/// Label names defined anywhere under `s`.
fn goto_labels_in(s: &Stmt, out: &mut HashSet<Vec<u8>>) {
    match s {
        Stmt::GotoLabel { name, body } => {
            out.insert(name.clone());
            goto_labels_in(body, out);
        }
        Stmt::Compound(b) => {
            for item in &b.0 {
                if let BlockItem::Stmt(s) = item {
                    goto_labels_in(s, out);
                }
            }
        }
        Stmt::If { then, els, .. } => {
            goto_labels_in(then, out);
            if let Some(e) = els {
                goto_labels_in(e, out);
            }
        }
        Stmt::Switch { body, .. }
        | Stmt::While { body, .. }
        | Stmt::DoWhile { body, .. }
        | Stmt::For { body, .. }
        | Stmt::Case { body, .. }
        | Stmt::Default { body } => goto_labels_in(body, out),
        _ => {}
    }
}

/// Skip a length modifier (`hh`, `h`, `ll`, `l`, `j`, `z`, `t`)
/// in a format string, advancing `i` past it. Lengths are no-ops
/// here (every value is one byte); flags, widths, and precisions
/// are not skipped and still fail as unexpected specifiers.
fn skip_length_modifiers(format: &[u8], i: &mut usize) {
    match format.get(*i).copied() {
        Some(c @ (b'h' | b'l')) => {
            *i += 1;
            if format.get(*i).copied() == Some(c) {
                *i += 1;
            }
        }
        Some(b'j' | b'z' | b't') => *i += 1,
        _ => {}
    }
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

    // SPIKE (goto): forward jumps, cleanup jumps out of an `if`,
    // exits out of a loop, and a forward jump inside a loop body
    // all run through the real runner; the rejected shapes each
    // fail with their own message.
    #[test]
    fn goto_forward_skips_statements() {
        let bf = compile(
            b"int main() { int x = 1; goto done; x = 2; done: printf(\"%d\", x); }",
        )
        .expect("forward goto compiles");
        let mut out = Vec::new();
        crate::runner::run(&bf, &mut std::io::empty(), &mut out).expect("bf runs");
        assert_eq!(out, b"1");
    }

    #[test]
    fn goto_cleanup_out_of_if_and_loop() {
        let bf = compile(
            b"int main() { int x = 0; if (1) { goto done; } x = 5; done: printf(\"%d,\", x); int i = 0; while (i < 10) { i = i + 1; if (i == 3) { goto out; } } out: printf(\"%d\", i); }",
        )
        .expect("cleanup and loop-exit gotos compile");
        let mut out = Vec::new();
        crate::runner::run(&bf, &mut std::io::empty(), &mut out).expect("bf runs");
        assert_eq!(out, b"0,3");
    }

    #[test]
    fn goto_forward_inside_loop_body() {
        let bf = compile(
            b"int main() { int i = 0; int s = 0; while (i < 3) { i = i + 1; goto add; s = 99; add: s = s + 1; } printf(\"%d,%d\", i, s); }",
        )
        .expect("goto inside loop body compiles");
        let mut out = Vec::new();
        crate::runner::run(&bf, &mut std::io::empty(), &mut out).expect("bf runs");
        assert_eq!(out, b"3,3");
    }

    #[test]
    fn goto_skips_initializer() {
        // Jumping over an initializer leaves the cell zero: the
        // declaration's init emission is guarded by the pending
        // flag. No g++-oracle fixture can pin this — jumping over
        // an initialization is an error in C++ — so the runner
        // pins it here: without the declaration guard the
        // output would be `5`.
        let bf = compile(
            b"int main() { int x = 0; goto done; int y = 5; done: printf(\"%d\", x + y); }",
        )
        .expect("goto over initializer compiles");
        let mut out = Vec::new();
        crate::runner::run(&bf, &mut std::io::empty(), &mut out).expect("bf runs");
        assert_eq!(out, b"0");
    }

    #[test]
    fn goto_rejections() {
        for (src, msg) in [
            (
                "int main() { goto nowhere; }",
                "undefined label 'nowhere'",
            ),
            (
                "int main() { done: printf(\"a\"); goto done; }",
                "backward goto to 'done' is not supported",
            ),
            (
                "int main() { goto in; while (1) { in: printf(\"x\"); break; } }",
                "goto into a loop to 'in' is not supported",
            ),
            (
                "int main() { goto in; if (1) { in: printf(\"x\"); } }",
                "goto into an if branch to 'in' is not supported",
            ),
            (
                "int main() { int x = 1; switch (x) { case 1: goto out; } out: printf(\"x\"); }",
                "goto in switch is not supported",
            ),
            (
                "int main() { a: printf(\"a\"); { a: printf(\"b\"); } }",
                "duplicate label 'a'",
            ),
        ] {
            let e = compile(src.as_bytes()).expect_err("goto shape is rejected");
            assert!(
                e.msg.contains(msg),
                "expected `{msg}` in `{}` for {src}",
                e.msg
            );
        }
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
    fn direct_pointer_return_runs() {
        // The direct declarator form carries the same signature
        // as the typedef-alias form: a prototyped pointer return
        // agrees with its definition, and the returned address
        // is usable through `%s` and dereference.
        let bf = compile(
            b"char buf[3] = \"hi\"; \
              char *get(void); \
              char *get(void) { return buf; } \
              char *other() { return &buf[0]; } \
              int main() { char *p = get(); printf(\"%s %s %d\", p, other(), *get()); }",
        )
        .expect("pointer-return program compiles");
        let mut out = Vec::new();
        crate::runner::run(&bf, &mut std::io::empty(), &mut out).expect("bf runs");
        assert_eq!(out, b"hi hi 104");
        // A scalar definition conflicts with a pointer-return
        // prototype spelled directly.
        let err = compile(b"char *f(); int f() { return 0; } int main() {}")
            .expect_err("pointer-return conflict");
        assert_eq!(err.msg, "conflicting prototype for function 'f'");
        // And a scalar prototype conflicts with a pointer-return
        // definition.
        let err = compile(b"int f(); char *f() { return 0; } int main() {}")
            .expect_err("scalar-prototype conflict");
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
    fn pointer_to_pointer_runs() {
        // Two levels of indirection compose through the same
        // dispatch: `**pp` reads and writes the ultimate
        // target, `(*pp)[k]` and `pp[0][k]` step through the
        // loaded pointer, and a `**` return carries an address
        // through a call.
        let bf = compile(
            b"int **getpp(int **q) { return q; } \
              int main() { \
                int x = 10; int *p = &x; int **pp = &p; \
                printf(\"%d,\", **pp); \
                **pp = 42; \
                printf(\"%d,\", x); \
                int a[3] = {1, 2, 3}; int *q = a; int **qp = &q; \
                printf(\"%d,%d,\", (*qp)[1], qp[0][2]); \
                qp[0] = &a[2]; \
                printf(\"%d,\", *q); \
                printf(\"%d\", **getpp(pp)); \
              }",
        )
        .expect("pointer-to-pointer program compiles");
        let mut out = Vec::new();
        crate::runner::run(&bf, &mut std::io::empty(), &mut out).expect("bf runs");
        assert_eq!(out, b"10,42,2,3,3,42");
        // Return depth is part of the signature: depth 1 and
        // depth 2 disagree in either direction.
        let err = compile(b"int **f(); int *f() { return 0; } int main() {}")
            .expect_err("return-depth conflict");
        assert_eq!(err.msg, "conflicting prototype for function 'f'");
        let err = compile(b"int f(int **q); int f(int *q) { return 0; } int main() {}")
            .expect_err("param-depth conflict");
        assert_eq!(err.msg, "conflicting prototype for function 'f'");
    }

    #[test]
    fn multi_dimensional_arrays_run() {
        // Nested and flat lists, unsized outer dimensions,
        // typedef-composed dimensions, row decay, a
        // multi-dimensional parameter, and `sizeof` of the
        // whole array and of a row.
        let bf = compile(
            b"typedef int Row[3]; \
              int get(int m[][3], int i, int j) { return m[i][j]; } \
              int main() { \
                int a[2][3] = {{1, 2, 3}, {4, 5, 6}}; \
                int b[2][3] = {7, 8, 9, 10}; \
                int c[][2] = {{1, 2}, {3, 4}, {5, 6}}; \
                Row g[2] = {{9, 8, 7}, {6, 5, 4}}; \
                int i = 1; int j = 2; \
                int *p = a[1]; \
                printf(\"%d,%d,%d,%d,\", a[0][0], a[1][2], a[i][j], b[1][1]); \
                printf(\"%d,%d,%d,\", c[2][1], c[0][1], g[1][0]); \
                printf(\"%d,%d,\", sizeof(a), sizeof(a[0])); \
                printf(\"%d,\", get(a, 1, 0)); \
                p[2] = 99; \
                printf(\"%d,\", a[1][2]); \
                int *q = *a; \
                printf(\"%d\", *q); \
              }",
        )
        .expect("multi-dimensional program compiles");
        let mut out = Vec::new();
        crate::runner::run(&bf, &mut std::io::empty(), &mut out).expect("bf runs");
        assert_eq!(out, b"1,6,6,0,6,2,6,6,3,4,99,1");
    }

    #[test]
    fn assignment_yields_stored_value() {
        // Each assignment contributes the value it stored,
        // even when the same target is assigned again later
        // in the same expression: the result must be a copy,
        // not the target cell read after both assignments.
        for (src, want) in [
            (
                b"int main() { int x = 0; printf(\"%d\", (x = 1) + (x = 2)); }"
                    as &[u8],
                b"3" as &[u8],
            ),
            (
                b"int main() { int x = 1; int a[2]; printf(\"%d,%d\", (a[0] = x) + (x = 5), a[0]); }",
                b"6,1",
            ),
            (
                b"int main() { int x = 0; int *p = &x; printf(\"%d\", (*p = 3) + (x = 4)); }",
                b"7",
            ),
        ] {
            let bf = compile(src).expect("assignment program compiles");
            let mut out = Vec::new();
            crate::runner::run(&bf, &mut std::io::empty(), &mut out).expect("bf runs");
            assert_eq!(out, want, "for {:?}", src);
        }
    }

    #[test]
    fn function_pointers_run() {
        // Callee IDs dispatch on the declared signature for
        // both call spellings, dereference chains collapse,
        // null calls yield zero, and designators compare by
        // identity. A `**` parameter receives an ID the same
        // way a variable does.
        let bf = compile(
            b"int dbl(int x) { return x * 2; } \
              int tri(int x) { return x * 3; } \
              void inc(int *p) { *p = *p + 1; } \
              int apply(int (*cb)(int), int x) { return cb(x) + (*cb)(x); } \
              int main() { \
                int (*fp)(int) = dbl; \
                printf(\"%d,\", fp(4)); \
                printf(\"%d,\", (*fp)(4)); \
                printf(\"%d,\", (**fp)(4)); \
                fp = tri; \
                printf(\"%d,\", fp(4)); \
                printf(\"%d,\", apply(fp, 5)); \
                printf(\"%d,\", fp == tri); \
                printf(\"%d,\", fp == dbl); \
                printf(\"%d,\", fp == 0); \
                fp = 0; \
                printf(\"%d,\", fp(4)); \
                printf(\"%d,\", !fp); \
                int x = 10; \
                void (*vp)(int *) = inc; \
                vp(&x); \
                printf(\"%d,\", x); \
                printf(\"%d\", apply(dbl, 3)); \
              }",
        )
        .expect("function-pointer program compiles");
        let mut out = Vec::new();
        crate::runner::run(&bf, &mut std::io::empty(), &mut out).expect("bf runs");
        assert_eq!(out, b"8,8,8,12,30,1,0,0,0,1,11,12");
    }

    #[test]
    fn function_pointer_errors() {
        for (src, want) in [
            (
                b"int g(int x) { return x; } int main() { int (*fp)(int, int) = g; return 0; }"
                    as &[u8],
                "incompatible function pointer type",
            ),
            (
                // Note: a return operand in `main` is never
                // compiled (documented at `Stmt::Return`), so
                // the call sits in a printf argument, where it
                // is compiled.
                b"int g(int x) { return x; } int main() { int (*fp)(int) = g; printf(\"%d\", fp(1, 2)); return 0; }",
                "wrong number of arguments in indirect call",
            ),
            (
                b"int g(int x) { return x; } int main() { int (*fp)(int) = g; fp++; return 0; }",
                "arithmetic on function pointers is not supported",
            ),
            (
                b"int g(int x) { return x; } int main() { int (*fp)(int) = g; fp += 1; return 0; }",
                "arithmetic on function pointers is not supported",
            ),
            (
                b"int g(int x) { return x; } int main() { int (*fp)(int) = g; *fp = g; return 0; }",
                "cannot assign to a function",
            ),
            (
                b"int g(int); int main() { int (*fp)(int) = g; return 0; }",
                "undefined function 'g'",
            ),
            (
                b"int g(int x) { return x; } int main() { int (*fp)(int) = 5; return 0; }",
                "incompatible function pointer type",
            ),
            (
                b"int g(int x) { return x; } int main() { g = 1; return 0; }",
                "cannot assign to a function",
            ),
        ] {
            let err = compile(src).expect_err("function-pointer misuse");
            assert_eq!(err.msg, want, "for {:?}", src);
        }
        // Recursion through a pointer is rejected like direct
        // recursion: expanding `f` again finds it already
        // expanding.
        let err = compile(
            b"int f(int x) { int (*fp)(int) = f; return fp(x); } int main() { printf(\"%d\", f(1)); return 0; }",
        )
        .expect_err("recursive indirect call");
        assert!(
            err.msg.contains("recursive call to function 'f'"),
            "unexpected error: {}",
            err.msg
        );
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
    fn storage_classes_compile_and_run() {
        let bf = compile(
            b"static int counter = 10; extern int shared; int shared = 7; \
              static int get(void) { return counter; } \
              int main() { \
                register int r = 3; auto int a = 4; \
                printf(\"%d,%d,%d,%d\", counter, shared, get(), r + a); \
              }",
        )
        .expect("storage program compiles");
        let mut out = Vec::new();
        crate::runner::run(&bf, &mut std::io::empty(), &mut out).expect("bf runs");
        assert_eq!(out, b"10,7,10,7");
    }

    #[test]
    fn extern_undefined_is_an_error() {
        // A bare `extern` declaration defines nothing: using
        // the name without a definition is an undefined
        // reference, in every use position (value, address,
        // store, subscript, dereference). g++ would not link
        // these either, so they are pinned here, not in a
        // fixture.
        for (src, want) in [
            (
                b"extern int x; int main() { printf(\"%d\", x); }"
                    as &[u8],
                "undefined reference to 'x'",
            ),
            (
                b"extern int x; int main() { int *p = &x; return 0; }",
                "undefined reference to 'x'",
            ),
            (
                b"extern int x; int main() { x = 1; return 0; }",
                "undefined reference to 'x'",
            ),
            (
                b"extern char a[3]; int main() { printf(\"%d\", a[0]); }",
                "undefined reference to 'a'",
            ),
            (
                b"extern int x; int main() { printf(\"%d\", *x); }",
                "undefined reference to 'x'",
            ),
        ] {
            let err = compile(src).expect_err("undefined extern use");
            assert_eq!(err.msg, want, "for {:?}", src);
        }
        // A later definition satisfies the declaration, in
        // either order, and repeated bare declarations are
        // fine when unused. `sizeof` does not count as a use:
        // the operand is unevaluated. A use inside a function
        // that is never called is never compiled, so it does
        // not fail either.
        let bf = compile(b"extern int x; int x = 9; int main() { printf(\"%d\", x); }")
            .expect("extern-then-define compiles");
        let mut out = Vec::new();
        crate::runner::run(&bf, &mut std::io::empty(), &mut out).expect("bf runs");
        assert_eq!(out, b"9");
        let bf = compile(b"int x = 8; extern int x; int main() { printf(\"%d\", x); }")
            .expect("define-then-extern compiles");
        let mut out = Vec::new();
        crate::runner::run(&bf, &mut std::io::empty(), &mut out).expect("bf runs");
        assert_eq!(out, b"8");
        compile(b"extern int x; extern int x; int main() { return 0; }")
            .expect("repeated extern declarations compile");
        let bf = compile(b"extern int x; int main() { printf(\"%d\", sizeof(x)); }")
            .expect("sizeof of extern compiles");
        let mut out = Vec::new();
        crate::runner::run(&bf, &mut std::io::empty(), &mut out).expect("bf runs");
        assert_eq!(out, b"1");
        compile(b"extern int x; int f() { return x; } int main() { return 0; }")
            .expect("use in uncalled function is never compiled");
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
    fn braced_initializers_compile_and_run() {
        // Sized lists zero-fill; unsized lists size by count;
        // elements may be arbitrary expressions evaluated in
        // source order; a one-string list is a string init.
        let bf = compile(
            b"char g[4] = {1, 2}; char h[] = {3, 4, 5}; char s[] = {\"hi\"}; \
              int main() { \
                int x = {7}; \
                char a[3] = {x, x + 1, 0}; \
                char t[] = {'a', 'b'}; \
                printf(\"%d,%d,%d,%d,%d,%d,%d,%d,%s,%d,%d\", \
                  g[0], g[1], g[2], g[3], h[0], h[2], x, a[1], s, t[0], t[1]); \
              }",
        )
        .expect("braced program compiles");
        let mut out = Vec::new();
        crate::runner::run(&bf, &mut std::io::empty(), &mut out).expect("bf runs");
        assert_eq!(out, b"1,2,0,0,3,5,7,8,hi,97,98");
        // Lists rename inside function bodies: elements may name
        // parameters and locals.
        let bf = compile(
            b"int f(int q) { int a[2] = {q, q + 1}; return a[0] + a[1]; } \
              int main() { printf(\"%d\", f(10)); }",
        )
        .expect("braced function compiles");
        let mut out = Vec::new();
        crate::runner::run(&bf, &mut std::io::empty(), &mut out).expect("bf runs");
        assert_eq!(out, b"21");
        // A `for` scalar declarator takes a one-element list.
        let bf = compile(
            b"int main() { int s = 0; for (int i = {3}; i > 0; i = i - 1) { s = s + i; } printf(\"%d\", s); }",
        )
        .expect("braced for-decl compiles");
        let mut out = Vec::new();
        crate::runner::run(&bf, &mut std::io::empty(), &mut out).expect("bf runs");
        assert_eq!(out, b"6");
        // Redeclaring with a list re-fills the elements.
        let bf = compile(
            b"int main() { char a[2] = {1, 2}; a[0] = 9; char a[2] = {3, 4}; printf(\"%d%d\", a[0], a[1]); }",
        )
        .expect("redeclared list compiles");
        let mut out = Vec::new();
        crate::runner::run(&bf, &mut std::io::empty(), &mut out).expect("bf runs");
        assert_eq!(out, b"34");
    }

    #[test]
    fn braced_initializer_errors() {
        let cases: &[(&[u8], &str)] = &[
            (
                b"int main() { char a[2] = {1, 2, 3}; }",
                "initializer list is too long",
            ),
            (
                b"int main() { int x = {1, 2}; }",
                "scalar initializer list must contain exactly one element",
            ),
            (
                b"int main() { for (int i = {1, 2};;) { } return 0; }",
                "scalar initializer list must contain exactly one element",
            ),
            (
                b"int main() { char a[] = 5; }",
                "unsized array requires a string initializer",
            ),
            (
                b"int main() { char a[2] = 5; }",
                "array initializers are not supported",
            ),
        ];
        for (src, want) in cases {
            match compile(src) {
                Err(e) => assert_eq!(e.msg, *want, "for {:?}", src),
                Ok(_) => panic!("accepted {:?}", src),
            }
        }
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
                "too many subscripts",
            ),
            (
                b"int main() { int a[2][]; return 0; }",
                "only the first array dimension may be unsized",
            ),
            (
                b"int main() { char a[2][3] = \"hi\"; return 0; }",
                "string initializer is not supported for multi-dimensional arrays",
            ),
            (
                b"int main() { int a[2][2] = {{1, 2}, 3}; return 0; }",
                "mixed nested and flat initializer lists are not supported",
            ),
            (
                b"int main() { int a[2][2]; a[0] = 1; return 0; }",
                "cannot assign to an array",
            ),
            (
                b"int main() { int a[2][2][2]; a[0][1][2][3] = 1; return 0; }",
                "too many subscripts",
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
    fn io_formats_compile_and_run() {
        // Octal and uppercase-hex printing, plus skipped length
        // modifiers (every value is one byte).
        let bf = compile(
            b"int main() { printf(\"%o,%o,%o,%o,%X,%X,%X,%X,%ld,%hhd,%o\", 0, 7, 8, 255, 0, 10, 171, 255, 42, 200, 64); }",
        )
        .expect("octal/hex print compiles");
        let mut out = Vec::new();
        crate::runner::run(&bf, &mut std::io::empty(), &mut out).expect("bf runs");
        assert_eq!(out, b"0,7,10,377,0,A,AB,FF,42,200,100");
        // Based `scanf` conversions, including `0x` prefixes,
        // `%i` base detection, and signs.
        let bf = compile(
            b"int main() { int a = 0, b = 0, c = 0, d = 0, e = 0, f = 0, g = 0; \
              scanf(\"%x %o %i %i %i %x %u\", &a, &b, &c, &d, &e, &f, &g); \
              printf(\"%d,%d,%d,%d,%d,%d,%d\", a, b, c, d, e, f, g); }",
        )
        .expect("based scanf compiles");
        let mut out = Vec::new();
        let mut input = std::io::Cursor::new(b"ff 77 010 0x1f 0 -0x10 -5\n");
        crate::runner::run(&bf, &mut input, &mut out).expect("bf runs");
        assert_eq!(out, b"255,63,8,31,0,240,251");
        // `%i` on `09` reads the value C reads (0): the peeked
        // `9` is consumed here, unlike C, so no conversion
        // follows it in this check.
        let bf = compile(
            b"int main() { int a = 0; scanf(\"%i\", &a); printf(\"%d\", a); }",
        )
        .expect("auto-base prefix compiles");
        let mut out = Vec::new();
        let mut input = std::io::Cursor::new(b"09\n");
        crate::runner::run(&bf, &mut input, &mut out).expect("bf runs");
        assert_eq!(out, b"0");
        // Uppercase hex letters and a negative octal value.
        let bf = compile(
            b"int main() { int a = 0, b = 0; scanf(\"%x %o\", &a, &b); printf(\"%d,%d\", a, b); }",
        )
        .expect("uppercase scanf compiles");
        let mut out = Vec::new();
        let mut input = std::io::Cursor::new(b"AB -17\n");
        crate::runner::run(&bf, &mut input, &mut out).expect("bf runs");
        assert_eq!(out, b"171,241");
        // Length modifiers on `scanf`, and assignment
        // suppression for numbers and strings.
        let bf = compile(
            b"int main() { int a = 0, b = 0; scanf(\"%ld %hhd\", &a, &b); printf(\"%d,%d\", a, b); }",
        )
        .expect("modifier scanf compiles");
        let mut out = Vec::new();
        let mut input = std::io::Cursor::new(b"12 34\n");
        crate::runner::run(&bf, &mut input, &mut out).expect("bf runs");
        assert_eq!(out, b"12,34");
        let bf = compile(
            b"int main() { int a = 0; char buf[6]; scanf(\"%*d %d %*s %s\", &a, buf); printf(\"%d,%s\", a, buf); }",
        )
        .expect("suppression scanf compiles");
        let mut out = Vec::new();
        let mut input = std::io::Cursor::new(b"99 42 junk hello\n");
        crate::runner::run(&bf, &mut input, &mut out).expect("bf runs");
        assert_eq!(out, b"42,hello");
    }

    #[test]
    fn scanf_numbers_end_at_eof() {
        // Numbers ending exactly at end of input (no trailing
        // newline) used to spin forever: the reader re-read its
        // last digit, since `,` leaves the cell unchanged at
        // EOF. The zero-sentinel reads terminate them with the
        // accumulated value. Inputs here deliberately have no
        // trailing newline.
        let run = |src: &[u8], input: &[u8]| {
            let bf = compile(src).expect("scanf program compiles");
            let mut out = Vec::new();
            let mut input = std::io::Cursor::new(input);
            crate::runner::run(&bf, &mut input, &mut out).expect("bf runs");
            out
        };
        assert_eq!(
            run(
                b"int main() { int a = 0; scanf(\"%d\", &a); printf(\"%d\", a); }",
                b"20",
            ),
            b"20"
        );
        assert_eq!(
            run(
                b"int main() { int a = 0; scanf(\"%d\", &a); printf(\"%d\", a); }",
                b"-5",
            ),
            b"251"
        );
        assert_eq!(
            run(
                b"int main() { int a = 0; scanf(\"%d\", &a); printf(\"%d\", a); }",
                b"  42",
            ),
            b"42"
        );
        assert_eq!(
            run(
                b"int main() { int a = 0; scanf(\"%d\", &a); printf(\"%d\", a); }",
                b"",
            ),
            b"0"
        );
        assert_eq!(
            run(
                b"int main() { int a = 0; int b = 0; scanf(\"%d %d\", &a, &b); printf(\"%d,%d\", a, b); }",
                b"20 7",
            ),
            b"20,7"
        );
        assert_eq!(
            run(
                b"int main() { int a = 0; scanf(\"%i\", &a); printf(\"%d\", a); }",
                b"0x1f",
            ),
            b"31"
        );
        assert_eq!(
            run(
                b"int main() { int a = 0; scanf(\"%o\", &a); printf(\"%d\", a); }",
                b"17",
            ),
            b"15"
        );
        assert_eq!(
            run(
                b"int main() { int a = 0; scanf(\"%x\", &a); printf(\"%d\", a); }",
                b"1f",
            ),
            b"31"
        );
        assert_eq!(
            run(
                b"int main() { int a = 0; char s[8] = \"\"; scanf(\"%d\", &a); scanf(\"%s\", s); printf(\"%d,%s\", a, s); }",
                b"20 hello",
            ),
            b"20,hello"
        );
    }

    #[test]
    fn io_format_errors() {
        let cases: &[(&[u8], &str)] = &[
            (b"int main() { printf(\"%5d\", 1); }", "Unexpected printf format"),
            (b"int main() { printf(\"%e\", 1); }", "Unexpected printf format"),
            (b"int main() { printf(\"%p\", 1); }", "Unexpected printf format"),
            (
                b"int main() { int a = 0; scanf(\"%5d\", &a); }",
                "Unexpected scanf format",
            ),
            (
                b"int main() { int a = 0; scanf(\"%e\", &a); }",
                "Unexpected scanf format",
            ),
            (
                b"int main() { int a = 0; scanf(\"x\", &a); }",
                "Unexpected scanf format",
            ),
        ];
        for (src, want) in cases {
            match compile(src) {
                Err(e) => assert_eq!(e.msg, *want, "for {:?}", src),
                Ok(_) => panic!("accepted {:?}", src),
            }
        }
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
    fn string_literal_straddling_255_errors() {
        // A literal whose NUL would land past cell 255 used to
        // compile, and its print walk then wrapped to cell 0
        // and read unrelated tape (fuzz seed 65). Pad sizes
        // leave margin for prologue cells before the first
        // declaration, so the straddling case errors and the
        // fitting case compiles regardless of the exact base.
        assert_eq!(
            compile(b"int main() { char pad[252] = {0}; printf(\"%s\", \"abcd\"); }")
                .unwrap_err()
                .msg,
            "address is outside the addressable range"
        );
        let bf = compile(b"int main() { char pad[240] = {0}; printf(\"%s\", \"abcd\"); }")
            .expect("fitting literal compiles");
        let mut out = Vec::new();
        crate::runner::run(&bf, &mut std::io::empty(), &mut out).expect("bf runs");
        assert_eq!(out, b"abcd");
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
