//! The memory bus: routes every 16-bit address to the hardware behind it.
//!
//! | Range             | What lives there                         |
//! | ----------------- | ---------------------------------------- |
//! | `0x0000..=0x7FFF` | Cartridge ROM (through the mapper)       |
//! | `0x8000..=0x9FFF` | Video RAM (owned by the PPU)             |
//! | `0xA000..=0xBFFF` | Cartridge RAM (through the mapper)       |
//! | `0xC000..=0xDFFF` | Work RAM                                 |
//! | `0xE000..=0xFDFF` | Echo of `0xC000..=0xDDFF`                |
//! | `0xFE00..=0xFE9F` | Object attribute memory (owned by the PPU) |
//! | `0xFEA0..=0xFEFF` | Unusable                                 |
//! | `0xFF00..=0xFF7F` | I/O registers                            |
//! | `0xFF80..=0xFFFE` | High RAM                                 |
//! | `0xFFFF`          | Interrupt enable register                |
//!
//! Reference: <https://gbdev.io/pandocs/Memory_Map.html>
//!
//! [`Bus::peek`] and [`Bus::poke`] access memory without the passage of time.
//! The CPU goes through the [`CpuBus`] implementation instead, where every
//! access first advances the hardware by one M-cycle.

use crate::apu::{self, Apu};
use crate::cartridge::Cartridge;
use crate::cpu::CpuBus;
use crate::interrupts;
use crate::joypad::{self, Button, Joypad};
use crate::observer::{NullObserver, Observer};
use crate::ppu::{self, Ppu};
use crate::serial::{self, Serial};
use crate::state::{Reader, StateError, Writer};
use crate::timer::{self, Timer};

/// OAM DMA: writing a page number here copies 160 bytes from that page into
/// OAM, one byte per M-cycle.
const DMA: u16 = 0xFF46;
const DMA_LENGTH: u8 = 160;
/// M-cycles between the write to DMA and the first byte being copied.
const DMA_START_DELAY: u8 = 2;

const WRAM_SIZE: usize = 0x2000;
const IO_SIZE: usize = 0x80;
const HRAM_SIZE: usize = 0x7F;

/// Everything the CPU can address.
pub struct Bus<O: Observer = NullObserver> {
    cartridge: Cartridge,
    /// On the heap so that moving a `Bus` stays cheap.
    wram: Box<[u8]>,
    /// I/O addresses that no component claims. They read `0xFF` until
    /// something is written, which only matters to badly behaved programs.
    io: [u8; IO_SIZE],
    hram: [u8; HRAM_SIZE],
    /// IF: one bit per interrupt source that is asking for service.
    interrupt_flag: u8,
    /// IE: which sources the program wants to be interrupted by.
    interrupt_enable: u8,
    joypad: Joypad,
    serial: Serial,
    timer: Timer,
    ppu: Ppu,
    apu: Apu,
    /// The last value written to the DMA register.
    dma_page: u8,
    /// M-cycles until a requested DMA transfer starts; 0 when none is waiting.
    dma_delay: u8,
    /// Whether a transfer is copying bytes right now.
    dma_active: bool,
    /// The page the running transfer reads from.
    dma_source: u8,
    /// The next byte of the running transfer.
    dma_index: u8,
    /// A byte was copied in the current M-cycle, which keeps the CPU away
    /// from OAM for that cycle.
    dma_copying: bool,
    /// M-cycles elapsed since power on.
    cycles: u64,
    pub observer: O,
}

impl<O: Observer> Bus<O> {
    /// Creates a bus in the post-boot state with `cartridge` inserted.
    pub fn new(cartridge: Cartridge, observer: O) -> Self {
        Self {
            cartridge,
            wram: vec![0; WRAM_SIZE].into_boxed_slice(),
            io: [0xFF; IO_SIZE],
            hram: [0; HRAM_SIZE],
            // The boot ROM leaves a VBlank request behind.
            interrupt_flag: interrupts::VBLANK,
            interrupt_enable: 0x00,
            joypad: Joypad::new(),
            serial: Serial::new(),
            timer: Timer::new(),
            ppu: Ppu::new(),
            apu: Apu::new(),
            dma_page: 0xFF,
            dma_delay: 0,
            dma_active: false,
            dma_source: 0,
            dma_index: 0,
            dma_copying: false,
            cycles: 0,
            observer,
        }
    }

