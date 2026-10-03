//! Effective addresses: the twelve modes of the 68000 (PRM 2.2).
//!
//! Transcribed from `m68000_python/_ea.py`. The 6-bit EA field is
//! `mode:register`; mode 7 uses the register field to select five more
//! forms. [`EA_KIND`] maps the field to one of the twelve kinds below, and
//! handlers carry the kind, not the field.
//!
//! Timing is Table 8-1 of the UM, spent in the order the microcode spends
//! it: `-(An)` and `(d8,An,Xn)` start with two internal clocks, extension
//! words are taken from the prefetch queue (each one refills it with a
//! program read), and the operand access comes last. The steps that bring
//! the microcode's PC register up to the fetch address (what an address
//! error stacks) are the corpus's, T3.
//!
//! The reference's addressing categories are frozensets of kinds; here they
//! are bit sets over the kind numbers.

use crate::core::{Bus, GroupZero, M68000, MASK};

// The twelve kinds, in PRM 2.2 order.
pub const DN: u8 = 0; // Dn                data register direct
pub const AN: u8 = 1; // An                address register direct
pub const IND: u8 = 2; // (An)              address register indirect
pub const POSTINC: u8 = 3; // (An)+        indirect with postincrement
pub const PREDEC: u8 = 4; // -(An)         indirect with predecrement
pub const DISP: u8 = 5; // (d16,An)        indirect with displacement
pub const INDEX: u8 = 6; // (d8,An,Xn)     indirect with index
pub const ABSW: u8 = 7; // (xxx).W        absolute short
pub const ABSL: u8 = 8; // (xxx).L        absolute long
pub const PCDISP: u8 = 9; // (d16,PC)      program counter with displacement
pub const PCINDEX: u8 = 10; // (d8,PC,Xn)  program counter with index
pub const IMM: u8 = 11; // #<data>         immediate
/// Mode 7 registers 5-7: no addressing mode (the reference's `None`).
pub const NONE: u8 = 0xFF;

const fn kind(field: usize) -> u8 {
    let mode = field >> 3;
    let register = field & 7;
    if mode < 7 {
        return mode as u8;
    }
    [ABSW, ABSL, PCDISP, PCINDEX, IMM, NONE, NONE, NONE][register]
}

const fn kinds() -> [u8; 64] {
    let mut table = [0u8; 64];
    let mut field = 0;
    while field < 64 {
        table[field] = kind(field);
        field += 1;
    }
    table
}

/// EA field (bits 5-0) -> kind, or [`NONE`] for mode 7 registers 5-7 (illegal).
pub const EA_KIND: [u8; 64] = kinds();

/// The kind of the EA field in the low six bits of `opcode`.
#[inline]
pub fn ea_kind(opcode: u16) -> u8 {
    EA_KIND[usize::from(opcode & 0x3F)]
}

/// A set of kinds: bit n set means kind n is in the set.
pub type Category = u16;

const fn set(kinds: &[u8]) -> Category {
    let mut bits = 0;
    let mut index = 0;
    while index < kinds.len() {
        bits |= 1 << kinds[index];
        index += 1;
    }
    bits
}

// The addressing categories of PRM Table 2-4, as sets of kinds.
pub const ALL: Category = 0x0FFF;
pub const DATA: Category = ALL & !set(&[AN]);
pub const MEMORY: Category = ALL & !set(&[DN, AN]);
pub const CONTROL: Category = set(&[IND, DISP, INDEX, ABSW, ABSL, PCDISP, PCINDEX]);
pub const ALTERABLE: Category = set(&[DN, AN, IND, POSTINC, PREDEC, DISP, INDEX, ABSW, ABSL]);
pub const DATA_ALTERABLE: Category = DATA & ALTERABLE;
pub const MEMORY_ALTERABLE: Category = MEMORY & ALTERABLE;

/// Whether `kind` (possibly [`NONE`]) is in `category`.
#[inline]
pub fn within(kind: u8, category: Category) -> bool {
    kind != NONE && category & (1 << kind) != 0
}

pub fn sign_extend_8(value: u32) -> i32 {
    i32::from(value as u8 as i8)
}

pub fn sign_extend_16(value: u32) -> i32 {
    i32::from(value as u16 as i16)
}

/// A word sign-extended to 32 bits, as An, MOVEM.W and EXT.L take it (PRM 2.3).
pub fn word_to_long(value: u32) -> u32 {
    sign_extend_16(value) as u32
}

impl<B: Bus> M68000<B> {
    pub(crate) fn step_size(&self, register: usize, size: usize) -> u32 {
        // (A7)+ and -(A7) move by 2 for a byte, keeping SP even (PRM 2.2.4-2.2.5).
        if size == 1 && register == 7 {
            2
        } else {
            size as u32
        }
    }

