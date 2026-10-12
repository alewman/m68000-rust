//! Processor state, the bus, the prefetch queue, and exception entry.
//!
//! Transcribed from `m68000_python/_core.py`. Everything a handler touches
//! goes through the helpers here: sized register access, the host bus with
//! its odd-address check, the two-word prefetch queue, and the group 0/1/2
//! exception sequences. Sources are cited as in the reference's
//! `docs/start-here.md`: PRM is the M68000 Family Programmer's Reference
//! Manual, UM the M68000 User's Manual. Where the manuals are silent, the
//! order of bus accesses, the internal ("idle") clocks, and the value an
//! address error stacks as PC follow the pinned SingleStepTests/m68000
//! corpus, which records them as MAME 0.285's microcode-transcribed core
//! produces them (tier T3).
//!
//! Time is counted in clocks as the core goes: every bus access adds 4 and
//! every internal step adds the clocks the microcode spends there, so an
//! instruction that faults halfway has spent exactly the clocks up to the
//! fault.
//!
//! Differences forced by the language:
//!
//! - The reference's host passes four callables and optional hooks; here the
//!   host is a [`Bus`] value owned by [`M68000`], with the hooks as provided
//!   methods. Every bus call receives a [`Cycle`]: the function code (which
//!   the reference passes only to a host that asks, `function_codes=True`)
//!   and `step_clocks` (which the reference's host reads from the CPU inside
//!   the callable).
//! - `BusError`, which the reference's host raises, is a returned error, and
//!   the internal `GroupZero` exception is the `Err` of every handler,
//!   propagated with `?`.
//! - Python's unbounded integers become `u32` with wrapping arithmetic where
//!   the reference masks with `& 0xFFFFFFFF`, and `i64`/`u64` where it
//!   compares an unmasked result.

use crate::dispatch::Op;

// -- status register (PRM 1.3.1) ----------------------------------------------
pub const T: u16 = 0x8000; // trace
pub const S: u16 = 0x2000; // supervisor
pub const IPL_MASK: u16 = 0x0700; // interrupt priority mask, I2-I0
pub const X: u16 = 0x10; // extend
pub const N: u16 = 0x08; // negative
pub const Z: u16 = 0x04; // zero
pub const V: u16 = 0x02; // overflow
pub const C: u16 = 0x01; // carry
pub const CCR_BITS: u16 = 0x1F;
/// Bits 14, 12, 11, 7, 6 and 5 read as zero on the 68000 (PRM 1.3.1).
pub const SR_BITS: u16 = 0xA71F;

pub const MASK24: u32 = 0xFF_FFFF; // the 68000 drives A23-A1 (UM 3.1)

// Function codes FC2-FC0 (UM Table 3-2).
pub const FC_USER_DATA: u8 = 1;
pub const FC_USER_PROGRAM: u8 = 2;
pub const FC_SUPERVISOR_DATA: u8 = 5;
pub const FC_SUPERVISOR_PROGRAM: u8 = 6;
pub const FC_CPU_SPACE: u8 = 7;

// Exception vectors (UM Table 6-2).
pub const VECTOR_BUS_ERROR: u32 = 2;
pub const VECTOR_ADDRESS_ERROR: u32 = 3;
pub const VECTOR_ILLEGAL: u32 = 4;
pub const VECTOR_ZERO_DIVIDE: u32 = 5;
pub const VECTOR_CHK: u32 = 6;
pub const VECTOR_TRAPV: u32 = 7;
pub const VECTOR_PRIVILEGE: u32 = 8;
pub const VECTOR_TRACE: u32 = 9;
pub const VECTOR_LINE_A: u32 = 10;
pub const VECTOR_LINE_F: u32 = 11;
pub const VECTOR_UNINITIALIZED: u32 = 15;
pub const VECTOR_SPURIOUS: u32 = 24;
pub const VECTOR_AUTOVECTOR_BASE: u32 = 24; // level n autovectors through vector 24 + n
pub const VECTOR_TRAP_BASE: u32 = 32;

