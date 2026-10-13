//! Blargg's `cpu_instrs`: each ROM exercises one group of instructions with
//! many operand combinations and prints the verdict over the serial port.
//!
//! `02-interrupts` needs the timer and interrupt dispatch and joins this
//! list in Phase 3, together with the combined `cpu_instrs.gb`.

use glassbit_runner::{run_serial_test, test_roms_dir, Outcome};

/// The slowest ROM (11) finishes after about 1050 frames.
const MAX_FRAMES: u32 = 3600;

fn passes(name: &str) {
    let Some(dir) = test_roms_dir() else { return };
    let path = dir.join("blargg/cpu_instrs/individual").join(name);
    let rom = std::fs::read(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    let run = run_serial_test(&rom, MAX_FRAMES).unwrap_or_else(|e| panic!("{name}: {e}"));
    assert_eq!(
        run.outcome,
        Outcome::Passed,
        "{name} printed:\n{}",
        run.output
    );
}

#[test]
fn special() {
    passes("01-special.gb");
}

#[test]
fn op_sp_hl() {
    passes("03-op sp,hl.gb");
}

#[test]
fn op_r_imm() {
    passes("04-op r,imm.gb");
}

#[test]
fn op_rp() {
    passes("05-op rp.gb");
}

#[test]
fn ld_r_r() {
    passes("06-ld r,r.gb");
}

#[test]
fn jr_jp_call_ret_rst() {
    passes("07-jr,jp,call,ret,rst.gb");
}

#[test]
fn misc_instrs() {
    passes("08-misc instrs.gb");
}

#[test]
fn op_r_r() {
    passes("09-op r,r.gb");
}

#[test]
fn bit_ops() {
    passes("10-bit ops.gb");
}

#[test]
fn op_a_hl() {
    passes("11-op a,(hl).gb");
}
