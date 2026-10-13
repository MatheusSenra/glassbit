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
//! This implementation renders a whole line at once when mode 3 starts. The
//! real PPU pushes pixels one by one and stretches mode 3 depending on
//! sprites, fine scrolling and the window; only the fine scrolling part is
//! modeled here. Programs that change registers in the middle of a line, or
//! that time themselves against the exact end of mode 3, would notice.
//!
//! The M-cycles at which modes change were fitted to Mooneye's `acceptance/ppu`
//! timing tests, since Pan Docs gives them in dots relative to a line start
//! that the CPU cannot observe directly.
//!
//! Reference: <https://gbdev.io/pandocs/Rendering.html>

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
const OBJ_FLIP_X: u8 = 0x20;
const OBJ_FLIP_Y: u8 = 0x40;
const OBJ_BEHIND_BG: u8 = 0x80;

/// M-cycles per scanline (456 dots).
const CYCLES_PER_LINE: u8 = 114;
/// M-cycle of the line at which mode 3 starts. LY goes up one M-cycle before
/// the line proper begins, so this is the 80 dots of mode 2 plus that cycle.
const DRAWING_START: u8 = 21;
/// M-cycle of the line at which mode 0 starts: mode 3 lasts at least 172
/// dots (43 M-cycles).
const HBLANK_START: u8 = 64;
/// On line 0 the OAM scan is only reported one M-cycle after LY wraps.
const FIRST_LINE_OAM_START: u8 = 1;
const LINES_PER_FRAME: u8 = 154;
const SPRITES_PER_LINE: usize = 10;

const VRAM_SIZE: usize = 0x2000;
const OAM_SIZE: usize = 0xA0;

/// What the PPU is doing; the numbers are the ones STAT reports.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    HBlank = 0,
    VBlank = 1,
    OamScan = 2,
    Drawing = 3,
}

