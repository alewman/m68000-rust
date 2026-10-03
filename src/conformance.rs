//! The conformance host and manifest runner: the machine both cores must see.
//!
//! m68000-python's `docs/conformance.md` defines a manifest (memory, the
//! initial state or a reset, how the host answers acknowledge cycles, bus
//! errors, TAS writes and device reads, when the interrupt level changes,
//! and when to stop) and the one host every run uses, so that two cores
//! given the same manifest see the same machine and any difference between
//! their traces is a difference between the CPUs. This module implements
//! that text; [`trace_manifest`] produces the trace the reference's
//! `python -m m68000_python.conformance diff` compares against its own.

use std::path::{Path, PathBuf};

use serde_json::{Map, Value};

use crate::core::{Bus, BusError, Cycle, AUTOVECTOR, M68000, SPURIOUS};
use crate::state::CpuState;
use crate::trace::{instruction_at, next_boundary, Access, BoundaryKind, StepRecord};

pub const MANIFEST_SCHEMA_VERSION: u64 = 1;
const MEMORY_SIZE: usize = 1 << 24;

/// A lifecycle change applied immediately before boundary `at_step`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EventKind {
    /// `set_ipl(level)`.
    Ipl(u8),
    /// `reset()`: no record; its clocks land in `clock`.
    Reset,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Event {
    pub at_step: u64,
    pub kind: EventKind,
}

/// How the run starts.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Start {
    /// `reset()` after memory is loaded.
    Reset,
    /// `restore_state` of this state; `fill_queue` reads ir and irc from
    /// memory at pc and pc + 2 (the manifest gave neither).
    State { state: CpuState, fill_queue: bool },
}

/// A complete, deterministic description of one conformance run.
#[derive(Clone, Debug)]
pub struct Manifest {
    pub name: String,
    pub replay: bool,
    pub memory: Vec<(u32, Vec<u8>)>,
    pub start: Start,
    /// Per level 1-7 (index 0 unused): a vector, `AUTOVECTOR` or `SPURIOUS`.
    pub acknowledge: [i32; 8],
    pub bus_error: Vec<(u32, u32)>,
    pub tas_drop: bool,
    pub devices: Vec<(u32, u32)>,
    pub reads: Vec<u16>,
    pub events: Vec<Event>,
    pub max_steps: u64,
    pub on_idle: bool,
    pub at_pc: Vec<u32>,
}

fn object<'a>(value: &'a Value, name: &str) -> Result<&'a Map<String, Value>, String> {
    value
        .as_object()
        .ok_or_else(|| format!("{name} must be an object with string keys"))
}

fn list<'a>(value: &'a Value, name: &str) -> Result<&'a Vec<Value>, String> {
    value
        .as_array()
        .ok_or_else(|| format!("{name} must be a list"))
}

fn allowed(
    value: &Map<String, Value>,
    name: &str,
    keys: &[&str],
    required: &[&str],
) -> Result<(), String> {
    let unknown: Vec<&String> = value
        .keys()
        .filter(|k| !keys.contains(&k.as_str()))
        .collect();
    let missing: Vec<&&str> = required
        .iter()
        .filter(|k| !value.contains_key(**k))
        .collect();
    if unknown.is_empty() && missing.is_empty() {
        return Ok(());
    }
    Err(format!(
        "{name} fields do not match schema (missing={missing:?}, unknown={unknown:?})"
    ))
}

fn uint(value: &Value, name: &str, maximum: u64) -> Result<u64, String> {
    match value.as_u64() {
        Some(number) if number <= maximum => Ok(number),
        _ => Err(format!(
            "{name} must be an integer in range 0..{maximum:#x}"
        )),
    }
}

fn file_bytes(item: &Map<String, Value>, name: &str, base: &Path) -> Result<Vec<u8>, String> {
    let file = item["file"]
        .as_str()
        .ok_or_else(|| format!("{name}.file must be a path string"))?;
    let path: PathBuf = if Path::new(file).is_absolute() {
        PathBuf::from(file)
    } else {
        base.join(file)
    };
    std::fs::read(&path).map_err(|e| format!("{name}: {}: {e}", path.display()))
}

