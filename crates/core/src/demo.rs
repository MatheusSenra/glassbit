//! A tiny built-in cartridge, so a frontend has something to show before the
//! user loads a ROM, and so input can be tested end to end.
//!
//! It plays a short chime, draws one sprite on a checkerboard and moves it
//! with the direction pad; holding A inverts the colors. The program is assembled below, by
//! hand, one instruction per line.

/// Where the sprite starts (in OAM coordinates, which are offset by 8 and 16).
pub const START_X: u8 = 84;
pub const START_Y: u8 = 88;

const ENTRY: u16 = 0x0150;

/// A minimal assembler: collects bytes and resolves relative jumps to labels.
struct Program {
    bytes: Vec<u8>,
    labels: Vec<(&'static str, usize)>,
    jumps: Vec<(usize, &'static str)>,
}

impl Program {
    fn new() -> Self {
        Self {
            bytes: Vec::new(),
            labels: Vec::new(),
            jumps: Vec::new(),
        }
    }

    fn emit(&mut self, bytes: &[u8]) {
        self.bytes.extend_from_slice(bytes);
    }

    fn label(&mut self, name: &'static str) {
        self.labels.push((name, self.bytes.len()));
    }

    /// Emits a two-byte relative jump (`JR`, `JR NZ`, ...) to a label.
    fn jump(&mut self, opcode: u8, target: &'static str) {
        self.emit(&[opcode, 0]);
        self.jumps.push((self.bytes.len() - 1, target));
    }

    /// The address a label will have once the program is placed at `ENTRY`.
    fn address(&self, name: &str) -> u16 {
        let (_, offset) = self
            .labels
            .iter()
            .find(|(label, _)| *label == name)
            .unwrap();
        ENTRY + *offset as u16
    }

    fn finish(mut self) -> Vec<u8> {
        for &(operand, target) in &self.jumps {
            let (_, destination) = self.labels.iter().find(|(l, _)| *l == target).unwrap();
            // Relative to the address after the jump instruction.
            let distance = *destination as isize - (operand as isize + 1);
            self.bytes[operand] = i8::try_from(distance).expect("jump in range") as u8;
        }
        self.bytes
    }
}

const JR: u8 = 0x18;
const JR_NZ: u8 = 0x20;
const JR_Z: u8 = 0x28;

/// Tile 1: a ball, in color 3 with a color 1 highlight.
#[rustfmt::skip]
const BALL: [u8; 16] = [
    0x3C, 0x3C,
    0x7E, 0x7E,
    0xFF, 0xCF,
    0xFF, 0xCF,
    0xFF, 0xFF,
    0xFF, 0xFF,
    0x7E, 0x7E,
    0x3C, 0x3C,
];

/// Tile 2: a solid block of color 1, for the checkerboard.
const BLOCK: [u8; 16] = [
    0xFF, 0x00, 0xFF, 0x00, 0xFF, 0x00, 0xFF, 0x00, 0xFF, 0x00, 0xFF, 0x00, 0xFF, 0x00, 0xFF, 0x00,
];

/// Builds the demo ROM: 32 KiB, no mapper, valid header checksum.
pub fn rom() -> Vec<u8> {
    let mut p = Program::new();

    p.emit(&[0xF3]); //                 DI
    p.label("wait_vblank");
    p.emit(&[0xF0, 0x44]); //           LDH A,(LY)
    p.emit(&[0xFE, 0x90]); //           CP 144
    p.jump(JR_NZ, "wait_vblank");
    p.emit(&[0xAF]); //                 XOR A
    p.emit(&[0xE0, 0x40]); //           LDH (LCDC),A      ; LCD off

    // Clear all of video memory.
    p.emit(&[0x21, 0x00, 0x80]); //     LD HL,0x8000
    p.emit(&[0x01, 0x00, 0x20]); //     LD BC,0x2000
    p.label("clear_vram");
    p.emit(&[0xAF]); //                 XOR A
    p.emit(&[0x22]); //                 LD (HL+),A
    p.emit(&[0x0B]); //                 DEC BC
    p.emit(&[0x78]); //                 LD A,B
    p.emit(&[0xB1]); //                 OR C
    p.jump(JR_NZ, "clear_vram");

    // Copy the two tiles (32 bytes) to tile 1 and tile 2.
    p.emit(&[0x21, 0x10, 0x80]); //     LD HL,0x8010
    let tiles_operand = p.bytes.len() + 1;
    p.emit(&[0x11, 0x00, 0x00]); //     LD DE,tiles
    p.emit(&[0x06, 0x20]); //           LD B,32
    p.label("copy_tiles");
    p.emit(&[0x1A]); //                 LD A,(DE)
    p.emit(&[0x13]); //                 INC DE
    p.emit(&[0x22]); //                 LD (HL+),A
    p.emit(&[0x05]); //                 DEC B
    p.jump(JR_NZ, "copy_tiles");

    // Checkerboard: tile 2 where (row + column) is odd, over the 32x32 map.
    p.emit(&[0x21, 0x00, 0x98]); //     LD HL,0x9800
    p.emit(&[0x0E, 0x00]); //           LD C,0            ; row
    p.label("board_row");
    p.emit(&[0x06, 0x00]); //           LD B,0            ; column
    p.label("board_cell");
    p.emit(&[0x78]); //                 LD A,B
    p.emit(&[0x81]); //                 ADD A,C
    p.emit(&[0xE6, 0x01]); //           AND 1
    p.emit(&[0x87]); //                 ADD A,A           ; 0 or 2
    p.emit(&[0x22]); //                 LD (HL+),A
    p.emit(&[0x04]); //                 INC B
    p.emit(&[0x78]); //                 LD A,B
    p.emit(&[0xFE, 0x20]); //           CP 32
    p.jump(JR_NZ, "board_cell");
    p.emit(&[0x0C]); //                 INC C
    p.emit(&[0x79]); //                 LD A,C
    p.emit(&[0xFE, 0x20]); //           CP 32
    p.jump(JR_NZ, "board_row");

    // Clear OAM so no stray sprites show up.
    p.emit(&[0x21, 0x00, 0xFE]); //     LD HL,0xFE00
    p.emit(&[0x06, 0xA0]); //           LD B,160
    p.emit(&[0xAF]); //                 XOR A
    p.label("clear_oam");
    p.emit(&[0x22]); //                 LD (HL+),A
    p.emit(&[0x05]); //                 DEC B
    p.jump(JR_NZ, "clear_oam");

    p.emit(&[0x16, START_X]); //        LD D,START_X
    p.emit(&[0x1E, START_Y]); //        LD E,START_Y
    p.emit(&[0x3E, 0xE4]); //           LD A,0xE4
    p.emit(&[0xE0, 0x48]); //           LDH (OBP0),A
    p.emit(&[0x3E, 0x93]); //           LD A,0x93
    p.emit(&[0xE0, 0x40]); //           LDH (LCDC),A      ; LCD, BG and sprites on

    // A chime on channel 2: a 1048 Hz square wave that fades out.
    p.emit(&[0x3E, 0x80]); //           LD A,0x80
    p.emit(&[0xE0, 0x26]); //           LDH (NR52),A      ; sound on
    p.emit(&[0x3E, 0x77]); //           LD A,0x77
    p.emit(&[0xE0, 0x24]); //           LDH (NR50),A      ; full volume
    p.emit(&[0x3E, 0x22]); //           LD A,0x22
    p.emit(&[0xE0, 0x25]); //           LDH (NR51),A      ; channel 2, both sides
    p.emit(&[0x3E, 0x80]); //           LD A,0x80
    p.emit(&[0xE0, 0x16]); //           LDH (NR21),A      ; 50% duty
    p.emit(&[0x3E, 0xF3]); //           LD A,0xF3
    p.emit(&[0xE0, 0x17]); //           LDH (NR22),A      ; loud, fading
    p.emit(&[0x3E, 0x83]); //           LD A,0x83
    p.emit(&[0xE0, 0x18]); //           LDH (NR23),A      ; frequency 0x783
    p.emit(&[0x3E, 0x87]); //           LD A,0x87
    p.emit(&[0xE0, 0x19]); //           LDH (NR24),A      ; play

    p.label("frame");
    p.label("wait_frame");
    p.emit(&[0xF0, 0x44]); //           LDH A,(LY)
    p.emit(&[0xFE, 0x90]); //           CP 144
    p.jump(JR_NZ, "wait_frame");

    // Read the direction pad into B: 1 = held.
    p.emit(&[0x3E, 0x20]); //           LD A,0x20
    p.emit(&[0xE0, 0x00]); //           LDH (P1),A
    p.emit(&[0xF0, 0x00]); //           LDH A,(P1)
    p.emit(&[0xF0, 0x00]); //           LDH A,(P1)        ; let the lines settle
    p.emit(&[0x2F]); //                 CPL
    p.emit(&[0x47]); //                 LD B,A
    p.emit(&[0xCB, 0x40]); //           BIT 0,B           ; Right
    p.jump(JR_Z, "not_right");
    p.emit(&[0x14]); //                 INC D
    p.label("not_right");
    p.emit(&[0xCB, 0x48]); //           BIT 1,B           ; Left
    p.jump(JR_Z, "not_left");
    p.emit(&[0x15]); //                 DEC D
    p.label("not_left");
    p.emit(&[0xCB, 0x50]); //           BIT 2,B           ; Up
    p.jump(JR_Z, "not_up");
    p.emit(&[0x1D]); //                 DEC E
    p.label("not_up");
    p.emit(&[0xCB, 0x58]); //           BIT 3,B           ; Down
    p.jump(JR_Z, "not_down");
    p.emit(&[0x1C]); //                 INC E
    p.label("not_down");

    // Read the action buttons: A held inverts the background palette.
    p.emit(&[0x3E, 0x10]); //           LD A,0x10
    p.emit(&[0xE0, 0x00]); //           LDH (P1),A
    p.emit(&[0xF0, 0x00]); //           LDH A,(P1)
    p.emit(&[0xF0, 0x00]); //           LDH A,(P1)
    p.emit(&[0x0E, 0xE4]); //           LD C,0xE4
    p.emit(&[0xCB, 0x47]); //           BIT 0,A           ; A (0 = held)
    p.jump(JR_NZ, "not_a");
    p.emit(&[0x0E, 0x1B]); //           LD C,0x1B
    p.label("not_a");
    p.emit(&[0x79]); //                 LD A,C
    p.emit(&[0xE0, 0x47]); //           LDH (BGP),A
    p.emit(&[0x3E, 0x30]); //           LD A,0x30
    p.emit(&[0xE0, 0x00]); //           LDH (P1),A        ; deselect both groups

    // Sprite 0: Y, X, tile 1, no attributes.
    p.emit(&[0x21, 0x00, 0xFE]); //     LD HL,0xFE00
    p.emit(&[0x7B]); //                 LD A,E
    p.emit(&[0x22]); //                 LD (HL+),A
    p.emit(&[0x7A]); //                 LD A,D
    p.emit(&[0x22]); //                 LD (HL+),A
    p.emit(&[0x3E, 0x01]); //           LD A,1
    p.emit(&[0x22]); //                 LD (HL+),A
    p.emit(&[0xAF]); //                 XOR A
    p.emit(&[0x77]); //                 LD (HL),A

    // Leave line 144 before looking for the next frame.
    p.label("leave_vblank_line");
    p.emit(&[0xF0, 0x44]); //           LDH A,(LY)
    p.emit(&[0xFE, 0x90]); //           CP 144
    p.jump(JR_Z, "leave_vblank_line");
    p.jump(JR, "frame");

    p.label("tiles");
    p.emit(&BALL);
    p.emit(&BLOCK);

    let tiles = p.address("tiles").to_le_bytes();
    p.bytes[tiles_operand..tiles_operand + 2].copy_from_slice(&tiles);
    let code = p.finish();

    let mut rom = vec![0u8; 0x8000];
    // Entry point: NOP ; JP 0x0150
    rom[0x100..0x104].copy_from_slice(&[0x00, 0xC3, 0x50, 0x01]);
    rom[0x134..0x141].copy_from_slice(b"GLASSBIT DEMO");
    rom[ENTRY as usize..ENTRY as usize + code.len()].copy_from_slice(&code);
    // Header checksum over 0x134..=0x14C.
    rom[0x14D] = rom[0x134..0x14D]
        .iter()
        .fold(0u8, |sum, &b| sum.wrapping_sub(b).wrapping_sub(1));
    rom
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Button, GameBoy};

    fn demo() -> GameBoy {
        let mut gb = GameBoy::new(&rom()).unwrap_or_else(|e| panic!("{e}"));
        // Setup clears 8 KiB of VRAM with the LCD off; give it time.
        for _ in 0..12 {
            gb.run_frame();
        }
        gb
    }

    fn sprite_position(gb: &GameBoy) -> (u8, u8) {
        (gb.peek(0xFE01), gb.peek(0xFE00))
    }

    #[test]
    fn has_a_valid_header() {
        let gb = demo();
        assert_eq!(gb.header().title, "GLASSBIT DEMO");
        assert!(gb.header().header_checksum_ok());
    }

    #[test]
    fn draws_the_sprite_over_a_checkerboard() {
        let gb = demo();
        assert_eq!(sprite_position(&gb), (START_X, START_Y));
        let shades: std::collections::BTreeSet<u8> = gb.framebuffer().iter().copied().collect();
        assert_eq!(shades.into_iter().collect::<Vec<_>>(), vec![0, 1, 3]);
    }

    #[test]
    fn plays_a_chime_that_fades_to_silence() {
        let mut gb = GameBoy::new(&rom()).unwrap_or_else(|e| panic!("{e}"));
        let mut loudest_per_frame = Vec::new();
        for _ in 0..80 {
            gb.run_frame();
            let peak = gb
                .audio_samples()
                .iter()
                .fold(0.0f32, |max, &s| max.max(s.abs()));
            loudest_per_frame.push(peak);
        }
        let loudest = loudest_per_frame.iter().cloned().fold(0.0, f32::max);
        assert!(loudest > 0.1, "peak {loudest}");
        assert!(loudest_per_frame[79] < 0.01, "silent by the end");
    }

    #[test]
    fn direction_pad_moves_the_sprite_one_pixel_per_frame() {
        let mut gb = demo();
        gb.set_button(Button::Right, true);
        gb.set_button(Button::Up, true);
        for _ in 0..10 {
            gb.run_frame();
        }
        gb.set_button(Button::Right, false);
        gb.set_button(Button::Up, false);
        gb.run_frame();
        assert_eq!(sprite_position(&gb), (START_X + 10, START_Y - 10));

        gb.set_button(Button::Left, true);
        gb.set_button(Button::Down, true);
        for _ in 0..4 {
            gb.run_frame();
        }
        assert_eq!(sprite_position(&gb), (START_X + 6, START_Y - 6));
    }

    #[test]
    fn holding_a_inverts_the_background() {
        let mut gb = demo();
        assert_eq!(gb.peek(0xFF47), 0xE4);
        gb.set_button(Button::A, true);
        gb.run_frame();
        gb.run_frame();
        assert_eq!(gb.peek(0xFF47), 0x1B);
    }
}
