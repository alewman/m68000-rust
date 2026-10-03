//! Boundary traces: produce, write, read, and compare them.
//!
//! The trace schema is m68000-python's `docs/trace-schema.md`, version 1:
//! JSON Lines, one record per processor boundary, with the kind, the clocks,
//! the instruction's address and bytes, the complete state before and
//! after, and the bus accesses in order. This core writes no `mnemonic` or
//! `operands` (the reference's reader decodes the bytes with its own
//! disassembler, so a port never reproduces its formatting); it reads them
//! when present and ignores them.
//!
//! The instruction's bytes are read from host memory at `before.pc` before
//! the step, as the reference's `DebugSession` disassembles them, so the
//! length must agree with the reference's disassembler: [`instruction_words`]
//! is that disassembler's extension-word count, rule for rule.

use std::fmt::Write as _;

use serde_json::Value;

use crate::dispatch::{names, Op};
use crate::state::CpuState;

pub const TRACE_SCHEMA_VERSION: u64 = 1;

/// What one `step()` did (the reference's `BoundaryKind`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BoundaryKind {
    Instruction,
    /// The trace exception after a traced instruction.
    Trace,
    /// An interrupt accepted.
    Interrupt,
    /// Clocks spent inside STOP.
    StoppedIdle,
    /// Clocks spent halted by a double bus fault.
    HaltedIdle,
}

impl BoundaryKind {
    pub fn as_str(self) -> &'static str {
        match self {
            BoundaryKind::Instruction => "instruction",
            BoundaryKind::Trace => "trace",
            BoundaryKind::Interrupt => "interrupt",
            BoundaryKind::StoppedIdle => "stopped_idle",
            BoundaryKind::HaltedIdle => "halted_idle",
        }
    }

    fn parse(text: &str) -> Result<Self, String> {
        Ok(match text {
            "instruction" => BoundaryKind::Instruction,
            "trace" => BoundaryKind::Trace,
            "interrupt" => BoundaryKind::Interrupt,
            "stopped_idle" => BoundaryKind::StoppedIdle,
            "halted_idle" => BoundaryKind::HaltedIdle,
            other => return Err(format!("unknown kind {other:?}")),
        })
    }
}

/// What the next `step()` will do, decided exactly as `M68000::step()` decides it.
pub fn next_boundary(state: &CpuState) -> BoundaryKind {
    if state.halted {
        return BoundaryKind::HaltedIdle;
    }
    if state.trace_pending {
        return BoundaryKind::Trace;
    }
    if state.ipl != 0
        && (u16::from(state.ipl) > (state.sr >> 8) & 7 || (state.ipl == 7 && state.nmi_edge))
    {
        return BoundaryKind::Interrupt;
    }
    if state.stopped {
        return BoundaryKind::StoppedIdle;
    }
    BoundaryKind::Instruction
}

/// One bus access: write or read, 24-bit address, value, size in bytes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Access {
    pub write: bool,
    pub address: u32,
    pub value: u32,
    pub size: u8,
}

/// The instruction at an instruction boundary: its address and every byte it occupies.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Instruction {
    pub address: u32,
    pub data: Vec<u8>,
}

/// Immutable before/after evidence for one boundary.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StepRecord {
    pub sequence: u64,
    pub kind: BoundaryKind,
    pub before: CpuState,
    pub after: CpuState,
    pub cycles: u32,
    pub instruction: Option<Instruction>,
    /// Every bus access the step made, in order, when recorded.
    pub accesses: Option<Vec<Access>>,
}

/// Extension words an EA field takes, as the reference's disassembler reads them.
///
/// `size` is 0/1/2 for a byte/word/long immediate.
fn ea_words(field: u16, size: u16) -> usize {
    let (mode, register) = (field >> 3, field & 7);
    match mode {
        0..=4 => 0,
        5 | 6 => 1,
        _ => match register {
            0 | 2 | 3 => 1,
            1 => 2,
            4 => {
                if size == 2 {
                    2
                } else {
                    1
                }
            }
            _ => 0, // the reference prints "?" and reads nothing
        },
    }
}

