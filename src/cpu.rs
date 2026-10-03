//! The public MC68000 CPU: construction, state capture, signals, `reset()` and `step()`.
//!
//! Transcribed from `m68000_python/cpu.py`. The host owns memory and every
//! device: it implements [`Bus`], calls [`M68000::step`], and adds the
//! returned clock count to its own clock. Interrupts are a level the host
//! sets with [`M68000::set_ipl`]; `Bus::acknowledge` answers the
//! interrupt-acknowledge cycle with a vector number, [`AUTOVECTOR`] or
//! [`SPURIOUS`].

use crate::core::{
    Bus, BusError, Cycle, AUTOVECTOR, FC_CPU_SPACE, IPL_MASK, M68000, MASK24, S, SPURIOUS, T,
    VECTOR_AUTOVECTOR_BASE, VECTOR_SPURIOUS, VECTOR_TRACE, VECTOR_UNINITIALIZED,
};
use crate::dispatch::names;
use crate::state::CpuState;

/// Why [`M68000::set_pc`] refused to start.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StartError {
    /// Instructions live at even addresses (UM 6.3.10).
    OddAddress,
    /// The host's bus returned [`BusError`] on a queue read.
    Bus,
}

impl<B: Bus> M68000<B> {
    /// A CPU with the reference's power-on values: every register zero, SR
    /// $2700 (S set, mask 7; undefined at reset on silicon apart from those,
    /// UM 6.3.1), the queue empty. Call [`M68000::reset`] or
    /// [`M68000::set_pc`] before stepping.
    pub fn new(bus: B) -> Self {
        M68000 {
            bus,
            r: [0; 16],
            other_sp: 0,
            sr: S | IPL_MASK,
            ir: 0,
            irc: 0,
            pc: 0,
            fault_pc: 0,
            opcode: 0,
            cycles: 0,
            clock: 0,
            ipl: 0,
            nmi_edge: false,
            trace_pending: false,
            untraced: false,
            processing_exception: false,
            stopped: false,
            halted: false,
            last_acknowledge_phase: 0,
            table: names(),
        }
    }

    // -- state capture and restore ------------------------------------------

    /// Return an immutable snapshot of all CPU-owned state; no host reads.
    pub fn capture_state(&self) -> CpuState {
        let r = &self.r;
        CpuState {
            d: r[0..8].try_into().unwrap(),
            a: r[8..15].try_into().unwrap(),
            usp: self.usp(),
            ssp: self.ssp(),
            sr: self.sr,
            pc: self.pc.wrapping_sub(4),
            ir: self.ir,
            irc: self.irc,
            ipl: self.ipl,
            nmi_edge: self.nmi_edge,
            trace_pending: self.trace_pending,
            stopped: self.stopped,
            halted: self.halted,
            clock: self.clock,
        }
    }

    /// Restore a captured state without touching the host's memory or devices.
    pub fn restore_state(&mut self, state: &CpuState) {
        self.r[0..8].copy_from_slice(&state.d);
        self.r[8..15].copy_from_slice(&state.a);
        self.sr = state.sr;
        if state.sr & S != 0 {
            self.r[15] = state.ssp;
            self.other_sp = state.usp;
        } else {
            self.r[15] = state.usp;
            self.other_sp = state.ssp;
        }
        self.pc = state.pc.wrapping_add(4);
        self.ir = state.ir;
        self.irc = state.irc;
        self.ipl = state.ipl;
        self.nmi_edge = state.nmi_edge;
        self.trace_pending = state.trace_pending;
        self.stopped = state.stopped;
        self.halted = state.halted;
        self.clock = state.clock;
    }

    // -- the program counter as a programmer sees it --------------------------

    /// The address of the next instruction (the queue reads 4 bytes ahead):
    /// the reference's `PC` property.
    pub fn program_counter(&self) -> u32 {
        self.pc.wrapping_sub(4)
    }

    /// Start executing at `address`: refill the prefetch queue from it.
    ///
    /// Two program reads, as a jump does; no clocks are counted. The host
    /// uses this instead of a reset when it loads a program itself. An odd
    /// address is refused: instructions live at even addresses (UM 6.3.10),
    /// and a jump to an odd one is an address error, not a start.
    pub fn set_pc(&mut self, address: u32) -> Result<(), StartError> {
        if address & 1 != 0 {
            return Err(StartError::OddAddress);
        }
        let cycle = Cycle {
            fc: self.fc(true),
            step_clocks: self.cycles,
        };
        let ir = self.bus.read_word(address & MASK24, cycle);
        let ir = ir.map_err(|BusError| StartError::Bus)?;
        let irc = self.bus.read_word(address.wrapping_add(2) & MASK24, cycle);
        self.ir = ir;
        self.irc = irc.map_err(|BusError| StartError::Bus)?;
        self.pc = address.wrapping_add(4);
        self.stopped = false;
        Ok(())
    }

