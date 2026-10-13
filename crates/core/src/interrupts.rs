//! Interrupt sources, in priority order (lowest bit wins).
//!
//! Each source owns one bit in IF (`0xFF0F`, "something happened") and in IE
//! (`0xFFFF`, "the program cares"). The CPU serves an interrupt when the same
//! bit is set in both and the master enable (IME) is on.
//!
//! Reference: <https://gbdev.io/pandocs/Interrupts.html>

/// The interrupt request register.
pub const IF: u16 = 0xFF0F;
/// The interrupt enable register.
pub const IE: u16 = 0xFFFF;

/// The screen finished drawing its last visible line.
pub const VBLANK: u8 = 0x01;
/// A condition selected in the STAT register happened.
pub const STAT: u8 = 0x02;
/// TIMA overflowed.
pub const TIMER: u8 = 0x04;
/// A serial transfer finished.
pub const SERIAL: u8 = 0x08;
/// A button was pressed.
pub const JOYPAD: u8 = 0x10;

/// Only five interrupt sources exist.
pub const MASK: u8 = 0x1F;

/// The address the CPU jumps to when serving the interrupt with this bit.
pub fn vector(bit: u8) -> u16 {
    0x0040 + 8 * bit.trailing_zeros() as u16
}

/// A short name for the interrupt with this bit.
pub fn name(bit: u8) -> &'static str {
    match bit {
        VBLANK => "VBlank",
        STAT => "STAT",
        TIMER => "Timer",
        SERIAL => "Serial",
        JOYPAD => "Joypad",
        _ => "unknown",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn vectors_are_eight_bytes_apart_starting_at_0x40() {
        assert_eq!(vector(VBLANK), 0x40);
        assert_eq!(vector(STAT), 0x48);
        assert_eq!(vector(TIMER), 0x50);
        assert_eq!(vector(SERIAL), 0x58);
        assert_eq!(vector(JOYPAD), 0x60);
    }
}
