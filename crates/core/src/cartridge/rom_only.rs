//! Cartridges with no mapper: 32 KiB of ROM mapped directly, plus optional RAM.

use super::Mbc;
use crate::state::{Reader, StateError, Writer};

const RAM_START: u16 = 0xA000;

pub struct RomOnly {
    rom: Vec<u8>,
    ram: Vec<u8>,
}

impl RomOnly {
    /// `rom` must hold at least 32 KiB.
    pub fn new(rom: Vec<u8>, ram_size: usize) -> Self {
        Self {
            rom,
            ram: vec![0; ram_size],
        }
    }

    fn ram_index(&self, addr: u16) -> Option<usize> {
        if self.ram.is_empty() {
            None
        } else {
            // A chip smaller than the 8 KiB window repeats across it.
            Some((addr - RAM_START) as usize % self.ram.len())
        }
    }
}

impl Mbc for RomOnly {
    fn read_rom(&self, addr: u16) -> u8 {
        self.rom[addr as usize]
    }

    fn write_rom(&mut self, _addr: u16, _value: u8) {
        // No mapper registers: writes to ROM do nothing.
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
        1
    }

    fn ram(&self) -> &[u8] {
        &self.ram
    }

    fn ram_mut(&mut self) -> &mut [u8] {
        &mut self.ram
    }

    fn save_state(&self, w: &mut Writer) {
        w.bytes(&self.ram);
    }

    fn load_state(&mut self, r: &mut Reader) -> Result<(), StateError> {
        r.bytes(&mut self.ram)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rom_is_read_only() {
        let mut rom = vec![0u8; 0x8000];
        rom[0x1234] = 0x42;
        let mut cart = RomOnly::new(rom, 0);
        cart.write_rom(0x1234, 0x99);
        assert_eq!(cart.read_rom(0x1234), 0x42);
    }

    #[test]
    fn missing_ram_reads_open_bus() {
        let mut cart = RomOnly::new(vec![0; 0x8000], 0);
        cart.write_ram(0xA000, 0x12);
        assert_eq!(cart.read_ram(0xA000), 0xFF);
    }

    #[test]
    fn ram_stores_values() {
        let mut cart = RomOnly::new(vec![0; 0x8000], 0x2000);
        cart.write_ram(0xA000, 0x12);
        cart.write_ram(0xBFFF, 0x34);
        assert_eq!(cart.read_ram(0xA000), 0x12);
        assert_eq!(cart.read_ram(0xBFFF), 0x34);
    }

    #[test]
    fn small_ram_mirrors_across_the_window() {
        let mut cart = RomOnly::new(vec![0; 0x8000], 0x800);
        cart.write_ram(0xA000, 0x56);
        assert_eq!(cart.read_ram(0xA800), 0x56);
    }
}
