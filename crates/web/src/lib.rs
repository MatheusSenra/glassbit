//! WebAssembly bindings for Glassbit.
//!
//! This crate only translates between JavaScript and the core: the page
//! (`www/main.js`) owns the canvas, the frame loop and the input events.
//!
//! The emulator exists in two flavors. Normally it runs with the core's
//! `NullObserver`, which costs nothing. While the X-ray panel is open it is
//! swapped, state and all, for one with a [`Recorder`] observer that keeps
//! a log of recent hardware events for the panel to show.

use std::collections::VecDeque;

use glassbit_core::cpu::Registers;
use glassbit_core::observer::Observer;
use glassbit_core::ppu::Mode;
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

/// How many entries each event log keeps.
const LOG_CAPACITY: usize = 32;

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

/// The observer used while the X-ray panel is open.
#[derive(Default)]
struct Recorder {
    /// Frames seen since recording started.
    frame: u32,
    /// The line the PPU was on at its last mode change.
    line: u8,
    instructions: u32,
    /// `[bit, frame, line]` for each interrupt served, oldest first.
    interrupts: VecDeque<[u32; 3]>,
    /// `[address, old value, new value]` for each I/O write, oldest first.
    io_writes: VecDeque<[u16; 3]>,
}

fn push_capped<T>(log: &mut VecDeque<T>, entry: T) {
    if log.len() == LOG_CAPACITY {
        log.pop_front();
    }
    log.push_back(entry);
}

impl Observer for Recorder {
    fn instruction(&mut self, _before: &Registers, _after: &Registers) {
        self.instructions = self.instructions.wrapping_add(1);
    }

    fn interrupt(&mut self, bit: u8) {
        let entry = [bit as u32, self.frame, self.line as u32];
        push_capped(&mut self.interrupts, entry);
    }

    fn ppu_mode(&mut self, mode: Mode, ly: u8) {
        self.line = ly;
        if mode == Mode::VBlank {
            self.frame += 1;
        }
    }

    fn io_write(&mut self, addr: u16, old: u8, new: u8) {
        push_capped(&mut self.io_writes, [addr, old as u16, new as u16]);
    }
}

enum Machine {
    Plain(GameBoy),
    Xray(GameBoy<Recorder>),
}

/// Runs the same expression on whichever flavor of emulator is active.
macro_rules! on {
    ($machine:expr, $gb:ident => $body:expr) => {
        match $machine {
            Machine::Plain($gb) => $body,
            Machine::Xray($gb) => $body,
        }
    };
}

/// A running Game Boy, as seen from JavaScript.
#[wasm_bindgen]
pub struct Emulator {
    /// Only `None` for an instant while the flavor is being swapped.
    machine: Option<Machine>,
}

impl Emulator {
    fn from_core(gb: GameBoy) -> Self {
        Self {
            machine: Some(Machine::Plain(gb)),
        }
    }

    fn machine(&self) -> &Machine {
        self.machine.as_ref().expect("machine is present")
    }

    fn machine_mut(&mut self) -> &mut Machine {
        self.machine.as_mut().expect("machine is present")
    }
}

#[wasm_bindgen]
impl Emulator {
    /// Powers on with the given ROM image. Throws when the ROM is invalid or
    /// uses a mapper that is not supported yet.
    #[wasm_bindgen(constructor)]
    pub fn new(rom: &[u8]) -> Result<Emulator, JsError> {
        let gb = GameBoy::new(rom).map_err(|err| JsError::new(&err.to_string()))?;
        Ok(Self::from_core(gb))
    }

    /// Powers on with the built-in demo cartridge.
    pub fn demo() -> Emulator {
        let gb = GameBoy::new(&glassbit_core::demo::rom()).expect("the demo ROM is valid");
        Self::from_core(gb)
    }

    /// The cartridge title from its header.
    pub fn title(&self) -> String {
        on!(self.machine(), gb => gb.header().title.clone())
    }

    /// Whether the cartridge has battery-backed RAM, i.e. can save.
    pub fn has_battery(&self) -> bool {
        on!(self.machine(), gb => gb.has_battery())
    }

