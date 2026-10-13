//! The SM83 register file.

/// Zero flag: the result was zero.
pub const FLAG_Z: u8 = 0x80;
/// Subtract flag: the last operation was a subtraction (used by DAA).
pub const FLAG_N: u8 = 0x40;
/// Half-carry flag: carry out of bit 3 (used by DAA).
pub const FLAG_H: u8 = 0x20;
/// Carry flag.
pub const FLAG_C: u8 = 0x10;

/// The CPU registers. `f` holds the flags; its low four bits are always zero.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Registers {
    pub a: u8,
    pub f: u8,
    pub b: u8,
    pub c: u8,
    pub d: u8,
    pub e: u8,
    pub h: u8,
    pub l: u8,
    pub sp: u16,
    pub pc: u16,
}

impl Registers {
    /// Register values right after the DMG boot ROM finishes.
    ///
    /// The boot ROM leaves H and C set only when the header checksum is not
    /// zero, so it is needed here.
    ///
    /// Reference: <https://gbdev.io/pandocs/Power_Up_Sequence.html#cpu-registers>
    pub fn post_boot(header_checksum: u8) -> Self {
        Self {
            a: 0x01,
            f: if header_checksum == 0 {
                FLAG_Z
            } else {
                FLAG_Z | FLAG_H | FLAG_C
            },
            b: 0x00,
            c: 0x13,
            d: 0x00,
            e: 0xD8,
            h: 0x01,
            l: 0x4D,
            sp: 0xFFFE,
            pc: 0x0100,
        }
    }

    pub fn af(&self) -> u16 {
        u16::from_be_bytes([self.a, self.f])
    }

    pub fn bc(&self) -> u16 {
        u16::from_be_bytes([self.b, self.c])
    }

    pub fn de(&self) -> u16 {
        u16::from_be_bytes([self.d, self.e])
    }

    pub fn hl(&self) -> u16 {
        u16::from_be_bytes([self.h, self.l])
    }

    pub fn set_af(&mut self, value: u16) {
        [self.a, self.f] = value.to_be_bytes();
        self.f &= 0xF0;
    }

    pub fn set_bc(&mut self, value: u16) {
        [self.b, self.c] = value.to_be_bytes();
    }

    pub fn set_de(&mut self, value: u16) {
        [self.d, self.e] = value.to_be_bytes();
    }

    pub fn set_hl(&mut self, value: u16) {
        [self.h, self.l] = value.to_be_bytes();
    }

    pub fn flag(&self, flag: u8) -> bool {
        self.f & flag != 0
    }

    /// Replaces all four flags at once.
    pub fn set_flags(&mut self, z: bool, n: bool, h: bool, c: bool) {
        self.f = (if z { FLAG_Z } else { 0 })
            | (if n { FLAG_N } else { 0 })
            | (if h { FLAG_H } else { 0 })
            | (if c { FLAG_C } else { 0 });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pairs_are_big_endian_views_of_two_registers() {
        let mut regs = Registers::default();
        regs.set_bc(0x1234);
        regs.set_de(0x5678);
        regs.set_hl(0x9ABC);
        assert_eq!((regs.b, regs.c), (0x12, 0x34));
        assert_eq!(regs.de(), 0x5678);
        assert_eq!(regs.hl(), 0x9ABC);
    }

    #[test]
    fn low_nibble_of_f_is_always_zero() {
        let mut regs = Registers::default();
        regs.set_af(0x12FF);
        assert_eq!(regs.af(), 0x12F0);
    }

    #[test]
    fn post_boot_flags_depend_on_the_header_checksum() {
        assert_eq!(Registers::post_boot(0x00).f, 0x80);
        assert_eq!(Registers::post_boot(0x66).f, 0xB0);
        assert_eq!(Registers::post_boot(0x66).pc, 0x0100);
    }
}
