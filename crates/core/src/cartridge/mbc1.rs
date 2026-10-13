//! MBC1, ROM banking only.
//!
//! This covers cartridges of up to 512 KiB with no RAM, which is what the
//! Blargg test ROMs need. The second banking register, the mode switch and
//! RAM are not implemented yet; cartridges that need them are rejected when
//! loading.
//!
//! Reference: <https://gbdev.io/pandocs/MBC1.html>

use super::Mbc;

const BANK_SIZE: usize = 0x4000;

/// Largest ROM reachable with the 5-bit bank register alone.
pub const MAX_ROM_SIZE: usize = 32 * BANK_SIZE;

pub struct Mbc1 {
    rom: Vec<u8>,
    /// The bank mapped at `0x4000..=0x7FFF`.
    rom_bank: u8,
}

impl Mbc1 {
    /// `rom` must be a power-of-two size between 32 KiB and [`MAX_ROM_SIZE`].
    pub fn new(rom: Vec<u8>) -> Self {
        Self { rom, rom_bank: 1 }
    }

    fn bank_count(&self) -> usize {
        self.rom.len() / BANK_SIZE
    }
}

impl Mbc for Mbc1 {
    fn read_rom(&self, addr: u16) -> u8 {
        match addr {
            0x0000..=0x3FFF => self.rom[addr as usize],
            _ => {
                // Bank bits the ROM is too small to use are ignored.
                let bank = self.rom_bank as usize % self.bank_count();
                self.rom[bank * BANK_SIZE + (addr as usize - BANK_SIZE)]
            }
        }
    }

    fn write_rom(&mut self, addr: u16, value: u8) {
        if let 0x2000..=0x3FFF = addr {
            // The register is 5 bits wide and cannot hold zero: the chip
            // turns a 0 into a 1, so bank 0 never appears in this window.
            self.rom_bank = match value & 0x1F {
                0 => 1,
                bank => bank,
            };
        }
    }

    fn read_ram(&self, _addr: u16) -> u8 {
        0xFF
    }

    fn write_ram(&mut self, _addr: u16, _value: u8) {}
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A ROM where every byte holds the number of the bank it belongs to.
    fn mbc1(banks: usize) -> Mbc1 {
        let rom = (0..banks * BANK_SIZE)
            .map(|i| (i / BANK_SIZE) as u8)
            .collect();
        Mbc1::new(rom)
    }

    #[test]
    fn starts_with_bank_1_in_the_switchable_window() {
        let cart = mbc1(4);
        assert_eq!(cart.read_rom(0x0000), 0);
        assert_eq!(cart.read_rom(0x4000), 1);
        assert_eq!(cart.read_rom(0x7FFF), 1);
    }

    #[test]
    fn switches_banks() {
        let mut cart = mbc1(8);
        cart.write_rom(0x2000, 5);
        assert_eq!(cart.read_rom(0x4000), 5);
        cart.write_rom(0x3FFF, 7);
        assert_eq!(cart.read_rom(0x7FFF), 7);
        assert_eq!(cart.read_rom(0x0000), 0, "fixed window never moves");
    }

    #[test]
    fn bank_0_selects_bank_1() {
        let mut cart = mbc1(4);
        cart.write_rom(0x2000, 3);
        cart.write_rom(0x2000, 0);
        assert_eq!(cart.read_rom(0x4000), 1);
    }

    #[test]
    fn bank_number_wraps_to_the_rom_size() {
        let mut cart = mbc1(4);
        cart.write_rom(0x2000, 6);
        assert_eq!(cart.read_rom(0x4000), 2);
    }

    #[test]
    fn only_the_low_five_bits_count() {
        let mut cart = mbc1(32);
        cart.write_rom(0x2000, 0xE3);
        assert_eq!(cart.read_rom(0x4000), 3);
    }

    #[test]
    fn writes_outside_the_bank_register_are_ignored() {
        let mut cart = mbc1(4);
        cart.write_rom(0x0000, 3);
        cart.write_rom(0x4000, 3);
        cart.write_rom(0x6000, 3);
        assert_eq!(cart.read_rom(0x4000), 1);
    }
}
