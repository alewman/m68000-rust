//! Shifts and rotates: ASL/ASR, LSL/LSR, ROXL/ROXR, ROL/ROR (PRM Table 3-5).
//!
//! Transcribed from `m68000_python/_shifts.py`. Register forms shift Dn by
//! 1-8 (an immediate count, 0 meaning 8) or by Dx mod 64, at any size, in
//! 6 + 2n clocks for a byte or word and 8 + 2n for a long (UM Table 8-7):
//! the queue is refilled first, then the shifter runs. Memory forms shift a
//! word by one: read, refill, write.
//!
//! Flags (PRM Table 3-18): N and Z from the result; C is the last bit
//! shifted out; X takes C for the shifts and ROXL/ROXR and is untouched by
//! ROL/ROR. V is cleared except by ASL, which sets it if the sign bit
//! changed at any point of the shift. A count of 0 shifts nothing and
//! clears C, except ROXL/ROXR, which copy X into C.
//!
//! The reference names the four kinds with strings; here they are
//! [`ShiftKind`] values. Its unbounded shifts are `u64` here: every shifted
//! value is masked to at most 33 bits, so the bits a `u64` drops never
//! matter.

use crate::alu::{size_of, QUICK};
use crate::core::{Bus, Step, C, M68000, MASK, MSB, N, V, X, Z};
use crate::ea::ea_kind;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ShiftKind {
    As,
    Ls,
    Rox,
    Ro,
}

fn nz(result: u32, size: usize) -> u16 {
    let mut flags = 0;
    if result & MSB[size] != 0 {
        flags |= N;
    }
    if result & MASK[size] == 0 {
        flags |= Z;
    }
    flags
}

/// Shift `value` (`size` bytes) `count` places; return (result, X N Z V C bits).
///
/// The returned flags include X only where the operation defines it; the
/// caller keeps the old X for ROL/ROR and for a count of 0.
pub fn shift(
    kind: ShiftKind,
    left: bool,
    value: u32,
    count: u32,
    size: usize,
    x: u32,
) -> (u32, u16) {
    let bits = 8 * size as u32;
    let mask = MASK[size];
    let msb = MSB[size];
    if count == 0 {
        let carry = if kind == ShiftKind::Rox && x != 0 {
            C
        } else {
            0
        };
        let extend = if kind == ShiftKind::Rox && x != 0 {
            X
        } else {
            0
        };
        return (value, nz(value, size) | carry | extend);
    }
    match kind {
        ShiftKind::As if left => {
            let mut result = value;
            let mut overflow = false;
            let mut carry = 0;
            for _ in 0..count {
                carry = u32::from(result & msb != 0);
                let shifted = (result << 1) & mask;
                if (shifted ^ result) & msb != 0 {
                    overflow = true; // the sign bit changed at some point (PRM 4-22)
                }
                result = shifted;
            }
            let flags = nz(result, size) | if overflow { V } else { 0 };
            (result, flags | if carry != 0 { X | C } else { 0 })
        }
        ShiftKind::As => {
            let signed: i64 = if value & msb != 0 {
                i64::from(value) - (1i64 << bits)
            } else {
                i64::from(value)
            };
            let (result, carry) = if count >= bits {
                (if signed < 0 { mask } else { 0 }, u32::from(signed < 0))
            } else {
                (
                    ((signed >> count) as u32) & mask,
                    ((signed >> (count - 1)) & 1) as u32,
                )
            };
            (
                result,
                nz(result, size) | if carry != 0 { X | C } else { 0 },
            )
        }
        ShiftKind::Ls => {
            let wide = u64::from(value);
            let (result, carry) = if left {
                let carry = if count <= bits {
                    ((wide >> (bits - count)) & 1) as u32
                } else {
                    0
                };
                ((wide.wrapping_shl(count) as u32) & mask, carry)
            } else {
                let carry = if count <= bits {
                    ((wide >> (count - 1)) & 1) as u32
                } else {
                    0
                };
                (
                    if count < bits {
                        (wide >> count) as u32
                    } else {
                        0
                    },
                    carry,
                )
            };
            (
                result,
                nz(result, size) | if carry != 0 { X | C } else { 0 },
            )
        }
        ShiftKind::Ro => {
            let steps = count % bits;
            let wide = u64::from(value);
            let (result, carry) = if left {
                let result = (((wide << steps) | (wide >> (bits - steps))) as u32) & mask;
                (result, result & 1)
            } else {
                let result = (((wide >> steps) | (wide << (bits - steps))) as u32) & mask;
                (result, u32::from(result & msb != 0))
            };
            (result, nz(result, size) | if carry != 0 { C } else { 0 })
        }
        ShiftKind::Rox => {
            // rotate through X, a (bits + 1)-bit rotation
            let steps = count % (bits + 1);
            let wide_mask = (u64::from(mask) << 1) | 1;
            let mut wide = (u64::from(x) << bits) | u64::from(value);
            if left {
                wide = (wide.wrapping_shl(steps) | (wide >> (bits + 1 - steps))) & wide_mask;
            } else {
                wide = ((wide >> steps) | wide.wrapping_shl(bits + 1 - steps)) & wide_mask;
            }
            let extend = (wide >> bits) & 1;
            let result = (wide as u32) & mask;
            (
                result,
                nz(result, size) | if extend != 0 { X | C } else { 0 },
            )
        }
    }
}