/// `Bus::acknowledge` answers: a vector number 0-255, or one of these.
pub const AUTOVECTOR: i32 = -1;
pub const SPURIOUS: i32 = -2;

/// Operand sizes in bytes (1, 2, 4) index their masks and sign bits.
pub const MASK: [u32; 5] = [0, 0xFF, 0xFFFF, 0, 0xFFFF_FFFF];
pub const MSB: [u32; 5] = [0, 0x80, 0x8000, 0, 0x8000_0000];

/// Returned by a host bus method to assert BERR on that access.
///
/// The core catches it at the access, aborts the instruction, and takes the
/// bus-error exception (vector 2) with the seven-word group 0 frame
/// (UM 6.3.9.1). Most hosts never return it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BusError;

/// What the core tells the host about the access being made.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Cycle {
    /// The function code FC2-FC0 (UM Table 3-2): 1/2 user data/program,
    /// 5/6 supervisor data/program, 7 CPU space (the acknowledge cycle).
    pub fc: u8,
    /// Clocks the step in progress has spent so far, counting this access
    /// as complete: it occupies clocks `step_clocks - 4` to `step_clocks`
    /// of the step (the reference's `step_clocks` property read inside a
    /// callable). In `acknowledge` the cycle's own four clocks are not yet
    /// counted.
    pub step_clocks: u32,
    /// The prefetch queue's second word, IRC, as the access begins (the
    /// reference's `irc` attribute read inside a callable): at a data access
    /// it is the next opcode word, which a board drives onto an undriven
    /// data bus (megadrive-python's open bus).
    pub irc: u16,
}

/// The host: memory and every device.
///
/// Addresses are already masked to 24 bits. Values are bytes and words.
/// The provided methods are the reference's optional keyword arguments,
/// with its defaults.
pub trait Bus {
    fn read_byte(&mut self, address: u32, cycle: Cycle) -> Result<u8, BusError>;
    fn read_word(&mut self, address: u32, cycle: Cycle) -> Result<u16, BusError>;
    fn write_byte(&mut self, address: u32, value: u8, cycle: Cycle) -> Result<(), BusError>;
    fn write_word(&mut self, address: u32, value: u16, cycle: Cycle) -> Result<(), BusError>;

    /// The interrupt-acknowledge cycle: a vector number (0-255),
    /// [`AUTOVECTOR`], or [`SPURIOUS`]; any other value is an uninitialized
    /// vector. The default autovectors every interrupt, as on most arcade
    /// boards.
    fn acknowledge(&mut self, _level: u8, _cycle: Cycle) -> i32 {
        AUTOVECTOR
    }

    /// The write half of TAS's read-modify-write cycle, for a bus that
    /// treats it differently (the Genesis drops it). Defaults to
    /// `write_byte`.
    fn tas_write(&mut self, address: u32, value: u8, cycle: Cycle) -> Result<(), BusError> {
        self.write_byte(address, value, cycle)
    }

    /// Clocks the host adds to [`M68000::clock`] once `acknowledge` has
    /// answered, before an autovectored acknowledge reads the E clock from
    /// it. The reference's host may set `clock` inside its acknowledge
    /// callable (megadrive-python credits the bus holds of the instruction
    /// so far there); a Rust host cannot reach the CPU from its bus, so it
    /// returns the amount here. The default adds nothing.
    fn acknowledge_clock_credit(&mut self) -> u64 {
        0
    }

    /// Told about the access an address error aborted, which never reaches
    /// the bus (UM 6.3.10).
    fn address_error(&mut self, _address: u32, _write: bool, _fc: u8) {}

    /// Called when the RESET instruction pulses the RESET line (PRM 6-83);
    /// the processor itself is not reset.
    fn reset_devices(&mut self) {}
}

