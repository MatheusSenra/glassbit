//! Cartridge mapper tests. They test the cartridge, not the console: each
//! ROM writes to the mapper registers and checks which ROM and RAM banks
//! show up.

use glassbit_runner::{assert_rom_passes, assert_screen_matches};

/// Mooneye's `emulator-only/mbc1`. `multicart_rom_8Mb` is left out:
/// multicart cartridges (MBC1M) are not supported.
mod mbc1 {
    use super::assert_rom_passes;

    fn passes(name: &str) {
        assert_rom_passes(
            &format!("mooneye-test-suite/emulator-only/mbc1/{name}"),
            600,
        );
    }

    #[test]
    fn bits_bank1() {
        passes("bits_bank1.gb");
    }

    #[test]
    fn bits_bank2() {
        passes("bits_bank2.gb");
    }

    #[test]
    fn bits_mode() {
        passes("bits_mode.gb");
    }

    #[test]
    fn bits_ramg() {
        passes("bits_ramg.gb");
    }

    #[test]
    fn ram_64kb() {
        passes("ram_64kb.gb");
    }

    #[test]
    fn ram_256kb() {
        passes("ram_256kb.gb");
    }

    #[test]
    fn rom_512kb() {
        passes("rom_512kb.gb");
    }

    #[test]
    fn rom_1mb() {
        passes("rom_1Mb.gb");
    }

    #[test]
    fn rom_2mb() {
        passes("rom_2Mb.gb");
    }

    #[test]
    fn rom_4mb() {
        passes("rom_4Mb.gb");
    }

    #[test]
    fn rom_8mb() {
        passes("rom_8Mb.gb");
    }

    #[test]
    fn rom_16mb() {
        passes("rom_16Mb.gb");
    }
}

/// The MBC3 bank tester switches through all 256 banks of a 4 MiB ROM and
/// draws a check mark for each one that shows up. Being that large, the
/// cartridge is the MBC30 variant.
#[test]
fn mbc3_reaches_every_bank() {
    assert_screen_matches(
        "mbc3-tester/mbc3-tester.gb",
        "mbc3-tester/mbc3-tester-dmg.png",
        60,
    );
}

/// Mooneye's `emulator-only/mbc5`.
mod mbc5 {
    use super::assert_rom_passes;

    fn passes(name: &str) {
        assert_rom_passes(
            &format!("mooneye-test-suite/emulator-only/mbc5/{name}"),
            300,
        );
    }

    #[test]
    fn rom_512kb() {
        passes("rom_512kb.gb");
    }

    #[test]
    fn rom_1mb() {
        passes("rom_1Mb.gb");
    }

    #[test]
    fn rom_2mb() {
        passes("rom_2Mb.gb");
    }

    #[test]
    fn rom_4mb() {
        passes("rom_4Mb.gb");
    }

    #[test]
    fn rom_8mb() {
        passes("rom_8Mb.gb");
    }

    #[test]
    fn rom_16mb() {
        passes("rom_16Mb.gb");
    }

    #[test]
    fn rom_32mb() {
        passes("rom_32Mb.gb");
    }

    #[test]
    fn rom_64mb() {
        passes("rom_64Mb.gb");
    }
}
