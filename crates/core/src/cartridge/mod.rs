//! Cartridges: the header and the memory bank controllers (MBCs).

mod header;
mod mbc1;
mod rom_only;

use std::fmt;

pub use header::{CartridgeType, CgbSupport, Header, MapperKind};
use mbc1::Mbc1;
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
        let mbc: Box<dyn Mbc> = match header.cartridge_type.mapper {
            MapperKind::RomOnly => Box::new(RomOnly::new(rom.to_vec(), ram_size)),
            // Only the ROM banking half of MBC1 exists so far.
            MapperKind::Mbc1 if ram_size == 0 && header.rom_size <= mbc1::MAX_ROM_SIZE => {
                Box::new(Mbc1::new(rom[..header.rom_size].to_vec()))
            }
            _ => {
                return Err(CartridgeError::UnsupportedMapper(
                    header.cartridge_type.code,
                ))
            }
        };

        Ok(Self { header, mbc })
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
    fn builds_an_mbc1_cartridge_without_ram() {
        let rom = RomBuilder::new()
            .cartridge_type(0x01)
            .rom_size_code(1)
            .build();
        let mut cart = Cartridge::new(&rom).unwrap_or_else(|e| panic!("{e}"));
        // The builder fills the ROM with the low byte of each offset, so
        // every bank looks alike; switching must at least not disturb reads.
        cart.write_rom(0x2000, 3);
        assert_eq!(cart.read_rom(0x4042), 0x42);
    }

    #[test]
    fn rejects_unimplemented_mappers() {
        // MBC1 with RAM is not supported yet.
        let rom = RomBuilder::new()
            .cartridge_type(0x03)
            .ram_size_code(2)
            .build();
        assert_eq!(
            Cartridge::new(&rom).err(),
            Some(CartridgeError::UnsupportedMapper(0x03))
        );

        let rom = RomBuilder::new().cartridge_type(0x19).build();
        assert_eq!(
            Cartridge::new(&rom).err(),
            Some(CartridgeError::UnsupportedMapper(0x19))
        );
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