/// Internal: an address or bus error aborting the current instruction.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct GroupZero {
    pub vector: u32,
    /// The full 32-bit value the microcode computed.
    pub address: u32,
    pub write: bool,
    pub program: bool,
}

/// What every handler returns: `Err` is the reference's `GroupZero` raise.
pub type Step = Result<(), GroupZero>;

/// A Motorola MC68000 instruction core (also the MC68HC000 and MC68EC000).
///
/// Registers: `r` is sixteen 32-bit values, D0-D7 then A0-A7, where A7 is
/// the stack pointer of the current mode; [`M68000::usp`] and
/// [`M68000::ssp`] name the two stack pointers whatever the mode. `sr` is
/// the 16-bit status register. The prefetch queue is `ir` (the next
/// instruction's first word) and `irc` (the word after it), read from
/// memory ahead of execution as the chip does.
pub struct M68000<B: Bus> {
    pub bus: B,
    // r[0..7] are D0-D7, r[8..15] are A0-A7; A7 is the active stack
    // pointer and the inactive one waits in other_sp (PRM 1.3).
    pub r: [u32; 16],
    pub(crate) other_sp: u32,
    pub sr: u16,
    // The prefetch queue: ir is the opcode word of the next instruction,
    // irc the word after it, and pc the address the next queue refill
    // reads, which is the instruction's address + 4 at a boundary (the
    // reference's `_pc`, the corpus's `pc`).
    pub ir: u16,
    pub irc: u16,
    pub(crate) pc: u32,
    // The microcode's PC register: an address error stacks it. It trails
    // pc and is brought up to it only at the steps where the microcode
    // copies the address unit into PC (see commit_pc).
    pub(crate) fault_pc: u32,
    pub(crate) opcode: u16, // the instruction being executed (IRD)
    pub(crate) cycles: u32, // clocks of the step in progress
    /// Clocks run since construction: the sum of every `step()` and
    /// `reset()`. The host may set it; only the E-clock phase of an
    /// autovectored interrupt acknowledge depends on it.
    pub clock: u64,
    // Interrupts, trace and the stopped/halted states (UM 6.3).
    pub(crate) ipl: u8,        // the level on IPL2-IPL0 as the host last set it
    pub(crate) nmi_edge: bool, // a 0-to-7 transition not yet taken
    pub(crate) trace_pending: bool,
    // Set by an instruction that is never executed (illegal, line A/F,
    // privilege violation): no trace follows it (UM 6.3.8).
    pub(crate) untraced: bool,
    pub(crate) processing_exception: bool,
    pub stopped: bool,
    pub halted: bool,
    /// E-clock phase (clock mod 10) at the last autovectored acknowledge.
    pub last_acknowledge_phase: u32,
    pub(crate) table: &'static [Op; 0x10000],
}

impl<B: Bus> M68000<B> {
    // -- the stack pointers and the status register --------------------------

    pub(crate) fn fc(&self, program: bool) -> u8 {
        if self.sr & S != 0 {
            if program {
                FC_SUPERVISOR_PROGRAM
            } else {
                FC_SUPERVISOR_DATA
            }
        } else if program {
            FC_USER_PROGRAM
        } else {
            FC_USER_DATA
        }
    }

    fn cycle(&self, program: bool) -> Cycle {
        Cycle {
            fc: self.fc(program),
            step_clocks: self.cycles,
            irc: self.irc,
        }
    }

    /// Write the whole SR, swapping A7 with the other stack pointer if S changes.
    pub(crate) fn set_sr_internal(&mut self, value: u16) {
        let value = value & SR_BITS;
        if (value ^ self.sr) & S != 0 {
            std::mem::swap(&mut self.r[15], &mut self.other_sp);
        }
        self.sr = value;
    }

    pub(crate) fn set_ccr(&mut self, value: u16) {
        self.sr = (self.sr & 0xFF00) | (value & CCR_BITS);
    }

