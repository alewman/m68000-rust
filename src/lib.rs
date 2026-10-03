//! A Rust MC68000 instruction core transcribed from `m68000-python`.
//!
//! The module layout mirrors the reference so the two can be read side by
//! side: `core`, `ea`, `flags`, `alu`, `bcd`, `bits`, `control`, `loads`,
//! `shifts`, `system`, `dispatch`, `state` and `cpu` correspond one-to-one
//! with `m68000_python/_core.py`, `_ea.py`, `_flags.py`, and so on (the
//! reference's mixins are `impl` blocks here). `trace` and `conformance`
//! implement the trace schema and the conformance kit (`docs/trace-schema.md`
//! and `docs/conformance.md` in m68000-python).

pub mod alu;
pub mod bcd;
pub mod bits;
pub mod conformance;
pub mod control;
pub mod core;
pub mod cpu;
pub mod dispatch;
pub mod ea;
pub mod flags;
pub mod loads;
pub mod shifts;
pub mod state;
pub mod system;
pub mod trace;

pub use crate::core::{Bus, BusError, Cycle, AUTOVECTOR, M68000, SPURIOUS};
pub use crate::cpu::StartError;
pub use crate::state::CpuState;
