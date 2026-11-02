//! Brainfuck code generation: emitter plus emission helpers.
//!
//! Ported from the retired C++ `BrainfuckCompiler`, helper-for-helper: same
//! allocation order, same pointer motion, same emitted bytes. The visitor
//! wiring (AST → helpers) lands in slice 4c.
//!
//! Notes on the port:
//! - `printAsDigit` is declared but never defined or called in C++;
//!   it is omitted here.
//! - The closures passed to `perform_while`/`perform_for` re-run their
//!   computation on every call (fresh cells each time), exactly like the
//!   C++ lambdas.
//! - Argument-evaluation order at multi-argument call sites is measured
//!   per site against GCC (order probe plus differentials): most sites are
//!   right-to-left, but binary-operator arithmetic operands are
//!   left-to-right (the `scanf` accumulate site excepted). Apple Clang
//!   differs per site too, so the reference
//!   output is platform-dependent; the byte differential is therefore
//!   Ubuntu-only (macOS CI stays behavioral).

use crate::memory::Memory;

/// Code emitter: cell allocator, output bytes, tracked data pointer.
#[derive(Debug, Default)]
pub struct Emitter {
    pub(crate) memory: Memory,
    output: Vec<u8>,
    pointer: i32,
    pub(crate) break_stack: Vec<i32>,
    pub(crate) continue_stack: Vec<i32>,
}

/// Compile failure: mirrors a C++ `throw "..."` site.
///
/// Message only, no position — the reference carries none either.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompileError {
    pub msg: String,
}

/// How [`Emitter::bitwise_fold`] combines one bit from each operand.
/// An enum (not raw op text) so the fold never has to defend against
/// an operator it does not implement.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BitCombine {
    And,
    Or,
    Xor,
}

impl std::fmt::Display for CompileError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.msg)
    }
}

impl std::error::Error for CompileError {}

impl Emitter {
    /// Fresh emitter; data pointer starts at cell 0.
    pub fn new() -> Self {
        Self::default()
    }

    /// Emitted bytes so far.
    pub fn output(&self) -> &[u8] {
        &self.output
    }

    fn emit(&mut self, s: &str) {
        self.output.extend_from_slice(s.as_bytes());
    }

    /// Emit pointer motion to `dest` (`movePointer`).
    pub fn move_pointer(&mut self, dest: i32) {
        if self.pointer == dest {
            return;
        }
        let c = if self.pointer > dest { b'<' } else { b'>' };
        for _ in 0..(self.pointer - dest).abs() {
            self.output.push(c);
        }
        self.pointer = dest;
    }

    /// Copy `source` to `destination`, preserving `source` (`copyValue`).
    pub fn copy_value(&mut self, source: i32, dest: i32) -> i32 {
        if source == dest {
            return dest;
        }
        let helper = self.memory.temp_cell();
        self.move_pointer(helper);
        self.emit("[-]");
        self.move_pointer(dest);
        self.emit("[-]");
        self.move_pointer(source);
        self.emit("[-");
        self.move_pointer(dest);
        self.emit("+");
        self.move_pointer(helper);
        self.emit("+");
        self.move_pointer(source);
        self.emit("]");
        self.move_pointer(helper);
        self.emit("[-");
        self.move_pointer(source);
        self.emit("+");
        self.move_pointer(helper);
        self.emit("]");
        dest
    }

    /// Move `source` to `destination`, zeroing `source` (`moveValue`).
    pub fn move_value(&mut self, source: i32, dest: i32) -> i32 {
        if source == dest {
            return dest;
        }
        self.move_pointer(dest);
        self.emit("[-]");
        self.move_pointer(source);
        self.emit("[-");
        self.move_pointer(dest);
        self.emit("+");
        self.move_pointer(source);
        self.emit("]");
        dest
    }

    /// Copy `source` into a fresh cell (`duplicateValue`).
    pub fn duplicate_value(&mut self, source: i32) -> i32 {
        let dest = self.memory.temp_cell();
        self.copy_value(source, dest);
        dest
    }

