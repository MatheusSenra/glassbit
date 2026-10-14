//! Inspection hooks for the X-ray mode.
//!
//! The emulator is generic over an [`Observer`]. Every method has an empty
//! default body, so with [`NullObserver`] the calls compile away and normal
//! play pays nothing for them. More events are added as the hardware that
//! produces them is implemented.

use crate::cpu::Registers;
use crate::ppu::Mode;

/// Receives events from the emulated hardware as they happen.
pub trait Observer {
    /// The CPU executed one instruction. `before.pc` is its address.
    fn instruction(&mut self, _before: &Registers, _after: &Registers) {}

    /// The CPU started serving an interrupt. `bit` is its bit in IF/IE; see
    /// [`crate::interrupts`] for the sources.
    fn interrupt(&mut self, _bit: u8) {}

    /// The PPU entered a new mode while on line `ly`.
    fn ppu_mode(&mut self, _mode: Mode, _ly: u8) {}

    /// The cartridge mapper now shows ROM bank `bank` at `0x4000..=0x7FFF`.
    fn rom_bank_switch(&mut self, _bank: u16) {}

    /// A write landed on an I/O register (`0xFF00..=0xFF7F`) or on IE (`0xFFFF`).
    fn io_write(&mut self, _addr: u16, _old: u8, _new: u8) {}

    /// The CPU wrote `value` to `addr` (any address, including writes that
    /// the hardware then ignores).
    fn memory_write(&mut self, _addr: u16, _value: u8) {}

    /// Asked after every instruction while running a frame or a line: return
    /// `true` to stop there, e.g. because a breakpoint was reached.
    fn should_stop(&mut self) -> bool {
        false
    }
}

/// An observer that ignores everything.
#[derive(Debug, Default, Clone, Copy)]
pub struct NullObserver;

impl Observer for NullObserver {}
