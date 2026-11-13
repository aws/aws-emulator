// SPDX-License-Identifier: CC0-1.0

//! The lettering a graph's axes are labelled with.
//!
//! A reader of a rendered graph reads the series' VALUES and the window's instants off the
//! axes, so those numbers have to be in the image — a plot normalised to the data it drew says
//! which shape the series took but not which values, and two stores whose values differ would
//! otherwise be the same picture. The glyphs are the maintained 8x8 bitmap font's; what this
//! module owns is writing one into the bitmap the plot was drawn into.
//!
//! The writing is deliberately the LAST thing done to a bitmap: the plotting library borrows
//! the buffer while it draws, so the labels are written straight into the pixels once it has
//! handed the buffer back, which is also what puts them over the grid rather than under it.

use ::font8x8::legacy::BASIC_LEGACY;

/// How wide one glyph's cell is, in pixels — the font is monospaced, so every character
/// advances by this.
pub const GLYPH_WIDTH: i32 = 8;

/// How tall one glyph's cell is, in pixels.
pub const GLYPH_HEIGHT: i32 = 8;

/// The glyph a character the font does not letter is written as.
const UNKNOWN: char = '?';

/// How wide `text` is once written, in pixels.
pub fn text_width(text: &str) -> i32 {
    #[allow(clippy::cast_possible_truncation, clippy::cast_possible_wrap)]
    let characters = text.chars().count().min(i32::MAX as usize) as i32;
    characters.saturating_mul(GLYPH_WIDTH)
}

/// The bitmap the lettering is written into: `width` x `height` RGB pixels, row major — the
/// same buffer the plot was drawn into.
pub struct Canvas<'a> {
    pixels: &'a mut [u8],
    width: i32,
    height: i32,
}

impl<'a> Canvas<'a> {
    /// The canvas over `pixels`, which holds `width * height` RGB pixels.
    pub fn new(pixels: &'a mut [u8], (width, height): (u32, u32)) -> Self {
        #[allow(clippy::cast_possible_truncation, clippy::cast_possible_wrap)]
        let (width, height) = (width.min(i32::MAX as u32) as i32, height.min(i32::MAX as u32) as i32);
        Self {
            pixels,
            width,
            height,
        }
    }

    /// Write `text` with its top-left corner at `at`, in `color`.
    ///
    /// Anything outside the bitmap is simply not written, so a label that does not fit the image
    /// is clipped at its edge rather than refused or wrapped.
    pub fn write(&mut self, text: &str, at: (i32, i32), color: [u8; 3]) {
        let (mut x, y) = at;
        for character in text.chars() {
            self.glyph(character, (x, y), color);
            x = x.saturating_add(GLYPH_WIDTH);
        }
    }

    /// Write one character's glyph with its top-left corner at `at`.
    fn glyph(&mut self, character: char, at: (i32, i32), color: [u8; 3]) {
        // The table holds one cell per ASCII code, so a character outside it is written as the
        // glyph for one that is in it rather than indexed for.
        let index = usize::try_from(u32::from(character)).unwrap_or(usize::MAX);
        let Some(rows) = BASIC_LEGACY
            .get(index)
            .or_else(|| BASIC_LEGACY.get(UNKNOWN as usize))
            .copied()
        else {
            return;
        };
        for (row, bits) in rows.iter().enumerate() {
            for column in 0..GLYPH_WIDTH {
                // The font's rows are least significant bit first, so bit `column` is the
                // pixel `column` places right of the glyph's left edge.
                if bits & (1u8 << column) == 0 {
                    continue;
                }
                #[allow(clippy::cast_possible_truncation, clippy::cast_possible_wrap)]
                let row = row as i32;
                self.set(at.0 + column, at.1 + row, color);
            }
        }
    }

    /// Paint the pixel at `(x, y)`, when the bitmap has one there.
    fn set(&mut self, x: i32, y: i32, color: [u8; 3]) {
        if x < 0 || y < 0 || x >= self.width || y >= self.height {
            return;
        }
        let Some(offset) = y
            .checked_mul(self.width)
            .and_then(|row| row.checked_add(x))
            .and_then(|pixel| pixel.checked_mul(3))
            .and_then(|offset| usize::try_from(offset).ok())
        else {
            return;
        };
        if let Some(pixel) = self.pixels.get_mut(offset..offset + 3) {
            pixel.copy_from_slice(&color);
        }
    }
}