    /// Swaps the observer for one of another type, keeping all hardware
    /// state. This is how inspection is switched on and off at run time
    /// while still costing nothing when off.
    pub fn with_observer<N: Observer>(self, observer: N) -> Bus<N> {
        Bus {
            cartridge: self.cartridge,
            wram: self.wram,
            io: self.io,
            hram: self.hram,
            interrupt_flag: self.interrupt_flag,
            interrupt_enable: self.interrupt_enable,
            joypad: self.joypad,
            serial: self.serial,
            timer: self.timer,
            ppu: self.ppu,
            apu: self.apu,
            dma_page: self.dma_page,
            dma_delay: self.dma_delay,
            dma_active: self.dma_active,
            dma_source: self.dma_source,
            dma_index: self.dma_index,
            dma_copying: self.dma_copying,
            cycles: self.cycles,
            observer,
        }
    }

    pub(crate) fn save_state(&self, w: &mut Writer) {
        self.cartridge.save_state(w);
        w.bytes(&self.wram);
        w.bytes(&self.io);
        w.bytes(&self.hram);
        w.u8(self.interrupt_flag);
        w.u8(self.interrupt_enable);
        self.joypad.save_state(w);
        self.serial.save_state(w);
        self.timer.save_state(w);
        self.ppu.save_state(w);
        self.apu.save_state(w);
        w.u8(self.dma_page);
        w.u8(self.dma_delay);
        w.bool(self.dma_active);
        w.u8(self.dma_source);
        w.u8(self.dma_index);
        w.bool(self.dma_copying);
        w.u64(self.cycles);
    }

    pub(crate) fn load_state(&mut self, r: &mut Reader) -> Result<(), StateError> {
        self.cartridge.load_state(r)?;
        r.bytes(&mut self.wram)?;
        r.bytes(&mut self.io)?;
        r.bytes(&mut self.hram)?;
        self.interrupt_flag = r.u8()? & interrupts::MASK;
        self.interrupt_enable = r.u8()?;
        self.joypad.load_state(r)?;
        self.serial.load_state(r)?;
        self.timer.load_state(r)?;
        self.ppu.load_state(r)?;
        self.apu.load_state(r)?;
        self.dma_page = r.u8()?;
        self.dma_delay = r.u8()?;
        self.dma_active = r.bool()?;
        self.dma_source = r.u8()?;
        self.dma_index = r.u8()?;
        self.dma_copying = r.bool()?;
        self.cycles = r.u64()?;
        // A transfer in progress indexes OAM with this.
        if self.dma_active && self.dma_index >= DMA_LENGTH {
            return Err(StateError::Corrupt);
        }
        Ok(())
    }

    pub fn cartridge(&self) -> &Cartridge {
        &self.cartridge
    }

    pub fn cartridge_mut(&mut self) -> &mut Cartridge {
        &mut self.cartridge
    }

    /// Updates whether `button` is held, requesting the joypad interrupt
    /// when the program is listening for it.
    pub fn set_button(&mut self, button: Button, pressed: bool) {
        if self.joypad.set_button(button, pressed) {
            self.interrupt_flag |= interrupts::JOYPAD;
        }
    }

    pub fn is_pressed(&self, button: Button) -> bool {
        self.joypad.is_pressed(button)
    }

    pub fn apu(&self) -> &Apu {
        &self.apu
    }

    pub fn apu_mut(&mut self) -> &mut Apu {
        &mut self.apu
    }

    pub fn ppu(&self) -> &Ppu {
        &self.ppu
    }

    /// Returns `true` once after each frame is fully drawn.
    pub fn take_frame_ready(&mut self) -> bool {
        self.ppu.take_frame_ready()
    }

    /// M-cycles elapsed since power on.
    pub fn cycles(&self) -> u64 {
        self.cycles
    }