    /// Loop-based plan `(iterations, per_iteration, remainder)` for
    /// building `value` as `a * b + r`, or `None` when repeated `+`s
    /// are cheaper. `scratch_dist` is the distance from the destination
    /// cell to the scratch cell the loop would use.
    ///
    /// A loop build costs `9 + a + b + r + 4d` emitted bytes against
    /// `3 + value` direct (`d` = scratch distance), but executes
    /// roughly `a * (b + 2d + 2)` increments against `value`. The plan
    /// is taken only when it is smaller AND its estimated steps stay
    /// within 25% of direct, so constant construction shrinks programs
    /// without giving the win back at runtime. Small values never
    /// loop: no `(a, b)` with `a, b >= 2` beats `3 + value` bytes
    /// for them, and the scratch cell is allocated only when a plan
    /// is taken, so direct sites keep their cell layout.
    fn loop_plan(value: u8, scratch_dist: i32) -> Option<(u8, u8, u8)> {
        let v = u32::from(value);
        let d = scratch_dist.unsigned_abs();
        let direct_bytes = 3 + v;
        let direct_steps = 2 + v;
        let mut best: Option<(u32, u32, u8, u8, u8)> = None;
        for a in 2..=v / 2 {
            let b = v / a;
            let r = v - a * b;
            let bytes = 9 + a + b + r + 4 * d;
            if bytes >= direct_bytes {
                continue;
            }
            let steps = 3 + a + a * (b + 2 * d + 2) + 2 * d + r;
            if 4 * steps > 5 * direct_steps {
                continue;
            }
            let candidate = (bytes, steps, a as u8, b as u8, r as u8);
            if best.map_or(true, |prev| candidate < prev) {
                best = Some(candidate);
            }
        }
        best.map(|(_, _, a, b, r)| (a, b, r))
    }

    /// Set `dest` to `value` (`setValue`).
    pub fn set_value(&mut self, dest: i32, value: u8) -> i32 {
        // The scratch cell must differ from `dest`. Normally `dest` is
        // already allocated, so the next free cell qualifies; but a
        // caller-supplied `dest` can equal the next free index (e.g.
        // `set_value(0, _)` on a fresh emitter), in which case the
        // scratch would be `dest` itself and the loop below could
        // never empty it. Price the plan against the cell after
        // `dest` in that case, and reserve `dest` before allocating
        // the scratch so the two stay distinct.
        let peeked = self.memory.peek_temp_cell();
        let planned = if peeked == dest { peeked + 1 } else { peeked };
        if let Some((a, b, r)) = Self::loop_plan(value, (planned - dest).abs()) {
            if peeked == dest {
                let _reserved = self.memory.temp_cell();
            }
            let scratch = self.memory.temp_cell();
            debug_assert_eq!(scratch, planned);
            self.move_pointer(dest);
            self.emit("[-]");
            self.move_pointer(scratch);
            self.emit("[-]");
            for _ in 0..a {
                self.emit("+");
            }
            self.emit("[");
            self.move_pointer(dest);
            for _ in 0..b {
                self.emit("+");
            }
            self.move_pointer(scratch);
            self.emit("-");
            self.emit("]");
            self.move_pointer(dest);
            for _ in 0..r {
                self.emit("+");
            }
        } else {
            self.move_pointer(dest);
            self.emit("[-]");
            for _ in 0..value {
                self.emit("+");
            }
        }
        dest
    }

    /// Fresh cell initialized to `value` (`getPointerForConstValue`).
    pub fn const_value(&mut self, value: u8) -> i32 {
        let cell = self.memory.temp_cell();
        self.set_value(cell, value);
        cell
    }

    /// `result = a + b` in a fresh cell (`addValues`).
    pub fn add_values(&mut self, a: i32, b: i32) -> i32 {
        let a_copy = self.duplicate_value(a);
        let b_copy = self.duplicate_value(b);
        let result = self.memory.temp_cell();
        self.move_pointer(result);
        self.emit("[-]");
        self.move_pointer(a_copy);
        self.emit("[-");
        self.move_pointer(result);
        self.emit("+");
        self.move_pointer(a_copy);
        self.emit("]");
        self.move_pointer(b_copy);
        self.emit("[-");
        self.move_pointer(result);
        self.emit("+");
        self.move_pointer(b_copy);
        self.emit("]");
        result
    }

    /// `result = a - b` in a fresh cell (`subtractValues`).
    pub fn subtract_values(&mut self, a: i32, b: i32) -> i32 {
        let a_copy = self.duplicate_value(a);
        let b_copy = self.duplicate_value(b);
        let result = self.memory.temp_cell();
        self.move_pointer(result);
        self.emit("[-]");
        self.move_pointer(a_copy);
        self.emit("[-");
        self.move_pointer(result);
        self.emit("+");
        self.move_pointer(a_copy);
        self.emit("]");
        self.move_pointer(b_copy);
        self.emit("[-");
        self.move_pointer(result);
        self.emit("-");
        self.move_pointer(b_copy);
        self.emit("]");
        result
    }

    /// `result = a * b` in a fresh cell (`multiplyValues`).
    pub fn multiply_values(&mut self, a: i32, b: i32) -> i32 {
        let a_copy = self.duplicate_value(a);
        let result = self.const_value(0);
        self.move_pointer(a_copy);
        self.emit("[-");
        let new_result = self.add_values(b, result);
        self.move_value(new_result, result);
        self.move_pointer(a_copy);
        self.emit("]");
        result
    }

