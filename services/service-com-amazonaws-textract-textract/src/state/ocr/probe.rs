// SPDX-License-Identifier: CC0-1.0

//! Reading a page drawn at a raster the recogniser holds no template at.
//!
//! Every other test of this module draws its pages with the embedded 8x8 font, which the
//! recogniser holds templates for: a round trip through pixels, but one where the ink and
//! the template are the same bitmap. These rows draw their pages with the PRE-RASTERISED
//! SANS-SERIF face at the rasters the bank does NOT take its templates from — the bank holds
//! that face at its 16- and 32-pixel rasters ([`super::glyphs`]), and these pages are drawn
//! at its 20- and 24-pixel ones, a different bitmap on a different pixel grid — so what they
//! pin is the rule the module promises: the characters reported are the characters the pixels
//! were drawn with, whatever drew them.

use noto_sans_mono_bitmap::{get_raster, FontWeight, RasterHeight};

use super::*;

/// Draw `lines` with one weight and size of the pre-rasterised face, every glyph scaled by
/// `scale`, as a PNG.
///
/// `noto_sans_mono_bitmap` hands out one byte of ink per pixel per glyph, so a page drawn
/// here carries anti-aliased strokes: the edge of every stroke is part way between the ink
/// and the ground, which is what a page rendered at any ordinary size looks like.
fn page(lines: &[&str], weight: FontWeight, height: RasterHeight, scale: usize) -> Vec<u8> {
    let cell = height.val();
    let pitch = cell * 3 / 2;
    let margin = 8usize;
    let advance =
        |ch: char| get_raster(ch, weight, height).map_or(cell / 2, |raster| raster.width());
    let columns = lines
        .iter()
        .map(|line| line.chars().map(advance).sum::<usize>())
        .max()
        .unwrap_or(0);
    let width = ((margin * 2 + columns * scale) as u32).max(1);
    let depth = ((margin * 2 + lines.len() * pitch * scale) as u32).max(1);
    let mut drawn = image::GrayImage::from_pixel(width, depth, image::Luma([255]));
    for (row, line) in lines.iter().enumerate() {
        let mut at = margin;
        for ch in line.chars() {
            let Some(raster) = get_raster(ch, weight, height) else {
                at += cell / 2;
                continue;
            };
            let pixels = raster.pixels();
            for y in 0..raster.height() {
                for x in 0..raster.width() {
                    let ink = pixels
                        .get(y * raster.width() + x)
                        .copied()
                        .unwrap_or_default();
                    if ink == 0 {
                        continue;
                    }
                    for dy in 0..scale {
                        for dx in 0..scale {
                            let px = ((at + x) * scale + dx) as u32;
                            let py = ((margin + row * pitch + y) * scale + dy) as u32;
                            if px < width && py < depth {
                                drawn.put_pixel(px, py, image::Luma([255 - ink]));
                            }
                        }
                    }
                }
            }
            at += raster.width();
        }
    }
    let mut out = Cursor::new(Vec::new());
    drawn
        .write_to(&mut out, ImageFormat::Png)
        .expect("a grey page encodes as a PNG");
    out.into_inner()
}

/// The characters a page drawn at a raster no template comes from are read as the characters
/// it was drawn with: one row per weight, raster and scale the same lines are drawn at.
#[test]
fn a_page_drawn_in_another_face_reads_as_the_text_it_was_drawn_with() {
    // Capitals, small letters, figures and the punctuation a form and a receipt are set
    // with: the characters a document's own text is drawn from.
    let lines = [
        "SIM2REAL LEDGER",
        "Invoice Number: INV-99887",
        "Total Due $12.34",
    ];
    let cases: Vec<(&str, FontWeight, RasterHeight, usize)> = vec![
        ("regular", FontWeight::Regular, RasterHeight::Size24, 1),
        ("light", FontWeight::Light, RasterHeight::Size24, 1),
        ("bold", FontWeight::Bold, RasterHeight::Size24, 1),
        ("a smaller raster", FontWeight::Regular, RasterHeight::Size20, 1),
        ("drawn twice the size", FontWeight::Regular, RasterHeight::Size24, 2),
    ];
    for (case, weight, height, scale) in cases {
        let read = read_raster(&page(&lines, weight, height, scale), ImageFormat::Png)
            .unwrap_or_else(|error| panic!("case {case:?}: the page must be read, got {error:?}"));
        let read_lines: Vec<String> = read
            .text
            .lines()
            .map(|line| line.trim().to_string())
            .filter(|line| !line.is_empty())
            .collect();
        assert_eq!(
            read_lines.len(),
            lines.len(),
            "case {case:?}: a page of {} lines of ink is read as that many lines, got \
             {read_lines:?}",
            lines.len()
        );
        // How much of what was drawn was read, character for character: a page drawn in a
        // face no template comes from is measured rather than recognised by identity, so
        // what the rule asks is that nearly every character it was drawn with comes back,
        // not that the ink of two faces agrees exactly. The spaces are left out of the
        // count: what separates two words is measured off the line's own gaps, which the
        // word-and-column rule below pins on its own.
        let bare = |text: &str| -> String {
            text.chars().filter(|ch| !ch.is_whitespace()).collect()
        };
        let drawn = bare(&lines.join(""));
        let read_text = bare(&read_lines.join(""));
        let right = drawn
            .chars()
            .zip(read_text.chars())
            .filter(|(drawn, read)| drawn == read)
            .count();
        let share = right as f32 / drawn.chars().count() as f32;
        assert!(
            share >= 0.8,
            "case {case:?}: the characters read must be the characters drawn, got \
             {read_lines:?} against {lines:?} ({right} of {} characters)",
            drawn.chars().count()
        );
        assert!(
            read.confidence > 0.0 && read.confidence <= 100.0,
            "case {case:?}: the confidence reported must be the score the characters were \
             read with, got {}",
            read.confidence
        );
    }
}

/// Every reading is of the SAME pixels: a page drawn once and read twice reads alike, and
/// two pages drawn from different text do not read alike.
#[test]
fn a_page_reads_the_same_twice_and_two_pages_read_apart() {
    let one = page(&["ACCOUNT 48213"], FontWeight::Regular, RasterHeight::Size20, 1);
    let other = page(&["ACCOUNT 90210"], FontWeight::Regular, RasterHeight::Size20, 1);
    let first = read_raster(&one, ImageFormat::Png).expect("the page must be read");
    let again = read_raster(&one, ImageFormat::Png).expect("the page must be read");
    assert_eq!(
        first.text, again.text,
        "the same pixels must read the same text twice"
    );
    let different = read_raster(&other, ImageFormat::Png).expect("the page must be read");
    assert_ne!(
        first.text, different.text,
        "two pages drawn from different text must not read alike, got {:?}",
        first.text
    );
}