fn range(item: &Value, name: &str) -> Result<(u32, u32), String> {
    let item = object(item, name)?;
    allowed(item, name, &["address", "length"], &["address", "length"])?;
    let address = uint(&item["address"], &format!("{name}.address"), 0xFF_FFFF)? as u32;
    let length = uint(&item["length"], &format!("{name}.length"), 1 << 24)? as u32;
    Ok((address, length))
}

/// Build a validated manifest from its JSON form; `file` paths resolve against `base`.
pub fn manifest_from_json(value: &Value, base: &Path) -> Result<Manifest, String> {
    let root = object(value, "manifest")?;
    allowed(
        root,
        "manifest",
        &[
            "version",
            "name",
            "host",
            "memory",
            "reset",
            "initial",
            "acknowledge",
            "bus_error",
            "tas_write",
            "replay",
            "events",
            "stop",
        ],
        &["version", "name", "memory", "stop"],
    )?;
    if root["version"].as_u64() != Some(MANIFEST_SCHEMA_VERSION) {
        return Err(format!("unsupported manifest version: {}", root["version"]));
    }
    let name = root["name"]
        .as_str()
        .filter(|name| !name.is_empty())
        .ok_or("manifest name must be a non-empty string")?
        .to_string();
    let replay = match root.get("host").map(Value::as_str) {
        None | Some(Some("flat")) => false,
        Some(Some("replay")) => true,
        _ => return Err("manifest host must be one of (\"flat\", \"replay\")".into()),
    };
    let mut memory = Vec::new();
    for (index, item) in list(&root["memory"], "memory")?.iter().enumerate() {
        let label = format!("memory[{index}]");
        let item = object(item, &label)?;
        allowed(
            item,
            &label,
            &["address", "data", "file", "offset", "length"],
            &["address"],
        )?;
        if item.contains_key("data") == item.contains_key("file") {
            return Err(format!("{label} must have exactly one of 'data' or 'file'"));
        }
        let address = uint(&item["address"], &format!("{label}.address"), 0xFF_FFFF)? as u32;
        let data = if let Some(text) = item.get("data") {
            let text = text
                .as_str()
                .filter(|t| t.len() % 2 == 0)
                .ok_or_else(|| format!("{label}.data must be a hexadecimal string"))?;
            (0..text.len())
                .step_by(2)
                .map(|i| u8::from_str_radix(&text[i..i + 2], 16))
                .collect::<Result<Vec<u8>, _>>()
                .map_err(|_| format!("{label}.data must be a hexadecimal string"))?
        } else {
            let data = file_bytes(item, &label, base)?;
            let offset = item
                .get("offset")
                .map_or(Ok(0), |v| uint(v, "offset", u64::MAX))?;
            let length = match item.get("length") {
                Some(v) => uint(v, "length", u64::MAX)?,
                None => (data.len() as u64).saturating_sub(offset),
            };
            let (offset, length) = (offset as usize, length as usize);
            if length == 0 || offset + length > data.len() {
                return Err(format!("{label}.offset/length out of the file"));
            }
            data[offset..offset + length].to_vec()
        };
        if data.is_empty() || address as usize + data.len() > MEMORY_SIZE {
            return Err(format!("{label}: segment does not fit below 0x1000000"));
        }
        memory.push((address, data));
    }
    let reset = match root.get("reset") {
        None => false,
        Some(value) => value.as_bool().ok_or("reset must be a bool")?,
    };
    let start = if reset {
        if root.contains_key("initial") {
            return Err("reset and initial are mutually exclusive".into());
        }
        Start::Reset
    } else {
        let empty = Map::new();
        let initial = match root.get("initial") {
            Some(value) => object(value, "initial")?,
            None => &empty,
        };
        initial_state(initial)?
    };
    let mut acknowledge = [AUTOVECTOR; 8];
    if let Some(value) = root.get("acknowledge") {
        for (key, answer) in object(value, "acknowledge")? {
            let level: usize = key
                .parse()
                .ok()
                .filter(|level| (1..=7).contains(level))
                .ok_or_else(|| format!("acknowledge level {key:?} must be 1-7"))?;
            acknowledge[level] = match answer {
                Value::String(text) if text == "autovector" => AUTOVECTOR,
                Value::String(text) if text == "spurious" => SPURIOUS,
                other => uint(other, &format!("acknowledge[{key}]"), 255)? as i32,
            };
        }
    }
    let mut bus_error = Vec::new();
    if let Some(value) = root.get("bus_error") {
        for (index, item) in list(value, "bus_error")?.iter().enumerate() {
            bus_error.push(range(item, &format!("bus_error[{index}]"))?);
        }
    }
    let tas_drop = match root.get("tas_write").map(Value::as_str) {
        None | Some(Some("write")) => false,
        Some(Some("drop")) => true,
        _ => return Err("tas_write must be \"write\" or \"drop\"".into()),
    };
    let (mut devices, mut reads) = (Vec::new(), Vec::new());
    match (replay, root.get("replay")) {
        (true, Some(value)) => {
            let spec = object(value, "replay")?;
            allowed(spec, "replay", &["devices", "reads"], &["devices", "reads"])?;
            for (index, item) in list(&spec["devices"], "replay.devices")?.iter().enumerate() {
                devices.push(range(item, &format!("replay.devices[{index}]"))?);
            }
            let source = object(&spec["reads"], "replay.reads")?;
            allowed(source, "replay.reads", &["file", "data"], &[])?;
            if source.contains_key("file") == source.contains_key("data") {
                return Err("replay.reads must have exactly one of 'data' or 'file'".into());
            }
            let bytes = match source.get("data") {
                Some(text) => {
                    let text = text
                        .as_str()
                        .filter(|t| t.len() % 2 == 0)
                        .ok_or("replay.reads.data must be a hexadecimal string")?;
                    (0..text.len())
                        .step_by(2)
                        .map(|i| u8::from_str_radix(&text[i..i + 2], 16))
                        .collect::<Result<Vec<u8>, _>>()
                        .map_err(|_| "replay.reads.data must be a hexadecimal string")?
                }
                None => file_bytes(source, "replay.reads", base)?,
            };
            if bytes.len() % 2 != 0 {
                return Err("replay.reads must hold whole 16-bit values".into());
            }
            reads = bytes
                .chunks(2)
                .map(|pair| u16::from_be_bytes([pair[0], pair[1]]))
                .collect();
        }
        (true, None) => return Err("the replay host needs a replay object".into()),
        (false, Some(_)) => return Err("replay belongs only to the replay host".into()),
        (false, None) => {}
    }
    let mut events = Vec::new();
    if let Some(value) = root.get("events") {
        for (index, item) in list(value, "events")?.iter().enumerate() {
            let label = format!("events[{index}]");
            let item = object(item, &label)?;
            allowed(
                item,
                &label,
                &["at_step", "kind", "level"],
                &["at_step", "kind"],
            )?;
            let at_step = uint(&item["at_step"], &format!("{label}.at_step"), u64::MAX)?;
            let kind = match item["kind"].as_str() {
                Some("ipl") => {
                    let level = item
                        .get("level")
                        .ok_or(format!("{label}: ipl needs a level"))?;
                    EventKind::Ipl(uint(level, &format!("{label}.level"), 7)? as u8)
                }
                Some("reset") if !item.contains_key("level") => EventKind::Reset,
                _ => return Err(format!("{label}: kind must be \"ipl\" or \"reset\"")),
            };
            events.push(Event { at_step, kind });
        }
    }
    if events
        .windows(2)
        .any(|pair| pair[0].at_step > pair[1].at_step)
    {
        return Err("manifest events must be ordered by at_step".into());
    }
    let stop = object(&root["stop"], "stop")?;
    allowed(
        stop,
        "stop",
        &["max_steps", "on_idle", "at_pc"],
        &["max_steps"],
    )?;
    let max_steps = uint(&stop["max_steps"], "stop.max_steps", u64::MAX)?;
    if max_steps == 0 {
        return Err("stop.max_steps must be a positive integer".into());
    }
    let on_idle = match stop.get("on_idle") {
        None => true,
        Some(value) => value.as_bool().ok_or("stop.on_idle must be a bool")?,
    };
    let mut at_pc = Vec::new();
    if let Some(value) = stop.get("at_pc") {
        for item in list(value, "stop.at_pc")? {
            at_pc.push(uint(item, "stop.at_pc", 0xFFFF_FFFF)? as u32);
        }
    }
    Ok(Manifest {
        name,
        replay,
        memory,
        start,
        acknowledge,
        bus_error,
        tas_drop,
        devices,
        reads,
        events,
        max_steps,
        on_idle,
        at_pc,
    })
}

