//! Binary-coded decimal: ABCD, SBCD, NBCD (PRM Table 3-8).
//!
//! Transcribed from `m68000_python/_bcd.py`. PRM Table 3-18 gives X and C
//! (the decimal carry or borrow) and a sticky Z, and calls N and V
//! undefined. They are not undefined on silicon, and the rule below is the
//! one flamewing's 68k-bcd-verifier checks exhaustively on a Model 1 and a
//! Model 3 Sega Genesis (T1): the byte is first added or subtracted in
//! binary, then corrected by 6 in each nibble that produced a binary carry
//! (borrow) or, for an add, a digit above 9; N is bit 7 of the corrected
//! byte, and V is set when the correction flipped bit 7 -- from 0 to 1 for
//! an add, from 1 to 0 for a subtract. C and X are set by a carry (borrow)
//! out of either the binary step or the correction.
//!
//! The three instructions' bus sequences are the microcode's (SST ABCD,
//! SBCD, NBCD; T3): the register forms prefetch and then spend two internal
//! clocks; the memory forms read, prefetch, then write.

use crate::core::{Bus, Step, C, M68000, N, V, X, Z};
use crate::ea::{ea_kind, DN};

/// ABCD's result byte and X N Z V C bits (Z to be ANDed in: sticky).
pub fn decimal_add(destination: u32, source: u32, extend: u32) -> (u32, u16) {
    let binary = destination + source + extend;
    let low_carry = (destination & 0xF) + (source & 0xF) + extend > 0xF;
    let uncorrected = binary & 0xFF;
    let mut correction = 0;
    if low_carry || (uncorrected & 0xF) > 9 {
        correction = 0x06;
    }
    if binary > 0xFF || uncorrected > 0x99 {
        correction |= 0x60;
    }
    let corrected = uncorrected + correction;
    let result = corrected & 0xFF;
    let mut flags = if result == 0 { Z } else { 0 };
    if binary > 0xFF || corrected > 0xFF {
        flags |= X | C;
    }
    if result & 0x80 != 0 {
        flags |= N;
    }
    if result & 0x80 != 0 && uncorrected & 0x80 == 0 {
        flags |= V; // the correction carried into bit 7 (T1, flamewing)
    }
    (result, flags)
}

/// SBCD's (and NBCD's, with destination 0) result byte and X N Z V C bits.
pub fn decimal_subtract(destination: u32, source: u32, extend: u32) -> (u32, u16) {
    let (destination, source, extend) = (destination as i32, source as i32, extend as i32);
    let binary = destination - source - extend;
    let low_borrow = (destination & 0xF) - (source & 0xF) - extend < 0;
    let uncorrected = binary & 0xFF;
    let mut correction = 0;
    if low_borrow {
        correction = 0x06;
    }
    if binary < 0 {
        correction |= 0x60;
    }
    let corrected = uncorrected - correction;
    let result = corrected & 0xFF;
    let mut flags = if result == 0 { Z } else { 0 };
    if binary < 0 || corrected < 0 {
        flags |= X | C;
    }
    if result & 0x80 != 0 {
        flags |= N;
    }
    if uncorrected & 0x80 != 0 && result & 0x80 == 0 {
        flags |= V; // the correction borrowed out of bit 7 (T1, flamewing)
    }
    (result as u32, flags)
}

/// `operation(destination, source, extend)`: the byte and its flag bits.
type DecimalOperation = fn(u32, u32, u32) -> (u32, u16);

impl<B: Bus> M68000<B> {
    fn set_bcd_flags(&mut self, flags: u16) {
        // X N V C replaced; Z only ever cleared (PRM Table 3-18, sticky Z).
        let sr = self.sr;
        self.sr = (sr & 0xFF00) | (flags & (X | N | V | C)) | (sr & flags & Z);
    }

    /// The two forms shared by ABCD and SBCD: Dy,Dx and -(Ay),-(Ax).
    fn bcd_pair(&mut self, opcode: u16, operation: DecimalOperation) -> Step {
        let rx = usize::from((opcode >> 9) & 7);
        let ry = usize::from(opcode & 7);
        let extend = u32::from((self.sr >> 4) & 1);
        if opcode & 8 == 0 {
            let (result, flags) = operation(self.r[rx] & 0xFF, self.r[ry] & 0xFF, extend);
            self.set_bcd_flags(flags);
            self.prefetch()?;
            self.cycles += 2;
            self.r[rx] = (self.r[rx] & 0xFFFF_FF00) | result;
            return Ok(());
        }
        self.cycles += 2;
        self.commit_pc();
        let source_address = self.r[8 + ry].wrapping_sub(self.step_size(ry, 1));
        self.r[8 + ry] = source_address;
        let source = self.read_byte(source_address)?;
        let destination_address = self.r[8 + rx].wrapping_sub(self.step_size(rx, 1));
        self.r[8 + rx] = destination_address;
        let destination = self.read_byte(destination_address)?;
        let (result, flags) = operation(destination, source, extend);
        self.set_bcd_flags(flags);
        self.prefetch()?;
        self.write_byte(destination_address, result)
    }

    /// ABCD -- destination <- destination + source + X, decimal (PRM 4-3; UM Table 8-11).
    ///
    /// N and V, undefined in PRM: flamewing's hardware-captured tables (T1).
    /// Bus order, the 2 idle clocks of the register form and the PC an
    /// address error stacks: SST ABCD.
    pub(crate) fn op_abcd(&mut self, opcode: u16) -> Step {
        self.bcd_pair(opcode, decimal_add)
    }

    /// SBCD -- destination <- destination - source - X, decimal (PRM 4-176; UM Table 8-11).
    pub(crate) fn op_sbcd(&mut self, opcode: u16) -> Step {
        self.bcd_pair(opcode, decimal_subtract)
    }

    /// NBCD -- destination <- 0 - destination - X, decimal (PRM 4-142; UM Table 8-6).
    pub(crate) fn op_nbcd(&mut self, opcode: u16) -> Step {
        let kind = ea_kind(opcode);
        let register = usize::from(opcode & 7);
        let extend = u32::from((self.sr >> 4) & 1);
        if kind == DN {
            let (result, flags) = decimal_subtract(0, self.r[register] & 0xFF, extend);
            self.set_bcd_flags(flags);
            self.prefetch()?;
            self.cycles += 2;
            self.r[register] = (self.r[register] & 0xFFFF_FF00) | result;
            return Ok(());
        }
        let address = self.ea_address(kind, register, 1)?;
        let value = self.read_byte(address)?;
        let (result, flags) = decimal_subtract(0, value, extend);
        self.set_bcd_flags(flags);
        self.prefetch()?;
        self.write_byte(address, result)
    }
}
