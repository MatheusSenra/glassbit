//! The PPU (picture processing unit): turns video memory into pixels.
//!
//! The screen is drawn one line at a time, 154 lines per frame, each lasting
//! 456 clock ticks ("dots"), which is 114 M-cycles. On the 144 visible lines
//! the PPU goes through three modes:
//!
//! | Mode | What happens                                   | Dots       |
//! | ---- | ---------------------------------------------- | ---------- |
//! | 2    | OAM scan: pick the sprites on this line        | 80         |
//! | 3    | Drawing: pixels go to the LCD                  | 172 (min.) |
//! | 0    | HBlank: wait for the line to end               | the rest   |
//!
//! Lines 144 to 153 are mode 1 (VBlank), when games update video memory.
//!
//! Mode 3 works like the hardware's pixel pipeline, one dot at a time. A
//! tile fetcher reads a tile number and two bytes of pixel data (two dots
//! each) and pushes the row of 8 pixels into the background FIFO when that
//! is empty; every dot one pixel leaves the FIFO for the LCD, mixed with
//! the sprite FIFO. Fine scrolling throws the first SCX % 8 pixels away,
//! the window restarts the fetcher, and each sprite pauses the output while
//! it is fetched, so all three make mode 3 longer, and a register written
//! in the middle of the line takes effect from the next fetch or pixel on.
//! A pixel leaving the FIFO is mixed one dot later (reading LCDC) and gets
//! its palette color one dot after that (reading BGP/OBP).
//!
//! The M-cycles at which modes change were fitted to Mooneye's `acceptance/ppu`
//! timing tests, since Pan Docs gives them in dots relative to a line start
//! that the CPU cannot observe directly. The dots at which the fetcher and
//! the pixel stages read registers were fitted to the Mealybug Tearoom
//! tests, which change registers in the middle of a line.
//!
//! On a Game Boy Color in color mode the same machinery gets a second bank
//! of video memory for tile attributes (palette, flips, priority) and eight
//! palettes of four RGB colors each for the background and for sprites.
//!
//! Reference: <https://gbdev.io/pandocs/Rendering.html> and
//! <https://gbdev.io/pandocs/Palettes.html>

use crate::interrupts;
use crate::state::{Reader, StateError, Writer};
use crate::{SCREEN_HEIGHT, SCREEN_WIDTH};

pub const LCDC: u16 = 0xFF40;
pub const STAT: u16 = 0xFF41;
pub const SCY: u16 = 0xFF42;
pub const SCX: u16 = 0xFF43;
pub const LY: u16 = 0xFF44;
pub const LYC: u16 = 0xFF45;
pub const BGP: u16 = 0xFF47;
pub const OBP0: u16 = 0xFF48;
pub const OBP1: u16 = 0xFF49;
pub const WY: u16 = 0xFF4A;
pub const WX: u16 = 0xFF4B;
/// CGB: selects the VRAM bank the CPU sees.
pub const VBK: u16 = 0xFF4F;
/// CGB: background palette index (bits 0-5) and auto-increment (bit 7).
pub const BCPS: u16 = 0xFF68;
/// CGB: background palette data at that index.
pub const BCPD: u16 = 0xFF69;
/// CGB: sprite palette index.
pub const OCPS: u16 = 0xFF6A;
/// CGB: sprite palette data.
pub const OCPD: u16 = 0xFF6B;
/// CGB: sprite priority mode (bit 0 clear: by OAM position).
pub const OPRI: u16 = 0xFF6C;

// LCDC bits.
const LCDC_BG_ENABLE: u8 = 0x01;
const LCDC_OBJ_ENABLE: u8 = 0x02;
const LCDC_OBJ_TALL: u8 = 0x04;
const LCDC_BG_MAP_HIGH: u8 = 0x08;
const LCDC_TILE_DATA_LOW: u8 = 0x10;
const LCDC_WINDOW_ENABLE: u8 = 0x20;
const LCDC_WINDOW_MAP_HIGH: u8 = 0x40;
const LCDC_LCD_ON: u8 = 0x80;

// STAT interrupt source bits.
const STAT_HBLANK: u8 = 0x08;
const STAT_VBLANK: u8 = 0x10;
const STAT_OAM: u8 = 0x20;
const STAT_LYC: u8 = 0x40;

// OAM attribute bits.
const OBJ_PALETTE_1: u8 = 0x10;
/// CGB: the sprite's tiles come from VRAM bank 1.
const OBJ_BANK_1: u8 = 0x08;
/// CGB: the sprite's palette (0-7).
const OBJ_CGB_PALETTE: u8 = 0x07;

// CGB background map attribute bits (stored in VRAM bank 1).
const ATTR_PALETTE: u8 = 0x07;
const ATTR_BANK_1: u8 = 0x08;
const ATTR_FLIP_X: u8 = 0x20;
const ATTR_FLIP_Y: u8 = 0x40;
const ATTR_PRIORITY: u8 = 0x80;

/// RGB555 white, the color of a blank screen.
const WHITE: u16 = 0x7FFF;

/// A 24-bit `0xRRGGBB` color as RGB555 (red in the lowest bits).
pub const fn rgb555(rgb: u32) -> u16 {
    let red = (rgb >> 19 & 0x1F) as u16;
    let green = (rgb >> 11 & 0x1F) as u16;
    let blue = (rgb >> 3 & 0x1F) as u16;
    red | green << 5 | blue << 10
}

/// The colors a Game Boy Color gives a DMG game in compatibility mode when
/// it does not recognize the title: one palette for the background and two
/// for sprites (OBP0 and OBP1), each from the lightest shade to the
/// darkest. The real boot ROM picks other palettes for many games by
/// hashing the title; Glassbit does not run it, so this is the default.
pub const DEFAULT_COMPAT_PALETTES: [[u16; 4]; 3] = [
    [
        rgb555(0xFFFFFF),
        rgb555(0x7BFF31),
        rgb555(0x0063C6),
        rgb555(0x000000),
    ],
    [
        rgb555(0xFFFFFF),
        rgb555(0xFF8484),
        rgb555(0x943939),
        rgb555(0x000000),
    ],
    [
        rgb555(0xFFFFFF),
        rgb555(0xFF8484),
        rgb555(0x943939),
        rgb555(0x000000),
    ],
];
const OBJ_FLIP_X: u8 = 0x20;
const OBJ_FLIP_Y: u8 = 0x40;
const OBJ_BEHIND_BG: u8 = 0x80;

/// M-cycles per scanline (456 dots).
const CYCLES_PER_LINE: u8 = 114;
/// M-cycle of the line at which mode 3 starts. LY goes up one M-cycle before
/// the line proper begins, so this is the 80 dots of mode 2 plus that cycle.
const DRAWING_START: u8 = 21;
/// Dots of mode 3 that already go by in the M-cycle in which it starts.
/// Fitted to Mooneye's `intr_2_mode0_timing_sprites`, which measures how
/// sprites lengthen mode 3 down to the dot.
const FIRST_CYCLE_DOTS: u8 = 3;
/// The dot of mode 3 at which the first pixel can come out: after the
/// startup and the first tile fetch.
const FIRST_PIXEL_DOT: u8 = STARTUP_DOTS + FETCH_DOTS;
/// Dots of mode 3 spent on a first tile fetch that is thrown away.
const STARTUP_DOTS: u8 = 6;
/// Dots the fetcher takes to read a tile row: tile number, low byte and
/// high byte, two dots each.
const FETCH_DOTS: u8 = 6;
/// Dots a sprite fetch pauses the pixel output, once the background
/// fetcher has finished its own fetch.
const SPRITE_FETCH_DOTS: u8 = 6;
/// On line 0 the OAM scan is only reported one M-cycle after LY wraps.
const FIRST_LINE_OAM_START: u8 = 1;
const LINES_PER_FRAME: u8 = 154;
const SPRITES_PER_LINE: usize = 10;

/// Two banks of 8 KiB; DMG mode only uses the first.
const VRAM_SIZE: usize = 0x4000;
const VRAM_BANK_SIZE: usize = 0x2000;
const PALETTE_RAM_SIZE: usize = 64;
const OAM_SIZE: usize = 0xA0;

/// What the PPU is doing; the numbers are the ones STAT reports.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    HBlank = 0,
    VBlank = 1,
    OamScan = 2,
    Drawing = 3,
}

/// One row of 8 pixels of a background or window tile, as the PPU fetches
/// it: two bytes of pixel data and the CGB attributes.
#[derive(Debug, Clone, Copy, Default)]
struct TileRow {
    low: u8,
    high: u8,
    attributes: u8,
}

impl TileRow {
    /// The color number of pixel `column` (0-7) of the row.
    fn pixel(&self, column: u8) -> u8 {
        let bit = if self.attributes & ATTR_FLIP_X != 0 {
            column
        } else {
            7 - column
        };
        color_number(self.low, self.high, bit)
    }
}

