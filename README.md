# m68000-rust

A Rust Motorola MC68000 instruction core transcribed from
[m68000-python](https://github.com/alewman/m68000-python) and proven
equivalent to it at every processor boundary: the boundary kind, the clocks,
the instruction's address and bytes, all fourteen fields of `CPUState` before
and after, and every bus access in order. Not merely "passes the corpus".

The module layout mirrors the reference so the two can be read side by
side. Each `src/*.rs` file is the transcription of one `m68000_python`
module (`core`, `ea`, `flags`, `alu`, `bcd`, `bits`, `control`, `loads`,
`shifts`, `system`, `dispatch`, `state`, `cpu`), with the same handler
names, the same opcode map built from the same `RULES`, and the hardware
comments and their sources carried over. Nothing is redesigned; where the
language forced a difference, the module header says what it is.

## Certification

Pinned oracles, as m68000-python's `docs/conformance.md` requires:

| What | Version |
| --- | --- |
| m68000-python (reference core and conformance kit) | 0.1.0 plus the conformance kit at commit `b3df005` (branch `conformance-kit`, [m68000-python#5](https://github.com/alewman/m68000-python/pull/5): main `38dbd0f` + the kit + the reader fix this port found) |
| Trace schema | version 1 |
| SingleStepTests/m68000 corpus | revision `64b253116a3de04aaac4346c43680960dc9b67e5` |
| flamewing/68k-bcd-verifier | commit `39a01be528b0744302bf1dc9b3463fc22a3fc45f`, table SHA-256 `8432868c…80147e5` |

Ladder status at this commit, each rung reproduced with the script named
(`scripts/`) on Linux x86_64 with rustc 1.93.1, CPython 3.14.4 and PyPy
7.3.20 / Python 3.11.13:

| Rung | What | Result | Script |
| --- | --- | --- | --- |
| 1 | The example manifests of the reference's kit diff clean | `exceptions`, `flags-and-branches`, `tas-drop`: `traces are identical` | `rung1.sh` |
| 2 | SingleStepTests/m68000, 127 files: registers, SR, both stack pointers, the prefetch address and pair, RAM, clocks, every bus transaction with strobes and function code, and `step_clocks` at every access end of the cases without an address error | `TOTAL: 317500 passed, 0 failed / 317500 cases in 127 files; step_clocks checked in 261894` | `rung2.sh` |
| 3 | The reference's two MAME 0.285 lockstep recordings as `replay` manifests (real game code, its interrupts, every device read replayed), diffed in lockstep through a pipe | System 16B `altbeast`, 30 s: 24,597,382 records, 263,504,451 clocks; Genesis Altered Beast, 40 s: 28,253,448 records, 306,763,197 clocks; both `traces are identical`, 5 min 6 s and 5 min 49 s wall with the reference on PyPy | `rung3.sh` |
| 4 | ABCD, SBCD and NBCD over every input against flamewing's hardware-verified table (T1) | `BCD tables: all 525,312 inputs agree` | `rung4.sh` |
| 5 | The interrupt and STOP scenarios as manifests with events (`examples/conformance/interrupts/` in the reference) | all 20 `traces are identical` | `rung5.sh` |

A gate that cannot fail proves nothing, so rung 2 was checked against two
seeded faults: V never set for ADD.w fails 250 cases, and TAS's write moved
two clocks earlier (same total) fails 2,108 on `step_clocks` alone.

CI (`.github/workflows/ci.yml`) runs `cargo fmt --check`, `cargo clippy -D
warnings` and `cargo test` (which replays the example manifests against
copies of the reference traces without Python), and rungs 2 and 4 on every
push; rungs 1 and 5 need the reference checked out (below).

The replay manifests are made from ROMs and MAME traces, so neither
repository commits them: `python validation/lockstep.py export` in the
reference builds them from its recorded runs (its `docs/conformance.md`,
rung 3). The export follows MAME's registers before every instruction of
both games, so the replayed machine is the game, not an approximation of it.

### Reproducing

```text
scripts/fetch_m68000python.sh        # m68000-python at the pin into external/, with a venv
scripts/fetch_test_vectors.sh        # SingleStepTests/m68000 at the pinned revision into external/
scripts/rung1.sh
scripts/rung2.sh
scripts/rung3.sh                     # needs the replay manifests, below
scripts/rung4.sh
scripts/rung5.sh
```

## Using the core

```rust
use m68000_rust::{Bus, BusError, Cycle, M68000};

struct Flat(Vec<u8>);

impl Bus for Flat {
    fn read_byte(&mut self, a: u32, _: Cycle) -> Result<u8, BusError> { Ok(self.0[a as usize]) }
    fn read_word(&mut self, a: u32, _: Cycle) -> Result<u16, BusError> {
        Ok(u16::from_be_bytes([self.0[a as usize], self.0[a as usize + 1]]))
    }
    fn write_byte(&mut self, a: u32, v: u8, _: Cycle) -> Result<(), BusError> {
        self.0[a as usize] = v;
        Ok(())
    }
    fn write_word(&mut self, a: u32, v: u16, _: Cycle) -> Result<(), BusError> {
        self.0[a as usize..a as usize + 2].copy_from_slice(&v.to_be_bytes());
        Ok(())
    }
}

let mut memory = vec![0u8; 1 << 24];
memory[0x1000..0x1002].copy_from_slice(&[0x70, 0x2A]); // moveq #42, D0
let mut cpu = M68000::new(Flat(memory));
cpu.set_pc(0x1000).unwrap();
assert_eq!(cpu.step(), 4);
assert_eq!(cpu.r[0], 42);
```

The embedding contract is the reference's (its README, "The embedding
contract"), in Rust form:

- The host implements `Bus`: four accessors over a 24-bit address space,
  each told the access's function code and `step_clocks` (where in the step
  the access ends) in a `Cycle`. Returning `Err(BusError)` asserts BERR.
- Its provided methods are the reference's optional hooks, with the same
  defaults: `acknowledge(level)` (autovector), `tas_write` (an ordinary
  byte write; the Genesis drops it), `address_error` and `reset_devices`.
- `step()` runs one instruction or exception entry and returns its clocks;
  `reset()`, `set_ipl()`, `set_pc()`, `capture_state()` and `restore_state()`
  are the reference's.

## Notes for the reference

Rung 3 found one bug, in the reference's trace reader rather than its core:
`disassemble_bytes` keyed an instruction's words at the full address while
the disassembler reads at the 24-bit one, so any record at a PC above
`$FFFFFF` was rejected. Genesis Altered Beast runs code at `$FFFFF200`,
reached through a sign-extended short jump. The test and fix are commits
`f7dec3f` and `b3df005` on the reference's `conformance-kit` branch. No
divergence between the two cores was found at any rung.

## Binaries

- `m68k-trace <manifest>`: run a conformance manifest and stream its trace
  for `python -m m68000_python.conformance diff manifest -`.
- `m68k-vectors <dir>`: the SingleStepTests/m68000 runner (rung 2).
- `m68k-bcd [table]`: the BCD table check (rung 4).
- `m68k-bench`: the reference's four benchmark workloads.

## Speed

`m68k-bench` runs the reference's benchmark programs through `step()`, one
process pinned to one core of an i9-13900K that was also running other
people's jobs (2026-10-02):

| Workload | m68000-rust | m68000-python, PyPy 7.3.20 | m68000-python, CPython 3.14.4 |
| --- | ---: | ---: | ---: |
| base (a copy loop through (An)+, DBF) | 156 M instr/s | 32.9 M | 1.39 M |
| memory (long moves, MOVEM, PEA) | 101 M | 18.7 M | 0.65 M |
| arithmetic (MULU, DIVU, shifts, ABCD) | 157 M | 19.1 M | 1.18 M |
| exceptions (TRAP #0, RTE) | 157 M | 33.5 M | 1.32 M |

The reference's columns are its own record (its `docs/validation.md`,
"Speed"); this is a transcription, not an optimised core, and speed is the
job of a `-fast` fork, as z80-rust-fast is for the Z80.

## Where the brief is

[docs/handoff-brief.md](docs/handoff-brief.md) is the request this
repository was built from and the plan read from it, and
[docs/build-record.md](docs/build-record.md) is what happened.

## License

MIT, like the reference. The SingleStepTests/m68000 corpus (MIT) and
flamewing's verifier (GPL-3.0; only the SHA-256 of its table is used) are
external and not bundled.
