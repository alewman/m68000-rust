# Provenance

Audit of 2026-11-13 (provenance audit wave 1). This crate is a transcription
of m68000-python (MIT, same author) and inherits its verdicts: see
m68000-python's `docs/provenance.md`. Only this crate's own additions were
checked here. No GPL source was opened.

## Inherited

| Piece | Verdict (from m68000-python) |
| --- | --- |
| `src/alu.rs` `divide_unsigned_clocks`, `divide_signed_clocks` | **Rewritten 2026-11-19 (wave 2)**: the old bodies were moved aside unread (`trash/`, git-ignored) and the functions transcribed from m68000-python's rewrite (`7b11f4d`: a shift-and-subtract rule FITTED to SingleStepTests/m68000, MIT, with UM Table 8-4's maxima). **clean** (FITTED, DOC). |
| `src/system.rs` divide-by-zero flags | **Rewritten 2026-11-19** from m68000-python's rewrite, itself written from the recorded closed-box WinUAE run results (OBSERVED); the old body moved aside unread. **OBSERVED; accepted by the owner 2026-11-19** (a closed-box basis is acceptable where no permitted source defines it). |
| `src/bcd.rs` | cite: name Musashi (MIT) beside the BCD mechanism, as in m68000-python. |
| `src/core.rs`, `src/cpu.rs`, and the hidden-state steps in `ea.rs`, `loads.rs`, `bits.rs`, `bcd.rs`, `control.rs`, `alu.rs`, `flags.rs`, `system.rs` | **Re-sourced 2026-11-19 (wave 2, second pass)** with m68000-python: nothing rests on MAME's header-less microcode files any more. Each hidden-state step is pinned by SingleStepTests/m68000 (37 of 43, shown by the reference's `scripts/ablate_hidden_state.py`), or by WinUAE run as a closed box (2, OBSERVED), or is the boundary model (2, HYPOTHESIS); the 2 pinned by nothing (the MOVE.L -(An) and PEA decoder hand-overs in `loads.rs`) are removed. The E-clock rule cites MAME's `m68000.cpp` (BSD-3-Clause). **clean**; `cpu.rs` **cite** (NOTICE). |
| Everything else in `src/` | clean, as m68000-python. |

## Own additions

| Piece | Source | Verdict |
| --- | --- | --- |
| `src/bin/*` (vectors, BCD, 680x0, trace, bench runners) | own; `m68k-680x0` transcribes m68000-python's own harness and classifier | clean |
| `scripts/*.sh` | fetch SingleStepTests (MIT; 680x0 no licence stated, run only), m68000-python, and flamewing's verifier (GPL-3.0, built and run; only its table's SHA-256 is compared) | clean |
| `tests/data/*.json(l)` | m68000-python's conformance manifests (own) | clean |
| `tests/data/instruction-words.bin` | generated from m68000-python's disassembler at `38dbd0f` | clean |
| SPDX headers | `// SPDX-License-Identifier: MIT` on every `src/` and `tests/` file (2026-11-19) | clean |

Counts: clean 4 (own additions) + inherited (after wave 2: no rewrite, no unclear; cite: Musashi in `bcd.rs`, MAME `m68000.cpp` in `cpu.rs`).

## History

Public (alewman/m68000-rust). The divide clocks are in every commit since
`e00bb12` (2026-10-02, "Core transcribed"), on `main` and `board-hooks`
(both on origin). No tags; not on crates.io. No file was deleted from
history.

## Wave 2 (2026-11-19)

In place, following m68000-python `7b11f4d` (local, not yet pushed). Gates,
before and after:

| Gate | Before | After |
| --- | --- | --- |
| Divide clocks vs m68000-python's rewritten functions | -- | 0 of 633,847 inputs differ (random, small quotients, quotient -32768) |
| Rung 1 (kit manifests vs the reference) | identical | identical (4 manifests, against m68000-python `7b11f4d`) |
| Rung 2 (SingleStepTests/m68000) | 317,500 / 317,500 | 317,500 / 317,500 |
| Rung 3 (two MAME lockstep replays vs the reference) | identical | identical: System 16B altbeast 24,597,382 records, Genesis Altered Beast 28,253,448 records, against m68000-python `7b11f4d` on PyPy |
| Rung 4 (BCD tables) | 525,312 agree | 525,312 agree |
| Rung 5 (interrupt manifests) | 20 identical | 20 identical |
| Rung 6 (680x0 classification) | 787,660 / 1,000,060, = reference | identical table, = the reference's (m68000-python `7b11f4d`) |

`scripts/fetch_m68000python.sh` now pins `7b11f4d`, which exists only
locally until m68000-python is pushed.

Also inherited from m68000-python's wave 2 and **resolved** in its second
pass (below): what the core said it learned from MAME 0.285's microcode
rested partly on MAME files with no licence header (`m68000gen.py`,
`m68000.lst`, the generated handlers), which MAME's COPYING leaves under its
GPL.

## Wave 2, second pass (2026-11-19): MAME's header-less files

Transcribed from m68000-python `27c3792` (its second pass, `7e40bfa`, plus
the merge of its published `main`, whose only change is Python pickling):
the comments that cited the microcode now cite the corpus, the manuals, a
closed-box run or MAME's BSD-3-Clause `m68000.cpp`, as the reference's do,
and the two decoder hand-overs pinned by nothing are removed from
`loads.rs` (`move_long_to` -(An), `op_pea`). The only behaviour they could
show is the IR of a bus error on MOVE.L -(An)'s second write, outside the
contract; no rung reaches it. No MAME file was opened for this pass
(`m68000.cpp`/`.h`, BSD-3-Clause, were fetched by the reference's pass only
to check their header and the two names it cites, `vpa_sync` and `m_au`).

| Gate | Before (this crate's `7172473` vs m68000-python `f96150a`) | After (vs `27c3792`) |
| --- | --- | --- |
| Rung 1 (kit manifests) | identical | identical |
| Rung 2 (SingleStepTests/m68000, with `step_clocks`) | 317,500 / 317,500 | 317,500 / 317,500 |
| Rung 3 (MAME lockstep replays vs the reference, PyPy) | altbeast 24,597,382 and Genesis 28,253,448 records identical | identical (same counts) |
| Rung 4 (BCD tables) | 525,312 agree | 525,312 agree |
| Rung 5 (interrupt manifests) | 20 identical | 20 identical |
| Rung 6 (680x0 classification) | = reference | = reference, line for line (787,660 / 1,000,060) |
| clippy (`--all-targets`), `cargo fmt --check` | clean | clean |

`scripts/fetch_m68000python.sh` pins `27c3792` (local until m68000-python's
history rewrite and push: the rewrite gives this tree a new commit, and the
repin follows it).
