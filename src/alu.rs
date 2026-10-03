//! Integer arithmetic and logic (PRM Tables 3-3 and 3-4).
//!
//! Transcribed from `m68000_python/_alu.py`. ADD, SUB, AND, OR, EOR, CMP
//! and their A/I/Q/X/M forms; NEG, NEGX, NOT, CLR, TST, TAS; MULU, MULS,
//! DIVU, DIVS. Flag rules are `flags.rs`'s. The bus sequences are three
//! shapes, all the microcode's (corpus, T3):
//!
//! * **to a data register**: the operand is read (EA timing, UM Table 8-1),
//!   the result written, the queue refilled; a long result then spends 2
//!   internal clocks after a memory operand and 4 after a register or
//!   immediate one (UM Table 8-4 note **);
//! * **to memory** (read-modify-write): the operand is read, the queue
//!   refilled, the result written -- a long one low word first;
//! * **compares** read and refill only.
//!
//! Only the operand read can fault in a read-modify-write: the write goes to
//! the address the read already proved even.
//!
//! The reference passes the operation as a bound method; here it is a
//! method pointer of the same shape, [`BinaryOperation`] or
//! [`UnaryOperation`].

use crate::core::{Bus, BusError, GroupZero, Step, M68000, MASK, N, V, VECTOR_BUS_ERROR, X, Z};
use crate::ea::{ea_kind, sign_extend_16, word_to_long, AN, DN, IMM};

/// Bits 7-6 of the common size field (PRM Section 8): 00 byte, 01 word, 10 long.
pub const SIZE: [usize; 4] = [1, 2, 4, 0];
/// ADDQ/SUBQ data: 1-8, with 0 meaning 8 (PRM 4, ADDQ).
pub const QUICK: [u32; 8] = [8, 1, 2, 3, 4, 5, 6, 7];

/// `operation(destination, source, size)`: the result, flags set.
pub type BinaryOperation<B> = fn(&mut M68000<B>, u32, u32, usize) -> u32;
/// `operation(value, size)`: the result, flags set.
pub type UnaryOperation<B> = fn(&mut M68000<B>, u32, usize) -> u32;

#[inline]
pub(crate) fn size_of(opcode: u16) -> usize {
    SIZE[usize::from((opcode >> 6) & 3)]
}

/// MULU: 38 + 2n, n the number of 1 bits in the source word (UM Table 8-4, note).
pub fn multiply_unsigned_clocks(source: u32) -> u32 {
    38 + 2 * (source & 0xFFFF).count_ones()
}

/// MULS: 38 + 2n, n the 01/10 transitions of the source with a 0 appended (UM Table 8-4).
pub fn multiply_signed_clocks(source: u32) -> u32 {
    let pattern = (source & 0xFFFF) << 1;
    38 + 2 * ((pattern ^ (pattern >> 1)) & 0xFFFF).count_ones()
}

/// DIVU's internal clocks, from the microcode's shift-and-subtract loop.
///
/// The manual gives only a maximum (UM Table 8-4). The loop: overflow is
/// detected first; otherwise 15 steps each shift the dividend left, and a
/// step whose shifted-out bit was 0 costs 2 more clocks, less 1 if the
/// trial subtraction succeeded. Jorge Cwik's analysis (2005, carried in
/// WinUAE, T2) states the same count; SST DIVU checks it on every case (T3).
pub fn divide_unsigned_clocks(dividend: u32, divisor: u32) -> u32 {
    if dividend >> 16 >= divisor {
        return 10;
    }
    let mut dividend = u64::from(dividend);
    let mut half_clocks = 38;
    let shifted_divisor = u64::from(divisor) << 16;
    for _ in 0..15 {
        let carry = dividend & 0x8000_0000;
        dividend = (dividend << 1) & 0xFFFF_FFFF;
        if carry != 0 {
            dividend = dividend.wrapping_sub(shifted_divisor) & 0xFFFF_FFFF;
        } else {
            half_clocks += 2;
            if dividend >= shifted_divisor {
                dividend -= shifted_divisor;
                half_clocks -= 1;
            }
        }
    }
    half_clocks * 2
}

