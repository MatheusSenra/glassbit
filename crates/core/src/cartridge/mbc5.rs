//! MBC5: up to 8 MiB of ROM and 128 KiB of RAM.
//!
//! | Written at        | Meaning                                  |
//! | ----------------- | ---------------------------------------- |
//! | `0x0000..=0x1FFF` | `0x0A` in the low nibble enables RAM     |
//! | `0x2000..=0x2FFF` | Low 8 bits of the ROM bank               |
//! | `0x3000..=0x3FFF` | Bit 8 of the ROM bank                    |
//! | `0x4000..=0x5FFF` | RAM bank, 4 bits                         |
//!
//! Unlike the earlier mappers, bank 0 is a valid choice for the switchable
//! window. On cartridges with a rumble motor, bit 3 of the RAM bank register
//! drives the motor instead of selecting a bank; the motor is ignored.
//!
//! Reference: <https://gbdev.io/pandocs/MBC5.html>

use super::Mbc;
use crate::state::{Reader, StateError, Writer};

const ROM_BANK_SIZE: usize = 0x4000;
const RAM_BANK_SIZE: usize = 0x2000;
const RAM_START: u16 = 0xA000;

pub struct Mbc5 {
    rom: Vec<u8>,
    ram: Vec<u8>,
    has_rumble: bool,
    ram_enabled: bool,
    /// 9 bits.
    rom_bank: u16,
    ram_bank: u8,
}

impl Mbc5 {
    /// `rom` must be a power-of-two size of at least 32 KiB.
    pub fn new(rom: Vec<u8>, ram_size: usize, has_rumble: bool) -> Self {
        Self {
            rom,
            ram: vec![0; ram_size],
            has_rumble,
            ram_enabled: false,
            rom_bank: 1,
            ram_bank: 0,
        }
    }

    fn ram_index(&self, addr: u16) -> Option<usize> {
        if !self.ram_enabled || self.ram.is_empty() {
            return None;
        }
        let offset = self.ram_bank as usize * RAM_BANK_SIZE + (addr - RAM_START) as usize;
        Some(offset % self.ram.len())
    }
}

impl Mbc for Mbc5 {
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
        match addr {
            0x0000..=0x1FFF => self.ram_enabled = value & 0x0F == 0x0A,
            0x2000..=0x2FFF => self.rom_bank = self.rom_bank & 0x100 | value as u16,
            0x3000..=0x3FFF => self.rom_bank = self.rom_bank & 0x0FF | (value as u16 & 1) << 8,
            0x4000..=0x5FFF => {
                let mask = if self.has_rumble { 0x07 } else { 0x0F };
                self.ram_bank = value & mask;
            }
            _ => {}
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

    fn rom_bank(&self) -> u16 {
        self.rom_bank
    }

    fn ram(&self) -> &[u8] {
        &self.ram
    }

    fn ram_mut(&mut self) -> &mut [u8] {
        &mut self.ram
    }

    fn save_state(&self, w: &mut Writer) {
        w.bool(self.ram_enabled);
        w.u16(self.rom_bank);
        w.u8(self.ram_bank);
        w.bytes(&self.ram);
    }

    fn load_state(&mut self, r: &mut Reader) -> Result<(), StateError> {
        self.ram_enabled = r.bool()?;
        self.rom_bank = r.u16()? & 0x01FF;
        self.ram_bank = r.u8()? & 0x0F;
        r.bytes(&mut self.ram)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A ROM where every bank starts with its own number, as two bytes.
    fn mbc5(rom_banks: usize, ram_size: usize, has_rumble: bool) -> Mbc5 {
        let mut rom = vec![0u8; rom_banks * ROM_BANK_SIZE];
        for bank in 0..rom_banks {
            rom[bank * ROM_BANK_SIZE..][..2].copy_from_slice(&(bank as u16).to_le_bytes());
        }
        Mbc5::new(rom, ram_size, has_rumble)
    }

    fn bank_at_4000(cart: &Mbc5) -> u16 {
        u16::from_le_bytes([cart.read_rom(0x4000), cart.read_rom(0x4001)])
    }

    #[test]
    fn rom_bank_has_nine_bits_split_over_two_registers() {
        let mut cart = mbc5(512, 0, false);
        assert_eq!(bank_at_4000(&cart), 1);
        cart.write_rom(0x2000, 0xFF);
        assert_eq!(bank_at_4000(&cart), 0x0FF);
        cart.write_rom(0x3000, 0x01);
        assert_eq!(bank_at_4000(&cart), 0x1FF);
        cart.write_rom(0x2000, 0x34);
        assert_eq!(bank_at_4000(&cart), 0x134);
        assert_eq!(cart.rom_bank(), 0x134);
    }

    #[test]
    fn bank_zero_can_be_selected() {
        let mut cart = mbc5(8, 0, false);
        cart.write_rom(0x2000, 0x00);
        assert_eq!(bank_at_4000(&cart), 0);
    }

    #[test]
    fn bank_number_wraps_to_the_rom_size() {
        let mut cart = mbc5(8, 0, false);
        cart.write_rom(0x2000, 0x0B);
        assert_eq!(bank_at_4000(&cart), 3);
    }

    #[test]
    fn ram_has_sixteen_banks_behind_an_enable_switch() {
        let mut cart = mbc5(4, 0x2_0000, false);
        cart.write_ram(0xA000, 0x11);
        assert_eq!(cart.read_ram(0xA000), 0xFF, "disabled at power on");

        cart.write_rom(0x0000, 0x0A);
        cart.write_rom(0x4000, 0x0F);
        cart.write_ram(0xBFFF, 0x77);
        cart.write_rom(0x4000, 0x00);
        assert_eq!(cart.read_ram(0xBFFF), 0x00);
        cart.write_rom(0x4000, 0x0F);
        assert_eq!(cart.read_ram(0xBFFF), 0x77);
        assert_eq!(cart.ram()[0x1_FFFF], 0x77);
    }

    #[test]
    fn rumble_bit_does_not_select_a_ram_bank() {
        let mut cart = mbc5(4, 0x8000, true);
        cart.write_rom(0x0000, 0x0A);
        cart.write_ram(0xA000, 0x55);
        cart.write_rom(0x4000, 0x08);
        assert_eq!(cart.read_ram(0xA000), 0x55, "still bank 0");
    }
}
