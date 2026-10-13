//! The memory bus: routes every 16-bit address to the hardware behind it.
//!
//! | Range             | What lives there                         |
//! | ----------------- | ---------------------------------------- |
//! | `0x0000..=0x7FFF` | Cartridge ROM (through the mapper)       |
//! | `0x8000..=0x9FFF` | Video RAM                                |
//! | `0xA000..=0xBFFF` | Cartridge RAM (through the mapper)       |
//! | `0xC000..=0xDFFF` | Work RAM                                 |
//! | `0xE000..=0xFDFF` | Echo of `0xC000..=0xDDFF`                |
//! | `0xFE00..=0xFE9F` | Object attribute memory (sprites)        |
//! | `0xFEA0..=0xFEFF` | Unusable                                 |
//! | `0xFF00..=0xFF7F` | I/O registers                            |
//! | `0xFF80..=0xFFFE` | High RAM                                 |
//! | `0xFFFF`          | Interrupt enable register                |
//!
//! Reference: <https://gbdev.io/pandocs/Memory_Map.html>
//!
//! Most I/O registers are still plain storage holding their post-boot
//! values. Each one gains its real behavior when the component that owns it
//! (timer, PPU, joypad, APU...) is implemented.
//!
//! [`Bus::peek`] and [`Bus::poke`] access memory without the passage of time.
//! The CPU goes through the [`CpuBus`] implementation instead, where every
//! access first advances the hardware by one M-cycle.

use crate::cartridge::Cartridge;
use crate::cpu::CpuBus;
use crate::observer::{NullObserver, Observer};
use crate::serial::{self, Serial};

const IF: u16 = 0xFF0F;
const LCDC: u16 = 0xFF40;
const LY: u16 = 0xFF44;

const INTERRUPT_SERIAL: u8 = 0x08;

/// A scanline lasts 456 clock ticks, and there are 154 of them per frame
/// (144 visible plus 10 of vertical blanking).
const CYCLES_PER_LINE: u16 = 114;
const LINES_PER_FRAME: u8 = 154;

/// I/O register values right after the DMG boot ROM hands control to the
/// cartridge. Registers not listed here are unmapped and read `0xFF`.
///
/// Reference: <https://gbdev.io/pandocs/Power_Up_Sequence.html#hardware-registers>
const POST_BOOT_IO: &[(u16, u8)] = &[
    (0xFF00, 0xCF), // P1
    (0xFF04, 0xAB), // DIV
    (0xFF05, 0x00), // TIMA
    (0xFF06, 0x00), // TMA
    (0xFF07, 0xF8), // TAC
    (0xFF0F, 0xE1), // IF
    (0xFF10, 0x80), // NR10
    (0xFF11, 0xBF), // NR11
    (0xFF12, 0xF3), // NR12
    (0xFF13, 0xFF), // NR13
    (0xFF14, 0xBF), // NR14
    (0xFF16, 0x3F), // NR21
    (0xFF17, 0x00), // NR22
    (0xFF18, 0xFF), // NR23
    (0xFF19, 0xBF), // NR24
    (0xFF1A, 0x7F), // NR30
    (0xFF1B, 0xFF), // NR31
    (0xFF1C, 0x9F), // NR32
    (0xFF1D, 0xFF), // NR33
    (0xFF1E, 0xBF), // NR34
    (0xFF20, 0xFF), // NR41
    (0xFF21, 0x00), // NR42
    (0xFF22, 0x00), // NR43
    (0xFF23, 0xBF), // NR44
    (0xFF24, 0x77), // NR50
    (0xFF25, 0xF3), // NR51
    (0xFF26, 0xF1), // NR52
    (0xFF40, 0x91), // LCDC
    (0xFF41, 0x85), // STAT
    (0xFF42, 0x00), // SCY
    (0xFF43, 0x00), // SCX
    (0xFF44, 0x00), // LY
    (0xFF45, 0x00), // LYC
    (0xFF46, 0xFF), // DMA
    (0xFF47, 0xFC), // BGP
    (0xFF4A, 0x00), // WY
    (0xFF4B, 0x00), // WX
];

const VRAM_SIZE: usize = 0x2000;
const WRAM_SIZE: usize = 0x2000;
const OAM_SIZE: usize = 0xA0;
const IO_SIZE: usize = 0x80;
const HRAM_SIZE: usize = 0x7F;

/// Everything the CPU can address.
pub struct Bus<O: Observer = NullObserver> {
    cartridge: Cartridge,
    vram: [u8; VRAM_SIZE],
    wram: [u8; WRAM_SIZE],
    oam: [u8; OAM_SIZE],
    io: [u8; IO_SIZE],
    hram: [u8; HRAM_SIZE],
    ie: u8,
    serial: Serial,
    /// M-cycles into the current scanline. Stands in for the PPU, which for
    /// now only counts lines so that programs waiting on LY make progress.
    line_cycles: u16,
    /// M-cycles elapsed since power on.
    cycles: u64,
    pub observer: O,
}

