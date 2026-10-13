//! Headless runner for test ROMs.
//!
//! For now it loads a ROM and prints its header. Serial capture arrives with
//! the CPU (Phase 2) and PNG screenshots with the PPU (Phase 4).

use std::process::ExitCode;

use glassbit_core::cartridge::Header;

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

    // Parse the header on its own so it can be shown even when the mapper is
    // not supported yet.
    let header = match Header::parse(&rom) {
        Ok(header) => header,
        Err(err) => {
            eprintln!("error: {path}: {err}");
            return ExitCode::FAILURE;
        }
    };
    print_header(&header);

    if let Err(err) = glassbit_core::GameBoy::new(&rom) {
        eprintln!("error: {path}: {err}");
        return ExitCode::FAILURE;
    }
    println!("cartridge loaded; emulation is not implemented yet");
    ExitCode::SUCCESS
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
