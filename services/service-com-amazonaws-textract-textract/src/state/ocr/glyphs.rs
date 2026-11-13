// SPDX-License-Identifier: CC0-1.0

//! Naming the characters a line of ink is drawn with: the recognition stage of
//! [`super::read_raster`].
//!
//! A mark is named by comparing it against a BANK of templates — every printable ASCII
//! glyph of two typeface families, at several sizes and weights — and taking the character
//! of the template whose ink it shares most of ([`Mark::agreement`], the Sørensen-Dice
//! coefficient of the two). Nothing here looks at the request, at a digest or at the text
//! of anything: a character is reported only because the ink the page carries is shaped
//! like that character's.
//!
//! ## One frame for both sides
//!
//! What makes a page drawn in a face NO template comes from readable is that a mark and a
//! template are measured the same way. A mark is resampled into a frame anchored on the
//! line it sits in — the row that line's glyphs stand on, scaled by the height of its
//! capitals ([`Line`]) — rather than over its own bounding box, so the grid records the
//! mark's size, its shape and where it sits against the baseline at once: a full stop inks
//! a cell or two at the foot of the frame, a capital fills its middle, a comma hangs below
//! it, and a mark too tall or too wide to be a character of its line (a ruling line, or two
//! letters whose ink runs together) fills a frame no glyph fills and is named nothing at
//! all. A template is resampled into the same frame, taken from its own font's capitals, so
//! the two grids are comparable whatever pixel size either was drawn at.
//!
//! Both grids are then blurred one cell wide, which is what lets a stroke that lands half a
//! cell off the template's still match it: at the sizes a page is drawn at, a stem is one or
//! two cells across.
//!
//! ## What it costs
//!
//! The bank is built once ([`bank`]) and holds a few hundred templates, so every mark is
//! compared against all of them. The comparison is therefore in two stages: a coarse grid
//! shortlists the templates worth a closer look ([`SHORTLIST`] of them) and only those are
//! compared on the fine grid, which keeps the work per mark a small multiple of the bank's
//! size. How many marks one page may hold is bounded by its caller
//! ([`super::MAX_GLYPHS_PER_PAGE`]).

use std::sync::OnceLock;

use embedded_graphics::draw_target::DrawTarget;
use embedded_graphics::geometry::{Dimensions, OriginDimensions, Point, Size};
use embedded_graphics::image::{ImageDrawable, ImageRaw};
use embedded_graphics::mono_font::{ascii, MonoFont};
use embedded_graphics::pixelcolor::BinaryColor;
use embedded_graphics::primitives::Rectangle;
use embedded_graphics::Pixel;
use noto_sans_mono_bitmap::{get_raster, FontWeight, RasterHeight};

use super::{GlyphBox, Ink};

/// What the comparison this module makes SAW, and the test that holds the shortlist it
/// narrows to against the whole bank.
#[cfg(test)]
mod unused;

/// The side of the grid a mark and a template are compared on.
const FINE: usize = 16;

/// How many CHARACTERS the proportions shortlist for the fine comparison. The shortlist is
/// of characters rather than of faces so that one character drawn in several faces cannot
/// crowd another off it.
const SHORTLIST: usize = 40;

/// How many characters the grids themselves keep for the one-cell search of
/// [`Mark::agreement`], the last and closest step of the comparison.
const CLOSE_LOOK: usize = 10;

/// How far above its line's baseline a frame reaches, in capital heights: over the top of
/// an ascender, which stands a little above a capital.
const FRAME_ABOVE: f32 = 1.35;

/// How far below its line's baseline a frame reaches, in capital heights: under the foot of
/// a descender.
const FRAME_BELOW: f32 = 0.45;

/// How wide a frame is, in capital heights: wider than the widest Latin capital, so a
/// character's ink falls inside it and ink wider than any character does not.
const FRAME_WIDE: f32 = 1.8;

/// The share of a cell's ink a blurred cell keeps for itself, the rest being spread over
/// its eight neighbours: enough for a stroke a cell off the template's to meet it, without
/// washing a thin stroke out of the grid altogether.
const BLUR_CENTRE: f32 = 0.5;