    /// `result = a / b` in a fresh cell (`divideValues`).
    pub fn divide_values(&mut self, a: i32, b: i32) -> Result<i32, CompileError> {
        let a_copy = self.duplicate_value(a);
        let result = self.const_value(0);
        self.perform_while(
            |em| Ok(em.greater_than_or_equal(a_copy, b)?),
            |em| {
                em.move_pointer(result);
                em.emit("+");
                let diff = em.subtract_values(a_copy, b);
                em.move_value(diff, a_copy);
                Ok(())
            },
        )?;
        Ok(result)
    }

    /// `result = a % b` in a fresh cell (`modValues`).
    pub fn mod_values(&mut self, a: i32, b: i32) -> Result<i32, CompileError> {
        let result = self.const_value(0);
        let q = self.divide_values(a, b)?;
        let prod = self.multiply_values(q, b);
        let diff = self.subtract_values(a, prod);
        self.move_value(diff, result);
        Ok(result)
    }

    /// `result = a <op> b` in a fresh cell, for a bitwise `op`.
    ///
    /// Walks the eight bits LSB-first, halving duplicated copies of
    /// both operands, and rebuilds the result one place value at a
    /// time. The operands are never consumed. The loop always runs
    /// exactly eight iterations (a counter cell), so termination
    /// does not depend on the operands reaching zero early, and
    /// exhausted high bits simply contribute zeros.
    fn bitwise_fold(&mut self, a: i32, b: i32, mode: BitCombine) -> Result<i32, CompileError> {
        let result = self.const_value(0);
        let ac = self.duplicate_value(a);
        let bc = self.duplicate_value(b);
        let place = self.const_value(1);
        let two = self.const_value(2);
        let counter = self.const_value(8);
        self.perform_while(
            |em| Ok(counter),
            |em| {
                let one = em.const_value(1);
                let d = em.subtract_values(counter, one);
                em.move_value(d, counter);
                let abit = em.mod_values(ac, two)?;
                let bbit = em.mod_values(bc, two)?;
                let bit = match mode {
                    BitCombine::And => em.multiply_values(abit, bbit),
                    BitCombine::Or => {
                        let both = em.multiply_values(abit, bbit);
                        let sum = em.add_values(abit, bbit);
                        em.subtract_values(sum, both)
                    }
                    BitCombine::Xor => {
                        let sum = em.add_values(abit, bbit);
                        em.mod_values(sum, two)?
                    }
                };
                let scaled = em.multiply_values(bit, place);
                let sum = em.add_values(result, scaled);
                em.move_value(sum, result);
                let doubled = em.multiply_values(place, two);
                em.move_value(doubled, place);
                let qa = em.divide_values(ac, two)?;
                em.move_value(qa, ac);
                let qb = em.divide_values(bc, two)?;
                em.move_value(qb, bc);
                Ok(())
            },
        )?;
        Ok(result)
    }

    /// `result = a & b` in a fresh cell.
    pub fn bitwise_and_values(&mut self, a: i32, b: i32) -> Result<i32, CompileError> {
        self.bitwise_fold(a, b, BitCombine::And)
    }

    /// `result = a | b` in a fresh cell.
    pub fn bitwise_or_values(&mut self, a: i32, b: i32) -> Result<i32, CompileError> {
        self.bitwise_fold(a, b, BitCombine::Or)
    }

    /// `result = a ^ b` in a fresh cell.
    pub fn bitwise_xor_values(&mut self, a: i32, b: i32) -> Result<i32, CompileError> {
        self.bitwise_fold(a, b, BitCombine::Xor)
    }

    /// `result = a << b` in a fresh cell: double `a` once per unit of
    /// the count (duplicated, never consumed). Counts of eight or
    /// more wrap the result to zero on their own, one doubling at a
    /// time mod 256.
    pub fn shift_left_values(&mut self, a: i32, b: i32) -> Result<i32, CompileError> {
        let result = self.duplicate_value(a);
        let counter = self.duplicate_value(b);
        let two = self.const_value(2);
        self.perform_while(
            |em| Ok(counter),
            |em| {
                let one = em.const_value(1);
                let d = em.subtract_values(counter, one);
                em.move_value(d, counter);
                let doubled = em.multiply_values(result, two);
                em.move_value(doubled, result);
                Ok(())
            },
        )?;
        Ok(result)
    }

