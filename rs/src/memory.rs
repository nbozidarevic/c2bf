//! Compile-time tape-cell allocator for Brainfuck code generation.
//!
//! Ported from the retired C++ `BrainfuckMemory`: a bump allocator handing out
//! cell indices from 0 upward. Named variables are memoized: asking twice
//! for the same name yields the same cell. Indices only grow, except for
//! scoped reclaim of temporaries inside branch/loop emission (see
//! [`Memory::release_temps`]): cells allocated since a checkpoint are
//! handed out again once the construct that owned them is done, while
//! variable cells are never reclaimed.

use std::collections::HashMap;

/// Bump-allocating tape-cell index assignment.
#[derive(Debug, Default)]
pub struct Memory {
    next: i32,
    high_water: i32,
    vars: HashMap<Vec<u8>, i32>,
}

impl Memory {
    /// Fresh allocator; first index handed out is 0.
    pub fn new() -> Self {
        Self::default()
    }

    /// Index the next fresh temporary cell would get, without
    /// allocating it, so a caller can price a plan that may need a
    /// scratch cell before committing to one.
    pub fn peek_temp_cell(&self) -> i32 {
        self.next
    }

    /// Highest cell index ever handed out, plus one: the tape
    /// footprint the transpiler reserved for a program. Unlike
    /// [`Memory::checkpoint`], this never shrinks when temporaries
    /// are reclaimed, because emitted code still visits the
    /// reclaimed indices while its region executes.
    pub fn allocated_cells(&self) -> usize {
        self.high_water as usize
    }

    /// Checkpoint for scoped temporary reclaim: the next index that
    /// would be handed out right now (see [`Memory::peek_temp_cell`]).
    pub fn checkpoint(&self) -> i32 {
        self.next
    }

    /// Reclaim temporary cells allocated at or after `checkpoint`,
    /// so later code reuses those indices.
    ///
    /// Only temporaries may be reclaimed: the floor is one past the
    /// largest variable cell allocated, so a variable declared
    /// inside the reclaimed region keeps its cell (temporaries
    /// allocated after it are still reclaimed). A stale checkpoint
    /// from before an earlier reclaim never grows `next` again.
    /// Callers must guarantee that no value live after the region
    /// sits in a reclaimed cell; branch/loop emission satisfies
    /// this because values only flow out through cells allocated
    /// before the region or through variables.
    pub fn release_temps(&mut self, checkpoint: i32) {
        let floor = self.vars.values().copied().max().unwrap_or(-1) + 1;
        self.next = checkpoint.clamp(floor, self.next);
    }

    /// Allocate a fresh temporary cell (`getTemporaryCell`).
    pub fn temp_cell(&mut self) -> i32 {
        let cell = self.next;
        self.next += 1;
        self.high_water = self.high_water.max(self.next);
        cell
    }

    /// Look up (allocating on first use) the cell for `name`
    /// (`getVariableCell`).
    pub fn var_cell(&mut self, name: &[u8]) -> i32 {
        if let Some(&cell) = self.vars.get(name) {
            return cell;
        }
        let cell = self.temp_cell();
        self.vars.insert(name.to_vec(), cell);
        cell
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn peek_does_not_allocate() {
        let mut m = Memory::new();
        assert_eq!(m.peek_temp_cell(), 0);
        assert_eq!(m.peek_temp_cell(), 0);
        assert_eq!(m.temp_cell(), 0);
        assert_eq!(m.peek_temp_cell(), 1);
    }

    #[test]
    fn temps_count_from_zero() {
        let mut m = Memory::new();
        assert_eq!(m.temp_cell(), 0);
        assert_eq!(m.temp_cell(), 1);
        assert_eq!(m.temp_cell(), 2);
    }

    #[test]
    fn vars_are_memoized() {
        let mut m = Memory::new();
        assert_eq!(m.var_cell(b"x"), 0);
        assert_eq!(m.var_cell(b"y"), 1);
        assert_eq!(m.var_cell(b"x"), 0);
        assert_eq!(m.var_cell(b"y"), 1);
    }

    #[test]
    fn temps_and_vars_share_one_sequence() {
        let mut m = Memory::new();
        assert_eq!(m.temp_cell(), 0);
        assert_eq!(m.var_cell(b"a"), 1);
        assert_eq!(m.var_cell(b"a"), 1);
        assert_eq!(m.temp_cell(), 2);
        assert_eq!(m.var_cell(b"b"), 3);
    }

    #[test]
    fn release_reclaims_temps_and_keeps_high_water() {
        let mut m = Memory::new();
        assert_eq!(m.temp_cell(), 0);
        let checkpoint = m.checkpoint();
        assert_eq!(m.temp_cell(), 1);
        assert_eq!(m.temp_cell(), 2);
        assert_eq!(m.allocated_cells(), 3);
        m.release_temps(checkpoint);
        assert_eq!(m.peek_temp_cell(), 1);
        assert_eq!(m.temp_cell(), 1);
        assert_eq!(m.allocated_cells(), 3);
    }

    #[test]
    fn release_never_reclaims_variables() {
        let mut m = Memory::new();
        let checkpoint = m.checkpoint();
        assert_eq!(m.var_cell(b"a"), 0);
        assert_eq!(m.temp_cell(), 1);
        m.release_temps(checkpoint);
        assert_eq!(m.peek_temp_cell(), 1);
        assert_eq!(m.var_cell(b"a"), 0);
        assert_eq!(m.var_cell(b"b"), 1);
    }

    #[test]
    fn release_with_stale_checkpoint_never_grows() {
        let mut m = Memory::new();
        assert_eq!(m.temp_cell(), 0);
        m.release_temps(99);
        assert_eq!(m.peek_temp_cell(), 1);
        m.release_temps(-5);
        assert_eq!(m.peek_temp_cell(), 1);
    }

    #[test]
    fn similar_names_are_distinct() {
        let mut m = Memory::new();
        assert_eq!(m.var_cell(b"x"), 0);
        assert_eq!(m.var_cell(b"xx"), 1);
        assert_eq!(m.var_cell(b"X"), 2);
        assert_eq!(m.var_cell(b"x"), 0);
    }
}
