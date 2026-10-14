//! Platform-independent Game Boy (DMG) emulator core.
//!
//! This crate knows nothing about browsers, windows or audio devices. A
//! frontend loads a ROM, feeds button state in, runs one frame at a time and
//! reads the framebuffer and audio samples back out.

pub mod apu;
pub mod bus;
pub mod cartridge;
pub mod cpu;
pub mod demo;
pub mod disasm;
pub mod interrupts;
pub mod joypad;
pub mod observer;
pub mod ppu;
pub mod serial;
pub mod state;
pub mod timer;

use bus::{Bus, Hardware};
use cartridge::{Cartridge, CartridgeError, CgbSupport, Header};
use cpu::Cpu;
pub use joypad::Button;
use observer::{NullObserver, Observer};
use state::{Reader, StateError, Writer};

/// Version of the core crate.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// Visible screen width in pixels.
pub const SCREEN_WIDTH: usize = 160;
/// Visible screen height in pixels.
pub const SCREEN_HEIGHT: usize = 144;

/// M-cycles in one scanline (456 clock ticks).
pub const CYCLES_PER_LINE: u64 = 114;

/// M-cycles in one video frame: 154 scanlines of 114 M-cycles each, which at
/// 1,048,576 M-cycles per second gives about 59.73 frames per second.
pub const CYCLES_PER_FRAME: u64 = 154 * 114;

/// Which console to emulate.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Model {
    /// The original Game Boy.
    Dmg,
    /// The Game Boy Color, running a game in color mode.
    Cgb,
}

impl Model {
    /// The console a cartridge asks for: games that support the Color run in
    /// color mode, the rest on a DMG.
    pub fn for_header(header: &Header) -> Self {
        match header.cgb {
            CgbSupport::None => Model::Dmg,
            CgbSupport::Enhanced | CgbSupport::Only => Model::Cgb,
        }
    }
}

/// A Game Boy with a cartridge inserted.
///
/// The type parameter is the [`Observer`] that receives inspection events;
/// the default [`NullObserver`] costs nothing.
pub struct GameBoy<O: Observer = NullObserver> {
    cpu: Cpu,
    bus: Bus<O>,
    audio: Vec<f32>,
}

impl GameBoy {
    /// Powers on a console with the given ROM image inserted: a Game Boy
    /// Color if the game supports it, otherwise a DMG.
    pub fn new(rom: &[u8]) -> Result<Self, CartridgeError> {
        Self::with_observer(rom, NullObserver)
    }

    /// Like [`GameBoy::new`], choosing the console. A game made only for the
    /// DMG runs on a Game Boy Color in compatibility mode, which shows it
    /// in colors (see [`GameBoy::set_compat_palettes`]); a color game runs
    /// on a DMG as well as it can.
    pub fn new_as(rom: &[u8], model: Model) -> Result<Self, CartridgeError> {
        Self::with_observer_as(rom, NullObserver, Some(model))
    }
}

impl<O: Observer> GameBoy<O> {
    /// Like [`GameBoy::new`], reporting hardware events to `observer`.
    pub fn with_observer(rom: &[u8], observer: O) -> Result<Self, CartridgeError> {
        Self::with_observer_as(rom, observer, None)
    }

    fn with_observer_as(
        rom: &[u8],
        observer: O,
        model: Option<Model>,
    ) -> Result<Self, CartridgeError> {
        let cartridge = Cartridge::new(rom)?;
        let wanted = Model::for_header(cartridge.header());
        let hardware = match (model.unwrap_or(wanted), wanted) {
            (Model::Dmg, _) => Hardware::Dmg,
            (Model::Cgb, Model::Cgb) => Hardware::Cgb,
            (Model::Cgb, Model::Dmg) => Hardware::CgbCompat,
        };
        let cpu = match hardware {
            Hardware::Dmg => Cpu::post_boot(cartridge.header().header_checksum),
            Hardware::Cgb => Cpu::post_boot_cgb(),
            Hardware::CgbCompat => Cpu::post_boot_compat(),
        };
        Ok(Self {
            cpu,
            bus: Bus::new_model(cartridge, observer, hardware),
            audio: Vec::new(),
        })
    }