/// A sprite pixel waiting to be combined with the background.
#[derive(Clone, Copy)]
struct SpritePixel {
    /// Color number 1-3 (0 is transparent and never stored).
    color: u8,
    palette: u8,
    behind_bg: bool,
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
    /// Extra M-cycles mode 3 takes on the current line.
    drawing_penalty: u8,
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
    /// One shade (0 = lightest, 3 = darkest) per pixel.
    framebuffer: Box<[u8]>,
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
            drawing_penalty: 0,
            vblank_started: false,
            window_triggered: false,
            window_line: 0,
            interrupts: 0,
            frame_ready: false,
            framebuffer: vec![0; SCREEN_WIDTH * SCREEN_HEIGHT].into_boxed_slice(),
        }
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
                // Discarding the first SCX % 8 pixels of the leftmost tile
                // costs that many dots; the CPU sees the delay rounded up
                // to whole M-cycles.
                self.drawing_penalty = (self.scx % 8).div_ceil(4);
                self.render_line();
            } else if self.line_cycle == HBLANK_START + self.drawing_penalty {
                self.mode = Mode::HBlank;
            }
        }
        self.update_stat_line();
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

    pub fn read_vram(&self, addr: u16) -> u8 {
        self.vram[(addr - 0x8000) as usize]
    }

    pub fn write_vram(&mut self, addr: u16, value: u8) {
        self.vram[(addr - 0x8000) as usize] = value;
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
            _ => 0xFF,
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
            BGP => self.bgp = value,
            OBP0 => self.obp0 = value,
            OBP1 => self.obp1 = value,
            WY => self.wy = value,
            WX => self.wx = value,
            _ => {}
        }
    }

    /// Draws line `ly` into the framebuffer.
    fn render_line(&mut self) {
        if self.ly == self.wy {
            self.window_triggered = true;
        }

        // Color numbers (before the palette) of the background and window,
        // kept because sprites need them to decide who is in front.
        let mut bg_colors = [0u8; SCREEN_WIDTH];
        let bg_enabled = self.lcdc & LCDC_BG_ENABLE != 0;
        if bg_enabled {
            self.render_background(&mut bg_colors);
            self.render_window(&mut bg_colors);
        }

        let mut sprites = [None; SCREEN_WIDTH];
        if self.lcdc & LCDC_OBJ_ENABLE != 0 {
            self.render_sprites(&mut sprites);
        }

        let start = self.ly as usize * SCREEN_WIDTH;
        let row = &mut self.framebuffer[start..start + SCREEN_WIDTH];
        for (x, pixel) in row.iter_mut().enumerate() {
            *pixel = match sprites[x] {
                Some(sprite) if !(sprite.behind_bg && bg_colors[x] != 0) => {
                    let palette = if sprite.palette == 0 {
                        self.obp0
                    } else {
                        self.obp1
                    };
                    shade(palette, sprite.color)
                }
                // With the background switched off the screen is white, not
                // whatever shade BGP gives to color 0.
                _ if !bg_enabled => 0,
                _ => shade(self.bgp, bg_colors[x]),
            };
        }
    }

    fn render_background(&self, colors: &mut [u8; SCREEN_WIDTH]) {
        let map = self.tile_map(LCDC_BG_MAP_HIGH);
        let y = self.ly.wrapping_add(self.scy);
        for (screen_x, color) in colors.iter_mut().enumerate() {
            // The background is a 256x256 picture that wraps around.
            let x = (screen_x as u8).wrapping_add(self.scx);
            *color = self.tile_map_pixel(map, x, y);
        }
    }

    fn render_window(&mut self, colors: &mut [u8; SCREEN_WIDTH]) {
        let visible =
            self.lcdc & LCDC_WINDOW_ENABLE != 0 && self.window_triggered && self.wx <= 166;
        if !visible {
            return;
        }
        let map = self.tile_map(LCDC_WINDOW_MAP_HIGH);
        // WX is offset by 7: the window's left edge is at screen x = WX - 7.
        let first_x = (self.wx as usize).saturating_sub(7);
        for (screen_x, color) in colors.iter_mut().enumerate().skip(first_x) {
            let x = (screen_x + 7 - self.wx as usize) as u8;
            *color = self.tile_map_pixel(map, x, self.window_line);
        }
        self.window_line += 1;
    }

    fn render_sprites(&self, pixels: &mut [Option<SpritePixel>; SCREEN_WIDTH]) {
        let height = if self.lcdc & LCDC_OBJ_TALL != 0 {
            16
        } else {
            8
        };

        // OAM scan: the first ten sprites, in OAM order, that cover this
        // line. Sprite Y is offset by 16 so sprites can slide in from above.
        let line = self.ly + 16;
        let mut selected: Vec<&[u8]> = self
            .oam
            .chunks_exact(4)
            .filter(|sprite| (sprite[0]..sprite[0].saturating_add(height)).contains(&line))
            .take(SPRITES_PER_LINE)
            .collect();
        // Where sprites overlap, the one further left is in front; ties go
        // to the one earlier in OAM (the sort is stable).
        selected.sort_by_key(|sprite| sprite[1]);

        for sprite in selected {
            let (y, x, tile, attributes) = (sprite[0], sprite[1], sprite[2], sprite[3]);
            let mut row = line - y;
            if attributes & OBJ_FLIP_Y != 0 {
                row = height - 1 - row;
            }
            // A tall sprite uses two consecutive tiles, the first one even.
            let tile = if height == 16 { tile & 0xFE } else { tile };
            let data = tile as usize * 16 + row as usize * 2;
            let (low, high) = (self.vram[data], self.vram[data + 1]);

            for column in 0..8u8 {
                // Sprite X is offset by 8.
                let Some(screen_x) = (x as usize + column as usize).checked_sub(8) else {
                    continue;
                };
                if screen_x >= SCREEN_WIDTH || pixels[screen_x].is_some() {
                    continue;
                }
                let bit = if attributes & OBJ_FLIP_X != 0 {
                    column
                } else {
                    7 - column
                };
                let color = color_number(low, high, bit);
                if color != 0 {
                    pixels[screen_x] = Some(SpritePixel {
                        color,
                        palette: (attributes & OBJ_PALETTE_1 != 0) as u8,
                        behind_bg: attributes & OBJ_BEHIND_BG != 0,
                    });
                }
            }
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

    /// The color number at pixel (`x`, `y`) of a 256x256 tile map.
    fn tile_map_pixel(&self, map: usize, x: u8, y: u8) -> u8 {
        let tile = self.vram[map + (y as usize / 8) * 32 + x as usize / 8];
        // Tile numbers index 0x8000 directly, or are signed around 0x9000.
        let base = if self.lcdc & LCDC_TILE_DATA_LOW != 0 {
            tile as usize * 16
        } else {
            (0x1000 + (tile as i8 as i32) * 16) as usize
        };
        let data = base + (y as usize % 8) * 2;
        color_number(self.vram[data], self.vram[data + 1], 7 - x % 8)
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
        w.u8(self.drawing_penalty);
        w.bool(self.vblank_started);
        w.bool(self.window_triggered);
        w.u8(self.window_line);
        w.u8(self.interrupts);
        w.bool(self.frame_ready);
        w.bytes(&self.framebuffer);
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
        self.drawing_penalty = r.u8()?;
        self.vblank_started = r.bool()?;
        self.window_triggered = r.bool()?;
        self.window_line = r.u8()?;
        self.interrupts = r.u8()?;
        self.frame_ready = r.bool()?;
        r.bytes(&mut self.framebuffer)?;
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
        let mut ppu = ppu();
        ppu.write_register(SCX, 5);
        run_cycles(&mut ppu, 65);
        assert_eq!(ppu.mode(), Mode::Drawing);
        run_cycles(&mut ppu, 1);
        assert_eq!(ppu.mode(), Mode::HBlank);
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
