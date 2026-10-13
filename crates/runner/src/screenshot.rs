//! Saving the screen as a PNG and loading reference images to compare with.
//!
//! Images are 160x144 grayscale. The four DMG shades map to evenly spaced
//! grays, which is what the reference images of the test ROM collection use.

use std::error::Error;
use std::fs::File;
use std::io::{BufReader, BufWriter};
use std::path::Path;

use glassbit_core::{SCREEN_HEIGHT, SCREEN_WIDTH};

/// Gray level of each shade, from 0 (lightest) to 3 (darkest).
const GRAYS: [u8; 4] = [0xFF, 0xAA, 0x55, 0x00];

/// Writes a framebuffer of shades (as returned by `GameBoy::framebuffer`).
pub fn save(framebuffer: &[u8], path: &Path) -> Result<(), Box<dyn Error>> {
    let mut encoder = png::Encoder::new(
        BufWriter::new(File::create(path)?),
        SCREEN_WIDTH as u32,
        SCREEN_HEIGHT as u32,
    );
    encoder.set_color(png::ColorType::Grayscale);
    encoder.set_depth(png::BitDepth::Eight);
    let pixels: Vec<u8> = framebuffer
        .iter()
        .map(|&shade| GRAYS[shade as usize])
        .collect();
    encoder.write_header()?.write_image_data(&pixels)?;
    Ok(())
}

/// Reads a 160x144 PNG and converts it back to shades.
pub fn load(path: &Path) -> Result<Vec<u8>, Box<dyn Error>> {
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

    // The first channel is enough: the images are gray.
    let channels = info.color_type.samples();
    Ok(pixels[..info.buffer_size()]
        .chunks_exact(channels)
        .map(|pixel| nearest_shade(pixel[0]))
        .collect())
}

fn nearest_shade(gray: u8) -> u8 {
    (0..4u8)
        .min_by_key(|&shade| GRAYS[shade as usize].abs_diff(gray))
        .expect("four shades")
}

/// Compares two framebuffers. Returns the number of differing pixels and the
/// coordinates of the first one.
pub fn compare(actual: &[u8], expected: &[u8]) -> (usize, Option<(usize, usize)>) {
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
    fn png_round_trip_preserves_every_shade() {
        let framebuffer: Vec<u8> = (0..SCREEN_WIDTH * SCREEN_HEIGHT)
            .map(|i| (i % 4) as u8)
            .collect();
        let path = std::env::temp_dir().join(format!(
            "glassbit-screenshot-test-{}.png",
            std::process::id()
        ));
        save(&framebuffer, &path).unwrap();
        let loaded = load(&path).unwrap();
        std::fs::remove_file(&path).unwrap();
        assert_eq!(loaded, framebuffer);
    }

    #[test]
    fn compare_counts_differences_and_finds_the_first() {
        let a = vec![0u8; SCREEN_WIDTH * SCREEN_HEIGHT];
        let mut b = a.clone();
        assert_eq!(compare(&a, &b), (0, None));
        b[SCREEN_WIDTH * 2 + 5] = 3;
        b[SCREEN_WIDTH * 9] = 1;
        assert_eq!(compare(&a, &b), (2, Some((5, 2))));
    }
}
