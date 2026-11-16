use std::time::Duration;

use sysinfo::{MemoryRefreshKind, RefreshKind, System};

use super::error::ReplayError;

const MEBIBYTE: u64 = 1024 * 1024;
const SMALLEST_BUDGET_BYTES: u64 = 256 * MEBIBYTE;
const LARGEST_BUDGET_BYTES: u64 = 2048 * MEBIBYTE;
const BUDGET_PERCENT_OF_INSTALLED_MEMORY: u64 = 3;

/// The default ceiling for a replay buffer held in memory: 3% of installed
/// memory, never below 256 MiB and never above 2 GiB.
pub fn memory_budget_bytes(installed_memory_bytes: u64) -> u64 {
    (installed_memory_bytes / 100)
        .saturating_mul(BUDGET_PERCENT_OF_INSTALLED_MEMORY)
        .clamp(SMALLEST_BUDGET_BYTES, LARGEST_BUDGET_BYTES)
}

/// Zero when the operating system does not report it, which
/// [`memory_budget_bytes`] turns into the smallest budget.
pub fn installed_memory_bytes() -> u64 {
    System::new_with_specifics(
        RefreshKind::nothing().with_memory(MemoryRefreshKind::nothing().with_ram()),
    )
    .map_or(0, |system| system.total_memory())
}

/// How long a stretch the buffer keeps: 15 seconds to 30 minutes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct ReplayLength(Duration);

impl ReplayLength {
    pub const SHORTEST: Self = Self(Duration::from_secs(15));
    pub const LONGEST: Self = Self(Duration::from_mins(30));
    pub const DEFAULT: Self = Self(Duration::from_mins(1));

    pub fn new(length: Duration) -> Result<Self, ReplayError> {
        if (Self::SHORTEST.0..=Self::LONGEST.0).contains(&length) {
            Ok(Self(length))
        } else {
            Err(ReplayError::LengthOutOfRange { asked: length })
        }
    }

    pub fn get(self) -> Duration {
        self.0
    }
}

impl Default for ReplayLength {
    fn default() -> Self {
        Self::DEFAULT
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BackendKind {
    Memory,
    Disk,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BackendChoice {
    /// Memory while the expected size fits the budget, disk beyond it.
    Auto {
        memory_budget_bytes: u64,
    },
    Memory,
    Disk,
}

/// The size a full buffer reaches when all streams together stay at or under
/// `max_bitrate_bits_per_second`.
pub fn expected_size_bytes(max_bitrate_bits_per_second: u64, length: ReplayLength) -> u64 {
    let bits = u128::from(max_bitrate_bits_per_second) * length.get().as_millis() / 1000;
    u64::try_from(bits / 8).unwrap_or(u64::MAX)
}

pub fn choose_backend(expected_size_bytes: u64, choice: BackendChoice) -> BackendKind {
    match choice {
        BackendChoice::Memory => BackendKind::Memory,
        BackendChoice::Disk => BackendKind::Disk,
        BackendChoice::Auto {
            memory_budget_bytes,
        } => {
            if expected_size_bytes <= memory_budget_bytes {
                BackendKind::Memory
            } else {
                BackendKind::Disk
            }
        }
    }
}