/// DIVS's internal clocks: sign handling, then one step per quotient bit.
///
/// `dividend` and `divisor` are signed. Absolute overflow (|dividend| >> 16
/// >= |divisor|) is found early; otherwise the unsigned loop runs on the
/// magnitudes and each of the 15 high quotient bits that is 0 costs 2
/// clocks (Cwik 2005, T2; checked against the corpus, T3).
pub fn divide_signed_clocks(dividend: i64, divisor: i64) -> u32 {
    let mut half_clocks: i64 = 6;
    if dividend < 0 {
        half_clocks += 1;
    }
    if dividend.abs() >> 16 >= divisor.abs() {
        return ((half_clocks + 2) * 2) as u32;
    }
    let quotient = dividend.abs() / divisor.abs();
    half_clocks += 55;
    if divisor >= 0 {
        half_clocks += if dividend >= 0 { -1 } else { 1 };
    }
    for bit in 0..15 {
        if quotient & (0x8000 >> bit) == 0 {
            half_clocks += 1;
        }
    }
    (half_clocks * 2) as u32
}

impl<B: Bus> M68000<B> {
    // -- the operations: result from destination and source, flags set --------

    pub(crate) fn add(&mut self, destination: u32, source: u32, size: usize) -> u32 {
        let result = i64::from(destination) + i64::from(source);
        self.flags_add(i64::from(destination), i64::from(source), result, size);
        (result as u32) & MASK[size]
    }

    pub(crate) fn sub(&mut self, destination: u32, source: u32, size: usize) -> u32 {
        let result = i64::from(destination) - i64::from(source);
        self.flags_sub(i64::from(destination), i64::from(source), result, size);
        (result as u32) & MASK[size]
    }

    pub(crate) fn and(&mut self, destination: u32, source: u32, size: usize) -> u32 {
        let result = destination & source;
        self.flags_logic(result, size);
        result
    }

    pub(crate) fn or(&mut self, destination: u32, source: u32, size: usize) -> u32 {
        let result = destination | source;
        self.flags_logic(result, size);
        result
    }

    pub(crate) fn eor(&mut self, destination: u32, source: u32, size: usize) -> u32 {
        let result = destination ^ source;
        self.flags_logic(result, size);
        result
    }

    pub(crate) fn addx(&mut self, destination: u32, source: u32, size: usize) -> u32 {
        let result = i64::from(destination) + i64::from(source) + i64::from((self.sr >> 4) & 1);
        self.flags_addx(i64::from(destination), i64::from(source), result, size);
        (result as u32) & MASK[size]
    }

    pub(crate) fn subx(&mut self, destination: u32, source: u32, size: usize) -> u32 {
        let result = i64::from(destination) - i64::from(source) - i64::from((self.sr >> 4) & 1);
        self.flags_subx(i64::from(destination), i64::from(source), result, size);
        (result as u32) & MASK[size]
    }

    pub(crate) fn cmp(&mut self, destination: u32, source: u32, size: usize) {
        let (destination, source) = (i64::from(destination), i64::from(source));
        self.flags_cmp(destination, source, destination - source, size);
    }

    // -- the shapes ------------------------------------------------------------

    /// Refill the queue and write Dn; a long spends 2 or 4 more clocks (Table 8-4).
    pub(crate) fn to_data_register(
        &mut self,
        register: usize,
        size: usize,
        result: u32,
        kind: u8,
    ) -> Step {
        self.prefetch()?;
        if size == 4 {
            self.cycles += if matches!(kind, DN | AN | IMM) { 4 } else { 2 };
            self.r[register] = result;
        } else {
            self.write_register(register, size, result);
        }
        Ok(())
    }