/// How much of a mark's ink must be SHARED with the template that named it, as the
/// Sørensen-Dice coefficient of the two grids, for the character to be reported. Ink that
/// agrees with no glyph this well matched nothing, so it contributes no text.
pub(super) const MIN_MATCH: f32 = 0.52;

/// The side of one glyph cell of the embedded 8x8 bitmap font.
pub(super) const FONT_CELL: usize = 8;

/// How much ink a pixel must carry to be part of a glyph's own extent when a template's
/// metrics are measured off it: an anti-aliased edge carries less.
const EXTENT_FLOOR: f32 = 0.4;

/// The sizes and weights of the pre-rasterised typeface the bank's second family is taken
/// from: every weight the face is cut in, since a light stroke and a bold one fill a frame
/// differently, at a small and a large raster, since what anti-aliasing does to a glyph
/// drawn a dozen pixels tall is part of that glyph's own shape at that size.
const FACES: &[(FontWeight, RasterHeight)] = &[
    (FontWeight::Light, RasterHeight::Size16),
    (FontWeight::Regular, RasterHeight::Size16),
    (FontWeight::Bold, RasterHeight::Size16),
    (FontWeight::Light, RasterHeight::Size32),
    (FontWeight::Regular, RasterHeight::Size32),
    (FontWeight::Bold, RasterHeight::Size32),
];

/// The X11 fixed bitmap faces the bank's third family is taken from: small, middling and
/// large, in regular and in bold, so a page set in a pixel font is matched against glyphs
/// drawn the way that font draws them.
const PIXEL_FACES: &[&MonoFont<'static>] = &[
    &ascii::FONT_4X6,
    &ascii::FONT_5X7,
    &ascii::FONT_6X10,
    &ascii::FONT_6X13,
    &ascii::FONT_6X13_BOLD,
    &ascii::FONT_9X15,
    &ascii::FONT_10X20,
];

/// Where a line of text sets its characters: the row its glyphs stand on and the height of
/// its capitals, in the pixels of whatever it was measured on.
///
/// This is the frame every measurement here is taken in, so one `Line` describes both a line
/// of ink on a submitted page and the line a font sets its own glyphs on.
#[derive(Debug, Clone, Copy)]
pub(super) struct Line {
    /// The y of the row the line's glyphs stand on: the foot of a capital.
    baseline: f32,
    /// How tall the line's capitals are, in pixels. Never zero.
    pub(super) cap: f32,
}

impl Line {
    /// The frame of this line around `centre`: the rectangle of the page a grid measured in
    /// this line covers, as `(left, top, width, height)`.
    fn frame(&self, centre: f32) -> (f32, f32, f32, f32) {
        let width = FRAME_WIDE * self.cap;
        let height = (FRAME_ABOVE + FRAME_BELOW) * self.cap;
        (
            centre - width / 2.0,
            self.baseline - FRAME_ABOVE * self.cap,
            width,
            height,
        )
    }
}

/// The ink of ONE mark: what `coverage` reports inside the mark's own rectangle
/// `x0..=x1` by `y0..=y1`, and nothing outside it.
///
/// A frame reaches a capital's width either side of the mark it is around, so on a line of
/// text it reaches over the marks beside it: without this, the grid a narrow mark fills
/// would be mostly its NEIGHBOURS' ink and it would be named after them. A template is
/// bounded the same way, by the rectangle its own glyph inks.
fn own_ink<'a>(
    coverage: &'a impl Fn(isize, isize) -> f32,
    (x0, x1, y0, y1): (usize, usize, usize, usize),
) -> impl Fn(isize, isize) -> f32 + 'a {
    move |x: isize, y: isize| {
        let inside = (x0 as isize..=x1 as isize).contains(&x)
            && (y0 as isize..=y1 as isize).contains(&y);
        if inside {
            coverage(x, y)
        } else {
            0.0
        }
    }
}

/// One mark measured in the frame of its line: its PROPORTIONS in that frame, and how much
/// ink it puts in each cell of the frame's grid.
struct Mark {
    fine: [f32; FINE * FINE],
    fine_ink: f32,
    /// How wide the mark's own ink is, in capital heights.
    width: f32,
    /// How tall it is, in capital heights.
    height: f32,
    /// How far its top stands above its line's baseline, in capital heights.
    top: f32,
    /// How much ink it carries, in capital heights squared.
    mass: f32,
}

