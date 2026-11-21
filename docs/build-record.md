# Build record

What happened when [handoff-brief.md](handoff-brief.md) was run, in order,
with the numbers. All on 2026-10-02, local time (PDT), on an i9-13900K
shared with other people's jobs; times come from the commit log and run
logs.

- **Reading.** z80-rust's brief, build record and README for the shape; then
  every module of `m68000_python` at `38dbd0f` (which includes the 40-clock
  reset of m68000-python#4), its trace schema, its `DebugSession`, its
  corpus harness, its BCD test and its benchmark, and z80-python's
  conformance kit as the model for the missing one.
- **The long-run candidate that failed.** MicroCoreLabs'
  `MC68000_test_all_opcodes` (MIT, written on Easy68K) was loaded on the
  reference first. It stops in its MOVE-from-SR failure loop after 2,002
  instructions: it expects SR bit 6 to read back, which a real 68000 never
  does (SR bits 7-5 read as zero). It was dropped as an oracle.
- **Two streams in parallel.** A fork agent built the conformance kit in
  m68000-python (worktree on branch `conformance-kit`; commits `b1f1b1b`,
  `5cf74de`, `72cb5b0`) while the core was transcribed here.
- **Core transcribed** (`e00bb12`): thirteen modules. It compiled on the
  first build with no warnings.
- **Rung 2** (`eed7719`): the SingleStepTests/m68000 runner. 317,500 of
  317,500 cases on the first run, `step_clocks` in all 261,894 cases without
  an address error, in 2.5 s. Two seeded faults showed the gate bites: V
  never set for ADD.w (250 failures) and TAS's write two clocks early with
  the same total (2,108 failures, on `step_clocks` alone).
- **Rung 4** (`c281e85`): the BCD tables, all 525,312 inputs, SHA-256 equal
  to flamewing's hardware-verified table, in 11 ms.
- **Trace producer and host** (`2b68dce`). The instruction bytes in a record
  need the reference disassembler's lengths; a table of the reference's
  length for every first word was generated and all 65,536 agree. One rule
  of the reference's access tracking had to be matched exactly and was told
  to the kit's author mid-run: reads are logged after they return, writes
  before the host's write runs, so a write that raises BusError is in the
  trace and a read that does is not.
- **Rungs 1 and 5** against the kit's examples: 3 programs and 20 interrupt
  and STOP scenarios, every one `traces are identical` on the first run. A
  hand-corrupted access value was reported by the reference's `diff`, so the
  comparison covers the access log.
- **Speed** (`aae36b7`): the reference's benchmark programs at 101-157 M
  instructions per second (the reference: 19-34 M on PyPy).
- **Rung 3.** The kit's `lockstep.py export` turned the two MAME recordings
  into replay manifests (the System 16B one starts after the i8751's early
  reset of the CPU, which no event can express). System 16B `altbeast`:
  24,597,382 records identical, 5 min 6 s. Genesis Altered Beast stopped at
  record 2,518,593: the reference's reader rejected a correct `lea
  $c00004.l, A5` at PC `$FFFFF200`, because `disassemble_bytes` keyed the
  words at the full address and read at the 24-bit one. Fixed in the
  reference test-first (`f7dec3f`, `b3df005`); the rerun: 28,253,448
  records identical, 5 min 49 s.
- **Repinned** at `b3df005`: rungs 1 and 5 rerun clean there, and the copies
  of the reference traces in `tests/data` are byte-identical to it.

## What was not needed

No divergence between the cores was found at any rung, so neither core was
changed for behavior. The only change in the reference this port caused is
the reader fix above, in its tooling.

## 2026-10-03: rung 6

- The user asked to continue with the transcription. z80-rust's rung 6 is
  FUSE, an emulator-derived set run natively with its divergences pinned;
  the 68000's counterpart is SingleStepTests/680x0, which the reference
  runs as a detector and classifies (its validation.md, "Rung 5").
- `m68k-680x0` transcribes the reference's 680x0 harness and classifier;
  `scripts/rung6.sh` diffs the two tables. Identical on the first run:
  787,660 of 1,000,060 agree, the same 19 causes with the same counts and
  files (9 s here, 33 s for the reference on PyPy). With V never set for
  ADD.w, ADD.w cases come out unclassified and the run fails.