    /// Read-modify-write `operation(destination, source)` on an EA operand.
    pub(crate) fn modify(
        &mut self,
        kind: u8,
        register: usize,
        size: usize,
        operation: BinaryOperation<B>,
        source: u32,
    ) -> Step {
        if kind == DN {
            let result = operation(self, self.r[register] & MASK[size], source, size);
            self.prefetch()?;
            if size == 4 {
                self.cycles += 4;
            }
            self.write_register(register, size, result);
            return Ok(());
        }
        let (address, value) = self.ea_fetch(kind, register, size)?;
        let result = operation(self, value, source, size);
        self.prefetch()?;
        self.write_result(address, size, result)
    }

    pub(crate) fn write_result(&mut self, address: u32, size: usize, result: u32) -> Step {
        if size == 4 {
            self.write_long_low_first(address, result)
        } else if size == 2 {
            self.write_word(address, result)
        } else {
            self.write_byte(address, result)
        }
    }

    /// ADD, SUB, AND, OR: `<ea>,Dn` when bit 8 is 0, `Dn,<ea>` when it is 1.
    fn binary(&mut self, opcode: u16, operation: BinaryOperation<B>) -> Step {
        let size = size_of(opcode);
        let kind = ea_kind(opcode);
        let register = usize::from(opcode & 7);
        let dn = usize::from((opcode >> 9) & 7);
        if opcode & 0x0100 != 0 {
            let source = self.r[dn] & MASK[size];
            return self.modify(kind, register, size, operation, source);
        }
        let source = self.ea_read(kind, register, size)?;
        let result = operation(self, self.r[dn] & MASK[size], source, size);
        self.to_data_register(dn, size, result, kind)
    }

    /// ADD -- destination <- destination + source; X N Z V C (PRM 4-4; UM Table 8-4).
    ///
    /// SST ADD.b/.w/.l pin the bus order, the idle clocks and the PC an
    /// address error stacks (their cases also cover ADDI and ADDQ).
    pub(crate) fn op_add(&mut self, opcode: u16) -> Step {
        self.binary(opcode, Self::add)
    }

    /// SUB -- destination <- destination - source; X N Z V C (PRM 4-174; UM Table 8-4).
    pub(crate) fn op_sub(&mut self, opcode: u16) -> Step {
        self.binary(opcode, Self::sub)
    }

    /// AND -- destination <- destination AND source; N Z, V C cleared (PRM 4-15; UM Table 8-4).
    pub(crate) fn op_and(&mut self, opcode: u16) -> Step {
        self.binary(opcode, Self::and)
    }

    /// OR -- destination <- destination OR source; N Z, V C cleared (PRM 4-150; UM Table 8-4).
    pub(crate) fn op_or(&mut self, opcode: u16) -> Step {
        self.binary(opcode, Self::or)
    }

    /// EOR -- <ea> <- <ea> XOR Dn; N Z, V C cleared (PRM 4-100; UM Table 8-4).
    pub(crate) fn op_eor(&mut self, opcode: u16) -> Step {
        let size = size_of(opcode);
        let source = self.r[usize::from((opcode >> 9) & 7)] & MASK[size];
        self.modify(
            ea_kind(opcode),
            usize::from(opcode & 7),
            size,
            Self::eor,
            source,
        )
    }

    /// CMP -- set N Z V C from Dn - source; X unchanged (PRM 4-75; UM Table 8-4).
    ///
    /// SST CMP.b/.w/.l pin the bus order, the 2 idle clocks of a long and the
    /// PC an address error stacks (their cases also cover CMPI and CMPM).
    pub(crate) fn op_cmp(&mut self, opcode: u16) -> Step {
        let size = size_of(opcode);
        let source = self.ea_read(ea_kind(opcode), usize::from(opcode & 7), size)?;
        self.cmp(
            self.r[usize::from((opcode >> 9) & 7)] & MASK[size],
            source,
            size,
        );
        self.prefetch()?;
        if size == 4 {
            self.cycles += 2;
        }
        Ok(())
    }