    /// A name to store this cartridge's save under: the title plus the
    /// header's global checksum, so different games (and different versions
    /// of one game) do not overwrite each other.
    pub fn save_key(&self) -> String {
        on!(self.machine(), gb => {
            let header = gb.header();
            format!("{}-{:04x}", header.title, header.global_checksum)
        })
    }

    /// A copy of the battery-backed RAM, or `undefined` without a battery.
    pub fn save_data(&self) -> Option<Vec<u8>> {
        on!(self.machine(), gb => gb.save_data().map(<[u8]>::to_vec))
    }

    /// Restores battery-backed RAM. Call before running the first frame.
    pub fn load_save(&mut self, data: &[u8]) -> bool {
        on!(self.machine_mut(), gb => gb.load_save(data))
    }

    /// Whether the save data changed since the last call.
    pub fn take_save_dirty(&mut self) -> bool {
        on!(self.machine_mut(), gb => gb.take_save_dirty())
    }

    /// Emulates one video frame.
    pub fn run_frame(&mut self) {
        on!(self.machine_mut(), gb => gb.run_frame())
    }

    /// Sets the audio sample rate, normally the `AudioContext`'s.
    pub fn set_sample_rate(&mut self, hz: u32) {
        on!(self.machine_mut(), gb => gb.set_sample_rate(hz))
    }

    /// The sound of the last frame: interleaved stereo samples (left, right)
    /// between -1 and 1, as a `Float32Array`.
    pub fn audio_samples(&self) -> Vec<f32> {
        on!(self.machine(), gb => gb.audio_samples().to_vec())
    }

    /// Writes the screen as RGBA into `out`, which must hold
    /// `screen_width() * screen_height() * 4` bytes.
    pub fn copy_frame(&self, out: &mut [u8]) {
        on!(self.machine(), gb => write_rgba(gb.framebuffer(), out))
    }

    /// Updates a button. `button` is 0-7 for Right, Left, Up, Down, A, B,
    /// Select, Start; other values are ignored.
    pub fn set_button(&mut self, button: u8, pressed: bool) {
        if let Some(&button) = Button::ALL.get(button as usize) {
            on!(self.machine_mut(), gb => gb.set_button(button, pressed));
        }
    }

    // ---- X-ray mode ----------------------------------------------------

    /// Switches event recording on or off without disturbing the game.
    pub fn set_xray(&mut self, enabled: bool) {
        let machine = self.machine.take().expect("machine is present");
        self.machine = Some(match (machine, enabled) {
            (Machine::Plain(gb), true) => Machine::Xray(gb.into_observed(Recorder::default())),
            (Machine::Xray(gb), false) => {
                Machine::Plain(gb.into_observed(glassbit_core::observer::NullObserver))
            }
            (unchanged, _) => unchanged,
        });
    }

    /// Whether events are being recorded.
    pub fn xray(&self) -> bool {
        matches!(self.machine(), Machine::Xray(_))
    }

    /// Executes one CPU instruction.
    pub fn step_instruction(&mut self) {
        on!(self.machine_mut(), gb => gb.step())
    }

    /// Runs until the PPU reaches the next scanline.
    pub fn step_line(&mut self) {
        on!(self.machine_mut(), gb => gb.step_line())
    }

    /// The CPU state as eight numbers: AF, BC, DE, HL, SP, PC, IME (0 or 1)
    /// and halted (0 or 1).
    pub fn cpu_state(&self) -> Vec<u16> {
        on!(self.machine(), gb => {
            let cpu = gb.cpu();
            let regs = &cpu.regs;
            vec![
                regs.af(),
                regs.bc(),
                regs.de(),
                regs.hl(),
                regs.sp,
                regs.pc,
                cpu.ime as u16,
                cpu.halted as u16,
            ]
        })
    }

    /// The instruction at `addr` as `"length|operation|operand,operand"`,
    /// for example `"2|LD|A,$42"`.
    pub fn disassemble(&self, addr: u16) -> String {
        let instruction = on!(self.machine(), gb => gb.disassemble(addr));
        format!(
            "{}|{}|{}",
            instruction.length,
            instruction.operation,
            instruction.operands.join(",")
        )
    }

    /// One byte of memory, as the CPU would see it.
    pub fn peek(&self, addr: u16) -> u8 {
        on!(self.machine(), gb => gb.peek(addr))
    }