    /// `result = a >> b` in a fresh cell: halve `a` once per unit of
    /// the count (duplicated, never consumed). Halving is logical —
    /// bytes are unsigned — so counts of eight or more give zero.
    pub fn shift_right_values(&mut self, a: i32, b: i32) -> Result<i32, CompileError> {
        let result = self.duplicate_value(a);
        let counter = self.duplicate_value(b);
        let two = self.const_value(2);
        self.perform_while(
            |em| Ok(counter),
            |em| {
                let one = em.const_value(1);
                let d = em.subtract_values(counter, one);
                em.move_value(d, counter);
                let halved = em.divide_values(result, two)?;
                em.move_value(halved, result);
                Ok(())
            },
        )?;
        Ok(result)
    }

    /// Logical NOT of `a` in a fresh cell (`negate`).
    pub fn negate(&mut self, a: i32) -> Result<i32, CompileError> {
        let result = self.memory.temp_cell();
        self.perform_if_else(
            a,
            |em| {
                em.set_value(result, 0);
                Ok(())
            },
            |em| {
                em.set_value(result, 1);
                Ok(())
            },
        )?;
        Ok(result)
    }

    /// `result = (a == b)` in a fresh cell (`isEqual`).
    pub fn is_equal(&mut self, a: i32, b: i32) -> Result<i32, CompileError> {
        let a_copy = self.duplicate_value(a);
        let b_copy = self.duplicate_value(b);
        let result = self.const_value(0);
        self.move_pointer(a_copy);
        self.emit("[-");
        self.move_pointer(b_copy);
        self.emit("-");
        self.move_pointer(a_copy);
        self.emit("]");
        let not_a = self.negate(a_copy)?;
        self.perform_if_else(
            not_a,
            |em| {
                let not_b = em.negate(b_copy)?;
                em.perform_if_else(
                    not_b,
                    |em| {
                        em.set_value(result, 1);
                        Ok(())
                    },
                    |_| Ok(()),
                )
            },
            |_| Ok(()),
        )?;
        Ok(result)
    }

    /// `result = (a && b)` in a fresh cell (`logicalAnd`).
    pub fn logical_and(&mut self, a: i32, b: i32) -> Result<i32, CompileError> {
        let result = self.const_value(0);
        self.perform_if_else(
            a,
            |em| {
                em.perform_if_else(
                    b,
                    |em| {
                        em.set_value(result, 1);
                        Ok(())
                    },
                    |_| Ok(()),
                )
            },
            |_| Ok(()),
        )?;
        Ok(result)
    }

    /// `result = (a || b)` in a fresh cell (`logicalOr`).
    pub fn logical_or(&mut self, a: i32, b: i32) -> Result<i32, CompileError> {
        let result = self.const_value(0);
        self.perform_if_else(
            a,
            |em| {
                em.set_value(result, 1);
                Ok(())
            },
            |_| Ok(()),
        )?;
        self.perform_if_else(
            b,
            |em| {
                em.set_value(result, 1);
                Ok(())
            },
            |_| Ok(()),
        )?;
        Ok(result)
    }

    /// `result = (a < b)` in a fresh cell (`lessThan`).
    pub fn less_than(&mut self, a: i32, b: i32) -> Result<i32, CompileError> {
        let a_copy = self.duplicate_value(a);
        let b_copy = self.duplicate_value(b);
        let result = self.memory.temp_cell();
        self.move_pointer(result);
        self.emit("[-]");
        self.perform_while(
            |em| Ok(em.logical_and(a_copy, b_copy)?),
            |em| {
                em.move_pointer(a_copy);
                em.emit("-");
                em.move_pointer(b_copy);
                em.emit("-");
                Ok(())
            },
        )?;
        let not_a = self.negate(a_copy)?;
        let cond = self.logical_and(not_a, b_copy)?;
        self.perform_if_else(
            cond,
            |em| {
                em.set_value(result, 1);
                Ok(())
            },
            |_| Ok(()),
        )?;
        Ok(result)
    }

    /// `result = (a <= b)` (`lessThanOrEqual`).
    ///
    /// Operand order is GCC right-to-left (`isEqual` first), per the probe.
    pub fn less_than_or_equal(&mut self, a: i32, b: i32) -> Result<i32, CompileError> {
        let eq = self.is_equal(a, b)?;
        let lt = self.less_than(a, b)?;
        self.logical_or(lt, eq)
    }

    /// `result = (a > b)` (`greaterThan`).
    pub fn greater_than(&mut self, a: i32, b: i32) -> Result<i32, CompileError> {
        let le = self.less_than_or_equal(a, b)?;
        self.negate(le)
    }

    /// `result = (a >= b)` (`greaterThanOrEqual`).
    pub fn greater_than_or_equal(&mut self, a: i32, b: i32) -> Result<i32, CompileError> {
        let lt = self.less_than(a, b)?;
        self.negate(lt)
    }