    // -- address-register forms (PRM 4-7, 4-77, 4-178) ------------------------

    /// The source of ADDA/SUBA/CMPA, sign-extended to 32 bits, and its kind.
    fn address_source(&mut self, opcode: u16) -> Result<(u32, u8), GroupZero> {
        let size = if opcode & 0x0100 != 0 { 4 } else { 2 };
        let kind = ea_kind(opcode);
        let mut source = self.ea_read(kind, usize::from(opcode & 7), size)?;
        if size == 2 {
            source = word_to_long(source);
        }
        Ok((source, kind))
    }

    fn address_idle(&mut self, opcode: u16, kind: u8) {
        // Word forms and register/immediate sources: 4 clocks; long from memory: 2.
        if opcode & 0x0100 == 0 || matches!(kind, DN | AN | IMM) {
            self.cycles += 4;
        } else {
            self.cycles += 2;
        }
    }

    /// ADDA -- An <- An + source, a word sign-extended; no flags (PRM 4-7; UM Table 8-4).
    pub(crate) fn op_adda(&mut self, opcode: u16) -> Step {
        let (source, kind) = self.address_source(opcode)?;
        let an = 8 + usize::from((opcode >> 9) & 7);
        self.prefetch()?;
        self.address_idle(opcode, kind);
        self.r[an] = self.r[an].wrapping_add(source);
        Ok(())
    }

    /// SUBA -- An <- An - source, a word sign-extended; no flags (PRM 4-178; UM Table 8-4).
    pub(crate) fn op_suba(&mut self, opcode: u16) -> Step {
        let (source, kind) = self.address_source(opcode)?;
        let an = 8 + usize::from((opcode >> 9) & 7);
        self.prefetch()?;
        self.address_idle(opcode, kind);
        self.r[an] = self.r[an].wrapping_sub(source);
        Ok(())
    }

    /// CMPA -- set N Z V C from An - source, a 32-bit compare (PRM 4-77; UM Table 8-4).
    pub(crate) fn op_cmpa(&mut self, opcode: u16) -> Step {
        let (source, _) = self.address_source(opcode)?;
        self.cmp(self.r[8 + usize::from((opcode >> 9) & 7)], source, 4);
        self.prefetch()?;
        self.cycles += 2;
        Ok(())
    }

    // -- immediate forms (PRM 4-9, 4-18, 4-79, 4-102, 4-153, 4-180) ----------

    /// The immediate operand that follows the opcode word (byte in the low half).
    pub(crate) fn immediate(&mut self, size: usize) -> Result<u32, GroupZero> {
        if size == 4 {
            let high = self.extension()?;
            self.commit_pc();
            return Ok((high << 16) | self.extension()?);
        }
        self.commit_pc();
        Ok(self.extension()? & MASK[size])
    }

    fn immediate_to_ea(&mut self, opcode: u16, operation: BinaryOperation<B>) -> Step {
        let size = size_of(opcode);
        let source = self.immediate(size)?;
        self.modify(
            ea_kind(opcode),
            usize::from(opcode & 7),
            size,
            operation,
            source,
        )
    }

    /// ADDI -- destination <- destination + #data (PRM 4-9; UM Table 8-5).
    ///
    /// The immediate's refills come before the operand and the PC is
    /// committed at the last of them: SST ADD.b/.w/.l (the ADDI cases).
    pub(crate) fn op_addi(&mut self, opcode: u16) -> Step {
        self.immediate_to_ea(opcode, Self::add)
    }

    /// SUBI -- destination <- destination - #data (PRM 4-180; UM Table 8-5).
    pub(crate) fn op_subi(&mut self, opcode: u16) -> Step {
        self.immediate_to_ea(opcode, Self::sub)
    }

