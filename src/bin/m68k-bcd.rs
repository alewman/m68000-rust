//! ABCD, SBCD and NBCD over every input, against flamewing's hardware-verified tables (rung 4).
//!
//! A transcription of m68000-python's `tests/test_bcd.py`.
//! flamewing/68k-bcd-verifier (GPL-3.0, commit
//! 39a01be528b0744302bf1dc9b3463fc22a3fc45f) is a Sega Genesis ROM that runs
//! ABCD and SBCD on every pair of bytes and NBCD on every byte, each with X
//! and Z clear and set, and checks the result and all five flags against a
//! table its `bcd-gen.cc` generates; its README says real hardware passes
//! every test (Model 1 and Model 3 VA2 Genesis), so the table is T1. Neither
//! the table nor the generator is copied here: the core's table, in the
//! generator's exact layout, is hashed and compared with the SHA-256 of the
//! generator's output.
//!
//! Layout (bcd-gen.cc): for ABCD then SBCD, for source 0-255, destination
//! 0-255, X 0-1, Z 0-1: one byte of flags (X N Z V C in bits 4-0) and one
//! byte of result; then NBCD for operand 0-255, X 0-1, Z 0-1. Initial N, V
//! clear and C equal to X. 1,050,624 bytes.
//!
//! Usage: `m68k-bcd [bcd-table.bin]`; with a local copy of the table a
//! mismatch is reported case by case instead of as a hash.

use std::process::ExitCode;

use m68000_rust::{Bus, BusError, CpuState, Cycle, M68000};

const TABLE_SHA256: &str = "8432868c9aa93c92574bae48bebd4efb2834e298340eb09530625365b80147e5";
const ABCD_D1_D0: u16 = 0xC101; // ABCD D1,D0: destination D0, source D1
const SBCD_D1_D0: u16 = 0x8101;
const NBCD_D0: u16 = 0x4800;

/// Every word reads as NOP, for the prefetch; writes go nowhere.
struct Nops;

impl Bus for Nops {
    fn read_byte(&mut self, _address: u32, _cycle: Cycle) -> Result<u8, BusError> {
        Ok(0x4E)
    }
    fn read_word(&mut self, _address: u32, _cycle: Cycle) -> Result<u16, BusError> {
        Ok(0x4E71)
    }
    fn write_byte(&mut self, _address: u32, _value: u8, _cycle: Cycle) -> Result<(), BusError> {
        Ok(())
    }
    fn write_word(&mut self, _address: u32, _value: u16, _cycle: Cycle) -> Result<(), BusError> {
        Ok(())
    }
}

fn run(cpu: &mut M68000<Nops>, opcode: u16, destination: u32, source: u32, x: bool, z: bool) -> [u8; 2] {
    let mut d = [0; 8];
    d[0] = destination;
    d[1] = source;
    cpu.restore_state(&CpuState {
        d,
        sr: 0x2700 | if x { 0x11 } else { 0 } | if z { 0x04 } else { 0 },
        pc: 0x1000,
        ir: opcode,
        irc: 0x4E71,
        ..CpuState::default()
    });
    cpu.step();
    [(cpu.sr & 0x1F) as u8, cpu.r[0] as u8]
}

fn core_table() -> Vec<u8> {
    let mut cpu = M68000::new(Nops);
    let mut out = Vec::with_capacity(1_050_624);
    for opcode in [ABCD_D1_D0, SBCD_D1_D0] {
        for source in 0..256 {
            for destination in 0..256 {
                for x in [false, true] {
                    for z in [false, true] {
                        out.extend(run(&mut cpu, opcode, destination, source, x, z));
                    }
                }
            }
        }
    }
    for operand in 0..256 {
        for x in [false, true] {
            for z in [false, true] {
                out.extend(run(&mut cpu, NBCD_D0, operand, 0, x, z));
            }
        }
    }
    out
}

fn describe(index: usize) -> String {
    let mut case = index / 2;
    if case < 2 * 262_144 {
        let name = if case < 262_144 { "ABCD" } else { "SBCD" };
        case %= 262_144;
        let (source, rest) = (case / 1024, case % 1024);
        let (destination, rest) = (rest / 4, rest % 4);
        return format!(
            "{name} source={source:#04x} destination={destination:#04x} X={} Z={}",
            rest >> 1,
            rest & 1
        );
    }
    case -= 2 * 262_144;
    let (operand, rest) = (case / 4, case % 4);
    format!("NBCD operand={operand:#04x} X={} Z={}", rest >> 1, rest & 1)
}

