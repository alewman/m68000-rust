//! Data movement: MOVE, MOVEA, MOVEQ, MOVEM, MOVEP, LEA, PEA, LINK, UNLK, EXG, SWAP, EXT.
//!
//! Transcribed from `m68000_python/_loads.py`. PRM Table 3-2. MOVE's
//! destination has its own sequence, not the source's: no extension word is
//! taken from the queue before the source operand is read, `-(An)` costs no
//! internal clocks and is written after the closing prefetch, and
//! `(xxx).L` writes between its two extension-word refills (UM Tables 8-2
//! and 8-3; the order is the corpus's: SST MOVE.b/.w/.l, T3).

use crate::core::{Bus, GroupZero, Step, M68000};
use crate::ea::{
    ea_kind, sign_extend_16, sign_extend_8, word_to_long, ABSL, ABSW, AN, DISP, DN, EA_KIND, IMM,
    IND, INDEX, PCDISP, PCINDEX, POSTINC, PREDEC,
};

/// MOVE's size field, bits 13-12 (PRM 4, MOVE): 01 byte, 11 word, 10 long.
fn move_size(opcode: u16) -> usize {
    match opcode >> 12 {
        1 => 1,
        3 => 2,
        _ => 4,
    }
}

impl<B: Bus> M68000<B> {
    /// MOVE -- destination <- source; N Z set, V C cleared (PRM 4-116; UM Tables 8-2, 8-3).
    ///
    /// The destination sequence (the module docs), which flags a faulting
    /// long write has set (`move_long_to`), when (An)+ and -(An) move, and
    /// the PC an address error stacks: SST MOVE.b/.w/.l, 60% of whose cases
    /// fault. WinUAE run agrees except that it leaves An alone when a word
    /// (An)+ faults: contested, the core follows the gate.
    pub(crate) fn op_move(&mut self, opcode: u16) -> Step {
        let size = move_size(opcode);
        let source = ea_kind(opcode);
        let value = self.ea_read(source, usize::from(opcode & 7), size)?;
        let kind = EA_KIND[usize::from(((opcode >> 3) & 0x38) | ((opcode >> 9) & 7))];
        let register = usize::from((opcode >> 9) & 7);
        let from_register = matches!(source, DN | AN | IMM);
        if size == 4 {
            return self.move_long_to(kind, register, value, from_register);
        }
        if kind == DN {
            self.flags_logic(value, size);
            self.write_register(register, size, value);
            return self.prefetch();
        }
        if kind == IND || kind == POSTINC {
            let address = self.r[8 + register];
            self.flags_logic(value, size);
            self.commit_pc();
            self.write(size, address, value)?;
            if kind == POSTINC {
                // after the write: a faulting write leaves An alone
                self.r[8 + register] = address.wrapping_add(self.step_size(register, size));
            }
            return self.prefetch();
        }
        if kind == PREDEC {
            let address = self.r[8 + register].wrapping_sub(self.step_size(register, size));
            self.r[8 + register] = address;
            self.flags_logic(value, size);
            self.prefetch()?;
            return self.write(size, address, value);
        }
        let address = if kind == DISP {
            self.commit_pc();
            let displacement = sign_extend_16(self.extension()?) as u32;
            self.r[8 + register].wrapping_add(displacement)
        } else if kind == INDEX {
            self.cycles += 2;
            self.commit_pc();
            self.index(self.r[8 + register])?
        } else if kind == ABSW {
            self.commit_pc();
            word_to_long(self.extension()?)
        } else if from_register {
            // (xxx).L from a register or immediate: both refills, then the write.
            let high = self.extension()?;
            self.commit_pc();
            (high << 16) | self.extension()?
        } else {
            // (xxx).L from memory: the write comes between the two refills.
            self.commit_pc();
            let high = self.extension()?;
            let address = (high << 16) | u32::from(self.irc);
            self.flags_logic(value, size);
            self.write(size, address, value)?;
            self.extension()?;
            return self.prefetch();
        };
        self.flags_logic(value, size);
        self.write(size, address, value)?;
        self.prefetch()
    }