    /// ANDI -- destination <- destination AND #data (PRM 4-18; UM Table 8-5).
    pub(crate) fn op_andi(&mut self, opcode: u16) -> Step {
        self.immediate_to_ea(opcode, Self::and)
    }

    /// ORI -- destination <- destination OR #data (PRM 4-153; UM Table 8-5).
    pub(crate) fn op_ori(&mut self, opcode: u16) -> Step {
        self.immediate_to_ea(opcode, Self::or)
    }

    /// EORI -- destination <- destination XOR #data (PRM 4-102; UM Table 8-5).
    pub(crate) fn op_eori(&mut self, opcode: u16) -> Step {
        self.immediate_to_ea(opcode, Self::eor)
    }

    /// CMPI -- set N Z V C from destination - #data (PRM 4-79; UM Table 8-5).
    ///
    /// Refills, commit point, and the 2 idle clocks of a long compare with
    /// Dn: SST CMP.b/.w/.l (the CMPI cases).
    pub(crate) fn op_cmpi(&mut self, opcode: u16) -> Step {
        let size = size_of(opcode);
        let source = self.immediate(size)?;
        let kind = ea_kind(opcode);
        let destination = self.ea_read(kind, usize::from(opcode & 7), size)?;
        self.cmp(destination, source, size);
        self.prefetch()?;
        if size == 4 && kind == DN {
            self.cycles += 2;
        }
        Ok(())
    }

    // -- quick forms (PRM 4-11, 4-182) -------------------------------------------

    fn quick(&mut self, opcode: u16, operation: BinaryOperation<B>, subtract: bool) -> Step {
        let size = size_of(opcode);
        let data = QUICK[usize::from((opcode >> 9) & 7)];
        let kind = ea_kind(opcode);
        let register = usize::from(opcode & 7);
        if kind == AN {
            // An: the whole register, no flags, any size (PRM 4-11).
            self.prefetch()?;
            self.cycles += 4;
            let an = 8 + register;
            self.r[an] = if subtract {
                self.r[an].wrapping_sub(data)
            } else {
                self.r[an].wrapping_add(data)
            };
            return Ok(());
        }
        self.modify(kind, register, size, operation, data)
    }

    /// ADDQ -- destination <- destination + 1..8 (PRM 4-11; UM Table 8-5).
    ///
    /// ADDQ.W #,An takes 8 clocks, not the 4 UM Table 8-5 prints: SST
    /// ADD.b/.w/.l (the ADDQ cases, 118 of them to An) and WinUAE run agree.
    pub(crate) fn op_addq(&mut self, opcode: u16) -> Step {
        self.quick(opcode, Self::add, false)
    }

    /// SUBQ -- destination <- destination - 1..8 (PRM 4-182; UM Table 8-5).
    pub(crate) fn op_subq(&mut self, opcode: u16) -> Step {
        self.quick(opcode, Self::sub, true)
    }

    // -- multiprecision (PRM 4-14, 4-184, 4-81) ------------------------------------

