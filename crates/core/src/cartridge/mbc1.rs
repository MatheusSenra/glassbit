//! MBC1: up to 2 MiB of ROM and 32 KiB of RAM.
//!
//! The chip has four registers, written through the ROM address space:
//!
//! | Written at        | Register | Meaning                                    |
//! | ----------------- | -------- | ------------------------------------------ |
//! | `0x0000..=0x1FFF` | RAMG     | `0x0A` in the low nibble enables RAM        |
//! | `0x2000..=0x3FFF` | BANK1    | Low 5 bits of the ROM bank (0 becomes 1)   |
//! | `0x4000..=0x5FFF` | BANK2    | 2 more bits: upper ROM bank bits or RAM bank |
//! | `0x6000..=0x7FFF` | MODE     | Whether BANK2 also affects `0x0000` and RAM |
//!
//! Multicart cartridges (MBC1M), which wire BANK2 differently, are not
//! supported.
//!
//! Reference: <https://gbdev.io/pandocs/MBC1.html>

use super::Mbc;

const ROM_BANK_SIZE: usize = 0x4000;
const RAM_BANK_SIZE: usize = 0x2000;
const RAM_START: u16 = 0xA000;

/// Largest ROM the 7 bank bits can address.
pub const MAX_ROM_SIZE: usize = 128 * ROM_BANK_SIZE;

pub struct Mbc1 {
    rom: Vec<u8>,
    ram: Vec<u8>,
    ram_enabled: bool,
    /// 5 bits, never zero.
    bank1: u8,
    /// 2 bits.
    bank2: u8,
    /// MODE register: `true` lets BANK2 reach the fixed ROM window and RAM.
    advanced_mode: bool,
}

impl Mbc1 {
    /// `rom` must be a power-of-two size between 32 KiB and [`MAX_ROM_SIZE`].
    pub fn new(rom: Vec<u8>, ram_size: usize) -> Self {
        Self {
            rom,
            ram: vec![0; ram_size],
            ram_enabled: false,
            bank1: 1,
            bank2: 0,
            advanced_mode: false,
        }
    }

    /// Reads ROM from `bank`; bank bits the ROM is too small to use are
    /// ignored.
    fn rom_byte(&self, bank: usize, offset: usize) -> u8 {
        let bank = bank % (self.rom.len() / ROM_BANK_SIZE);
        self.rom[bank * ROM_BANK_SIZE + offset]
    }

    fn ram_index(&self, addr: u16) -> Option<usize> {
        if !self.ram_enabled || self.ram.is_empty() {
            return None;
        }
        let bank = if self.advanced_mode {
            self.bank2 as usize
        } else {
            0
        };
        let offset = bank * RAM_BANK_SIZE + (addr - RAM_START) as usize;
        // Small chips repeat; missing bank bits are ignored.
        Some(offset % self.ram.len())
    }
}

impl Mbc for Mbc1 {
    fn read_rom(&self, addr: u16) -> u8 {
        match addr {
            0x0000..=0x3FFF => {
                let bank = if self.advanced_mode {
                    (self.bank2 as usize) << 5
                } else {
                    0
                };
                self.rom_byte(bank, addr as usize)
            }
            _ => {
                let bank = (self.bank2 as usize) << 5 | self.bank1 as usize;
                self.rom_byte(bank, addr as usize - ROM_BANK_SIZE)
            }
        }
    }

    fn write_rom(&mut self, addr: u16, value: u8) {
        match addr {
            0x0000..=0x1FFF => self.ram_enabled = value & 0x0F == 0x0A,
            // The register cannot hold zero: the chip turns a 0 into a 1,
            // looking only at these 5 bits. That is why bank 0x20 (whose low
            // bits are 0) can never be mapped here either.
            0x2000..=0x3FFF => self.bank1 = (value & 0x1F).max(1),
            0x4000..=0x5FFF => self.bank2 = value & 0x03,
            _ => self.advanced_mode = value & 0x01 != 0,
        }
    }

    fn read_ram(&self, addr: u16) -> u8 {
        match self.ram_index(addr) {
            Some(index) => self.ram[index],
            None => 0xFF,
        }
    }

