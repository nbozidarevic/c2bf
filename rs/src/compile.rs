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

use crate::ast::{Block, BlockItem, Decl, Expr, External, ForDecl, ForInit, InitDecl, Stmt};
use crate::codegen::{CompileError, Emitter};
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
    v.clamp(i32::MIN as i64, i32::MAX as i64) as i32
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
    /// Runtime address, searching `cells` in order (array range).
    Range { addr: i32, cells: Vec<i32> },
    /// Runtime address, searching every byte address (dereference
    /// or pointer subscript).
    Any { addr: i32 },
}

/// A `scanf` destination: a static cell, or a runtime address to
/// store through.
#[derive(Clone, Copy)]
enum ScanfTarget {
    Cell(i32),
    Through(i32),
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
    for item in &tu.0 {
        match item {
            External::FuncDef(f) => {
                if f.name.as_slice() != b"main" {
                    return Err(err("Unsupported function definition"));
                }
                em.compile_block(&f.body)?;
            }
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
            if init.array_len.is_some() {
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
        if init.init.is_some() {
            return Err(err("array initializers are not supported"));
        }
        let len_expr = init.array_len.as_ref().expect("array declarator");
        let len = match len_expr {
            Expr::Int(bytes) => int_literal_value(bytes),
            _ => return Err(err("array size must be an integer literal")),
        };
        if len <= 0 {
            return Err(err("array size must be positive"));
        }
        let len = len as usize;
        if let Some((base, old_len)) = self.memory.array_info(&init.name) {
            if old_len != len {
                return Err(err("array size mismatch on redeclaration"));
            }
            for cell in base..base + len as i32 {
                self.set_value(cell, 0);
            }
            return Ok(());
        }
        let base = self.memory.alloc_array(&init.name, len);
        for cell in base..base + len as i32 {
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
                        let addr = self.index_address(base, index)?;
                        let n = len.min(256) as i32;
                        Ok(StoreTarget::Range {
                            addr,
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
            StoreTarget::Range { addr, cells } => {
                self.store_to_cells(*addr, cells, value)?;
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
            StoreTarget::Range { addr, cells } => self.load_from_cells(*addr, cells)?,
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
        // Once a sibling may have executed `break` or `continue`, the rest
        // of the block must be skipped at runtime (a bare flag checked only
        // by the loop condition would let them run once more).
        let mut after_break = false;
        let mut after_continue = false;
        for item in &b.0 {
            match item {
                BlockItem::Decl(d) => {
                    if after_break || after_continue {
                        self.compile_decl_guarded(d)?
                    } else {
                        self.compile_decl(d)?
                    }
                }
                BlockItem::Stmt(s) => {
                    if after_break || after_continue {
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
                return self.perform_if_else(
                    nb,
                    |em| em.compile_stmt_guarded_continue(s),
                    |_| Ok(()),
                );
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
                return self.perform_if_else(nc, |em| em.compile_stmt(s), |_| Ok(()));
            }
        }
        self.compile_stmt(s)
    }

    /// Like `compile_stmt_guarded` for declarations.
    fn compile_decl_guarded(&mut self, d: &Decl) -> Result<(), CompileError> {
        if let Some(&flag) = self.break_stack.last() {
            if flag >= 0 {
                let nb = self.negate(flag)?;
                return self.perform_if_else(
                    nb,
                    |em| em.compile_decl_guarded_continue(d),
                    |_| Ok(()),
                );
            }
        }
        self.compile_decl_guarded_continue(d)
    }

    /// Continue half of `compile_decl_guarded`.
    fn compile_decl_guarded_continue(&mut self, d: &Decl) -> Result<(), CompileError> {
        if let Some(&flag) = self.continue_stack.last() {
            if flag >= 0 {
                let nc = self.negate(flag)?;
                return self.perform_if_else(nc, |em| em.compile_decl(d), |_| Ok(()));
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
                        em.and_not_broken(c, broken)
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
                        em.and_not_broken(c, broken)
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
                        em.and_not_broken(c, broken)
                    },
                    |em| {
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
                    },
                    |em| em.compile_loop_body(body, continued),
                );
                self.continue_stack.pop();
                self.break_stack.pop();
                r
            }
            // Return values are ignored: the C++ never compiles the operand.
            Stmt::Return(_) => Ok(()),
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
            let run_cond = self.and_not_continued(run_cond)?;
            self.perform_if_else(
                run_cond,
                |em| {
                    em.set_value(fall, 1);
                    // Same sibling rule as blocks: statements after a
                    // potential `break` or `continue` in this case are
                    // flag-guarded.
                    let mut after_break = false;
                    let mut after_continue = false;
                    for item in &c.body {
                        match item {
                            SwitchBodyItem::Decl(d) => {
                                if after_break || after_continue {
                                    em.compile_decl_guarded(d)?
                                } else {
                                    em.compile_decl(d)?
                                }
                            }
                            SwitchBodyItem::Stmt(s) => {
                                if after_break || after_continue {
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
            Expr::StringLit { .. } => Err(err("Unsupported string expression")),
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
                        let addr = self.index_address(base, index)?;
                        let n = len.min(256) as i32;
                        let cells: Vec<i32> = (cell_base..cell_base + n).collect();
                        self.load_from_cells(addr, &cells)
                    }
                }
                SubBase::Pointer(_) => {
                    let addr = self.index_address(base, index)?;
                    self.load_through(addr)
                }
            },
        }
    }

    /// Compile a `scanf` destination: a static cell for a variable,
    /// an array base, or a statically known element; otherwise a
    /// runtime address to store through. A bare pointer variable
    /// holds its address, so it stores through its own cell.
    fn compile_scanf_target(&mut self, a: &Expr) -> Result<ScanfTarget, CompileError> {
        match a {
            Expr::Ident(name) => {
                if let Some((base, _)) = self.memory.array_info(name) {
                    Ok(ScanfTarget::Cell(base))
                } else if self.memory.is_pointer(name) {
                    Ok(ScanfTarget::Through(self.memory.var_cell(name)))
                } else {
                    Ok(ScanfTarget::Cell(self.memory.var_cell(name)))
                }
            }
            Expr::Paren(inner) => self.compile_scanf_target(inner),
            Expr::Unary { op, e: inner } if op.as_slice() == b"&" => {
                if let Some(cell) = self.static_address_cell(inner) {
                    Ok(ScanfTarget::Cell(cell))
                } else {
                    Ok(ScanfTarget::Through(self.address_value(inner)?))
                }
            }
            _ => Err(err("scanf argument must be an address")),
        }
    }

    /// Read one `scanf` value (`%c` or `%d`) into a destination:
    /// directly for a static cell, through dispatch for a runtime
    /// address.
    fn read_into_target(&mut self, target: ScanfTarget, is_char: bool) -> Result<(), CompileError> {
        match target {
            ScanfTarget::Cell(cell) => {
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
                            Some(spec @ (b'c' | b'd' | b'u' | b'x')) => {
                                let cell = next_format_arg(&vals, &mut next_var)?;
                                match spec {
                                    b'c' => self.print_as_char(cell),
                                    b'x' => self.print_as_hex(cell)?,
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
            b"int main() { char a[3]; char *p = 0; a[0] = 1; a[1] = 2; a[2] = 3; p = &a[0]; (*p)++; printf(\"%d,\", a[0]); ++*p; printf(\"%d,\", a[0]); *p += 10; printf(\"%d,\", a[0]); *p--; printf(\"%d,\", a[0]); *p++ = 99; printf(\"%d,%d,\", a[0], *p); printf(\"%d\", a[1]); }",
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
                b"int main() { char f[260]; char a[3]; int i = 0; printf(\"%d\", a[i]); }",
                "address is outside the addressable range",
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
