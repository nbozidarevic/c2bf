//! Compile-time tape-cell allocator for Brainfuck code generation.
//!
//! Ported from the retired C++ `BrainfuckMemory`: a bump allocator handing out
//! cell indices from 0 upward. Named variables are memoized: asking twice
//! for the same name yields the same cell. Indices only grow, except for
//! scoped reclaim of temporaries inside branch/loop emission (see
//! [`Memory::release_temps`]): cells allocated since a checkpoint are
//! handed out again once the construct that owned them is done, while
//! variable and array cells are never reclaimed.

use std::collections::{HashMap, HashSet};

/// Bump-allocating tape-cell index assignment.
#[derive(Debug, Default)]
pub struct Memory {
    next: i32,
    high_water: i32,
    vars: HashMap<Vec<u8>, i32>,
    /// Array name to (base cell, element count). Array elements are
    /// consecutive cells starting at the base, allocated like
    /// temporaries but never reclaimed. A name lives in `arrays` or
    /// in `vars`, never both (pointer names are in `vars`, and
    /// additionally recorded in `pointers`).
    arrays: HashMap<Vec<u8>, (i32, usize)>,
    /// Names declared with a single-`*` declarator. A pointer
    /// variable's cell holds a tape index (a byte), like any scalar;
    /// the set only records which names are pointers.
    pointers: HashSet<Vec<u8>>,
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
    /// largest variable or array cell allocated, so a variable or
    /// array declared inside the reclaimed region keeps its cells
    /// (temporaries allocated after it are still reclaimed). A stale
    /// checkpoint from before an earlier reclaim never grows `next`
    /// again. Callers must guarantee that no value live after the
    /// region sits in a reclaimed cell; branch/loop emission
    /// satisfies this because values only flow out through cells
    /// allocated before the region or through variables.
    pub fn release_temps(&mut self, checkpoint: i32) {
        let var_top = self.vars.values().copied().max().unwrap_or(-1);
        let array_top = self
            .arrays
            .values()
            .map(|&(base, len)| base + len as i32 - 1)
            .max()
            .unwrap_or(-1);
        let floor = var_top.max(array_top) + 1;
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

    /// Whether `name` already denotes a scalar variable (pointer
    /// variables included) rather than an array.
    pub fn has_var(&self, name: &[u8]) -> bool {
        self.vars.contains_key(name)
    }

    /// Cell already assigned to `name`, if any, without allocating.
    pub fn var_index(&self, name: &[u8]) -> Option<i32> {
        self.vars.get(name).copied()
    }

    /// Whether `name` denotes an array.
    pub fn is_array(&self, name: &[u8]) -> bool {
        self.arrays.contains_key(name)
    }

    /// Whether `name` was declared with a pointer declarator.
    pub fn is_pointer(&self, name: &[u8]) -> bool {
        self.pointers.contains(name)
    }

    /// Base cell and length of the array `name`, if declared.
    pub fn array_info(&self, name: &[u8]) -> Option<(i32, usize)> {
        self.arrays.get(name).copied()
    }

    /// Reserve `len` consecutive cells for the array `name`, starting
    /// at the returned base cell. The caller guarantees the name is
    /// not already declared (see [`Memory::is_array`] and
    /// [`Memory::has_var`]); redeclaring an array returns its
    /// original base.
    pub fn alloc_array(&mut self, name: &[u8], len: usize) -> i32 {
        if let Some(&(base, _)) = self.arrays.get(name) {
            return base;
        }
        let base = self.next;
        for _ in 0..len {
            self.temp_cell();
        }
        self.arrays.insert(name.to_vec(), (base, len));
        base
    }

    /// Record `name` as a pointer variable. The cell itself comes
    /// from [`Memory::var_cell`]; this only records the kind.
    pub fn mark_pointer(&mut self, name: &[u8]) {
        self.pointers.insert(name.to_vec());
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
    fn release_clamps_out_of_range_checkpoints() {
        let mut m = Memory::new();
        assert_eq!(m.temp_cell(), 0);
        // A future checkpoint never grows `next` again.
        m.release_temps(99);
        assert_eq!(m.peek_temp_cell(), 1);
        // A checkpoint below the floor clamps to the floor (0 here).
        m.release_temps(-5);
        assert_eq!(m.peek_temp_cell(), 0);
    }

    #[test]
    fn similar_names_are_distinct() {
        let mut m = Memory::new();
        assert_eq!(m.var_cell(b"x"), 0);
        assert_eq!(m.var_cell(b"xx"), 1);
        assert_eq!(m.var_cell(b"X"), 2);
        assert_eq!(m.var_cell(b"x"), 0);
    }

    #[test]
    fn arrays_are_contiguous_and_memoized() {
        let mut m = Memory::new();
        assert_eq!(m.alloc_array(b"a", 3), 0);
        assert_eq!(m.array_info(b"a"), Some((0, 3)));
        assert_eq!(m.alloc_array(b"a", 3), 0);
        assert_eq!(m.alloc_array(b"b", 2), 3);
        assert_eq!(m.array_info(b"b"), Some((3, 2)));
        assert_eq!(m.allocated_cells(), 5);
        assert!(m.is_array(b"a"));
        assert!(!m.is_array(b"x"));
    }

    #[test]
    fn arrays_survive_temp_reclaim() {
        let mut m = Memory::new();
        assert_eq!(m.alloc_array(b"a", 2), 0);
        let checkpoint = m.checkpoint();
        assert_eq!(m.temp_cell(), 2);
        m.release_temps(checkpoint);
        // The reclaim floor covers array cells too, so the array
        // keeps its cells and the reclaimed temp is not reused
        // over them.
        assert_eq!(m.array_info(b"a"), Some((0, 2)));
        assert_eq!(m.peek_temp_cell(), 2);
    }

    #[test]
    fn pointers_are_recorded_beside_vars() {
        let mut m = Memory::new();
        let cell = m.var_cell(b"p");
        assert!(!m.is_pointer(b"p"));
        m.mark_pointer(b"p");
        assert!(m.is_pointer(b"p"));
        assert!(m.has_var(b"p"));
        assert_eq!(m.var_index(b"p"), Some(cell));
        assert_eq!(m.var_index(b"q"), None);
        assert_eq!(m.var_cell(b"p"), cell);
    }
}