    fn write_ram(&mut self, addr: u16, value: u8) {
        if let Some(index) = self.ram_index(addr) {
            self.ram[index] = value;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A ROM where every byte holds the number of the bank it belongs to.
    fn mbc1(rom_banks: usize, ram_size: usize) -> Mbc1 {
        let rom = (0..rom_banks * ROM_BANK_SIZE)
            .map(|i| (i / ROM_BANK_SIZE) as u8)
            .collect();
        Mbc1::new(rom, ram_size)
    }

    #[test]
    fn starts_with_bank_1_in_the_switchable_window() {
        let cart = mbc1(4, 0);
        assert_eq!(cart.read_rom(0x0000), 0);
        assert_eq!(cart.read_rom(0x4000), 1);
        assert_eq!(cart.read_rom(0x7FFF), 1);
    }

    #[test]
    fn switches_banks() {
        let mut cart = mbc1(8, 0);
        cart.write_rom(0x2000, 5);
        assert_eq!(cart.read_rom(0x4000), 5);
        cart.write_rom(0x3FFF, 7);
        assert_eq!(cart.read_rom(0x7FFF), 7);
        assert_eq!(cart.read_rom(0x0000), 0, "fixed window stays put");
    }

    #[test]
    fn bank_0_selects_bank_1() {
        let mut cart = mbc1(4, 0);
        cart.write_rom(0x2000, 3);
        cart.write_rom(0x2000, 0);
        assert_eq!(cart.read_rom(0x4000), 1);
    }

    #[test]
    fn bank_number_wraps_to_the_rom_size() {
        let mut cart = mbc1(4, 0);
        cart.write_rom(0x2000, 6);
        assert_eq!(cart.read_rom(0x4000), 2);
    }

    #[test]
    fn bank2_supplies_the_upper_bank_bits() {
        let mut cart = mbc1(128, 0);
        cart.write_rom(0x2000, 0x04);
        cart.write_rom(0x4000, 0x02);
        assert_eq!(cart.read_rom(0x4000), 0x44);
        // Bank 0x20 is unreachable: its low bits are zero, so it becomes 0x21.
        cart.write_rom(0x2000, 0x00);
        cart.write_rom(0x4000, 0x01);
        assert_eq!(cart.read_rom(0x4000), 0x21);
    }

    #[test]
    fn advanced_mode_moves_the_fixed_window_too() {
        let mut cart = mbc1(128, 0);
        cart.write_rom(0x4000, 0x03);
        assert_eq!(cart.read_rom(0x0000), 0x00);
        cart.write_rom(0x6000, 0x01);
        assert_eq!(cart.read_rom(0x0000), 0x60);
    }

    #[test]
    fn ram_must_be_enabled() {
        let mut cart = mbc1(4, 0x2000);
        cart.write_ram(0xA000, 0x12);
        assert_eq!(cart.read_ram(0xA000), 0xFF, "disabled at power on");

        cart.write_rom(0x0000, 0x0A);
        cart.write_ram(0xA000, 0x12);
        assert_eq!(cart.read_ram(0xA000), 0x12);

        cart.write_rom(0x0000, 0x00);
        assert_eq!(cart.read_ram(0xA000), 0xFF);
        cart.write_rom(0x0000, 0x1A);
        assert_eq!(cart.read_ram(0xA000), 0x12, "only the low nibble counts");
    }

    #[test]
    fn ram_banks_switch_only_in_advanced_mode() {
        let mut cart = mbc1(4, 0x8000);
        cart.write_rom(0x0000, 0x0A);
        cart.write_ram(0xA000, 0x11);

        cart.write_rom(0x4000, 0x02);
        assert_eq!(cart.read_ram(0xA000), 0x11, "still bank 0");
        cart.write_rom(0x6000, 0x01);
        assert_eq!(cart.read_ram(0xA000), 0x00, "now bank 2");
        cart.write_ram(0xA000, 0x22);

        cart.write_rom(0x6000, 0x00);
        assert_eq!(cart.read_ram(0xA000), 0x11);
    }

    #[test]
    fn small_ram_mirrors_and_missing_ram_reads_open_bus() {
        let mut cart = mbc1(4, 0x800);
        cart.write_rom(0x0000, 0x0A);
        cart.write_ram(0xA000, 0x56);
        assert_eq!(cart.read_ram(0xA800), 0x56);

        let mut cart = mbc1(4, 0);
        cart.write_rom(0x0000, 0x0A);
        cart.write_ram(0xA000, 0x56);
        assert_eq!(cart.read_ram(0xA000), 0xFF);
    }
}