impl Mark {
    /// Measure the ink `coverage` reports inside the rectangle `x0..=x1` by `y0..=y1`, in
    /// the frame `line` sets around it.
    fn measure(
        coverage: &impl Fn(isize, isize) -> f32,
        (x0, x1, y0, y1): (usize, usize, usize, usize),
        line: Line,
    ) -> Self {
        let own = own_ink(coverage, (x0, x1, y0, y1));
        let centre = (x0 + x1 + 1) as f32 / 2.0;
        let mut fine = [0f32; FINE * FINE];
        sample(&own, centre, line, FINE, &mut fine);
        blur(FINE, &mut fine);
        // The proportions are measured off the PIXELS rather than off the grid: a grid cell
        // covers a tenth of a capital height, and rounding a mark's extent to whole cells
        // of it would leave two marks of the same proportions a cell apart as often as not.
        let mut ink = 0f32;
        for y in y0..=y1 {
            for x in x0..=x1 {
                ink += stroke(coverage(x as isize, y as isize));
            }
        }
        Self {
            fine_ink: fine.iter().sum(),
            width: (x1 + 1 - x0) as f32 / line.cap,
            height: (y1 + 1 - y0) as f32 / line.cap,
            top: (line.baseline - y0 as f32) / line.cap,
            mass: ink / (line.cap * line.cap),
            fine,
        }
    }

    /// How far this mark's PROPORTIONS are from `other`'s: how wide and how tall each is,
    /// where each stands against its line's baseline, and how much ink each carries.
    ///
    /// What the comparison starts from, because a mark's proportions survive every size it
    /// is drawn at and no amount of blur can confuse a stem with a bowl: the characters
    /// whose proportions are nothing like a mark's need not be compared with it cell by
    /// cell at all. The ink counts for least of the four, since how much of it a stroke
    /// lays down is the weight the page was drawn at rather than the character it draws.
    fn proportion_distance(&self, other: &Self) -> f32 {
        (self.width - other.width).abs()
            + (self.height - other.height).abs()
            + 0.7 * (self.top - other.top).abs()
            + 0.3 * (self.mass - other.mass).abs()
    }

    /// How much of this mark's ink and `other`'s is the same ink, on the grid of their
    /// frames: the Sørensen-Dice coefficient, which is 1 for two marks inked alike and 0
    /// for two that share no cell.
    ///
    /// The best agreement over a ONE-CELL search, because where a mark sits in its frame is
    /// measured off its line and a line's baseline and capital height are themselves read
    /// off the marks it carries: a stroke the width of a cell, and a mark of a cell or two
    /// altogether — a full stop, a colon, a hyphen — would otherwise miss the glyph it is
    /// drawn as over a fraction of a pixel. Ink shifted out of the frame still counts
    /// against the match, since the denominator is both marks' whole ink.
    fn agreement(&self, other: &Self) -> f32 {
        let mut best = 0f32;
        for down in -1..=1 {
            for across in -1..=1 {
                let scored = dice_shifted(
                    &self.fine,
                    self.fine_ink,
                    &other.fine,
                    other.fine_ink,
                    FINE,
                    across,
                    down,
                );
                best = best.max(scored);
            }
        }
        best
    }

    /// The same measure without that search: what narrows the shortlist to the characters
    /// worth taking it over.
    fn unshifted_agreement(&self, other: &Self) -> f32 {
        dice(&self.fine, self.fine_ink, &other.fine, other.fine_ink)
    }
}

/// The Sørensen-Dice coefficient of two grids of ink, the right one moved `across` cells
/// right and `down` cells down: the ink of both counts in the denominator, so ink one grid
/// carries where the other has no cell left to carry it tells against the agreement.
fn dice_shifted(
    left: &[f32],
    left_ink: f32,
    right: &[f32],
    right_ink: f32,
    grid: usize,
    across: isize,
    down: isize,
) -> f32 {
    let total = left_ink + right_ink;
    if total <= 0.0 {
        return 0.0;
    }
    let mut shared = 0f32;
    for row in 0..grid as isize {
        let other_row = row + down;
        if other_row < 0 || other_row >= grid as isize {
            continue;
        }
        for column in 0..grid as isize {
            let other_column = column + across;
            if other_column < 0 || other_column >= grid as isize {
                continue;
            }
            let one = left[row as usize * grid + column as usize];
            let another = right[other_row as usize * grid + other_column as usize];
            shared += one.min(another);
        }
    }
    2.0 * shared / total
}