/// SHA-256 (FIPS 180-4), so the gate needs no dependency.
fn sha256(data: &[u8]) -> String {
    const K: [u32; 64] = [
        0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5, 0x3956c25b, 0x59f111f1, 0x923f82a4,
        0xab1c5ed5, 0xd807aa98, 0x12835b01, 0x243185be, 0x550c7dc3, 0x72be5d74, 0x80deb1fe,
        0x9bdc06a7, 0xc19bf174, 0xe49b69c1, 0xefbe4786, 0x0fc19dc6, 0x240ca1cc, 0x2de92c6f,
        0x4a7484aa, 0x5cb0a9dc, 0x76f988da, 0x983e5152, 0xa831c66d, 0xb00327c8, 0xbf597fc7,
        0xc6e00bf3, 0xd5a79147, 0x06ca6351, 0x14292967, 0x27b70a85, 0x2e1b2138, 0x4d2c6dfc,
        0x53380d13, 0x650a7354, 0x766a0abb, 0x81c2c92e, 0x92722c85, 0xa2bfe8a1, 0xa81a664b,
        0xc24b8b70, 0xc76c51a3, 0xd192e819, 0xd6990624, 0xf40e3585, 0x106aa070, 0x19a4c116,
        0x1e376c08, 0x2748774c, 0x34b0bcb5, 0x391c0cb3, 0x4ed8aa4a, 0x5b9cca4f, 0x682e6ff3,
        0x748f82ee, 0x78a5636f, 0x84c87814, 0x8cc70208, 0x90befffa, 0xa4506ceb, 0xbef9a3f7,
        0xc67178f2,
    ];
    let mut h: [u32; 8] = [
        0x6a09e667, 0xbb67ae85, 0x3c6ef372, 0xa54ff53a, 0x510e527f, 0x9b05688c, 0x1f83d9ab,
        0x5be0cd19,
    ];
    let mut message = data.to_vec();
    message.push(0x80);
    while message.len() % 64 != 56 {
        message.push(0);
    }
    message.extend_from_slice(&((data.len() as u64) * 8).to_be_bytes());
    for block in message.chunks(64) {
        let mut w = [0u32; 64];
        for (i, word) in block.chunks(4).enumerate() {
            w[i] = u32::from_be_bytes(word.try_into().unwrap());
        }
        for i in 16..64 {
            let s0 = w[i - 15].rotate_right(7) ^ w[i - 15].rotate_right(18) ^ (w[i - 15] >> 3);
            let s1 = w[i - 2].rotate_right(17) ^ w[i - 2].rotate_right(19) ^ (w[i - 2] >> 10);
            w[i] = w[i - 16]
                .wrapping_add(s0)
                .wrapping_add(w[i - 7])
                .wrapping_add(s1);
        }
        let mut v = h;
        for i in 0..64 {
            let s1 = v[4].rotate_right(6) ^ v[4].rotate_right(11) ^ v[4].rotate_right(25);
            let ch = (v[4] & v[5]) ^ (!v[4] & v[6]);
            let t1 = v[7]
                .wrapping_add(s1)
                .wrapping_add(ch)
                .wrapping_add(K[i])
                .wrapping_add(w[i]);
            let s0 = v[0].rotate_right(2) ^ v[0].rotate_right(13) ^ v[0].rotate_right(22);
            let maj = (v[0] & v[1]) ^ (v[0] & v[2]) ^ (v[1] & v[2]);
            let t2 = s0.wrapping_add(maj);
            v = [
                t1.wrapping_add(t2),
                v[0],
                v[1],
                v[2],
                v[3].wrapping_add(t1),
                v[4],
                v[5],
                v[6],
            ];
        }
        for (state, value) in h.iter_mut().zip(v) {
            *state = state.wrapping_add(value);
        }
    }
    h.iter().map(|word| format!("{word:08x}")).collect()
}

fn main() -> ExitCode {
    let table = core_table();
    assert_eq!(table.len(), 1_050_624);
    if let Some(path) = std::env::args().nth(1) {
        let expected = std::fs::read(&path).expect("readable table");
        for (index, (got, want)) in table.iter().zip(&expected).enumerate() {
            if got != want {
                println!("{}: {got:#04x} != {want:#04x}", describe(index));
                return ExitCode::FAILURE;
            }
        }
    }
    let digest = sha256(&table);
    if digest == TABLE_SHA256 {
        println!("BCD tables: all 525,312 inputs agree (SHA-256 {digest})");
        ExitCode::SUCCESS
    } else {
        println!("BCD tables differ: SHA-256 {digest}, expected {TABLE_SHA256}");
        ExitCode::FAILURE
    }
}