/// Where a mixed pixel takes its color from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Source {
    /// Background palette (BGP, or CGB background palette 0-7).
    Background(u8),
    /// Sprite palette (OBP0/OBP1, or CGB sprite palette 0-7).
    Sprite(u8),
    /// DMG with the background off: always white.
    Blank,
}

impl Source {
    fn to_byte(self) -> u8 {
        match self {
            Source::Background(palette) => palette,
            Source::Sprite(palette) => 0x08 | palette,
            Source::Blank => 0x10,
        }
    }

    fn from_byte(byte: u8) -> Self {
        match byte {
            0x10.. => Source::Blank,
            0x08.. => Source::Sprite(byte & 0x07),
            _ => Source::Background(byte),
        }
    }
}

/// A pixel that left the FIFOs and is on its way to the LCD. It is mixed
/// with LCDC as it is one dot later and colored with the palettes as they
/// are one dot after that.
#[derive(Debug, Clone, Copy)]
struct PoppedPixel {
    x: u8,
    bg_color: u8,
    bg_attributes: u8,
    obj: ObjPixel,
}

#[derive(Debug, Clone, Copy)]
struct MixedPixel {
    x: u8,
    color: u8,
    source: Source,
}

/// One slot of the sprite FIFO.
#[derive(Debug, Clone, Copy, Default)]
struct ObjPixel {
    /// Color number; 0 is transparent, which also means an empty slot.
    color: u8,
    /// DMG: 0 or 1 for OBP0/OBP1. CGB: palette 0-7.
    palette: u8,
    behind_bg: bool,
    /// The OAM slot of the sprite it came from.
    slot: u8,
}

pub struct Ppu {
    // The two large buffers live on the heap so that moving a `Ppu` (and
    // everything that contains one) stays cheap.
    vram: Box<[u8]>,
    oam: [u8; OAM_SIZE],
    lcdc: u8,
    /// The interrupt source bits of STAT (bits 3-6).
    stat_sources: u8,
    scy: u8,
    scx: u8,
    ly: u8,
    lyc: u8,
    bgp: u8,
    obp0: u8,
    obp1: u8,
    wy: u8,
    wx: u8,
    mode: Mode,
    /// M-cycles into the current line.
    line_cycle: u8,
    /// The OR of all enabled STAT sources. The STAT interrupt fires only
    /// when this goes from low to high, so one source can mask another.
    stat_line: bool,
    /// Set for the one M-cycle in which VBlank begins. The OAM source of
    /// STAT also fires then, although the PPU is not scanning OAM.
    vblank_started: bool,
    /// LY has matched WY at some point in this frame.
    window_triggered: bool,
    /// The window keeps its own line counter, which only advances on lines
    /// where the window is actually drawn.
    window_line: u8,
    /// Interrupt requests raised since the bus last collected them.
    interrupts: u8,
    frame_ready: bool,
    /// One shade (0 = lightest, 3 = darkest) per pixel. DMG mode only.
    framebuffer: Box<[u8]>,
    /// Game Boy Color mode: attributes, banks and color palettes.
    cgb: bool,
    /// A Game Boy Color running a DMG game: drawn like a DMG, but every
    /// shade goes through the color palettes the boot ROM set up.
    compat: bool,
    /// One RGB555 color per pixel. CGB mode only.
    color_framebuffer: Box<[u16]>,
    vram_bank: u8,
    bg_palettes: [u8; PALETTE_RAM_SIZE],
    obj_palettes: [u8; PALETTE_RAM_SIZE],
    bcps: u8,
    ocps: u8,
    opri: u8,
    /// Set when a visible line enters HBlank; HDMA copies a block then.
    hblank_started: bool,
    /// Pixels of the current line already sent to the LCD.
    line_x: u8,
    /// Dots of mode 3 left before the fetcher starts its first real fetch.
    startup_dots: u8,
    /// The OAM slots of the sprites on this line, ordered by X.
    line_sprites: [u8; SPRITES_PER_LINE],
    line_sprite_count: u8,
    /// How many of `line_sprites` have already been fetched (or skipped).
    next_sprite: u8,
    /// The fetcher is fetching the window instead of the background.
    window_active: bool,
    /// The window started somewhere on this line.
    window_drawn: bool,
    /// Dots of mode 3 so far on this line.
    line_dots: u16,
    /// The WX value (below 7) that matched before the first pixel: the
    /// window then starts at the left edge with its first columns cut off.
    edge_wx: Option<u8>,
    /// Pixels still to be thrown away before the first one is shown: SCX %
    /// 8, or the part of the window left of the screen when WX is below 7.
    discard: u8,
    /// Dots the pixel output stays paused for a sprite fetch.
    stall: u8,
    /// The background tile the last sprite fetch happened on.
    sprite_tile: Option<u16>,
    /// The background FIFO: one tile row and how many of its pixels are
    /// left. The fetcher only pushes into an empty FIFO, so it never holds
    /// more than one row.
    bg_fifo: TileRow,
    bg_fifo_len: u8,
    /// The sprite FIFO: slot 0 is mixed with the next pixel to be shown.
    obj_fifo: [ObjPixel; 8],
    /// Dots the fetcher has spent on its current fetch; at `FETCH_DOTS`
    /// the row is ready and waits for the FIFO to empty.
    fetch_dots: u8,
    /// The tile column the fetcher is on, counted from the line start (or
    /// from where the window started).
    fetch_x: u8,
    fetch_tile: u8,
    fetch_row: TileRow,
    /// The pixel popped one dot ago, waiting to be mixed.
    popped: Option<PoppedPixel>,
    /// The pixel mixed one dot ago, waiting for its palette.
    mixed: Option<MixedPixel>,
    /// DMG: the BGP value before a write in the M-cycle that just ended.
    /// For one dot the PPU sees both values ORed together.
    old_bgp: u8,
}

impl Ppu {
    /// A PPU in the post-boot state: LCD on, at the top of a frame.
    pub fn new() -> Self {
        Self {
            vram: vec![0; VRAM_SIZE].into_boxed_slice(),
            oam: [0; OAM_SIZE],
            lcdc: 0x91,
            stat_sources: 0x00,
            scy: 0,
            scx: 0,
            ly: 0,
            lyc: 0,
            bgp: 0xFC,
            obp0: 0xFF,
            obp1: 0xFF,
            wy: 0,
            wx: 0,
            mode: Mode::OamScan,
            line_cycle: 0,
            stat_line: false,
            vblank_started: false,
            window_triggered: false,
            window_line: 0,
            interrupts: 0,
            frame_ready: false,
            framebuffer: vec![0; SCREEN_WIDTH * SCREEN_HEIGHT].into_boxed_slice(),
            cgb: false,
            compat: false,
            color_framebuffer: vec![WHITE; SCREEN_WIDTH * SCREEN_HEIGHT].into_boxed_slice(),
            vram_bank: 0,
            bg_palettes: [0xFF; PALETTE_RAM_SIZE],
            obj_palettes: [0xFF; PALETTE_RAM_SIZE],
            bcps: 0,
            ocps: 0,
            opri: 0,
            hblank_started: false,
            line_x: 0,
            startup_dots: 0,
            line_sprites: [0; SPRITES_PER_LINE],
            line_sprite_count: 0,
            next_sprite: 0,
            window_active: false,
            window_drawn: false,
            line_dots: 0,
            edge_wx: None,
            discard: 0,
            stall: 0,
            sprite_tile: None,
            bg_fifo: TileRow::default(),
            bg_fifo_len: 0,
            obj_fifo: [ObjPixel::default(); 8],
            fetch_dots: 0,
            fetch_x: 0,
            fetch_tile: 0,
            fetch_row: TileRow::default(),
            popped: None,
            mixed: None,
            old_bgp: 0,
        }
    }

    /// A PPU in Game Boy Color mode, in the post-boot state.
    pub fn new_cgb() -> Self {
        Self {
            cgb: true,
            ..Self::new()
        }
    }

    /// A PPU of a Game Boy Color in compatibility mode, in the state its
    /// boot ROM leaves: the default colors in background palette 0 and
    /// sprite palettes 0 and 1, and the palette index registers just past
    /// them.
    pub fn new_compat() -> Self {
        let mut ppu = Self {
            compat: true,
            ..Self::new()
        };
        ppu.set_compat_palettes(&DEFAULT_COMPAT_PALETTES);
        ppu.bcps = 0x88;
        ppu.ocps = 0x90;
        ppu
    }

    /// In compatibility mode, replaces the background palette and the two
    /// sprite palettes (RGB555 colors, lightest shade first).
    pub fn set_compat_palettes(&mut self, palettes: &[[u16; 4]; 3]) {
        let [background, obp0, obp1] = palettes;
        // Each palette is 8 bytes: four little-endian colors.
        let bytes = |colors: &[u16; 4]| colors.map(|color| (color & 0x7FFF).to_le_bytes());
        self.bg_palettes[..8].copy_from_slice(bytes(background).as_flattened());
        self.obj_palettes[..8].copy_from_slice(bytes(obp0).as_flattened());
        self.obj_palettes[8..16].copy_from_slice(bytes(obp1).as_flattened());
    }