    /// The user stack pointer, whichever mode the CPU is in.
    pub fn usp(&self) -> u32 {
        if self.sr & S != 0 {
            self.other_sp
        } else {
            self.r[15]
        }
    }

    pub fn set_usp(&mut self, value: u32) {
        if self.sr & S != 0 {
            self.other_sp = value;
        } else {
            self.r[15] = value;
        }
    }

    /// The supervisor stack pointer, whichever mode the CPU is in.
    pub fn ssp(&self) -> u32 {
        if self.sr & S != 0 {
            self.r[15]
        } else {
            self.other_sp
        }
    }

    pub fn set_ssp(&mut self, value: u32) {
        if self.sr & S != 0 {
            self.r[15] = value;
        } else {
            self.other_sp = value;
        }
    }

    // -- the bus ------------------------------------------------------------
    // Every access checks alignment first: a word or long at an odd address
    // is an address error, and the aborted cycle never reaches the host
    // (UM 6.3.10; the corpus marks it "re"/"we", AS not asserted).

    pub(crate) fn fault(&self, vector: u32, address: u32, write: bool, program: bool) -> GroupZero {
        GroupZero {
            vector,
            address,
            write,
            program,
        }
    }

    pub(crate) fn read_word(&mut self, address: u32) -> Result<u32, GroupZero> {
        if address & 1 != 0 {
            return Err(self.fault(VECTOR_ADDRESS_ERROR, address, false, false));
        }
        self.cycles += 4;
        let cycle = self.cycle(false);
        match self.bus.read_word(address & MASK24, cycle) {
            Ok(value) => Ok(u32::from(value)),
            Err(BusError) => Err(self.fault(VECTOR_BUS_ERROR, address, false, false)),
        }
    }

    pub(crate) fn read_byte(&mut self, address: u32) -> Result<u32, GroupZero> {
        self.cycles += 4;
        let cycle = self.cycle(false);
        match self.bus.read_byte(address & MASK24, cycle) {
            Ok(value) => Ok(u32::from(value)),
            Err(BusError) => Err(self.fault(VECTOR_BUS_ERROR, address, false, false)),
        }
    }

    /// Two word reads, high word first (UM 2.4).
    pub(crate) fn read_long(&mut self, address: u32) -> Result<u32, GroupZero> {
        let high = self.read_word(address)?;
        Ok((high << 16) | self.read_word(address.wrapping_add(2))?)
    }

    pub(crate) fn write_word(&mut self, address: u32, value: u32) -> Step {
        if address & 1 != 0 {
            return Err(self.fault(VECTOR_ADDRESS_ERROR, address, true, false));
        }
        self.cycles += 4;
        let cycle = self.cycle(false);
        match self.bus.write_word(address & MASK24, value as u16, cycle) {
            Ok(()) => Ok(()),
            Err(BusError) => Err(self.fault(VECTOR_BUS_ERROR, address, true, false)),
        }
    }

    pub(crate) fn write_byte(&mut self, address: u32, value: u32) -> Step {
        self.cycles += 4;
        let cycle = self.cycle(false);
        match self.bus.write_byte(address & MASK24, value as u8, cycle) {
            Ok(()) => Ok(()),
            Err(BusError) => Err(self.fault(VECTOR_BUS_ERROR, address, true, false)),
        }
    }

    /// Two word writes, high word first.
    pub(crate) fn write_long(&mut self, address: u32, value: u32) -> Step {
        if address & 1 != 0 {
            return Err(self.fault(VECTOR_ADDRESS_ERROR, address, true, false));
        }
        self.write_word(address, value >> 16)?;
        self.write_word(address.wrapping_add(2), value)
    }

