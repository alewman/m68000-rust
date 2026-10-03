//! Condition codes: the flag rules of PRM Table 3-18 and the tests of Table 3-19.
//!
//! Transcribed from `m68000_python/_flags.py`. Every rule is written once
//! here and used by every family that shares it: `flags_logic` for the
//! moves, logic and tests (N, Z from the result, V and C cleared, X
//! untouched), `flags_add`/`flags_sub` for binary arithmetic, `flags_cmp`
//! for the compares (X untouched), and the `x` variants for ADDX/SUBX/NEGX,
//! whose Z is sticky: cleared by a nonzero result, otherwise unchanged (PRM
//! Table 3-18).
//!
//! The reference passes unmasked Python integers: an add's result may
//! exceed the size's mask and a subtract's may be negative. Here results
//! are `i64`, which holds both.

use crate::core::{Bus, C, M68000, MASK, MSB, N, V, X, Z};

pub const CONDITION_NAMES: [&str; 16] = [
    "T", "F", "HI", "LS", "CC", "CS", "NE", "EQ", "VC", "VS", "PL", "MI", "GE", "LT", "GT", "LE",
];

/// PRM Table 3-19, evaluated for one condition and one N/Z/V/C value.
const fn test(cc: usize, ccr: u16) -> bool {
    let n = ccr & N != 0;
    let z = ccr & Z != 0;
    let v = ccr & V != 0;
    let c = ccr & C != 0;
    match cc {
        0 => true,          // T
        1 => false,         // F
        2 => !c && !z,      // HI
        3 => c || z,        // LS
        4 => !c,            // CC (HS)
        5 => c,             // CS (LO)
        6 => !z,            // NE
        7 => z,             // EQ
        8 => !v,            // VC
        9 => v,             // VS
        10 => !n,           // PL
        11 => n,            // MI
        12 => n == v,       // GE
        13 => n != v,       // LT
        14 => !z && n == v, // GT
        _ => z || n != v,   // LE
    }
}

const fn conditions() -> [[bool; 16]; 16] {
    let mut table = [[false; 16]; 16];
    let mut cc = 0;
    while cc < 16 {
        let mut ccr = 0;
        while ccr < 16 {
            table[cc][ccr] = test(cc, ccr as u16);
            ccr += 1;
        }
        cc += 1;
    }
    table
}

/// `CONDITION[cc][sr & 0xF]` is the truth of condition `cc`.
pub const CONDITION: [[bool; 16]; 16] = conditions();

/// The truth of the condition in bits 11-8 of `opcode` under `sr`.
#[inline]
pub fn condition(opcode: u16, sr: u16) -> bool {
    CONDITION[usize::from((opcode >> 8) & 0xF)][usize::from(sr & 0xF)]
}

impl<B: Bus> M68000<B> {
    /// N and Z from `result`; V and C cleared; X unchanged.
    pub(crate) fn flags_logic(&mut self, result: u32, size: usize) {
        let mut ccr = self.sr & (0xFF00 | X);
        if result & MSB[size] != 0 {
            ccr |= N;
        }
        if result & MASK[size] == 0 {
            ccr |= Z;
        }
        self.sr = ccr;
    }

    // A long result leaves the ALU as two words, and the microcode updates
    // the flags once per word: N Z V C from the low word, then N from the
    // high word with Z kept only if the high word is zero too. Only an
    // address error between the two can see the halfway state; the stacked
    // SR does (SST MOVE.l, T3). flags_logic is the two applied in order.

    /// N Z from the low word; V C cleared (the first half of a long's flags).
    pub(crate) fn flags_low_word(&mut self, value: u32) {
        let mut ccr = self.sr & (0xFF00 | X);
        if value & 0x8000 != 0 {
            ccr |= N;
        }
        if value & 0xFFFF == 0 {
            ccr |= Z;
        }
        self.sr = ccr;
    }

    /// N from bit 31; Z kept only if the high word is also zero (the second half).
    pub(crate) fn flags_high_word(&mut self, value: u32) {
        let mut ccr = self.sr & !N;
        if value & 0x8000_0000 != 0 {
            ccr |= N;
        }
        if value & 0xFFFF_0000 != 0 {
            ccr &= !Z;
        }
        self.sr = ccr;
    }

    /// X N Z V C of `destination + source` (`result` unmasked).
    pub(crate) fn flags_add(&mut self, destination: i64, source: i64, result: i64, size: usize) {
        let msb = i64::from(MSB[size]);
        let mask = i64::from(MASK[size]);
        let mut ccr = self.sr & 0xFF00;
        if result & msb != 0 {
            ccr |= N;
        }
        if result & mask == 0 {
            ccr |= Z;
        }
        if (source ^ result) & (destination ^ result) & msb != 0 {
            ccr |= V;
        }
        if result > mask {
            ccr |= X | C;
        }
        self.sr = ccr;
    }

    /// X N Z V C of `destination - source` (`result` unmasked, may be negative).
    pub(crate) fn flags_sub(&mut self, destination: i64, source: i64, result: i64, size: usize) {
        let msb = i64::from(MSB[size]);
        let mask = i64::from(MASK[size]);
        let mut ccr = self.sr & 0xFF00;
        if result & msb != 0 {
            ccr |= N;
        }
        if result & mask == 0 {
            ccr |= Z;
        }
        if (source ^ destination) & (result ^ destination) & msb != 0 {
            ccr |= V;
        }
        if result < 0 {
            ccr |= X | C;
        }
        self.sr = ccr;
    }

    /// N Z V C of `destination - source`; X unchanged (PRM Table 3-18, CMP).
    pub(crate) fn flags_cmp(&mut self, destination: i64, source: i64, result: i64, size: usize) {
        let msb = i64::from(MSB[size]);
        let mask = i64::from(MASK[size]);
        let mut ccr = self.sr & (0xFF00 | X);
        if result & msb != 0 {
            ccr |= N;
        }
        if result & mask == 0 {
            ccr |= Z;
        }
        if (source ^ destination) & (result ^ destination) & msb != 0 {
            ccr |= V;
        }
        if result < 0 {
            ccr |= C;
        }
        self.sr = ccr;
    }

    /// ADDX: as ADD, but Z is only ever cleared (PRM Table 3-18).
    pub(crate) fn flags_addx(&mut self, destination: i64, source: i64, result: i64, size: usize) {
        let msb = i64::from(MSB[size]);
        let mask = i64::from(MASK[size]);
        let mut ccr = self.sr & (0xFF00 | Z);
        if result & msb != 0 {
            ccr |= N;
        }
        if result & mask != 0 {
            ccr &= !Z;
        }
        if (source ^ result) & (destination ^ result) & msb != 0 {
            ccr |= V;
        }
        if result > mask {
            ccr |= X | C;
        }
        self.sr = ccr;
    }

    /// SUBX and NEGX: as SUB, but Z is only ever cleared (PRM Table 3-18).
    pub(crate) fn flags_subx(&mut self, destination: i64, source: i64, result: i64, size: usize) {
        let msb = i64::from(MSB[size]);
        let mask = i64::from(MASK[size]);
        let mut ccr = self.sr & (0xFF00 | Z);
        if result & msb != 0 {
            ccr |= N;
        }
        if result & mask != 0 {
            ccr &= !Z;
        }
        if (source ^ destination) & (result ^ destination) & msb != 0 {
            ccr |= V;
        }
        if result < 0 {
            ccr |= X | C;
        }
        self.sr = ccr;
    }
}
