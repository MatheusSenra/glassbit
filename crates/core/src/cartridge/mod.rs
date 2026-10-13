//! Cartridges: the header and the memory bank controllers (MBCs).

mod header;
mod mbc1;
mod mbc3;
mod mbc5;
mod rom_only;

use std::fmt;

use crate::state::{Reader, StateError, Writer};

pub use header::{CartridgeType, CgbSupport, Header, MapperKind};
use mbc1::Mbc1;
use mbc3::Mbc3;
use mbc5::Mbc5;
use rom_only::RomOnly;

/// A memory bank controller: the chip that decides what the CPU sees in the
/// cartridge's two address windows.
///
/// All methods take the address as the CPU sees it.
pub trait Mbc {
    /// Reads from `0x0000..=0x7FFF`.
    fn read_rom(&self, addr: u16) -> u8;
    /// Writes to `0x0000..=0x7FFF`. ROM cannot change; mappers use these
    /// writes as their control registers.
    fn write_rom(&mut self, addr: u16, value: u8);
    /// Reads from `0xA000..=0xBFFF`.
    fn read_ram(&self, addr: u16) -> u8;
    /// Writes to `0xA000..=0xBFFF`.
    fn write_ram(&mut self, addr: u16, value: u8);
    /// The bank number selected for `0x4000..=0x7FFF`, as the program set it.
    fn rom_bank(&self) -> u16;
    /// The whole RAM chip, all banks; empty when there is none. This is
    /// what a battery keeps alive.
    fn ram(&self) -> &[u8];
    fn ram_mut(&mut self) -> &mut [u8];
    /// Writes the mapper's registers and RAM for a save state.
    fn save_state(&self, w: &mut Writer);
    /// Reads back what [`Mbc::save_state`] wrote.
    fn load_state(&mut self, r: &mut Reader) -> Result<(), StateError>;
}

/// Why a ROM image could not be turned into a cartridge.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CartridgeError {
    /// The image is too short to contain a header.
    TooSmall { len: usize },
    /// The ROM size byte (`0x148`) is not a documented value.
    InvalidRomSize(u8),
    /// The RAM size byte (`0x149`) is not a documented value.
    InvalidRamSize(u8),
    /// The image is shorter than the size its header declares.
    Truncated { declared: usize, actual: usize },
    /// The cartridge type byte (`0x147`) names a mapper that is not implemented.
    UnsupportedMapper(u8),
}

impl fmt::Display for CartridgeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match *self {
            Self::TooSmall { len } => {
                write!(f, "ROM is {len} bytes, too small to hold a header")
            }
            Self::InvalidRomSize(code) => write!(f, "invalid ROM size code {code:#04x}"),
            Self::InvalidRamSize(code) => write!(f, "invalid RAM size code {code:#04x}"),
            Self::Truncated { declared, actual } => write!(
                f,
                "header declares {declared} bytes of ROM but the file has {actual}"
            ),
            Self::UnsupportedMapper(code) => {
                write!(f, "unsupported cartridge type {code:#04x}")
            }
        }
    }
}

impl std::error::Error for CartridgeError {}

/// A cartridge: its parsed header plus the mapper that serves its memory.
pub struct Cartridge {
    header: Header,
    mbc: Box<dyn Mbc>,
    /// RAM was written since the frontend last asked.
    ram_dirty: bool,
}

impl Cartridge {
    /// Builds a cartridge from a ROM image.
    pub fn new(rom: &[u8]) -> Result<Self, CartridgeError> {
        let header = Header::parse(rom)?;
        if rom.len() < header.rom_size {
            return Err(CartridgeError::Truncated {
                declared: header.rom_size,
                actual: rom.len(),
            });
        }

        let ram_size = if header.cartridge_type.ram {
            header.ram_size
        } else {
            0
        };
        let kind = header.cartridge_type;
        let image = rom[..header.rom_size].to_vec();
        let mbc: Box<dyn Mbc> = match kind.mapper {
            MapperKind::RomOnly => Box::new(RomOnly::new(image, ram_size)),
            MapperKind::Mbc1 if header.rom_size <= mbc1::MAX_ROM_SIZE => {
                Box::new(Mbc1::new(image, ram_size))
            }
            MapperKind::Mbc3 => Box::new(Mbc3::new(image, ram_size, kind.timer)),
            MapperKind::Mbc5 => Box::new(Mbc5::new(image, ram_size, kind.rumble)),
            _ => {
                return Err(CartridgeError::UnsupportedMapper(
                    header.cartridge_type.code,
                ))
            }
        };

        Ok(Self {
            header,
            mbc,
            ram_dirty: false,
        })
    }

    pub fn header(&self) -> &Header {
        &self.header
    }

    /// Reads from `0x0000..=0x7FFF`.
    pub fn read_rom(&self, addr: u16) -> u8 {
        self.mbc.read_rom(addr)
    }