impl<B: Bus> M68000<B> {
    fn shift_register(&mut self, opcode: u16, kind: ShiftKind) -> Step {
        let size = size_of(opcode);
        let count = if opcode & 0x20 != 0 {
            self.r[usize::from((opcode >> 9) & 7)] & 63
        } else {
            QUICK[usize::from((opcode >> 9) & 7)]
        };
        let register = usize::from(opcode & 7);
        let x = u32::from((self.sr >> 4) & 1);
        let (result, flags) = shift(
            kind,
            opcode & 0x0100 != 0,
            self.r[register] & MASK[size],
            count,
            size,
            x,
        );
        self.set_shift_flags(kind, flags, count);
        self.prefetch()?;
        self.cycles += (if size == 4 { 4 } else { 2 }) + 2 * count;
        self.write_register(register, size, result);
        Ok(())
    }

    fn set_shift_flags(&mut self, kind: ShiftKind, flags: u16, count: u32) {
        let mut keep = 0xFF00;
        if kind == ShiftKind::Ro || (count == 0 && kind != ShiftKind::Rox) {
            keep |= X; // ROL/ROR never touch X; a zero count leaves X alone
        }
        self.sr = (self.sr & keep) | (flags & !keep & 0xFF);
    }

    fn shift_memory(&mut self, opcode: u16, kind: ShiftKind) -> Step {
        let (address, value) = self.ea_fetch(ea_kind(opcode), usize::from(opcode & 7), 2)?;
        let x = u32::from((self.sr >> 4) & 1);
        let (result, flags) = shift(kind, opcode & 0x0100 != 0, value, 1, 2, x);
        self.set_shift_flags(kind, flags, 1);
        self.prefetch()?;
        self.write_word(address, result)
    }

    /// ASd -- arithmetic shift Dn left or right (PRM 4-22; UM Table 8-7).
    pub(crate) fn op_asd(&mut self, opcode: u16) -> Step {
        self.shift_register(opcode, ShiftKind::As)
    }

    /// LSd -- logical shift Dn left or right (PRM 4-113; UM Table 8-7).
    pub(crate) fn op_lsd(&mut self, opcode: u16) -> Step {
        self.shift_register(opcode, ShiftKind::Ls)
    }

    /// ROXd -- rotate Dn through X, left or right (PRM 4-163; UM Table 8-7).
    pub(crate) fn op_roxd(&mut self, opcode: u16) -> Step {
        self.shift_register(opcode, ShiftKind::Rox)
    }

    /// ROd -- rotate Dn left or right, X untouched (PRM 4-160; UM Table 8-7).
    pub(crate) fn op_rod(&mut self, opcode: u16) -> Step {
        self.shift_register(opcode, ShiftKind::Ro)
    }

    /// ASd -- arithmetic shift a memory word by one (PRM 4-22; UM Table 8-7).
    pub(crate) fn op_asd_memory(&mut self, opcode: u16) -> Step {
        self.shift_memory(opcode, ShiftKind::As)
    }

    /// LSd -- logical shift a memory word by one (PRM 4-113; UM Table 8-7).
    pub(crate) fn op_lsd_memory(&mut self, opcode: u16) -> Step {
        self.shift_memory(opcode, ShiftKind::Ls)
    }

    /// ROXd -- rotate a memory word through X by one (PRM 4-163; UM Table 8-7).
    pub(crate) fn op_roxd_memory(&mut self, opcode: u16) -> Step {
        self.shift_memory(opcode, ShiftKind::Rox)
    }

    /// ROd -- rotate a memory word by one (PRM 4-160; UM Table 8-7).
    pub(crate) fn op_rod_memory(&mut self, opcode: u16) -> Step {
        self.shift_memory(opcode, ShiftKind::Ro)
    }
}
