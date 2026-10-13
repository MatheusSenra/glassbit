//! Inspection hooks for the X-ray mode.
//!
//! The emulator is generic over an [`Observer`]. Every method has an empty
//! default body, so with [`NullObserver`] the calls compile away and normal
//! play pays nothing for them. More events are added as the hardware that
//! produces them is implemented.

/// Receives events from the emulated hardware as they happen.
pub trait Observer {
    /// A write landed on an I/O register (`0xFF00..=0xFF7F`) or on IE (`0xFFFF`).
    fn io_write(&mut self, _addr: u16, _old: u8, _new: u8) {}
}

/// An observer that ignores everything.
#[derive(Debug, Default, Clone, Copy)]
pub struct NullObserver;

impl Observer for NullObserver {}