/// `CpuState(**initial)`: any subset of the fields, the rest at their defaults.
fn initial_state(initial: &Map<String, Value>) -> Result<Start, String> {
    let fields = crate::state::STATE_FIELDS;
    allowed(initial, "initial", &fields, &[])?;
    let has_ir = initial.contains_key("ir");
    if has_ir != initial.contains_key("irc") {
        return Err("initial gives ir without irc, or irc without ir".into());
    }
    let mut full = Map::new();
    let mut text = String::new();
    crate::trace::write_state(&mut text, &CpuState::default());
    let default_json: Value = serde_json::from_str(&text).unwrap();
    for field in fields {
        let value = initial
            .get(field)
            .cloned()
            .unwrap_or_else(|| default_json[field].clone());
        full.insert(field.to_string(), value);
    }
    let state = crate::trace::state_from_json(&Value::Object(full))
        .map_err(|e| format!("invalid initial state: {e}"))?;
    Ok(Start::State {
        state,
        fill_queue: !has_ir,
    })
}

/// Read and validate a manifest file; `file` segments resolve beside it.
pub fn load_manifest(path: &Path) -> Result<Manifest, String> {
    let text = std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let value: Value = serde_json::from_str(&text)
        .map_err(|e| format!("{}: not valid JSON: {e}", path.display()))?;
    manifest_from_json(&value, path.parent().unwrap_or(Path::new(".")))
}

