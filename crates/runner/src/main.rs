//! Headless runner for test ROMs.
//!
//! Prints the cartridge header, runs the ROM until it reports a verdict (see
//! the library docs) and echoes what it sent over the serial port.
//!
//! `--screenshot out.png` saves the final screen. `--reference ref.png`
//! turns the run into a visual test: the verdict is whether the screen
//! matches that image pixel for pixel.

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use glassbit_core::cartridge::Header;
use glassbit_runner::{run_screen_test, run_test, screenshot, Outcome};

/// About two minutes of emulated time.
const DEFAULT_MAX_FRAMES: u32 = 7200;

const USAGE: &str =
    "usage: glassbit-runner <rom.gb> [--frames N] [--screenshot out.png] [--reference ref.png]";

fn main() -> ExitCode {
    let mut path = None;
    let mut max_frames = DEFAULT_MAX_FRAMES;
    let mut screenshot_path: Option<PathBuf> = None;
    let mut reference_path: Option<PathBuf> = None;
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        if arg == "--screenshot" || arg == "--reference" {
            let Some(value) = args.next() else {
                eprintln!("{USAGE}");
                return ExitCode::from(2);
            };
            if arg == "--screenshot" {
                screenshot_path = Some(value.into());
            } else {
                reference_path = Some(value.into());
            }
        } else if arg == "--frames" {
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

    if let Some(reference) = reference_path {
        let screen = match run_screen_test(&rom, max_frames) {
            Ok(screen) => screen,
            Err(err) => {
                eprintln!("error: {path}: {err}");
                return ExitCode::FAILURE;
            }
        };
        if !save_screenshot(&screen, screenshot_path.as_deref()) {
            return ExitCode::FAILURE;
        }
        let expected = match screenshot::load(&reference) {
            Ok(expected) => expected,
            Err(err) => {
                eprintln!("error: {}: {err}", reference.display());
                return ExitCode::FAILURE;
            }
        };
        return match screenshot::compare(&screen, &expected) {
            (_, None) => {
                println!("PASSED: screen matches {}", reference.display());
                ExitCode::SUCCESS
            }
            (differing, Some((x, y))) => {
                println!("FAILED: {differing} pixels differ, first at ({x}, {y})");
                ExitCode::FAILURE
            }
        };
    }

    let run = match run_test(&rom, max_frames) {
        Ok(run) => run,
        Err(err) => {
            eprintln!("error: {path}: {err}");
            return ExitCode::FAILURE;
        }
    };

    if !save_screenshot(&run.framebuffer, screenshot_path.as_deref()) {
        return ExitCode::FAILURE;
    }
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

/// Saves the screen when a path was given. Returns `false` on failure.
fn save_screenshot(framebuffer: &[u8], path: Option<&Path>) -> bool {
    let Some(path) = path else { return true };
    match screenshot::save(framebuffer, path) {
        Ok(()) => {
            println!("screenshot saved to {}", path.display());
            true
        }
        Err(err) => {
            eprintln!("error: {}: {err}", path.display());
            false
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