/// How many words the instruction whose first word is `opcode` occupies.
///
/// The reference's `disasm._decode`, reduced to the words it reads.
pub fn instruction_words(opcode: u16) -> usize {
    let size_bits = (opcode >> 6) & 3;
    let ea = opcode & 0x3F;
    let rx = (opcode >> 9) & 7;
    let extension = match names()[usize::from(opcode)] {
        Op::subi | Op::addi | Op::cmpi => {
            (if size_bits == 2 { 2 } else { 1 }) + ea_words(ea, size_bits)
        }
        Op::ori | Op::andi | Op::eori => ea_words(0x3C, size_bits) + ea_words(ea, size_bits),
        Op::ori_to_ccr | Op::andi_to_ccr | Op::eori_to_ccr => ea_words(0x3C, 0),
        Op::ori_to_sr | Op::andi_to_sr | Op::eori_to_sr => ea_words(0x3C, 1),
        Op::btst | Op::bchg | Op::bclr | Op::bset => {
            (if opcode & 0x0100 != 0 { 0 } else { 1 }) + ea_words(ea, 0)
        }
        Op::movep | Op::link | Op::stop | Op::dbcc => 1,
        Op::r#move => {
            let size = match opcode >> 12 {
                1 => 0,
                3 => 1,
                _ => 2,
            };
            ea_words(ea, size) + ea_words(((opcode >> 3) & 0x38) | rx, 0)
        }
        Op::movea => ea_words(ea, if opcode >> 12 == 3 { 1 } else { 2 }),
        Op::move_from_sr | Op::move_to_ccr | Op::move_to_sr => ea_words(ea, 1),
        Op::negx | Op::clr | Op::neg | Op::not | Op::tst => ea_words(ea, size_bits),
        Op::nbcd | Op::tas | Op::pea | Op::jsr | Op::jmp => ea_words(ea, 2),
        Op::movem => 1 + ea_words(ea, 1),
        Op::chk => ea_words(ea, 1),
        Op::lea => ea_words(ea, 2),
        Op::addq | Op::subq => ea_words(ea, size_bits),
        Op::scc => ea_words(ea, 0),
        Op::bra | Op::bsr | Op::bcc => usize::from(opcode & 0xFF == 0),
        Op::divu | Op::divs | Op::mulu | Op::muls => ea_words(ea, 1),
        Op::or | Op::and | Op::sub | Op::add | Op::eor | Op::cmp => ea_words(ea, size_bits),
        Op::suba | Op::adda | Op::cmpa => ea_words(ea, if opcode & 0x0100 != 0 { 2 } else { 1 }),
        Op::asd_memory | Op::lsd_memory | Op::roxd_memory | Op::rod_memory => ea_words(ea, 1),
        _ => 0,
    };
    1 + extension
}

/// The instruction at `address`, its words read with `peek_word` (side-effect free).
pub fn instruction_at(address: u32, mut peek_word: impl FnMut(u32) -> u16) -> Instruction {
    let opcode = peek_word(address & 0xFF_FFFF);
    let mut data = opcode.to_be_bytes().to_vec();
    for index in 1..instruction_words(opcode) {
        let word = peek_word(address.wrapping_add(2 * index as u32) & 0xFF_FFFF);
        data.extend_from_slice(&word.to_be_bytes());
    }
    Instruction { address, data }
}

// -- JSON --------------------------------------------------------------------

/// A state object as JSON, keys sorted.
pub fn write_state(out: &mut String, state: &CpuState) {
    // Sorted keys, no whitespace: the reference's json.dumps(sort_keys=True).
    let join = |values: &[u32]| {
        values
            .iter()
            .map(u32::to_string)
            .collect::<Vec<_>>()
            .join(",")
    };
    write!(
        out,
        "{{\"a\":[{}],\"clock\":{},\"d\":[{}],\"halted\":{},\"ipl\":{},\"ir\":{},\"irc\":{},\
         \"nmi_edge\":{},\"pc\":{},\"sr\":{},\"ssp\":{},\"stopped\":{},\"trace_pending\":{},\
         \"usp\":{}}}",
        join(&state.a),
        state.clock,
        join(&state.d),
        state.halted,
        state.ipl,
        state.ir,
        state.irc,
        state.nmi_edge,
        state.pc,
        state.sr,
        state.ssp,
        state.stopped,
        state.trace_pending,
        state.usp
    )
    .unwrap();
}

/// One record as a JSON line (with its newline), keys sorted as the reference sorts them.
pub fn record_to_json(record: &StepRecord) -> String {
    let mut out = String::with_capacity(512);
    out.push('{');
    if let Some(accesses) = &record.accesses {
        out.push_str("\"accesses\":[");
        for (index, access) in accesses.iter().enumerate() {
            if index > 0 {
                out.push(',');
            }
            let kind = if access.write { "w" } else { "r" };
            write!(
                out,
                "[\"{kind}\",{},{},{}]",
                access.address, access.value, access.size
            )
            .unwrap();
        }
        out.push_str("],");
    }
    out.push_str("\"after\":");
    write_state(&mut out, &record.after);
    out.push_str(",\"before\":");
    write_state(&mut out, &record.before);
    write!(out, ",\"cycles\":{},\"instruction\":", record.cycles).unwrap();
    match &record.instruction {
        None => out.push_str("null"),
        Some(instruction) => {
            write!(out, "{{\"address\":{},\"data\":\"", instruction.address).unwrap();
            for byte in &instruction.data {
                write!(out, "{byte:02x}").unwrap();
            }
            out.push_str("\"}");
        }
    }
    write!(
        out,
        ",\"kind\":\"{}\",\"sequence\":{},\"version\":{}}}\n",
        record.kind.as_str(),
        record.sequence,
        TRACE_SCHEMA_VERSION
    )
    .unwrap();
    out
}

