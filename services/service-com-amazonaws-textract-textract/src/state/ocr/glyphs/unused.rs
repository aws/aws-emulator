// SPDX-License-Identifier: CC0-1.0

//! What the comparison saw: the ranking [`super::recognise`] takes its answer from, over
//! the WHOLE bank rather than the shortlist it narrows to, and the test that holds the two
//! to each other.
//!
//! The ladder in [`super::recognise`] exists to keep the work per mark a small multiple of
//! the bank's size; what it must not do is lose the answer a comparison against every
//! character would have given. That is what the test here pins.

use super::*;

/// The characters whose faces agree most with the mark `glyph`, best first: the same
/// measure [`recognise`] decides by, taken over every character the bank holds.
pub(super) fn ranked(ink: &Ink, glyph: &GlyphBox, line: Line) -> Vec<(char, f32)> {
    let page = |x: isize, y: isize| ink.coverage(x, y);
    let mark = Mark::measure(&page, (glyph.x0, glyph.x1, glyph.y0, glyph.y1), line);
    let mut scored: Vec<(char, f32)> = bank()
        .iter()
        .map(|candidate| {
            (
                candidate.ch,
                candidate
                    .faces
                    .iter()
                    .map(|face| mark.agreement(face))
                    .fold(0.0f32, f32::max),
            )
        })
        .collect();
    scored.sort_by(|one, other| other.1.total_cmp(&one.1));
    scored
}

/// The mark one glyph of the embedded 8x8 font draws, as a page of one character carries
/// it: the ink of that glyph, scaled, on its own line.
fn drawn(ch: char, scale: usize) -> (Ink, Line, GlyphBox) {
    let rows = font8x8::legacy::BASIC_LEGACY[ch as usize];
    let side = FONT_CELL * scale;
    let margin = 4usize;
    let width = side + margin * 2;
    let height = side + margin * 2;
    let mut page = image::GrayImage::from_pixel(width as u32, height as u32, image::Luma([255]));
    for (y, bits) in rows.iter().enumerate() {
        for x in 0..FONT_CELL {
            if bits & (1 << x) == 0 {
                continue;
            }
            for dy in 0..scale {
                for dx in 0..scale {
                    page.put_pixel(
                        (margin + x * scale + dx) as u32,
                        (margin + y * scale + dy) as u32,
                        image::Luma([0]),
                    );
                }
            }
        }
    }
    let ink = Ink::of(&page);
    let marks = ink.text_lines().expect("one line of ink");
    let band = marks.into_iter().next().expect("one line of ink");
    let line = line_of(&band.glyphs, band.top, band.bottom);
    let glyph = band.glyphs.into_iter().next().expect("one mark");
    (ink, line, glyph)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The answer the LADDER gives is the answer the whole bank would have given: for every
    /// letter and digit of the embedded font, drawn at several sizes, the character
    /// [`recognise`] names is the one whose faces agree with the mark most over the whole
    /// bank — so narrowing the comparison to a shortlist does not lose the answer.
    #[test]
    fn narrowing_the_comparison_keeps_the_answer_the_whole_bank_gives() {
        for scale in [1usize, 2, 3] {
            for ch in ('A'..='Z').chain('0'..='9') {
                let (ink, line, glyph) = drawn(ch, scale);
                let over_all = ranked(&ink, &glyph, line);
                let (best, score) = over_all
                    .first()
                    .copied()
                    .expect("the bank names something for a mark of ink");
                let named = recognise(&ink, &glyph, line).unwrap_or_else(|| {
                    panic!("case {ch:?} at scale {scale}: a glyph of the font must be named")
                });
                // The ladder may answer with ANOTHER character only where that character's
                // faces agree with the mark exactly as well, which is what a tie is.
                let tied = (score - named.score).abs() <= f32::EPSILON;
                assert!(
                    named.ch == best || tied,
                    "case {ch:?} at scale {scale}: the comparison named {:?} ({:.3}) where \
                     the whole bank names {best:?} ({score:.3})",
                    named.ch,
                    named.score
                );
            }
        }
    }
}
