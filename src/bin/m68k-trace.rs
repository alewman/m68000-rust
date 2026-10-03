//! Run a conformance manifest and stream its trace (rungs 1, 3 and 5).
//!
//! Usage: `m68k-trace <manifest.json> [--out FILE] [--no-trace]`. The trace
//! goes to stdout (or `--out`) as JSON Lines in m68000-python's trace
//! schema, ready for `python -m m68000_python.conformance diff manifest -`.
//! `--no-trace` steps the run bare and reports only its length. The summary
//! line goes to stderr.

use std::io::{BufWriter, Write};
use std::path::Path;
use std::process::ExitCode;

use m68000_rust::conformance::{load_manifest, trace_manifest};
use m68000_rust::trace::record_to_json;

fn main() -> ExitCode {
    let mut arguments = std::env::args().skip(1);
    let (mut manifest_path, mut out_path, mut record) = (None, None, true);
    while let Some(argument) = arguments.next() {
        match argument.as_str() {
            "--out" => out_path = arguments.next(),
            "--no-trace" => record = false,
            _ if manifest_path.is_none() => manifest_path = Some(argument),
            _ => {
                eprintln!("usage: m68k-trace <manifest.json> [--out FILE] [--no-trace]");
                return ExitCode::from(2);
            }
        }
    }
    let Some(manifest_path) = manifest_path else {
        eprintln!("usage: m68k-trace <manifest.json> [--out FILE] [--no-trace]");
        return ExitCode::from(2);
    };
    let manifest = match load_manifest(Path::new(&manifest_path)) {
        Ok(manifest) => manifest,
        Err(message) => {
            eprintln!("error: {message}");
            return ExitCode::from(2);
        }
    };
    let sink: Box<dyn Write> = match &out_path {
        Some(path) => match std::fs::File::create(path) {
            Ok(file) => Box::new(file),
            Err(e) => {
                eprintln!("error: {path}: {e}");
                return ExitCode::from(2);
            }
        },
        None => Box::new(std::io::stdout().lock()),
    };
    let mut writer = BufWriter::with_capacity(1 << 20, sink);
    let mut broken = false;
    let run = trace_manifest(&manifest, record, |step| {
        if !broken && writer.write_all(record_to_json(&step).as_bytes()).is_err() {
            broken = true; // a closed pipe: the diff found its divergence and stopped reading
        }
    });
    let _ = writer.flush();
    eprintln!(
        "{}: {} records, {} clocks, stopped on {}",
        manifest.name, run.steps, run.clocks, run.reason
    );
    ExitCode::SUCCESS
}