    /// Returns the bytes sent through the serial port since the last call.
    pub fn take_serial_output(&mut self) -> Vec<u8> {
        self.serial.take_output()
    }

    /// Advances every component by one M-cycle.
    fn tick(&mut self) {
        self.cycles += 1;

        if self.timer.tick() {
            self.interrupt_flag |= interrupts::TIMER;
        }
        if self.serial.tick() {
            self.interrupt_flag |= interrupts::SERIAL;
        }

        let mode = self.ppu.mode();
        self.ppu.tick();
        if self.ppu.mode() != mode {
            self.observer.ppu_mode(self.ppu.mode(), self.ppu.ly());
        }
        self.interrupt_flag |= self.ppu.take_interrupts();

        self.apu.tick();
        self.tick_dma();
    }

    fn tick_dma(&mut self) {
        // A new request does not stop a running transfer until it starts.
        if self.dma_delay > 0 {
            self.dma_delay -= 1;
            if self.dma_delay == 0 {
                self.dma_active = true;
                self.dma_source = self.dma_page;
                self.dma_index = 0;
            }
        }
        self.dma_copying = self.dma_active;
        if self.dma_active {
            let source = u16::from_be_bytes([self.dma_source, self.dma_index]);
            // Pages above 0xDF are not decoded and read work RAM instead.
            let value = match source {
                0xE000..=0xFFFF => self.wram[(source - 0xE000) as usize],
                _ => self.peek(source),
            };
            self.ppu.write_oam(0xFE00 + self.dma_index as u16, value);
            self.dma_index += 1;
            self.dma_active = self.dma_index < DMA_LENGTH;
        }
    }

    /// Whether the CPU is currently cut off from `addr`: reads return `0xFF`
    /// and writes are lost. The PPU keeps video memory to itself while it is
    /// using it, and a DMA transfer does the same with OAM.
    fn cpu_locked_out(&self, addr: u16) -> bool {
        match addr {
            0x8000..=0x9FFF => self.ppu.vram_locked(),
            0xFE00..=0xFE9F => self.dma_copying || self.ppu.oam_locked(),
            _ => false,
        }
    }

    /// Reads the byte the CPU would see at `addr`, without advancing time
    /// and ignoring any lockout.
    pub fn peek(&self, addr: u16) -> u8 {
        match addr {
            0x0000..=0x7FFF => self.cartridge.read_rom(addr),
            0x8000..=0x9FFF => self.ppu.read_vram(addr),
            0xA000..=0xBFFF => self.cartridge.read_ram(addr),
            0xC000..=0xDFFF => self.wram[(addr - 0xC000) as usize],
            0xE000..=0xFDFF => self.wram[(addr - 0xE000) as usize],
            0xFE00..=0xFE9F => self.ppu.read_oam(addr),
            // On DMG this area reads zero.
            0xFEA0..=0xFEFF => 0x00,
            joypad::P1 => self.joypad.read(),
            serial::SB | serial::SC => self.serial.read(addr),
            timer::DIV..=timer::TAC => self.timer.read(addr),
            // The upper three bits do not exist and read as 1.
            interrupts::IF => self.interrupt_flag | !interrupts::MASK,
            apu::NR10..=apu::WAVE_RAM_END => self.apu.read(addr),
            DMA => self.dma_page,
            ppu::LCDC..=ppu::WX => self.ppu.read_register(addr),
            0xFF01..=0xFF7F => self.io[(addr - 0xFF00) as usize],
            0xFF80..=0xFFFE => self.hram[(addr - 0xFF80) as usize],
            interrupts::IE => self.interrupt_enable,
        }
    }

