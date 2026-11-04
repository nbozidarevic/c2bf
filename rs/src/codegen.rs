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

use crate::ast::{Block, Expr, FnParam};
use crate::memory::Memory;
use std::collections::HashMap;
use std::rc::Rc;

/// A function definition with its body scope-renamed (see
/// [`rename_function`]): every declared name is replaced by a fresh
/// per-declaration name, so nested scopes no longer share cells and
/// two expansions of the same function never interfere.
#[derive(Debug)]
pub struct ExpandedFunction {
    pub is_void: bool,
    pub params: Vec<FnParam>,
    pub body: Block,
}

/// A function signature, from a prototype (or, for comparison, a
/// definition): void-ness plus per-parameter pointer/array flags.
/// Parameter names are ignored, as in C; all type spellings denote
/// one byte here, so only the flags are compared.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FuncSig {
    pub is_void: bool,
    pub params: Vec<FnParam>,
}

impl FuncSig {
    /// Whether two signatures agree: same void-ness, same parameter
    /// count, same pointer/array flags per parameter.
    pub fn matches(&self, other: &FuncSig) -> bool {
        self.is_void == other.is_void
            && self.params.len() == other.params.len()
            && self
                .params
                .iter()
                .zip(other.params.iter())
                .all(|(a, b)| a.pointer == b.pointer && a.array == b.array)
    }
}

/// Code emitter: cell allocator, output bytes, tracked data pointer.
#[derive(Debug, Default)]
pub struct Emitter {
    pub(crate) memory: Memory,
    output: Vec<u8>,
    pointer: i32,
    pub(crate) break_stack: Vec<i32>,
    pub(crate) continue_stack: Vec<i32>,
    /// Scope-renamed definitions, keyed by function name, built by
    /// `compile_program` before any code is emitted; a call expands
    /// the shared definition inline.
    pub(crate) functions: HashMap<Vec<u8>, Rc<ExpandedFunction>>,
    /// Prototypes (forward declarations), keyed by function name,
    /// registered in `compile_program` before any code is emitted;
    /// a call to a prototyped-but-undefined function is an error.
    pub(crate) prototypes: HashMap<Vec<u8>, Rc<FuncSig>>,
    /// Names of functions whose expansion is in progress; a cycle
    /// here is a recursive call, which inline expansion cannot do.
    pub(crate) expanding: Vec<Vec<u8>>,
    /// `(returned_cell, result_cell)` of the innermost function being
    /// expanded, or `None` while compiling `main`.
    pub(crate) return_context: Option<(i32, i32)>,
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

    /// Cells allocated during emission so far (see
    /// [`Memory::allocated_cells`]).
    pub fn allocated_cells(&self) -> usize {
        self.memory.allocated_cells()
    }

    /// Allocation checkpoint for scoped temporary reclaim (see
    /// [`Memory::checkpoint`]).
    pub fn checkpoint(&self) -> i32 {
        self.memory.checkpoint()
    }

