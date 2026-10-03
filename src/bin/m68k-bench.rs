//! The reference's four benchmark workloads on this core.
//!
//! A transcription of m68000-python's `benchmarks/m68000_core_benchmark.py`:
//! the same programs on the same host (flat 16 MB RAM, every exception vector
//! pointing at an RTE, the CPU reset to $1000), run through `step()` for a
//! fixed instruction count; the rate is instructions per second over the
//! median of the timed samples. Absolute rates depend on the machine and its
//! load.
//!
//! Usage: `m68k-bench [instructions per sample] [samples]`.

use std::time::Instant;

use m68000_rust::{Bus, BusError, Cycle, M68000};

const START: u32 = 0x1000;
const HANDLER: u32 = 0x4000; // every exception vector points here: an RTE

struct Flat(Vec<u8>);

impl Bus for Flat {
    fn read_byte(&mut self, address: u32, _cycle: Cycle) -> Result<u8, BusError> {
        Ok(self.0[address as usize])
    }
    fn read_word(&mut self, address: u32, _cycle: Cycle) -> Result<u16, BusError> {
        let a = address as usize;
        Ok(u16::from_be_bytes([self.0[a], self.0[a + 1]]))
    }
    fn write_byte(&mut self, address: u32, value: u8, _cycle: Cycle) -> Result<(), BusError> {
        self.0[address as usize] = value;
        Ok(())
    }
    fn write_word(&mut self, address: u32, value: u16, _cycle: Cycle) -> Result<(), BusError> {
        let a = address as usize;
        self.0[a..a + 2].copy_from_slice(&value.to_be_bytes());
        Ok(())
    }
}

fn host(program: &[u16]) -> M68000<Flat> {
    let mut memory = vec![0u8; 1 << 24];
    let mut word = |address: u32, value: u16| {
        let a = address as usize;
        memory[a..a + 2].copy_from_slice(&value.to_be_bytes());
    };
    for vector in 0..256 {
        word(vector * 4, (HANDLER >> 16) as u16);
        word(vector * 4 + 2, HANDLER as u16);
    }
    word(0, 0);
    word(2, 0x8000); // SSP
    word(4, 0);
    word(6, START as u16); // PC
    word(HANDLER, 0x4E73); // rte
    for (index, value) in program.iter().enumerate() {
        word(START + 2 * index as u32, *value);
    }
    for offset in (0..0x400).step_by(2) {
        word(0x2000 + offset, ((offset * 17 + 3) & 0xFFFF) as u16); // data the memory workload reads
    }
    let mut cpu = M68000::new(Flat(memory));
    cpu.reset();
    cpu
}

#[rustfmt::skip]
const BASE: &[u16] = &[
    0x41F9, 0x0000, 0x2000,  // lea     $2000.l, A0
    0x43F9, 0x0000, 0x3000,  // lea     $3000.l, A1
    0x303C, 0x00FF,          // move.w  #$ff, D0
    0x3218,                  // move.w  (A0)+, D1          loop:
    0xD241,                  // add.w   D1, D1
    0x32C1,                  // move.w  D1, (A1)+
    0x0C41, 0x1234,          // cmpi.w  #$1234, D1
    0x6702,                  // beq.s   +2
    0x5282,                  // addq.l  #1, D2
    0x51C8, 0xFFF0,          // dbf     D0, loop
    0x60E2,                  // bra.s   start
];
#[rustfmt::skip]
const MEMORY: &[u16] = &[
    0x41F9, 0x0000, 0x2000,  // lea     $2000.l, A0        start:
    0x43F9, 0x0000, 0x3000,  // lea     $3000.l, A1
    0x22D8,                  // move.l  (A0)+, (A1)+
    0x22D8,                  // move.l  (A0)+, (A1)+
    0x2368, 0x0004, 0x0008,  // move.l  ($4,A0), ($8,A1)
    0x48E7, 0xFF00,          // movem.l D0-D7, -(A7)
    0x4CDF, 0x00FF,          // movem.l (A7)+, D0-D7
    0x4868, 0x0010,          // pea     ($10,A0)
    0x245F,                  // movea.l (A7)+, A2
    0x60E0,                  // bra.s   start
];
#[rustfmt::skip]
const ARITHMETIC: &[u16] = &[
    0x203C, 0x1234, 0x5678,  // move.l  #$12345678, D0     start:
    0xC0FC, 0x1234,          // mulu.w  #$1234, D0
    0x80FC, 0x0056,          // divu.w  #$56, D0
    0xE781,                  // asl.l   #3, D1
    0xEA5A,                  // ror.w   #5, D2
    0xC903,                  // abcd    D3, D4
    0x4842,                  // swap    D2
    0x60EA,                  // bra.s   start
];
#[rustfmt::skip]
const EXCEPTIONS: &[u16] = &[
    0x4E40,                  // trap    #0                 start:
    0x4E71,                  // nop
    0x60FA,                  // bra.s   start
];

fn main() {
    let mut arguments = std::env::args().skip(1);
    let count: u64 = arguments.next().map_or(50_000_000, |a| a.parse().unwrap());
    let samples: usize = arguments.next().map_or(5, |a| a.parse().unwrap());
    for (name, program) in [
        ("base", BASE),
        ("memory", MEMORY),
        ("arithmetic", ARITHMETIC),
        ("exceptions", EXCEPTIONS),
    ] {
        let mut times = Vec::new();
        let mut clocks = 0u64;
        for _ in 0..samples {
            let mut cpu = host(program);
            let started = Instant::now();
            let mut total = 0u64;
            for _ in 0..count {
                total += u64::from(cpu.step());
            }
            times.push(started.elapsed().as_secs_f64());
            clocks = total;
        }
        times.sort_by(f64::total_cmp);
        let median = times[times.len() / 2];
        println!(
            "{name:11} {:8.1} M instr/s  {:8.1} emulated MHz",
            count as f64 / median / 1e6,
            clocks as f64 / median / 1e6
        );
    }
}