    /// Whether this is a Game Boy Color running a DMG game.
    pub fn is_compat(&self) -> bool {
        self.compat
    }

    /// Whether this PPU works in Game Boy Color mode.
    pub fn is_cgb(&self) -> bool {
        self.cgb
    }

    /// The screen of a Game Boy Color (in either mode): one RGB555 color
    /// per pixel, row by row.
    pub fn color_framebuffer(&self) -> &[u16] {
        &self.color_framebuffer
    }

    /// The Color palette RAM: background palettes, then sprite palettes,
    /// 64 bytes each (8 palettes of 4 little-endian RGB555 colors).
    pub fn palette_ram(&self) -> (&[u8], &[u8]) {
        (&self.bg_palettes, &self.obj_palettes)
    }

    /// Returns `true` once each time a visible line enters HBlank.
    pub fn take_hblank_started(&mut self) -> bool {
        std::mem::take(&mut self.hblank_started)
    }

    pub fn mode(&self) -> Mode {
        self.mode
    }

    pub fn ly(&self) -> u8 {
        self.ly
    }

    pub fn framebuffer(&self) -> &[u8] {
        &self.framebuffer
    }

    /// Returns and clears the interrupt requests raised since the last call.
    pub fn take_interrupts(&mut self) -> u8 {
        std::mem::take(&mut self.interrupts)
    }

    /// Returns `true` once after each frame is fully drawn (VBlank began).
    pub fn take_frame_ready(&mut self) -> bool {
        std::mem::take(&mut self.frame_ready)
    }

    /// The PPU reads tile data only while drawing.
    pub fn vram_locked(&self) -> bool {
        self.lcd_on() && self.mode == Mode::Drawing
    }

    /// The PPU reads OAM while scanning for sprites and while drawing them.
    pub fn oam_locked(&self) -> bool {
        self.lcd_on() && matches!(self.mode, Mode::OamScan | Mode::Drawing)
    }

    fn lcd_on(&self) -> bool {
        self.lcdc & LCDC_LCD_ON != 0
    }

    /// Advances one M-cycle (4 dots).
    pub fn tick(&mut self) {
        if !self.lcd_on() {
            return;
        }

        self.vblank_started = false;
        self.line_cycle += 1;
        if self.line_cycle == CYCLES_PER_LINE {
            self.line_cycle = 0;
            self.ly += 1;
            if self.ly == LINES_PER_FRAME {
                self.ly = 0;
                self.window_triggered = false;
                self.window_line = 0;
            }
            if self.ly == SCREEN_HEIGHT as u8 {
                self.mode = Mode::VBlank;
                self.vblank_started = true;
                self.interrupts |= interrupts::VBLANK;
                self.frame_ready = true;
            } else if (1..SCREEN_HEIGHT as u8).contains(&self.ly) {
                self.mode = Mode::OamScan;
            }
        } else if self.ly < SCREEN_HEIGHT as u8 {
            if self.ly == 0 && self.line_cycle == FIRST_LINE_OAM_START {
                // Not after the LCD was just switched on: that first line
                // has no OAM scan at all.
                if self.mode == Mode::VBlank {
                    self.mode = Mode::OamScan;
                }
            } else if self.line_cycle == DRAWING_START {
                self.mode = Mode::Drawing;
                self.begin_line();
                self.draw_dots(FIRST_CYCLE_DOTS);
            } else if self.mode == Mode::Drawing {
                self.draw_dots(4);
            }
        }
        self.update_stat_line();
    }

    /// Runs `dots` dots of mode 3; HBlank begins with the last pixel.
    fn draw_dots(&mut self, dots: u8) {
        for _ in 0..dots {
            self.dot();
            if self.line_x as usize == SCREEN_WIDTH {
                self.enter_hblank();
                return;
            }
        }
        // A line can never draw longer than the line itself, but a corrupt
        // state could.
        if self.line_cycle == CYCLES_PER_LINE - 1 {
            self.enter_hblank();
        }
    }

    fn enter_hblank(&mut self) {
        // The last pixels still on their way reach the LCD.
        self.advance_pixels();
        self.advance_pixels();
        self.end_line();
        self.mode = Mode::HBlank;
        self.hblank_started = true;
    }

    fn update_stat_line(&mut self) {
        let line = self.stat_sources & STAT_LYC != 0 && self.ly == self.lyc
            || self.stat_sources & STAT_OAM != 0
                && (self.mode == Mode::OamScan || self.vblank_started)
            || self.stat_sources & STAT_VBLANK != 0 && self.mode == Mode::VBlank
            || self.stat_sources & STAT_HBLANK != 0 && self.mode == Mode::HBlank;
        if line && !self.stat_line {
            self.interrupts |= interrupts::STAT;
        }
        self.stat_line = line;
    }

    /// Where `addr` (0x8000-0x9FFF) lands in VRAM, given the selected bank.
    fn vram_index(&self, addr: u16) -> usize {
        self.vram_bank as usize * VRAM_BANK_SIZE + (addr - 0x8000) as usize
    }

    pub fn read_vram(&self, addr: u16) -> u8 {
        self.vram[self.vram_index(addr)]
    }

    pub fn write_vram(&mut self, addr: u16, value: u8) {
        let index = self.vram_index(addr);
        self.vram[index] = value;
    }

    pub fn read_oam(&self, addr: u16) -> u8 {
        self.oam[(addr - 0xFE00) as usize]
    }

    pub fn write_oam(&mut self, addr: u16, value: u8) {
        self.oam[(addr - 0xFE00) as usize] = value;
    }

    pub fn read_register(&self, addr: u16) -> u8 {
        match addr {
            LCDC => self.lcdc,
            STAT => {
                let lyc_match = if self.ly == self.lyc { 0x04 } else { 0x00 };
                // Bit 7 does not exist and reads as 1.
                0x80 | self.stat_sources | lyc_match | self.mode as u8
            }
            SCY => self.scy,
            SCX => self.scx,
            LY => self.ly,
            LYC => self.lyc,
            BGP => self.bgp,
            OBP0 => self.obp0,
            OBP1 => self.obp1,
            WY => self.wy,
            WX => self.wx,
            // In compatibility mode the boot ROM locked the Color
            // registers; a few still read back what it left.
            VBK if self.compat => 0xFE,
            BCPS if self.compat => self.bcps | 0x40,
            OCPS if self.compat => self.ocps | 0x40,
            // The Color registers do not exist in DMG mode.
            _ if !self.cgb => 0xFF,
            VBK => 0xFE | self.vram_bank,
            BCPS => self.bcps | 0x40,
            BCPD => self.bg_palettes[(self.bcps & 0x3F) as usize],
            OCPS => self.ocps | 0x40,
            OCPD => self.obj_palettes[(self.ocps & 0x3F) as usize],
            OPRI => 0xFE | self.opri,
            _ => 0xFF,
        }
    }

    /// Writes palette RAM at the index register's position, advancing it
    /// when its bit 7 asks for that.
    fn write_palette(ram: &mut [u8; PALETTE_RAM_SIZE], index: &mut u8, value: u8) {
        ram[(*index & 0x3F) as usize] = value;
        if *index & 0x80 != 0 {
            *index = 0x80 | (*index + 1) & 0x3F;
        }
    }

    pub fn write_register(&mut self, addr: u16, value: u8) {
        match addr {
            LCDC => {
                let was_on = self.lcd_on();
                self.lcdc = value;
                if was_on && !self.lcd_on() {
                    // Switching the LCD off resets the PPU to the top of the
                    // frame and blanks the screen.
                    self.ly = 0;
                    self.line_cycle = 0;
                    self.mode = Mode::HBlank;
                    self.window_triggered = false;
                    self.window_line = 0;
                    self.framebuffer.fill(0);
                    self.color_framebuffer.fill(WHITE);
                } else if !was_on && self.lcd_on() {
                    // The first line after switching on has no OAM scan and
                    // reports mode 0 until drawing starts.
                    self.mode = Mode::HBlank;
                }
                self.update_stat_line();
            }
            STAT => {
                self.stat_sources = value & 0x78;
                self.update_stat_line();
            }
            SCY => self.scy = value,
            SCX => self.scx = value,
            // LY is read-only.
            LY => {}
            LYC => {
                self.lyc = value;
                if self.lcd_on() {
                    self.update_stat_line();
                }
            }
            BGP => {
                if !self.cgb && self.mode == Mode::Drawing {
                    self.old_bgp = self.bgp;
                }
                self.bgp = value;
            }
            OBP0 => self.obp0 = value,
            OBP1 => self.obp1 = value,
            WY => self.wy = value,
            WX => self.wx = value,
            _ if !self.cgb => {}
            VBK => self.vram_bank = value & 0x01,
            BCPS => self.bcps = value & 0xBF,
            BCPD => Self::write_palette(&mut self.bg_palettes, &mut self.bcps, value),
            OCPS => self.ocps = value & 0xBF,
            OCPD => Self::write_palette(&mut self.obj_palettes, &mut self.ocps, value),
            OPRI => self.opri = value & 0x01,
            _ => {}
        }
    }