impl<O: Observer> Bus<O> {
    /// Creates a bus in the post-boot state with `cartridge` inserted.
    pub fn new(cartridge: Cartridge, observer: O) -> Self {
        let mut io = [0xFF; IO_SIZE];
        for &(addr, value) in POST_BOOT_IO {
            io[(addr - 0xFF00) as usize] = value;
        }
        Self {
            cartridge,
            vram: [0; VRAM_SIZE],
            wram: [0; WRAM_SIZE],
            oam: [0; OAM_SIZE],
            io,
            hram: [0; HRAM_SIZE],
            ie: 0x00,
            serial: Serial::new(),
            line_cycles: 0,
            cycles: 0,
            observer,
        }
    }

    pub fn cartridge(&self) -> &Cartridge {
        &self.cartridge
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

        if self.serial.tick() {
            self.io[(IF - 0xFF00) as usize] |= INTERRUPT_SERIAL;
        }

        let lcd_on = self.io[(LCDC - 0xFF00) as usize] & 0x80 != 0;
        let ly = &mut self.io[(LY - 0xFF00) as usize];
        if lcd_on {
            self.line_cycles += 1;
            if self.line_cycles == CYCLES_PER_LINE {
                self.line_cycles = 0;
                *ly = (*ly + 1) % LINES_PER_FRAME;
            }
        } else {
            self.line_cycles = 0;
            *ly = 0;
        }
    }

    /// Reads the byte the CPU would see at `addr`, without advancing time.
    pub fn peek(&self, addr: u16) -> u8 {
        match addr {
            0x0000..=0x7FFF => self.cartridge.read_rom(addr),
            0x8000..=0x9FFF => self.vram[(addr - 0x8000) as usize],
            0xA000..=0xBFFF => self.cartridge.read_ram(addr),
            0xC000..=0xDFFF => self.wram[(addr - 0xC000) as usize],
            0xE000..=0xFDFF => self.wram[(addr - 0xE000) as usize],
            0xFE00..=0xFE9F => self.oam[(addr - 0xFE00) as usize],
            // On DMG this area reads zero.
            0xFEA0..=0xFEFF => 0x00,
            serial::SB | serial::SC => self.serial.read(addr),
            0xFF00..=0xFF7F => self.io[(addr - 0xFF00) as usize],
            0xFF80..=0xFFFE => self.hram[(addr - 0xFF80) as usize],
            0xFFFF => self.ie,
        }
    }

    /// Writes `value` to `addr` as the CPU would, without advancing time.
    pub fn poke(&mut self, addr: u16, value: u8) {
        match addr {
            0x0000..=0x7FFF => self.cartridge.write_rom(addr, value),
            0x8000..=0x9FFF => self.vram[(addr - 0x8000) as usize] = value,
            0xA000..=0xBFFF => self.cartridge.write_ram(addr, value),
            0xC000..=0xDFFF => self.wram[(addr - 0xC000) as usize] = value,
            0xE000..=0xFDFF => self.wram[(addr - 0xE000) as usize] = value,
            0xFE00..=0xFE9F => self.oam[(addr - 0xFE00) as usize] = value,
            0xFEA0..=0xFEFF => {}
            0xFF00..=0xFF7F => {
                let old = self.peek(addr);
                match addr {
                    serial::SB | serial::SC => self.serial.write(addr, value),
                    // LY is read-only.
                    LY => {}
                    _ => self.io[(addr - 0xFF00) as usize] = value,
                }
                self.observer.io_write(addr, old, value);
            }
            0xFF80..=0xFFFE => self.hram[(addr - 0xFF80) as usize] = value,
            0xFFFF => {
                let old = self.ie;
                self.ie = value;
                self.observer.io_write(addr, old, value);
            }
        }
    }
}

impl<O: Observer> CpuBus for Bus<O> {
    fn read(&mut self, addr: u16) -> u8 {
        self.tick();
        self.peek(addr)
    }

    fn write(&mut self, addr: u16, value: u8) {
        self.tick();
        self.poke(addr, value);
    }

    fn idle(&mut self) {
        self.tick();
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
        assert_eq!(bus.peek(0xFF40), 0x91); // LCDC
        assert_eq!(bus.peek(0xFF47), 0xFC); // BGP
        assert_eq!(bus.peek(0xFFFF), 0x00); // IE
        assert_eq!(bus.peek(0xFF03), 0xFF, "unmapped register");
        assert_eq!(bus.peek(0xFF7F), 0xFF, "unmapped register");
    }

    #[derive(Default)]
    struct Recorder {
        writes: Vec<(u16, u8, u8)>,
    }

    impl Observer for Recorder {
        fn io_write(&mut self, addr: u16, old: u8, new: u8) {
            self.writes.push((addr, old, new));
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
