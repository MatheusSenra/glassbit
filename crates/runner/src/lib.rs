//! Runs test ROMs headlessly and decides whether they passed.
//!
//! Two reporting conventions are understood:
//!
//! - **Blargg**: the ROM prints text over the serial port, ending with a line
//!   that contains "Passed" or "Failed".
//! - **Blargg, in memory**: his later ROMs write the same text to cartridge
//!   RAM instead. `0xA001..=0xA003` hold the signature `DE B0 61`, `0xA000`
//!   holds `0x80` while running and then the result code (0 = passed), and
//!   the text starts at `0xA004`.
//! - **Mooneye**: the ROM executes `LD B,B` as a breakpoint, with the
//!   Fibonacci numbers 3, 5, 8, 13, 21, 34 in B, C, D, E, H, L on success and
//!   `0x42` in all of them on failure.
//!
//! Visual tests such as dmg-acid2 have no verdict of their own: they draw a
//! picture and signal that it is ready with the same `LD B,B` breakpoint. For
//! those, [`run_screen_test`] returns the screen to compare with a reference.

pub mod recording;
pub mod screenshot;

use std::path::{Path, PathBuf};

use glassbit_core::cartridge::CartridgeError;
use glassbit_core::cpu::Registers;
use glassbit_core::{GameBoy, CYCLES_PER_FRAME};

/// How a test ROM run ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    Passed,
    Failed,
    /// The ROM reported nothing conclusive within the frame budget.
    Timeout,
}

/// The result of running a test ROM.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TestRun {
    pub outcome: Outcome,
    /// Everything the ROM printed over the serial port.
    pub output: String,
    pub frames: u32,
    /// The screen at the end of the run, as RGB.
    pub screen: Vec<screenshot::Rgb>,
}

/// Frames to keep running after a serial verdict shows up, so the rest of
/// that line is captured.
const VERDICT_TAIL_FRAMES: u64 = 30;

const LD_B_B: u8 = 0x40;

/// Runs a test ROM until it reports a verdict or `max_frames` go by.
pub fn run_test(rom: &[u8], max_frames: u32) -> Result<TestRun, CartridgeError> {
    let mut gb = GameBoy::new(rom)?;
    let mut output = String::new();
    let mut outcome = Outcome::Timeout;
    let mut deadline = max_frames as u64 * CYCLES_PER_FRAME;
    let mut next_serial_check = CYCLES_PER_FRAME;

    while gb.cycles() < deadline {
        let breakpoint = !gb.cpu().halted && gb.peek(gb.cpu().regs.pc) == LD_B_B;
        gb.step();
        if breakpoint {
            // Blargg's ROMs also execute LD B,B as an ordinary instruction,
            // so only the two exact register patterns count.
            if let Some(verdict) = mooneye_verdict(&gb.cpu().regs) {
                outcome = verdict;
                break;
            }
        }

        if gb.cycles() < next_serial_check {
            continue;
        }
        next_serial_check += CYCLES_PER_FRAME;
        let bytes = gb.take_serial_output();
        output.extend(bytes.iter().map(|&b| b as char));
        if outcome != Outcome::Timeout {
            continue;
        }
        if let Some((verdict, text)) = memory_verdict(&gb) {
            outcome = verdict;
            output.push_str(&text);
            break;
        }
        if output.contains("Passed") {
            outcome = Outcome::Passed;
        } else if output.contains("Failed") {
            outcome = Outcome::Failed;
        } else {
            continue;
        }
        deadline = deadline.min(gb.cycles() + VERDICT_TAIL_FRAMES * CYCLES_PER_FRAME);
    }

    output.extend(gb.take_serial_output().iter().map(|&b| b as char));
    let frames = gb.cycles().div_ceil(CYCLES_PER_FRAME) as u32;
    // Let the frame in progress finish so the screen is complete.
    gb.run_frame();
    Ok(TestRun {
        outcome,
        output,
        frames,
        screen: gb.rgb_framebuffer(),
    })
}