    /// ADDX and SUBX: Dy,Dx or -(Ay),-(Ax) (UM Table 8-11).
    fn extended(&mut self, opcode: u16, operation: BinaryOperation<B>) -> Step {
        let size = size_of(opcode);
        let rx = usize::from((opcode >> 9) & 7);
        let ry = usize::from(opcode & 7);
        if opcode & 8 == 0 {
            let result = operation(self, self.r[rx] & MASK[size], self.r[ry] & MASK[size], size);
            self.prefetch()?;
            if size == 4 {
                self.cycles += 4;
                self.r[rx] = result;
            } else {
                self.write_register(rx, size, result);
            }
            return Ok(());
        }
        self.cycles += 2;
        self.commit_pc();
        if size == 4 {
            // Low words first, each register stepping once its high word is next.
            let source_address = self.r[8 + ry].wrapping_sub(4);
            let low = self.read_word(source_address.wrapping_add(2))?;
            self.r[8 + ry] = source_address;
            let source = (self.read_word(source_address)? << 16) | low;
            let destination_address = self.r[8 + rx].wrapping_sub(4);
            let low = self.read_word(destination_address.wrapping_add(2))?;
            self.r[8 + rx] = destination_address;
            let destination = (self.read_word(destination_address)? << 16) | low;
            let result = operation(self, destination, source, 4);
            self.write_word(destination_address.wrapping_add(2), result)?;
            self.prefetch()?;
            return self.write_word(destination_address, result >> 16);
        }
        let source_address = self.r[8 + ry].wrapping_sub(self.step_size(ry, size));
        self.r[8 + ry] = source_address;
        let source = self.read(size, source_address)?;
        let destination_address = self.r[8 + rx].wrapping_sub(self.step_size(rx, size));
        self.r[8 + rx] = destination_address;
        let destination = self.read(size, destination_address)?;
        let result = operation(self, destination, source, size);
        self.prefetch()?;
        self.write(size, destination_address, result)
    }

    /// ADDX -- destination <- destination + source + X; Z sticky (PRM 4-14; UM Table 8-11).
    ///
    /// The -(Ay),-(Ax) form's read order (low words first, each register
    /// stepping once its high word is next), the write between the closing
    /// prefetch and its high word, and the PC an address error stacks: SST
    /// ADDX.b/.w/.l.
    pub(crate) fn op_addx(&mut self, opcode: u16) -> Step {
        self.extended(opcode, Self::addx)
    }

    /// SUBX -- destination <- destination - source - X; Z sticky (PRM 4-184; UM Table 8-11).
    pub(crate) fn op_subx(&mut self, opcode: u16) -> Step {
        self.extended(opcode, Self::subx)
    }

    /// CMPM -- set N Z V C from (Ax)+ - (Ay)+ (PRM 4-81; UM Table 8-11).
    ///
    /// Ay steps before each word it reads; Ax only after its operand is in,
    /// so an address error leaves Ay moved by 2 and Ax alone: SST
    /// CMP.b/.w/.l (the CMPM cases). For CMPM.L, WinUAE run leaves the
    /// faulting Ay 2 less: the rule is contested and the core follows the
    /// gate (the reference's docs/claims.md).
    pub(crate) fn op_cmpm(&mut self, opcode: u16) -> Step {
        let size = size_of(opcode);
        let ay = 8 + usize::from(opcode & 7);
        let ax = 8 + usize::from((opcode >> 9) & 7);
        self.commit_pc();
        let address = self.r[ay];
        let source = if size == 4 {
            self.r[ay] = address.wrapping_add(2);
            let high = self.read_word(address)?;
            self.r[ay] = address.wrapping_add(4);
            (high << 16) | self.read_word(address.wrapping_add(2))?
        } else {
            self.r[ay] = address.wrapping_add(self.step_size(usize::from(opcode & 7), size));
            self.read(size, address)?
        };
        let address = self.r[ax];
        let destination = self.read(size, address)?;
        let step = if size != 4 {
            self.step_size(ax - 8, size)
        } else {
            4
        };
        self.r[ax] = address.wrapping_add(step);
        self.cmp(destination, source, size);
        self.prefetch()
    }

    // -- single operand (PRM 4-144, 4-146, 4-148, 4-73, 4-192, 4-190) ------------

    fn unary(&mut self, opcode: u16, operation: UnaryOperation<B>) -> Step {
        let size = size_of(opcode);
        let kind = ea_kind(opcode);
        let register = usize::from(opcode & 7);
        if kind == DN {
            let result = operation(self, self.r[register] & MASK[size], size);
            self.prefetch()?;
            if size == 4 {
                self.cycles += 2;
            }
            self.write_register(register, size, result);
            return Ok(());
        }
        let (address, value) = self.ea_fetch(kind, register, size)?;
        let result = operation(self, value, size);
        self.prefetch()?;
        self.write_result(address, size, result)
    }

