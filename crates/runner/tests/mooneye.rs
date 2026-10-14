//! Mooneye acceptance tests, which report through a register pattern and an
//! `LD B,B` breakpoint. Each one checks a single hardware behavior down to
//! the M-cycle.
//!
//! Only tests that already pass are listed, as a guard against regressions.
//! Of the rest, most target other console models (DMG0, MGB, SGB), and a
//! few PPU ones need timing finer than M-cycles (the LCD switching on, and
//! when LY changes relative to the HBlank interrupt).

use glassbit_runner::assert_rom_passes;

/// These ROMs finish in well under a second of emulated time.
const MAX_FRAMES: u32 = 300;

fn passes(name: &str) {
    assert_rom_passes(&format!("mooneye-test-suite/acceptance/{name}"), MAX_FRAMES);
}

mod general {
    use super::passes;

    #[test]
    fn add_sp_e_timing() {
        passes("add_sp_e_timing.gb");
    }

    #[test]
    fn boot_div_dmgabcmgb() {
        passes("boot_div-dmgABCmgb.gb");
    }

    #[test]
    fn boot_hwio_dmgabcmgb() {
        passes("boot_hwio-dmgABCmgb.gb");
    }

    #[test]
    fn boot_regs_dmgabc() {
        passes("boot_regs-dmgABC.gb");
    }

    #[test]
    fn call_cc_timing() {
        passes("call_cc_timing.gb");
    }

    #[test]
    fn call_cc_timing2() {
        passes("call_cc_timing2.gb");
    }

    #[test]
    fn call_timing() {
        passes("call_timing.gb");
    }

    #[test]
    fn call_timing2() {
        passes("call_timing2.gb");
    }

    #[test]
    fn div_timing() {
        passes("div_timing.gb");
    }

    #[test]
    fn ei_sequence() {
        passes("ei_sequence.gb");
    }

    #[test]
    fn ei_timing() {
        passes("ei_timing.gb");
    }

    #[test]
    fn halt_ime0_ei() {
        passes("halt_ime0_ei.gb");
    }

    #[test]
    fn halt_ime0_nointr_timing() {
        passes("halt_ime0_nointr_timing.gb");
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
    fn jp_cc_timing() {
        passes("jp_cc_timing.gb");
    }

    #[test]
    fn jp_timing() {
        passes("jp_timing.gb");
    }

    #[test]
    fn ld_hl_sp_e_timing() {
        passes("ld_hl_sp_e_timing.gb");
    }

    #[test]
    fn oam_dma_restart() {
        passes("oam_dma_restart.gb");
    }

    #[test]
    fn oam_dma_start() {
        passes("oam_dma_start.gb");
    }

    #[test]
    fn oam_dma_timing() {
        passes("oam_dma_timing.gb");
    }

    #[test]
    fn pop_timing() {
        passes("pop_timing.gb");
    }

    #[test]
    fn push_timing() {
        passes("push_timing.gb");
    }

    #[test]
    fn rapid_di_ei() {
        passes("rapid_di_ei.gb");
    }

    #[test]
    fn ret_cc_timing() {
        passes("ret_cc_timing.gb");
    }

    #[test]
    fn ret_timing() {
        passes("ret_timing.gb");
    }

    #[test]
    fn reti_intr_timing() {
        passes("reti_intr_timing.gb");
    }

    #[test]
    fn reti_timing() {
        passes("reti_timing.gb");
    }

    #[test]
    fn rst_timing() {
        passes("rst_timing.gb");
    }
}

mod bits {
    use super::passes;

    #[test]
    fn mem_oam() {
        passes("bits/mem_oam.gb");
    }

    #[test]
    fn reg_f() {
        passes("bits/reg_f.gb");
    }
}

mod instr {
    use super::passes;

    #[test]
    fn daa() {
        passes("instr/daa.gb");
    }
}

mod interrupts {
    use super::passes;

    #[test]
    fn ie_push() {
        passes("interrupts/ie_push.gb");
    }
}

mod oam_dma {
    use super::passes;

    #[test]
    fn basic() {
        passes("oam_dma/basic.gb");
    }

    #[test]
    fn reg_read() {
        passes("oam_dma/reg_read.gb");
    }

    #[test]
    fn sources_gs() {
        passes("oam_dma/sources-GS.gb");
    }
}

mod ppu {
    use super::{assert_rom_passes, passes};

    #[test]
    fn intr_1_2_timing_gs() {
        passes("ppu/intr_1_2_timing-GS.gb");
    }

    #[test]
    fn intr_2_0_timing() {
        passes("ppu/intr_2_0_timing.gb");
    }

    #[test]
    fn intr_2_mode0_timing() {
        passes("ppu/intr_2_mode0_timing.gb");
    }

    /// About a hundred sprite layouts, each lengthening mode 3 by a
    /// different number of dots. It takes a few seconds of emulated time.
    #[test]
    fn intr_2_mode0_timing_sprites() {
        assert_rom_passes(
            "mooneye-test-suite/acceptance/ppu/intr_2_mode0_timing_sprites.gb",
            600,
        );
    }

    #[test]
    fn intr_2_mode3_timing() {
        passes("ppu/intr_2_mode3_timing.gb");
    }

    #[test]
    fn intr_2_oam_ok_timing() {
        passes("ppu/intr_2_oam_ok_timing.gb");
    }

    #[test]
    fn stat_irq_blocking() {
        passes("ppu/stat_irq_blocking.gb");
    }

    #[test]
    fn vblank_stat_intr_gs() {
        passes("ppu/vblank_stat_intr-GS.gb");
    }
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
