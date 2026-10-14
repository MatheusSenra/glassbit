//! MBC2: up to 256 KiB of ROM and 512 half-bytes of RAM inside the chip.
//!
//! Both registers live in `0x0000..=0x3FFF`; bit 8 of the address picks
//! which one a write goes to:
//!
//! | Address bit 8 | Register | Meaning                                  |
//! | ------------- | -------- | ---------------------------------------- |
//! | 0             | RAMG     | `0x0A` in the low nibble enables RAM     |
//! | 1             | ROMB     | ROM bank, 4 bits (0 becomes 1)           |
//!
//! The RAM stores only the low 4 bits of each byte; the high bits read as 1.
//! It sits at `0xA000..=0xA1FF` and repeats up to `0xBFFF`.
//!
//! Reference: <https://gbdev.io/pandocs/MBC2.html>

use super::Mbc;
use crate::state::{Reader, StateError, Writer};

const ROM_BANK_SIZE: usize = 0x4000;
const RAM_SIZE: usize = 512;

pub struct Mbc2 {
    rom: Vec<u8>,
    /// One 4-bit value per byte, kept in the low nibble.
    ram: Vec<u8>,
    ram_enabled: bool,
    /// 4 bits, never zero.
    rom_bank: u8,
}

impl Mbc2 {
    /// `rom` must be a power-of-two size of at least 32 KiB.
    pub fn new(rom: Vec<u8>) -> Self {
        Self {
            rom,
            ram: vec![0; RAM_SIZE],
            ram_enabled: false,
            rom_bank: 1,
        }
    }

    fn ram_index(addr: u16) -> usize {
        addr as usize & (RAM_SIZE - 1)
    }
}

impl Mbc for Mbc2 {
    fn read_rom(&self, addr: u16) -> u8 {
        match addr {
            0x0000..=0x3FFF => self.rom[addr as usize],
            _ => {
                let bank = self.rom_bank as usize % (self.rom.len() / ROM_BANK_SIZE);
                self.rom[bank * ROM_BANK_SIZE + (addr as usize - ROM_BANK_SIZE)]
            }
        }
    }

    fn write_rom(&mut self, addr: u16, value: u8) {
        if addr >= 0x4000 {
            return;
        }
        if addr & 0x0100 == 0 {
            self.ram_enabled = value & 0x0F == 0x0A;
        } else {
            self.rom_bank = (value & 0x0F).max(1);
        }
    }

    fn read_ram(&self, addr: u16) -> u8 {
        if self.ram_enabled {
            0xF0 | self.ram[Self::ram_index(addr)]
        } else {
            0xFF
        }
    }

    fn write_ram(&mut self, addr: u16, value: u8) {
        if self.ram_enabled {
            self.ram[Self::ram_index(addr)] = value & 0x0F;
        }
    }

    fn rom_bank(&self) -> u16 {
        self.rom_bank as u16
    }

    fn ram(&self) -> &[u8] {
        &self.ram
    }

    fn ram_mut(&mut self) -> &mut [u8] {
        &mut self.ram
    }

    fn save_state(&self, w: &mut Writer) {
        w.bool(self.ram_enabled);
        w.u8(self.rom_bank);
        w.bytes(&self.ram);
    }

    fn load_state(&mut self, r: &mut Reader) -> Result<(), StateError> {
        self.ram_enabled = r.bool()?;
        self.rom_bank = (r.u8()? & 0x0F).max(1);
        r.bytes(&mut self.ram)?;
        for value in &mut self.ram {
            *value &= 0x0F;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A ROM where every byte holds the number of the bank it belongs to.
    fn mbc2(banks: usize) -> Mbc2 {
        let rom = (0..banks * ROM_BANK_SIZE)
            .map(|i| (i / ROM_BANK_SIZE) as u8)
            .collect();
        Mbc2::new(rom)
    }

    #[test]
    fn address_bit_8_chooses_the_register() {
        let mut cart = mbc2(16);
        cart.write_rom(0x2100, 5);
        assert_eq!(cart.read_rom(0x4000), 5);
        // Bit 8 clear: this is the RAM enable, not the bank.
        cart.write_rom(0x2000, 7);
        assert_eq!(cart.read_rom(0x4000), 5);
        cart.write_rom(0x0100, 0);
        assert_eq!(cart.read_rom(0x4000), 1, "bank 0 becomes 1");
        cart.write_rom(0x3FFF, 0x1F);
        assert_eq!(cart.read_rom(0x4000), 0x0F, "4 bits");
    }

    #[test]
    fn ram_holds_half_bytes_and_repeats() {
        let mut cart = mbc2(2);
        cart.write_ram(0xA000, 0x5A);
        assert_eq!(cart.read_ram(0xA000), 0xFF, "disabled at power on");
        cart.write_rom(0x0000, 0x0A);
        cart.write_ram(0xA000, 0x5A);
        assert_eq!(cart.read_ram(0xA000), 0xFA, "upper bits read as 1");
        assert_eq!(cart.read_ram(0xA200), 0xFA, "512 bytes repeat");
        assert_eq!(cart.read_ram(0xBE00), 0xFA);
        assert_eq!(cart.ram()[0], 0x0A);
    }
}