    /// Two word writes, low word (at address + 2) first.
    ///
    /// The order of -(An) destinations and of read-modify-write results: the
    /// microcode works toward the high word (SST MOVE.l, ADDX.l, SUBX.l,
    /// MOVEM.l and the long read-modify-write files; T3). An odd address
    /// faults on the first write, at `address + 2`.
    pub(crate) fn write_long_low_first(&mut self, address: u32, value: u32) -> Step {
        self.write_word(address.wrapping_add(2), value)?;
        self.write_word(address, value >> 16)
    }

    pub(crate) fn read(&mut self, size: usize, address: u32) -> Result<u32, GroupZero> {
        if size == 2 {
            return self.read_word(address);
        }
        if size == 1 {
            return self.read_byte(address);
        }
        self.read_long(address)
    }

    /// A byte or word write; every long write orders its halves at its caller.
    pub(crate) fn write(&mut self, size: usize, address: u32, value: u32) -> Step {
        if size == 2 {
            self.write_word(address, value)
        } else {
            self.write_byte(address, value)
        }
    }

    /// One program-space word from the host: the reference's `_read_program`.
    fn bus_read_program(&mut self, address: u32) -> Result<u32, BusError> {
        let cycle = self.cycle(true);
        self.bus.read_word(address & MASK24, cycle).map(u32::from)
    }

    pub(crate) fn read_program_word(&mut self, address: u32) -> Result<u32, GroupZero> {
        if address & 1 != 0 {
            return Err(self.fault(VECTOR_ADDRESS_ERROR, address, false, true));
        }
        self.cycles += 4;
        match self.bus_read_program(address) {
            Ok(value) => Ok(value),
            Err(BusError) => Err(self.fault(VECTOR_BUS_ERROR, address, false, true)),
        }
    }

    /// A PC-relative operand: read in program space (UM Table 3-2, FC 2/6).
    pub(crate) fn read_program_operand(
        &mut self,
        size: usize,
        address: u32,
    ) -> Result<u32, GroupZero> {
        if size == 1 {
            self.cycles += 4;
            let cycle = self.cycle(true);
            return match self.bus.read_byte(address & MASK24, cycle) {
                Ok(value) => Ok(u32::from(value)),
                Err(BusError) => Err(self.fault(VECTOR_BUS_ERROR, address, false, true)),
            };
        }
        let high = self.read_program_word(address)?;
        if size == 2 {
            return Ok(high);
        }
        Ok((high << 16) | self.read_program_word(address.wrapping_add(2))?)
    }

    // -- the prefetch queue ---------------------------------------------------

    /// Copy the fetch address into the microcode's PC register.
    ///
    /// Only an address error can see the difference: it stacks this value
    /// (UM 6.2.5 "unpredictable"; the steps that commit it are the corpus's,
    /// T3).
    pub(crate) fn commit_pc(&mut self) {
        self.fault_pc = self.pc;
    }

    // The refills make no odd-address check because pc is always even:
    // every path that sets it either faults first on an odd target (jump,
    // jump_idle, DBcc, JSR read the target's word through read_program_word
    // before taking it) or refuses one (set_pc, restore_state), and the
    // refills add 2.

    /// Take the extension word waiting in IRC and refill IRC from pc.
    pub(crate) fn extension(&mut self) -> Result<u32, GroupZero> {
        let value = u32::from(self.irc);
        let address = self.pc;
        self.cycles += 4;
        match self.bus_read_program(address) {
            Ok(word) => self.irc = word as u16,
            Err(BusError) => return Err(self.fault(VECTOR_BUS_ERROR, address, false, true)),
        }
        self.pc = address.wrapping_add(2);
        Ok(value)
    }

    pub(crate) fn extension_long(&mut self) -> Result<u32, GroupZero> {
        let high = self.extension()?;
        Ok((high << 16) | self.extension()?)
    }