    /// Reclaim temporaries allocated since `checkpoint` (see
    /// [`Memory::release_temps`]).
    pub fn release_temps(&mut self, checkpoint: i32) {
        self.memory.release_temps(checkpoint);
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
        self.copy_value_with_helper(source, dest, helper)
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

    /// Materialize a string literal's cooked bytes plus a NUL
    /// terminator in anonymous, never-reclaimed cells, and return a
    /// fresh cell holding the base address. The base must fit in a
    /// byte, like every other address.
    pub fn string_value(&mut self, cooked: &[u8]) -> Result<i32, CompileError> {
        let base = self.memory.alloc_anon_array(cooked.len() + 1);
        if !(0..=255).contains(&base) {
            return Err(CompileError {
                msg: "address is outside the addressable range".to_string(),
            });
        }
        for (i, &byte) in cooked.iter().enumerate() {
            self.set_value(base + i as i32, byte);
        }
        self.set_value(base + cooked.len() as i32, 0);
        Ok(self.const_value(base as u8))
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
    ///
    /// Division by zero is defined here, not a hang: when `b` is
    /// zero the loop is skipped and the quotient is zero, so
    /// `a % 0` (see [`Emitter::mod_values`]) keeps the identity
    /// `a == q * b + r` with `r == a`. C leaves both undefined
    /// and the g++ oracle traps, so the behavior is pinned by unit
    /// tests, not by an oracle fixture.
    pub fn divide_values(&mut self, a: i32, b: i32) -> Result<i32, CompileError> {
        let a_copy = self.duplicate_value(a);
        let result = self.const_value(0);
        // `b` itself is the branch condition: a zero divisor
        // skips the loop entirely. The previous shape tested
        // `a >= 0`, which is always true, and subtracted zero
        // forever. `b` is preserved by the branch, so
        // `mod_values` can still read it afterwards.
        self.perform_if_else(
            b,
            |em| em.divide_loop(a_copy, result, b),
            |_| Ok(()),
        )?;
        Ok(result)
    }

    /// The division loop shared by [`Emitter::divide_values`] and
    /// [`Emitter::divide_values_unchecked`]: repeated subtraction
    /// into `result`, draining `a_copy`.
    fn divide_loop(&mut self, a_copy: i32, result: i32, b: i32) -> Result<(), CompileError> {
        self.perform_while(
            |em| Ok(em.greater_than_or_equal(a_copy, b)?),
            |em| {
                em.move_pointer(result);
                em.emit("+");
                let diff = em.subtract_values(a_copy, b);
                em.move_value(diff, a_copy);
                Ok(())
            },
        )
    }

    /// `result = a / b` without the zero-divisor branch: the caller
    /// guarantees `b` is nonzero at runtime. Only the print helpers
    /// use this — they divide by constants and by a place value
    /// that is itself the loop condition, so a guard there would
    /// be pure overhead on every `%d`/`%x` print.
    pub fn divide_values_unchecked(&mut self, a: i32, b: i32) -> Result<i32, CompileError> {
        let a_copy = self.duplicate_value(a);
        let result = self.const_value(0);
        self.divide_loop(a_copy, result, b)?;
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

    /// Remainder of `a % 2`; `a` is left holding the quotient (`a / 2`).
    ///
    /// Fused divide/modulo by two for [`Emitter::bitwise_fold`]: the
    /// pair-removal loop in [`Emitter::halve_with_remainder`] yields
    /// both halves with plain zero tests, instead of a
    /// `divide_values` plus a `mod_values` per operand (each paying
    /// a full compare chain per check). `a` is a fold-owned scratch
    /// copy, never a caller's operand cell.
    fn divmod_two(&mut self, a: i32) -> Result<i32, CompileError> {
        self.halve_with_remainder(a)
    }

    /// Double `cell` in place, wrapping (`cell *= 2`).
    ///
    /// Drains `cell` into a fresh zeroed scratch cell, adding two per
    /// unit, then moves the scratch back. Cheaper than the general
    /// `multiply_values(cell, 2)`, which duplicates and re-adds per
    /// unit. `cell` keeps its index; the scratch ends zero.
    fn double_in_place(&mut self, cell: i32) {
        let scratch = self.memory.temp_cell();
        self.set_value(scratch, 0);
        self.move_pointer(cell);
        self.emit("[-");
        self.move_pointer(scratch);
        self.emit("++");
        self.move_pointer(cell);
        self.emit("]");
        self.move_value(scratch, cell);
    }

    /// Halve `cell` in place, flooring (`cell /= 2`, logical),
    /// returning the remainder (`0` or `1`) in a fresh cell.
    ///
    /// Each outer pass removes one pair: it decrements `cell`, then
    /// copies the remainder to a probe cell — if the probe is
    /// nonzero a pair was present, so one more unit is removed and
    /// the quotient grows by one. The probe copy makes the pair
    /// test a plain zero test, with no compare helper and no
    /// wrapping hazard (the second decrement only runs when at
    /// least two units were present). The remainder cannot be read
    /// from `cell` afterwards — the final lone unit is consumed by
    /// the last pass's decrement — so each pass records whether it
    /// started with exactly one unit (probe zero) into the
    /// remainder cell; the last pass's record wins, and zero passes
    /// leave the initial zero.
    fn halve_with_remainder(&mut self, cell: i32) -> Result<i32, CompileError> {
        let quotient = self.memory.temp_cell();
        self.set_value(quotient, 0);
        let probe = self.memory.temp_cell();
        let remainder = self.memory.temp_cell();
        self.set_value(remainder, 0);
        self.move_pointer(cell);
        self.emit("[");
        self.move_pointer(cell);
        self.emit("-");
        self.copy_value(cell, probe);
        let lone = self.negate(probe)?;
        self.move_value(lone, remainder);
        self.move_pointer(probe);
        self.emit("[");
        self.move_pointer(cell);
        self.emit("-");
        self.move_pointer(quotient);
        self.emit("+");
        self.set_value(probe, 0);
        self.move_pointer(probe);
        self.emit("]");
        self.move_pointer(cell);
        self.emit("]");
        self.move_value(quotient, cell);
        Ok(remainder)
    }

    /// Halve `cell` in place, flooring (`cell /= 2`, logical).
    fn halve_in_place(&mut self, cell: i32) -> Result<(), CompileError> {
        self.halve_with_remainder(cell)?;
        Ok(())
    }

    /// Add a copy of `source` into `dest`, preserving `source`.
    /// Unlike `add_values`, this adds to `dest`'s existing value
    /// instead of returning a fresh cell, and needs only one
    /// duplicate plus one drain loop.
    fn add_copy_to(&mut self, source: i32, dest: i32) {
        let helper = self.duplicate_value(source);
        self.move_pointer(helper);
        self.emit("[-");
        self.move_pointer(dest);
        self.emit("+");
        self.move_pointer(helper);
        self.emit("]");
    }

    /// Combine two bit cells (`0`/`1`) with AND, returning a fresh
    /// bit cell. Consumes `abit`: the loop clears it after copying
    /// `bbit` through. `bbit` is preserved but unused afterwards.
    fn combine_and(&mut self, abit: i32, bbit: i32) -> Result<i32, CompileError> {
        let bit = self.const_value(0);
        self.move_pointer(abit);
        self.emit("[");
        self.copy_value(bbit, bit);
        self.set_value(abit, 0);
        self.move_pointer(abit);
        self.emit("]");
        Ok(bit)
    }

    /// Combine two bit cells (`0`/`1`) with OR, returning a fresh
    /// bit cell. Consumes `bbit`: the loop sets the result to `1`
    /// and clears `bbit` to exit. `abit` is preserved but unused
    /// afterwards.
    fn combine_or(&mut self, abit: i32, bbit: i32) -> Result<i32, CompileError> {
        let bit = self.const_value(0);
        self.copy_value(abit, bit);
        self.move_pointer(bbit);
        self.emit("[");
        self.set_value(bit, 1);
        self.set_value(bbit, 0);
        self.move_pointer(bbit);
        self.emit("]");
        Ok(bit)
    }

    /// Combine two bit cells (`0`/`1`) with XOR, returning a fresh
    /// bit cell. Consumes `bbit`: the loop flips the copied `abit`
    /// once and clears `bbit` to exit.
    fn combine_xor(&mut self, abit: i32, bbit: i32) -> Result<i32, CompileError> {
        let bit = self.const_value(0);
        self.copy_value(abit, bit);
        self.move_pointer(bbit);
        self.emit("[");
        self.perform_if_else(
            bit,
            |em| {
                em.set_value(bit, 0);
                Ok(())
            },
            |em| {
                em.set_value(bit, 1);
                Ok(())
            },
        )?;
        self.set_value(bbit, 0);
        self.move_pointer(bbit);
        self.emit("]");
        Ok(bit)
    }

    /// Add `place` into `result` when `bit` is nonzero, preserving
    /// both cells. Cheaper than scaling the bit with
    /// `multiply_values` and adding the product: the bit is only
    /// ever `0` or `1`, so the multiply's loop is pure overhead.
    fn accumulate_bit(&mut self, bit: i32, result: i32, place: i32) -> Result<(), CompileError> {
        self.perform_if_else(
            bit,
            |em| {
                em.add_copy_to(place, result);
                Ok(())
            },
            |_| Ok(()),
        )
    }

    /// `result = a <op> b` in a fresh cell, for a bitwise `op`.
    ///
    /// Walks the eight bits LSB-first, halving duplicated copies of
    /// both operands, and rebuilds the result one place value at a
    /// time. The operands are never consumed. Bit extraction is
    /// fused: [`Emitter::divmod_two`] leaves each quotient in its
    /// operand copy and returns the bit, instead of a `mod_values`
    /// plus a `divide_values` per operand. The loop runs at most
    /// eight iterations (a counter cell) and stops early once both
    /// quotients are exhausted, so exhausted high bits simply
    /// contribute zeros.
    fn bitwise_fold(&mut self, a: i32, b: i32, mode: BitCombine) -> Result<i32, CompileError> {
        let result = self.const_value(0);
        let ac = self.duplicate_value(a);
        let bc = self.duplicate_value(b);
        let place = self.const_value(1);
        let counter = self.const_value(8);
        self.perform_while(
            |em| Ok(counter),
            |em| {
                em.move_pointer(counter);
                em.emit("-");
                let abit = em.divmod_two(ac)?;
                let bbit = em.divmod_two(bc)?;
                let bit = match mode {
                    BitCombine::And => em.combine_and(abit, bbit)?,
                    BitCombine::Or => em.combine_or(abit, bbit)?,
                    BitCombine::Xor => em.combine_xor(abit, bbit)?,
                };
                em.accumulate_bit(bit, result, place)?;
                em.double_in_place(place);
                let zero_a = em.negate(ac)?;
                let zero_b = em.negate(bc)?;
                let both_zero = em.multiply_values(zero_a, zero_b);
                em.perform_if_else(
                    both_zero,
                    |em| {
                        em.set_value(counter, 0);
                        Ok(())
                    },
                    |_| Ok(()),
                )?;
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
        self.perform_while(
            |em| Ok(counter),
            |em| {
                em.move_pointer(counter);
                em.emit("-");
                em.double_in_place(result);
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
        self.perform_while(
            |em| Ok(counter),
            |em| {
                em.move_pointer(counter);
                em.emit("-");
                em.halve_in_place(result)?;
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
    ///
    /// Temporaries allocated inside (guard cells, branch temps) are
    /// reclaimed afterwards: values only flow out through cells
    /// allocated before the call or through variables, which the
    /// reclaim never touches.
    pub fn perform_if_else(
        &mut self,
        expression: i32,
        mut iff: impl FnMut(&mut Self) -> Result<(), CompileError>,
        mut els: impl FnMut(&mut Self) -> Result<(), CompileError>,
    ) -> Result<(), CompileError> {
        let checkpoint = self.checkpoint();
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
        self.release_temps(checkpoint);
        Ok(())
    }

    /// While loop, re-evaluating `cond` every iteration (`performWhile`).
    ///
    /// Like [`Emitter::perform_if_else`], temporaries allocated for
    /// the condition and body evaluations are reclaimed afterwards:
    /// counters, results, and flags live in cells allocated before
    /// the loop, and variables are never reclaimed.
    pub fn perform_while(
        &mut self,
        mut cond: impl FnMut(&mut Self) -> Result<i32, CompileError>,
        mut body: impl FnMut(&mut Self) -> Result<(), CompileError>,
    ) -> Result<(), CompileError> {
        let checkpoint = self.checkpoint();
        let expression = cond(self)?;
        self.move_pointer(expression);
        self.emit("[");
        self.move_pointer(expression);
        body(self)?;
        let new_expression = cond(self)?;
        self.move_value(new_expression, expression);
        self.move_pointer(expression);
        self.emit("]");
        self.release_temps(checkpoint);
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

    /// Decrement `cell` by one in place, wrapping like all
    /// arithmetic. Only used where a fresh result cell would be
    /// waste; the pointer ends on `cell`, tracked as usual.
    fn decrement_in_place(&mut self, cell: i32) {
        self.move_pointer(cell);
        self.emit("-");
    }

    /// Copy `source` to `destination` with a caller-provided
    /// helper cell, preserving `source` (see [`Emitter::copy_value`]).
    /// The helper must differ from both cells; it ends at zero.
    fn copy_value_with_helper(
        &mut self,
        source: i32,
        dest: i32,
        helper: i32,
    ) -> i32 {
        if source == dest {
            return dest;
        }
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

    /// One arm of a subtractive dispatch over `counter` (SPIKE:
    /// hand-rolled replacement for the per-arm `perform_if_else`
    /// guard the dispatch used before).
    ///
    /// The guard keeps `perform_if_else`'s branch shape — copy the
    /// counter into `if_value`, run the empty if-branch when it is
    /// nonzero, run `body` in the else-branch when it is zero — but
    /// `if_value`, `else_value`, and the copy's `helper` are
    /// allocated once per dispatch and shared by every arm,
    /// instead of being duplicated and reclaimed per arm. `body`
    /// must not touch `counter`, `if_value`, or `else_value` (it
    /// may use `helper`, which the guard copy leaves at zero).
    /// Afterwards the counter is decremented in place, so after a
    /// match it wraps (0 to 255, downward) and cannot reach zero
    /// again within the remaining arms (at most 255 follow a match
    /// when a dispatch holds at most 256 arms), so exactly one arm
    /// fires. All pointer motion is static, so the tracked pointer
    /// stays exact throughout.
    fn dispatch_arm(
        &mut self,
        counter: i32,
        if_value: i32,
        else_value: i32,
        helper: i32,
        mut body: impl FnMut(&mut Self) -> Result<(), CompileError>,
    ) -> Result<(), CompileError> {
        self.copy_value_with_helper(counter, if_value, helper);
        self.set_value(else_value, 1);
        self.move_pointer(if_value);
        self.emit("[");
        self.set_value(if_value, 0);
        self.set_value(else_value, 0);
        self.move_pointer(if_value);
        self.emit("]");
        self.move_pointer(else_value);
        self.emit("[");
        self.set_value(else_value, 0);
        body(self)?;
        self.move_pointer(else_value);
        self.emit("]");
        self.decrement_in_place(counter);
        Ok(())
    }

    /// Load the cell at the tape index held in `addr`, searching
    /// `cells` in order, into a fresh cell: a subtractive dispatch.
    ///
    /// (SPIKE.) Each arm is hand-rolled instead of a
    /// `perform_if_else` guard plus `copy_value`: the guard cells
    /// are shared by every arm (see [`Emitter::dispatch_arm`]),
    /// the result is zeroed once, and one helper cell serves every
    /// arm (the fired arm clears it before its copy; non-firing
    /// arms never touch it). An address with no arm leaves the
    /// result zero. The address cell and every target cell are
    /// preserved; only the result and the dispatch scratch
    /// (`counter`, guard cells, `helper`) are written.
    pub fn load_from_cells(
        &mut self,
        addr: i32,
        cells: &[i32],
    ) -> Result<i32, CompileError> {
        // The counter wraps after 256 decrements; more arms could
        // fire twice. All callers pass at most 256 cells (an array
        // dispatch covers offsets 0..255, a pointer dispatch covers
        // every byte address).
        debug_assert!(cells.len() <= 256);
        let result = self.const_value(0);
        let counter = self.duplicate_value(addr);
        // SPIKE v2: the shared guard cells are scratch, reclaimed
        // after the dispatch — without this, every dispatch site
        // leaves three cells allocated and shifts all later code
        // (v1: pointers +53 cells, +344K bytes). The result was
        // allocated before the checkpoint, so it survives.
        let checkpoint = self.checkpoint();
        let if_value = self.memory.temp_cell();
        let else_value = self.memory.temp_cell();
        let helper = self.memory.temp_cell();
        for &cell in cells {
            self.dispatch_arm(counter, if_value, else_value, helper, |em| {
                if cell == result {
                    // `copy_value` no-op: the value already sits in
                    // the result cell.
                    return Ok(());
                }
                if cell == helper {
                    // The copy loop below would spin forever with
                    // the helper sharing the source cell; fall back
                    // to `copy_value`, whose own helper is fresh.
                    em.copy_value(cell, result);
                    return Ok(());
                }
                em.move_pointer(helper);
                em.emit("[-]");
                em.move_pointer(cell);
                em.emit("[-");
                em.move_pointer(result);
                em.emit("+");
                em.move_pointer(helper);
                em.emit("+");
                em.move_pointer(cell);
                em.emit("]");
                em.move_pointer(helper);
                em.emit("[-");
                em.move_pointer(cell);
                em.emit("+");
                em.move_pointer(helper);
                em.emit("]");
                Ok(())
            })?;
        }
        self.release_temps(checkpoint);
        Ok(result)
    }

    /// Store `value` into the cell at the tape index held in `addr`,
    /// searching `cells` in order, with the same hand-rolled
    /// dispatch as [`Emitter::load_from_cells`]. An address with no
    /// arm stores nowhere. The address and value cells are
    /// preserved.
    pub fn store_to_cells(
        &mut self,
        addr: i32,
        cells: &[i32],
        value: i32,
    ) -> Result<(), CompileError> {
        debug_assert!(cells.len() <= 256);
        let counter = self.duplicate_value(addr);
        // As in `load_from_cells` (SPIKE v2): reclaim the shared
        // guard cells so later code keeps its layout.
        let checkpoint = self.checkpoint();
        let if_value = self.memory.temp_cell();
        let else_value = self.memory.temp_cell();
        let helper = self.memory.temp_cell();
        for &cell in cells {
            self.dispatch_arm(counter, if_value, else_value, helper, |em| {
                if value == cell {
                    // `copy_value` no-op.
                    return Ok(());
                }
                if cell == helper || value == helper {
                    // As in `load_from_cells`: avoid a copy loop
                    // whose helper shares the source or target.
                    em.copy_value(value, cell);
                    return Ok(());
                }
                em.move_pointer(helper);
                em.emit("[-]");
                em.move_pointer(cell);
                em.emit("[-]");
                em.move_pointer(value);
                em.emit("[-");
                em.move_pointer(cell);
                em.emit("+");
                em.move_pointer(helper);
                em.emit("+");
                em.move_pointer(value);
                em.emit("]");
                em.move_pointer(helper);
                em.emit("[-");
                em.move_pointer(value);
                em.emit("+");
                em.move_pointer(helper);
                em.emit("]");
                Ok(())
            })?;
        }
        self.release_temps(checkpoint);
        Ok(())
    }

    /// Load the cell at any byte tape index held in `addr`.
    pub fn load_through(&mut self, addr: i32) -> Result<i32, CompileError> {
        let cells: Vec<i32> = (0..256).collect();
        self.load_from_cells(addr, &cells)
    }

    /// Store `value` into the cell at any byte tape index held in
    /// `addr`.
    pub fn store_through(&mut self, addr: i32, value: i32) -> Result<(), CompileError> {
        let cells: Vec<i32> = (0..256).collect();
        self.store_to_cells(addr, &cells, value)
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
                        let q = em.divide_values_unchecked(digit, ten)?;
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
                        let q = em.divide_values_unchecked(a_copy, helper)?;
                        em.move_value(q, digit);
                        let glyph = em.add_values(digit, zero_char);
                        em.print_as_char(glyph);
                        let scaled = em.multiply_values(digit, helper);
                        let rest = em.subtract_values(a_copy, scaled);
                        em.move_value(rest, a_copy);
                        let q = em.divide_values_unchecked(helper, ten)?;
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
                        let q = em.divide_values_unchecked(digit, sixteen)?;
                        em.move_value(q, digit);
                        let h = em.multiply_values(helper, sixteen);
                        em.move_value(h, helper);
                        Ok(())
                    },
                )?;
                em.perform_while(
                    |em| Ok(helper),
                    |em| {
                        let q = em.divide_values_unchecked(a_copy, helper)?;
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
                        let q = em.divide_values_unchecked(helper, sixteen)?;
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
    /// Print the NUL-terminated string at the tape index held in
    /// `addr` (`printf %s`). A working copy of the address advances
    /// through a dispatch load per character. The loop is
    /// hand-rolled do-while style: `ch` starts nonzero so the loop
    /// is entered once, each pass loads one character into `ch`
    /// and prints/advances only when it is nonzero, and the loop
    /// re-tests `ch` itself. A `perform_while` shape would evaluate
    /// its condition twice during emission and pay the 256-arm
    /// dispatch twice per site; this shape pays it once. The
    /// string must be NUL-terminated; reading past the end
    /// continues into following cells until a zero cell.
    pub fn print_as_string(&mut self, addr: i32) -> Result<(), CompileError> {
        let cur = self.duplicate_value(addr);
        let ch = self.const_value(1);
        let checkpoint = self.checkpoint();
        self.move_pointer(ch);
        self.emit("[");
        let loaded = self.load_through(cur)?;
        self.copy_value(loaded, ch);
        let one = self.const_value(1);
        self.perform_if_else(
            ch,
            |em| {
                em.print_as_char(ch);
                let next = em.add_values(cur, one);
                em.copy_value(next, cur);
                Ok(())
            },
            |_| Ok(()),
        )?;
        self.move_pointer(ch);
        self.emit("]");
        self.release_temps(checkpoint);
        Ok(())
    }

    pub fn read_as_char(&mut self, a: i32) {
        self.move_pointer(a);
        self.emit(",");
    }

    /// Read a decimal number into cell `a` (`readAsNumber`).
    ///
    /// A `-` before the first digit negates the value (wrapping, so
    /// `-5` reads as 251); a `+` is ignored, as is anything else
    /// skipped before the digits, as before. The sign only applies
    /// to characters skipped before the first digit: a `-` after
    /// digits start terminates the number, unchanged.
    pub fn read_as_number(&mut self, a: i32) -> Result<(), CompileError> {
        let reader = self.const_value(0);
        let result = self.const_value(0);
        let zero = self.const_value(b'0');
        let nine = self.const_value(b'9');
        let ten = self.const_value(10);
        let minus = self.const_value(b'-');
        let negative = self.const_value(0);
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
                let is_minus = em.is_equal(reader, minus)?;
                em.perform_if_else(
                    is_minus,
                    |em| {
                        em.set_value(negative, 1);
                        Ok(())
                    },
                    |_| Ok(()),
                )?;
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
        // Apply the sign, if one was seen: 0 - result, wrapping.
        self.perform_if_else(
            negative,
            |em| {
                let zero_value = em.const_value(0);
                let negated = em.subtract_values(zero_value, result);
                em.move_value(negated, result);
                Ok(())
            },
            |_| Ok(()),
        )?;
        self.move_value(result, a);
        Ok(())
    }

    /// Whether `cell` holds a C whitespace byte: 9..13 or 32.
    fn is_space(&mut self, cell: i32) -> Result<i32, CompileError> {
        let nine = self.const_value(9);
        let thirteen = self.const_value(13);
        let space = self.const_value(32);
        let ge = self.greater_than_or_equal(cell, nine)?;
        let le = self.less_than_or_equal(cell, thirteen)?;
        let in_range = self.logical_and(ge, le)?;
        let is_sp = self.is_equal(cell, space)?;
        self.logical_or(in_range, is_sp)
    }

    /// Read one input byte into `cell`, first resetting it to zero.
    /// The runner leaves the cell unchanged at EOF, so a zero here
    /// means end of input (a NUL byte in the input reads the same
    /// way; `scanf %s` is a text-input conversion).
    fn read_char_into(&mut self, cell: i32) {
        self.set_value(cell, 0);
        self.move_pointer(cell);
        self.emit(",");
    }

    /// Read a whitespace-delimited string (`scanf %s`) into the
    /// array at `base` with `len` elements, truncating the token to
    /// `len - 1` characters so a NUL terminator always fits (at
    /// most 256 cells are addressed, as for variable-index stores;
    /// a longer array keeps only its first 256 cells reachable
    /// here). Leading whitespace is skipped, as in C. The token
    /// ends at whitespace or end of input, and the terminating
    /// whitespace is consumed, like the terminating character of
    /// `%d` here — it is not pushed back, unlike C. If end of
    /// input arrives before any token character, nothing is
    /// written: the array keeps its previous contents and no NUL
    /// is stored. The rest of an over-long token is read and
    /// discarded, so a following conversion sees the next token.
    pub fn read_as_string_bounded(
        &mut self,
        base: i32,
        len: usize,
    ) -> Result<(), CompileError> {
        let eff = len.min(256);
        let cells: Vec<i32> = (base..base + eff as i32).collect();
        let reader = self.const_value(0);
        let going = self.const_value(1);
        let seen = self.const_value(0);
        let zero = self.const_value(0);
        let one = self.const_value(1);
        let offset = self.const_value(0);
        // Characters still writable, counting the NUL slot: a zero
        // byte here means 256 (only when `eff` is 256). The token
        // stops storing when one slot is left.
        let remaining = self.const_value(eff as u8);
        self.read_char_into(reader);
        self.perform_while(
            |em| em.is_space(reader),
            |em| {
                em.read_char_into(reader);
                Ok(())
            },
        )?;
        self.perform_while(
            |_| Ok(going),
            |em| {
                let is_zero = em.is_equal(reader, zero)?;
                let space = em.is_space(reader)?;
                let end = em.logical_or(is_zero, space)?;
                em.perform_if_else(
                    end,
                    |em| {
                        em.set_value(going, 0);
                        Ok(())
                    },
                    |em| {
                        em.set_value(seen, 1);
                        let at_limit = em.is_equal(remaining, one)?;
                        em.perform_if_else(
                            at_limit,
                            |_| Ok(()),
                            |em| {
                                em.store_to_cells(offset, &cells, reader)?;
                                let next = em.add_values(offset, one);
                                em.move_value(next, offset);
                                let left = em.subtract_values(remaining, one);
                                em.move_value(left, remaining);
                                Ok(())
                            },
                        )?;
                        em.read_char_into(reader);
                        Ok(())
                    },
                )
            },
        )?;
        self.perform_if_else(
            seen,
            |em| {
                em.store_to_cells(offset, &cells, zero)?;
                Ok(())
            },
            |_| Ok(()),
        )
    }

    /// Read a whitespace-delimited string (`scanf %s`) through the
    /// runtime address held in `addr`, writing characters until
    /// whitespace, end of input, or the end of the addressable
    /// tape (cell 255). Unlike the bounded form, there is no known
    /// size here, so the write is unbounded like C: the caller
    /// must provide room. A string that fills the tape to cell 255
    /// is left unterminated, as there is nowhere to put the NUL.
    /// Leading-whitespace skipping, end-of-input, and delimiter
    /// handling are as in [`Emitter::read_as_string_bounded`].
    pub fn read_as_string_through(&mut self, addr: i32) -> Result<(), CompileError> {
        let reader = self.const_value(0);
        let going = self.const_value(1);
        let seen = self.const_value(0);
        let zero = self.const_value(0);
        let one = self.const_value(1);
        let max = self.const_value(255);
        let cur = self.duplicate_value(addr);
        let full = self.const_value(0);
        self.read_char_into(reader);
        self.perform_while(
            |em| em.is_space(reader),
            |em| {
                em.read_char_into(reader);
                Ok(())
            },
        )?;
        self.perform_while(
            |_| Ok(going),
            |em| {
                let is_zero = em.is_equal(reader, zero)?;
                let space = em.is_space(reader)?;
                let end = em.logical_or(is_zero, space)?;
                em.perform_if_else(
                    end,
                    |em| {
                        em.set_value(going, 0);
                        Ok(())
                    },
                    |em| {
                        em.set_value(seen, 1);
                        em.perform_if_else(
                            full,
                            |_| Ok(()),
                            |em| {
                                em.store_through(cur, reader)?;
                                let at_end = em.is_equal(cur, max)?;
                                em.perform_if_else(
                                    at_end,
                                    |em| {
                                        em.set_value(full, 1);
                                        Ok(())
                                    },
                                    |em| {
                                        let next = em.add_values(cur, one);
                                        em.move_value(next, cur);
                                        Ok(())
                                    },
                                )?;
                                Ok(())
                            },
                        )?;
                        em.read_char_into(reader);
                        Ok(())
                    },
                )
            },
        )?;
        self.perform_if_else(
            seen,
            |em| {
                em.perform_if_else(
                    full,
                    |_| Ok(()),
                    |em| {
                        em.store_through(cur, zero)?;
                        Ok(())
                    },
                )?;
                Ok(())
            },
            |_| Ok(()),
        )
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

    /// Run the emitter's output, then print each listed cell in
    /// order and return the printed bytes.
    fn run_cells(em: &mut Emitter, cells: &[i32]) -> Vec<u8> {
        for &cell in cells {
            em.move_pointer(cell);
            em.emit(".");
        }
        let code = em.output().to_vec();
        let mut out = Vec::new();
        runner::run(&code, &mut std::io::empty(), &mut out).expect("bf runs");
        out
    }

    #[test]
    fn bitwise_and_or_xor_values() {
        for (a, b, and, or, xor) in [
            (0b1100u8, 0b1010u8, 0b1000u8, 0b1110u8, 0b0110u8),
            (0xff, 0x0f, 0x0f, 0xff, 0xf0),
            (0, 0, 0, 0, 0),
            (0x55, 0xaa, 0x00, 0xff, 0xff),
            (0x01, 0x80, 0x00, 0x81, 0x81),
            (0xff, 0xff, 0xff, 0xff, 0x00),
            (0x00, 0xff, 0x00, 0xff, 0xff),
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
            (255, 1, 254, 127),
            (0, 5, 0, 0),
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
    fn divmod_two_quotient_and_remainder() {
        for a in 0..=u8::MAX {
            // Fresh emitter per check, mirroring run_cell's single-run
            // use (it appends pointer motion to the output).
            let mut em = Emitter::new();
            let ca = em.const_value(a);
            let r = em.divmod_two(ca).unwrap();
            assert_eq!(run_cell(&mut em, r), a % 2, "{a} % 2");
            let mut em = Emitter::new();
            let ca = em.const_value(a);
            em.divmod_two(ca).unwrap();
            assert_eq!(run_cell(&mut em, ca), a / 2, "{a} / 2");
        }
    }

    #[test]
    fn double_and_halve_in_place_exhaustive() {
        for a in 0..=u8::MAX {
            let mut em = Emitter::new();
            let ca = em.const_value(a);
            em.double_in_place(ca);
            assert_eq!(run_cell(&mut em, ca), a.wrapping_mul(2), "double {a}");
            let mut em = Emitter::new();
            let ca = em.const_value(a);
            em.halve_in_place(ca).unwrap();
            assert_eq!(run_cell(&mut em, ca), a / 2, "halve {a}");
        }
    }

    #[test]
    fn branch_and_loop_temps_are_reclaimed() {
        // if/else: the second region reuses the first region's indices.
        let mut em = Emitter::new();
        let cond = em.const_value(1);
        let before = em.allocated_cells();
        em.perform_if_else(cond, |em| { em.const_value(7); Ok(()) }, |_| Ok(()))
            .unwrap();
        let after_one = em.allocated_cells();
        assert!(after_one > before);
        em.perform_if_else(cond, |em| { em.const_value(7); Ok(()) }, |_| Ok(()))
            .unwrap();
        assert_eq!(em.allocated_cells(), after_one);
        // while: a zero-iteration loop's condition/body temps are
        // reclaimed, so the next free index is unchanged (the
        // high-water mark still records the indices the loop's
        // emitted code visits while it runs).
        let mut em = Emitter::new();
        let counter = em.const_value(0);
        let before = em.checkpoint();
        em.perform_while(
            |em| {
                let one = em.const_value(1);
                Ok(em.greater_than_or_equal(counter, one)?)
            },
            |em| {
                em.const_value(9);
                Ok(())
            },
        )
        .unwrap();
        assert_eq!(em.checkpoint(), before);
        // Values written through reused indices still execute
        // correctly: two sequential branches write distinct
        // pre-allocated cells.
        let build = |which: u8| {
            let mut em = Emitter::new();
            let cond = em.const_value(1);
            let r1 = em.const_value(0);
            let r2 = em.const_value(0);
            em.perform_if_else(
                cond,
                |em| {
                    em.set_value(r1, 5);
                    Ok(())
                },
                |_| Ok(()),
            )
            .unwrap();
            em.perform_if_else(
                cond,
                |em| {
                    em.set_value(r2, 6);
                    Ok(())
                },
                |_| Ok(()),
            )
            .unwrap();
            (em, if which == 1 { r1 } else { r2 })
        };
        let (mut em, cell) = build(1);
        assert_eq!(run_cell(&mut em, cell), 5);
        let (mut em, cell) = build(2);
        assert_eq!(run_cell(&mut em, cell), 6);
    }

    #[test]
    fn bit_combine_values() {
        for (a, b, and, or, xor) in [
            (0u8, 0u8, 0u8, 0u8, 0u8),
            (0, 1, 0, 1, 1),
            (1, 0, 0, 1, 1),
            (1, 1, 1, 1, 0),
        ] {
            let mut em = Emitter::new();
            let ca = em.const_value(a);
            let cb = em.const_value(b);
            let r = em.combine_and(ca, cb).unwrap();
            assert_eq!(run_cell(&mut em, r), and, "{a} & {b}");
            let mut em = Emitter::new();
            let ca = em.const_value(a);
            let cb = em.const_value(b);
            let r = em.combine_or(ca, cb).unwrap();
            assert_eq!(run_cell(&mut em, r), or, "{a} | {b}");
            let mut em = Emitter::new();
            let ca = em.const_value(a);
            let cb = em.const_value(b);
            let r = em.combine_xor(ca, cb).unwrap();
            assert_eq!(run_cell(&mut em, r), xor, "{a} ^ {b}");
        }
    }

    #[test]
    fn add_copy_to_preserves_source() {
        for (s, d, want) in [
            (0u8, 0u8, 0u8),
            (0, 255, 255),
            (255, 0, 255),
            (255, 255, 254),
            (200, 100, 44),
            (128, 128, 0),
            (1, 254, 255),
        ] {
            let mut em = Emitter::new();
            let cs = em.const_value(s);
            let cd = em.const_value(d);
            em.add_copy_to(cs, cd);
            assert_eq!(run_cell(&mut em, cd), want, "{s} + {d}");
            let mut em = Emitter::new();
            let cs = em.const_value(s);
            let cd = em.const_value(d);
            em.add_copy_to(cs, cd);
            assert_eq!(run_cell(&mut em, cs), s, "source {s} preserved");
        }
    }

    #[test]
    fn accumulate_bit_adds_place_when_set() {
        for (bit, place, result, want) in [
            (0u8, 1u8, 0u8, 0u8),
            (0, 128, 200, 200),
            (1, 1, 0, 1),
            (1, 2, 3, 5),
            (1, 128, 200, 72),
            (1, 255, 255, 254),
            (1, 4, 0, 4),
        ] {
            let mut em = Emitter::new();
            let cb = em.const_value(bit);
            let cr = em.const_value(result);
            let cp = em.const_value(place);
            em.accumulate_bit(cb, cr, cp).unwrap();
            assert_eq!(run_cell(&mut em, cr), want, "bit {bit} place {place}");
            let mut em = Emitter::new();
            let cb = em.const_value(bit);
            let cr = em.const_value(result);
            let cp = em.const_value(place);
            em.accumulate_bit(cb, cr, cp).unwrap();
            assert_eq!(run_cell(&mut em, cp), place, "place {place} preserved");
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

    #[test]
    fn load_from_cells_exhaustive() {
        for addr in [0u8, 1, 2, 3, 4, 5, 255] {
            for values in [[7u8, 99, 0, 255], [255, 1, 128, 64]] {
                let mut em = Emitter::new();
                let targets: Vec<i32> =
                    values.iter().map(|&v| em.const_value(v)).collect();
                let sentinel = em.const_value(77);
                let addr_cell = em.const_value(addr);
                let result = em.load_from_cells(addr_cell, &targets).unwrap();
                let mut probe = vec![result, addr_cell, sentinel];
                probe.extend_from_slice(&targets);
                let out = run_cells(&mut em, &probe);
                let want_target = if (addr as usize) < targets.len() {
                    values[addr as usize]
                } else {
                    0
                };
                let mut want = vec![want_target, addr, 77];
                want.extend_from_slice(&values);
                assert_eq!(out, want, "addr {addr} values {values:?}");
            }
        }
    }

    #[test]
    fn store_to_cells_exhaustive() {
        for addr in [0u8, 1, 2, 3, 4, 5, 255] {
            let mut em = Emitter::new();
            let targets: Vec<i32> = [11u8, 22, 33, 44]
                .iter()
                .map(|&v| em.const_value(v))
                .collect();
            let sentinel = em.const_value(77);
            let value = em.const_value(201);
            let addr_cell = em.const_value(addr);
            em.store_to_cells(addr_cell, &targets, value).unwrap();
            let mut probe = vec![value, addr_cell, sentinel];
            probe.extend_from_slice(&targets);
            let out = run_cells(&mut em, &probe);
            let mut want_values = [11u8, 22, 33, 44];
            if (addr as usize) < want_values.len() {
                want_values[addr as usize] = 201;
            }
            let mut want = vec![201, addr, 77];
            want.extend_from_slice(&want_values);
            assert_eq!(out, want, "addr {addr}");
        }
    }

    #[test]
    fn load_and_store_through_full_range() {
        // Tape indices are cell indices: burn the low indices so
        // dispatch temporaries allocate above the setup cells, then
        // set values by literal index.
        for addr in [0u8, 3, 7, 200] {
            let mut em = Emitter::new();
            for _ in 0..64 {
                em.memory.temp_cell();
            }
            em.set_value(3, 30);
            em.set_value(7, 70);
            em.set_value(50, addr);
            let result = em.load_through(50).unwrap();
            let out = run_cells(&mut em, &[result, 50, 3, 7]);
            let want = if addr == 3 {
                30
            } else if addr == 7 {
                70
            } else {
                0
            };
            assert_eq!(out, vec![want, addr, 30, 70], "load addr {addr}");
        }
        for addr in [0u8, 3, 7, 200] {
            let mut em = Emitter::new();
            for _ in 0..64 {
                em.memory.temp_cell();
            }
            em.set_value(50, addr);
            em.set_value(60, 42);
            em.store_through(50, 60).unwrap();
            // Probe: addr cell, value cell, target cell, and the
            // neighbors of cell 200 (for addr 200 they must stay
            // zero around the stored cell).
            let out = run_cells(&mut em, &[50, 60, addr as i32, 199, 201]);
            assert_eq!(out[0], addr, "addr preserved {addr}");
            assert_eq!(out[1], 42, "value preserved {addr}");
            assert_eq!(out[2], 42, "stored value at {addr}");
            if addr == 200 {
                assert_eq!(out[3], 0, "left neighbor untouched");
                assert_eq!(out[4], 0, "right neighbor untouched");
            }
        }
    }

    #[test]
    fn dispatch_with_no_cells_is_noop() {
        let mut em = Emitter::new();
        let addr_cell = em.const_value(3);
        let result = em.load_from_cells(addr_cell, &[]).unwrap();
        assert_eq!(run_cell(&mut em, result), 0);
        let mut em = Emitter::new();
        let addr_cell = em.const_value(3);
        let value = em.const_value(9);
        em.store_to_cells(addr_cell, &[], value).unwrap();
        assert_eq!(run_cell(&mut em, value), 9);
    }

    #[test]
    fn dispatch_store_with_value_as_target_is_noop() {
        // A store whose value cell is the target is a no-op, as in
        // `copy_value`: the hand-rolled copy would zero the cell
        // before reading it.
        let mut em = Emitter::new();
        em.set_value(10, 99);
        em.set_value(50, 0);
        em.store_to_cells(50, &[10, 20], 10).unwrap();
        assert_eq!(run_cell(&mut em, 10), 99);
    }
}
