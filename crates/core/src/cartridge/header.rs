//! Cartridge header parsing (`0x0100..=0x014F`).
//!
//! Reference: <https://gbdev.io/pandocs/The_Cartridge_Header.html>

use super::CartridgeError;

/// First address after the header; a ROM image cannot be shorter than this.
pub const HEADER_END: usize = 0x150;

const TITLE_START: usize = 0x134;
const CGB_FLAG: usize = 0x143;
const CARTRIDGE_TYPE: usize = 0x147;
const ROM_SIZE: usize = 0x148;
const RAM_SIZE: usize = 0x149;
const VERSION: usize = 0x14C;
const HEADER_CHECKSUM: usize = 0x14D;
const GLOBAL_CHECKSUM: usize = 0x14E;

/// The family of memory bank controller on the cartridge.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MapperKind {
    RomOnly,
    Mbc1,
    Mbc2,
    Mbc3,
    Mbc5,
    /// A mapper Glassbit does not know about.
    Unknown,
}

/// Decoded cartridge type byte (`0x147`): the mapper and the extra hardware.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CartridgeType {
    /// The raw byte from the header.
    pub code: u8,
    pub mapper: MapperKind,
    /// External RAM chip. MBC2 has RAM built into the mapper and does not set this.
    pub ram: bool,
    pub battery: bool,
    /// Real-time clock (MBC3 only).
    pub timer: bool,
    pub rumble: bool,
}

impl CartridgeType {
    pub fn from_code(code: u8) -> Self {
        use MapperKind::*;
        // (mapper, ram, battery, timer, rumble)
        let (mapper, ram, battery, timer, rumble) = match code {
            0x00 => (RomOnly, false, false, false, false),
            0x01 => (Mbc1, false, false, false, false),
            0x02 => (Mbc1, true, false, false, false),
            0x03 => (Mbc1, true, true, false, false),
            0x05 => (Mbc2, false, false, false, false),
            0x06 => (Mbc2, false, true, false, false),
            0x08 => (RomOnly, true, false, false, false),
            0x09 => (RomOnly, true, true, false, false),
            0x0F => (Mbc3, false, true, true, false),
            0x10 => (Mbc3, true, true, true, false),
            0x11 => (Mbc3, false, false, false, false),
            0x12 => (Mbc3, true, false, false, false),
            0x13 => (Mbc3, true, true, false, false),
            0x19 => (Mbc5, false, false, false, false),
            0x1A => (Mbc5, true, false, false, false),
            0x1B => (Mbc5, true, true, false, false),
            0x1C => (Mbc5, false, false, false, true),
            0x1D => (Mbc5, true, false, false, true),
            0x1E => (Mbc5, true, true, false, true),
            _ => (Unknown, false, false, false, false),
        };
        Self {
            code,
            mapper,
            ram,
            battery,
            timer,
            rumble,
        }
    }
}

/// Whether the cartridge declares Game Boy Color support.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CgbSupport {
    /// A plain DMG game.
    None,
    /// Works on DMG, has extras on CGB.
    Enhanced,
    /// Requires a CGB.
    Only,
}

/// The parsed cartridge header.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Header {
    pub title: String,
    pub cgb: CgbSupport,
    pub cartridge_type: CartridgeType,
    /// ROM size in bytes, as declared by the header.
    pub rom_size: usize,
    /// External RAM size in bytes, as declared by the header.
    pub ram_size: usize,
    pub version: u8,
    /// Header checksum stored at `0x14D`.
    pub header_checksum: u8,
    /// Header checksum computed over `0x134..=0x14C`.
    pub computed_header_checksum: u8,
    /// Global checksum stored at `0x14E..=0x14F` (big endian).
    pub global_checksum: u16,
    /// Sum of every ROM byte except the two checksum bytes.
    pub computed_global_checksum: u16,
}

impl Header {
    /// Parses the header of a ROM image.
    pub fn parse(rom: &[u8]) -> Result<Self, CartridgeError> {
        if rom.len() < HEADER_END {
            return Err(CartridgeError::TooSmall { len: rom.len() });
        }

        let cgb = match rom[CGB_FLAG] {
            0xC0 => CgbSupport::Only,
            flag if flag & 0x80 != 0 => CgbSupport::Enhanced,
            _ => CgbSupport::None,
        };
        // Old cartridges use all 16 bytes for the title; on newer ones the
        // last byte is the CGB flag.
        let title_end = if cgb == CgbSupport::None {
            CGB_FLAG + 1
        } else {
            CGB_FLAG
        };

        let rom_size = match rom[ROM_SIZE] {
            code @ 0..=8 => 0x8000 << code,
            code => return Err(CartridgeError::InvalidRomSize(code)),
        };
        let ram_size = match rom[RAM_SIZE] {
            0 => 0,
            // Never used by a licensed cartridge, but some homebrew expects 2 KiB.
            1 => 0x800,
            2 => 0x2000,
            3 => 0x8000,
            4 => 0x2_0000,
            5 => 0x1_0000,
            code => return Err(CartridgeError::InvalidRamSize(code)),
        };

        Ok(Self {
            title: parse_title(&rom[TITLE_START..title_end]),
            cgb,
            cartridge_type: CartridgeType::from_code(rom[CARTRIDGE_TYPE]),
            rom_size,
            ram_size,
            version: rom[VERSION],
            header_checksum: rom[HEADER_CHECKSUM],
            computed_header_checksum: compute_header_checksum(rom),
            global_checksum: u16::from_be_bytes([rom[GLOBAL_CHECKSUM], rom[GLOBAL_CHECKSUM + 1]]),
            computed_global_checksum: compute_global_checksum(rom),
        })
    }

