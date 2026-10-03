//! Bit manipulation: BTST, BCHG, BCLR, BSET (PRM Table 3-6; UM Table 8-8).
//!
//! Transcribed from `m68000_python/_bits.py`. The bit number comes from Dn
//! (dynamic) or from the word after the opcode (static). On a data register
//! the operation is on the whole long and the number is taken mod 32; on
//! memory it is on a byte, mod 8 (PRM 4, BTST). Z is set if the tested bit
//! was 0; nothing else changes. On a register the changing forms cost 2
//! clocks more for bits 16-31 (UM Table 8-8 note), and BCLR 2 more again
//! (SST BCLR, T3).

use crate::core::{Bus, Step, M68000, Z};
use crate::ea::{ea_kind, DN, IMM};

// The operations, by bits 7-6 of the opcode: test, change, clear, set.
const TEST: u8 = 0;
const CHANGE: u8 = 1;
const CLEAR: u8 = 2;
const SET: u8 = 3;

impl<B: Bus> M68000<B> {
    fn bit_operation(&mut self, opcode: u16, operation: u8) -> Step {
        let mut number = if opcode & 0x0100 != 0 {
            self.r[usize::from((opcode >> 9) & 7)]
        } else {
            self.commit_pc();
            self.extension()? & 0xFF
        };
        let kind = ea_kind(opcode);
        let register = usize::from(opcode & 7);
        if kind == DN {
            number &= 31;
            let value = self.r[register];
            self.set_z(value, number);
            self.prefetch()?;
            if operation == TEST {
                self.cycles += 2;
                return Ok(());
            }
            self.cycles += if operation == CLEAR { 4 } else { 2 };
            if number >= 16 {
                self.cycles += 2;
            }
            self.r[register] = apply_bit(value, number, operation);
            return Ok(());
        }
        number &= 7;
        if kind == IMM {
            // BTST Dn,#data: the operand is the byte after the opcode
            let value = self.extension()? & 0xFF;
            self.set_z(value, number);
            self.prefetch()?;
            self.cycles += 2;
            return Ok(());
        }
        let (address, value) = self.ea_fetch(kind, register, 1)?;
        self.set_z(value, number);
        self.prefetch()?;
        if operation != TEST {
            self.write_byte(address, apply_bit(value, number, operation))?;
        }
        Ok(())
    }

    fn set_z(&mut self, value: u32, number: u32) {
        if (value >> number) & 1 != 0 {
            self.sr &= !Z;
        } else {
            self.sr |= Z;
        }
    }

    /// BTST -- Z <- NOT bit n of the destination (PRM 4-62; UM Table 8-8).
    pub(crate) fn op_btst(&mut self, opcode: u16) -> Step {
        self.bit_operation(opcode, TEST)
    }

    /// BCHG -- Z <- NOT bit n, then invert bit n (PRM 4-28; UM Table 8-8).
    pub(crate) fn op_bchg(&mut self, opcode: u16) -> Step {
        self.bit_operation(opcode, CHANGE)
    }

    /// BCLR -- Z <- NOT bit n, then clear bit n (PRM 4-31; UM Table 8-8).
    pub(crate) fn op_bclr(&mut self, opcode: u16) -> Step {
        self.bit_operation(opcode, CLEAR)
    }

    /// BSET -- Z <- NOT bit n, then set bit n (PRM 4-57; UM Table 8-8).
    pub(crate) fn op_bset(&mut self, opcode: u16) -> Step {
        self.bit_operation(opcode, SET)
    }
}

fn apply_bit(value: u32, number: u32, operation: u8) -> u32 {
    let bit = 1 << number;
    match operation {
        CHANGE => value ^ bit,
        CLEAR => value & !bit,
        _ => value | bit,
    }
}