    fn negate(&mut self, value: u32, size: usize) -> u32 {
        self.sub(0, value, size)
    }

    fn negate_extended(&mut self, value: u32, size: usize) -> u32 {
        self.subx(0, value, size)
    }

    fn complement(&mut self, value: u32, size: usize) -> u32 {
        let result = !value & MASK[size];
        self.flags_logic(result, size);
        result
    }

    fn clear(&mut self, _value: u32, _size: usize) -> u32 {
        self.sr = (self.sr & (0xFF00 | X)) | Z;
        0
    }

    /// NEG -- destination <- 0 - destination; X N Z V C (PRM 4-144; UM Table 8-6).
    pub(crate) fn op_neg(&mut self, opcode: u16) -> Step {
        self.unary(opcode, Self::negate)
    }

    /// NEGX -- destination <- 0 - destination - X; Z sticky (PRM 4-146; UM Table 8-6).
    pub(crate) fn op_negx(&mut self, opcode: u16) -> Step {
        self.unary(opcode, Self::negate_extended)
    }

    /// NOT -- destination <- ones' complement; N Z, V C cleared (PRM 4-148; UM Table 8-6).
    pub(crate) fn op_not(&mut self, opcode: u16) -> Step {
        self.unary(opcode, Self::complement)
    }

    /// CLR -- destination <- 0, the operand read first on the 68000 (PRM 4-73; UM Table 8-6).
    pub(crate) fn op_clr(&mut self, opcode: u16) -> Step {
        self.unary(opcode, Self::clear)
    }

    /// TST -- set N Z from the operand; V C cleared (PRM 4-192; UM Table 8-6).
    pub(crate) fn op_tst(&mut self, opcode: u16) -> Step {
        let size = size_of(opcode);
        let value = self.ea_read(ea_kind(opcode), usize::from(opcode & 7), size)?;
        self.flags_logic(value, size);
        self.prefetch()
    }

    /// TAS -- test a byte, then set its bit 7, indivisibly (PRM 4-190; UM Table 8-6).
    ///
    /// The write half goes through the host's `tas_write`: the Genesis bus
    /// drops it. Read, 2 idle clocks, write, then the prefetch (14 clocks
    /// for (An)): SST TAS, whose totals WinUAE run confirms on every case it
    /// judges.
    pub(crate) fn op_tas(&mut self, opcode: u16) -> Step {
        let kind = ea_kind(opcode);
        let register = usize::from(opcode & 7);
        if kind == DN {
            let value = self.r[register] & 0xFF;
            self.flags_logic(value, 1);
            self.prefetch()?;
            self.r[register] |= 0x80;
            return Ok(());
        }
        let (address, value) = self.ea_fetch(kind, register, 1)?;
        self.cycles += 2;
        self.flags_logic(value, 1);
        self.cycles += 4;
        let cycle = crate::core::Cycle {
            fc: self.fc(false),
            step_clocks: self.cycles,
        };
        if let Err(BusError) = self
            .bus
            .tas_write(address & 0xFF_FFFF, (value | 0x80) as u8, cycle)
        {
            // BERR on the write half is a bus error like any other (UM 6.3.9.1).
            return Err(self.fault(VECTOR_BUS_ERROR, address, true, false));
        }
        self.prefetch()
    }

    // -- multiply and divide (PRM 4-139, 4-141, 4-93, 4-96; UM Table 8-4) ------