    /// Starts drawing line `ly`: decides whether the window can appear,
    /// picks the sprites on the line (the OAM scan) and resets the FIFOs.
    fn begin_line(&mut self) {
        if self.ly == self.wy {
            self.window_triggered = true;
        }
        self.line_x = 0;
        self.startup_dots = STARTUP_DOTS;
        self.window_active = false;
        self.window_drawn = false;
        self.line_dots = 0;
        self.edge_wx = None;
        self.discard = self.scx % 8;
        self.stall = 0;
        self.sprite_tile = None;
        self.bg_fifo_len = 0;
        self.obj_fifo = [ObjPixel::default(); 8];
        self.popped = None;
        self.mixed = None;
        self.fetch_dots = 0;
        self.fetch_x = 0;
        self.next_sprite = 0;

        let height = self.sprite_height();
        let line = self.ly + 16;
        let mut count = 0;
        for slot in 0..40u8 {
            let y = self.oam[slot as usize * 4];
            if (y..y.saturating_add(height)).contains(&line) {
                self.line_sprites[count] = slot;
                count += 1;
                if count == SPRITES_PER_LINE {
                    break;
                }
            }
        }
        // Sprites are fetched as the line reaches them, so from left to
        // right; at the same X, in OAM order (the sort is stable).
        let oam = &self.oam;
        self.line_sprites[..count].sort_by_key(|&slot| oam[slot as usize * 4 + 1]);
        self.line_sprite_count = count as u8;
    }

    /// Finishes the line: the window's line counter only advances when the
    /// window was actually drawn on it.
    fn end_line(&mut self) {
        if self.window_drawn {
            self.window_line += 1;
        }
    }

    fn sprite_height(&self) -> u8 {
        if self.lcdc & LCDC_OBJ_TALL != 0 {
            16
        } else {
            8
        }
    }

    /// One dot of mode 3: the fetcher works, then a pixel may come out.
    fn dot(&mut self) {
        // While the first tile is fetched the PPU is still left of the
        // screen, at x -7 to -1, and compares WX with those positions too.
        let before_first_pixel = self.line_dots as i16 - FIRST_PIXEL_DOT as i16;
        if (-7..0).contains(&before_first_pixel) && self.edge_wx.is_none() {
            let wx = (before_first_pixel + 7) as u8;
            if self.wx == wx && self.lcdc & LCDC_WINDOW_ENABLE != 0 && self.window_triggered {
                self.edge_wx = Some(wx);
            }
        }
        self.line_dots += 1;
        self.advance_pixels();
        self.fetcher_dot();
        if self.stall > 0 {
            self.stall -= 1;
        } else {
            self.output_dot();
        }
    }

    /// One dot of the background/window tile fetcher.
    fn fetcher_dot(&mut self) {
        if self.startup_dots > 0 {
            self.startup_dots -= 1;
            return;
        }
        if self.fetch_dots == FETCH_DOTS {
            // The row waits until the FIFO is empty; the next fetch begins
            // in the same dot it is pushed.
            if self.bg_fifo_len != 0 {
                return;
            }
            self.bg_fifo = self.fetch_row;
            self.bg_fifo_len = 8;
            self.fetch_x = self.fetch_x.wrapping_add(1);
            self.fetch_dots = 0;
        }
        self.fetch_dots += 1;
        // Each read takes two dots; the dots at which the registers are
        // sampled were fitted to Mealybug's mid-line SCX and SCY tests.
        match self.fetch_dots {
            3 => self.fetch_tile_number(),
            5 => self.fetch_row.low = self.vram[self.tile_data_address()],
            6 => self.fetch_row.high = self.vram[self.tile_data_address() + 1],
            _ => {}
        }
    }

    /// Reads the tile number (and, on the Color, its attributes) from the
    /// background or window tile map, with the registers as they are now.
    fn fetch_tile_number(&mut self) {
        // Switching the window off in the middle of a line sends the
        // fetcher back to the background.
        if self.window_active && self.lcdc & LCDC_WINDOW_ENABLE == 0 {
            self.window_active = false;
        }
        let (map, tile_x) = if self.window_active {
            (self.tile_map(LCDC_WINDOW_MAP_HIGH), self.fetch_x)
        } else {
            // The background is a 256x256 picture that wraps around.
            let tile_x = (self.scx >> 3).wrapping_add(self.fetch_x);
            (self.tile_map(LCDC_BG_MAP_HIGH), tile_x)
        };
        let offset = map + (self.fetch_y() as usize / 8) * 32 + (tile_x % 32) as usize;
        self.fetch_tile = self.vram[offset];
        // On the Color, bank 1 holds each map entry's attributes at the
        // same position.
        self.fetch_row.attributes = if self.cgb {
            self.vram[VRAM_BANK_SIZE + offset]
        } else {
            0
        };
    }

    /// The line (0-255) of the tile map the fetcher reads from.
    fn fetch_y(&self) -> u8 {
        if self.window_active {
            self.window_line
        } else {
            self.ly.wrapping_add(self.scy)
        }
    }

    /// Where the low byte of the fetched tile's current row is in VRAM.
    fn tile_data_address(&self) -> usize {
        let attributes = self.fetch_row.attributes;
        // Tile numbers index 0x8000 directly, or are signed around 0x9000.
        let mut base = if self.lcdc & LCDC_TILE_DATA_LOW != 0 {
            self.fetch_tile as usize * 16
        } else {
            (0x1000 + (self.fetch_tile as i8 as i32) * 16) as usize
        };
        if attributes & ATTR_BANK_1 != 0 {
            base += VRAM_BANK_SIZE;
        }
        let mut row = self.fetch_y() % 8;
        if attributes & ATTR_FLIP_Y != 0 {
            row = 7 - row;
        }
        base + row as usize * 2
    }

    /// One dot of pixel output: starts the window or a sprite fetch when
    /// the line reaches them, or sends one pixel to the LCD.
    fn output_dot(&mut self) {
        if self.bg_fifo_len == 0 {
            return;
        }
        let x = self.line_x;

        // WX is offset by 7: the window starts where x + 7 equals WX. Below
        // 7 it starts at the left edge with its first columns cut off. The
        // FIFO is emptied and the fetcher starts over on the window; this
        // dot counts as the first of that fetch.
        if !self.window_drawn && self.window_starts_at(x) {
            self.window_active = true;
            self.window_drawn = true;
            self.bg_fifo_len = 0;
            self.fetch_x = 0;
            self.fetch_dots = 1;
            if let (0, Some(wx)) = (x, self.edge_wx) {
                self.discard = 7 - wx;
            }
            return;
        }

        if self.discard == 0 {
            while let Some(slot) = self.due_sprite(x) {
                self.next_sprite += 1;
                // With sprites off they are skipped, not fetched later.
                if self.lcdc & LCDC_OBJ_ENABLE == 0 {
                    continue;
                }
                self.fetch_sprite(slot, x);
                // This dot is the first one lost.
                self.stall = self.sprite_penalty(slot) - 1;
                return;
            }
        }

        let bg_color = self.bg_fifo.pixel(8 - self.bg_fifo_len);
        self.bg_fifo_len -= 1;
        if self.discard > 0 {
            self.discard -= 1;
            return;
        }
        let obj = self.obj_fifo[0];
        self.obj_fifo.copy_within(1.., 0);
        self.obj_fifo[7] = ObjPixel::default();
        self.popped = Some(PoppedPixel {
            x,
            bg_color,
            bg_attributes: self.bg_fifo.attributes,
            obj,
        });
        self.line_x += 1;
    }

    /// Dots the pixel output pauses for sprite `slot`. The first sprite
    /// found on a background (or window) tile also waits for the fetcher
    /// to finish that tile, which takes longer the further left on the tile
    /// the sprite starts.
    fn sprite_penalty(&mut self, slot: u8) -> u8 {
        let sprite_x = self.oam[slot as usize * 4 + 1] as u16;
        // Where the sprite's left edge falls on the background or window
        // tiles: a tile number for the line and a column within the tile.
        let position = if self.window_active {
            0x100 + sprite_x + 255 - self.wx as u16
        } else {
            sprite_x + (self.scx % 8) as u16
        };
        let tile = position / 8;
        let mut penalty = SPRITE_FETCH_DOTS;
        if self.sprite_tile != Some(tile) {
            self.sprite_tile = Some(tile);
            penalty += 5u8.saturating_sub((position % 8) as u8);
        }
        penalty
    }

    fn window_starts_at(&self, x: u8) -> bool {
        self.lcdc & LCDC_WINDOW_ENABLE != 0
            && self.window_triggered
            && (x as u16 + 7 == self.wx as u16 || x == 0 && self.edge_wx.is_some())
    }