    // -- external signals -----------------------------------------------------

    /// Write SR as an instruction would: A7 follows S to the other stack pointer.
    ///
    /// Assigning `sr` directly changes the bits and nothing else, which is
    /// what a state restore wants and a host usually does not.
    pub fn set_sr(&mut self, value: u16) {
        self.set_sr_internal(value);
    }

    /// The address execution resumes at: what an interrupt or trace stacks.
    ///
    /// At a boundary the queue has read 4 bytes ahead. STOP leaves the queue
    /// as it was (its immediate still in IRC, corpus T3), so a stopped CPU
    /// resumes at the prefetch address itself: the instruction after STOP.
    fn next_pc(&self) -> u32 {
        if self.stopped {
            self.pc
        } else {
            self.pc.wrapping_sub(4)
        }
    }

    /// The interrupt level on IPL2-IPL0 as the host last set it.
    pub fn ipl(&self) -> u8 {
        self.ipl
    }

    /// Set the interrupt level on IPL2-IPL0 (0 = none, 7 = non-maskable).
    ///
    /// Level 7 is edge-triggered: a change to 7 from below is taken once
    /// even with the mask at 7 (UM 6.3.2). Panics on a level above 7, where
    /// the reference raises `ValueError`.
    pub fn set_ipl(&mut self, level: u8) {
        assert!(level <= 7, "IPL level must be 0-7");
        if level == 7 && self.ipl != 7 {
            self.nmi_edge = true;
        }
        self.ipl = level;
    }

    /// The reset exception (UM 6.3.1): S set, T clear, mask 7, SSP and PC from 0 and 4.
    ///
    /// Nothing is pushed. A fault while fetching the vectors or the first
    /// instruction (an odd initial PC) halts the processor as a double bus
    /// fault (UM 5.4.4). Returns the clocks spent, 40 as UM Table 8-14
    /// prints: 14 internal, the four vector reads, and the two-read refill
    /// of the queue with its 2 idle clocks, as every other exception entry
    /// refills it. The 14 are Nuked-MD's: its gate-level 68000 reads the SSP
    /// vector 14 clocks after RESET is released.
    pub fn reset(&mut self) -> u32 {
        self.cycles = 0;
        self.halted = false;
        self.stopped = false;
        self.trace_pending = false;
        self.nmi_edge = false;
        self.set_sr_internal((self.sr | S | IPL_MASK) & !T);
        self.cycles += 14;
        let sequence = (|| {
            let high = self.read_program_word(0)?;
            self.r[15] = (high << 16) | self.read_program_word(2)?;
            let high = self.read_program_word(4)?;
            let pc = (high << 16) | self.read_program_word(6)?;
            self.fault_pc = pc;
            self.jump_idle(pc)
        })();
        if sequence.is_err() {
            // An address or bus error during the reset sequence (an odd
            // initial PC, BERR on a vector) is a double bus fault: halt
            // (UM 5.4.4).
            self.halted = true;
        }
        self.clock += u64::from(self.cycles);
        self.cycles
    }

    // -- execution ------------------------------------------------------------

    /// Clocks the step in progress has spent so far.
    ///
    /// Between steps it is the last `step()`'s (or `reset()`'s) total.
    /// Inside a bus method the same value arrives as `Cycle::step_clocks`.
    pub fn step_clocks(&self) -> u32 {
        self.cycles
    }