/// The Sørensen-Dice coefficient of two grids of ink.
fn dice(left: &[f32], left_ink: f32, right: &[f32], right_ink: f32) -> f32 {
    let total = left_ink + right_ink;
    if total <= 0.0 {
        return 0.0;
    }
    let shared: f32 = left
        .iter()
        .zip(right.iter())
        .map(|(one, other)| one.min(*other))
        .sum();
    2.0 * shared / total
}

/// Resample the ink `coverage` reports into a `grid` x `grid` measurement of the frame
/// `line` sets around `centre`.
///
/// Each cell holds the ink of the rectangle of the page it covers, by AREA: a pixel counts
/// for the part of the cell it covers, and no more. That is what makes a glyph drawn large
/// and the same glyph drawn a dozen pixels tall fill the grid alike — one cell of the grid
/// is a fraction of a pixel of the one and several pixels of the other, and the edge of a
/// stroke lands part way into a cell either way.
///
/// Each pixel counts for its ink past the floor that tells ink from ground ([`stroke`]),
/// which is what keeps the two comparable at the sizes they are drawn at: anti-aliasing
/// leaves a stroke a halo a pixel or so wide whatever the glyph's size, so on a glyph drawn
/// small that halo is a large part of the ink and on one drawn large a small part, and
/// taking it at face value would read the small one as the bolder letter — its counters
/// filled, its strokes run together.
fn sample(
    coverage: &impl Fn(isize, isize) -> f32,
    centre: f32,
    line: Line,
    grid: usize,
    out: &mut [f32],
) {
    let (left, top, width, height) = line.frame(centre);
    let step_x = width / grid as f32;
    let step_y = height / grid as f32;
    // How much of `from..to` the pixel at `at` covers, where a pixel covers the square
    // `at..at + 1` of the page.
    let share = |at: isize, from: f32, to: f32| {
        let start = (at as f32).max(from);
        let end = ((at + 1) as f32).min(to);
        (end - start).max(0.0)
    };
    for row in 0..grid {
        let from_y = top + row as f32 * step_y;
        let to_y = from_y + step_y;
        for column in 0..grid {
            let from_x = left + column as f32 * step_x;
            let to_x = from_x + step_x;
            let mut total = 0f32;
            for y in from_y.floor() as isize..to_y.ceil() as isize {
                let down = share(y, from_y, to_y);
                if down <= 0.0 {
                    continue;
                }
                for x in from_x.floor() as isize..to_x.ceil() as isize {
                    let across = share(x, from_x, to_x);
                    if across <= 0.0 {
                        continue;
                    }
                    total += down * across * stroke(coverage(x, y));
                }
            }
            out[row * grid + column] = total / (step_x * step_y);
        }
    }
}

/// How much of a pixel's ink is a STROKE's rather than the halo anti-aliasing leaves around
/// one: its ink past the floor that tells a page's ink from its ground, rescaled so a solid
/// stroke is still a whole pixel of ink.
fn stroke(ink: f32) -> f32 {
    ((ink - super::INK_FLOOR) / (1.0 - super::INK_FLOOR)).clamp(0.0, 1.0)
}

/// Spread each cell's ink over its neighbours, so a stroke that lands a cell off the
/// template's still meets it.
fn blur(grid: usize, cells: &mut [f32]) {
    let source = cells.to_vec();
    let share = (1.0 - BLUR_CENTRE) / 8.0;
    for row in 0..grid {
        for column in 0..grid {
            let mut total = BLUR_CENTRE * source[row * grid + column];
            for (dy, dx) in [
                (-1i32, -1i32),
                (-1, 0),
                (-1, 1),
                (0, -1),
                (0, 1),
                (1, -1),
                (1, 0),
                (1, 1),
            ] {
                let y = row as i32 + dy;
                let x = column as i32 + dx;
                if y < 0 || x < 0 || y >= grid as i32 || x >= grid as i32 {
                    continue;
                }
                total += share * source[y as usize * grid + x as usize];
            }
            cells[row * grid + column] = total;
        }
    }
}