    /// The next sprite of the line, if the output has reached its X (which
    /// is offset by 8, so sprites partly off the left edge are due at 0).
    fn due_sprite(&self, x: u8) -> Option<u8> {
        if self.next_sprite >= self.line_sprite_count {
            return None;
        }
        let slot = self.line_sprites[self.next_sprite as usize];
        let sprite_x = self.oam[slot as usize * 4 + 1];
        (sprite_x as u16 <= x as u16 + 8).then_some(slot)
    }

    /// Fetches a row of sprite `slot` and mixes it into the sprite FIFO,
    /// whose slot 0 is screen pixel `x`.
    fn fetch_sprite(&mut self, slot: u8, x: u8) {
        let base = slot as usize * 4;
        let (y, sprite_x, tile, attributes) = (
            self.oam[base],
            self.oam[base + 1],
            self.oam[base + 2],
            self.oam[base + 3],
        );
        let height = self.sprite_height();
        let mut row = (self.ly + 16).wrapping_sub(y) & (height - 1);
        if attributes & OBJ_FLIP_Y != 0 {
            row = height - 1 - row;
        }
        // A tall sprite uses two consecutive tiles, the first one even.
        let tile = if height == 16 { tile & 0xFE } else { tile };
        let bank = if self.cgb && attributes & OBJ_BANK_1 != 0 {
            VRAM_BANK_SIZE
        } else {
            0
        };
        let data = bank + tile as usize * 16 + row as usize * 2;
        let (low, high) = (self.vram[data], self.vram[data + 1]);
        let palette = if self.cgb {
            attributes & OBJ_CGB_PALETTE
        } else {
            (attributes & OBJ_PALETTE_1 != 0) as u8
        };
        // On DMG, and on the Color when OPRI asks for it, a pixel already
        // in the FIFO wins: it came from a sprite further left or earlier
        // in OAM. Otherwise the sprite earlier in OAM wins.
        let by_oam = self.cgb && self.opri & 0x01 == 0;

        // Columns left of the screen edge are dropped.
        let skip = (x as usize + 8 - sprite_x as usize).min(8);
        for column in skip..8 {
            let bit = if attributes & OBJ_FLIP_X != 0 {
                column as u8
            } else {
                7 - column as u8
            };
            let color = color_number(low, high, bit);
            let target = &mut self.obj_fifo[column - skip];
            if color != 0 && (target.color == 0 || by_oam && slot < target.slot) {
                *target = ObjPixel {
                    color,
                    palette,
                    behind_bg: attributes & OBJ_BEHIND_BG != 0,
                    slot,
                };
            }
        }
    }

    /// Moves the pixels after the FIFOs one stage on: the mixed one gets
    /// its color and reaches the screen, the popped one is mixed.
    fn advance_pixels(&mut self) {
        if let Some(pixel) = self.mixed.take() {
            self.write_pixel(pixel);
        }
        self.old_bgp = 0;
        if let Some(pixel) = self.popped.take() {
            self.mixed = Some(self.mix(pixel));
        }
    }

    /// Decides whether the background or the sprite shows, with LCDC as it
    /// is right now.
    fn mix(&self, pixel: PoppedPixel) -> MixedPixel {
        let PoppedPixel {
            x,
            bg_color,
            bg_attributes,
            obj,
        } = pixel;
        let sprite_shown = obj.color != 0 && self.lcdc & LCDC_OBJ_ENABLE != 0;
        let (color, source) = if self.cgb {
            // In CGB mode LCDC bit 0 no longer hides the background; it
            // only takes its priority over sprites away.
            let bg_has_priority = self.lcdc & LCDC_BG_ENABLE != 0;
            let sprite_wins = bg_color == 0
                || !bg_has_priority
                || bg_attributes & ATTR_PRIORITY == 0 && !obj.behind_bg;
            if sprite_shown && sprite_wins {
                (obj.color, Source::Sprite(obj.palette))
            } else {
                (bg_color, Source::Background(bg_attributes & ATTR_PALETTE))
            }
        } else {
            let bg_enabled = self.lcdc & LCDC_BG_ENABLE != 0;
            let bg_color = if bg_enabled { bg_color } else { 0 };
            if sprite_shown && !(obj.behind_bg && bg_color != 0) {
                (obj.color, Source::Sprite(obj.palette))
            } else if bg_enabled {
                (bg_color, Source::Background(0))
            } else {
                // With the background switched off the screen is white,
                // not whatever shade BGP gives to color 0.
                (0, Source::Blank)
            }
        };
        MixedPixel { x, color, source }
    }

    /// Looks a mixed pixel up in the palettes as they are right now and
    /// writes it to the screen.
    fn write_pixel(&mut self, pixel: MixedPixel) {
        let index = self.ly as usize * SCREEN_WIDTH + pixel.x as usize;
        if self.cgb {
            self.color_framebuffer[index] = match pixel.source {
                Source::Sprite(palette) => cgb_color(&self.obj_palettes, palette, pixel.color),
                Source::Background(palette) => cgb_color(&self.bg_palettes, palette, pixel.color),
                Source::Blank => WHITE,
            };
            return;
        }
        let shade = match pixel.source {
            Source::Sprite(0) => shade(self.obp0, pixel.color),
            Source::Sprite(_) => shade(self.obp1, pixel.color),
            Source::Background(_) => shade(self.bgp | self.old_bgp, pixel.color),
            Source::Blank => 0,
        };
        self.framebuffer[index] = shade;
        if self.compat {
            // The shade picks a color from the palettes the boot ROM set.
            self.color_framebuffer[index] = match pixel.source {
                Source::Sprite(palette) => cgb_color(&self.obj_palettes, palette, shade),
                Source::Background(_) => cgb_color(&self.bg_palettes, 0, shade),
                Source::Blank => WHITE,
            };
        }
    }

    /// Offset in VRAM of the 32x32 tile map selected by an LCDC bit.
    fn tile_map(&self, lcdc_bit: u8) -> usize {
        if self.lcdc & lcdc_bit != 0 {
            0x1C00
        } else {
            0x1800
        }
    }
}

impl Default for Ppu {
    fn default() -> Self {
        Self::new()
    }
}

/// A pixel's 2-bit color number comes from the same bit of two bytes.
fn color_number(low: u8, high: u8, bit: u8) -> u8 {
    (high >> bit & 1) << 1 | (low >> bit & 1)
}

/// Looks a color up in CGB palette RAM: 8 palettes of 4 little-endian
/// RGB555 colors.
fn cgb_color(ram: &[u8; PALETTE_RAM_SIZE], palette: u8, color: u8) -> u16 {
    let index = palette as usize * 8 + color as usize * 2;
    u16::from_le_bytes([ram[index], ram[index + 1]]) & 0x7FFF
}

/// Looks a color number up in a palette register (2 bits per color).
fn shade(palette: u8, color: u8) -> u8 {
    palette >> (color * 2) & 0x03
}

impl Ppu {
    pub(crate) fn save_state(&self, w: &mut Writer) {
        w.bytes(&self.vram);
        w.bytes(&self.oam);
        for register in [
            self.lcdc,
            self.stat_sources,
            self.scy,
            self.scx,
            self.ly,
            self.lyc,
            self.bgp,
            self.obp0,
            self.obp1,
            self.wy,
            self.wx,
        ] {
            w.u8(register);
        }
        w.u8(self.mode as u8);
        w.u8(self.line_cycle);
        w.bool(self.stat_line);
        w.bool(self.vblank_started);
        w.bool(self.window_triggered);
        w.u8(self.window_line);
        w.u8(self.interrupts);
        w.bool(self.frame_ready);
        w.bytes(&self.framebuffer);
        w.bool(self.cgb);
        w.bool(self.compat);
        for &color in self.color_framebuffer.iter() {
            w.u16(color);
        }
        w.u8(self.vram_bank);
        w.bytes(&self.bg_palettes);
        w.bytes(&self.obj_palettes);
        w.u8(self.bcps);
        w.u8(self.ocps);
        w.u8(self.opri);
        w.bool(self.hblank_started);
        w.u8(self.line_x);
        w.u8(self.startup_dots);
        w.bytes(&self.line_sprites);
        w.u8(self.line_sprite_count);
        w.u8(self.next_sprite);
        w.bool(self.window_active);
        w.bool(self.window_drawn);
        w.u16(self.line_dots);
        w.bool(self.edge_wx.is_some());
        w.u8(self.edge_wx.unwrap_or_default());
        w.u8(self.discard);
        w.u8(self.stall);
        w.bool(self.sprite_tile.is_some());
        w.u16(self.sprite_tile.unwrap_or_default());
        w.u8(self.bg_fifo_len);
        w.u8(self.fetch_dots);
        w.u8(self.fetch_x);
        w.u8(self.fetch_tile);
        for row in [self.bg_fifo, self.fetch_row] {
            w.u8(row.low);
            w.u8(row.high);
            w.u8(row.attributes);
        }
        for pixel in self.obj_fifo {
            w.u8(pixel.color);
            w.u8(pixel.palette);
            w.bool(pixel.behind_bg);
            w.u8(pixel.slot);
        }
        w.bool(self.popped.is_some());
        let popped = self.popped.unwrap_or(PoppedPixel {
            x: 0,
            bg_color: 0,
            bg_attributes: 0,
            obj: ObjPixel::default(),
        });
        for byte in [
            popped.x,
            popped.bg_color,
            popped.bg_attributes,
            popped.obj.color,
            popped.obj.palette,
            popped.obj.behind_bg as u8,
            popped.obj.slot,
        ] {
            w.u8(byte);
        }
        w.bool(self.mixed.is_some());
        let mixed = self.mixed.unwrap_or(MixedPixel {
            x: 0,
            color: 0,
            source: Source::Blank,
        });
        w.u8(mixed.x);
        w.u8(mixed.color);
        w.u8(mixed.source.to_byte());
        w.u8(self.old_bgp);
    }

