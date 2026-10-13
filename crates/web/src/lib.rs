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

    /// Whether the cartridge has battery-backed RAM, i.e. can save.
    pub fn has_battery(&self) -> bool {
        self.gb.has_battery()
    }

    /// A name to store this cartridge's save under: the title plus the
    /// header's global checksum, so different games (and different versions
    /// of one game) do not overwrite each other.
    pub fn save_key(&self) -> String {
        let header = self.gb.header();
        format!("{}-{:04x}", header.title, header.global_checksum)
    }

    /// A copy of the battery-backed RAM, or `undefined` without a battery.
    pub fn save_data(&self) -> Option<Vec<u8>> {
        self.gb.save_data().map(<[u8]>::to_vec)
    }

    /// Restores battery-backed RAM. Call before running the first frame.
    pub fn load_save(&mut self, data: &[u8]) -> bool {
        self.gb.load_save(data)
    }

    /// Whether the save data changed since the last call.
    pub fn take_save_dirty(&mut self) -> bool {
        self.gb.take_save_dirty()
    }

    /// Emulates one video frame.
    pub fn run_frame(&mut self) {
        self.gb.run_frame();
    }

    /// Sets the audio sample rate, normally the `AudioContext`'s.
    pub fn set_sample_rate(&mut self, hz: u32) {
        self.gb.set_sample_rate(hz);
    }

    /// The sound of the last frame: interleaved stereo samples (left, right)
    /// between -1 and 1, as a `Float32Array`.
    pub fn audio_samples(&self) -> Vec<f32> {
        self.gb.audio_samples().to_vec()
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
    fn frames_carry_audio_at_the_requested_rate() {
        let mut emulator = Emulator::demo();
        emulator.set_sample_rate(44_100);
        emulator.run_frame();
        emulator.run_frame();
        // 44100 Hz / 59.73 fps is about 738 stereo samples per frame.
        let values = emulator.audio_samples().len();
        assert!((2 * 730..=2 * 746).contains(&values), "{values}");
    }

    #[test]
    fn reports_the_cartridge_title() {
        assert_eq!(Emulator::demo().title(), "GLASSBIT DEMO");
    }

    #[test]
    fn demo_has_no_save_but_still_has_a_key() {
        let mut emulator = Emulator::demo();
        assert!(!emulator.has_battery());
        assert!(emulator.save_data().is_none());
        assert!(!emulator.take_save_dirty());
        assert!(!emulator.load_save(&[0; 16]));
        assert!(emulator.save_key().starts_with("GLASSBIT DEMO-"));
    }
}
