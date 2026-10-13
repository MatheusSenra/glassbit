//! The joypad: eight buttons read through a 2x4 matrix.
//!
//! The P1 register has only four input lines for eight buttons. The program
//! picks a group by pulling one of two select lines low (bit 4 for the
//! direction pad, bit 5 for the action buttons) and then reads the four
//! lines in the low nibble, where **0 means pressed**.
//!
//! | Line  | Bit 4 low (directions) | Bit 5 low (actions) |
//! | ----- | ---------------------- | ------------------- |
//! | Bit 0 | Right                  | A                   |
//! | Bit 1 | Left                   | B                   |
//! | Bit 2 | Up                     | Select              |
//! | Bit 3 | Down                   | Start               |
//!
//! Reference: <https://gbdev.io/pandocs/Joypad_Input.html>

/// The joypad register.
pub const P1: u16 = 0xFF00;

const SELECT_DIRECTIONS: u8 = 0x10;
const SELECT_ACTIONS: u8 = 0x20;

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
    /// All buttons, in the order of their numeric values.
    pub const ALL: [Button; 8] = [
        Button::Right,
        Button::Left,
        Button::Up,
        Button::Down,
        Button::A,
        Button::B,
        Button::Select,
        Button::Start,
    ];

    fn mask(self) -> u8 {
        1 << self as u8
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Joypad {
    /// Bit set = held. Low nibble: directions; high nibble: actions.
    held: u8,
    /// The two select bits as last written (bits 4 and 5 of P1).
    select: u8,
}

impl Joypad {
    /// A joypad in the post-boot state: nothing held, both groups selected.
    pub fn new() -> Self {
        Self { held: 0, select: 0 }
    }

    /// Updates a button. Returns `true` when the joypad interrupt fires,
    /// which happens when an input line of a selected group goes low.
    pub fn set_button(&mut self, button: Button, pressed: bool) -> bool {
        let before = self.lines();
        if pressed {
            self.held |= button.mask();
        } else {
            self.held &= !button.mask();
        }
        before & !self.lines() != 0
    }

    pub fn is_pressed(&self, button: Button) -> bool {
        self.held & button.mask() != 0
    }

    pub fn read(&self) -> u8 {
        // Bits 6-7 do not exist and read as 1.
        0xC0 | self.select | self.lines()
    }

    /// Writes P1; only the select bits are writable. Returns `true` when the
    /// joypad interrupt fires (selecting a group with a button already held).
    pub fn write(&mut self, value: u8) -> bool {
        let before = self.lines();
        self.select = value & (SELECT_DIRECTIONS | SELECT_ACTIONS);
        before & !self.lines() != 0
    }

    /// The four input lines: 1 = released, 0 = pressed. With both groups
    /// selected a line goes low if either of its two buttons is held.
    fn lines(&self) -> u8 {
        let mut pressed = 0;
        if self.select & SELECT_DIRECTIONS == 0 {
            pressed |= self.held & 0x0F;
        }
        if self.select & SELECT_ACTIONS == 0 {
            pressed |= self.held >> 4;
        }
        !pressed & 0x0F
    }
}

impl Default for Joypad {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn starts_like_post_boot() {
        assert_eq!(Joypad::new().read(), 0xCF);
    }

    #[test]
    fn buttons_show_up_only_in_their_selected_group() {
        let mut joypad = Joypad::new();
        joypad.set_button(Button::Left, true);
        joypad.set_button(Button::Start, true);
        joypad.write(0x30);
        assert_eq!(joypad.read() & 0x0F, 0x0F, "nothing selected");

        joypad.write(0x20);
        assert_eq!(joypad.read(), 0xC0 | 0x20 | 0b1101, "directions: Left");
        joypad.write(0x10);
        assert_eq!(joypad.read(), 0xC0 | 0x10 | 0b0111, "actions: Start");
        joypad.write(0x00);
        assert_eq!(joypad.read() & 0x0F, 0b0101, "both groups merge");
    }

    #[test]
    fn each_button_maps_to_its_line() {
        let lines = [0b1110, 0b1101, 0b1011, 0b0111];
        for (index, button) in Button::ALL.into_iter().enumerate() {
            let mut joypad = Joypad::new();
            joypad.write(if index < 4 { 0x20 } else { 0x10 });
            joypad.set_button(button, true);
            assert_eq!(joypad.read() & 0x0F, lines[index % 4], "{button:?}");
            assert!(joypad.is_pressed(button));
        }
    }

    #[test]
    fn interrupt_fires_when_a_selected_line_goes_low() {
        let mut joypad = Joypad::new();
        joypad.write(0x30);
        assert!(!joypad.set_button(Button::A, true), "group not selected");
        assert!(joypad.write(0x10), "selecting it pulls the line low");
        assert!(!joypad.set_button(Button::A, true), "already low");
        assert!(!joypad.set_button(Button::A, false), "releasing is silent");
        assert!(joypad.set_button(Button::B, true));
    }
}
