//! MBC3: up to 2 MiB of ROM, 32 KiB of RAM and an optional real-time clock.
//! Cartridges larger than that carry the MBC30 variant, which has one more
//! ROM bank bit and twice the RAM banks; it is told apart by the sizes.
//!
//! | Written at        | Meaning                                              |
//! | ----------------- | ---------------------------------------------------- |
//! | `0x0000..=0x1FFF` | `0x0A` in the low nibble enables RAM and the clock   |
//! | `0x2000..=0x3FFF` | ROM bank, 7 bits (8 on MBC30); 0 becomes 1           |
//! | `0x4000..=0x5FFF` | `0x00-0x03`: RAM bank (`-0x07` on MBC30); `0x08-0x0C`: a clock register |
//! | `0x6000..=0x7FFF` | Latches the clock (ignored here)                     |
//!
//! The clock registers exist and can be read and written, but **the clock
//! does not tick**: the core has no notion of wall-clock time yet. Games
//! that use it still run; their in-game time just stands still.
//!
//! Reference: <https://gbdev.io/pandocs/MBC3.html>

use super::Mbc;
use crate::state::{Reader, StateError, Writer};

const ROM_BANK_SIZE: usize = 0x4000;
const RAM_BANK_SIZE: usize = 0x2000;
const RAM_START: u16 = 0xA000;

/// What the `0xA000..=0xBFFF` window currently shows.
#[derive(Clone, Copy)]
enum Selected {
    RamBank(u8),
    /// One of the five clock registers: seconds, minutes, hours, day low,
    /// day high and flags.
    Clock(usize),
    Nothing,
}

pub struct Mbc3 {
    rom: Vec<u8>,
    ram: Vec<u8>,
    has_clock: bool,
    clock: [u8; 5],
    enabled: bool,
    /// 7 bits (8 on MBC30), never zero.
    rom_bank: u8,
    selected: Selected,
}

impl Mbc3 {
    /// `rom` must be a power-of-two size of at least 32 KiB.
    pub fn new(rom: Vec<u8>, ram_size: usize, has_clock: bool) -> Self {
        Self {
            rom,
            ram: vec![0; ram_size],
            has_clock,
            clock: [0; 5],
            enabled: false,
            rom_bank: 1,
            selected: Selected::RamBank(0),
        }
    }

    /// More than 2 MiB of ROM means the extra bank bit of an MBC30.
    fn rom_bank_mask(&self) -> u8 {
        if self.rom.len() > 128 * ROM_BANK_SIZE {
            0xFF
        } else {
            0x7F
        }
    }

    /// More than 32 KiB of RAM means the eight RAM banks of an MBC30.
    fn last_ram_bank(&self) -> u8 {
        if self.ram.len() > 4 * RAM_BANK_SIZE {
            7
        } else {
            3
        }
    }

    /// Applies a write to the RAM bank / clock select register.
    fn select(&mut self, value: u8) {
        self.selected = match value {
            bank if bank <= self.last_ram_bank() => Selected::RamBank(bank),
            0x08..=0x0C if self.has_clock => Selected::Clock(value as usize - 0x08),
            _ => Selected::Nothing,
        }
    }

    fn ram_index(&self, bank: u8, addr: u16) -> Option<usize> {
        if self.ram.is_empty() {
            return None;
        }
        let offset = bank as usize * RAM_BANK_SIZE + (addr - RAM_START) as usize;
        Some(offset % self.ram.len())
    }
}

