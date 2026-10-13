//! Headless runner for test ROMs.
//!
//! Prints the cartridge header, runs the ROM until it reports a verdict (see
//! the library docs) and echoes what it sent over the serial port.

use std::process::ExitCode;

use glassbit_core::cartridge::Header;
use glassbit_runner::{run_test, Outcome};

/// About two minutes of emulated time.
const DEFAULT_MAX_FRAMES: u32 = 7200;

const USAGE: &str = "usage: glassbit-runner <rom.gb> [--frames N]";

fn main() -> ExitCode {
    let mut path = None;
    let mut max_frames = DEFAULT_MAX_FRAMES;
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        if arg == "--frames" {
            match args.next().and_then(|value| value.parse().ok()) {
                Some(frames) => max_frames = frames,
                None => {
                    eprintln!("{USAGE}");
                    return ExitCode::from(2);
                }
            }
        } else if path.is_none() {
            path = Some(arg);
        } else {
            eprintln!("{USAGE}");
            return ExitCode::from(2);
        }
    }
    let Some(path) = path else {
        eprintln!("{USAGE}");
        return ExitCode::from(2);
    };

    let rom = match std::fs::read(&path) {
        Ok(rom) => rom,
        Err(err) => {
            eprintln!("error: cannot read {path}: {err}");
            return ExitCode::FAILURE;
        }
    };

    // Parse the header on its own so it can be shown even when the mapper is
    // not supported yet.
    match Header::parse(&rom) {
        Ok(header) => print_header(&header),
        Err(err) => {
            eprintln!("error: {path}: {err}");
            return ExitCode::FAILURE;
        }
    }

    let run = match run_test(&rom, max_frames) {
        Ok(run) => run,
        Err(err) => {
            eprintln!("error: {path}: {err}");
            return ExitCode::FAILURE;
        }
    };

    if !run.output.is_empty() {
        println!("--- serial output ---");
        println!("{}", run.output.trim_end());
        println!("---------------------");
    }
    match run.outcome {
        Outcome::Passed => {
            println!("PASSED after {} frames", run.frames);
            ExitCode::SUCCESS
        }
        Outcome::Failed => {
            println!("FAILED after {} frames", run.frames);
            ExitCode::FAILURE
        }
        Outcome::Timeout => {
            println!("TIMEOUT: no verdict after {} frames", run.frames);
            ExitCode::FAILURE
        }
    }
}

fn print_header(header: &Header) {
    let kind = &header.cartridge_type;
    let mut extras = Vec::new();
    for (present, name) in [
        (kind.ram, "RAM"),
        (kind.battery, "battery"),
        (kind.timer, "timer"),
        (kind.rumble, "rumble"),
    ] {
        if present {
            extras.push(name);
        }
    }

    println!("title:           {}", header.title);
    println!(
        "cartridge type:  {:#04x} ({:?}{}{})",
        kind.code,
        kind.mapper,
        if extras.is_empty() { "" } else { " + " },
        extras.join(" + ")
    );
    println!("ROM size:        {} KiB", header.rom_size / 1024);
    println!("RAM size:        {} KiB", header.ram_size / 1024);
    println!("CGB support:     {:?}", header.cgb);
    println!(
        "header checksum: {:#04x} ({})",
        header.header_checksum,
        verdict(header.header_checksum_ok())
    );
    println!(
        "global checksum: {:#06x} ({})",
        header.global_checksum,
        verdict(header.global_checksum_ok())
    );
}

fn verdict(ok: bool) -> &'static str {
    if ok {
        "ok"
    } else {
        "MISMATCH"
    }
}