    /// Writes `value` to `addr` as the CPU would, without advancing time.
    pub fn poke(&mut self, addr: u16, value: u8) {
        match addr {
            0x0000..=0x7FFF => {
                let bank = self.cartridge.rom_bank();
                self.cartridge.write_rom(addr, value);
                if self.cartridge.rom_bank() != bank {
                    self.observer.rom_bank_switch(self.cartridge.rom_bank());
                }
            }
            0x8000..=0x9FFF => self.ppu.write_vram(addr, value),
            0xA000..=0xBFFF => self.cartridge.write_ram(addr, value),
            0xC000..=0xDFFF => self.wram[(addr - 0xC000) as usize] = value,
            0xE000..=0xFDFF => self.wram[(addr - 0xE000) as usize] = value,
            0xFE00..=0xFE9F => self.ppu.write_oam(addr, value),
            0xFEA0..=0xFEFF => {}
            0xFF00..=0xFF7F => {
                let old = self.peek(addr);
                match addr {
                    joypad::P1 => {
                        if self.joypad.write(value) {
                            self.interrupt_flag |= interrupts::JOYPAD;
                        }
                    }
                    serial::SB | serial::SC => self.serial.write(addr, value),
                    timer::DIV..=timer::TAC => self.timer.write(addr, value),
                    interrupts::IF => self.interrupt_flag = value & interrupts::MASK,
                    apu::NR10..=apu::WAVE_RAM_END => self.apu.write(addr, value),
                    DMA => {
                        self.dma_page = value;
                        self.dma_delay = DMA_START_DELAY;
                    }
                    ppu::LCDC..=ppu::WX => self.ppu.write_register(addr, value),
                    _ => self.io[(addr - 0xFF00) as usize] = value,
                }
                self.observer.io_write(addr, old, value);
            }
            0xFF80..=0xFFFE => self.hram[(addr - 0xFF80) as usize] = value,
            interrupts::IE => {
                let old = self.interrupt_enable;
                // Unlike IF, all eight bits of IE can be stored.
                self.interrupt_enable = value;
                self.observer.io_write(addr, old, value);
            }
        }
    }
}

impl<O: Observer> CpuBus for Bus<O> {
    fn read(&mut self, addr: u16) -> u8 {
        self.tick();
        if self.cpu_locked_out(addr) {
            return 0xFF;
        }
        self.peek(addr)
    }

    fn write(&mut self, addr: u16, value: u8) {
        self.tick();
        if self.cpu_locked_out(addr) {
            return;
        }
        self.poke(addr, value);
    }

    fn idle(&mut self) {
        self.tick();
    }

    fn pending_interrupts(&self) -> u8 {
        self.interrupt_flag & self.interrupt_enable & interrupts::MASK
    }

