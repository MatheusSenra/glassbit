//! Runs test ROMs headlessly and decides whether they passed.

use std::path::{Path, PathBuf};

use glassbit_core::cartridge::CartridgeError;
use glassbit_core::GameBoy;

/// How a test ROM run ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    Passed,
    Failed,
    /// The ROM reported nothing conclusive within the frame budget.
    Timeout,
}

/// The result of running a ROM that reports over the serial port.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SerialRun {
    pub outcome: Outcome,
    /// Everything the ROM printed.
    pub output: String,
    pub frames: u32,
}

/// Frames to keep running after the verdict shows up.
const VERDICT_TAIL_FRAMES: u32 = 30;

/// Runs a Blargg-style test ROM, which prints its progress over the serial
/// port and ends with a line containing "Passed" or "Failed".
pub fn run_serial_test(rom: &[u8], mut max_frames: u32) -> Result<SerialRun, CartridgeError> {
    let mut gb = GameBoy::new(rom)?;
    let mut output = String::new();
    let mut outcome = Outcome::Timeout;
    let mut frames = 0;

    while frames < max_frames {
        gb.run_frame();
        frames += 1;
        let bytes = gb.take_serial_output();
        if bytes.is_empty() {
            continue;
        }
        output.extend(bytes.iter().map(|&b| b as char));
        if outcome != Outcome::Timeout {
            continue;
        }
        if output.contains("Passed") {
            outcome = Outcome::Passed;
        } else if output.contains("Failed") {
            outcome = Outcome::Failed;
        } else {
            continue;
        }
        // Keep going briefly so the rest of the verdict line is captured.
        max_frames = max_frames.min(frames + VERDICT_TAIL_FRAMES);
    }

    Ok(SerialRun {
        outcome,
        output,
        frames,
    })
}

/// Where `scripts/fetch-test-roms.sh` puts the prebuilt test ROMs, if they
/// have been downloaded.
///
/// When they are missing, tests should skip themselves; setting
/// `GLASSBIT_REQUIRE_TEST_ROMS` (as CI does) turns that into a panic.
pub fn test_roms_dir() -> Option<PathBuf> {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../test-roms/game-boy-test-roms");
    if dir.is_dir() {
        return Some(dir);
    }
    assert!(
        std::env::var_os("GLASSBIT_REQUIRE_TEST_ROMS").is_none(),
        "{} not found; run scripts/fetch-test-roms.sh",
        dir.display()
    );
    eprintln!("skipping: run scripts/fetch-test-roms.sh to enable ROM tests");
    None
}