impl Mbc for Mbc3 {
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
            0x0000..=0x1FFF => self.enabled = value & 0x0F == 0x0A,
            0x2000..=0x3FFF => self.rom_bank = (value & self.rom_bank_mask()).max(1),
            0x4000..=0x5FFF => self.select(value),
            // Latching copies the running clock into the registers; with a
            // clock that does not tick there is nothing to copy.
            _ => {}
        }
    }

    fn read_ram(&self, addr: u16) -> u8 {
        if !self.enabled {
            return 0xFF;
        }
        match self.selected {
            Selected::RamBank(bank) => match self.ram_index(bank, addr) {
                Some(index) => self.ram[index],
                None => 0xFF,
            },
            Selected::Clock(register) => self.clock[register],
            Selected::Nothing => 0xFF,
        }
    }

    fn write_ram(&mut self, addr: u16, value: u8) {
        if !self.enabled {
            return;
        }
        match self.selected {
            Selected::RamBank(bank) => {
                if let Some(index) = self.ram_index(bank, addr) {
                    self.ram[index] = value;
                }
            }
            Selected::Clock(register) => self.clock[register] = value,
            Selected::Nothing => {}
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
        w.bool(self.enabled);
        w.u8(self.rom_bank);
        // Stored as the register value that produces the selection.
        w.u8(match self.selected {
            Selected::RamBank(bank) => bank,
            Selected::Clock(register) => 0x08 + register as u8,
            Selected::Nothing => 0xFF,
        });
        w.bytes(&self.clock);
        w.bytes(&self.ram);
    }

    fn load_state(&mut self, r: &mut Reader) -> Result<(), StateError> {
        self.enabled = r.bool()?;
        self.rom_bank = (r.u8()? & self.rom_bank_mask()).max(1);
        let selection = r.u8()?;
        self.select(selection);
        r.bytes(&mut self.clock)?;
        r.bytes(&mut self.ram)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A ROM where every byte holds the number of the bank it belongs to.
    fn mbc3(rom_banks: usize, ram_size: usize, has_clock: bool) -> Mbc3 {
        let rom = (0..rom_banks * ROM_BANK_SIZE)
            .map(|i| (i / ROM_BANK_SIZE) as u8)
            .collect();
        Mbc3::new(rom, ram_size, has_clock)
    }

    #[test]
    fn rom_bank_uses_seven_bits_and_zero_becomes_one() {
        let mut cart = mbc3(128, 0, false);
        assert_eq!(cart.read_rom(0x4000), 1);
        cart.write_rom(0x2000, 0x7F);
        assert_eq!(cart.read_rom(0x4000), 0x7F);
        // Unlike MBC1, bank 0x20 is reachable.
        cart.write_rom(0x2000, 0x20);
        assert_eq!(cart.read_rom(0x4000), 0x20);
        cart.write_rom(0x2000, 0x80);
        assert_eq!(cart.read_rom(0x4000), 1, "bit 7 is ignored");
        assert_eq!(cart.read_rom(0x0000), 0, "fixed window stays put");
    }

    #[test]
    fn four_mebibyte_cartridges_get_the_eighth_bank_bit() {
        let mut cart = mbc3(256, 0, false);
        cart.write_rom(0x2000, 0xFF);
        assert_eq!(cart.read_rom(0x4000), 0xFF);
        cart.write_rom(0x2000, 0x80);
        assert_eq!(cart.read_rom(0x4000), 0x80);
    }

    #[test]
    fn large_ram_gets_eight_banks() {
        let mut cart = mbc3(4, 0x1_0000, false);
        cart.write_rom(0x0000, 0x0A);
        cart.write_rom(0x4000, 0x07);
        cart.write_ram(0xA000, 0x77);
        assert_eq!(cart.ram()[0xE000], 0x77);

        // On a plain MBC3 the same selection is not a RAM bank.
        let mut cart = mbc3(4, 0x8000, false);
        cart.write_rom(0x0000, 0x0A);
        cart.write_rom(0x4000, 0x07);
        assert_eq!(cart.read_ram(0xA000), 0xFF);
    }

    #[test]
    fn bank_number_wraps_to_the_rom_size() {
        let mut cart = mbc3(4, 0, false);
        cart.write_rom(0x2000, 6);
        assert_eq!(cart.read_rom(0x4000), 2);
        assert_eq!(cart.rom_bank(), 6);
    }

    #[test]
    fn ram_has_four_banks_behind_an_enable_switch() {
        let mut cart = mbc3(4, 0x8000, false);
        cart.write_ram(0xA000, 0x11);
        assert_eq!(cart.read_ram(0xA000), 0xFF, "disabled at power on");

        cart.write_rom(0x0000, 0x0A);
        cart.write_ram(0xA000, 0x11);
        cart.write_rom(0x4000, 0x03);
        cart.write_ram(0xA000, 0x33);
        assert_eq!(cart.read_ram(0xA000), 0x33);
        cart.write_rom(0x4000, 0x00);
        assert_eq!(cart.read_ram(0xA000), 0x11);
        assert_eq!(cart.ram()[0x6000], 0x33);
    }

    #[test]
    fn clock_registers_hold_values_when_the_cartridge_has_a_clock() {
        let mut cart = mbc3(4, 0x2000, true);
        cart.write_rom(0x0000, 0x0A);
        cart.write_rom(0x4000, 0x08);
        cart.write_ram(0xA000, 42);
        assert_eq!(cart.read_ram(0xA000), 42);
        cart.write_rom(0x4000, 0x00);
        assert_eq!(cart.read_ram(0xA000), 0x00, "RAM was not touched");
    }

    #[test]
    fn clock_selection_reads_open_bus_without_a_clock() {
        let mut cart = mbc3(4, 0x2000, false);
        cart.write_rom(0x0000, 0x0A);
        cart.write_rom(0x4000, 0x08);
        cart.write_ram(0xA000, 42);
        assert_eq!(cart.read_ram(0xA000), 0xFF);
    }
}
