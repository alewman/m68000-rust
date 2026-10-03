//! The native SingleStepTests/m68000 runner (rung 2).
//!
//! A transcription of m68000-python's `tests/corpus.py` (the binary
//! container), `tests/harness.py` (the corpus host and what each case
//! compares) and `tests/test_step_clocks.py` (where in a step each access
//! falls). Per case: D0-D7, A0-A6, USP, SSP, SR, the prefetch address, the
//! prefetch pair, every RAM word the final state names, the clock total,
//! the ordered bus transactions with kind, address, size, value, strobes
//! and function code (an aborted access's value is not compared: no data
//! moves when AS is not asserted), and, for the cases without an address
//! error, `step_clocks` at the end of every access.
//!
//! Usage: `m68k-vectors <dir-or-files>...` (a directory means every
//! `*.json.bin` in it).

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use m68000_rust::{Bus, BusError, CpuState, Cycle, M68000};

const FILE_MAGIC: u32 = 0x1A3F_5D71;
const TEST_MAGIC: u32 = 0xABC1_2367;
const NAME_MAGIC: u32 = 0x89AB_CDEF;
const STATE_MAGIC: u32 = 0x0123_4567;
const TRANSACTIONS_MAGIC: u32 = 0x4567_89AB;

/// d0-d7, a0-a6, usp, ssp, sr, pc: the container's register order.
const REGISTERS: [&str; 19] = [
    "d0", "d1", "d2", "d3", "d4", "d5", "d6", "d7", "a0", "a1", "a2", "a3", "a4", "a5", "a6",
    "usp", "ssp", "sr", "pc",
];

struct State {
    registers: [u32; 19],
    prefetch: [u32; 2],
    ram: Vec<(u32, u16)>,
}

/// One corpus access: kind, address, byte or word, bus value, strobes, FC.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Access {
    kind: &'static str,
    address: u32,
    word: bool,
    data: Option<u32>,
    uds: u32,
    lds: u32,
    fc: u32,
}

struct Case {
    name: String,
    initial: State,
    final_state: State,
    cycles: u32,
    accesses: Vec<Access>,
    /// The clock at which each non-aborted access ends.
    ends: Vec<u32>,
    aborted: bool,
}

struct Reader<'a> {
    buffer: &'a [u8],
    position: usize,
}

impl Reader<'_> {
    fn u32(&mut self) -> u32 {
        let bytes = &self.buffer[self.position..self.position + 4];
        self.position += 4;
        u32::from_le_bytes(bytes.try_into().unwrap())
    }
    fn u16(&mut self) -> u16 {
        let bytes = &self.buffer[self.position..self.position + 2];
        self.position += 2;
        u16::from_le_bytes(bytes.try_into().unwrap())
    }
    fn u8(&mut self) -> u8 {
        let byte = self.buffer[self.position];
        self.position += 1;
        byte
    }
}

fn read_state(reader: &mut Reader) -> Result<State, String> {
    let start = reader.position;
    let size = reader.u32() as usize;
    if reader.u32() != STATE_MAGIC {
        return Err(format!("bad state magic at {start:#x}"));
    }
    let mut registers = [0; 19];
    for value in &mut registers {
        *value = reader.u32();
    }
    let prefetch = [reader.u32(), reader.u32()];
    let count = reader.u32();
    let mut ram = Vec::with_capacity(count as usize);
    for _ in 0..count {
        let address = reader.u32();
        ram.push((address, reader.u16()));
    }
    if reader.position != start + size {
        return Err(format!("state length mismatch at {start:#x}"));
    }
    Ok(State {
        registers,
        prefetch,
        ram,
    })
}

