//! dmg-acid2: a single picture that exercises most PPU features at once
//! (window, sprite priority and flipping, 8x16 sprites, the ten sprite limit,
//! tile addressing modes, palettes). Any mistake distorts the face, so the
//! screen must match the reference image pixel for pixel.
//!
//! <https://github.com/mattcurrie/dmg-acid2>

use glassbit_runner::assert_screen_matches;

/// cgb-acid2 does the same for the Game Boy Color: tile attributes, both
/// VRAM banks, color palettes and the Color's sprite priority rules.
///
/// <https://github.com/mattcurrie/cgb-acid2>
#[test]
fn cgb_screen_matches_the_reference() {
    assert_screen_matches("cgb-acid2/cgb-acid2.gbc", "cgb-acid2/cgb-acid2.png", 120);
}

#[test]
fn screen_matches_the_reference() {
    assert_screen_matches("dmg-acid2/dmg-acid2.gb", "dmg-acid2/dmg-acid2-dmg.png", 120);
}