    /// The last step of every instruction: IRC moves to IR, IRC refills.
    ///
    /// The opcode of the next instruction is then in IR and its first
    /// extension word in IRC (UM "Prefetch"; corpus `prefetch` pair).
    pub(crate) fn prefetch(&mut self) -> Step {
        let address = self.pc;
        self.fault_pc = address;
        // IR takes IRC, and the decoder (IRD) takes IR, before the read: a
        // bus error on this read already stacks the next opcode (every
        // faulting closing prefetch of the corpus, T3, where the fault is an
        // odd jump target's caught in jump before the queue moves).
        self.ir = self.irc;
        self.opcode = self.irc;
        self.cycles += 4;
        match self.bus_read_program(address) {
            Ok(word) => self.irc = word as u16,
            Err(BusError) => return Err(self.fault(VECTOR_BUS_ERROR, address, false, true)),
        }
        self.pc = address.wrapping_add(2);
        Ok(())
    }

    /// The closing prefetch of an instruction that still has a write to do.
    ///
    /// IR takes IRC and IRC refills, but the decoder keeps the current
    /// opcode until the handler hands it over with `self.opcode = self.ir`
    /// at the step the microcode does (SST MOVE.l, PEA; T3): an address
    /// error on a write before that point stacks the old opcode.
    pub(crate) fn prefetch_before_write(&mut self) -> Step {
        let address = self.pc;
        self.fault_pc = address;
        self.ir = self.irc;
        self.cycles += 4;
        match self.bus_read_program(address) {
            Ok(word) => self.irc = word as u16,
            Err(BusError) => return Err(self.fault(VECTOR_BUS_ERROR, address, false, true)),
        }
        self.pc = address.wrapping_add(2);
        Ok(())
    }

    /// Refill the queue from `target`: two program reads (UM Table 8-9).
    ///
    /// The first read lands in IRC; the closing prefetch moves it to IR and
    /// reads the word after it.
    pub(crate) fn jump(&mut self, target: u32) -> Step {
        self.irc = self.read_program_word(target)? as u16;
        self.pc = target.wrapping_add(2);
        self.prefetch()
    }

    /// A jump whose refill has two internal clocks between its reads.
    pub(crate) fn jump_idle(&mut self, target: u32) -> Step {
        self.irc = self.read_program_word(target)? as u16;
        self.pc = target.wrapping_add(2);
        self.cycles += 2;
        self.prefetch()
    }

    // -- the stack -------------------------------------------------------------

    /// Push a long, high word first (BSR, JSR, PEA; corpus, T3).
    pub(crate) fn push_long(&mut self, value: u32) -> Step {
        let sp = self.r[15].wrapping_sub(4);
        self.r[15] = sp;
        self.write_long(sp, value)
    }

    pub(crate) fn pop_word(&mut self) -> Result<u32, GroupZero> {
        let sp = self.r[15];
        let value = self.read_word(sp)?;
        self.r[15] = sp.wrapping_add(2);
        Ok(value)
    }

    pub(crate) fn pop_long(&mut self) -> Result<u32, GroupZero> {
        let sp = self.r[15];
        let value = self.read_long(sp)?;
        self.r[15] = sp.wrapping_add(4);
        Ok(value)
    }

    // -- exceptions (UM 6.2, 6.3) ---------------------------------------------

    /// Step 1 of exception processing: copy SR, set S, clear T (UM 6.2.5).
    pub(crate) fn enter_supervisor(&mut self) -> u16 {
        let saved = self.sr;
        self.set_sr_internal((saved | S) & !T);
        saved
    }

