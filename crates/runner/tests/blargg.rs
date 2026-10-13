//! Blargg's test ROMs, which print their verdict over the serial port.
//!
//! - `cpu_instrs`: every instruction with many operand combinations.
//! - `instr_timing`: the duration of every instruction, measured with the timer.
//! - `mem_timing`: *when* inside an instruction each memory access happens.

use glassbit_runner::assert_rom_passes;

mod cpu_instrs {
    use super::*;

    const MAX_FRAMES: u32 = 4000;

    #[test]
    fn combined() {
        assert_rom_passes("blargg/cpu_instrs/cpu_instrs.gb", MAX_FRAMES);
    }

    #[test]
    fn t01_special() {
        assert_rom_passes("blargg/cpu_instrs/individual/01-special.gb", MAX_FRAMES);
    }

    #[test]
    fn t02_interrupts() {
        assert_rom_passes("blargg/cpu_instrs/individual/02-interrupts.gb", MAX_FRAMES);
    }

    #[test]
    fn t03_op_sp_hl() {
        assert_rom_passes("blargg/cpu_instrs/individual/03-op sp,hl.gb", MAX_FRAMES);
    }

    #[test]
    fn t04_op_r_imm() {
        assert_rom_passes("blargg/cpu_instrs/individual/04-op r,imm.gb", MAX_FRAMES);
    }

    #[test]
    fn t05_op_rp() {
        assert_rom_passes("blargg/cpu_instrs/individual/05-op rp.gb", MAX_FRAMES);
    }

    #[test]
    fn t06_ld_r_r() {
        assert_rom_passes("blargg/cpu_instrs/individual/06-ld r,r.gb", MAX_FRAMES);
    }

    #[test]
    fn t07_jr_jp_call_ret_rst() {
        assert_rom_passes(
            "blargg/cpu_instrs/individual/07-jr,jp,call,ret,rst.gb",
            MAX_FRAMES,
        );
    }

    #[test]
    fn t08_misc_instrs() {
        assert_rom_passes("blargg/cpu_instrs/individual/08-misc instrs.gb", MAX_FRAMES);
    }

    #[test]
    fn t09_op_r_r() {
        assert_rom_passes("blargg/cpu_instrs/individual/09-op r,r.gb", MAX_FRAMES);
    }

    #[test]
    fn t10_bit_ops() {
        assert_rom_passes("blargg/cpu_instrs/individual/10-bit ops.gb", MAX_FRAMES);
    }

    #[test]
    fn t11_op_a_hl() {
        assert_rom_passes("blargg/cpu_instrs/individual/11-op a,(hl).gb", MAX_FRAMES);
    }
}

mod instr_timing {
    use super::*;

    const MAX_FRAMES: u32 = 600;

    #[test]
    fn combined() {
        assert_rom_passes("blargg/instr_timing/instr_timing.gb", MAX_FRAMES);
    }
}

mod mem_timing {
    use super::*;

    const MAX_FRAMES: u32 = 600;

    #[test]
    fn combined() {
        assert_rom_passes("blargg/mem_timing/mem_timing.gb", MAX_FRAMES);
    }

    #[test]
    fn t01_read_timing() {
        assert_rom_passes("blargg/mem_timing/individual/01-read_timing.gb", MAX_FRAMES);
    }

    #[test]
    fn t02_write_timing() {
        assert_rom_passes(
            "blargg/mem_timing/individual/02-write_timing.gb",
            MAX_FRAMES,
        );
    }

    #[test]
    fn t03_modify_timing() {
        assert_rom_passes(
            "blargg/mem_timing/individual/03-modify_timing.gb",
            MAX_FRAMES,
        );
    }
}

/// `dmg_sound`: the APU's registers, length counters, sweep and power
/// switch. These report through cartridge RAM instead of the serial port.
///
/// Tests 09, 10 and 12 are left out: they check what reading and writing
/// wave RAM does while channel 3 is playing, which is not modeled.
mod dmg_sound {
    use super::*;

    const MAX_FRAMES: u32 = 1800;

    fn passes(name: &str) {
        assert_rom_passes(&format!("blargg/dmg_sound/rom_singles/{name}"), MAX_FRAMES);
    }

    #[test]
    fn t01_registers() {
        passes("01-registers.gb");
    }

    #[test]
    fn t02_len_ctr() {
        passes("02-len ctr.gb");
    }

    #[test]
    fn t03_trigger() {
        passes("03-trigger.gb");
    }

    #[test]
    fn t04_sweep() {
        passes("04-sweep.gb");
    }

    #[test]
    fn t05_sweep_details() {
        passes("05-sweep details.gb");
    }

    #[test]
    fn t06_overflow_on_trigger() {
        passes("06-overflow on trigger.gb");
    }

    #[test]
    fn t07_len_sweep_period_sync() {
        passes("07-len sweep period sync.gb");
    }

    #[test]
    fn t08_len_ctr_during_power() {
        passes("08-len ctr during power.gb");
    }

    #[test]
    fn t11_regs_after_power() {
        passes("11-regs after power.gb");
    }
}
