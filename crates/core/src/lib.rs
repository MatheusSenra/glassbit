//! Platform-independent Game Boy (DMG) emulator core.
//!
//! This crate knows nothing about browsers, windows or audio devices. A
//! frontend loads a ROM, feeds button state in, runs one frame at a time and
//! reads the framebuffer and audio samples back out.

pub mod bus;
pub mod cartridge;
pub mod observer;

use bus::Bus;
use cartridge::{Cartridge, CartridgeError, Header};
use observer::{NullObserver, Observer};

/// Version of the core crate.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// Visible screen width in pixels.
pub const SCREEN_WIDTH: usize = 160;
/// Visible screen height in pixels.
pub const SCREEN_HEIGHT: usize = 144;

/// The eight Game Boy buttons.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Button {
    Right,
    Left,
    Up,
    Down,
    A,
    B,
    Select,
    Start,
}

impl Button {
    fn mask(self) -> u8 {
        1 << self as u8
    }
}

/// A Game Boy with a cartridge inserted.
///
/// The type parameter is the [`Observer`] that receives inspection events;
/// the default [`NullObserver`] costs nothing.
pub struct GameBoy<O: Observer = NullObserver> {
    bus: Bus<O>,
    /// One DMG shade (0 = lightest, 3 = darkest) per pixel, row by row.
    framebuffer: [u8; SCREEN_WIDTH * SCREEN_HEIGHT],
    audio: Vec<f32>,
    /// Bit set = button held, indexed by [`Button`].
    buttons: u8,
}

impl GameBoy {
    /// Powers on a console with the given ROM image inserted.
    pub fn new(rom: &[u8]) -> Result<Self, CartridgeError> {
        Self::with_observer(rom, NullObserver)
    }
}

impl<O: Observer> GameBoy<O> {
    /// Like [`GameBoy::new`], reporting hardware events to `observer`.
    pub fn with_observer(rom: &[u8], observer: O) -> Result<Self, CartridgeError> {
        let cartridge = Cartridge::new(rom)?;
        Ok(Self {
            bus: Bus::new(cartridge, observer),
            framebuffer: [0; SCREEN_WIDTH * SCREEN_HEIGHT],
            audio: Vec::new(),
            buttons: 0,
        })
    }

    /// The header of the inserted cartridge.
    pub fn header(&self) -> &Header {
        self.bus.cartridge().header()
    }

    /// Reads a byte from the memory map, as the CPU would see it.
    pub fn peek(&self, addr: u16) -> u8 {
        self.bus.read(addr)
    }

    pub fn observer(&self) -> &O {
        &self.bus.observer
    }

    pub fn observer_mut(&mut self) -> &mut O {
        &mut self.bus.observer
    }

    /// Emulates one video frame (70224 T-cycles). Does nothing until the CPU exists.
    pub fn run_frame(&mut self) {
        self.audio.clear();
    }

    /// The last completed frame: `SCREEN_WIDTH * SCREEN_HEIGHT` shades in `0..=3`.
    pub fn framebuffer(&self) -> &[u8] {
        &self.framebuffer
    }

    /// Audio samples produced by the last call to [`GameBoy::run_frame`].
    pub fn audio_samples(&self) -> &[f32] {
        &self.audio
    }

    /// Updates whether `button` is currently held.
    pub fn set_button(&mut self, button: Button, pressed: bool) {
        if pressed {
            self.buttons |= button.mask();
        } else {
            self.buttons &= !button.mask();
        }
    }

    /// Whether `button` is currently held.
    pub fn is_pressed(&self, button: Button) -> bool {
        self.buttons & button.mask() != 0
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cartridge::test_util::RomBuilder;

    fn gameboy() -> GameBoy {
        GameBoy::new(&RomBuilder::new().title("LIBTEST").build()).unwrap_or_else(|e| panic!("{e}"))
    }

    #[test]
    fn framebuffer_covers_the_whole_screen() {
        assert_eq!(gameboy().framebuffer().len(), 160 * 144);
    }

    #[test]
    fn buttons_are_tracked_independently() {
        let mut gb = gameboy();
        gb.set_button(Button::A, true);
        gb.set_button(Button::Start, true);
        gb.set_button(Button::A, false);
        assert!(!gb.is_pressed(Button::A));
        assert!(gb.is_pressed(Button::Start));
    }

    #[test]
    fn exposes_the_cartridge_header_and_memory() {
        let gb = gameboy();
        assert_eq!(gb.header().title, "LIBTEST");
        assert_eq!(gb.peek(0x0000), 0x00);
        assert_eq!(gb.peek(0x0134), b'L');
        assert_eq!(gb.peek(0xFF40), 0x91);
    }

    #[test]
    fn rejects_an_empty_rom() {
        assert_eq!(
            GameBoy::new(&[]).err(),
            Some(CartridgeError::TooSmall { len: 0 })
        );
    }
}