    /// The check the boot ROM performs; real hardware refuses to boot when it fails.
    pub fn header_checksum_ok(&self) -> bool {
        self.header_checksum == self.computed_header_checksum
    }

    /// Real hardware never verifies this one.
    pub fn global_checksum_ok(&self) -> bool {
        self.global_checksum == self.computed_global_checksum
    }
}

fn parse_title(bytes: &[u8]) -> String {
    bytes
        .iter()
        .take_while(|&&b| b != 0)
        .map(|&b| {
            if b.is_ascii_graphic() || b == b' ' {
                b as char
            } else {
                '?'
            }
        })
        .collect::<String>()
        .trim_end()
        .to_string()
}

pub(super) fn compute_header_checksum(rom: &[u8]) -> u8 {
    rom[TITLE_START..HEADER_CHECKSUM]
        .iter()
        .fold(0u8, |sum, &b| sum.wrapping_sub(b).wrapping_sub(1))
}

pub(super) fn compute_global_checksum(rom: &[u8]) -> u16 {
    rom.iter()
        .enumerate()
        .filter(|(i, _)| *i != GLOBAL_CHECKSUM && *i != GLOBAL_CHECKSUM + 1)
        .fold(0u16, |sum, (_, &b)| sum.wrapping_add(b as u16))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cartridge::test_util::RomBuilder;

    #[test]
    fn parses_basic_fields() {
        let rom = RomBuilder::new()
            .title("GLASSBIT")
            .cartridge_type(0x1B)
            .rom_size_code(2)
            .ram_size_code(3)
            .build();
        let header = Header::parse(&rom).unwrap();

        assert_eq!(header.title, "GLASSBIT");
        assert_eq!(header.cgb, CgbSupport::None);
        assert_eq!(header.cartridge_type.mapper, MapperKind::Mbc5);
        assert!(header.cartridge_type.ram);
        assert!(header.cartridge_type.battery);
        assert!(!header.cartridge_type.timer);
        assert_eq!(header.rom_size, 128 * 1024);
        assert_eq!(header.ram_size, 32 * 1024);
    }

    #[test]
    fn checksums_match_on_a_well_formed_rom() {
        let rom = RomBuilder::new().title("CHECK").build();
        let header = Header::parse(&rom).unwrap();
        assert!(header.header_checksum_ok());
        assert!(header.global_checksum_ok());
    }

    #[test]
    fn detects_corrupted_checksums() {
        let mut rom = RomBuilder::new().title("CHECK").build();
        rom[TITLE_START] ^= 0xFF;
        let header = Header::parse(&rom).unwrap();
        assert!(!header.header_checksum_ok());
        assert!(!header.global_checksum_ok());
    }

    #[test]
    fn header_checksum_follows_the_documented_formula() {
        // An all-zero header region: 25 bytes, each subtracting (0 + 1).
        let rom = vec![0u8; 0x8000];
        assert_eq!(compute_header_checksum(&rom), 0u8.wrapping_sub(25));
    }

    #[test]
    fn sixteen_character_title_on_a_dmg_cartridge() {
        let rom = RomBuilder::new().title("ABCDEFGHIJKLMNOP").build();
        let header = Header::parse(&rom).unwrap();
        assert_eq!(header.title, "ABCDEFGHIJKLMNOP");
    }

    #[test]
    fn cgb_flag_is_not_part_of_the_title() {
        let mut rom = RomBuilder::new().title("ABCDEFGHIJKLMNO").build();
        rom[CGB_FLAG] = 0x80;
        let header = Header::parse(&rom).unwrap();
        assert_eq!(header.title, "ABCDEFGHIJKLMNO");
        assert_eq!(header.cgb, CgbSupport::Enhanced);

        rom[CGB_FLAG] = 0xC0;
        assert_eq!(Header::parse(&rom).unwrap().cgb, CgbSupport::Only);
    }

    #[test]
    fn unknown_cartridge_type_still_parses() {
        let rom = RomBuilder::new().cartridge_type(0xFC).build();
        let header = Header::parse(&rom).unwrap();
        assert_eq!(header.cartridge_type.mapper, MapperKind::Unknown);
        assert_eq!(header.cartridge_type.code, 0xFC);
    }

    #[test]
    fn rejects_images_shorter_than_the_header() {
        assert_eq!(
            Header::parse(&[0; 0x14F]),
            Err(CartridgeError::TooSmall { len: 0x14F })
        );
    }

    #[test]
    fn rejects_invalid_size_codes() {
        let mut rom = RomBuilder::new().build();
        rom[ROM_SIZE] = 0x09;
        assert_eq!(
            Header::parse(&rom),
            Err(CartridgeError::InvalidRomSize(0x09))
        );

        let mut rom = RomBuilder::new().build();
        rom[RAM_SIZE] = 0x06;
        assert_eq!(
            Header::parse(&rom),
            Err(CartridgeError::InvalidRamSize(0x06))
        );
    }
}