    /// Writes to `0x0000..=0x7FFF` (mapper control registers).
    pub fn write_rom(&mut self, addr: u16, value: u8) {
        self.mbc.write_rom(addr, value);
    }

    /// Reads from `0xA000..=0xBFFF`.
    pub fn read_ram(&self, addr: u16) -> u8 {
        self.mbc.read_ram(addr)
    }

    /// Writes to `0xA000..=0xBFFF`.
    pub fn write_ram(&mut self, addr: u16, value: u8) {
        self.mbc.write_ram(addr, value);
        self.ram_dirty = true;
    }

    /// The bank number selected for `0x4000..=0x7FFF`.
    pub fn rom_bank(&self) -> u16 {
        self.mbc.rom_bank()
    }

    /// Whether the cartridge keeps its RAM when powered off, i.e. whether
    /// there is anything worth saving.
    pub fn has_battery(&self) -> bool {
        self.header.cartridge_type.battery && !self.mbc.ram().is_empty()
    }

    /// The battery-backed RAM, to be stored by the frontend. `None` when
    /// the cartridge has no battery.
    pub fn save_data(&self) -> Option<&[u8]> {
        self.has_battery().then(|| self.mbc.ram())
    }

    /// Restores battery-backed RAM from a previous [`Cartridge::save_data`].
    /// Data of the wrong size is accepted: what fits is copied. Returns
    /// `false` when the cartridge has no battery.
    pub fn load_save(&mut self, data: &[u8]) -> bool {
        if !self.has_battery() {
            return false;
        }
        let ram = self.mbc.ram_mut();
        let len = ram.len().min(data.len());
        ram[..len].copy_from_slice(&data[..len]);
        true
    }

    pub(crate) fn save_state(&self, w: &mut Writer) {
        self.mbc.save_state(w);
    }

    pub(crate) fn load_state(&mut self, r: &mut Reader) -> Result<(), StateError> {
        self.mbc.load_state(r)?;
        // The RAM just changed under the frontend's feet: have it stored.
        self.ram_dirty = true;
        Ok(())
    }

    /// Returns `true` if battery-backed RAM was written since the last call.
    pub fn take_save_dirty(&mut self) -> bool {
        std::mem::take(&mut self.ram_dirty) && self.has_battery()
    }
}

#[cfg(test)]
pub(crate) mod test_util {
    use super::header::{compute_global_checksum, compute_header_checksum};

    /// Builds synthetic ROM images with a valid header for tests.
    pub struct RomBuilder {
        title: String,
        cartridge_type: u8,
        rom_size_code: u8,
        ram_size_code: u8,
    }

    impl RomBuilder {
        pub fn new() -> Self {
            Self {
                title: "TEST".to_string(),
                cartridge_type: 0x00,
                rom_size_code: 0,
                ram_size_code: 0,
            }
        }

        pub fn title(mut self, title: &str) -> Self {
            self.title = title.to_string();
            self
        }

        pub fn cartridge_type(mut self, code: u8) -> Self {
            self.cartridge_type = code;
            self
        }

        pub fn rom_size_code(mut self, code: u8) -> Self {
            self.rom_size_code = code;
            self
        }

        pub fn ram_size_code(mut self, code: u8) -> Self {
            self.ram_size_code = code;
            self
        }

        /// The image is filled with a pattern (low byte of the address) so
        /// reads can be checked, then the header and checksums are written.
        pub fn build(self) -> Vec<u8> {
            let mut rom: Vec<u8> = (0..0x8000usize << self.rom_size_code)
                .map(|i| i as u8)
                .collect();
            rom[0x134..0x150].fill(0);
            rom[0x134..0x134 + self.title.len()].copy_from_slice(self.title.as_bytes());
            rom[0x147] = self.cartridge_type;
            rom[0x148] = self.rom_size_code;
            rom[0x149] = self.ram_size_code;
            rom[0x14D] = compute_header_checksum(&rom);
            let global = compute_global_checksum(&rom);
            rom[0x14E..0x150].copy_from_slice(&global.to_be_bytes());
            rom
        }
    }
}

#[cfg(test)]
mod tests {
    use super::test_util::RomBuilder;
    use super::*;

    #[test]
    fn builds_a_rom_only_cartridge() {
        let rom = RomBuilder::new().title("PLAIN").build();
        let cart = Cartridge::new(&rom).unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(cart.header().title, "PLAIN");
        assert_eq!(cart.read_rom(0x0042), 0x42);
        assert_eq!(cart.read_rom(0x7FFF), 0xFF);
    }

