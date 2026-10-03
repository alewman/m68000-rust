# The brief this repository was built from

z80-rust was built from a written handoff prompt (kept in that repository's
`docs/handoff-brief.md`). This one was not: the request was one sentence, and
the session that received it read the shape of the z80 pair and wrote the
plan below before writing any Rust. It is kept here for the same reason
z80-rust keeps its brief: it is the specification the code was written to,
and it shows how much of the work the reference's own documents carried.
[build-record.md](build-record.md) says what happened.

## The request (2026-10-02)

> Let's start on a new project. I want you to convert our m68000 to rust.
> Follow the shape of z80-python to z80-rust

## How the session read it

The z80 pair's shape, as z80-rust's brief and README set it:

- **Transcribe, do not redesign.** The Rust modules mirror the reference's
  module for module, with the same handler names, the same explicit
  dispatch, and the hardware comments carried over; a difference forced by
  the language is described in the module header.
- **Equivalence at every processor boundary**, not "passes the tests at the
  end": kind, clocks, the instruction's address and bytes, and every field of
  the reference's `CPUState`, before and after. For the 68000 the bus accesses
  in order are added, since bus order is most of what its corpus pins.
- **A ladder of rungs, in order**, each reproduced by a script and pinned to
  the reference's commit and the oracles' revisions.
- **The reference is not modified to make the port pass.** A divergence is
  shown and the vectors decide; anything the port finds that belongs in the
  reference goes there as a pull request (z80-python#3 shipped z80-rust's
  interrupt manifests).
- Commits in reviewable units, local until the user asks to push.

What the 68000 needed that the z80 had not:

1. **A conformance kit in the reference.** z80-rust's rungs 1, 3 and 5 ran on
   z80-python's `conformance` module: manifests, one host profile both cores
   implement, and a lockstep `diff`. m68000-python's polish round had deferred
   that module ("trace-schema + one reference trace now"), so the first piece
   of work is that kit, built in m68000-python on a local branch: a
   `flat` host (16 MiB, the reset or an initial state, the acknowledge map,
   bus-error ranges, the Genesis TAS-write drop, ipl and reset events, stop
   rules), every trace record carrying its bus accesses, example manifests
   with reference traces, and the interrupt and STOP scenarios of the
   reference's rung 6 as manifests.
2. **A long real-code run.** The z80 had ZEXALL. The 68000 has no equivalent
   self-checking exerciser that runs on a bare host (MicroCoreLabs'
   `MC68000_test_all_opcodes` was tried first: it was written on Easy68K and
   fails on a real 68000's SR, whose bits 7-5 read as zero). What the
   reference does have are two MAME 0.285 lockstep recordings of real games
   (System 16B and Genesis Altered Beast, about 53 million instructions). The
   kit gains a `replay` host, a flat host whose device windows return the
   recorded reads in order, and the lockstep runner exports those recordings
   as replay manifests. Both cores then see the same machine whether or not
   it follows the game exactly; how closely it does is measured and stated.

## The ladder

| Rung | What | Claim it earns |
| --- | --- | --- |
| 1 | The example manifests of the reference's kit diff clean | the trace producer and host model are right |
| 2 | SingleStepTests/m68000 natively, 127 files, 317,500 cases: registers, RAM, prefetch, bus transactions with strobes and function codes, clocks, and `step_clocks` at every access end | instruction semantics, bus order and timing match the corpus |
| 3 | The replay manifests of the MAME recordings diffed in lockstep against the reference | equivalent over tens of millions of real-game instructions, interrupts included |
| 4 | flamewing's BCD tables natively, 525,312 inputs (T1, hardware-verified) | ABCD, SBCD, NBCD match a real 68000 |
| 5 | The interrupt and STOP scenarios as manifests with events | lifecycle sequencing, E-clock phases and acknowledge answers match |

The pins a claim needs: the m68000-python commit, the trace schema version,
the SingleStepTests/m68000 revision, and the BCD verifier revision.

## Constraints carried over

- Do not modify m68000-python's instruction core; a disagreement goes to the
  user with the evidence.
- Never commit ROMs, MAME traces or anything derived from a ROM.
- Shared machine: `nice`, at most 16 parallel processes.
- Commit locally; push and open pull requests only when the user asks.
