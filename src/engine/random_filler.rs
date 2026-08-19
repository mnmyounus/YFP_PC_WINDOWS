//! Fills a buffer with pseudo-random bytes fast enough to sustain
//! multi-hundred-MB/s writes.
//!
//! This deliberately uses `rand::rngs::SmallRng` (a fast, non-cryptographic
//! PRNG -- Xoshiro256++ under the hood) rather than a CSPRNG like
//! `rand::rngs::OsRng`. This mirrors the same reasoning as the Android
//! app's `FastRandomFiller`: a CSPRNG is deliberately slow (drawing from an
//! algorithm designed to resist cryptanalysis, meant for key material, not
//! bulk throughput) and would become the actual bottleneck instead of the
//! disk itself. The goal of the pseudo-random pattern here is to make sure
//! *new, non-repeating* bit patterns land on disk -- defeating naive
//! "look for the old file's byte signature" recovery and defeating
//! sparse-file shortcuts some filesystems take with long zero runs -- not
//! to produce bytes that need to resist cryptanalysis. `SmallRng` is seeded
//! from the OS's real entropy source (via `SeedableRng::from_entropy`, which
//! itself draws from `OsRng`) so the starting state isn't predictable
//! across runs, even though the per-byte generation algorithm afterward is
//! intentionally the fast, non-crypto one.

use rand::rngs::SmallRng;
use rand::{RngCore, SeedableRng};

pub struct FastRandomFiller {
    rng: SmallRng,
}

impl FastRandomFiller {
    pub fn new() -> Self {
        Self {
            rng: SmallRng::from_entropy(),
        }
    }

    /// Fills `buffer` entirely with pseudo-random bytes.
    pub fn fill(&mut self, buffer: &mut [u8]) {
        self.rng.fill_bytes(buffer);
    }
}

impl Default for FastRandomFiller {
    fn default() -> Self {
        Self::new()
    }
}