    /// `length` bytes of memory starting at `start` (wrapping at `0xFFFF`).
    pub fn read_range(&self, start: u16, length: u16) -> Vec<u8> {
        on!(self.machine(), gb => {
            (0..length).map(|offset| gb.peek(start.wrapping_add(offset))).collect()
        })
    }

    /// The ROM bank the mapper currently shows at `0x4000`.
    pub fn rom_bank(&self) -> u16 {
        on!(self.machine(), gb => gb.rom_bank())
    }

    /// Frames elapsed since recording was switched on.
    pub fn recorded_frames(&self) -> u32 {
        match self.machine() {
            Machine::Xray(gb) => gb.observer().frame,
            Machine::Plain(_) => 0,
        }
    }

    /// Instructions executed since recording was switched on.
    pub fn recorded_instructions(&self) -> u32 {
        match self.machine() {
            Machine::Xray(gb) => gb.observer().instructions,
            Machine::Plain(_) => 0,
        }
    }

    /// The most recent interrupts served, oldest first, as a flat list of
    /// `bit, frame, line` triples. Empty unless recording.
    pub fn interrupt_log(&self) -> Vec<u32> {
        match self.machine() {
            Machine::Xray(gb) => gb.observer().interrupts.iter().flatten().copied().collect(),
            Machine::Plain(_) => Vec::new(),
        }
    }

    /// The most recent I/O register writes, oldest first, as a flat list of
    /// `address, old value, new value` triples. Empty unless recording.
    pub fn io_write_log(&self) -> Vec<u16> {
        match self.machine() {
            Machine::Xray(gb) => gb.observer().io_writes.iter().flatten().copied().collect(),
            Machine::Plain(_) => Vec::new(),
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

    fn is_pressed(emulator: &Emulator, button: Button) -> bool {
        on!(emulator.machine(), gb => gb.is_pressed(button))
    }

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
        assert!(is_pressed(&emulator, Button::A));
        emulator.set_button(4, false);
        assert!(!is_pressed(&emulator, Button::A));
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

    #[test]
    fn xray_can_be_toggled_while_the_game_keeps_its_state() {
        let mut emulator = Emulator::demo();
        for _ in 0..12 {
            emulator.run_frame();
        }
        let before = emulator.cpu_state();

        emulator.set_xray(true);
        assert!(emulator.xray());
        assert_eq!(emulator.cpu_state(), before);
        emulator.set_xray(true);
        assert!(emulator.xray(), "switching on twice is harmless");

        emulator.set_xray(false);
        assert!(!emulator.xray());
        assert_eq!(emulator.cpu_state(), before);
        assert!(emulator.io_write_log().is_empty());
    }

    #[test]
    fn recording_logs_io_writes_and_counts_frames() {
        let mut emulator = Emulator::demo();
        for _ in 0..12 {
            emulator.run_frame();
        }
        emulator.set_xray(true);
        emulator.run_frame();
        emulator.run_frame();

        assert_eq!(emulator.recorded_frames(), 2);
        assert!(emulator.recorded_instructions() > 1000);
        // Each frame the demo writes P1 three times and BGP once.
        let log = emulator.io_write_log();
        assert_eq!(log.len() % 3, 0);
        assert!(log.chunks_exact(3).any(|entry| entry[0] == 0xFF47));
        assert!(log.len() <= 3 * LOG_CAPACITY);
    }

    #[test]
    fn stepping_and_inspection() {
        let mut emulator = Emulator::demo();
        // The entry point is NOP ; JP $0150.
        assert_eq!(emulator.disassemble(0x0100), "1|NOP|");
        assert_eq!(emulator.disassemble(0x0101), "3|JP|$0150");
        assert_eq!(emulator.cpu_state()[5], 0x0100);

        emulator.step_instruction();
        emulator.step_instruction();
        assert_eq!(emulator.cpu_state()[5], 0x0150);

        let line = emulator.peek(0xFF44);
        emulator.step_line();
        assert_eq!(emulator.peek(0xFF44), line + 1);

        assert_eq!(emulator.read_range(0x0104, 4), vec![0, 0, 0, 0]);
        assert_eq!(emulator.read_range(0x0134, 8), b"GLASSBIT");
        assert_eq!(emulator.rom_bank(), 1);
    }
}