    #[test]
    fn ram_exists_only_when_the_type_declares_it() {
        // Type 0x00 with a RAM size byte set: no RAM chip.
        let rom = RomBuilder::new().ram_size_code(2).build();
        let mut cart = Cartridge::new(&rom).unwrap_or_else(|e| panic!("{e}"));
        cart.write_ram(0xA000, 0x77);
        assert_eq!(cart.read_ram(0xA000), 0xFF);

        // Type 0x08 (ROM+RAM).
        let rom = RomBuilder::new()
            .cartridge_type(0x08)
            .ram_size_code(2)
            .build();
        let mut cart = Cartridge::new(&rom).unwrap_or_else(|e| panic!("{e}"));
        cart.write_ram(0xA000, 0x77);
        assert_eq!(cart.read_ram(0xA000), 0x77);
    }

    #[test]
    fn builds_an_mbc1_cartridge_with_ram() {
        let rom = RomBuilder::new()
            .cartridge_type(0x03)
            .rom_size_code(1)
            .ram_size_code(2)
            .build();
        let mut cart = Cartridge::new(&rom).unwrap_or_else(|e| panic!("{e}"));
        // The builder fills the ROM with the low byte of each offset, so
        // every bank looks alike; switching must at least not disturb reads.
        cart.write_rom(0x2000, 3);
        assert_eq!(cart.read_rom(0x4042), 0x42);

        cart.write_rom(0x0000, 0x0A);
        cart.write_ram(0xA000, 0x77);
        assert_eq!(cart.read_ram(0xA000), 0x77);
    }

    #[test]
    fn builds_mbc3_and_mbc5_cartridges() {
        for code in [0x13, 0x1B] {
            let rom = RomBuilder::new()
                .cartridge_type(code)
                .rom_size_code(2)
                .ram_size_code(3)
                .build();
            let mut cart = Cartridge::new(&rom).unwrap_or_else(|e| panic!("{e}"));
            cart.write_rom(0x0000, 0x0A);
            cart.write_rom(0x4000, 0x03);
            cart.write_ram(0xA000, 0x77);
            assert_eq!(cart.read_ram(0xA000), 0x77, "type {code:#04x}");
            cart.write_rom(0x2000, 0x05);
            assert_eq!(cart.rom_bank(), 5, "type {code:#04x}");
        }
    }

    #[test]
    fn rejects_unimplemented_mappers() {
        // MBC2.
        let rom = RomBuilder::new().cartridge_type(0x05).build();
        assert_eq!(
            Cartridge::new(&rom).err(),
            Some(CartridgeError::UnsupportedMapper(0x05))
        );
    }

    #[test]
    fn battery_ram_can_be_saved_and_restored() {
        let rom = RomBuilder::new()
            .cartridge_type(0x03)
            .ram_size_code(2)
            .build();
        let mut cart = Cartridge::new(&rom).unwrap_or_else(|e| panic!("{e}"));
        assert!(cart.has_battery());
        assert!(!cart.take_save_dirty());

        cart.write_rom(0x0000, 0x0A);
        cart.write_ram(0xA123, 0x99);
        assert!(cart.take_save_dirty());
        assert!(!cart.take_save_dirty(), "the flag is cleared by asking");
        let saved = cart.save_data().expect("has a battery").to_vec();
        assert_eq!(saved.len(), 0x2000);
        assert_eq!(saved[0x123], 0x99);

        let mut fresh = Cartridge::new(&rom).unwrap_or_else(|e| panic!("{e}"));
        assert!(fresh.load_save(&saved));
        fresh.write_rom(0x0000, 0x0A);
        assert_eq!(fresh.read_ram(0xA123), 0x99);
    }

    #[test]
    fn saves_of_the_wrong_size_are_copied_as_far_as_they_fit() {
        let rom = RomBuilder::new()
            .cartridge_type(0x03)
            .ram_size_code(2)
            .build();
        let mut cart = Cartridge::new(&rom).unwrap_or_else(|e| panic!("{e}"));
        assert!(cart.load_save(&[0xAB; 16]));
        assert!(cart.load_save(&vec![0xCD; 0x4000]));
        assert_eq!(cart.save_data().map(<[u8]>::len), Some(0x2000));
    }

    #[test]
    fn cartridges_without_a_battery_have_nothing_to_save() {
        // MBC1 + RAM, no battery.
        let rom = RomBuilder::new()
            .cartridge_type(0x02)
            .ram_size_code(2)
            .build();
        let mut cart = Cartridge::new(&rom).unwrap_or_else(|e| panic!("{e}"));
        cart.write_rom(0x0000, 0x0A);
        cart.write_ram(0xA000, 0x01);
        assert!(!cart.has_battery());
        assert!(cart.save_data().is_none());
        assert!(!cart.take_save_dirty());
        assert!(!cart.load_save(&[0; 0x2000]));
    }

    #[test]
    fn rejects_truncated_images() {
        let mut rom = RomBuilder::new().rom_size_code(1).build();
        rom.truncate(0x8000);
        assert_eq!(
            Cartridge::new(&rom).err(),
            Some(CartridgeError::Truncated {
                declared: 0x1_0000,
                actual: 0x8000
            })
        );
    }
}