    /// If/else on `expression` (`performIfElse`).
    pub fn perform_if_else(
        &mut self,
        expression: i32,
        mut iff: impl FnMut(&mut Self) -> Result<(), CompileError>,
        mut els: impl FnMut(&mut Self) -> Result<(), CompileError>,
    ) -> Result<(), CompileError> {
        let if_value = self.duplicate_value(expression);
        let else_value = self.const_value(1);
        self.move_pointer(if_value);
        self.emit("[");
        self.set_value(if_value, 0);
        self.set_value(else_value, 0);
        iff(self)?;
        self.move_pointer(if_value);
        self.emit("]");
        self.move_pointer(else_value);
        self.emit("[");
        self.set_value(else_value, 0);
        els(self)?;
        self.move_pointer(else_value);
        self.emit("]");
        Ok(())
    }

    /// While loop, re-evaluating `cond` every iteration (`performWhile`).
    pub fn perform_while(
        &mut self,
        mut cond: impl FnMut(&mut Self) -> Result<i32, CompileError>,
        mut body: impl FnMut(&mut Self) -> Result<(), CompileError>,
    ) -> Result<(), CompileError> {
        let expression = cond(self)?;
        self.move_pointer(expression);
        self.emit("[");
        self.move_pointer(expression);
        body(self)?;
        let new_expression = cond(self)?;
        self.move_value(new_expression, expression);
        self.move_pointer(expression);
        self.emit("]");
        Ok(())
    }

    /// Do...while loop: the body runs once unconditionally, then like
    /// `perform_while` (Phase 6: completes the thesis iteration set).
    pub fn perform_do_while(
        &mut self,
        cond: impl FnMut(&mut Self) -> Result<i32, CompileError>,
        mut body: impl FnMut(&mut Self) -> Result<(), CompileError>,
    ) -> Result<(), CompileError> {
        body(self)?;
        self.perform_while(cond, body)
    }

    /// For loop over init/cond/update/body (`performFor`).
    pub fn perform_for(
        &mut self,
        mut init: impl FnMut(&mut Self) -> Result<(), CompileError>,
        cond: impl FnMut(&mut Self) -> Result<i32, CompileError>,
        mut update: impl FnMut(&mut Self) -> Result<(), CompileError>,
        mut body: impl FnMut(&mut Self) -> Result<(), CompileError>,
    ) -> Result<(), CompileError> {
        init(self)?;
        self.perform_while(cond, |em| {
            body(em)?;
            update(em)
        })
    }

    /// Print cell `a` as a character (`printAsChar`).
    pub fn print_as_char(&mut self, a: i32) {
        self.move_pointer(a);
        self.emit(".");
    }

    /// Print cell `a` as a decimal number (`printAsNumber`).
    ///
    /// Place-value extraction: `helper` holds the current decimal
    /// place (100, 10, 1), computed once from `a`; each pass prints
    /// `a_copy / helper` and strips it. The previous shape recomputed
    /// the leading digit from the remainder alone, which lost interior
    /// zeros (101 printed as 110, 209 as 2900) and re-ran the divide
    /// machinery once per emitted digit.
    pub fn print_as_number(&mut self, a: i32) -> Result<(), CompileError> {
        let a_copy = self.duplicate_value(a);
        let digit = self.duplicate_value(a);
        let helper = self.const_value(1);
        let zero_char = self.const_value(b'0');
        let nine = self.const_value(9);
        let ten = self.const_value(10);
        self.perform_if_else(
            a,
            |em| {
                // Highest power of ten not exceeding `a`. `digit`
                // ends as the leading digit; the main loop recomputes
                // every digit from `a_copy` and `helper` anyway.
                em.perform_while(
                    |em| Ok(em.greater_than(digit, nine)?),
                    |em| {
                        let q = em.divide_values(digit, ten)?;
                        em.move_value(q, digit);
                        let h = em.multiply_values(helper, ten);
                        em.move_value(h, helper);
                        Ok(())
                    },
                )?;
                // One pass per place; `helper` reaching 0 ends the
                // loop. The condition is `helper` itself: it is
                // re-tested after each pass, once divided by ten.
                em.perform_while(
                    |em| Ok(helper),
                    |em| {
                        let q = em.divide_values(a_copy, helper)?;
                        em.move_value(q, digit);
                        let glyph = em.add_values(digit, zero_char);
                        em.print_as_char(glyph);
                        let scaled = em.multiply_values(digit, helper);
                        let rest = em.subtract_values(a_copy, scaled);
                        em.move_value(rest, a_copy);
                        let q = em.divide_values(helper, ten)?;
                        em.move_value(q, helper);
                        Ok(())
                    },
                )
            },
            |em| {
                em.print_as_char(zero_char);
                Ok(())
            },
        )
    }