    /// The destination half of MOVE.L.
    ///
    /// The long is written as two words and its flags are set as two halves,
    /// low word then high word (`flags.rs`), interleaved with the writes in
    /// an order that depends on where the value came from: the stacked SR of
    /// an address error on the first write shows it (SST MOVE.l).
    fn move_long_to(&mut self, kind: u8, register: usize, value: u32, from_register: bool) -> Step {
        let (high, low) = (value >> 16, value & 0xFFFF);
        if kind == DN {
            self.flags_logic(value, 4);
            self.r[register] = value;
            return self.prefetch();
        }
        if kind == IND || kind == POSTINC {
            let address = self.r[8 + register];
            self.commit_pc();
            if from_register {
                self.write_word(address, high)?; // flags not yet touched
                self.flags_low_word(value);
            } else {
                self.flags_low_word(value);
                self.write_word(address, high)?;
            }
            self.write_word(address.wrapping_add(2), low)?;
            if kind == POSTINC {
                self.r[8 + register] = address.wrapping_add(4);
            }
            self.flags_high_word(value);
            return self.prefetch();
        }
        if kind == PREDEC {
            let address = self.r[8 + register].wrapping_sub(4);
            self.flags_logic(value, 4);
            self.prefetch_before_write()?;
            self.write_word(address.wrapping_add(2), low)?; // low word first, An not yet moved
            self.opcode = self.ir;
            self.r[8 + register] = address;
            return self.write_word(address, high);
        }
        if kind == DISP || kind == INDEX {
            if kind == INDEX {
                self.cycles += 2;
            }
            self.commit_pc();
            let address = if kind == DISP {
                let displacement = sign_extend_16(self.extension()?) as u32;
                self.r[8 + register].wrapping_add(displacement)
            } else {
                self.index(self.r[8 + register])?
            };
            if from_register {
                self.flags_high_word(value); // the high word's half first
                self.write_word(address, high)?;
                self.flags_low_word(value);
            } else {
                self.flags_low_word(value);
                self.flags_high_word(value);
                self.write_word(address, high)?;
            }
            self.write_word(address.wrapping_add(2), low)?;
            self.flags_high_word(value);
            return self.prefetch();
        }
        let address = if kind == ABSW {
            self.commit_pc();
            word_to_long(self.extension()?)
        } else if from_register {
            let high_address = self.extension()?;
            self.commit_pc();
            (high_address << 16) | self.extension()?
        } else {
            // (xxx).L from memory: the writes come between the two refills.
            self.commit_pc();
            let high_address = self.extension()?;
            let address = (high_address << 16) | u32::from(self.irc);
            self.flags_low_word(value);
            self.write_word(address, high)?;
            self.flags_high_word(value);
            self.write_word(address.wrapping_add(2), low)?;
            self.extension()?;
            return self.prefetch();
        };
        self.flags_logic(value, 4);
        self.write_word(address, high)?;
        self.write_word(address.wrapping_add(2), low)?;
        self.prefetch()
    }

    /// MOVEA -- An <- source, a word sign-extended; no flags (PRM 4-119; UM Tables 8-2, 8-3).
    pub(crate) fn op_movea(&mut self, opcode: u16) -> Step {
        let size = if opcode >> 12 == 3 { 2 } else { 4 };
        let mut value = self.ea_read(ea_kind(opcode), usize::from(opcode & 7), size)?;
        if size == 2 {
            value = word_to_long(value);
        }
        self.r[8 + usize::from((opcode >> 9) & 7)] = value;
        self.prefetch()
    }

    /// MOVEQ -- Dn <- sign-extended byte; N Z, V C cleared (PRM 4-134; UM Table 8-5).
    pub(crate) fn op_moveq(&mut self, opcode: u16) -> Step {
        let value = sign_extend_8(u32::from(opcode)) as u32;
        self.flags_logic(value, 4);
        self.r[usize::from((opcode >> 9) & 7)] = value;
        self.prefetch()
    }

    // -- MOVEM (PRM 4-128; UM Table 8-10) ---------------------------------------