/// One character of the bank: the same character as each of the bank's faces draws it,
/// measured exactly as a mark of a page is.
struct Glyph {
    ch: char,
    faces: Vec<Mark>,
}

/// Every character the bank holds, with its faces, built once.
///
/// Grouped by CHARACTER rather than by face because that is how the comparison narrows: the
/// coarse pass asks each character for its closest face, and the fine pass then measures
/// every face of the characters it shortlisted.
fn bank() -> &'static [Glyph] {
    static BANK: OnceLock<Vec<Glyph>> = OnceLock::new();
    BANK.get_or_init(|| {
        let bitmap = bitmap_face();
        let faces: Vec<Box<dyn Fn(char) -> Option<Mark>>> = FACES
            .iter()
            .map(|(weight, height)| rastered_face(*weight, *height))
            .chain(PIXEL_FACES.iter().map(|font| pixel_face(font)))
            .collect();
        (b'!'..=b'~')
            .filter_map(|code| {
                let ch = code as char;
                let faces: Vec<Mark> = std::iter::once(bitmap(ch))
                    .chain(faces.iter().map(|face| face(ch)))
                    .flatten()
                    .collect();
                (!faces.is_empty()).then_some(Glyph { ch, faces })
            })
            .collect()
    })
}

/// The bbox of the ink `coverage` reports inside `width` by `height`, or `None` when it
/// reports none: `(x0, x1, y0, y1)`, each bound a pixel that carries ink.
fn extent(
    coverage: &impl Fn(isize, isize) -> f32,
    width: usize,
    height: usize,
) -> Option<(usize, usize, usize, usize)> {
    let (mut x0, mut x1, mut y0, mut y1) = (width, 0usize, height, 0usize);
    for y in 0..height {
        for x in 0..width {
            if coverage(x as isize, y as isize) >= EXTENT_FLOOR {
                x0 = x0.min(x);
                x1 = x1.max(x);
                y0 = y0.min(y);
                y1 = y1.max(y);
            }
        }
    }
    (x0 <= x1 && y0 <= y1).then_some((x0, x1, y0, y1))
}

/// The line a font sets its glyphs on, measured off the capital it draws for `coverage`:
/// the row that letter stands on, and how tall it is.
fn font_line(coverage: &impl Fn(isize, isize) -> f32, width: usize, height: usize) -> Option<Line> {
    let (_, _, y0, y1) = extent(coverage, width, height)?;
    Some(Line {
        baseline: (y1 + 1) as f32,
        cap: (y1 + 1 - y0) as f32,
    })
}

/// How the embedded 8x8 bitmap font draws one character, measured in the frame of its own
/// line.
///
/// `font8x8::legacy::BASIC_LEGACY` is the public-domain 8x8 bitmap font: one byte per row of
/// a glyph's cell, bit 0 the leftmost pixel.
fn bitmap_face() -> impl Fn(char) -> Option<Mark> {
    let rows_of = |ch: char| font8x8::legacy::BASIC_LEGACY.get(ch as usize).copied();
    let coverage = |rows: [u8; FONT_CELL]| {
        move |x: isize, y: isize| {
            let cell = 0..FONT_CELL as isize;
            if cell.contains(&x) && cell.contains(&y) && rows[y as usize] & (1 << x) != 0 {
                1.0
            } else {
                0.0
            }
        }
    };
    let line = rows_of('H').and_then(|rows| font_line(&coverage(rows), FONT_CELL, FONT_CELL));
    move |ch: char| {
        let line = line?;
        let ink = coverage(rows_of(ch)?);
        let own = extent(&ink, FONT_CELL, FONT_CELL)?;
        let mark = Mark::measure(&ink, own, line);
        Some(mark)
    }
}