/// The one host every conformance run uses, so hosts cannot differ between cores.
pub struct ConformanceHost {
    pub memory: Vec<u8>,
    devices: Vec<(u32, u32)>,
    reads: Vec<u16>,
    next_read: usize,
    bus_error: Vec<(u32, u32)>,
    acknowledge: [i32; 8],
    tas_drop: bool,
    /// The accesses of the step in progress, when the run records them.
    pub accesses: Option<Vec<Access>>,
}

fn inside(ranges: &[(u32, u32)], address: u32) -> bool {
    ranges
        .iter()
        .any(|&(start, length)| address >= start && address - start < length)
}

impl ConformanceHost {
    pub fn new(manifest: &Manifest) -> Self {
        let mut memory = vec![0u8; MEMORY_SIZE];
        for (address, data) in &manifest.memory {
            let start = *address as usize;
            memory[start..start + data.len()].copy_from_slice(data);
        }
        ConformanceHost {
            memory,
            devices: manifest.devices.clone(),
            reads: manifest.reads.clone(),
            next_read: 0,
            bus_error: manifest.bus_error.clone(),
            acknowledge: manifest.acknowledge,
            tas_drop: manifest.tas_drop,
            accesses: None,
        }
    }

    /// Side-effect-free read for the instruction bytes (memory even in a device window).
    pub fn peek_word(&self, address: u32) -> u16 {
        let address = (address & 0xFF_FFFF) as usize;
        u16::from_be_bytes([self.memory[address], self.memory[(address + 1) & 0xFF_FFFF]])
    }

    fn replayed(&mut self) -> u16 {
        let value = self.reads.get(self.next_read).copied().unwrap_or(0xFFFF);
        self.next_read += 1;
        value
    }

    fn note(&mut self, write: bool, address: u32, value: u32, size: u8) {
        if let Some(log) = &mut self.accesses {
            log.push(Access {
                write,
                address,
                value,
                size,
            });
        }
    }
}

impl Bus for ConformanceHost {
    fn read_byte(&mut self, address: u32, _cycle: Cycle) -> Result<u8, BusError> {
        if inside(&self.bus_error, address) {
            return Err(BusError);
        }
        let value = if inside(&self.devices, address) {
            self.replayed() as u8
        } else {
            self.memory[address as usize]
        };
        // The reference's tracked reads log after the read returns.
        self.note(false, address, u32::from(value), 1);
        Ok(value)
    }

    fn read_word(&mut self, address: u32, _cycle: Cycle) -> Result<u16, BusError> {
        if inside(&self.bus_error, address) {
            return Err(BusError);
        }
        let value = if inside(&self.devices, address) {
            self.replayed()
        } else {
            self.peek_word(address)
        };
        self.note(false, address, u32::from(value), 2);
        Ok(value)
    }

