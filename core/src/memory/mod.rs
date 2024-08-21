mod columns;
mod global;
mod local;
mod program;
mod trace;

pub use columns::*;
pub use global::*;
pub use local::*;
pub use program::*;

use serde::{Deserialize, Serialize};

use crate::runtime::MemoryRecord;

/// The type of memory chip that is being initialized.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MemoryChipType {
    Initialize,
    Finalize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryInitializeFinalizeEvent {
    pub addr: u32,
    pub value: u32,
    pub shard: u32,
    pub timestamp: u32,
    pub used: u32,
}

impl MemoryInitializeFinalizeEvent {
    pub const fn initialize(addr: u32, value: u32, used: bool) -> Self {
        // All memory initialization happen at shard 0, timestamp 0.
        Self {
            addr,
            value,
            shard: 1,
            timestamp: 1,
            used: if used { 1 } else { 0 },
        }
    }

    pub const fn finalize_from_record(addr: u32, record: &MemoryRecord) -> Self {
        Self {
            addr,
            value: record.value,
            shard: record.shard,
            timestamp: record.clk,
            used: 1,
        }
    }
}