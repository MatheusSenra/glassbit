//! Platform-independent Game Boy (DMG) emulator core.
//!
//! This crate knows nothing about browsers, windows or audio devices. A
//! frontend loads a ROM, feeds button state in, runs one frame at a time and
//! reads the framebuffer and audio samples back out.
//!
//! Phase 0 only fixes the shape of the public API; emulation starts in Phase 1.

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
pub struct GameBoy {
    rom: Vec<u8>,
    /// One DMG shade (0 = lightest, 3 = darkest) per pixel, row by row.
    framebuffer: [u8; SCREEN_WIDTH * SCREEN_HEIGHT],
    audio: Vec<f32>,
    /// Bit set = button held, indexed by [`Button`].
    buttons: u8,
}

impl GameBoy {
    /// Creates a console with no cartridge inserted.
    pub fn new() -> Self {
        Self {
            rom: Vec::new(),
            framebuffer: [0; SCREEN_WIDTH * SCREEN_HEIGHT],
            audio: Vec::new(),
            buttons: 0,
        }
    }

    /// Inserts a cartridge. Header parsing and mappers arrive in Phase 1.
    pub fn load_rom(&mut self, rom: &[u8]) {
        self.rom = rom.to_vec();
    }

    /// Size in bytes of the loaded ROM image.
    pub fn rom_len(&self) -> usize {
        self.rom.len()
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

impl Default for GameBoy {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn framebuffer_covers_the_whole_screen() {
        let gb = GameBoy::new();
        assert_eq!(gb.framebuffer().len(), 160 * 144);
    }

    #[test]
    fn buttons_are_tracked_independently() {
        let mut gb = GameBoy::new();
        gb.set_button(Button::A, true);
        gb.set_button(Button::Start, true);
        gb.set_button(Button::A, false);
        assert!(!gb.is_pressed(Button::A));
        assert!(gb.is_pressed(Button::Start));
    }

    #[test]
    fn load_rom_keeps_the_image() {
        let mut gb = GameBoy::new();
        gb.load_rom(&[0; 0x8000]);
        assert_eq!(gb.rom_len(), 0x8000);
    }
}
