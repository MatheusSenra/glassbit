//! Recording gameplay to an animated GIF, with scripted button presses.
//!
//! This is how the animation in the README is made, and it doubles as a way
//! to drive a game without a person at the keyboard.

use std::error::Error;
use std::fs::File;
use std::io::BufWriter;
use std::path::Path;

use glassbit_core::{Button, GameBoy, Model, SCREEN_HEIGHT, SCREEN_WIDTH};

/// The original LCD's green tint, lightest shade first.
const PALETTE: [u8; 12] = [
    0xE0, 0xF8, 0xD0, 0x88, 0xC0, 0x70, 0x34, 0x68, 0x56, 0x08, 0x18, 0x20,
];
/// Each Game Boy pixel becomes a square of this many GIF pixels.
const SCALE: usize = 2;
/// One GIF frame per this many emulated frames...
const FRAME_SKIP: u32 = 3;
/// ...shown for this many hundredths of a second: 3 frames at 59.73 fps
/// last 50.2 ms.
const FRAME_DELAY: u16 = 5;

/// A button held from frame `from` up to, but not including, frame `until`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Press {
    pub button: Button,
    pub from: u32,
    pub until: u32,
}

/// Parses a script such as `"start@70-75,left@140-146"`.
pub fn parse_presses(script: &str) -> Result<Vec<Press>, String> {
    script
        .split(',')
        .filter(|entry| !entry.trim().is_empty())
        .map(|entry| {
            let entry = entry.trim();
            let malformed = || format!("expected button@from-until, got {entry:?}");
            let (name, range) = entry.split_once('@').ok_or_else(malformed)?;
            let (from, until) = range.split_once('-').ok_or_else(malformed)?;
            let button = match name.to_ascii_lowercase().as_str() {
                "right" => Button::Right,
                "left" => Button::Left,
                "up" => Button::Up,
                "down" => Button::Down,
                "a" => Button::A,
                "b" => Button::B,
                "select" => Button::Select,
                "start" => Button::Start,
                _ => return Err(format!("unknown button {name:?}")),
            };
            let from: u32 = from.parse().map_err(|_| malformed())?;
            let until: u32 = until.parse().map_err(|_| malformed())?;
            if until <= from {
                return Err(format!("{entry:?} ends before it starts"));
            }
            Ok(Press {
                button,
                from,
                until,
            })
        })
        .collect()
}

/// Enlarges a screen by [`SCALE`], given as rows of `SCREEN_WIDTH` items of
/// `bytes_per_pixel` bytes each.
fn scale_bytes(pixels: &[u8], bytes_per_pixel: usize) -> Vec<u8> {
    let mut scaled = Vec::with_capacity(pixels.len() * SCALE * SCALE);
    for row in pixels.chunks_exact(SCREEN_WIDTH * bytes_per_pixel) {
        let wide: Vec<u8> = row
            .chunks_exact(bytes_per_pixel)
            .flat_map(|pixel| std::iter::repeat(pixel).take(SCALE).flatten().copied())
            .collect();
        for _ in 0..SCALE {
            scaled.extend_from_slice(&wide);
        }
    }
    scaled
}

/// Enlarges a screen of one-byte pixels, or of RGB pixels flattened to
/// bytes, by [`SCALE`].
fn scale(pixels: &[u8]) -> Vec<u8> {
    let bytes_per_pixel = pixels.len() / (SCREEN_WIDTH * SCREEN_HEIGHT);
    scale_bytes(pixels, bytes_per_pixel)
}

/// Lets `[[u8; 3]]` be passed where bytes are expected.
trait FlattenRgb {
    fn as_flattened_rgb(&self) -> &[u8];
}

impl FlattenRgb for Vec<[u8; 3]> {
    fn as_flattened_rgb(&self) -> &[u8] {
        self.as_flattened()
    }
}

/// Runs `gb` for `frames` frames, applying `presses`, and writes what the
/// screen showed to an animated GIF at `path`.
pub fn record_gif(
    gb: &mut GameBoy,
    frames: u32,
    presses: &[Press],
    path: &Path,
) -> Result<(), Box<dyn Error>> {
    let width = (SCREEN_WIDTH * SCALE) as u16;
    let height = (SCREEN_HEIGHT * SCALE) as u16;
    let mut encoder =
        gif::Encoder::new(BufWriter::new(File::create(path)?), width, height, &PALETTE)?;
    encoder.set_repeat(gif::Repeat::Infinite)?;

    for frame in 0..frames {
        for press in presses {
            if frame == press.from {
                gb.set_button(press.button, true);
            } else if frame == press.until {
                gb.set_button(press.button, false);
            }
        }
        gb.run_frame();
        if frame % FRAME_SKIP != 0 {
            continue;
        }

        let mut image = if gb.model() == Model::Dmg {
            // The framebuffer already holds palette indices 0-3; only
            // scaling is needed.
            gif::Frame::from_indexed_pixels(width, height, scale(gb.framebuffer()), None)
        } else {
            // Color frames get their own palette, chosen per frame.
            let rgb = scale(gb.rgb_framebuffer().as_flattened_rgb());
            gif::Frame::from_rgb_speed(width, height, &rgb, 10)
        };
        image.delay = FRAME_DELAY;
        encoder.write_frame(&image)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_a_script_of_presses() {
        assert_eq!(
            parse_presses("start@70-75, Left@140-146"),
            Ok(vec![
                Press {
                    button: Button::Start,
                    from: 70,
                    until: 75
                },
                Press {
                    button: Button::Left,
                    from: 140,
                    until: 146
                },
            ])
        );
        assert_eq!(parse_presses(""), Ok(vec![]));
    }

    #[test]
    fn rejects_malformed_scripts() {
        assert!(parse_presses("jump@1-2").is_err());
        assert!(parse_presses("a@5").is_err());
        assert!(parse_presses("a@x-9").is_err());
        assert!(parse_presses("a@9-9").is_err());
    }

    #[test]
    fn records_the_demo_and_the_presses_move_the_sprite() {
        let mut gb = GameBoy::new(&glassbit_core::demo::rom()).unwrap();
        let path = std::env::temp_dir().join(format!(
            "glassbit-recording-test-{}.gif",
            std::process::id()
        ));
        let presses = parse_presses("right@12-22").unwrap();
        record_gif(&mut gb, 30, &presses, &path).unwrap();

        let bytes = std::fs::read(&path).unwrap();
        std::fs::remove_file(&path).unwrap();
        assert_eq!(&bytes[..6], b"GIF89a");
        // Ten frames held: the sprite's X in OAM moved ten pixels.
        assert_eq!(gb.peek(0xFE01), glassbit_core::demo::START_X + 10);
    }
}