    /// The console being emulated.
    pub fn model(&self) -> Model {
        match self.bus.hardware() {
            Hardware::Dmg => Model::Dmg,
            Hardware::Cgb | Hardware::CgbCompat => Model::Cgb,
        }
    }

    /// Whether a Game Boy Color is running a DMG game in compatibility
    /// mode: drawn like on a DMG, but in colors.
    pub fn compatibility_mode(&self) -> bool {
        self.bus.hardware() == Hardware::CgbCompat
    }

    /// In compatibility mode, sets the colors DMG shades are shown with:
    /// the background palette, then the sprite palettes for OBP0 and OBP1,
    /// each four RGB555 colors from the lightest shade to the darkest. The
    /// default is [`ppu::DEFAULT_COMPAT_PALETTES`]. Does nothing otherwise.
    pub fn set_compat_palettes(&mut self, palettes: &[[u16; 4]; 3]) {
        self.bus.set_compat_palettes(palettes);
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

    /// The ROM bank the cartridge mapper currently shows at `0x4000`.
    pub fn rom_bank(&self) -> u16 {
        self.bus.cartridge().rom_bank()
    }

    /// Returns the bytes sent through the serial port since the last call.
    pub fn take_serial_output(&mut self) -> Vec<u8> {
        self.bus.take_serial_output()
    }

    /// Plugs in or unplugs a link cable. While plugged in, serial transfers
    /// wait for the other console instead of reading all ones; carry the
    /// messages between the two with [`GameBoy::take_link_messages`] and
    /// [`GameBoy::receive_link_message`].
    pub fn set_linked(&mut self, linked: bool) {
        self.bus.serial_mut().set_linked(linked);
    }

    /// Messages to deliver to the console at the other end of the cable.
    pub fn take_link_messages(&mut self) -> Vec<serial::LinkMessage> {
        self.bus.serial_mut().take_link_messages()
    }

    /// Delivers a message from the console at the other end of the cable.
    pub fn receive_link_message(&mut self, message: serial::LinkMessage) {
        self.bus.serial_mut().receive_link_message(message);
    }

    pub fn observer(&self) -> &O {
        &self.bus.observer
    }

    pub fn observer_mut(&mut self) -> &mut O {
        &mut self.bus.observer
    }

    /// Replaces the observer with one of another type without disturbing
    /// the running game: switch to a recording observer to inspect, and
    /// back to [`NullObserver`] for full speed.
    pub fn into_observed<N: Observer>(self, observer: N) -> GameBoy<N> {
        GameBoy {
            cpu: self.cpu,
            bus: self.bus.with_observer(observer),
            audio: self.audio,
        }
    }

    /// The PPU, for reading its mode and current line.
    pub fn ppu(&self) -> &ppu::Ppu {
        self.bus.ppu()
    }

    /// Decodes the instruction at `addr`.
    pub fn disassemble(&self, addr: u16) -> disasm::Instruction {
        disasm::disassemble(|addr| self.bus.peek(addr), addr)
    }

    /// Steps until the PPU moves to another scanline. With the LCD off,
    /// steps for the duration of one line instead.
    pub fn step_line(&mut self) {
        let line = self.bus.ppu().ly();
        let limit = self.bus.cycles() + CYCLES_PER_LINE * self.speed_factor();
        while self.bus.ppu().ly() == line && self.bus.cycles() < limit {
            self.step();
            if self.bus.observer.should_stop() {
                break;
            }
        }
    }

    /// Executes one CPU instruction, advancing all hardware along with it.
    pub fn step(&mut self) {
        let before = self.cpu.regs;
        self.cpu.step(&mut self.bus);
        self.bus.observer.instruction(&before, &self.cpu.regs);
    }

    /// Emulates until the PPU finishes drawing a frame (the start of VBlank).
    ///
    /// While the LCD is switched off no frame ever finishes, so this also
    /// returns after one frame's worth of time. It also returns early when
    /// the observer asks to stop (see [`Observer::should_stop`]).
    pub fn run_frame(&mut self) {
        // Anything produced by stepping outside of run_frame is dropped.
        self.bus.apu_mut().take_samples();
        self.audio.clear();
        let limit = self.bus.cycles() + CYCLES_PER_FRAME * self.speed_factor();
        while self.bus.cycles() < limit {
            self.step();
            if self.bus.take_frame_ready() || self.bus.observer.should_stop() {
                break;
            }
        }
        self.bus.sync_clock();
        self.audio.extend(self.bus.apu_mut().take_samples());
    }

    /// How many CPU M-cycles make up one M-cycle of video time: 2 while a
    /// Game Boy Color runs at double speed.
    fn speed_factor(&self) -> u64 {
        if self.bus.double_speed() {
            2
        } else {
            1
        }
    }

    /// The screen on a DMG: `SCREEN_WIDTH * SCREEN_HEIGHT` shades, row by
    /// row, where 0 is the lightest and 3 the darkest. Complete right after
    /// [`GameBoy::run_frame`] returns. In color mode use
    /// [`GameBoy::color_framebuffer`] instead.
    pub fn framebuffer(&self) -> &[u8] {
        self.bus.ppu().framebuffer()
    }

    /// The screen of a Game Boy Color, in color or compatibility mode: one
    /// RGB555 color per pixel (5 bits each, red in the lowest bits), row by
    /// row.
    pub fn color_framebuffer(&self) -> &[u16] {
        self.bus.ppu().color_framebuffer()
    }

    /// On a Game Boy Color, its palette RAM: background palettes, then
    /// sprite palettes (see [`ppu::Ppu::palette_ram`]).
    pub fn palette_ram(&self) -> Option<(&[u8], &[u8])> {
        (self.model() == Model::Cgb).then(|| self.bus.ppu().palette_ram())
    }

    /// The screen as 8-bit RGB, whichever the model: DMG shades become four
    /// grays, and 5-bit color channels are widened by repeating their top
    /// bits (`x << 3 | x >> 2`).
    pub fn rgb_framebuffer(&self) -> Vec<[u8; 3]> {
        const GRAYS: [u8; 4] = [0xFF, 0xAA, 0x55, 0x00];
        if self.model() == Model::Cgb {
            let widen = |channel: u16| ((channel << 3) | (channel >> 2)) as u8;
            self.color_framebuffer()
                .iter()
                .map(|&color| {
                    [
                        widen(color & 0x1F),
                        widen(color >> 5 & 0x1F),
                        widen(color >> 10 & 0x1F),
                    ]
                })
                .collect()
        } else {
            self.framebuffer()
                .iter()
                .map(|&shade| [GRAYS[shade as usize]; 3])
                .collect()
        }
    }

    /// Audio produced by the last call to [`GameBoy::run_frame`]:
    /// interleaved stereo samples (left, right) between -1.0 and 1.0, at the
    /// rate set with [`GameBoy::set_sample_rate`].
    pub fn audio_samples(&self) -> &[f32] {
        &self.audio
    }

    /// Sets the audio sample rate in Hz. The default is 48000.
    pub fn set_sample_rate(&mut self, hz: u32) {
        self.bus.apu_mut().set_sample_rate(hz);
    }

    /// Whether the cartridge has battery-backed RAM, i.e. can save.
    pub fn has_battery(&self) -> bool {
        self.bus.cartridge().has_battery()
    }

    /// What the cartridge battery keeps, for the frontend to store: the RAM
    /// and, on clock cartridges, the clock (see
    /// [`cartridge::Cartridge::save_data_at`]).
    pub fn save_data(&self) -> Option<Vec<u8>> {
        self.bus.cartridge().save_data()
    }

    /// Like [`GameBoy::save_data`], stamping the clock with `now` (seconds
    /// since 1970) so it can catch up on the time spent away when loaded.
    pub fn save_data_at(&self, now: u64) -> Option<Vec<u8>> {
        self.bus.cartridge().save_data_at(now)
    }

    /// Like [`GameBoy::load_save`], letting a saved clock catch up to `now`.
    pub fn load_save_at(&mut self, data: &[u8], now: u64) -> bool {
        self.bus.cartridge_mut().load_save_at(data, now)
    }

    /// Restores battery-backed RAM stored earlier. Call it right after
    /// creating the console, before running any frame. Returns `false` when
    /// the cartridge has no battery.
    pub fn load_save(&mut self, data: &[u8]) -> bool {
        self.bus.cartridge_mut().load_save(data)
    }

    /// Returns `true` if the save data changed since the last call, so the
    /// frontend knows when to store it again.
    pub fn take_save_dirty(&mut self) -> bool {
        self.bus.cartridge_mut().take_save_dirty()
    }

    /// Captures the whole machine as bytes: a save state. See [`state`]
    /// for the format.
    pub fn save_state(&self) -> Vec<u8> {
        let header = self.header();
        let mut w = Writer::new();
        w.bytes(&state::MAGIC);
        w.u8(state::VERSION);
        w.u16(header.global_checksum);
        w.u8(header.header_checksum);
        self.cpu.save_state(&mut w);
        self.bus.save_state(&mut w);
        w.finish()
    }

    /// Restores a state captured by [`GameBoy::save_state`] from the same
    /// cartridge. Buttons held at the moment of loading stay held.
    ///
    /// The header is checked before anything is touched, so a state from
    /// another game or version is rejected cleanly. If the rest turns out
    /// to be damaged the machine is left half restored and should be
    /// discarded.
    pub fn load_state(&mut self, data: &[u8]) -> Result<(), StateError> {
        let mut r = Reader::new(data);
        let mut magic = [0; 4];
        if r.bytes(&mut magic).is_err() || magic != state::MAGIC {
            return Err(StateError::NotAState);
        }
        let version = r.u8()?;
        if version != state::VERSION {
            return Err(StateError::UnsupportedVersion(version));
        }
        let header = self.header();
        if (r.u16()?, r.u8()?) != (header.global_checksum, header.header_checksum) {
            return Err(StateError::WrongCartridge);
        }
        self.cpu.load_state(&mut r)?;
        self.bus.load_state(&mut r)?;
        r.finish()?;
        self.audio.clear();
        Ok(())
    }

    /// Updates whether `button` is currently held.
    pub fn set_button(&mut self, button: Button, pressed: bool) {
        self.bus.set_button(button, pressed);
    }

    /// Whether `button` is currently held.
    pub fn is_pressed(&self, button: Button) -> bool {
        self.bus.is_pressed(button)
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
    fn frames_end_at_vblank_and_last_17556_cycles() {
        let rom = rom_running(&[]);
        let mut gb = GameBoy::new(&rom).unwrap_or_else(|e| panic!("{e}"));
        gb.run_frame();
        assert_eq!(gb.peek(0xFF44), 144, "stopped where VBlank begins");
        let start = gb.cycles();
        for _ in 0..60 {
            gb.run_frame();
        }
        let elapsed = gb.cycles() - start;
        // Instructions are never cut short, hence the small slack.
        assert!((60 * CYCLES_PER_FRAME - 6..60 * CYCLES_PER_FRAME + 6).contains(&elapsed));
    }

    #[test]
    fn run_frame_returns_even_with_the_lcd_off() {
        // XOR A ; LDH (LCDC),A
        let rom = rom_running(&[0xAF, 0xE0, 0x40]);
        let mut gb = GameBoy::new(&rom).unwrap_or_else(|e| panic!("{e}"));
        gb.run_frame();
        let start = gb.cycles();
        gb.run_frame();
        assert!(gb.cycles() - start >= CYCLES_PER_FRAME);
        assert_eq!(gb.peek(0xFF44), 0);
    }

    #[test]
    fn draws_what_the_program_puts_in_video_memory() {
        // Wait for VBlank, then make tile 0 solid black: every background
        // tile is tile 0, so the whole screen turns black.
        let rom = rom_running(&[
            0xF0, 0x44, // LDH A,(LY)
            0xFE, 0x90, // CP 144
            0x20, 0xFA, // JR NZ,-6
            0x21, 0x00, 0x80, // LD HL,0x8000
            0x06, 0x10, // LD B,16
            0x3E, 0xFF, // LD A,0xFF
            0x22, // LD (HL+),A
            0x05, // DEC B
            0x20, 0xFC, // JR NZ,-4
        ]);
        let mut gb = GameBoy::new(&rom).unwrap_or_else(|e| panic!("{e}"));
        gb.run_frame();
        assert!(gb.framebuffer().iter().all(|&shade| shade == 0));
        gb.run_frame();
        assert!(gb.framebuffer().iter().all(|&shade| shade == 3));
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

    #[derive(Default)]
    struct Banks {
        seen: Vec<u16>,
    }

    impl Observer for Banks {
        fn rom_bank_switch(&mut self, bank: u16) {
            self.seen.push(bank);
        }
    }

    #[test]
    fn a_game_can_save_and_the_observer_sees_bank_switches() {
        // Enable RAM, write 0x5A to 0xA000, select ROM bank 2 (twice).
        let mut rom = RomBuilder::new()
            .cartridge_type(0x13)
            .rom_size_code(1)
            .ram_size_code(2)
            .build();
        rom[0x100..0x104].copy_from_slice(&[0x00, 0xC3, 0x50, 0x01]);
        let program = [
            0x3E, 0x0A, 0xEA, 0x00, 0x00, // LD A,0x0A ; LD (0x0000),A
            0x3E, 0x5A, 0xEA, 0x00, 0xA0, // LD A,0x5A ; LD (0xA000),A
            0x3E, 0x02, 0xEA, 0x00, 0x20, // LD A,2 ; LD (0x2000),A
            0xEA, 0x00, 0x20, // LD (0x2000),A
            0x18, 0xFE, // JR -2
        ];
        rom[0x150..0x150 + program.len()].copy_from_slice(&program);

        let mut gb =
            GameBoy::with_observer(&rom, Banks::default()).unwrap_or_else(|e| panic!("{e}"));
        assert!(gb.has_battery());
        gb.run_frame();
        assert!(gb.take_save_dirty());
        assert_eq!(gb.save_data().map(|ram| ram[0]), Some(0x5A));
        assert_eq!(
            gb.observer().seen,
            vec![2],
            "only actual changes are reported"
        );

        let mut restored = GameBoy::new(&rom).unwrap_or_else(|e| panic!("{e}"));
        let saved = gb.save_data().unwrap().to_vec();
        assert!(restored.load_save(&saved));
        assert_eq!(restored.save_data().map(|ram| ram[0]), Some(0x5A));
    }

    #[test]
    fn each_frame_comes_with_its_share_of_audio() {
        let rom = rom_running(&[]);
        let mut gb = GameBoy::new(&rom).unwrap_or_else(|e| panic!("{e}"));
        gb.set_sample_rate(48_000);
        gb.run_frame();
        let mut values = 0;
        for _ in 0..60 {
            gb.run_frame();
            values += gb.audio_samples().len();
        }
        // 60 frames last 60 * 17556 / 1048576 s; two values per sample.
        let expected = 2.0 * 48_000.0 * 60.0 * CYCLES_PER_FRAME as f64 / 1_048_576.0;
        assert!(
            (values as f64 - expected).abs() < 4.0,
            "{values} vs {expected}"
        );
    }

    #[test]
    fn observer_can_be_swapped_without_disturbing_the_game() {
        let rom = demo::rom();
        let mut plain = GameBoy::new(&rom).unwrap_or_else(|e| panic!("{e}"));
        let mut swapped = GameBoy::new(&rom).unwrap_or_else(|e| panic!("{e}"));
        for _ in 0..8 {
            plain.run_frame();
            swapped.run_frame();
        }

        // Watch four instructions, then go back to not watching.
        let mut watched = swapped.into_observed(Trace::default());
        for _ in 0..4 {
            watched.step();
            plain.step();
        }
        assert_eq!(watched.observer().pcs.len(), 4);
        let mut swapped = watched.into_observed(NullObserver);

        for _ in 0..8 {
            plain.run_frame();
            swapped.run_frame();
        }
        assert_eq!(swapped.cycles(), plain.cycles());
        assert_eq!(swapped.cpu(), plain.cpu());
        assert_eq!(swapped.framebuffer(), plain.framebuffer());
    }

    #[test]
    fn step_line_stops_at_the_next_scanline() {
        let rom = rom_running(&[]);
        let mut gb = GameBoy::new(&rom).unwrap_or_else(|e| panic!("{e}"));
        for expected in 1..=5 {
            gb.step_line();
            assert_eq!(gb.ppu().ly(), expected);
        }
    }

    #[test]
    fn disassembles_code_in_the_cartridge() {
        let gb = gameboy();
        let rom = rom_running(&[0x3E, 0x42]);
        let running = GameBoy::new(&rom).unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(running.disassemble(0x0101).to_string(), "JP $0150");
        assert_eq!(running.disassemble(0x0150).to_string(), "LD A,$42");
        assert_eq!(gb.disassemble(0x0100).length, 1);
    }

    /// Runs `frames` frames and collects everything observable about them.
    fn trace(gb: &mut GameBoy, frames: usize) -> Vec<(Vec<u8>, Vec<f32>, Cpu, u64)> {
        (0..frames)
            .map(|_| {
                gb.run_frame();
                (
                    gb.framebuffer().to_vec(),
                    gb.audio_samples().to_vec(),
                    *gb.cpu(),
                    gb.cycles(),
                )
            })
            .collect()
    }

    /// Compares two traces part by part, so a failure names what diverged
    /// instead of printing whole framebuffers.
    fn assert_same(
        actual: &[(Vec<u8>, Vec<f32>, Cpu, u64)],
        expected: &[(Vec<u8>, Vec<f32>, Cpu, u64)],
    ) {
        assert_eq!(actual.len(), expected.len());
        for (frame, (a, e)) in actual.iter().zip(expected).enumerate() {
            assert_eq!(a.3, e.3, "cycle count, frame {frame}");
            assert_eq!(a.2, e.2, "CPU, frame {frame}");
            assert!(a.0 == e.0, "screen differs in frame {frame}");
            assert!(a.1 == e.1, "audio differs in frame {frame}");
        }
    }

    #[test]
    fn a_loaded_state_continues_exactly_like_the_original() {
        let rom = demo::rom();
        let mut original = GameBoy::new(&rom).unwrap_or_else(|e| panic!("{e}"));
        original.set_button(Button::Right, true);
        // Stop in the middle of the chime, with the sprite moving.
        for _ in 0..20 {
            original.run_frame();
        }
        let state = original.save_state();

        let mut restored = GameBoy::new(&rom).unwrap_or_else(|e| panic!("{e}"));
        restored.set_button(Button::Right, true);
        assert_eq!(restored.load_state(&state), Ok(()));
        assert_eq!(
            restored.save_state(),
            state,
            "saving again gives the same bytes"
        );

        assert_same(&trace(&mut restored, 40), &trace(&mut original, 40));
    }

    #[test]
    fn loading_rewinds_a_running_game() {
        let mut gb = GameBoy::new(&demo::rom()).unwrap_or_else(|e| panic!("{e}"));
        for _ in 0..15 {
            gb.run_frame();
        }
        let state = gb.save_state();
        let expected = trace(&mut gb, 10);
        // Wander off, then come back.
        gb.set_button(Button::Down, true);
        trace(&mut gb, 25);
        gb.set_button(Button::Down, false);
        assert_eq!(gb.load_state(&state), Ok(()));
        assert_same(&trace(&mut gb, 10), &expected);
    }

    #[test]
    fn mapper_registers_and_ram_survive_a_save_state() {
        // (cartridge type, bank to select)
        for (code, bank) in [(0x03u8, 3u8), (0x13, 5), (0x1B, 6)] {
            let rom = RomBuilder::new()
                .cartridge_type(code)
                .rom_size_code(2)
                .ram_size_code(3)
                .build();
            let mut gb = GameBoy::new(&rom).unwrap_or_else(|e| panic!("{e}"));
            gb.bus.poke(0x0000, 0x0A);
            gb.bus.poke(0x2000, bank);
            gb.bus.poke(0xA010, 0x5A);
            let state = gb.save_state();

            let mut restored = GameBoy::new(&rom).unwrap_or_else(|e| panic!("{e}"));
            assert_eq!(restored.load_state(&state), Ok(()), "type {code:#04x}");
            assert_eq!(restored.rom_bank(), bank as u16, "type {code:#04x}");
            assert_eq!(restored.peek(0xA010), 0x5A, "type {code:#04x}");
            assert!(
                restored.take_save_dirty(),
                "the frontend must store the RAM"
            );
        }
    }

    #[test]
    fn states_from_elsewhere_are_rejected_before_anything_changes() {
        let mut gb = gameboy();
        let state = gb.save_state();
        let before = *gb.cpu();

        assert_eq!(gb.load_state(b"not a state"), Err(StateError::NotAState));
        assert_eq!(gb.load_state(&[]), Err(StateError::NotAState));

        let mut newer = state.clone();
        newer[4] = state::VERSION + 1;
        assert_eq!(
            gb.load_state(&newer),
            Err(StateError::UnsupportedVersion(state::VERSION + 1))
        );

        let other = GameBoy::new(&demo::rom()).unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(
            gb.load_state(&other.save_state()),
            Err(StateError::WrongCartridge)
        );
        assert_eq!(*gb.cpu(), before);
    }

    #[test]
    fn damaged_states_are_reported() {
        let mut gb = gameboy();
        let state = gb.save_state();

        assert_eq!(
            gb.load_state(&state[..state.len() - 1]),
            Err(StateError::Corrupt)
        );
        let mut longer = state.clone();
        longer.push(0);
        assert_eq!(gb.load_state(&longer), Err(StateError::Corrupt));
        assert_eq!(gb.load_state(&state), Ok(()));
    }

    /// Stops when PC reaches an address, like a debugger breakpoint.
    struct StopAt {
        address: u16,
        hit: bool,
        writes: Vec<(u16, u8)>,
    }

    impl Observer for StopAt {
        fn instruction(&mut self, _before: &cpu::Registers, after: &cpu::Registers) {
            self.hit |= after.pc == self.address;
        }

        fn memory_write(&mut self, addr: u16, value: u8) {
            self.writes.push((addr, value));
        }

        fn should_stop(&mut self) -> bool {
            std::mem::take(&mut self.hit)
        }
    }

    #[test]
    fn observer_can_stop_a_frame_early_and_sees_every_write() {
        // LD A,7 ; LD ($C000),A ; then spin.
        let rom = rom_running(&[0x3E, 0x07, 0xEA, 0x00, 0xC0]);
        let observer = StopAt {
            address: 0x0152,
            hit: false,
            writes: Vec::new(),
        };
        let mut gb = GameBoy::with_observer(&rom, observer).unwrap_or_else(|e| panic!("{e}"));
        gb.run_frame();
        assert_eq!(gb.cpu().regs.pc, 0x0152, "stopped before the store");
        assert!(gb.cycles() < 100);
        assert!(gb.observer().writes.is_empty());

        // Running again executes the store and goes on to the end of the frame.
        gb.run_frame();
        assert_eq!(gb.observer().writes, vec![(0xC000, 0x07)]);
        assert!(gb.cycles() > 10_000);
    }

    /// Two consoles whose cable is a function call: everything one side
    /// sends is delivered after each frame.
    fn run_linked(a: &mut GameBoy, b: &mut GameBoy, frames: usize) {
        for _ in 0..frames {
            a.run_frame();
            b.run_frame();
            for message in a.take_link_messages() {
                b.receive_link_message(message);
            }
            for message in b.take_link_messages() {
                a.receive_link_message(message);
            }
        }
    }

    /// A program that exchanges `byte` over the link and stores what it gets
    /// at 0xC000: the master drives the clock, the other side waits for it.
    fn link_program(byte: u8, master: bool) -> Vec<u8> {
        let control = if master { 0x81 } else { 0x80 };
        rom_running(&[
            0x3E, byte, 0xE0, 0x01, // LD A,byte ; LDH (SB),A
            0x3E, control, 0xE0, 0x02, // LD A,control ; LDH (SC),A
            0xF0, 0x02, // wait: LDH A,(SC)
            0xE6, 0x80, // AND 0x80
            0x20, 0xFA, // JR NZ,wait
            0xF0, 0x01, 0xEA, 0x00, 0xC0, // LDH A,(SB) ; LD ($C000),A
        ])
    }

    #[test]
    fn two_linked_consoles_exchange_a_byte() {
        let mut master = GameBoy::new(&link_program(0x42, true)).unwrap_or_else(|e| panic!("{e}"));
        let mut slave = GameBoy::new(&link_program(0x99, false)).unwrap_or_else(|e| panic!("{e}"));
        master.set_linked(true);
        slave.set_linked(true);
        run_linked(&mut master, &mut slave, 5);
        assert_eq!(master.peek(0xC000), 0x99);
        assert_eq!(slave.peek(0xC000), 0x42);
    }

    #[test]
    fn without_a_cable_the_master_reads_all_ones() {
        let mut master = GameBoy::new(&link_program(0x42, true)).unwrap_or_else(|e| panic!("{e}"));
        master.run_frame();
        assert_eq!(master.peek(0xC000), 0xFF);
    }

    /// A ROM that declares Color support and runs `program`.
    fn color_rom_running(program: &[u8]) -> Vec<u8> {
        let mut rom = rom_running(program);
        rom[0x143] = 0x80;
        rom
    }

    #[test]
    fn color_games_start_a_game_boy_color() {
        let gb = GameBoy::new(&color_rom_running(&[])).unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(gb.model(), Model::Cgb);
        assert_eq!(gb.cpu().regs.a, 0x11, "how games detect a Color");

        let dmg =
            GameBoy::new_as(&color_rom_running(&[]), Model::Dmg).unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(dmg.model(), Model::Dmg);
        assert_eq!(dmg.cpu().regs.a, 0x01);

        let old = GameBoy::new_as(&rom_running(&[]), Model::Cgb).unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(old.model(), Model::Cgb);
        assert!(old.compatibility_mode());
        assert_eq!(old.cpu().regs.a, 0x11);
        assert!(!gb.compatibility_mode() && !dmg.compatibility_mode());
    }

    #[test]
    fn dmg_games_on_a_color_show_shades_through_its_palettes() {
        // The post-boot screen is blank: every pixel is background color 0,
        // which BGP (0xFC) makes the lightest shade.
        let mut gb =
            GameBoy::new_as(&rom_running(&[]), Model::Cgb).unwrap_or_else(|e| panic!("{e}"));
        gb.run_frame();
        gb.run_frame();
        let lightest = ppu::DEFAULT_COMPAT_PALETTES[0][0];
        assert!(gb
            .color_framebuffer()
            .iter()
            .all(|&color| color == lightest));

        let red = ppu::rgb555(0xFF0000);
        gb.set_compat_palettes(&[[red; 4], [0; 4], [0; 4]]);
        gb.run_frame();
        assert_eq!(gb.rgb_framebuffer()[0], [0xFF, 0x00, 0x00]);
        // The game cannot reach the palettes: the boot ROM locked them.
        assert_eq!(gb.peek(0xFF69), 0xFF);
        assert_eq!(gb.peek(0xFF68), 0xC8);
    }

    #[test]
    fn color_games_draw_in_color() {
        // Set background palette 0, color 0 to pure red (RGB555 0x001F).
        let rom = color_rom_running(&[
            0x3E, 0x80, 0xE0, 0x68, // LD A,0x80 ; LDH (BCPS),A
            0x3E, 0x1F, 0xE0, 0x69, // LD A,0x1F ; LDH (BCPD),A
            0xAF, 0xE0, 0x69, // XOR A ; LDH (BCPD),A
        ]);
        let mut gb = GameBoy::new(&rom).unwrap_or_else(|e| panic!("{e}"));
        gb.run_frame();
        gb.run_frame();
        assert!(gb.color_framebuffer().iter().all(|&color| color == 0x001F));
        assert_eq!(gb.rgb_framebuffer()[0], [0xFF, 0x00, 0x00]);
    }

    #[test]
    fn double_speed_keeps_frames_the_same_length_in_real_time() {
        // LD A,1 ; LDH (KEY1),A ; STOP
        let rom = color_rom_running(&[0x3E, 0x01, 0xE0, 0x4D, 0x10]);
        let mut gb = GameBoy::new(&rom).unwrap_or_else(|e| panic!("{e}"));
        gb.run_frame();
        gb.run_frame();
        let start = gb.cycles();
        gb.run_frame();
        assert_eq!(gb.peek(0xFF4D) & 0x80, 0x80, "double speed");
        let elapsed = gb.cycles() - start;
        assert!(
            (2 * CYCLES_PER_FRAME - 8..2 * CYCLES_PER_FRAME + 8).contains(&elapsed),
            "{elapsed} CPU cycles per frame"
        );
    }

    #[test]
    fn a_color_state_does_not_load_into_a_dmg() {
        let rom = color_rom_running(&[]);
        let color = GameBoy::new(&rom).unwrap_or_else(|e| panic!("{e}"));
        let mut dmg = GameBoy::new_as(&rom, Model::Dmg).unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(
            dmg.load_state(&color.save_state()),
            Err(StateError::Corrupt)
        );
    }

    #[test]
    fn rejects_an_empty_rom() {
        assert_eq!(
            GameBoy::new(&[]).err(),
            Some(CartridgeError::TooSmall { len: 0 })
        );
    }
}