    fn write_byte(&mut self, address: u32, value: u8, _cycle: Cycle) -> Result<(), BusError> {
        // The reference's tracked writes log before the host's write runs.
        self.note(true, address, u32::from(value), 1);
        if inside(&self.bus_error, address) {
            return Err(BusError);
        }
        if !inside(&self.devices, address) {
            self.memory[address as usize] = value;
        }
        Ok(())
    }

    fn write_word(&mut self, address: u32, value: u16, _cycle: Cycle) -> Result<(), BusError> {
        self.note(true, address, u32::from(value), 2);
        if inside(&self.bus_error, address) {
            return Err(BusError);
        }
        if !inside(&self.devices, address) {
            let [high, low] = value.to_be_bytes();
            self.memory[address as usize] = high;
            self.memory[(address as usize + 1) & 0xFF_FFFF] = low;
        }
        Ok(())
    }

    fn acknowledge(&mut self, level: u8, _cycle: Cycle) -> i32 {
        self.acknowledge[usize::from(level)]
    }

    fn tas_write(&mut self, address: u32, value: u8, cycle: Cycle) -> Result<(), BusError> {
        if !self.tas_drop {
            return self.write_byte(address, value, cycle);
        }
        // The Genesis drops it: it never reaches the bus, so nothing is recorded.
        if inside(&self.bus_error, address) {
            return Err(BusError);
        }
        Ok(())
    }
}

/// Why a run ended, and what it cost.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TraceRun {
    pub steps: u64,
    pub clocks: u64,
    pub reason: &'static str,
}

/// Build the CPU a manifest describes, ready for its first boundary.
pub fn start(manifest: &Manifest) -> M68000<ConformanceHost> {
    let mut cpu = M68000::new(ConformanceHost::new(manifest));
    match &manifest.start {
        Start::Reset => {
            cpu.reset();
        }
        Start::State { state, fill_queue } => {
            let mut state = state.clone();
            if *fill_queue {
                state.ir = cpu.bus.peek_word(state.pc);
                state.irc = cpu.bus.peek_word(state.pc.wrapping_add(2));
            }
            cpu.restore_state(&state);
        }
    }
    cpu
}

/// Run `manifest`, giving `emit` one record per boundary (`record = false`
/// steps bare, with no records and no access log).
///
/// Before each boundary, in this order: the events due at it are applied;
/// the run stops if PC is in `at_pc`; with `on_idle` it stops if the next
/// boundary would be a halted or stopped idle and no event remains; and it
/// stops when the step budget is spent.
pub fn trace_manifest(
    manifest: &Manifest,
    record: bool,
    mut emit: impl FnMut(StepRecord),
) -> TraceRun {
    let mut cpu = start(manifest);
    let mut events = manifest.events.iter().peekable();
    let (mut steps, mut clocks) = (0u64, 0u64);
    let reason = loop {
        if steps >= manifest.max_steps {
            break "max_steps";
        }
        while let Some(event) = events.next_if(|event| event.at_step == steps) {
            match event.kind {
                EventKind::Ipl(level) => cpu.set_ipl(level),
                EventKind::Reset => {
                    cpu.reset();
                }
            }
        }
        let before = cpu.capture_state();
        if manifest.at_pc.contains(&before.pc) {
            break "at_pc";
        }
        let kind = next_boundary(&before);
        if manifest.on_idle
            && matches!(kind, BoundaryKind::HaltedIdle | BoundaryKind::StoppedIdle)
            && events.peek().is_none()
        {
            break "idle";
        }
        if !record {
            clocks += u64::from(cpu.step());
            steps += 1;
            continue;
        }
        let instruction = (kind == BoundaryKind::Instruction)
            .then(|| instruction_at(before.pc, |address| cpu.bus.peek_word(address)));
        cpu.bus.accesses = Some(Vec::new());
        let cycles = cpu.step();
        let accesses = cpu.bus.accesses.take();
        emit(StepRecord {
            sequence: steps,
            kind,
            before,
            after: cpu.capture_state(),
            cycles,
            instruction,
            accesses,
        });
        clocks += u64::from(cycles);
        steps += 1;
    };
    TraceRun {
        steps,
        clocks,
        reason,
    }
}
