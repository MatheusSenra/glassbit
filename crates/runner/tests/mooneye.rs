//! Mooneye acceptance tests, which report through a register pattern and an
//! `LD B,B` breakpoint. Each one checks a single hardware behavior down to
//! the M-cycle.
//!
//! Only tests that already pass are listed, as a guard against regressions.
//! Most of the rest wait for the PPU and OAM DMA (phase 4), or target other
//! console models (DMG0, MGB, SGB) that Glassbit does not emulate.

use glassbit_runner::assert_rom_passes;

/// These ROMs finish in well under a second of emulated time.
const MAX_FRAMES: u32 = 300;

fn passes(name: &str) {
    assert_rom_passes(&format!("mooneye-test-suite/acceptance/{name}"), MAX_FRAMES);
}

mod timer {
    use super::passes;

    #[test]
    fn div_write() {
        passes("timer/div_write.gb");
    }

    #[test]
    fn rapid_toggle() {
        passes("timer/rapid_toggle.gb");
    }

    #[test]
    fn tim00() {
        passes("timer/tim00.gb");
    }

    #[test]
    fn tim00_div_trigger() {
        passes("timer/tim00_div_trigger.gb");
    }

    #[test]
    fn tim01() {
        passes("timer/tim01.gb");
    }

    #[test]
    fn tim01_div_trigger() {
        passes("timer/tim01_div_trigger.gb");
    }

    #[test]
    fn tim10() {
        passes("timer/tim10.gb");
    }

    #[test]
    fn tim10_div_trigger() {
        passes("timer/tim10_div_trigger.gb");
    }

    #[test]
    fn tim11() {
        passes("timer/tim11.gb");
    }

    #[test]
    fn tim11_div_trigger() {
        passes("timer/tim11_div_trigger.gb");
    }

    #[test]
    fn tima_reload() {
        passes("timer/tima_reload.gb");
    }

    #[test]
    fn tima_write_reloading() {
        passes("timer/tima_write_reloading.gb");
    }

    #[test]
    fn tma_write_reloading() {
        passes("timer/tma_write_reloading.gb");
    }
}

mod interrupts {
    use super::passes;

    #[test]
    fn ei_sequence() {
        passes("ei_sequence.gb");
    }

    #[test]
    fn ei_timing() {
        passes("ei_timing.gb");
    }

    #[test]
    fn halt_ime1_timing() {
        passes("halt_ime1_timing.gb");
    }

    #[test]
    fn if_ie_registers() {
        passes("if_ie_registers.gb");
    }

    #[test]
    fn intr_timing() {
        passes("intr_timing.gb");
    }

    #[test]
    fn rapid_di_ei() {
        passes("rapid_di_ei.gb");
    }

    #[test]
    fn reti_intr_timing() {
        passes("reti_intr_timing.gb");
    }

    #[test]
    fn ie_push() {
        passes("interrupts/ie_push.gb");
    }
}

mod cpu {
    use super::passes;

    #[test]
    fn pop_timing() {
        passes("pop_timing.gb");
    }

    #[test]
    fn reg_f() {
        passes("bits/reg_f.gb");
    }

    #[test]
    fn daa() {
        passes("instr/daa.gb");
    }

    #[test]
    fn mem_oam() {
        passes("bits/mem_oam.gb");
    }
}

mod boot {
    use super::passes;

    #[test]
    fn boot_div_dmgabcmgb() {
        passes("boot_div-dmgABCmgb.gb");
    }

    #[test]
    fn boot_regs_dmgabc() {
        passes("boot_regs-dmgABC.gb");
    }

    #[test]
    fn div_timing() {
        passes("div_timing.gb");
    }
}
