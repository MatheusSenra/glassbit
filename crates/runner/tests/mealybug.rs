//! Mealybug Tearoom tests: each one changes a PPU register in the middle of
//! mode 3, on many lines and at many points of the line, so the picture
//! shows exactly which pixel every change landed on. The screen must match
//! a photo-verified reference for the original DMG.
//!
//! Only the tests that already match are listed. Most of the others draw
//! the registered-trademark sprite that the boot ROM leaves in video memory,
//! which Glassbit does not run, or depend on quirks of the window being
//! switched off and on again in the middle of a line.
//!
//! <https://github.com/mattcurrie/mealybug-tearoom-tests>

use glassbit_core::Model;
use glassbit_runner::{assert_screen_matches, assert_screen_matches_on};

fn matches(name: &str) {
    assert_screen_matches(
        &format!("mealybug-tearoom-tests/ppu/{name}.gb"),
        &format!("mealybug-tearoom-tests/ppu/{name}_dmg_blob.png"),
        120,
    );
}

/// The window switched off and on during mode 2.
#[test]
fn m2_win_en_toggle() {
    matches("m2_win_en_toggle");
}

/// BGP rewritten mid-line, including the DMG quirk where the PPU sees the
/// old and new values ORed together for one dot.
#[test]
fn m3_bgp_change() {
    matches("m3_bgp_change");
}

/// The coarse part of SCX is read at every tile fetch.
#[test]
fn m3_scx_high_5_bits() {
    matches("m3_scx_high_5_bits");
}

/// WX changed while the first tile is being fetched, and again later.
#[test]
fn m3_wx_6_change() {
    matches("m3_wx_6_change");
}

/// The same test on a Game Boy Color, which runs it in compatibility mode:
/// its reference shows the colors the Color gives DMG games by default.
#[test]
fn m2_win_en_toggle_in_compatibility_mode() {
    assert_screen_matches_on(
        Some(Model::Cgb),
        "mealybug-tearoom-tests/ppu/m2_win_en_toggle.gb",
        "mealybug-tearoom-tests/ppu/m2_win_en_toggle_cgb_c.png",
        120,
    );
}