/// How one size and weight of the pre-rasterised sans-serif face draws one character,
/// measured in the frame of its own line.
///
/// `noto_sans_mono_bitmap` holds that face's printable ASCII glyphs already rasterised: one
/// byte of ink per pixel, every glyph of one size set on the same line.
fn rastered_face(weight: FontWeight, height: RasterHeight) -> Box<dyn Fn(char) -> Option<Mark>> {
    let measured = move |ch: char| {
        let raster = get_raster(ch, weight, height)?;
        let (width, rows) = (raster.width(), raster.height());
        let pixels = raster.pixels();
        let coverage = move |x: isize, y: isize| {
            if x < 0 || y < 0 || x >= width as isize || y >= rows as isize {
                return 0.0;
            }
            pixels
                .get(y as usize * width + x as usize)
                .map_or(0.0, |ink| f32::from(*ink) / 255.0)
        };
        Some((coverage, width, rows))
    };
    let line = measured('H').and_then(|(ink, width, rows)| font_line(&ink, width, rows));
    Box::new(move |ch: char| {
        let line = line?;
        let (ink, width, rows) = measured(ch)?;
        let own = extent(&ink, width, rows)?;
        let mark = Mark::measure(&ink, own, line);
        Some(mark)
    })
}

/// How one of the X11 fixed bitmap faces draws one character, measured in the frame of its
/// own line.
///
/// `embedded_graphics::mono_font` holds those faces as a sprite sheet of one bit per pixel:
/// the glyphs of a face side by side, as many to a row as its image is wide, each in a cell
/// of the face's own character size, and `GlyphMapping` says which cell a character is in.
///
/// They are in the bank because a page drawn a dozen pixels to the line is set in a face
/// like these — a few strokes wide, every stroke one pixel — and not in an outline face
/// scaled down, so nothing else here is shaped like one.
fn pixel_face(font: &'static MonoFont<'static>) -> Box<dyn Fn(char) -> Option<Mark>> {
    let cell = font.character_size;
    let sheet = Sheet::of(&font.image);
    let per_row = (sheet.width / cell.width.max(1) as usize).max(1);
    let coverage = move |ch: char| {
        let index = font.glyph_mapping.index(ch);
        let left = (index % per_row) * cell.width as usize;
        let top = (index / per_row) * cell.height as usize;
        let sheet = sheet.clone();
        move |x: isize, y: isize| {
            let inside = x >= 0 && y >= 0 && x < cell.width as isize && y < cell.height as isize;
            if inside && sheet.lit(left + x as usize, top + y as usize) {
                1.0
            } else {
                0.0
            }
        }
    };
    let width = cell.width as usize;
    let height = cell.height as usize;
    let line = font_line(&coverage('H'), width, height);
    Box::new(move |ch: char| {
        let line = line?;
        let ink = coverage(ch);
        let own = extent(&ink, width, height)?;
        let mark = Mark::measure(&ink, own, line);
        Some(mark)
    })
}

/// One face's glyph sheet, as the pixels it is drawn from.
///
/// `ImageRaw` keeps its bits to itself and offers them by DRAWING them, so the sheet is
/// drawn once per face into this, which is the smallest draw target there is: it records
/// which of its pixels were turned on.
#[derive(Clone)]
struct Sheet {
    width: usize,
    height: usize,
    lit: std::rc::Rc<Vec<bool>>,
}

impl Sheet {
    fn of(image: &ImageRaw<'static, BinaryColor>) -> Self {
        let size = image.size();
        let mut drawn = Drawn {
            width: size.width as usize,
            height: size.height as usize,
            lit: vec![false; (size.width * size.height) as usize],
        };
        // `Infallible`: the target below cannot fail, so this cannot.
        let _ = image.draw(&mut drawn);
        Self {
            width: drawn.width,
            height: drawn.height,
            lit: std::rc::Rc::new(drawn.lit),
        }
    }

    fn lit(&self, x: usize, y: usize) -> bool {
        if x >= self.width || y >= self.height {
            return false;
        }
        self.lit.get(y * self.width + x).copied().unwrap_or(false)
    }
}

/// The draw target [`Sheet`] collects a face's glyph sheet with.
struct Drawn {
    width: usize,
    height: usize,
    lit: Vec<bool>,
}

impl Dimensions for Drawn {
    fn bounding_box(&self) -> Rectangle {
        Rectangle::new(
            Point::zero(),
            Size::new(self.width as u32, self.height as u32),
        )
    }
}

impl DrawTarget for Drawn {
    type Color = BinaryColor;
    type Error = core::convert::Infallible;

