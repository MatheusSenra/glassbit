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
use glassbit_core::ppu::{rgb555, Mode};
use glassbit_core::serial::LinkMessage;
use glassbit_core::{Button, GameBoy, Model, SCREEN_HEIGHT, SCREEN_WIDTH};
use wasm_bindgen::prelude::*;

/// The default colors for the four DMG shades as RGBA, from lightest to
/// darkest: the greenish tint of the original LCD.
const DEFAULT_PALETTE: [[u8; 4]; 4] = [
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

/// One bit per address in the 64 KiB address space.
#[derive(Clone)]
struct AddressSet(Box<[u64]>);

impl AddressSet {
    fn new(addresses: &[u16]) -> Self {
        let mut bits = vec![0u64; 1024].into_boxed_slice();
        for &addr in addresses {
            bits[addr as usize / 64] |= 1 << (addr % 64);
        }
        Self(bits)
    }

    fn contains(&self, addr: u16) -> bool {
        self.0[addr as usize / 64] & 1 << (addr % 64) != 0
    }
}

impl Default for AddressSet {
    fn default() -> Self {
        Self::new(&[])
    }
}

/// Where the program stopped and why.
#[derive(Clone, Copy)]
enum Hit {
    /// PC arrived at an execution breakpoint.
    Execute(u16),
    /// The CPU wrote a value to a watched address.
    Write(u16, u8),
}

/// The observer used while the X-ray panel is open.
#[derive(Default)]
struct Recorder {
    /// Stop before executing the instruction at these addresses.
    execute_breakpoints: AddressSet,
    /// Stop right after a write to these addresses.
    write_breakpoints: AddressSet,
    hit: Option<Hit>,
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
    fn instruction(&mut self, _before: &Registers, after: &Registers) {
        self.instructions = self.instructions.wrapping_add(1);
        if self.execute_breakpoints.contains(after.pc) {
            self.hit = Some(Hit::Execute(after.pc));
        }
    }

    fn memory_write(&mut self, addr: u16, value: u8) {
        if self.write_breakpoints.contains(addr) {
            self.hit = Some(Hit::Write(addr, value));
        }
    }

    fn should_stop(&mut self) -> bool {
        self.hit.is_some()
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
    /// The RGBA color shown for each of the four shades.
    palette: [[u8; 4]; 4],
    /// Kept here so they survive the panel being closed and opened.
    execute_breakpoints: Vec<u16>,
    write_breakpoints: Vec<u16>,
}

impl Emulator {
    fn from_core(gb: GameBoy) -> Self {
        Self {
            machine: Some(Machine::Plain(gb)),
            palette: DEFAULT_PALETTE,
            execute_breakpoints: Vec::new(),
            write_breakpoints: Vec::new(),
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
    /// Powers on with the given ROM image. `model` is `"dmg"` or `"cgb"`
    /// to choose the console (a DMG game on `"cgb"` runs in compatibility
    /// mode), anything else for the one the game asks for. Throws when the
    /// ROM is invalid or uses a mapper that is not supported yet.
    #[wasm_bindgen(constructor)]
    pub fn new(rom: &[u8], model: &str) -> Result<Emulator, JsError> {
        let gb = boot(rom, model).map_err(|err| JsError::new(&err.to_string()))?;
        Ok(Self::from_core(gb))
    }

    /// Powers on with the built-in demo cartridge, a DMG game.
    pub fn demo(model: &str) -> Emulator {
        let gb = boot(&glassbit_core::demo::rom(), model).expect("the demo ROM is valid");
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

    /// What the battery keeps (RAM, and the clock on MBC3 cartridges that
    /// have one), or `undefined` without a battery. `now` is the current
    /// time in seconds since 1970, stored with the clock so it can catch up
    /// on the time the game spends closed.
    pub fn save_data(&self, now: f64) -> Option<Vec<u8>> {
        on!(self.machine(), gb => gb.save_data_at(now.max(0.0) as u64))
    }

    /// Restores what `save_data` returned, letting a saved clock catch up to
    /// `now` (seconds since 1970). Call before running the first frame.
    pub fn load_save(&mut self, data: &[u8], now: f64) -> bool {
        on!(self.machine_mut(), gb => gb.load_save_at(data, now.max(0.0) as u64))
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
        on!(self.machine(), gb => {
            if gb.model() == Model::Cgb {
                write_color_rgba(gb.color_framebuffer(), out);
            } else {
                write_rgba(gb.framebuffer(), &self.palette, out);
            }
        })
    }

    /// Whether the game runs on a Game Boy Color, in color or compatibility
    /// mode. The palette chosen with `set_palette` only applies to a DMG.
    pub fn is_cgb(&self) -> bool {
        on!(self.machine(), gb => gb.model() == Model::Cgb)
    }

    /// Whether a Game Boy Color runs a DMG game in compatibility mode.
    pub fn is_compat(&self) -> bool {
        on!(self.machine(), gb => gb.compatibility_mode())
    }

    /// In compatibility mode, chooses the colors: 36 bytes, red, green and
    /// blue for each of four shades (lightest first) of the background
    /// palette, then the sprite palettes for OBP0 and OBP1. Other lengths
    /// are ignored.
    pub fn set_compat_palettes(&mut self, rgb: &[u8]) {
        if rgb.len() != 36 {
            return;
        }
        let mut palettes = [[0u16; 4]; 3];
        for (color, channels) in palettes
            .as_flattened_mut()
            .iter_mut()
            .zip(rgb.chunks_exact(3))
        {
            let rgb = u32::from_be_bytes([0, channels[0], channels[1], channels[2]]);
            *color = rgb555(rgb);
        }
        on!(self.machine_mut(), gb => gb.set_compat_palettes(&palettes));
    }

    /// The Color palette RAM, background palettes then sprite palettes (128
    /// bytes); empty on a DMG.
    pub fn palette_ram(&self) -> Vec<u8> {
        on!(self.machine(), gb => match gb.palette_ram() {
            Some((background, sprites)) => [background, sprites].concat(),
            None => Vec::new(),
        })
    }

    /// Chooses the screen colors: 12 bytes, red, green and blue for each
    /// shade from lightest to darkest. Other lengths are ignored.
    pub fn set_palette(&mut self, rgb: &[u8]) {
        if rgb.len() != 12 {
            return;
        }
        for (color, channels) in self.palette.iter_mut().zip(rgb.chunks_exact(3)) {
            *color = [channels[0], channels[1], channels[2], 0xFF];
        }
    }

    /// Captures the whole machine as bytes.
    pub fn save_state(&self) -> Vec<u8> {
        on!(self.machine(), gb => gb.save_state())
    }

    /// Restores a state captured by `save_state` from the same cartridge.
    /// Throws when the data is not a state for this game. After a throw
    /// that is not about the wrong game or version, the machine may be half
    /// restored: load a known good state over it.
    pub fn load_state(&mut self, data: &[u8]) -> Result<(), JsError> {
        on!(self.machine_mut(), gb => gb.load_state(data))
            .map_err(|err| JsError::new(&err.to_string()))
    }

    /// Updates a button. `button` is 0-7 for Right, Left, Up, Down, A, B,
    /// Select, Start; other values are ignored.
    pub fn set_button(&mut self, button: u8, pressed: bool) {
        if let Some(&button) = Button::ALL.get(button as usize) {
            on!(self.machine_mut(), gb => gb.set_button(button, pressed));
        }
    }

    // ---- Link cable ---------------------------------------------------

    /// Plugs in or unplugs the link cable.
    pub fn set_linked(&mut self, linked: bool) {
        on!(self.machine_mut(), gb => gb.set_linked(linked))
    }

    /// Messages for the other console, as pairs of bytes: a kind (0 for
    /// data, 1 for a reply) followed by the value.
    pub fn take_link_messages(&mut self) -> Vec<u8> {
        let messages = on!(self.machine_mut(), gb => gb.take_link_messages());
        messages
            .into_iter()
            .flat_map(|message| match message {
                LinkMessage::Data(byte) => [0, byte],
                LinkMessage::Reply(byte) => [1, byte],
            })
            .collect()
    }

    /// Delivers messages from the other console, in the format of
    /// `take_link_messages`. Malformed input is ignored.
    pub fn receive_link_messages(&mut self, encoded: &[u8]) {
        for pair in encoded.chunks_exact(2) {
            let message = match pair[0] {
                0 => LinkMessage::Data(pair[1]),
                1 => LinkMessage::Reply(pair[1]),
                _ => continue,
            };
            on!(self.machine_mut(), gb => gb.receive_link_message(message));
        }
    }

    // ---- X-ray mode ----------------------------------------------------

    /// Switches event recording on or off without disturbing the game.
    pub fn set_xray(&mut self, enabled: bool) {
        let machine = self.machine.take().expect("machine is present");
        self.machine = Some(match (machine, enabled) {
            (Machine::Plain(gb), true) => Machine::Xray(gb.into_observed(Recorder {
                execute_breakpoints: AddressSet::new(&self.execute_breakpoints),
                write_breakpoints: AddressSet::new(&self.write_breakpoints),
                ..Recorder::default()
            })),
            (Machine::Xray(gb), false) => {
                Machine::Plain(gb.into_observed(glassbit_core::observer::NullObserver))
            }
            (unchanged, _) => unchanged,
        });
    }

    /// Sets the breakpoints: stop before executing an instruction at any of
    /// `execute`, or right after the CPU writes to any of `write`. They only
    /// take effect while recording (see `set_xray`).
    pub fn set_breakpoints(&mut self, execute: &[u16], write: &[u16]) {
        self.execute_breakpoints = execute.to_vec();
        self.write_breakpoints = write.to_vec();
        if let Some(Machine::Xray(gb)) = &mut self.machine {
            let recorder = gb.observer_mut();
            recorder.execute_breakpoints = AddressSet::new(execute);
            recorder.write_breakpoints = AddressSet::new(write);
        }
    }

    /// If the last frame, line or step stopped at a breakpoint, says which,
    /// as `"execute|0150"` or `"write|C000|07"` (hexadecimal), and clears
    /// it. Otherwise returns `undefined`.
    pub fn take_breakpoint_hit(&mut self) -> Option<String> {
        let Some(Machine::Xray(gb)) = &mut self.machine else {
            return None;
        };
        gb.observer_mut().hit.take().map(|hit| match hit {
            Hit::Execute(addr) => format!("execute|{addr:04X}"),
            Hit::Write(addr, value) => format!("write|{addr:04X}|{value:02X}"),
        })
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

/// Converts RGB555 to RGBA, widening each 5-bit channel to 8 bits.
fn write_color_rgba(framebuffer: &[u16], out: &mut [u8]) {
    let widen = |channel: u16| ((channel << 3) | (channel >> 2)) as u8;
    for (pixel, &color) in out.chunks_exact_mut(4).zip(framebuffer) {
        pixel.copy_from_slice(&[
            widen(color & 0x1F),
            widen(color >> 5 & 0x1F),
            widen(color >> 10 & 0x1F),
            0xFF,
        ]);
    }
}

/// Powers on a console as `Emulator::new` describes.
fn boot(rom: &[u8], model: &str) -> Result<GameBoy, glassbit_core::cartridge::CartridgeError> {
    match model {
        "dmg" => GameBoy::new_as(rom, Model::Dmg),
        "cgb" => GameBoy::new_as(rom, Model::Cgb),
        _ => GameBoy::new(rom),
    }
}

fn write_rgba(framebuffer: &[u8], palette: &[[u8; 4]; 4], out: &mut [u8]) {
    for (pixel, &shade) in out.chunks_exact_mut(4).zip(framebuffer) {
        pixel.copy_from_slice(&palette[shade as usize]);
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
        let mut emulator = Emulator::demo("auto");
        for _ in 0..12 {
            emulator.run_frame();
        }
        let mut rgba = vec![0u8; SCREEN_WIDTH * SCREEN_HEIGHT * 4];
        emulator.copy_frame(&mut rgba);

        assert!(rgba.chunks_exact(4).all(|pixel| pixel[3] == 0xFF));
        // The checkerboard starts with a blank tile, then a tinted one.
        assert_eq!(rgba[..4], DEFAULT_PALETTE[0]);
        assert_eq!(rgba[8 * 4..8 * 4 + 4], DEFAULT_PALETTE[1]);

        // Grayscale instead of green.
        emulator.set_palette(&[255, 255, 255, 170, 170, 170, 85, 85, 85, 0, 0, 0]);
        emulator.copy_frame(&mut rgba);
        assert_eq!(rgba[..4], [255, 255, 255, 255]);
        assert_eq!(rgba[8 * 4..8 * 4 + 4], [170, 170, 170, 255]);
        emulator.set_palette(&[1, 2, 3]);
        emulator.copy_frame(&mut rgba);
        assert_eq!(rgba[..4], [255, 255, 255, 255], "bad palettes are ignored");
    }

    #[test]
    fn save_state_round_trip_keeps_xray_recording_on() {
        let mut emulator = Emulator::demo("auto");
        for _ in 0..12 {
            emulator.run_frame();
        }
        emulator.set_xray(true);
        let state = emulator.save_state();
        let before = emulator.cpu_state();
        for _ in 0..5 {
            emulator.run_frame();
        }
        assert_ne!(emulator.cpu_state(), before);

        emulator
            .load_state(&state)
            .unwrap_or_else(|_| panic!("state loads"));
        assert_eq!(emulator.cpu_state(), before);
        assert!(emulator.xray());
    }

    #[test]
    fn buttons_reach_the_core_and_bad_indices_are_ignored() {
        let mut emulator = Emulator::demo("auto");
        emulator.set_button(4, true);
        assert!(is_pressed(&emulator, Button::A));
        emulator.set_button(4, false);
        assert!(!is_pressed(&emulator, Button::A));
        emulator.set_button(99, true);
    }

    #[test]
    fn frames_carry_audio_at_the_requested_rate() {
        let mut emulator = Emulator::demo("auto");
        emulator.set_sample_rate(44_100);
        emulator.run_frame();
        emulator.run_frame();
        // 44100 Hz / 59.73 fps is about 738 stereo samples per frame.
        let values = emulator.audio_samples().len();
        assert!((2 * 730..=2 * 746).contains(&values), "{values}");
    }

    #[test]
    fn reports_the_cartridge_title() {
        assert_eq!(Emulator::demo("auto").title(), "GLASSBIT DEMO");
    }

    #[test]
    fn demo_has_no_save_but_still_has_a_key() {
        let mut emulator = Emulator::demo("auto");
        assert!(!emulator.has_battery());
        assert!(emulator.save_data(0.0).is_none());
        assert!(!emulator.take_save_dirty());
        assert!(!emulator.load_save(&[0; 16], 0.0));
        assert!(emulator.save_key().starts_with("GLASSBIT DEMO-"));
    }

    #[test]
    fn xray_can_be_toggled_while_the_game_keeps_its_state() {
        let mut emulator = Emulator::demo("auto");
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
        let mut emulator = Emulator::demo("auto");
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
    fn execute_breakpoints_stop_a_frame_before_the_instruction() {
        let mut emulator = Emulator::demo("auto");
        // 0x0150 is the first instruction after the entry point's jump.
        emulator.set_breakpoints(&[0x0150], &[]);
        emulator.run_frame();
        assert_eq!(emulator.take_breakpoint_hit(), None, "only while recording");

        let mut emulator = Emulator::demo("auto");
        emulator.set_breakpoints(&[0x0150], &[]);
        emulator.set_xray(true);
        emulator.run_frame();
        assert_eq!(emulator.cpu_state()[5], 0x0150);
        assert_eq!(
            emulator.take_breakpoint_hit().as_deref(),
            Some("execute|0150")
        );
        assert_eq!(emulator.take_breakpoint_hit(), None, "taking clears it");

        // Resuming goes past it.
        emulator.run_frame();
        assert_ne!(emulator.cpu_state()[5], 0x0150);
    }

    #[test]
    fn write_breakpoints_stop_right_after_the_write() {
        let mut emulator = Emulator::demo("auto");
        emulator.set_xray(true);
        // The demo writes its sprite's Y position to OAM every frame, but
        // only after setup: let that finish first.
        for _ in 0..12 {
            emulator.run_frame();
        }
        emulator.set_breakpoints(&[], &[0xFE00]);
        emulator.run_frame();
        let hit = emulator.take_breakpoint_hit().expect("the write is caught");
        assert!(hit.starts_with("write|FE00|"), "{hit}");
        assert_eq!(emulator.peek(0xFE00), glassbit_core::demo::START_Y);

        // Breakpoints survive closing and reopening the panel. The stop
        // came in the middle of VBlank, so the first run only finishes that
        // frame; the next write comes in the frame after.
        emulator.set_xray(false);
        emulator.set_xray(true);
        emulator.run_frame();
        emulator.run_frame();
        assert!(emulator.take_breakpoint_hit().is_some());

        emulator.set_breakpoints(&[], &[]);
        emulator.run_frame();
        assert_eq!(emulator.take_breakpoint_hit(), None);
    }

    #[test]
    fn link_messages_round_trip_through_their_byte_encoding() {
        let mut emulator = Emulator::demo("auto");
        emulator.set_linked(true);
        assert!(emulator.take_link_messages().is_empty());
        // Unknown kinds and odd lengths are ignored rather than panicking.
        emulator.receive_link_messages(&[0, 0x42, 7, 1, 1]);
        emulator.set_linked(false);
    }

    #[test]
    fn color_conversion_widens_five_bit_channels() {
        let mut out = [0u8; 8];
        write_color_rgba(&[0x7FFF, 0x001F], &mut out);
        assert_eq!(out, [255, 255, 255, 255, 255, 0, 0, 255]);
    }

    #[test]
    fn the_demo_is_a_dmg_game() {
        let emulator = Emulator::demo("auto");
        assert!(!emulator.is_cgb());
        assert!(emulator.palette_ram().is_empty());
    }

    #[test]
    fn the_demo_on_a_color_runs_in_compatibility_mode() {
        let mut emulator = Emulator::demo("cgb");
        assert!(emulator.is_cgb() && emulator.is_compat());
        let red = [0xFF, 0x00, 0x00];
        emulator.set_compat_palettes(&red.repeat(12));
        for _ in 0..12 {
            emulator.run_frame();
        }
        let mut rgba = vec![0u8; SCREEN_WIDTH * SCREEN_HEIGHT * 4];
        emulator.copy_frame(&mut rgba);
        assert_eq!(rgba[..4], [0xFF, 0x00, 0x00, 0xFF]);
        assert_eq!(emulator.palette_ram()[..2], [0x1F, 0x00]);
    }

    #[test]
    fn stepping_and_inspection() {
        let mut emulator = Emulator::demo("auto");
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