    /// MOVEM -- move a list of registers to or from memory (PRM 4-128; UM Table 8-10).
    ///
    /// The mask word follows the opcode; bit 0 is D0 and bit 15 A7, except
    /// for -(An), where the order is reversed so that the registers still
    /// land in memory D0 lowest (PRM 4, MOVEM). Memory-to-register reads one
    /// word past the last register (the extra read of UM Table 8-10's
    /// `3+n`); word loads are sign-extended into the whole register. With
    /// -(An) the 68000 stores An's initial value if An is in the list; with
    /// (An)+ the loaded value is replaced by the final address (PRM 4,
    /// MOVEM).
    ///
    /// The extra read, the -(An) form's low-word-first longs, and the PC an
    /// address error stacks: SST MOVEM.w/.l. For (d8,An,Xn) and (d8,PC,Xn)
    /// at an odd address WinUAE run stacks 4 less: contested, the core
    /// follows the gate.
    pub(crate) fn op_movem(&mut self, opcode: u16) -> Step {
        let size: u32 = if opcode & 0x40 != 0 { 4 } else { 2 };
        let kind = ea_kind(opcode);
        let register = usize::from(opcode & 7);
        let mask = self.extension()?;
        if opcode & 0x0400 != 0 {
            let program = matches!(kind, PCDISP | PCINDEX);
            let mut address = if kind == POSTINC {
                self.r[8 + register]
            } else {
                self.ea_address(kind, register, 2)?
            };
            self.commit_pc();
            for index in 0..16 {
                if mask & (1 << index) != 0 {
                    if size == 4 {
                        let high = self.movem_read(program, address)?;
                        self.r[index] =
                            (high << 16) | self.movem_read(program, address.wrapping_add(2))?;
                    } else {
                        self.r[index] = word_to_long(self.movem_read(program, address)?);
                    }
                    address = address.wrapping_add(size);
                }
            }
            self.movem_read(program, address)?; // one word past the last register
            if kind == POSTINC {
                self.r[8 + register] = address;
            }
            return self.prefetch();
        }
        if kind == PREDEC {
            let mut address = self.r[8 + register];
            self.commit_pc();
            for bit in 0..16 {
                if mask & (1 << bit) != 0 {
                    let value = self.r[15 - bit];
                    address = address.wrapping_sub(size);
                    if size == 4 {
                        self.write_long_low_first(address, value)?;
                    } else {
                        self.write_word(address, value)?;
                    }
                }
            }
            self.r[8 + register] = address;
            return self.prefetch();
        }
        let mut address = self.ea_address(kind, register, 2)?;
        self.commit_pc();
        for index in 0..16 {
            if mask & (1 << index) != 0 {
                if size == 4 {
                    self.write_long(address, self.r[index])?;
                } else {
                    self.write_word(address, self.r[index])?;
                }
                address = address.wrapping_add(size);
            }
        }
        self.prefetch()
    }

    /// MOVEM's reads: program space for the PC-relative forms.
    fn movem_read(&mut self, program: bool, address: u32) -> Result<u32, GroupZero> {
        if program {
            self.read_program_word(address)
        } else {
            self.read_word(address)
        }
    }

    // -- MOVEP (PRM 4-133; UM Table 8-13) --------------------------------------

    /// MOVEP -- move Dn to or from alternate bytes at (d16,Ay) (PRM 4-133; UM Table 8-13).
    ///
    /// Byte accesses, high-order byte first, so never an address error at an
    /// odd address; the order is SST MOVEP.w/.l's.
    pub(crate) fn op_movep(&mut self, opcode: u16) -> Step {
        let dn = usize::from((opcode >> 9) & 7);
        let base = self.r[8 + usize::from(opcode & 7)];
        let mut address = base.wrapping_add(sign_extend_16(self.extension()?) as u32);
        let count: u32 = if opcode & 0x40 != 0 { 4 } else { 2 };
        if opcode & 0x80 != 0 {
            let value = self.r[dn];
            for index in (0..count).rev() {
                self.write_byte(address, value >> (8 * index))?;
                address = address.wrapping_add(2);
            }
        } else {
            let mut value = 0u32;
            for _ in 0..count {
                value = (value << 8) | self.read_byte(address)?;
                address = address.wrapping_add(2);
            }
            self.write_register(dn, count as usize, value);
        }
        self.prefetch()
    }

    // -- effective addresses as values (PRM 4-110, 4-159) ----------------------

    /// LEA -- An <- effective address (PRM 4-110; UM Table 8-10).
    pub(crate) fn op_lea(&mut self, opcode: u16) -> Step {
        let kind = ea_kind(opcode);
        let address = self.control_ea(kind, usize::from(opcode & 7))?;
        self.r[8 + usize::from((opcode >> 9) & 7)] = address;
        if matches!(kind, INDEX | PCINDEX) {
            self.cycles += 2;
        }
        self.prefetch()
    }