fn uint(value: &Value, name: &str, maximum: u64) -> Result<u64, String> {
    match value.as_u64() {
        Some(number) if number <= maximum => Ok(number),
        _ => Err(format!(
            "{name} must be an integer in range 0..{maximum:#x}"
        )),
    }
}

fn boolean(value: &Value, name: &str) -> Result<bool, String> {
    value
        .as_bool()
        .ok_or_else(|| format!("{name} must be a bool"))
}

/// A state object, every key required and no others (the reference's `_state_from_dict`).
pub fn state_from_json(value: &Value) -> Result<CpuState, String> {
    let object = value
        .as_object()
        .ok_or("a state object must be an object")?;
    let expected = crate::state::STATE_FIELDS;
    if object.len() != expected.len() || !expected.iter().all(|key| object.contains_key(*key)) {
        return Err(format!(
            "a state object must have exactly the keys {expected:?}"
        ));
    }
    let registers = |name: &str, count: usize| -> Result<Vec<u32>, String> {
        let list = object[name]
            .as_array()
            .filter(|list| list.len() == count)
            .ok_or_else(|| format!("{name} must hold {count} values"))?;
        list.iter()
            .enumerate()
            .map(|(index, item)| {
                uint(item, &format!("{name}{index}"), 0xFFFF_FFFF).map(|v| v as u32)
            })
            .collect()
    };
    let state = CpuState {
        d: registers("d", 8)?.try_into().unwrap(),
        a: registers("a", 7)?.try_into().unwrap(),
        usp: uint(&object["usp"], "usp", 0xFFFF_FFFF)? as u32,
        ssp: uint(&object["ssp"], "ssp", 0xFFFF_FFFF)? as u32,
        sr: uint(&object["sr"], "sr", 0xFFFF)? as u16,
        pc: uint(&object["pc"], "pc", 0xFFFF_FFFF)? as u32,
        ir: uint(&object["ir"], "ir", 0xFFFF)? as u16,
        irc: uint(&object["irc"], "irc", 0xFFFF)? as u16,
        ipl: uint(&object["ipl"], "ipl", 7)? as u8,
        nmi_edge: boolean(&object["nmi_edge"], "nmi_edge")?,
        trace_pending: boolean(&object["trace_pending"], "trace_pending")?,
        stopped: boolean(&object["stopped"], "stopped")?,
        halted: boolean(&object["halted"], "halted")?,
        clock: uint(&object["clock"], "clock", u64::MAX)?,
    };
    state.validate()?;
    Ok(state)
}

/// Validate one decoded JSON record and rebuild it.
pub fn record_from_json(value: &Value) -> Result<StepRecord, String> {
    let object = value
        .as_object()
        .ok_or("a trace record must be a JSON object")?;
    const REQUIRED: [&str; 7] = [
        "version",
        "sequence",
        "kind",
        "cycles",
        "instruction",
        "before",
        "after",
    ];
    if !REQUIRED.iter().all(|key| object.contains_key(*key))
        || object
            .keys()
            .any(|key| !REQUIRED.contains(&key.as_str()) && key != "accesses")
    {
        return Err(format!(
            "trace record keys must be {REQUIRED:?} (+ accesses)"
        ));
    }
    if object["version"].as_u64() != Some(TRACE_SCHEMA_VERSION) {
        return Err(format!("unsupported trace version {}", object["version"]));
    }
    let instruction = match &object["instruction"] {
        Value::Null => None,
        Value::Object(spec) => {
            let address = spec
                .get("address")
                .and_then(Value::as_u64)
                .ok_or("instruction address must be an integer")?;
            let text = spec
                .get("data")
                .and_then(Value::as_str)
                .ok_or("instruction data must be a hex string")?;
            if text.len() % 2 != 0 {
                return Err("instruction data must be hexadecimal".into());
            }
            let data = (0..text.len())
                .step_by(2)
                .map(|i| u8::from_str_radix(&text[i..i + 2], 16))
                .collect::<Result<Vec<u8>, _>>()
                .map_err(|_| "instruction data must be hexadecimal")?;
            if data.len() < 2
                || data.len() != 2 * instruction_words(u16::from_be_bytes([data[0], data[1]]))
            {
                return Err("instruction data must hold exactly one instruction".into());
            }
            Some(Instruction {
                address: address as u32,
                data,
            })
        }
        _ => return Err("instruction must be an object or null".into()),
    };
    let accesses = match object.get("accesses") {
        None => None,
        Some(Value::Array(list)) => {
            let mut out = Vec::with_capacity(list.len());
            for item in list {
                let parts = item.as_array().filter(|parts| parts.len() == 4);
                let parts = parts.ok_or("an access is [kind, address, value, size]")?;
                let write = match parts[0].as_str() {
                    Some("r") => false,
                    Some("w") => true,
                    _ => return Err("an access kind is \"r\" or \"w\"".into()),
                };
                out.push(Access {
                    write,
                    address: uint(&parts[1], "access address", 0xFF_FFFF)? as u32,
                    value: uint(&parts[2], "access value", 0xFFFF)? as u32,
                    size: uint(&parts[3], "access size", 2)? as u8,
                });
            }
            Some(out)
        }
        Some(_) => return Err("accesses must be a list".into()),
    };
    Ok(StepRecord {
        sequence: object["sequence"]
            .as_u64()
            .ok_or("sequence must be an integer")?,
        kind: BoundaryKind::parse(object["kind"].as_str().ok_or("kind must be a string")?)?,
        before: state_from_json(&object["before"])?,
        after: state_from_json(&object["after"])?,
        cycles: uint(&object["cycles"], "cycles", u64::from(u32::MAX))? as u32,
        instruction,
        accesses,
    })
}