    /// Print cell `a` as a lowercase hexadecimal number (`%x`).
    ///
    /// Same place-value shape as [`Self::print_as_number`] with base 16.
    /// Bytes need at most two hex digits, so the place loop runs while
    /// the digit exceeds 15 (one division): `helper` only ever reaches
    /// 16, and never wraps through 256 the way a second multiply would.
    pub fn print_as_hex(&mut self, a: i32) -> Result<(), CompileError> {
        let a_copy = self.duplicate_value(a);
        let digit = self.duplicate_value(a);
        let helper = self.const_value(1);
        let zero_char = self.const_value(b'0');
        let nine = self.const_value(9);
        let ten = self.const_value(10);
        let fifteen = self.const_value(15);
        let sixteen = self.const_value(16);
        let a_char = self.const_value(b'a');
        self.perform_if_else(
            a,
            |em| {
                em.perform_while(
                    |em| Ok(em.greater_than(digit, fifteen)?),
                    |em| {
                        let q = em.divide_values(digit, sixteen)?;
                        em.move_value(q, digit);
                        let h = em.multiply_values(helper, sixteen);
                        em.move_value(h, helper);
                        Ok(())
                    },
                )?;
                em.perform_while(
                    |em| Ok(helper),
                    |em| {
                        let q = em.divide_values(a_copy, helper)?;
                        em.move_value(q, digit);
                        let glyph = em.add_values(digit, zero_char);
                        let is_letter = em.greater_than(digit, nine)?;
                        em.perform_if_else(
                            is_letter,
                            |em| {
                                let d = em.subtract_values(digit, ten);
                                let g = em.add_values(d, a_char);
                                em.move_value(g, glyph);
                                Ok(())
                            },
                            |_| Ok(()),
                        )?;
                        em.print_as_char(glyph);
                        let scaled = em.multiply_values(digit, helper);
                        let rest = em.subtract_values(a_copy, scaled);
                        em.move_value(rest, a_copy);
                        let q = em.divide_values(helper, sixteen)?;
                        em.move_value(q, helper);
                        Ok(())
                    },
                )
            },
            |em| {
                em.print_as_char(zero_char);
                Ok(())
            },
        )
    }

    /// Read one character into cell `a` (`readAsChar`).
    pub fn read_as_char(&mut self, a: i32) {
        self.move_pointer(a);
        self.emit(",");
    }

    /// Read a decimal number into cell `a` (`readAsNumber`).
    pub fn read_as_number(&mut self, a: i32) -> Result<(), CompileError> {
        let reader = self.const_value(0);
        let result = self.const_value(0);
        let zero = self.const_value(b'0');
        let nine = self.const_value(b'9');
        let ten = self.const_value(10);
        self.move_pointer(reader);
        self.emit(",");
        self.perform_while(
            |em| {
                // GCC right-to-left here (measured: the divergence window
                // shows the nine-cell duplicated second).
                let above = em.greater_than(reader, nine)?;
                let below = em.less_than(reader, zero)?;
                em.logical_or(below, above)
            },
            |em| {
                em.move_pointer(reader);
                em.emit(",");
                Ok(())
            },
        )?;
        self.perform_while(
            |em| {
                // GCC right-to-left here (measured via io.c differential;
                // symmetric with the first condition).
                let le = em.less_than_or_equal(reader, nine)?;
                let ge = em.greater_than_or_equal(reader, zero)?;
                em.logical_and(ge, le)
            },
            |em| {
                // GCC right-to-left here (measured via io.c differential):
                // unlike BinOp arithmetic, the multiply side allocates first.
                let shifted = em.multiply_values(result, ten);
                let dig = em.subtract_values(reader, zero);
                let acc = em.add_values(dig, shifted);
                em.move_value(acc, result);
                em.move_pointer(reader);
                em.emit(",");
                Ok(())
            },
        )?;
        self.move_value(result, a);
        Ok(())
    }