    /// A control-mode address, taking the extension words from the queue.
    fn control_ea(&mut self, kind: u8, register: usize) -> Result<u32, GroupZero> {
        if kind == IND {
            return Ok(self.r[8 + register]);
        }
        self.ea_address(kind, register, 4)
    }

    /// PEA -- push the effective address (PRM 4-159; UM Table 8-10).
    ///
    /// Absolute forms push before the closing prefetch, the others after it
    /// with the decoder still holding PEA; the PC an address error stacks:
    /// SST PEA.
    pub(crate) fn op_pea(&mut self, opcode: u16) -> Step {
        let kind = ea_kind(opcode);
        let address = self.control_ea(kind, usize::from(opcode & 7))?;
        if matches!(kind, INDEX | PCINDEX) {
            self.cycles += 2;
        }
        if matches!(kind, ABSW | ABSL) {
            self.push_long(address)?;
            return self.prefetch();
        }
        self.prefetch_before_write()?;
        self.push_long(address)?;
        self.opcode = self.ir;
        Ok(())
    }

    // -- stack frames (PRM 4-111, 4-194) -----------------------------------------

    /// LINK -- push An, An <- SP, SP <- SP + d16 (PRM 4-111; UM Table 8-12: 16 clocks).
    ///
    /// LINK A7 pushes A7's value from before the push: SST LINK, and WinUAE
    /// run (its 4.3.0 fix against hardware).
    pub(crate) fn op_link(&mut self, opcode: u16) -> Step {
        let an = 8 + usize::from(opcode & 7);
        let displacement = sign_extend_16(self.extension()?) as u32;
        let value = self.r[an]; // LINK A7 pushes A7 as it was before the push
        let sp = self.r[15].wrapping_sub(4);
        self.r[an] = sp;
        self.write_long(sp, value)?;
        self.r[15] = sp.wrapping_add(displacement);
        self.prefetch()
    }

    /// UNLK -- SP <- An, An <- (SP)+ (PRM 4-194; UM Table 8-12: 12 clocks).
    ///
    /// SP moves only once both words are in, so a fault leaves it: SST UNLINK.
    pub(crate) fn op_unlk(&mut self, opcode: u16) -> Step {
        let an = 8 + usize::from(opcode & 7);
        let address = self.r[an];
        self.commit_pc();
        let value = self.read_long(address)?; // SP moves only once both words are in
        self.r[15] = address.wrapping_add(4);
        self.r[an] = value;
        self.prefetch()
    }

    // -- register shuffles (PRM 4-105, 4-187, 4-106) ------------------------------

    /// EXG -- exchange two registers (PRM 4-105; UM Table 8-12: 6 clocks).
    pub(crate) fn op_exg(&mut self, opcode: u16) -> Step {
        let mode = (opcode >> 3) & 0x1F;
        let rx = usize::from((opcode >> 9) & 7);
        let ry = usize::from(opcode & 7);
        let (x, y) = match mode {
            0x08 => (rx, ry),
            0x09 => (8 + rx, 8 + ry),
            _ => (rx, 8 + ry),
        };
        self.r.swap(x, y);
        self.prefetch()?;
        self.cycles += 2;
        Ok(())
    }

    /// SWAP -- swap the halves of Dn; N Z of the long, V C clear (PRM 4-187; UM Table 8-12).
    pub(crate) fn op_swap(&mut self, opcode: u16) -> Step {
        let register = usize::from(opcode & 7);
        let value = self.r[register].rotate_left(16);
        self.flags_logic(value, 4);
        self.r[register] = value;
        self.prefetch()
    }

    /// EXT -- sign-extend Dn's byte (EXT.W) or word (EXT.L) (PRM 4-106; UM Table 8-12).
    pub(crate) fn op_ext(&mut self, opcode: u16) -> Step {
        let register = usize::from(opcode & 7);
        if opcode & 0x40 != 0 {
            let value = word_to_long(self.r[register]);
            self.flags_logic(value, 4);
            self.r[register] = value;
        } else {
            let value = (sign_extend_8(self.r[register]) as u32) & 0xFFFF;
            self.flags_logic(value, 2);
            self.write_register(register, 2, value);
        }
        self.prefetch()
    }
}
