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

/// Mooneye's `emulator-only/mbc2`.
mod mbc2 {
    use super::assert_rom_passes;

    fn passes(name: &str) {
        assert_rom_passes(
            &format!("mooneye-test-suite/emulator-only/mbc2/{name}"),
            600,
        );
    }

    #[test]
    fn bits_ramg() {
        passes("bits_ramg.gb");
    }

    #[test]
    fn bits_romb() {
        passes("bits_romb.gb");
    }

    #[test]
    fn bits_unused() {
        passes("bits_unused.gb");
    }

    #[test]
    fn ram() {
        passes("ram.gb");
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
}

/// rtc3test: the MBC3 clock, timed against the Game Boy's own timer. Each
/// subtest is chosen from a menu with the buttons and draws PASS or FAIL
/// for every check; the screen must match the reference. The ROM supports
/// the Color, so the Color references apply.
mod rtc3test {
    use glassbit_runner::assert_scripted_screen_matches;

    const ROM: &str = "rtc3test/rtc3test.gb";

    #[test]
    fn basic_tests() {
        assert_scripted_screen_matches(
            ROM,
            "a@30-36",
            900,
            "rtc3test/rtc3test-basic-tests-cgb.png",
        );
    }

    #[test]
    fn range_tests() {
        assert_scripted_screen_matches(
            ROM,
            "down@30-36,a@50-56",
            700,
            "rtc3test/rtc3test-range-tests-cgb.png",
        );
    }

    #[test]
    fn sub_second_writes() {
        assert_scripted_screen_matches(
            ROM,
            "down@30-36,down@50-56,a@70-76",
            1800,
            "rtc3test/rtc3test-sub-second-writes-cgb.png",
        );
    }
}
