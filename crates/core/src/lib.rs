//! Platform-independent Game Boy (DMG) emulator core.
//!
//! This crate knows nothing about browsers, windows or audio devices. A
//! frontend loads a ROM, feeds button state in, runs one frame at a time and
//! reads the framebuffer and audio samples back out.

pub mod bus;
pub mod cartridge;
pub mod cpu;
pub mod interrupts;
pub mod observer;
pub mod serial;
pub mod timer;

use bus::Bus;
use cartridge::{Cartridge, CartridgeError, Header};
use cpu::Cpu;
use observer::{NullObserver, Observer};

/// Version of the core crate.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// Visible screen width in pixels.
pub const SCREEN_WIDTH: usize = 160;
/// Visible screen height in pixels.
pub const SCREEN_HEIGHT: usize = 144;

/// M-cycles in one video frame: 154 scanlines of 114 M-cycles each, which at
/// 1,048,576 M-cycles per second gives about 59.73 frames per second.
pub const CYCLES_PER_FRAME: u64 = 154 * 114;

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
    cpu: Cpu,
    bus: Bus<O>,
    /// The cycle count at which the frame being emulated ends.
    frame_end: u64,
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
            cpu: Cpu::post_boot(cartridge.header().header_checksum),
            bus: Bus::new(cartridge, observer),
            frame_end: CYCLES_PER_FRAME,
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
        self.bus.peek(addr)
    }

    /// The CPU state: registers, IME, halted.
    pub fn cpu(&self) -> &Cpu {
        &self.cpu
    }

    /// M-cycles elapsed since power on.
    pub fn cycles(&self) -> u64 {
        self.bus.cycles()
    }

    /// Returns the bytes sent through the serial port since the last call.
    pub fn take_serial_output(&mut self) -> Vec<u8> {
        self.bus.take_serial_output()
    }

    pub fn observer(&self) -> &O {
        &self.bus.observer
    }

    pub fn observer_mut(&mut self) -> &mut O {
        &mut self.bus.observer
    }

    /// Executes one CPU instruction, advancing all hardware along with it.
    pub fn step(&mut self) {
        let before = self.cpu.regs;
        self.cpu.step(&mut self.bus);
        self.bus.observer.instruction(&before, &self.cpu.regs);
    }

    /// Emulates one video frame's worth of time.
    ///
    /// Instructions are never cut short, so a frame may overshoot by a few
    /// cycles; the next one is shortened by the same amount.
    pub fn run_frame(&mut self) {
        self.audio.clear();
        while self.bus.cycles() < self.frame_end {
            self.step();
        }
        self.frame_end += CYCLES_PER_FRAME;
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

    /// A ROM whose entry point runs `program` and then loops forever.
    fn rom_running(program: &[u8]) -> Vec<u8> {
        let mut rom = RomBuilder::new().build();
        // The 4-byte entry point at 0x100 jumps past the header.
        rom[0x100..0x104].copy_from_slice(&[0x00, 0xC3, 0x50, 0x01]);
        rom[0x150..0x150 + program.len()].copy_from_slice(program);
        let end = 0x150 + program.len();
        // JR -2
        rom[end..end + 2].copy_from_slice(&[0x18, 0xFE]);
        rom
    }

    #[test]
    fn starts_at_the_entry_point_in_the_post_boot_state() {
        let gb = gameboy();
        assert_eq!(gb.cpu().regs.pc, 0x0100);
        assert_eq!(gb.cpu().regs.sp, 0xFFFE);
        assert_eq!(gb.cpu().regs.a, 0x01);
    }

    #[test]
    fn runs_a_program_that_writes_to_the_serial_port() {
        // LD A,'H' ; LDH (SB),A ; LD A,0x81 ; LDH (SC),A
        let rom = rom_running(&[0x3E, b'H', 0xE0, 0x01, 0x3E, 0x81, 0xE0, 0x02]);
        let mut gb = GameBoy::new(&rom).unwrap_or_else(|e| panic!("{e}"));
        gb.run_frame();
        assert_eq!(gb.take_serial_output(), b"H");
    }

    #[test]
    fn a_frame_lasts_17556_cycles() {
        let rom = rom_running(&[]);
        let mut gb = GameBoy::new(&rom).unwrap_or_else(|e| panic!("{e}"));
        gb.run_frame();
        assert!((CYCLES_PER_FRAME..CYCLES_PER_FRAME + 6).contains(&gb.cycles()));
        for _ in 0..59 {
            gb.run_frame();
        }
        assert!((60 * CYCLES_PER_FRAME..60 * CYCLES_PER_FRAME + 6).contains(&gb.cycles()));
    }

    #[derive(Default)]
    struct Trace {
        pcs: Vec<u16>,
    }

    impl Observer for Trace {
        fn instruction(&mut self, before: &cpu::Registers, _after: &cpu::Registers) {
            self.pcs.push(before.pc);
        }
    }

    #[test]
    fn observer_sees_every_instruction() {
        let rom = rom_running(&[0x00]);
        let mut gb =
            GameBoy::with_observer(&rom, Trace::default()).unwrap_or_else(|e| panic!("{e}"));
        for _ in 0..4 {
            gb.step();
        }
        assert_eq!(gb.observer().pcs, vec![0x0100, 0x0101, 0x0150, 0x0151]);
    }

    #[test]
    fn timer_interrupt_runs_its_handler() {
        // Handler at 0x50: LD B,0x99 ; RETI
        // Main: enable the timer interrupt, start the fastest timer from
        // 0xFF, EI, then spin.
        let mut rom = rom_running(&[
            0x3E, 0x04, 0xE0, 0xFF, // LD A,4 ; LDH (IE),A
            0xAF, 0xE0, 0x0F, // XOR A ; LDH (IF),A
            0x3E, 0xFF, 0xE0, 0x05, // LD A,0xFF ; LDH (TIMA),A
            0x3E, 0x05, 0xE0, 0x07, // LD A,5 ; LDH (TAC),A
            0xFB, // EI
        ]);
        rom[0x50..0x53].copy_from_slice(&[0x06, 0x99, 0xD9]);

        let mut gb =
            GameBoy::with_observer(&rom, Interrupts::default()).unwrap_or_else(|e| panic!("{e}"));
        for _ in 0..40 {
            gb.step();
        }
        assert_eq!(gb.cpu().regs.b, 0x99);
        assert!(gb.cpu().ime, "RETI turned interrupts back on");
        assert_eq!(gb.observer().bits[0], interrupts::TIMER);
    }

    #[derive(Default)]
    struct Interrupts {
        bits: Vec<u8>,
    }

    impl Observer for Interrupts {
        fn interrupt(&mut self, bit: u8) {
            self.bits.push(bit);
        }
    }

    #[test]
    fn rejects_an_empty_rom() {
        assert_eq!(
            GameBoy::new(&[]).err(),
            Some(CartridgeError::TooSmall { len: 0 })
        );
    }
}
