//! Save states against real ROMs: a console restored from a state must go on
//! exactly like the one that kept running. Each ROM leans on a different
//! part of the machine, so a field forgotten in the state shows up as a
//! divergence in one of them.

use glassbit_core::Model;
use glassbit_runner::{boot, test_roms_dir};

/// Saves after `warm_up` frames, restores into a fresh console and checks
/// that both stay identical for `compare` more frames.
fn continues_identically(relative: &str, warm_up: u32, compare: u32) {
    continues_identically_on(None, relative, warm_up, compare);
}

/// Like [`continues_identically`], on the given console.
fn continues_identically_on(model: Option<Model>, relative: &str, warm_up: u32, compare: u32) {
    let Some(dir) = test_roms_dir() else { return };
    let path = dir.join(relative);
    let rom = std::fs::read(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));

    let mut original = boot(&rom, model).unwrap_or_else(|e| panic!("{relative}: {e}"));
    for _ in 0..warm_up {
        original.run_frame();
    }
    // Stop between two instructions in the middle of a frame, where more
    // of the hardware is busy than at a frame boundary.
    for _ in 0..777 {
        original.step();
    }
    let state = original.save_state();
    // What was printed before the save is not part of the state.
    original.take_serial_output();

    let mut restored = boot(&rom, model).unwrap_or_else(|e| panic!("{relative}: {e}"));
    restored
        .load_state(&state)
        .unwrap_or_else(|e| panic!("{relative}: {e}"));

    for frame in 0..compare {
        original.run_frame();
        restored.run_frame();
        assert_eq!(
            restored.cycles(),
            original.cycles(),
            "{relative}: cycles, frame {frame}"
        );
        assert_eq!(
            restored.cpu(),
            original.cpu(),
            "{relative}: CPU, frame {frame}"
        );
        assert!(
            restored.framebuffer() == original.framebuffer()
                && restored.color_framebuffer() == original.color_framebuffer(),
            "{relative}: screen differs in frame {frame}"
        );
        assert!(
            restored.audio_samples() == original.audio_samples(),
            "{relative}: audio differs in frame {frame}"
        );
        assert!(
            restored.take_serial_output() == original.take_serial_output(),
            "{relative}: serial output differs in frame {frame}"
        );
    }
    assert!(
        restored.save_state() == original.save_state(),
        "{relative}: the two consoles ended in different states"
    );
}

/// CPU, serial port and MBC1 banking.
#[test]
fn cpu_instrs() {
    continues_identically("blargg/cpu_instrs/cpu_instrs.gb", 600, 300);
}

/// Timer and interrupts.
#[test]
fn instr_timing() {
    continues_identically("blargg/instr_timing/instr_timing.gb", 20, 60);
}

/// Sprites, window and palettes.
#[test]
fn dmg_acid2() {
    continues_identically("dmg-acid2/dmg-acid2.gb", 5, 30);
}

/// All four sound channels, length counters and sweep.
#[test]
fn dmg_sound() {
    continues_identically("blargg/dmg_sound/dmg_sound.gb", 300, 300);
}

/// OAM DMA in progress.
#[test]
fn oam_dma() {
    continues_identically("mooneye-test-suite/acceptance/oam_dma/basic.gb", 8, 20);
}

/// MBC5 with cartridge RAM.
#[test]
fn mbc5() {
    continues_identically("mooneye-test-suite/emulator-only/mbc5/rom_4Mb.gb", 6, 20);
}

/// MBC3 (the MBC30 variant).
#[test]
fn mbc3() {
    continues_identically("mbc3-tester/mbc3-tester.gb", 10, 40);
}

/// A DMG game on a Game Boy Color, in compatibility mode.
#[test]
fn compatibility_mode() {
    continues_identically_on(Some(Model::Cgb), "dmg-acid2/dmg-acid2.gb", 10, 20);
}