    fn draw_iter<I>(&mut self, pixels: I) -> Result<(), Self::Error>
    where
        I: IntoIterator<Item = Pixel<Self::Color>>,
    {
        for Pixel(point, colour) in pixels {
            if point.x < 0 || point.y < 0 {
                continue;
            }
            let (x, y) = (point.x as usize, point.y as usize);
            if x >= self.width || y >= self.height {
                continue;
            }
            self.lit[y * self.width + x] = colour.is_on();
        }
        Ok(())
    }
}

/// The character the mark `glyph` of a page is drawn with, and how well its ink agreed with
/// that character's, or `None` when no glyph's ink is this mark's.
///
/// The comparison narrows in three steps, each over fewer characters than the one before,
/// because the bank holds every character in every face. Every step ranks CHARACTERS, each
/// by its own closest face, so one character drawn in several faces cannot take several
/// places on a shortlist and crowd the right answer off it:
///
/// 1. the PROPORTIONS of the mark against each character's, over the whole bank, which
///    costs four numbers per face and shortlists the [`SHORTLIST`] closest characters;
/// 2. the grids themselves, cell against cell, over every face of those characters, which
///    keeps the [`CLOSE_LOOK`] closest;
/// 3. the grids again with the one-cell search of [`Mark::agreement`], which settles the
///    answer.
pub(super) fn recognise(ink: &Ink, glyph: &GlyphBox, line: Line) -> Option<Named> {
    if line.cap < 1.0 {
        return None;
    }
    let page = |x: isize, y: isize| ink.coverage(x, y);
    let mark = Mark::measure(&page, (glyph.x0, glyph.x1, glyph.y0, glyph.y1), line);
    if mark.fine_ink <= 0.0 {
        return None;
    }
    let bank = bank();
    let mut shortlist = Ranking::<SHORTLIST>::new();
    for (index, candidate) in bank.iter().enumerate() {
        let apart = candidate
            .faces
            .iter()
            .map(|face| mark.proportion_distance(face))
            .fold(f32::MAX, f32::min);
        shortlist.offer(-apart, index);
    }
    let mut close = Ranking::<CLOSE_LOOK>::new();
    for index in shortlist.taken() {
        let scored = bank[index]
            .faces
            .iter()
            .map(|face| mark.unshifted_agreement(face))
            .fold(0.0f32, f32::max);
        close.offer(scored, index);
    }
    let mut named: Vec<(char, f32)> = Vec::with_capacity(CLOSE_LOOK);
    for index in close.taken() {
        let scored = bank[index]
            .faces
            .iter()
            .map(|face| mark.agreement(face))
            .fold(0.0f32, f32::max);
        named.push((bank[index].ch, scored));
    }
    let best = named
        .iter()
        .copied()
        .reduce(|held, found| if found.1 > held.1 { found } else { held })?;
    if best.1 < MIN_MATCH {
        return None;
    }
    // Whether the shape left the reading OPEN: a bar is not a character of a word at all,
    // and where the mark is a stem the shape may not tell the stems apart — the best of the
    // others it could be scores as well.
    let open = best.0 == '|'
        || (STEMS.contains(&best.0)
            && named
                .iter()
                .filter(|(ch, _)| *ch != best.0 && STEMS.contains(ch))
                .any(|(_, scored)| best.1 - scored <= STEM_MARGIN));
    Some(Named {
        ch: best.0,
        score: best.1,
        open,
    })
}

/// What one mark was named: the character, how well its ink agreed with that character's,
/// and whether the shape left the reading OPEN — true for a mark that is a bare stem, which
/// several characters are drawn as, and for one named as a bar, which is no character of a
/// word at all (see [`STEMS`]).
#[derive(Debug, Clone, Copy)]
pub(super) struct Named {
    pub(super) ch: char,
    pub(super) score: f32,
    pub(super) open: bool,
}

/// The characters a face may draw as nothing but a stem: the capital i, the small l, the
/// figure one and the bar. Which of them a face gives a serif, a tail or a flag differs from
/// face to face, so a mark that is a bare stem is every one of them at once.
pub(super) const STEMS: &[char] = &['I', 'l', '1', '|'];

/// How close the best of the other [`STEMS`] must score for a mark's shape to have left
/// which of them it is OPEN: a hair, since the four are the same ink only where the face
/// draws them so.
const STEM_MARGIN: f32 = 0.04;