    /// Run one instruction or one exception entry; return its clock count.
    ///
    /// At an instruction boundary, in this order: a halted CPU idles; a
    /// trace exception left by the previous instruction is taken (UM 6.3.8:
    /// trace outranks interrupts); an interrupt above the mask, or a level 7
    /// edge, is taken (UM 6.3.2); a stopped CPU idles; otherwise the
    /// instruction in IR runs.
    pub fn step(&mut self) -> u32 {
        if self.halted {
            self.cycles = 4;
            self.clock += 4;
            return 4;
        }
        self.cycles = 0;
        let level = self.ipl;
        let outcome = if self.trace_pending {
            self.trace_pending = false;
            self.opcode = self.ir;
            let pc = self.next_pc();
            let entered = self.exception(VECTOR_TRACE, pc, 4, None, false);
            if entered.is_ok() {
                self.stopped = false;
            }
            entered
        } else if level != 0
            && (u16::from(level) > (self.sr >> 8) & 7 || (level == 7 && self.nmi_edge))
        {
            self.interrupt(level)
        } else if self.stopped {
            self.cycles = 4;
            Ok(())
        } else {
            let opcode = self.ir;
            self.opcode = opcode;
            self.fault_pc = self.pc.wrapping_sub(2);
            let traced = self.sr & T != 0;
            if traced {
                self.untraced = false;
            }
            let executed = self.execute(self.table[usize::from(opcode)], opcode);
            if executed.is_ok() && traced && !self.untraced {
                // Trace follows an instruction that completed, as its own
                // boundary: the next step takes it (UM 6.3.8). An illegal or
                // privileged instruction was never executed and is not
                // traced (system::not_executed). The corpus's final states
                // are captured before the trace (its issue #2).
                self.trace_pending = true;
            }
            executed
        };
        if let Err(fault) = outcome {
            self.group_zero(fault);
        }
        self.clock += u64::from(self.cycles);
        self.cycles
    }

    /// Interrupt entry (UM 6.3.2-6.3.4): acknowledge, stack, vector.
    ///
    /// The mask rises to the accepted level; the acknowledge cycle names the
    /// vector, or asks for the autovector (24 + level), or reports spurious
    /// (vector 24). A vector number outside 0-255 is an uninitialized vector
    /// (15).
    fn interrupt(&mut self, level: u8) -> crate::core::Step {
        if level == 7 {
            self.nmi_edge = false;
        }
        self.opcode = self.ir;
        let pc = self.next_pc(); // the instruction the interrupt came before
        self.stopped = false;
        // Three internal steps: SR copied, S set and T cleared, the mask
        // raised to the level being taken (UM 6.3.2; MAME 0.285's order).
        self.cycles += 6;
        let saved = self.sr;
        self.set_sr_internal(((saved | S) & !T & !IPL_MASK) | (u16::from(level) << 8));
        self.processing_exception = true;
        let sp = self.r[15];
        self.write_word(sp.wrapping_sub(2), pc)?;
        // The acknowledge cycle comes between the first push and the rest.
        let cycle = Cycle {
            fc: FC_CPU_SPACE,
            step_clocks: self.cycles,
        };
        let answer = self.bus.acknowledge(level, cycle);
        self.cycles += 4;
        let vector = if answer == AUTOVECTOR {
            // VPA: the cycle waits for the E clock (CLK/10) as the manual
            // describes (UM 5.1.4, 6.3.2); the phase is the host's running
            // clock count, `clock`, as MAME 0.285's vpa_sync takes it.
            self.cycles += self.e_clock_wait();
            VECTOR_AUTOVECTOR_BASE + u32::from(level)
        } else if answer == SPURIOUS {
            VECTOR_SPURIOUS
        } else if (0..=255).contains(&answer) {
            answer as u32
        } else {
            VECTOR_UNINITIALIZED
        };
        self.cycles += 4;
        let sp = sp.wrapping_sub(6);
        self.r[15] = sp;
        self.write_word(sp, u32::from(saved))?;
        self.write_word(sp.wrapping_add(2), pc >> 16)?;
        let target = self.read_vector(vector)?;
        self.jump_idle(target)?;
        self.processing_exception = false;
        Ok(())
    }

    /// Clocks an autovectored acknowledge waits for the E clock.
    ///
    /// MAME 0.285 (m68000.cpp, `vpa_sync` and `vpa_after`): with t the clock
    /// count at the start of the cycle, the transfer is aligned to the next
    /// E-clock period boundary, one period later when fewer than 3 clocks
    /// remain, and one clock is added after it. The MAME lockstep on System
    /// 16B agrees at every phase (the reference's docs/validation.md, rung 6).
    fn e_clock_wait(&mut self) -> u32 {
        let now = self.clock + u64::from(self.cycles) - 4;
        let phase = (now % 10) as u32;
        self.last_acknowledge_phase = phase;
        (if phase < 7 { 10 - phase } else { 20 - phase }) + 1
    }
}