    fn acknowledge_interrupt(&mut self, bit: u8) {
        self.interrupt_flag &= !bit;
        self.observer.interrupt(bit);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cartridge::test_util::RomBuilder;

    fn bus_with(rom: Vec<u8>) -> Bus {
        let cartridge = Cartridge::new(&rom).unwrap_or_else(|e| panic!("{e}"));
        Bus::new(cartridge, NullObserver)
    }

    fn bus() -> Bus {
        bus_with(RomBuilder::new().build())
    }

    #[test]
    fn rom_reads_come_from_the_cartridge_and_ignore_writes() {
        let mut bus = bus();
        assert_eq!(bus.peek(0x0042), 0x42);
        assert_eq!(bus.peek(0x7FFE), 0xFE);
        bus.poke(0x0042, 0x00);
        assert_eq!(bus.peek(0x0042), 0x42);
    }

    #[test]
    fn every_ram_region_stores_its_first_and_last_byte() {
        let mut bus = bus();
        let regions = [
            (0x8000, 0x9FFF), // VRAM
            (0xC000, 0xDFFF), // WRAM
            (0xFE00, 0xFE9F), // OAM
            (0xFF80, 0xFFFE), // HRAM
        ];
        for (i, &(first, last)) in regions.iter().enumerate() {
            let value = 0x10 + i as u8;
            bus.poke(first, value);
            bus.poke(last, !value);
            assert_eq!(bus.peek(first), value, "{first:#06x}");
            assert_eq!(bus.peek(last), !value, "{last:#06x}");
        }
    }

    #[test]
    fn regions_do_not_overlap() {
        let mut bus = bus();
        bus.poke(0x9FFF, 0x01);
        bus.poke(0xC000, 0x02);
        bus.poke(0xFE9F, 0x03);
        bus.poke(0xFF80, 0x04);
        bus.poke(0xFFFE, 0x05);
        bus.poke(0xFFFF, 0x06);
        assert_eq!(bus.peek(0x9FFF), 0x01);
        assert_eq!(bus.peek(0xC000), 0x02);
        assert_eq!(bus.peek(0xFE9F), 0x03);
        assert_eq!(bus.peek(0xFF80), 0x04);
        assert_eq!(bus.peek(0xFFFE), 0x05);
        assert_eq!(bus.peek(0xFFFF), 0x06);
    }

    #[test]
    fn echo_ram_mirrors_work_ram_both_ways() {
        let mut bus = bus();
        bus.poke(0xC000, 0xAA);
        bus.poke(0xDDFF, 0xBB);
        assert_eq!(bus.peek(0xE000), 0xAA);
        assert_eq!(bus.peek(0xFDFF), 0xBB);

        bus.poke(0xE123, 0xCC);
        assert_eq!(bus.peek(0xC123), 0xCC);
    }

    #[test]
    fn unusable_area_reads_zero_and_ignores_writes() {
        let mut bus = bus();
        bus.poke(0xFEA0, 0x55);
        bus.poke(0xFEFF, 0x55);
        assert_eq!(bus.peek(0xFEA0), 0x00);
        assert_eq!(bus.peek(0xFEFF), 0x00);
    }

    #[test]
    fn cartridge_ram_is_routed_to_the_mapper() {
        let mut bus = bus();
        bus.poke(0xA000, 0x12);
        assert_eq!(bus.peek(0xA000), 0xFF, "no RAM chip: open bus");

        let mut bus = bus_with(
            RomBuilder::new()
                .cartridge_type(0x08)
                .ram_size_code(2)
                .build(),
        );
        bus.poke(0xA000, 0x12);
        bus.poke(0xBFFF, 0x34);
        assert_eq!(bus.peek(0xA000), 0x12);
        assert_eq!(bus.peek(0xBFFF), 0x34);
    }

    #[test]
    fn io_registers_start_in_the_post_boot_state() {
        let bus = bus();
        assert_eq!(bus.peek(0xFF00), 0xCF); // P1
        assert_eq!(bus.peek(0xFF07), 0xF8); // TAC
        assert_eq!(bus.peek(0xFF0F), 0xE1); // IF
        assert_eq!(bus.peek(0xFF26), 0xF1); // NR52
        assert_eq!(bus.peek(0xFF40), 0x91); // LCDC
        assert_eq!(bus.peek(0xFF47), 0xFC); // BGP
        assert_eq!(bus.peek(0xFFFF), 0x00); // IE
        assert_eq!(bus.peek(0xFF03), 0xFF, "unmapped register");
        assert_eq!(bus.peek(0xFF7F), 0xFF, "unmapped register");
    }

    #[derive(Default)]
    struct Recorder {
        writes: Vec<(u16, u8, u8)>,
        interrupts: Vec<u8>,
        modes: Vec<(ppu::Mode, u8)>,
    }

    impl Observer for Recorder {
        fn io_write(&mut self, addr: u16, old: u8, new: u8) {
            self.writes.push((addr, old, new));
        }

        fn interrupt(&mut self, bit: u8) {
            self.interrupts.push(bit);
        }

        fn ppu_mode(&mut self, mode: ppu::Mode, ly: u8) {
            self.modes.push((mode, ly));
        }
    }

    #[test]
    fn observer_sees_io_writes_only() {
        let rom = RomBuilder::new().build();
        let cartridge = Cartridge::new(&rom).unwrap_or_else(|e| panic!("{e}"));
        let mut bus = Bus::new(cartridge, Recorder::default());

        bus.poke(0xC000, 0x11); // WRAM: not reported
        bus.poke(0xFF80, 0x22); // HRAM: not reported
        bus.poke(0xFF47, 0xE4); // BGP
        bus.poke(0xFFFF, 0x01); // IE

        assert_eq!(
            bus.observer.writes,
            vec![(0xFF47, 0xFC, 0xE4), (0xFFFF, 0x00, 0x01)]
        );

        bus.acknowledge_interrupt(0x01);
        assert_eq!(bus.observer.interrupts, vec![0x01]);

        for _ in 0..114 {
            bus.idle();
        }
        assert_eq!(
            bus.observer.modes,
            vec![
                (ppu::Mode::Drawing, 0),
                (ppu::Mode::HBlank, 0),
                (ppu::Mode::OamScan, 1)
            ]
        );
    }

    #[test]
    fn serial_port_is_mapped_and_starts_idle() {
        let mut bus = bus();
        assert_eq!(bus.peek(0xFF01), 0x00);
        assert_eq!(bus.peek(0xFF02), 0x7E);
        bus.poke(0xFF01, b'x');
        bus.poke(0xFF02, 0x81);
        assert_eq!(bus.take_serial_output(), b"x");
    }

    #[test]
    fn joypad_is_mapped_and_requests_its_interrupt() {
        let mut bus = bus();
        bus.poke(0xFF0F, 0x00);
        bus.poke(0xFF00, 0x10);
        bus.set_button(Button::Start, true);
        assert_eq!(bus.peek(0xFF00), 0xC0 | 0x10 | 0b0111);
        assert_eq!(bus.peek(0xFF0F) & 0x10, 0x10);
        assert!(bus.is_pressed(Button::Start));
    }

    #[test]
    fn sound_registers_and_wave_ram_reach_the_apu() {
        let mut bus = bus();
        bus.poke(0xFF24, 0x35);
        assert_eq!(bus.apu().read(0xFF24), 0x35);
        bus.poke(0xFF30, 0xAB);
        assert_eq!(bus.peek(0xFF30), 0xAB);
        for _ in 0..1000 {
            bus.idle();
        }
        assert!(!bus.apu_mut().take_samples().is_empty());
    }

    #[test]
    fn timer_registers_are_mapped() {
        let mut bus = bus();
        assert_eq!(bus.peek(0xFF04), 0xAB);
        assert_eq!(bus.peek(0xFF07), 0xF8);
        bus.poke(0xFF06, 0x42);
        assert_eq!(bus.peek(0xFF06), 0x42);
        bus.poke(0xFF04, 0x77);
        assert_eq!(bus.peek(0xFF04), 0x00, "any write resets DIV");
    }

    #[test]
    fn timer_overflow_requests_an_interrupt() {
        let mut bus = bus();
        bus.poke(0xFF0F, 0x00);
        bus.poke(0xFF05, 0xFF);
        bus.poke(0xFF07, 0x05);
        for _ in 0..8 {
            bus.idle();
        }
        assert_eq!(bus.peek(0xFF0F) & 0x04, 0x04);
    }

    #[test]
    fn only_interrupts_both_requested_and_enabled_are_pending() {
        let mut bus = bus();
        bus.poke(0xFF0F, 0x05);
        bus.poke(0xFFFF, 0x0C);
        assert_eq!(bus.pending_interrupts(), 0x04);
        bus.acknowledge_interrupt(0x04);
        assert_eq!(bus.pending_interrupts(), 0x00);
        assert_eq!(bus.peek(0xFF0F), 0xE1, "other requests are kept");
    }

    #[test]
    fn if_has_five_bits_and_ie_has_eight() {
        let mut bus = bus();
        bus.poke(0xFF0F, 0x00);
        assert_eq!(bus.peek(0xFF0F), 0xE0);
        bus.poke(0xFFFF, 0xFF);
        assert_eq!(bus.peek(0xFFFF), 0xFF);
        assert_eq!(bus.pending_interrupts(), 0x00);
    }

    #[test]
    fn finished_serial_transfer_requests_an_interrupt() {
        let mut bus = bus();
        bus.poke(0xFF0F, 0x00);
        bus.poke(0xFF02, 0x81);
        for _ in 0..1024 {
            bus.idle();
        }
        assert_eq!(bus.peek(0xFF0F) & 0x08, 0x08);
    }

    #[test]
    fn every_cpu_access_takes_one_cycle() {
        let mut bus = bus();
        CpuBus::read(&mut bus, 0xC000);
        CpuBus::write(&mut bus, 0xC000, 0x01);
        bus.idle();
        assert_eq!(bus.cycles(), 3);
        assert_eq!(bus.peek(0xC000), 0x01);
    }

    #[test]
    fn dma_copies_a_page_into_oam_after_a_short_delay() {
        let mut bus = bus();
        // LCD off, so the PPU does not lock OAM as well.
        bus.poke(0xFF40, 0x00);
        for i in 0..160u16 {
            bus.poke(0xC100 + i, i as u8 ^ 0x5A);
        }
        CpuBus::write(&mut bus, 0xFF46, 0xC1);
        assert_eq!(bus.peek(0xFF46), 0xC1);

        bus.idle();
        assert_eq!(bus.peek(0xFE00), 0x00, "still waiting to start");
        bus.idle();
        assert_eq!(bus.peek(0xFE00), 0x5A, "first byte copied");
        assert_eq!(CpuBus::read(&mut bus, 0xFE00), 0xFF, "OAM is cut off");
        for _ in 0..158 {
            bus.idle();
        }
        assert_eq!(bus.peek(0xFE9F), 159 ^ 0x5A);
        assert_eq!(CpuBus::read(&mut bus, 0xFE9F), 159 ^ 0x5A, "transfer over");
    }

    #[test]
    fn ppu_locks_the_cpu_out_of_video_memory_while_using_it() {
        let mut bus = bus();
        bus.poke(0x8000, 0x11);
        bus.poke(0xFE00, 0x22);

        // Mode 2: OAM is taken, VRAM is still free.
        assert_eq!(CpuBus::read(&mut bus, 0xFE00), 0xFF);
        assert_eq!(CpuBus::read(&mut bus, 0x8000), 0x11);
        // Mode 3: both are taken, and writes are lost.
        for _ in 0..30 {
            bus.idle();
        }
        assert_eq!(CpuBus::read(&mut bus, 0x8000), 0xFF);
        CpuBus::write(&mut bus, 0x8000, 0x99);
        // Mode 0: both are free again.
        for _ in 0..40 {
            bus.idle();
        }
        assert_eq!(CpuBus::read(&mut bus, 0x8000), 0x11);
        assert_eq!(CpuBus::read(&mut bus, 0xFE00), 0x22);
    }

    #[test]
    fn vram_oam_and_ppu_registers_reach_the_ppu() {
        let mut bus = bus();
        bus.poke(0x8000, 0x12);
        bus.poke(0xFE00, 0x34);
        bus.poke(0xFF42, 0x56);
        assert_eq!(bus.ppu().read_vram(0x8000), 0x12);
        assert_eq!(bus.ppu().read_oam(0xFE00), 0x34);
        assert_eq!(bus.ppu().read_register(0xFF42), 0x56);
    }

    #[test]
    fn vblank_requests_an_interrupt_and_reports_the_frame() {
        let mut bus = bus();
        bus.poke(0xFF0F, 0x00);
        for _ in 0..144 * 114 {
            bus.idle();
        }
        assert_eq!(bus.peek(0xFF0F) & 0x01, 0x01);
        assert!(bus.take_frame_ready());
        assert!(!bus.take_frame_ready());
    }

    #[test]
    fn ly_counts_scanlines_and_wraps_after_154() {
        let mut bus = bus();
        for _ in 0..114 {
            bus.idle();
        }
        assert_eq!(bus.peek(0xFF44), 1);
        for _ in 0..114 * 153 {
            bus.idle();
        }
        assert_eq!(bus.peek(0xFF44), 0);

        bus.poke(0xFF44, 0x55);
        assert_eq!(bus.peek(0xFF44), 0, "LY is read-only");
    }

    #[test]
    fn ly_stays_at_zero_while_the_lcd_is_off() {
        let mut bus = bus();
        for _ in 0..114 * 10 {
            bus.idle();
        }
        bus.poke(0xFF40, 0x00);
        for _ in 0..114 * 10 {
            bus.idle();
        }
        assert_eq!(bus.peek(0xFF44), 0);
    }
}
