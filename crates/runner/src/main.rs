//! Headless runner for test ROMs.
//!
//! Phase 0 only loads the ROM. Serial capture arrives with the CPU (Phase 2)
//! and PNG screenshots with the PPU (Phase 4).

use std::process::ExitCode;

use glassbit_core::GameBoy;

fn main() -> ExitCode {
    let Some(path) = std::env::args().nth(1) else {
        eprintln!("usage: glassbit-runner <rom.gb>");
        return ExitCode::from(2);
    };

    let rom = match std::fs::read(&path) {
        Ok(rom) => rom,
        Err(err) => {
            eprintln!("error: cannot read {path}: {err}");
            return ExitCode::FAILURE;
        }
    };

    let mut gb = GameBoy::new();
    gb.load_rom(&rom);
    println!("loaded {path} ({} bytes)", gb.rom_len());
    println!(
        "glassbit-core {}: emulation is not implemented yet",
        glassbit_core::VERSION
    );
    ExitCode::SUCCESS
}
