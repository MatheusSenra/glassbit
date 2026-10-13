//! WebAssembly bindings for Glassbit.
//!
//! This crate only translates between JavaScript and the core: the page
//! (`www/main.js`) owns the canvas, the frame loop and the input events.

use glassbit_core::{Button, GameBoy, SCREEN_HEIGHT, SCREEN_WIDTH};
use wasm_bindgen::prelude::*;

/// The four DMG shades as RGBA, from lightest to darkest: the greenish tint
/// of the original LCD.
const PALETTE: [[u8; 4]; 4] = [
    [0xE0, 0xF8, 0xD0, 0xFF],
    [0x88, 0xC0, 0x70, 0xFF],
    [0x34, 0x68, 0x56, 0xFF],
    [0x08, 0x18, 0x20, 0xFF],
];

/// Version of the emulator core this frontend was built against.
#[wasm_bindgen]
pub fn core_version() -> String {
    glassbit_core::VERSION.to_string()
}

/// Screen width in pixels.
#[wasm_bindgen]
pub fn screen_width() -> u32 {
    SCREEN_WIDTH as u32
}

/// Screen height in pixels.
#[wasm_bindgen]
pub fn screen_height() -> u32 {
    SCREEN_HEIGHT as u32
}

/// A running Game Boy, as seen from JavaScript.
#[wasm_bindgen]
pub struct Emulator {
    gb: GameBoy,
}

#[wasm_bindgen]
impl Emulator {
    /// Powers on with the given ROM image. Throws when the ROM is invalid or
    /// uses a mapper that is not supported yet.
    #[wasm_bindgen(constructor)]
    pub fn new(rom: &[u8]) -> Result<Emulator, JsError> {
        let gb = GameBoy::new(rom).map_err(|err| JsError::new(&err.to_string()))?;
        Ok(Self { gb })
    }

    /// Powers on with the built-in demo cartridge.
    pub fn demo() -> Emulator {
        let gb = GameBoy::new(&glassbit_core::demo::rom()).expect("the demo ROM is valid");
        Self { gb }
    }

    /// The cartridge title from its header.
    pub fn title(&self) -> String {
        self.gb.header().title.clone()
    }

    /// Emulates one video frame.
    pub fn run_frame(&mut self) {
        self.gb.run_frame();
    }

    /// Writes the screen as RGBA into `out`, which must hold
    /// `screen_width() * screen_height() * 4` bytes.
    pub fn copy_frame(&self, out: &mut [u8]) {
        write_rgba(self.gb.framebuffer(), out);
    }

    /// Updates a button. `button` is 0-7 for Right, Left, Up, Down, A, B,
    /// Select, Start; other values are ignored.
    pub fn set_button(&mut self, button: u8, pressed: bool) {
        if let Some(&button) = Button::ALL.get(button as usize) {
            self.gb.set_button(button, pressed);
        }
    }
}

fn write_rgba(framebuffer: &[u8], out: &mut [u8]) {
    for (pixel, &shade) in out.chunks_exact_mut(4).zip(framebuffer) {
        pixel.copy_from_slice(&PALETTE[shade as usize]);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn demo_frame_converts_to_opaque_rgba() {
        let mut emulator = Emulator::demo();
        for _ in 0..12 {
            emulator.run_frame();
        }
        let mut rgba = vec![0u8; SCREEN_WIDTH * SCREEN_HEIGHT * 4];
        emulator.copy_frame(&mut rgba);

        assert!(rgba.chunks_exact(4).all(|pixel| pixel[3] == 0xFF));
        // The checkerboard starts with a blank tile, then a tinted one.
        assert_eq!(rgba[..4], PALETTE[0]);
        assert_eq!(rgba[8 * 4..8 * 4 + 4], PALETTE[1]);
    }

    #[test]
    fn buttons_reach_the_core_and_bad_indices_are_ignored() {
        let mut emulator = Emulator::demo();
        emulator.set_button(4, true);
        assert!(emulator.gb.is_pressed(Button::A));
        emulator.set_button(4, false);
        assert!(!emulator.gb.is_pressed(Button::A));
        emulator.set_button(99, true);
    }

    #[test]
    fn reports_the_cartridge_title() {
        assert_eq!(Emulator::demo().title(), "GLASSBIT DEMO");
    }
}
