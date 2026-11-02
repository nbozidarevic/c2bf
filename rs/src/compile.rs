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

use crate::ast::{Block, BlockItem, Decl, Expr, External, ForDecl, ForInit, Stmt};
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

/// Parse and compile `src` to Brainfuck plus trailing newline.
///
/// Mirrors `BrainfuckCompiler::run()`: externals compile in source order
/// and a final newline is appended. Parse failures surface as
/// `CompileError` carrying the parse diagnostic.
pub fn compile(src: &[u8]) -> Result<Vec<u8>, CompileError> {
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
    let mut out = em.output().to_vec();
    out.push(b'\n');
    Ok(out)
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
            let e = match &init.init {
                None => return Err(err("Unsupported init declarator")),
                Some(e) => e,
            };
            let dest = self.memory.var_cell(&init.name);
            let src = self.compile_expr(e)?;
            self.copy_value(src, dest);
        }
        Ok(())
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
            Expr::Ident(name) => Ok(self.memory.var_cell(name)),
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
                    // GCC right-to-left: the target (last argument) first.
                    let target = self.compile_lvalue(lhs)?;
                    let r = self.compile_expr(rhs)?;
                    Ok(self.copy_value(r, target))
                } else {
                    // Sequenced statements in C++: right-hand side first.
                    let r = self.compile_expr(rhs)?;
                    let target = self.compile_lvalue(lhs)?;
                    let res = match op.as_slice() {
                        b"+=" => self.add_values(target, r),
                        b"-=" => self.subtract_values(target, r),
                        b"*=" => self.multiply_values(target, r),
                        b"/=" => self.divide_values(target, r)?,
                        b"%=" => self.mod_values(target, r)?,
                        b"&=" => self.bitwise_and_values(target, r)?,
                        b"|=" => self.bitwise_or_values(target, r)?,
                        b"^=" => self.bitwise_xor_values(target, r)?,
                        b"<<=" => self.shift_left_values(target, r)?,
                        b">>=" => self.shift_right_values(target, r)?,
                        _ => return Err(err("Unsupported assignment expression")),
                    };
                    Ok(self.copy_value(res, target))
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
                b"&" => self.compile_expr(e),
                b"++" => {
                    let t = self.compile_expr(e)?;
                    let one = self.const_value(1);
                    let sum = self.add_values(t, one);
                    self.copy_value(sum, t);
                    Ok(self.duplicate_value(t))
                }
                b"--" => {
                    let t = self.compile_expr(e)?;
                    let one = self.const_value(1);
                    let diff = self.subtract_values(t, one);
                    self.copy_value(diff, t);
                    Ok(self.duplicate_value(t))
                }
                _ => Err(err("Unsupported unary expression")),
            },
            // Casts are no-ops: every value is already a byte, and
            // literals and operations already truncate.
            Expr::Cast { e, .. } => self.compile_expr(e),
            // sizeof is always 1: every type is one byte here. The
            // operand is never compiled (unevaluated, as in C).
            Expr::Sizeof(_) | Expr::SizeofType(_) => Ok(self.const_value(1)),
            Expr::Postfix { op, e } => {
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
        }
    }

    /// Compile an assignment target. The C++ performs no lvalue check;
    /// our frontend rejects invalid targets at parse time, so anything
    /// reaching here is an identifier or parenthesized lvalue.
    fn compile_lvalue(&mut self, e: &Expr) -> Result<i32, CompileError> {
        match e {
            Expr::Ident(name) => Ok(self.memory.var_cell(name)),
            Expr::Paren(inner) => self.compile_lvalue(inner),
            _ => Err(err("invalid lvalue in assignment")),
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
        let mut vals: Vec<i32> = Vec::new();
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
            } else {
                vals.push(self.compile_expr(a)?);
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
                        let cell = next_format_arg(&vals, &mut next_var)?;
                        match format.get(i).copied() {
                            Some(b'c') => self.read_as_char(cell),
                            Some(b'd') => self.read_as_number(cell)?,
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

#[cfg(test)]
mod tests {
    use super::*;

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