    /// Group 1 and 2 exception entry with the three-word frame (UM Figure 6-5).
    ///
    /// `pc` is the value to stack; `idle` the internal clocks before the
    /// first push (the reference's default is 4). The pushes, the vector
    /// fetch and the refill are in the order the corpus records: PC low, SR,
    /// PC high, vector high and low, then the handler's first two words with
    /// two idle clocks between. `by_instruction` marks group 2 (TRAP, TRAPV,
    /// CHK, divide by zero), whose processing the instruction itself starts.
    pub(crate) fn exception(
        &mut self,
        vector: u32,
        pc: u32,
        idle: u32,
        saved: Option<u16>,
        by_instruction: bool,
    ) -> Step {
        let entered = self.enter_supervisor();
        let saved = saved.unwrap_or(entered);
        self.trace_pending = false;
        self.cycles += idle;
        // An address or bus error from here on is one "not an instruction"
        // access, I/N set in its information word (UM Figure 6-7), except in
        // group 2, where the instruction started the processing: I/N clear
        // (WinUAE's tester core, T2 by running; MAME 0.285's microcode, T3).
        self.processing_exception = !by_instruction;
        let sp = self.r[15].wrapping_sub(6);
        self.r[15] = sp;
        self.write_word(sp.wrapping_add(4), pc)?;
        self.write_word(sp, u32::from(saved))?;
        self.write_word(sp.wrapping_add(2), pc >> 16)?;
        self.fault_pc = pc;
        let target = self.read_vector(vector)?;
        self.jump_idle(target)?;
        self.processing_exception = false;
        Ok(())
    }

    pub(crate) fn read_vector(&mut self, vector: u32) -> Result<u32, GroupZero> {
        let address = vector << 2;
        let high = self.read_word(address)?;
        Ok((high << 16) | self.read_word(address + 2)?)
    }

    /// Address or bus error: abort, stack the seven-word frame (UM Figure 6-7).
    ///
    /// The aborted access costs its four clocks and four more; two internal
    /// steps of two clocks follow (the corpus's 55,606 address errors, T3;
    /// WinUAE run agrees on the clocks). The frame is written in the
    /// corpus's order: PC low, SR, PC high, IR, access address low, access
    /// information, access address high. A second group 0 fault while doing
    /// this halts the processor (UM 5.4.4).
    pub(crate) fn group_zero(&mut self, fault: GroupZero) {
        if fault.vector == VECTOR_ADDRESS_ERROR {
            let fc = self.fc(fault.program);
            self.bus
                .address_error(fault.address & MASK24, fault.write, fc);
        }
        self.cycles += 4 + 4 + 2 + 2;
        // The access information word: bits 15-5 are the undefined part and
        // carry IR's (the corpus's faulting cases, T3; WinUAE run agrees on
        // every frame it judges; UM Figure 6-7 marks them undefined), R/W is
        // bit 4, I/N bit 3 (set when the access was part of exception
        // processing rather than of an instruction), and the function code.
        let information = u32::from(self.opcode & 0xFFE0)
            | if fault.write { 0 } else { 0x10 }
            | if self.processing_exception { 0x08 } else { 0 }
            | u32::from(self.fc(fault.program));
        self.processing_exception = false;
        let saved = self.enter_supervisor();
        self.trace_pending = false;
        let pc = self.fault_pc;
        let sp = self.r[15].wrapping_sub(14);
        self.r[15] = sp;
        let frame = (|| -> Step {
            self.write_word(sp.wrapping_add(12), pc)?;
            self.write_word(sp.wrapping_add(8), u32::from(saved))?;
            self.write_word(sp.wrapping_add(10), pc >> 16)?;
            self.write_word(sp.wrapping_add(6), u32::from(self.opcode))?;
            self.write_word(sp.wrapping_add(4), fault.address)?;
            self.write_word(sp, information)?;
            self.write_word(sp.wrapping_add(2), fault.address >> 16)?;
            let target = self.read_vector(fault.vector)?;
            self.jump_idle(target)
        })();
        if frame.is_err() {
            // A group 0 fault while processing a group 0 exception: the double
            // bus fault halts the processor until reset (UM 5.4.4, 6.3.9.1).
            // MAME 0.285 takes another address error instead; the corpus has
            // no such case, and the manual is followed (the reference's
            // docs/claims.md, "Undecidable here").
            self.halted = true;
        }
    }
}
