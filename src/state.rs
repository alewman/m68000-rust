//! Immutable values for capturing and restoring MC68000 processor state.
//!
//! Transcribed from `m68000_python/state.py`. The Python dataclass validates
//! every field on construction; here [`CpuState::validate`] does the same
//! checks, and the trace reader and manifest loader call it.

/// Complete CPU-owned state at an instruction boundary.
///
/// Registers, the prefetch queue, the interrupt input as the core last saw
/// it, and the internal state that decides what the next `step()` does.
/// Host memory, devices and scheduling are excluded: restoring this value
/// restores the processor, not a machine.
///
/// `pc` is the address of the instruction in `ir`, always even (a jump to an
/// odd address faults before it becomes PC); the queue has read `irc` from
/// `pc + 2` and will next read `pc + 4` (the SingleStepTests corpus's `pc`).
/// Stopped by STOP, `pc` is the STOP's own address and execution resumes at
/// `pc + 4`.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct CpuState {
    pub d: [u32; 8],
    pub a: [u32; 7],
    pub usp: u32,
    pub ssp: u32,
    pub sr: u16,
    pub pc: u32,
    pub ir: u16,
    pub irc: u16,
    // Inputs, as the host has set them.
    pub ipl: u8,
    // Internal state.
    /// A level 7 edge seen and not yet taken.
    pub nmi_edge: bool,
    /// The last instruction ran with T set.
    pub trace_pending: bool,
    /// Inside STOP.
    pub stopped: bool,
    /// Double bus fault: only reset leaves it.
    pub halted: bool,
    /// Clocks run so far (the E-clock phase comes from it).
    pub clock: u64,
}

impl Default for CpuState {
    fn default() -> Self {
        CpuState {
            d: [0; 8],
            a: [0; 7],
            usp: 0,
            ssp: 0,
            sr: 0x2700,
            pc: 0,
            ir: 0,
            irc: 0,
            ipl: 0,
            nmi_edge: false,
            trace_pending: false,
            stopped: false,
            halted: false,
            clock: 0,
        }
    }
}

/// The field names in declaration order, as the trace schema lists them.
pub const STATE_FIELDS: [&str; 14] = [
    "d",
    "a",
    "usp",
    "ssp",
    "sr",
    "pc",
    "ir",
    "irc",
    "ipl",
    "nmi_edge",
    "trace_pending",
    "stopped",
    "halted",
    "clock",
];

impl CpuState {
    /// The checks `CPUState.__post_init__` makes that the Rust types do not.
    pub fn validate(&self) -> Result<(), String> {
        if self.pc & 1 != 0 {
            return Err("pc must be even: instructions live at even addresses".into());
        }
        if self.sr & !0xA71F != 0 {
            return Err("sr bits 14, 12, 11 and 7-5 read as zero on the 68000".into());
        }
        if self.ipl > 7 {
            return Err("ipl must be an integer in range 0..0x7".into());
        }
        Ok(())
    }

    /// The active stack pointer: `ssp` in supervisor mode, else `usp`.
    pub fn a7(&self) -> u32 {
        if self.sr & 0x2000 != 0 {
            self.ssp
        } else {
            self.usp
        }
    }
}