    /// Take the brief extension word from the queue and apply it to `base`.
    pub(crate) fn index(&mut self, base: u32) -> Result<u32, GroupZero> {
        let extension = self.extension()?;
        Ok(self.indexed(base, extension))
    }

    /// Apply a brief extension word (PRM 2.4) to `base`.
    ///
    /// Bits 15-12 name Xn (D0-D7, A0-A7), bit 11 chooses the sign-extended
    /// low word (0) or the whole register (1), bits 7-0 are the signed
    /// displacement. Bits 10-8, the 68020's scale, are ignored (PRM 2.4).
    /// JMP, JSR, LEA and PEA apply the word still waiting in IRC.
    pub(crate) fn indexed(&self, base: u32, extension: u32) -> u32 {
        let mut index = self.r[(extension >> 12) as usize & 15];
        if extension & 0x0800 == 0 {
            index = sign_extend_16(index) as u32;
        }
        base.wrapping_add(index)
            .wrapping_add(sign_extend_8(extension) as u32)
    }

    /// Compute a memory operand's address, with its extension words and clocks.
    ///
    /// The standard source-operand sequence: the one TST, CMP, ADD <ea>,Dn
    /// and the read half of a read-modify-write use.
    pub(crate) fn ea_address(
        &mut self,
        kind: u8,
        register: usize,
        size: usize,
    ) -> Result<u32, GroupZero> {
        match kind {
            IND => Ok(self.r[8 + register]),
            POSTINC => {
                let address = self.r[8 + register];
                self.r[8 + register] = address.wrapping_add(self.step_size(register, size));
                Ok(address)
            }
            PREDEC => {
                self.cycles += 2;
                self.commit_pc();
                let address = self.r[8 + register].wrapping_sub(self.step_size(register, size));
                self.r[8 + register] = address;
                Ok(address)
            }
            DISP => {
                let displacement = sign_extend_16(self.extension()?) as u32;
                Ok(self.r[8 + register].wrapping_add(displacement))
            }
            INDEX => {
                self.cycles += 2;
                self.index(self.r[8 + register])
            }
            ABSW => {
                self.commit_pc();
                Ok(word_to_long(self.extension()?))
            }
            ABSL => {
                let high = self.extension()?;
                self.commit_pc();
                Ok((high << 16) | self.extension()?)
            }
            PCDISP => {
                let base = self.pc.wrapping_sub(2); // the address of the extension word itself
                Ok(base.wrapping_add(sign_extend_16(self.extension()?) as u32))
            }
            PCINDEX => {
                self.cycles += 2;
                self.index(self.pc.wrapping_sub(2))
            }
            _ => unreachable!("not a memory kind: {kind}"),
        }
    }

    /// Fetch a source operand of any kind (UM Table 8-1).
    pub(crate) fn ea_read(
        &mut self,
        kind: u8,
        register: usize,
        size: usize,
    ) -> Result<u32, GroupZero> {
        match kind {
            DN => Ok(self.r[register] & MASK[size]),
            AN => Ok(self.r[8 + register] & MASK[size]),
            IMM => {
                if size == 4 {
                    self.extension_long()
                } else {
                    Ok(self.extension()? & MASK[size])
                }
            }
            _ => Ok(self.ea_fetch(kind, register, size)?.1),
        }
    }

    /// Address and value of a memory operand: the read half of every access.
    ///
    /// PC-relative operands are read in program space (FC 2/6, UM Table 3-2).
    pub(crate) fn ea_fetch(
        &mut self,
        kind: u8,
        register: usize,
        size: usize,
    ) -> Result<(u32, u32), GroupZero> {
        if size == 4 && kind == POSTINC {
            // A long (An)+ steps An between its two reads (SST ADD.l, MOVE.l
            // and every long (An)+ read; T3): a fault on the first leaves An
            // alone.
            let address = self.r[8 + register];
            let high = self.read_word(address)?;
            self.r[8 + register] = address.wrapping_add(4);
            return Ok((
                address,
                (high << 16) | self.read_word(address.wrapping_add(2))?,
            ));
        }
        if size == 4 && kind == PREDEC {
            // A long -(An) does not bring PC up to date, unlike a word (SST
            // ADD.l, MOVE.l and every long -(An) read; T3).
            self.cycles += 2;
            let address = self.r[8 + register].wrapping_sub(4);
            self.r[8 + register] = address;
            return Ok((address, self.read_long(address)?));
        }
        let address = self.ea_address(kind, register, size)?;
        if kind >= PCDISP {
            return Ok((address, self.read_program_operand(size, address)?));
        }
        Ok((address, self.read(size, address)?))
    }

    /// Write the low byte, word or all of a data register (PRM 1.1).
    pub(crate) fn write_register(&mut self, register: usize, size: usize, value: u32) {
        if size == 4 {
            self.r[register] = value;
        } else {
            let mask = MASK[size];
            self.r[register] = (self.r[register] & !mask) | (value & mask);
        }
    }
}
