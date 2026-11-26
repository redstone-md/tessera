// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! A small random number generator: xorshift64*, good for games, not for
//! secrets.

#[derive(Clone, Debug)]
pub struct XorShift {
    state: u64,
}

impl XorShift {
    /// A generator started from `seed`, such as the time; any seed works.
    pub fn new(seed: u64) -> Self {
        // One round of splitmix64 spreads seeds that differ little, and
        // keeps the state from being zero, where xorshift would stay.
        let mut mixed = seed.wrapping_add(0x9E37_79B9_7F4A_7C15);
        mixed = (mixed ^ (mixed >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        mixed = (mixed ^ (mixed >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        mixed ^= mixed >> 31;
        Self {
            state: if mixed == 0 { 1 } else { mixed },
        }
    }

    pub fn next_u64(&mut self) -> u64 {
        self.state ^= self.state >> 12;
        self.state ^= self.state << 25;
        self.state ^= self.state >> 27;
        self.state.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }

    /// A number from 0 up to, not including, `bound`; 0 when `bound` is 0.
    pub fn below(&mut self, bound: u32) -> u32 {
        (((self.next_u64() >> 32) * u64::from(bound)) >> 32) as u32
    }

    /// A number from 1 to `sides`, as a die with that many sides shows.
    pub fn roll(&mut self, sides: u32) -> u32 {
        self.below(sides) + 1
    }

    /// Mixes `entropy`, such as the time of a button press, into the state.
    pub fn stir(&mut self, entropy: u64) {
        let stirred = Self::new(self.state ^ entropy);
        self.state = stirred.state;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rolls_stay_on_the_die_and_reach_every_side() {
        let mut rng = XorShift::new(0);
        let mut seen = [0u32; 6];
        for _ in 0..6000 {
            let roll = rng.roll(6);
            assert!((1..=6).contains(&roll));
            seen[roll as usize - 1] += 1;
        }
        assert!(seen.iter().all(|&count| count > 800), "{seen:?}");
        assert_eq!(rng.below(0), 0);
    }

    #[test]
    fn the_same_seed_gives_the_same_numbers() {
        let (mut a, mut b) = (XorShift::new(42), XorShift::new(42));
        assert_eq!(a.next_u64(), b.next_u64());
        assert_ne!(XorShift::new(1).next_u64(), XorShift::new(2).next_u64());
    }
}