    pub(crate) fn load_state(&mut self, r: &mut Reader) -> Result<(), StateError> {
        r.bytes(&mut self.vram)?;
        r.bytes(&mut self.oam)?;
        for register in [
            &mut self.lcdc,
            &mut self.stat_sources,
            &mut self.scy,
            &mut self.scx,
            &mut self.ly,
            &mut self.lyc,
            &mut self.bgp,
            &mut self.obp0,
            &mut self.obp1,
            &mut self.wy,
            &mut self.wx,
        ] {
            *register = r.u8()?;
        }
        self.mode = match r.u8()? {
            0 => Mode::HBlank,
            1 => Mode::VBlank,
            2 => Mode::OamScan,
            3 => Mode::Drawing,
            _ => return Err(StateError::Corrupt),
        };
        self.line_cycle = r.u8()?;
        self.stat_line = r.bool()?;
        self.vblank_started = r.bool()?;
        self.window_triggered = r.bool()?;
        self.window_line = r.u8()?;
        self.interrupts = r.u8()?;
        self.frame_ready = r.bool()?;
        r.bytes(&mut self.framebuffer)?;
        // A state from a console in the other mode cannot be continued here.
        if r.bool()? != self.cgb || r.bool()? != self.compat {
            return Err(StateError::Corrupt);
        }
        for color in self.color_framebuffer.iter_mut() {
            *color = r.u16()? & 0x7FFF;
        }
        self.vram_bank = r.u8()? & 0x01;
        r.bytes(&mut self.bg_palettes)?;
        r.bytes(&mut self.obj_palettes)?;
        self.bcps = r.u8()? & 0xBF;
        self.ocps = r.u8()? & 0xBF;
        self.opri = r.u8()? & 0x01;
        self.hblank_started = r.bool()?;
        self.line_x = r.u8()?.min(SCREEN_WIDTH as u8);
        self.startup_dots = r.u8()?.min(STARTUP_DOTS);
        r.bytes(&mut self.line_sprites)?;
        for slot in &mut self.line_sprites {
            *slot %= 40;
        }
        self.line_sprite_count = r.u8()?.min(SPRITES_PER_LINE as u8);
        self.next_sprite = r.u8()?.min(self.line_sprite_count);
        self.window_active = r.bool()?;
        self.window_drawn = r.bool()?;
        self.line_dots = r.u16()?;
        let edge = r.bool()?;
        let wx = r.u8()? & 7;
        self.edge_wx = edge.then_some(wx.min(6));
        self.discard = r.u8()? & 7;
        self.stall = r.u8()?;
        let has_tile = r.bool()?;
        let tile = r.u16()?;
        self.sprite_tile = has_tile.then_some(tile);
        self.bg_fifo_len = r.u8()?.min(8);
        self.fetch_dots = r.u8()?.min(FETCH_DOTS);
        self.fetch_x = r.u8()?;
        self.fetch_tile = r.u8()?;
        for row in [&mut self.bg_fifo, &mut self.fetch_row] {
            *row = TileRow {
                low: r.u8()?,
                high: r.u8()?,
                attributes: r.u8()?,
            };
        }
        for pixel in &mut self.obj_fifo {
            *pixel = ObjPixel {
                color: r.u8()? & 3,
                palette: r.u8()? & 7,
                behind_bg: r.bool()?,
                slot: r.u8()? % 40,
            };
        }
        // Screen positions past the edge would write out of bounds.
        let x_limit = SCREEN_WIDTH as u8 - 1;
        let has_popped = r.bool()?;
        let popped = PoppedPixel {
            x: r.u8()?.min(x_limit),
            bg_color: r.u8()? & 3,
            bg_attributes: r.u8()?,
            obj: ObjPixel {
                color: r.u8()? & 3,
                palette: r.u8()? & 7,
                behind_bg: r.u8()? != 0,
                slot: r.u8()? % 40,
            },
        };
        self.popped = has_popped.then_some(popped);
        let has_mixed = r.bool()?;
        let mixed = MixedPixel {
            x: r.u8()?.min(x_limit),
            color: r.u8()? & 3,
            source: Source::from_byte(r.u8()?),
        };
        self.mixed = has_mixed.then_some(mixed);
        self.old_bgp = r.u8()?;
        // Values a running PPU can never hold would make it index out of
        // bounds later.
        let valid = self.ly < LINES_PER_FRAME
            && self.line_cycle < CYCLES_PER_LINE
            && self.framebuffer.iter().all(|&shade| shade < 4);
        if valid {
            Ok(())
        } else {
            Err(StateError::Corrupt)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A PPU with an identity palette, so shades equal color numbers.
    fn ppu() -> Ppu {
        let mut ppu = Ppu::new();
        ppu.write_register(BGP, 0xE4);
        ppu.write_register(OBP0, 0xE4);
        ppu.write_register(OBP1, 0x1B);
        ppu
    }

    fn run_cycles(ppu: &mut Ppu, cycles: u32) {
        for _ in 0..cycles {
            ppu.tick();
        }
    }

    fn run_frame(ppu: &mut Ppu) {
        run_cycles(ppu, 154 * 114);
    }

    /// Fills tile `index` (in the 0x8000 area) with one color number.
    fn solid_tile(ppu: &mut Ppu, index: usize, color: u8) {
        for row in 0..8 {
            ppu.vram[index * 16 + row * 2] = if color & 1 != 0 { 0xFF } else { 0x00 };
            ppu.vram[index * 16 + row * 2 + 1] = if color & 2 != 0 { 0xFF } else { 0x00 };
        }
    }

    fn set_sprite(ppu: &mut Ppu, slot: usize, y: u8, x: u8, tile: u8, attributes: u8) {
        ppu.oam[slot * 4..slot * 4 + 4].copy_from_slice(&[y, x, tile, attributes]);
    }

    fn pixel(ppu: &Ppu, x: usize, y: usize) -> u8 {
        ppu.framebuffer[y * SCREEN_WIDTH + x]
    }

    /// A CGB PPU with palette 0 set to four distinct colors, both for the
    /// background and for sprites, and palette 1 to four others.
    fn cgb_ppu() -> Ppu {
        let mut ppu = Ppu::new_cgb();
        ppu.write_register(BCPS, 0x80);
        ppu.write_register(OCPS, 0x80);
        for color in [
            0x7FFFu16, 0x001F, 0x03E0, 0x7C00, 0x0000, 0x1111, 0x2222, 0x3333,
        ] {
            for byte in color.to_le_bytes() {
                ppu.write_register(BCPD, byte);
                ppu.write_register(OCPD, byte);
            }
        }
        ppu
    }

    fn color(ppu: &Ppu, x: usize, y: usize) -> u16 {
        ppu.color_framebuffer[y * SCREEN_WIDTH + x]
    }

    #[test]
    fn cgb_palette_ram_auto_increments() {
        let mut ppu = Ppu::new_cgb();
        ppu.write_register(BCPS, 0x80 | 0x3E);
        ppu.write_register(BCPD, 0x12);
        ppu.write_register(BCPD, 0x34);
        assert_eq!(ppu.read_register(BCPS), 0x80 | 0x40, "wrapped to 0");
        ppu.write_register(BCPS, 0x3E);
        assert_eq!(ppu.read_register(BCPD), 0x12);
        ppu.write_register(BCPD, 0x56);
        assert_eq!(ppu.read_register(BCPS), 0x40 | 0x3E, "no auto-increment");
        assert_eq!(ppu.read_register(BCPD), 0x56);
    }

    #[test]
    fn color_registers_do_not_exist_in_dmg_mode() {
        let mut ppu = ppu();
        ppu.write_register(VBK, 1);
        assert_eq!(ppu.read_register(VBK), 0xFF);
        assert_eq!(ppu.read_register(BCPS), 0xFF);
        ppu.write_vram(0x8000, 0x12);
        assert_eq!(ppu.vram[0], 0x12, "always bank 0");
    }

    #[test]
    fn cgb_vram_has_two_banks() {
        let mut ppu = Ppu::new_cgb();
        ppu.write_vram(0x8000, 0x11);
        ppu.write_register(VBK, 1);
        assert_eq!(ppu.read_register(VBK), 0xFF);
        assert_eq!(ppu.read_vram(0x8000), 0x00);
        ppu.write_vram(0x8000, 0x22);
        ppu.write_register(VBK, 0);
        assert_eq!(ppu.read_vram(0x8000), 0x11);
        assert_eq!(ppu.vram[VRAM_BANK_SIZE], 0x22);
    }

    #[test]
    fn cgb_background_uses_attributes_for_palette_bank_and_flips() {
        let mut ppu = cgb_ppu();
        // Bank 1, tile 0: only the top-left pixel set, color 1.
        ppu.vram[VRAM_BANK_SIZE] = 0x80;
        // Map entry (0, 0): tile 0, attributes: palette 1, bank 1, flip X and Y.
        ppu.vram[0x1800] = 0;
        ppu.vram[VRAM_BANK_SIZE + 0x1800] = 0x01 | ATTR_BANK_1 | ATTR_FLIP_X | ATTR_FLIP_Y;
        run_frame(&mut ppu);
        assert_eq!(color(&ppu, 7, 7), 0x1111, "palette 1, color 1, flipped");
        assert_eq!(color(&ppu, 0, 0), 0x0000, "palette 1, color 0");
        assert_eq!(color(&ppu, 8, 0), 0x7FFF, "next tile: palette 0, color 0");
    }

    #[test]
    fn cgb_sprites_use_their_own_palette_and_oam_order() {
        let mut ppu = cgb_ppu();
        solid_tile(&mut ppu, 1, 1);
        solid_tile(&mut ppu, 2, 2);
        ppu.write_register(LCDC, 0x93);
        // Slot 0 is further right than slot 1, but on the Color the earlier
        // OAM slot wins.
        set_sprite(&mut ppu, 0, 16, 12, 1, 0x01);
        set_sprite(&mut ppu, 1, 16, 8, 2, 0x00);
        run_frame(&mut ppu);
        assert_eq!(color(&ppu, 4, 0), 0x1111, "slot 0, palette 1, color 1");
        assert_eq!(color(&ppu, 0, 0), 0x03E0, "slot 1, palette 0, color 2");
    }

    #[test]
    fn cgb_lcdc_bit_0_puts_sprites_above_everything() {
        let mut ppu = cgb_ppu();
        solid_tile(&mut ppu, 1, 3);
        solid_tile(&mut ppu, 2, 1);
        // Background tile 2 (color 1) everywhere, with BG priority set.
        ppu.vram[0x1800..0x1C00].fill(2);
        ppu.vram[VRAM_BANK_SIZE + 0x1800..VRAM_BANK_SIZE + 0x1C00].fill(ATTR_PRIORITY);
        set_sprite(&mut ppu, 0, 16, 8, 1, 0);
        ppu.write_register(LCDC, 0x93);
        run_frame(&mut ppu);
        assert_eq!(color(&ppu, 0, 0), 0x001F, "background wins");
        ppu.write_register(LCDC, 0x92);
        run_frame(&mut ppu);
        assert_eq!(
            color(&ppu, 0, 0),
            0x7C00,
            "sprite wins; background still drawn"
        );
        assert_eq!(color(&ppu, 20, 0), 0x001F);
    }

    #[test]
    fn modes_follow_the_scanline_schedule() {
        let mut ppu = ppu();
        assert_eq!(ppu.mode(), Mode::OamScan);
        run_cycles(&mut ppu, 20);
        assert_eq!(ppu.mode(), Mode::OamScan);
        run_cycles(&mut ppu, 1);
        assert_eq!(ppu.mode(), Mode::Drawing);
        run_cycles(&mut ppu, 43);
        assert_eq!(ppu.mode(), Mode::HBlank);
        run_cycles(&mut ppu, 50);
        assert_eq!((ppu.ly(), ppu.mode()), (1, Mode::OamScan));
    }

    #[test]
    fn fine_scroll_makes_drawing_take_longer() {
        // Mode 3 takes 172 + 5 dots. Three of them fall in M-cycle 21, so
        // the last pixel comes out in M-cycle 21 + ceil(174 / 4) = 65,
        // one later than without scrolling.
        let mut ppu = ppu();
        ppu.write_register(SCX, 5);
        run_cycles(&mut ppu, 64);
        assert_eq!(ppu.mode(), Mode::Drawing);
        run_cycles(&mut ppu, 1);
        assert_eq!(ppu.mode(), Mode::HBlank);
    }

    #[test]
    fn a_sprite_makes_drawing_take_longer() {
        // A sprite at the start of a background tile pauses the pixels for
        // 6 dots, plus 5 waiting for the background fetch: 183 dots, so
        // the last pixel comes out in M-cycle 21 + ceil(180 / 4) = 66.
        let mut ppu = ppu();
        ppu.write_register(LCDC, 0x93);
        set_sprite(&mut ppu, 0, 16, 8, 0, 0);
        run_cycles(&mut ppu, 65);
        assert_eq!(ppu.mode(), Mode::Drawing);
        run_cycles(&mut ppu, 1);
        assert_eq!(ppu.mode(), Mode::HBlank);
    }

    #[test]
    fn a_palette_change_in_mode_3_shows_from_that_point_of_the_line() {
        let mut ppu = ppu();
        solid_tile(&mut ppu, 0, 1);
        run_cycles(&mut ppu, 40);
        ppu.write_register(BGP, 0xEC);
        run_cycles(&mut ppu, 114 - 40);
        let line = &ppu.framebuffer()[..SCREEN_WIDTH];
        assert_eq!((line[0], line[SCREEN_WIDTH - 1]), (1, 3));
        let first_new = line.iter().position(|&shade| shade == 3).unwrap();
        assert!(line[first_new..].iter().all(|&shade| shade == 3));
    }

    #[test]
    fn vblank_starts_at_line_144_and_requests_an_interrupt() {
        let mut ppu = ppu();
        run_cycles(&mut ppu, 144 * 114 - 1);
        assert_eq!(ppu.take_interrupts() & interrupts::VBLANK, 0);
        assert!(!ppu.take_frame_ready());
        run_cycles(&mut ppu, 1);
        assert_eq!((ppu.ly(), ppu.mode()), (144, Mode::VBlank));
        assert_eq!(
            ppu.take_interrupts() & interrupts::VBLANK,
            interrupts::VBLANK
        );
        assert!(ppu.take_frame_ready());
        run_cycles(&mut ppu, 10 * 114);
        assert_eq!((ppu.ly(), ppu.mode()), (0, Mode::VBlank), "one cycle late");
        run_cycles(&mut ppu, 1);
        assert_eq!((ppu.ly(), ppu.mode()), (0, Mode::OamScan));
    }

    #[test]
    fn oam_source_also_fires_when_vblank_begins() {
        let mut ppu = ppu();
        run_cycles(&mut ppu, 143 * 114 + 70);
        ppu.write_register(STAT, STAT_OAM);
        ppu.take_interrupts();
        run_cycles(&mut ppu, 44);
        assert_eq!(ppu.ly(), 144);
        assert_eq!(ppu.take_interrupts() & interrupts::STAT, interrupts::STAT);
    }

    #[test]
    fn stat_reports_mode_and_lyc_match() {
        let mut ppu = ppu();
        ppu.write_register(LYC, 2);
        assert_eq!(ppu.read_register(STAT), 0x80 | 0x02);
        run_cycles(&mut ppu, 2 * 114);
        assert_eq!(ppu.read_register(STAT), 0x80 | 0x04 | 0x02);
    }

    #[test]
    fn lyc_interrupt_fires_once_when_the_line_matches() {
        let mut ppu = ppu();
        ppu.write_register(LYC, 5);
        ppu.write_register(STAT, STAT_LYC);
        run_cycles(&mut ppu, 5 * 114 - 1);
        assert_eq!(ppu.take_interrupts() & interrupts::STAT, 0);
        run_cycles(&mut ppu, 1);
        assert_eq!(ppu.take_interrupts() & interrupts::STAT, interrupts::STAT);
        run_cycles(&mut ppu, 100);
        assert_eq!(ppu.take_interrupts() & interrupts::STAT, 0);
    }

    #[test]
    fn stat_sources_already_high_block_each_other() {
        let mut ppu = ppu();
        // HBlank is followed directly by the next line's OAM scan, so with
        // both sources enabled the line never drops in between and the OAM
        // scan does not interrupt again.
        ppu.write_register(STAT, STAT_HBLANK | STAT_OAM);
        ppu.take_interrupts();
        run_cycles(&mut ppu, 64);
        assert_eq!(ppu.mode(), Mode::HBlank);
        assert_eq!(ppu.take_interrupts() & interrupts::STAT, interrupts::STAT);
        run_cycles(&mut ppu, 50);
        assert_eq!(ppu.mode(), Mode::OamScan);
        assert_eq!(ppu.take_interrupts() & interrupts::STAT, 0, "blocked");
    }

    #[test]
    fn turning_the_lcd_off_resets_ly_and_blanks_the_screen() {
        let mut ppu = ppu();
        solid_tile(&mut ppu, 0, 3);
        run_frame(&mut ppu);
        assert_eq!(pixel(&ppu, 0, 0), 3);
        run_cycles(&mut ppu, 30 * 114);

        ppu.write_register(LCDC, 0x11);
        assert_eq!(ppu.ly(), 0);
        assert_eq!(pixel(&ppu, 0, 0), 0);
        run_cycles(&mut ppu, 1000);
        assert_eq!(ppu.ly(), 0, "time stands still while off");
    }

    #[test]
    fn background_scrolls_and_wraps() {
        let mut ppu = ppu();
        solid_tile(&mut ppu, 1, 3);
        // Tile 1 at map position (31, 0): the last column of the first row.
        ppu.vram[0x1800 + 31] = 1;
        ppu.write_register(SCX, 252);
        run_frame(&mut ppu);
        // Screen x 0..4 shows map x 252..256; then the map wraps to x 0.
        assert_eq!(pixel(&ppu, 3, 0), 3);
        assert_eq!(pixel(&ppu, 4, 0), 0);
    }

    #[test]
    fn signed_tile_addressing_reaches_the_0x8800_block() {
        let mut ppu = ppu();
        // Tile 0x80 lives at 0x8800 in both addressing modes.
        solid_tile(&mut ppu, 0x80, 2);
        ppu.vram[0x1800] = 0x80;
        ppu.write_register(LCDC, 0x81);
        run_frame(&mut ppu);
        assert_eq!(pixel(&ppu, 0, 0), 2);

        // Tile 0 in signed mode is at 0x9000, i.e. tile 0x100 of the area.
        solid_tile(&mut ppu, 0x100, 1);
        ppu.vram[0x1800] = 0x00;
        run_frame(&mut ppu);
        assert_eq!(pixel(&ppu, 0, 0), 1);
    }

    #[test]
    fn disabled_background_is_white() {
        let mut ppu = ppu();
        solid_tile(&mut ppu, 0, 3);
        ppu.write_register(BGP, 0xFF);
        ppu.write_register(LCDC, 0x90);
        run_frame(&mut ppu);
        assert_eq!(pixel(&ppu, 80, 72), 0);
    }

    #[test]
    fn window_covers_the_background_from_wx_wy() {
        let mut ppu = ppu();
        solid_tile(&mut ppu, 1, 3);
        // The window uses the 0x9C00 map, filled with tile 1.
        ppu.vram[0x1C00..0x2000].fill(1);
        ppu.write_register(LCDC, 0x91 | LCDC_WINDOW_ENABLE | LCDC_WINDOW_MAP_HIGH);
        ppu.write_register(WY, 100);
        ppu.write_register(WX, 87);
        run_frame(&mut ppu);
        assert_eq!(pixel(&ppu, 80, 99), 0, "above the window");
        assert_eq!(pixel(&ppu, 79, 100), 0, "left of the window");
        assert_eq!(pixel(&ppu, 80, 100), 3);
        assert_eq!(pixel(&ppu, 159, 143), 3);
    }

    #[test]
    fn window_line_counter_ignores_lines_where_it_is_hidden() {
        let mut ppu = ppu();
        // Window map row 0 uses tile 1 (color 1), row 1 uses tile 2 (color 2).
        solid_tile(&mut ppu, 1, 1);
        solid_tile(&mut ppu, 2, 2);
        ppu.vram[0x1C00..0x1C20].fill(1);
        ppu.vram[0x1C20..0x1C40].fill(2);
        ppu.write_register(LCDC, 0x91 | LCDC_WINDOW_ENABLE | LCDC_WINDOW_MAP_HIGH);
        ppu.write_register(WY, 0);
        ppu.write_register(WX, 7);

        // Show the window for 4 lines, hide it for 10, then show it again.
        run_cycles(&mut ppu, 4 * 114);
        ppu.write_register(LCDC, 0x91 | LCDC_WINDOW_MAP_HIGH);
        run_cycles(&mut ppu, 10 * 114);
        ppu.write_register(LCDC, 0x91 | LCDC_WINDOW_ENABLE | LCDC_WINDOW_MAP_HIGH);
        run_cycles(&mut ppu, 130 * 114);

        assert_eq!(pixel(&ppu, 0, 3), 1);
        assert_eq!(pixel(&ppu, 0, 14), 1, "continues at window line 4");
        assert_eq!(pixel(&ppu, 0, 17), 1, "window line 7: still map row 0");
        assert_eq!(pixel(&ppu, 0, 18), 2, "window line 8: map row 1");
    }

    #[test]
    fn sprite_is_drawn_with_its_offsets_and_transparency() {
        let mut ppu = ppu();
        // Tile 1: left half color 0 (transparent), right half color 3.
        for row in 0..8 {
            ppu.vram[16 + row * 2] = 0x0F;
            ppu.vram[16 + row * 2 + 1] = 0x0F;
        }
        ppu.write_register(LCDC, 0x93);
        set_sprite(&mut ppu, 0, 16, 8, 1, 0);
        run_frame(&mut ppu);
        assert_eq!(
            pixel(&ppu, 3, 0),
            0,
            "transparent half shows the background"
        );
        assert_eq!(pixel(&ppu, 4, 0), 3);
        assert_eq!(pixel(&ppu, 7, 7), 3);
        assert_eq!(pixel(&ppu, 8, 0), 0);
        assert_eq!(pixel(&ppu, 4, 8), 0);
    }

    #[test]
    fn sprite_flips_and_palettes() {
        let mut ppu = ppu();
        // Tile 1: only the top-left pixel is set, color 1.
        ppu.vram[16] = 0x80;
        ppu.write_register(LCDC, 0x93);
        set_sprite(
            &mut ppu,
            0,
            16,
            8,
            1,
            OBJ_FLIP_X | OBJ_FLIP_Y | OBJ_PALETTE_1,
        );
        run_frame(&mut ppu);
        assert_eq!(pixel(&ppu, 0, 0), 0);
        // OBP1 is 0x1B in these tests: color 1 maps to shade 2.
        assert_eq!(pixel(&ppu, 7, 7), 2);
    }

    #[test]
    fn tall_sprites_use_two_tiles() {
        let mut ppu = ppu();
        solid_tile(&mut ppu, 2, 1);
        solid_tile(&mut ppu, 3, 2);
        ppu.write_register(LCDC, 0x93 | LCDC_OBJ_TALL);
        // Tile number 3 is rounded down to 2 for the top half.
        set_sprite(&mut ppu, 0, 16, 8, 3, 0);
        run_frame(&mut ppu);
        assert_eq!(pixel(&ppu, 0, 0), 1);
        assert_eq!(pixel(&ppu, 0, 8), 2);
        assert_eq!(pixel(&ppu, 0, 16), 0);
    }

    #[test]
    fn sprite_behind_background_only_shows_over_color_zero() {
        let mut ppu = ppu();
        solid_tile(&mut ppu, 1, 3);
        solid_tile(&mut ppu, 2, 1);
        // Background: tile 2 (color 1) in the first column, tile 0 elsewhere.
        ppu.vram[0x1800] = 2;
        ppu.write_register(LCDC, 0x93);
        set_sprite(&mut ppu, 0, 16, 12, 1, OBJ_BEHIND_BG);
        run_frame(&mut ppu);
        assert_eq!(pixel(&ppu, 7, 0), 1, "background color 1 hides the sprite");
        assert_eq!(pixel(&ppu, 8, 0), 3, "background color 0 lets it through");
    }

    #[test]
    fn leftmost_sprite_wins_and_oam_order_breaks_ties() {
        let mut ppu = ppu();
        solid_tile(&mut ppu, 1, 1);
        solid_tile(&mut ppu, 2, 2);
        solid_tile(&mut ppu, 3, 3);
        ppu.write_register(LCDC, 0x93);
        // Slot 0 is further right than slot 1; slot 2 ties with slot 1.
        set_sprite(&mut ppu, 0, 16, 12, 1, 0);
        set_sprite(&mut ppu, 1, 16, 8, 2, 0);
        set_sprite(&mut ppu, 2, 16, 8, 3, 0);
        run_frame(&mut ppu);
        assert_eq!(pixel(&ppu, 4, 0), 2, "x=8 beats x=12, slot 1 beats slot 2");
        assert_eq!(pixel(&ppu, 8, 0), 1);
    }

    #[test]
    fn only_ten_sprites_per_line() {
        let mut ppu = ppu();
        solid_tile(&mut ppu, 1, 3);
        ppu.write_register(LCDC, 0x93);
        for slot in 0..11 {
            set_sprite(&mut ppu, slot, 16, 8 + slot as u8 * 8, 1, 0);
        }
        run_frame(&mut ppu);
        assert_eq!(pixel(&ppu, 79, 0), 3, "tenth sprite");
        assert_eq!(pixel(&ppu, 80, 0), 0, "eleventh sprite is dropped");
    }
}