/// The `N` best-scoring items offered to it, best first: how each step of [`recognise`]
/// hands the next one the characters worth a closer look.
struct Ranking<const N: usize> {
    held: Vec<(f32, usize)>,
}

impl<const N: usize> Ranking<N> {
    fn new() -> Self {
        Self {
            held: Vec::with_capacity(N + 1),
        }
    }

    fn offer(&mut self, score: f32, item: usize) {
        if self.held.len() == N && self.held.last().is_some_and(|(worst, _)| score <= *worst) {
            return;
        }
        let at = self.held.partition_point(|(held, _)| *held > score);
        self.held.insert(at, (score, item));
        self.held.truncate(N);
    }

    fn taken(self) -> Vec<usize> {
        self.held.into_iter().map(|(_, item)| item).collect()
    }
}

/// Where the line whose marks are `glyphs`, found in the rows `top..=bottom`, sets its
/// characters — measured off those marks.
///
/// A line's baseline is the row most of its marks stand on, and its capital height is how
/// tall the taller of them are, so the frame a mark is measured in is the line's own rather
/// than a size fixed in advance. Both are read off the marks the line really carries: the
/// median foot, and the ninth decile of the heights — which is the height of a capital on a
/// line that holds one, while the x-height letters that outnumber it do not pull it down and
/// the one mark that reaches past it (a currency sign, a bracket) does not pull it up. A
/// mark whose height stands right outside the rest — a full stop below, a ruling line drawn
/// through — is left out of both. A line no mark of that kind was found on is framed by the
/// band of rows it inks.
pub(super) fn line_of(glyphs: &[GlyphBox], top: usize, bottom: usize) -> Line {
    let band = Line {
        baseline: (bottom + 1) as f32,
        cap: (bottom + 1 - top).max(1) as f32,
    };
    let heights: Vec<f32> = glyphs.iter().map(|glyph| glyph.height() as f32).collect();
    let Some(middle) = super::quantile(&heights, 0.5) else {
        return band;
    };
    let kept: Vec<&GlyphBox> = glyphs
        .iter()
        .filter(|glyph| {
            let height = glyph.height() as f32;
            height <= 2.0 * middle && height >= 0.3 * middle
        })
        .collect();
    let feet: Vec<f32> = kept.iter().map(|glyph| (glyph.y1 + 1) as f32).collect();
    let kept_heights: Vec<f32> = kept.iter().map(|glyph| glyph.height() as f32).collect();
    match (
        super::quantile(&feet, 0.5),
        super::quantile(&kept_heights, 0.9),
    ) {
        (Some(baseline), Some(cap)) if cap >= 1.0 => Line { baseline, cap },
        _ => band,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// No two characters of one face are the same ink: each letter and digit, as every face
    /// of the bank draws it, agrees with its own character more than with any other, so the
    /// comparison the recogniser decides by separates the glyphs it is given rather than
    /// collapsing two of them into one answer.
    #[test]
    fn a_face_agrees_with_its_own_character_best() {
        let bank = bank();
        let named: Vec<&Glyph> = bank
            .iter()
            .filter(|candidate| candidate.ch.is_ascii_alphanumeric())
            .collect();
        assert_eq!(
            named.len(),
            62,
            "the bank must hold every ASCII letter and digit, got {:?}",
            named.iter().map(|candidate| candidate.ch).collect::<String>()
        );
        for candidate in &named {
            assert_eq!(
                candidate.faces.len(),
                FACES.len() + PIXEL_FACES.len() + 1,
                "case {:?}: every face must draw it",
                candidate.ch
            );
            for drawn in &candidate.faces {
                let (mut best, mut best_ch) = (0.0f32, candidate.ch);
                for other in &named {
                    if other.ch == candidate.ch {
                        continue;
                    }
                    for other_drawn in &other.faces {
                        let scored = drawn.agreement(other_drawn);
                        if scored > best {
                            best = scored;
                            best_ch = other.ch;
                        }
                    }
                }
                let own = drawn.agreement(drawn);
                assert!(
                    own > best,
                    "case {:?}: a face must agree with its own character ({own:.3}) more \
                     than with {best_ch:?} ({best:.3})",
                    candidate.ch
                );
            }
        }
    }
}
