use sp1_derive::AlignedBorrow;

use crate::{mem::MemoryPreprocessedCols, poseidon2_skinny::WIDTH};

#[derive(AlignedBorrow, Clone, Copy, Debug)]
#[repr(C)]
pub struct RoundCountersPreprocessedCols<T: Copy> {
    pub is_external_round: T,
    pub is_internal_round: T,
    pub is_first_round: T,
    pub round_constants: [T; WIDTH],
}

#[derive(AlignedBorrow, Clone, Copy, Debug)]
#[repr(C)]
pub struct Poseidon2PreprocessedCols<T: Copy> {
    pub memory_preprocessed: [MemoryPreprocessedCols<T>; WIDTH],
    pub round_counters_preprocessed: RoundCountersPreprocessedCols<T>,
}