/// Runs a visual test ROM until it executes `LD B,B` (or `max_frames` go by)
/// and returns the finished screen as RGB.
pub fn run_screen_test(
    rom: &[u8],
    max_frames: u32,
) -> Result<Vec<screenshot::Rgb>, CartridgeError> {
    let mut gb = GameBoy::new(rom)?;
    let deadline = max_frames as u64 * CYCLES_PER_FRAME;
    while gb.cycles() < deadline {
        let breakpoint = !gb.cpu().halted && gb.peek(gb.cpu().regs.pc) == LD_B_B;
        gb.step();
        if breakpoint {
            break;
        }
    }
    // The picture is ready in video memory; give the PPU a full frame to
    // draw it, after finishing the one in progress.
    gb.run_frame();
    gb.run_frame();
    Ok(gb.rgb_framebuffer())
}

/// Checks for a finished Blargg test that reports through cartridge RAM.
fn memory_verdict(gb: &GameBoy) -> Option<(Outcome, String)> {
    const RUNNING: u8 = 0x80;
    let signature = [gb.peek(0xA001), gb.peek(0xA002), gb.peek(0xA003)];
    let status = gb.peek(0xA000);
    if signature != [0xDE, 0xB0, 0x61] || status == RUNNING {
        return None;
    }
    let text: String = (0xA004..0xC000)
        .map(|addr| gb.peek(addr))
        .take_while(|&byte| byte != 0)
        .map(|byte| byte as char)
        .collect();
    let outcome = if status == 0 {
        Outcome::Passed
    } else {
        Outcome::Failed
    };
    Some((outcome, text))
}

fn mooneye_verdict(regs: &Registers) -> Option<Outcome> {
    let values = [regs.b, regs.c, regs.d, regs.e, regs.h, regs.l];
    if values == [3, 5, 8, 13, 21, 34] {
        Some(Outcome::Passed)
    } else if values == [0x42; 6] {
        Some(Outcome::Failed)
    } else {
        None
    }
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

/// Runs the ROM at `relative` (inside the test ROM directory) and panics
/// unless it passes. Does nothing when the ROMs have not been downloaded.
pub fn assert_rom_passes(relative: &str, max_frames: u32) {
    let Some(dir) = test_roms_dir() else { return };
    let path = dir.join(relative);
    let rom = std::fs::read(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    let run = run_test(&rom, max_frames).unwrap_or_else(|e| panic!("{relative}: {e}"));
    assert_eq!(
        run.outcome,
        Outcome::Passed,
        "{relative} after {} frames printed:\n{}",
        run.frames,
        run.output
    );
}

/// Runs the visual test ROM at `rom` and panics unless the screen matches
/// the PNG at `reference` exactly (both paths inside the test ROM directory).
/// Does nothing when the ROMs have not been downloaded.
pub fn assert_screen_matches(rom: &str, reference: &str, max_frames: u32) {
    let Some(dir) = test_roms_dir() else { return };
    let path = dir.join(rom);
    let image = std::fs::read(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    let actual = run_screen_test(&image, max_frames).unwrap_or_else(|e| panic!("{rom}: {e}"));
    let expected =
        screenshot::load(&dir.join(reference)).unwrap_or_else(|e| panic!("{reference}: {e}"));

    let (differing, first) = screenshot::compare(&actual, &expected);
    if let Some((x, y)) = first {
        // Keep the evidence where it is easy to find.
        let out = std::env::temp_dir().join("glassbit-failed-screen.png");
        let saved = match screenshot::save(&actual, &out) {
            Ok(()) => format!("screen saved to {}", out.display()),
            Err(e) => format!("could not save the screen: {e}"),
        };
        panic!("{rom}: {differing} pixels differ from {reference}, first at ({x}, {y}); {saved}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn regs(values: [u8; 6]) -> Registers {
        let [b, c, d, e, h, l] = values;
        Registers {
            b,
            c,
            d,
            e,
            h,
            l,
            ..Registers::default()
        }
    }

    #[test]
    fn recognizes_the_mooneye_register_patterns() {
        assert_eq!(
            mooneye_verdict(&regs([3, 5, 8, 13, 21, 34])),
            Some(Outcome::Passed)
        );
        assert_eq!(mooneye_verdict(&regs([0x42; 6])), Some(Outcome::Failed));
        assert_eq!(mooneye_verdict(&regs([3, 5, 8, 13, 21, 0])), None);
    }
}
