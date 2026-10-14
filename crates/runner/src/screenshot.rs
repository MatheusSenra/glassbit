//! Saving the screen as a PNG and loading reference images to compare with.
//!
//! Screens are handled as 8-bit RGB, as `GameBoy::rgb_framebuffer` returns
//! them. On a DMG the four shades map to evenly spaced grays, and Game Boy
//! Color channels are widened from 5 to 8 bits: both match the reference
//! images of the test ROM collection.

use std::error::Error;
use std::fs::File;
use std::io::{BufReader, BufWriter};
use std::path::Path;

use glassbit_core::{SCREEN_HEIGHT, SCREEN_WIDTH};

/// One pixel: red, green, blue.
pub type Rgb = [u8; 3];

/// Writes a screen of RGB pixels.
pub fn save(screen: &[Rgb], path: &Path) -> Result<(), Box<dyn Error>> {
    let mut encoder = png::Encoder::new(
        BufWriter::new(File::create(path)?),
        SCREEN_WIDTH as u32,
        SCREEN_HEIGHT as u32,
    );
    encoder.set_color(png::ColorType::Rgb);
    encoder.set_depth(png::BitDepth::Eight);
    encoder
        .write_header()?
        .write_image_data(screen.as_flattened())?;
    Ok(())
}

/// Reads a 160x144 PNG (gray or color, any bit depth) as RGB pixels.
pub fn load(path: &Path) -> Result<Vec<Rgb>, Box<dyn Error>> {
    let mut decoder = png::Decoder::new(BufReader::new(File::open(path)?));
    // Expands palettes and low bit depths to 8 bits per channel.
    decoder.set_transformations(png::Transformations::EXPAND);
    let mut reader = decoder.read_info()?;
    let mut pixels = vec![0; reader.output_buffer_size()];
    let info = reader.next_frame(&mut pixels)?;
    if (info.width as usize, info.height as usize) != (SCREEN_WIDTH, SCREEN_HEIGHT) {
        return Err(format!(
            "{} is {}x{}, expected {SCREEN_WIDTH}x{SCREEN_HEIGHT}",
            path.display(),
            info.width,
            info.height
        )
        .into());
    }

    let channels = info.color_type.samples();
    Ok(pixels[..info.buffer_size()]
        .chunks_exact(channels)
        .map(|pixel| match channels {
            // Gray, possibly with alpha.
            1 | 2 => [pixel[0]; 3],
            // RGB, possibly with alpha.
            _ => [pixel[0], pixel[1], pixel[2]],
        })
        .collect())
}

/// Compares two screens. Returns the number of differing pixels and the
/// coordinates of the first one.
pub fn compare(actual: &[Rgb], expected: &[Rgb]) -> (usize, Option<(usize, usize)>) {
    let mut differing = actual
        .iter()
        .zip(expected)
        .enumerate()
        .filter(|(_, (a, b))| a != b)
        .map(|(index, _)| (index % SCREEN_WIDTH, index / SCREEN_WIDTH));
    let first = differing.next();
    (first.is_some() as usize + differing.count(), first)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn png_round_trip_preserves_every_color() {
        let screen: Vec<Rgb> = (0..SCREEN_WIDTH * SCREEN_HEIGHT)
            .map(|i| [i as u8, (i / 7) as u8, (i % 251) as u8])
            .collect();
        let path = std::env::temp_dir().join(format!(
            "glassbit-screenshot-test-{}.png",
            std::process::id()
        ));
        save(&screen, &path).unwrap();
        let loaded = load(&path).unwrap();
        std::fs::remove_file(&path).unwrap();
        assert_eq!(loaded, screen);
    }

    #[test]
    fn compare_counts_differences_and_finds_the_first() {
        let a = vec![[0u8; 3]; SCREEN_WIDTH * SCREEN_HEIGHT];
        let mut b = a.clone();
        assert_eq!(compare(&a, &b), (0, None));
        b[SCREEN_WIDTH * 2 + 5] = [0, 0, 1];
        b[SCREEN_WIDTH * 9] = [9, 9, 9];
        assert_eq!(compare(&a, &b), (2, Some((5, 2))));
    }
}