fn read_file(path: &Path) -> Result<Vec<Case>, String> {
    let buffer = std::fs::read(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let mut reader = Reader {
        buffer: &buffer,
        position: 0,
    };
    if reader.u32() != FILE_MAGIC {
        return Err(format!(
            "{}: not a SingleStepTests/m68000 file",
            path.display()
        ));
    }
    let count = reader.u32();
    let mut cases = Vec::with_capacity(count as usize);
    for _ in 0..count {
        let start = reader.position;
        let size = reader.u32() as usize;
        if reader.u32() != TEST_MAGIC {
            return Err(format!("bad test magic at {start:#x}"));
        }
        reader.u32();
        if reader.u32() != NAME_MAGIC {
            return Err(format!("bad name magic at {start:#x}"));
        }
        let length = reader.u32() as usize;
        let name = String::from_utf8_lossy(&buffer[reader.position..reader.position + length]);
        let name = name.into_owned();
        reader.position += length;
        let initial = read_state(&mut reader)?;
        let final_state = read_state(&mut reader)?;
        let transactions_start = reader.position;
        let transactions_size = reader.u32() as usize;
        if reader.u32() != TRANSACTIONS_MAGIC {
            return Err(format!("bad transactions magic at {transactions_start:#x}"));
        }
        let cycles = reader.u32();
        let count = reader.u32();
        let mut accesses = Vec::new();
        let mut ends = Vec::new();
        let mut aborted = false;
        let mut now = 0;
        for _ in 0..count {
            let kind = reader.u8();
            let length = reader.u32();
            now += length;
            if kind == 0 {
                continue; // idle: only its sum, part of the clock total, is compared
            }
            let fc = reader.u32();
            let address = reader.u32();
            let data = reader.u32();
            let uds = reader.u32();
            let lds = reader.u32();
            let kind = match kind {
                1 => "w",
                2 => "r",
                3 => "t",
                4 => "re",
                5 => "we",
                other => return Err(format!("unknown transaction kind {other}")),
            };
            let error = kind == "re" || kind == "we";
            aborted |= error;
            if !error {
                ends.push(now);
            }
            accesses.push(Access {
                kind,
                address,
                word: uds != 0 && lds != 0,
                data: if error { None } else { Some(data) },
                uds,
                lds,
                fc,
            });
        }
        if reader.position != transactions_start + transactions_size
            || reader.position != start + size
        {
            return Err(format!("test length mismatch at {start:#x}"));
        }
        cases.push(Case {
            name,
            initial,
            final_state,
            cycles,
            accesses,
            ends,
            aborted,
        });
    }
    Ok(cases)
}

/// A sparse word memory that logs every access the way the corpus does.
struct CorpusHost {
    memory: HashMap<u32, u16>,
    log: Vec<Access>,
    clocks: Vec<u32>,
}

impl CorpusHost {
    fn note(&mut self, access: Access, cycle: Cycle) {
        self.log.push(access);
        self.clocks.push(cycle.step_clocks);
    }
}

impl Bus for CorpusHost {
    fn read_word(&mut self, address: u32, cycle: Cycle) -> Result<u16, BusError> {
        let value = *self.memory.get(&address).unwrap_or(&0);
        let access = Access {
            kind: "r",
            address,
            word: true,
            data: Some(u32::from(value)),
            uds: 1,
            lds: 1,
            fc: u32::from(cycle.fc),
        };
        self.note(access, cycle);
        Ok(value)
    }

    fn write_word(&mut self, address: u32, value: u16, cycle: Cycle) -> Result<(), BusError> {
        self.memory.insert(address, value);
        let access = Access {
            kind: "w",
            address,
            word: true,
            data: Some(u32::from(value)),
            uds: 1,
            lds: 1,
            fc: u32::from(cycle.fc),
        };
        self.note(access, cycle);
        Ok(())
    }

    fn read_byte(&mut self, address: u32, cycle: Cycle) -> Result<u8, BusError> {
        let word = *self.memory.get(&(address & !1)).unwrap_or(&0);
        let fc = u32::from(cycle.fc);
        if address & 1 != 0 {
            let access = Access {
                kind: "r",
                address: address & !1,
                word: false,
                data: Some(u32::from(word & 0xFF)),
                uds: 0,
                lds: 1,
                fc,
            };
            self.note(access, cycle);
            return Ok(word as u8);
        }
        let access = Access {
            kind: "r",
            address,
            word: false,
            data: Some(u32::from(word & 0xFF00)),
            uds: 1,
            lds: 0,
            fc,
        };
        self.note(access, cycle);
        Ok((word >> 8) as u8)
    }

    fn write_byte(&mut self, address: u32, value: u8, cycle: Cycle) -> Result<(), BusError> {
        let even = address & !1;
        let word = *self.memory.get(&even).unwrap_or(&0);
        let fc = u32::from(cycle.fc);
        let value = u16::from(value);
        if address & 1 != 0 {
            self.memory.insert(even, (word & 0xFF00) | value);
            let access = Access {
                kind: "w",
                address: even,
                word: false,
                data: Some(u32::from(value)),
                uds: 0,
                lds: 1,
                fc,
            };
            self.note(access, cycle);
        } else {
            self.memory.insert(even, (word & 0x00FF) | (value << 8));
            let access = Access {
                kind: "w",
                address: even,
                word: false,
                data: Some(u32::from(value) << 8),
                uds: 1,
                lds: 0,
                fc,
            };
            self.note(access, cycle);
        }
        Ok(())
    }

    // The corpus logs TAS's write half as an ordinary write ("w"); the
    // provided `tas_write` is `write_byte`, as the reference's harness has it.

    fn address_error(&mut self, address: u32, write: bool, fc: u8) {
        self.log.push(Access {
            kind: if write { "we" } else { "re" },
            address: address & !1,
            word: true,
            data: None,
            uds: 1,
            lds: 1,
            fc: u32::from(fc),
        });
    }
}

fn state_of(state: &State) -> CpuState {
    let r = &state.registers;
    let mut d = [0; 8];
    d.copy_from_slice(&r[0..8]);
    let mut a = [0; 7];
    a.copy_from_slice(&r[8..15]);
    CpuState {
        d,
        a,
        usp: r[15],
        ssp: r[16],
        sr: r[17] as u16,
        // The corpus's pc is the prefetch address: the instruction's + 4.
        pc: r[18].wrapping_sub(4),
        ir: state.prefetch[0] as u16,
        irc: state.prefetch[1] as u16,
        ..CpuState::default()
    }
}

fn run_case(case: &Case) -> Vec<String> {
    let host = CorpusHost {
        memory: case.initial.ram.iter().copied().collect(),
        log: Vec::new(),
        clocks: Vec::new(),
    };
    let mut cpu = M68000::new(host);
    cpu.restore_state(&state_of(&case.initial));
    let cycles = cpu.step();
    let mut differences = Vec::new();
    let after = cpu.capture_state();
    let mut actual = [0u32; 19];
    actual[0..8].copy_from_slice(&after.d);
    actual[8..15].copy_from_slice(&after.a);
    actual[15] = after.usp;
    actual[16] = after.ssp;
    actual[17] = u32::from(after.sr);
    actual[18] = after.pc.wrapping_add(4);
    for (index, name) in REGISTERS.iter().enumerate() {
        let expected = case.final_state.registers[index];
        if actual[index] != expected {
            differences.push(format!("{name}: {:#x} != {expected:#x}", actual[index]));
        }
    }
    let prefetch = [u32::from(after.ir), u32::from(after.irc)];
    if prefetch != case.final_state.prefetch {
        differences.push(format!(
            "prefetch: {prefetch:04x?} != {:04x?}",
            case.final_state.prefetch
        ));
    }
    for &(address, value) in &case.final_state.ram {
        let got = *cpu.bus.memory.get(&address).unwrap_or(&0);
        if got != value {
            differences.push(format!("ram[{address:#08x}]: {got:#06x} != {value:#06x}"));
        }
    }
    if cycles != case.cycles {
        differences.push(format!("cycles: {cycles} != {}", case.cycles));
    }
    if cpu.bus.log != case.accesses {
        differences.push(format!(
            "transactions differ\n  got:      {:?}\n  expected: {:?}",
            cpu.bus.log, case.accesses
        ));
    }
    if !case.aborted && cpu.bus.clocks != case.ends {
        differences.push(format!(
            "step_clocks: {:?} != {:?}",
            cpu.bus.clocks, case.ends
        ));
    }
    if cpu.step_clocks() != cycles {
        differences.push(format!(
            "step_clocks() after the step: {}",
            cpu.step_clocks()
        ));
    }
    differences
}

fn main() -> ExitCode {
    let mut files: Vec<PathBuf> = Vec::new();
    for argument in std::env::args().skip(1) {
        let path = PathBuf::from(argument);
        if path.is_dir() {
            let mut found: Vec<PathBuf> = std::fs::read_dir(&path)
                .expect("readable directory")
                .filter_map(|entry| entry.ok().map(|entry| entry.path()))
                .filter(|p| p.to_string_lossy().ends_with(".json.bin"))
                .collect();
            found.sort();
            files.extend(found);
        } else {
            files.push(path);
        }
    }
    if files.is_empty() {
        eprintln!("usage: m68k-vectors <dir-or-files>...");
        return ExitCode::from(2);
    }
    let (mut passed, mut failed, mut timed) = (0usize, 0usize, 0usize);
    for path in &files {
        let cases = match read_file(path) {
            Ok(cases) => cases,
            Err(message) => {
                eprintln!("error: {message}");
                return ExitCode::from(2);
            }
        };
        let mut file_failed = 0;
        let mut first = None;
        for case in &cases {
            let differences = run_case(case);
            if !case.aborted {
                timed += 1;
            }
            if differences.is_empty() {
                passed += 1;
            } else {
                failed += 1;
                file_failed += 1;
                if first.is_none() {
                    first = Some((case.name.clone(), differences));
                }
            }
        }
        let stem = path.file_name().unwrap().to_string_lossy();
        let stem = stem.trim_end_matches(".json.bin");
        if let Some((name, differences)) = first {
            println!(
                "{stem}: {} passed, {file_failed} failed",
                cases.len() - file_failed
            );
            println!("  first: {name}");
            for difference in differences.iter().take(8) {
                println!("    {difference}");
            }
        } else {
            println!("{stem}: {} passed", cases.len());
        }
    }
    println!(
        "TOTAL: {passed} passed, {failed} failed / {} cases in {} files; step_clocks checked in {timed}",
        passed + failed,
        files.len()
    );
    if failed == 0 {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    }
}