/// Read one JSON line; `Ok(None)` for a blank line.
pub fn parse_line(line: &str) -> Result<Option<StepRecord>, String> {
    if line.trim().is_empty() {
        return Ok(None);
    }
    let value: Value = serde_json::from_str(line).map_err(|e| e.to_string())?;
    record_from_json(&value).map(Some)
}

// -- comparison ---------------------------------------------------------------

/// One unequal observation at an aligned position: the path and both values as text.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TraceDifference {
    pub path: String,
    pub left: String,
    pub right: String,
}

fn state_fields(state: &CpuState) -> [(&'static str, String); 14] {
    [
        ("d", format!("{:?}", state.d)),
        ("a", format!("{:?}", state.a)),
        ("usp", state.usp.to_string()),
        ("ssp", state.ssp.to_string()),
        ("sr", state.sr.to_string()),
        ("pc", state.pc.to_string()),
        ("ir", state.ir.to_string()),
        ("irc", state.irc.to_string()),
        ("ipl", state.ipl.to_string()),
        ("nmi_edge", state.nmi_edge.to_string()),
        ("trace_pending", state.trace_pending.to_string()),
        ("stopped", state.stopped.to_string()),
        ("halted", state.halted.to_string()),
        ("clock", state.clock.to_string()),
    ]
}

/// Every differing field of two aligned records (sequence numbers are ignored).
///
/// Bus accesses are compared only when both records carry them, as the
/// reference's `compare_step_records` does.
pub fn compare_step_records(left: &StepRecord, right: &StepRecord) -> Vec<TraceDifference> {
    let mut out = Vec::new();
    let mut push = |path: String, a: String, b: String| {
        if a != b {
            out.push(TraceDifference {
                path,
                left: a,
                right: b,
            });
        }
    };
    push(
        "kind".into(),
        left.kind.as_str().into(),
        right.kind.as_str().into(),
    );
    push(
        "cycles".into(),
        left.cycles.to_string(),
        right.cycles.to_string(),
    );
    match (&left.instruction, &right.instruction) {
        (Some(a), Some(b)) => {
            push(
                "instruction.address".into(),
                a.address.to_string(),
                b.address.to_string(),
            );
            push("instruction.data".into(), hex(&a.data), hex(&b.data));
        }
        (None, None) => {}
        (a, b) => push(
            "instruction".into(),
            a.as_ref().map_or("None".into(), |i| hex(&i.data)),
            b.as_ref().map_or("None".into(), |i| hex(&i.data)),
        ),
    }
    for (side, a, b) in [
        ("before", &left.before, &right.before),
        ("after", &left.after, &right.after),
    ] {
        for ((name, x), (_, y)) in state_fields(a).into_iter().zip(state_fields(b)) {
            push(format!("{side}.{name}"), x, y);
        }
    }
    if let (Some(a), Some(b)) = (&left.accesses, &right.accesses) {
        push("accesses".into(), format!("{a:?}"), format!("{b:?}"));
    }
    out
}

fn hex(data: &[u8]) -> String {
    data.iter().map(|byte| format!("{byte:02x}")).collect()
}