    /// MULU -- Dn <- Dn.w * source.w, unsigned (PRM 4-141; UM Table 8-4).
    ///
    /// The count of 1 bits sets the clocks (38 + 2n, UM Table 8-4's note),
    /// spent after the closing prefetch: SST MULU.
    pub(crate) fn op_mulu(&mut self, opcode: u16) -> Step {
        let source = self.ea_read(ea_kind(opcode), usize::from(opcode & 7), 2)?;
        let dn = usize::from((opcode >> 9) & 7);
        let product = (self.r[dn] & 0xFFFF) * source;
        self.flags_logic(product, 4);
        self.prefetch()?;
        self.cycles += multiply_unsigned_clocks(source) - 4;
        self.r[dn] = product;
        Ok(())
    }

    /// MULS -- Dn <- Dn.w * source.w, signed (PRM 4-139; UM Table 8-4).
    pub(crate) fn op_muls(&mut self, opcode: u16) -> Step {
        let source = self.ea_read(ea_kind(opcode), usize::from(opcode & 7), 2)?;
        let dn = usize::from((opcode >> 9) & 7);
        let product = (sign_extend_16(self.r[dn]) * sign_extend_16(source)) as u32;
        self.flags_logic(product, 4);
        self.prefetch()?;
        self.cycles += multiply_signed_clocks(source) - 4;
        self.r[dn] = product;
        Ok(())
    }

    /// DIVU -- Dn <- Dn / source.w, unsigned; remainder:quotient (PRM 4-96; UM Table 8-4).
    ///
    /// The data-dependent clocks (`divide_unsigned_clocks`) and the overflow
    /// flags N=1 V=1 Z=0 C=0, undefined in PRM: SST DIVU on every case, and
    /// WinUAE run. Division by zero is `divide_by_zero`, whose flags no
    /// corpus has: WinUAE run pins them.
    pub(crate) fn op_divu(&mut self, opcode: u16) -> Step {
        let source = self.ea_read(ea_kind(opcode), usize::from(opcode & 7), 2)?;
        let dn = usize::from((opcode >> 9) & 7);
        let dividend = self.r[dn];
        if source == 0 {
            return self.divide_by_zero(false, dividend);
        }
        let (quotient, remainder) = (dividend / source, dividend % source);
        self.cycles += divide_unsigned_clocks(dividend, source) - 4;
        if quotient > 0xFFFF {
            // Overflow: V set, Dn unchanged; N Z C as the microcode leaves them.
            self.sr = (self.sr & (0xFF00 | X)) | V | N;
        } else {
            self.r[dn] = (remainder << 16) | quotient;
            self.flags_logic(quotient, 2);
        }
        self.prefetch()
    }

    /// DIVS -- Dn <- Dn / source.w, signed; remainder:quotient (PRM 4-93; UM Table 8-4).
    ///
    /// The data-dependent clocks (`divide_signed_clocks`, Cwik's analysis)
    /// and the overflow flags N=1 V=1 Z=0 C=0, undefined in PRM: SST DIVS on
    /// every case, and WinUAE run.
    pub(crate) fn op_divs(&mut self, opcode: u16) -> Step {
        let source = self.ea_read(ea_kind(opcode), usize::from(opcode & 7), 2)?;
        let dn = usize::from((opcode >> 9) & 7);
        let dividend = i64::from(self.r[dn] as i32);
        let divisor = i64::from(sign_extend_16(source));
        if divisor == 0 {
            return self.divide_by_zero(true, self.r[dn]);
        }
        let mut quotient = dividend.abs() / divisor.abs();
        if (dividend < 0) != (divisor < 0) {
            quotient = -quotient;
        }
        let remainder = dividend - quotient * divisor; // takes the dividend's sign
        self.cycles += divide_signed_clocks(dividend, divisor) - 4;
        if !(-0x8000..=0x7FFF).contains(&quotient) {
            self.sr = (self.sr & (0xFF00 | X)) | V | N;
        } else {
            self.r[dn] = (((remainder as u32) & 0xFFFF) << 16) | ((quotient as u32) & 0xFFFF);
            self.flags_logic((quotient as u32) & 0xFFFF, 2);
        }
        self.prefetch()
    }
}