    /// Print the constant byte `c` (`printChar`).
    pub fn print_char(&mut self, c: u8) {
        let a = self.const_value(c);
        self.move_pointer(a);
        self.emit(".");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runner;

    fn bf(em: &Emitter) -> String {
        String::from_utf8(em.output().to_vec()).expect("non-utf8 bf")
    }

    #[test]
    fn const_cells_count_and_emit() {
        let mut em = Emitter::new();
        assert_eq!(em.const_value(3), 0);
        assert_eq!(bf(&em), "[-]+++");
        assert_eq!(em.const_value(1), 1);
        assert_eq!(bf(&em), "[-]+++>[-]+");
    }

    #[test]
    fn copy_value_exact() {
        let mut em = Emitter::new();
        let a = em.const_value(2);
        let b = em.const_value(0);
        assert_eq!(em.copy_value(a, b), 1);
        assert_eq!(
            bf(&em),
            "[-]++>[-]>[-]<[-]<[->+>+<<]>>[-<<+>>]"
        );
    }

    #[test]
    fn copy_same_cell_is_noop() {
        let mut em = Emitter::new();
        let a = em.const_value(1);
        let len = em.output().len();
        assert_eq!(em.copy_value(a, a), a);
        assert_eq!(em.output().len(), len);
    }

    #[test]
    fn move_value_exact() {
        let mut em = Emitter::new();
        let a = em.const_value(2);
        let b = em.const_value(0);
        assert_eq!(em.move_value(a, b), 1);
        assert_eq!(bf(&em), "[-]++>[-][-]<[->+<]");
    }

    #[test]
    fn move_same_cell_is_noop() {
        let mut em = Emitter::new();
        let a = em.const_value(1);
        let len = em.output().len();
        assert_eq!(em.move_value(a, a), a);
        assert_eq!(em.output().len(), len);
    }

    #[test]
    fn duplicate_value_exact() {
        let mut em = Emitter::new();
        let a = em.const_value(1);
        assert_eq!(em.duplicate_value(a), 1);
        assert_eq!(bf(&em), "[-]+>>[-]<[-]<[->+>+<<]>>[-<<+>>]");
    }

    #[test]
    fn add_values_exact() {
        let mut em = Emitter::new();
        let a = em.const_value(2);
        let b = em.const_value(3);
        assert_eq!(em.add_values(a, b), 6);
        assert_eq!(
            bf(&em),
            "[-]++>[-]+++\
             >>[-]<[-]<<[->>+>+<<<]>>>[-<<<+>>>]\
             >>[-]<[-]<<<[->>>+>+<<<<]>>>>[-<<<<+>>>>]\
             >[-]<<<<[->>>>+<<<<]>>[->>+<<]"
                .replace(|c: char| c.is_whitespace(), ""),
        );
    }

    #[test]
    fn subtract_values_exact() {
        let mut em = Emitter::new();
        let a = em.const_value(2);
        let b = em.const_value(3);
        assert_eq!(em.subtract_values(a, b), 6);
        assert_eq!(
            bf(&em),
            "[-]++>[-]+++\
             >>[-]<[-]<<[->>+>+<<<]>>>[-<<<+>>>]\
             >>[-]<[-]<<<[->>>+>+<<<<]>>>>[-<<<<+>>>>]\
             >[-]<<<<[->>>>+<<<<]>>[->>-<<]"
                .replace(|c: char| c.is_whitespace(), ""),
        );
    }

    #[test]
    fn compound_helpers_return_fresh_cells_in_order() {
        // Pins allocation order: result cell index after fixed setups.
        let mut em = Emitter::new();
        let a = em.const_value(5);
        assert_eq!(em.negate(a).unwrap(), 1);

        let mut em = Emitter::new();
        let a = em.const_value(5);
        let b = em.const_value(7);
        assert_eq!(em.is_equal(a, b).unwrap(), 6);

        let mut em = Emitter::new();
        let a = em.const_value(5);
        let b = em.const_value(7);
        assert_eq!(em.logical_and(a, b).unwrap(), 2);

        let mut em = Emitter::new();
        let a = em.const_value(5);
        let b = em.const_value(7);
        assert_eq!(em.logical_or(a, b).unwrap(), 2);

        let mut em = Emitter::new();
        let a = em.const_value(5);
        let b = em.const_value(7);
        assert_eq!(em.multiply_values(a, b), 4);

        let mut em = Emitter::new();
        let a = em.const_value(5);
        let b = em.const_value(7);
        assert_eq!(em.divide_values(a, b).unwrap(), 4);

        let mut em = Emitter::new();
        let a = em.const_value(5);
        let b = em.const_value(7);
        assert_eq!(em.mod_values(a, b).unwrap(), 2);

        let mut em = Emitter::new();
        let a = em.const_value(5);
        let b = em.const_value(7);
        assert_eq!(em.less_than(a, b).unwrap(), 6);
    }

    #[test]
    fn loop_plan_picks_smallest_bytes_within_step_bound() {
        assert_eq!(Emitter::loop_plan(200, 1), Some((8, 25, 0)));
        assert_eq!(Emitter::loop_plan(100, 1), Some((4, 25, 0)));
        assert_eq!(Emitter::loop_plan(48, 1), None);
        assert_eq!(Emitter::loop_plan(5, 1), None);
        assert_eq!(Emitter::loop_plan(0, 1), None);
        assert_eq!(Emitter::loop_plan(200, 10), None);
    }

    #[test]
    fn set_value_loop_construction_exact() {
        let mut em = Emitter::new();
        assert_eq!(em.set_value(0, 200), 0);
        assert_eq!(
            bf(&em),
            format!("[-]>[-]{}[<{}>-]<", "+".repeat(8), "+".repeat(25))
        );
    }

    #[test]
    fn set_value_scratch_is_distinct_from_unallocated_dest() {
        let mut em = Emitter::new();
        em.set_value(0, 200);
        // Cell 0 was the destination: the loop reserved it and used
        // cell 1 as the scratch, so the next fresh cell is 2.
        assert_eq!(em.const_value(0), 2);
    }

    #[test]
    fn set_value_small_and_distant_values_stay_direct() {
        let mut em = Emitter::new();
        em.set_value(0, 3);
        assert_eq!(bf(&em), "[-]+++");
        // Scratch ten cells away: no loop stays within the step bound.
        let mut em = Emitter::new();
        em.set_value(10, 200);
        assert_eq!(em.output().len(), 10 + 3 + 200);
    }

    /// Run the emitter's output with the pointer on `cell` and
    /// return the one byte it prints.
    fn run_cell(em: &mut Emitter, cell: i32) -> u8 {
        em.move_pointer(cell);
        let mut code = bf(em);
        code.push('.');
        let mut out = Vec::new();
        runner::run(code.as_bytes(), &mut std::io::empty(), &mut out).expect("bf runs");
        out[0]
    }

    #[test]
    fn bitwise_and_or_xor_values() {
        for (a, b, and, or, xor) in [
            (0b1100u8, 0b1010u8, 0b1000u8, 0b1110u8, 0b0110u8),
            (0xff, 0x0f, 0x0f, 0xff, 0xf0),
            (0, 0, 0, 0, 0),
            (0x55, 0xaa, 0x00, 0xff, 0xff),
            (0x01, 0x80, 0x00, 0x81, 0x81),
        ] {
            let mut em = Emitter::new();
            let ca = em.const_value(a);
            let cb = em.const_value(b);
            let r = em.bitwise_and_values(ca, cb).unwrap();
            assert_eq!(run_cell(&mut em, r), and, "0x{a:02x} & 0x{b:02x}");
            let mut em = Emitter::new();
            let ca = em.const_value(a);
            let cb = em.const_value(b);
            let r = em.bitwise_or_values(ca, cb).unwrap();
            assert_eq!(run_cell(&mut em, r), or, "0x{a:02x} | 0x{b:02x}");
            let mut em = Emitter::new();
            let ca = em.const_value(a);
            let cb = em.const_value(b);
            let r = em.bitwise_xor_values(ca, cb).unwrap();
            assert_eq!(run_cell(&mut em, r), xor, "0x{a:02x} ^ 0x{b:02x}");
        }
    }

    #[test]
    fn shift_values() {
        for (a, n, left, right) in [
            (1u8, 0u8, 1u8, 1u8),
            (1, 7, 128, 0),
            (3, 2, 12, 0),
            (128, 1, 0, 64),
            (255, 8, 0, 0),
            (200, 3, 64, 25),
            (5, 9, 0, 0),
        ] {
            let mut em = Emitter::new();
            let ca = em.const_value(a);
            let cb = em.const_value(n);
            let r = em.shift_left_values(ca, cb).unwrap();
            assert_eq!(run_cell(&mut em, r), left, "{a} << {n}");
            let mut em = Emitter::new();
            let ca = em.const_value(a);
            let cb = em.const_value(n);
            let r = em.shift_right_values(ca, cb).unwrap();
            assert_eq!(run_cell(&mut em, r), right, "{a} >> {n}");
        }
    }

    #[test]
    fn set_value_builds_every_byte_value() {
        for (dest, value) in [
            (0, 0u8),
            (0, 1),
            (0, 47),
            (0, 48),
            (0, 57),
            (0, 100),
            (2, 100),
            (0, 127),
            (4, 200),
            (0, 200),
            (0, 254),
            (0, 255),
        ] {
            let mut em = Emitter::new();
            em.set_value(dest, value);
            em.move_pointer(dest);
            let mut code = bf(&em);
            code.push('.');
            let mut out = Vec::new();
            runner::run(code.as_bytes(), &mut std::io::empty(), &mut out)
                .expect("bf runs");
            assert_eq!(out, vec![value], "dest {dest} value {value}");
        }
    }
}
