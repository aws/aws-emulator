// SPDX-License-Identifier: CC0-1.0

//! Naming the characters a line of ink is drawn with: the recognition stage of
//! [`super::read_raster`].
//!
//! A mark is named by comparing it against a BANK of templates — every printable ASCII
//! glyph of four typeface families, at several sizes and weights — and taking the character
//! of the template whose ink it shares most of ([`Mark::agreement_above`], the Sørensen-Dice
//! coefficient of the two). Nothing here looks at the request, at a digest or at the text
//! of anything: a character is reported only because the ink the page carries is shaped
//! like that character's.
//!
//! The families are what a document is really set in, because a mark is named after the
//! template it is shaped most like and nothing else: an 8x8 bitmap font, a pre-rasterised
//! sans-serif face at several weights and rasters, the X11 fixed faces at every raster they
//! are cut at — and a PROPORTIONAL outline face, rasterised here at the sizes a page is set
//! at. The first three are monospaced, and a monospaced cut is not the same letterform as a
//! proportional one: it grows serifs on the narrow letters and takes the shoulders off the
//! wide ones, which is exactly the ink that reads as another character when nothing shaped
//! like a proportional letter is there to match it.
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
//! What a blurred grid cannot see is told apart by counting: how many SEPARATE strokes the
//! ink falls into and how many HOLES they enclose ([`shape_of`]), measured off the pixels of
//! the mark and of the template alike, so a character a face draws with a mark of its own —
//! the dot of a `j`, the bars of an equals sign — is not read off ink that is one unbroken
//! stroke, and one drawn around two counters — an `8`, a `B` — is not read off ink that
//! closes once.
//!
//! The frame is the line's own, so the capital height it is scaled by is read off the marks
//! the line carries ([`line_of`]): the height its flat capitals agree on, since the round
//! letters of a face overshoot them and the few letters that descend through the baseline
//! reach past them, and framing a line by either measures every mark on it against a
//! capital height no letter of it has.
//!
//! ## What it costs
//!
//! The bank is built once ([`bank`]) and holds a few hundred templates, so every mark is
//! compared against all of them. The comparison is therefore in two stages: a coarse grid
//! shortlists the templates worth a closer look ([`SHORTLIST`] of them) and only those are
//! compared on the fine grid, which keeps the work per mark a small multiple of the bank's
//! size. How many marks one page may hold is bounded by its caller
//! ([`super::MAX_GLYPHS_PER_PAGE`]).
//!
//! Each stage is also measured only as far as its answer can still change. Every comparison
//! carries the agreement it has to beat to matter — the best face found so far, the poorest
//! character the shortlist still holds, or the floor below which a character cannot enter the
//! reading at all ([`NAMING_FLOOR`]) — and a template whose remaining cells cannot carry it
//! past that is dropped where it stands ([`Grid::best_shifted_above`]). What the comparison
//! ANSWERS is untouched by this: a template is only ever dropped once the ink it has left is
//! too little to overtake the one that is winning.

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

/// Why a comparison measured only as far as it can still change the answer gives the answer
/// an unbounded one gives.
#[cfg(test)]
#[path = "zprof.rs"]
mod bounds;

/// The side of the grid a mark and a template are compared on.
const FINE: usize = 16;

/// How many CHARACTERS the proportions shortlist for the fine comparison. The shortlist is
/// of characters rather than of faces so that one character drawn in several faces cannot
/// crowd another off it.
const SHORTLIST: usize = 40;

/// How many characters the grids themselves keep for the one-cell search of
/// [`Mark::agreement_above`], the last and closest step of the comparison.
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

/// What one stroke or one hole of difference between a mark's ink and a template's costs
/// that template ([`shape_of`]): enough to settle two characters that are the same ink but
/// for the mark a face draws apart from the rest, or for where one of them closes — a `J`
/// against a `j`, a `6` against an `8` — which a grid blurred a cell wide can hardly tell
/// apart, and not so much that a stroke which merged with its neighbour, a counter that
/// filled in, or a stroke that broke, is read as another character.
const SHAPE_APART: f32 = 0.05;

/// How close two characters' agreements must stand for the BLURRED grids to have left which
/// of them a mark is undecided, so that the ink as each of them draws it settles the answer
/// ([`settled`]).
///
/// Blurring a grid a cell wide spreads a stroke over its neighbours, so two characters that
/// differ by one stroke of a letterform — a crossbar, a flat side, a counter that closes —
/// come out within about this of each other, while two characters that differ by a whole
/// letterform do not. How far apart that one stroke leaves them depends on the size and the
/// weight the page was drawn at: the heavier the stroke and the smaller the counter it
/// closes, the smaller the share of the grid that one letterform covers and the nearer the
/// two readings stand — so the window has to cover the spread a single stroke of difference
/// accounts for at any of those sizes, and the sharp ink then settles which character it is.
const TIED: f32 = 0.05;

/// How much ink a mark and a template may have shared in the cells a comparison has not
/// reached, over and above what the rows left hold, before it gives up on that template: a
/// hair, so that a bound rounded a fraction of a cell short can never drop a template that
/// was about to win.
const BOUND_SLACK: f32 = 1e-6;

/// How well a character has to agree with a mark's ink for that agreement to be worth
/// measuring exactly.
///
/// A character agreeing less well than this is named no differently from one that agrees not
/// at all: the reading is taken from the character whose agreement is the best of them and
/// at least [`MIN_MATCH`], and from the ones standing within [`TIED`] or [`STEM_MARGIN`] of
/// it, so a character this far below the poorest reading there can be enters none of those.
/// The comparison therefore stops measuring a character the moment its agreement cannot
/// reach this.
const NAMING_FLOOR: f32 = MIN_MATCH - TIED - STEM_MARGIN;

/// The side of one glyph cell of the embedded 8x8 bitmap font.
pub(super) const FONT_CELL: usize = 8;

/// How much ink a pixel must carry to be part of a glyph's own extent when a template's
/// metrics are measured off it: an anti-aliased edge carries less.
const EXTENT_FLOOR: f32 = 0.4;

/// The most pixels ONE CELL of the grid is measured over on a side. NOT an AWS limit: a cell
/// of the frame covers a fraction of a capital height, so on a page whose capitals stand a
/// few dozen pixels tall every pixel of a cell is measured, while on one whose marks are as
/// tall as the page itself — a rule, a border, a stroke the length of a side — the cell's own
/// mean is taken over this many of them instead. It is what keeps the work of measuring one
/// mark a fixed amount rather than one that grows with the square of the page's own size.
const MAX_CELL_SAMPLES: usize = 8;

/// The most pixels ONE MARK's own ink is measured over on a side, for the same reason and in
/// the same way ([`MAX_CELL_SAMPLES`]): a mark of any ordinary size is measured pixel by
/// pixel, and one as large as a page is measured over this many of them.
const MAX_MARK_SAMPLES: usize = 64;

/// The step, in pixels, that measuring `span` pixels takes: every pixel of a span no longer
/// than `samples`, and one in every few of a longer one, so the measurement always costs at
/// most `samples` of them. Never zero.
fn stride_over(span: f32, samples: usize) -> usize {
    (span / samples as f32).ceil().max(1.0) as usize
}

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

/// The weights and pixel sizes the PROPORTIONAL outline face the bank's fourth family is
/// taken from is rasterised at.
///
/// A proportional face sets every letter in the width of its own ink — a narrow `l`, a wide
/// `m` — so its letterforms are not the monospaced families' letterforms squeezed into one
/// cell: in a monospaced cut the narrow letters grow serifs and the wide ones lose their
/// shoulders, which is exactly the ink a comparison against a monospaced template alone
/// reads as another character.
///
/// The REGULAR cut is held at a LADDER of rasters across the range a document's own text is
/// set at, rather than at a few points of it: what anti-aliasing and hinting do to an outline
/// is part of that glyph's shape AT THAT SIZE, so a page set between two held rasters is
/// matched against templates whose counters and stroke weights are a size away from its own —
/// which is what turns a `5` into an `S` and a `B` into an `8`, and what lets the thin middle
/// of a `W` break into two `V`s. The step is about a sixth of the size, which is as close as
/// two rasters of one outline are distinguishable at all. The BOLD cut is held at the rasters
/// where its heavier stroke fills a glyph's counters and changes its shape, which is the small
/// and middling ones, more coarsely: a bold outline at a large raster is the shape the regular
/// templates at that size already carry.
const OUTLINE_FACES: &[(&[u8], f32)] = &[
    (ttf_noto_sans::REGULAR, 12.0),
    (ttf_noto_sans::REGULAR, 14.0),
    (ttf_noto_sans::REGULAR, 16.0),
    (ttf_noto_sans::REGULAR, 18.0),
    (ttf_noto_sans::REGULAR, 20.0),
    (ttf_noto_sans::REGULAR, 22.0),
    (ttf_noto_sans::REGULAR, 24.0),
    (ttf_noto_sans::REGULAR, 26.0),
    (ttf_noto_sans::REGULAR, 28.0),
    (ttf_noto_sans::REGULAR, 32.0),
    (ttf_noto_sans::REGULAR, 36.0),
    (ttf_noto_sans::REGULAR, 42.0),
    (ttf_noto_sans::REGULAR, 48.0),
    (ttf_noto_sans::BOLD, 12.0),
    (ttf_noto_sans::BOLD, 16.0),
    (ttf_noto_sans::BOLD, 20.0),
    (ttf_noto_sans::BOLD, 24.0),
    (ttf_noto_sans::BOLD, 32.0),
];

/// The X11 fixed bitmap faces the bank's third family is taken from: small, middling and
/// large, in regular and in bold, so a page set in a pixel font is matched against glyphs
/// drawn the way that font draws them.
const PIXEL_FACES: &[&MonoFont<'static>] = &[
    &ascii::FONT_4X6,
    &ascii::FONT_5X7,
    &ascii::FONT_6X9,
    &ascii::FONT_6X10,
    &ascii::FONT_6X12,
    &ascii::FONT_6X13,
    &ascii::FONT_6X13_BOLD,
    &ascii::FONT_7X13,
    &ascii::FONT_7X14,
    &ascii::FONT_8X13,
    &ascii::FONT_9X15,
    &ascii::FONT_9X18,
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
    fine: Grid,
    /// The same grid before it was blurred: the ink exactly where the mark lays it down.
    ///
    /// What the blurred grid cannot see, because the blur is what lets a stroke that lands
    /// half a cell off its template's still meet it: the crossbar of a `t` against the bare
    /// stem of an `l`, the flat side of a `D` against the bowl of an `O`. Two characters
    /// whose blurred grids a mark agrees with equally well are therefore settled on this one
    /// ([`name_mark`]).
    sharp: Grid,
    /// How wide the mark's own ink is, in capital heights.
    width: f32,
    /// How tall it is, in capital heights.
    height: f32,
    /// How far its top stands above its line's baseline, in capital heights.
    top: f32,
    /// How much ink it carries, in capital heights squared.
    mass: f32,
    /// How many SEPARATE strokes its ink falls into, and how many holes they enclose: see
    /// [`shape_of`].
    shape: Shape,
}

/// One measurement of ink on the grid of a frame: the cells, how much ink they hold in all,
/// and how much of it the rows from each row down hold.
///
/// The row totals are what lets a comparison STOP once its answer cannot change: the ink
/// left in the rows it has not reached bounds everything those rows can still contribute, so
/// a template already behind the best one found is dropped where it stands rather than
/// measured to the end ([`Self::best_shifted_above`]).
struct Grid {
    cells: [f32; FINE * FINE],
    /// How much ink each row of the grid holds.
    rows: [f32; FINE],
    /// How much ink the whole grid holds.
    ink: f32,
}

impl Grid {
    /// The grid `cells` hold, with the totals a bounded comparison measures against.
    fn of(cells: [f32; FINE * FINE]) -> Self {
        let ink = cells.iter().sum();
        let mut rows = [0f32; FINE];
        for (row, holds) in rows.iter_mut().enumerate() {
            *holds = cells[row * FINE..(row + 1) * FINE].iter().sum();
        }
        Self { cells, rows, ink }
    }

    /// How much of this grid's ink and `other`'s is the same ink, with `other` moved
    /// `across` cells right and `down` cells down — or any amount at most `need`, once the
    /// rows left cannot carry the total past it.
    ///
    /// Two rows laid against each other share no more ink than the lesser of them holds, so
    /// the rows alone say how much the grids can still share below the row reached. That
    /// bound is taken before a single cell is measured — a shift that cannot reach `need` at
    /// all then costs a row's worth of comparisons rather than a grid's — and again at each
    /// row, so a comparison that has fallen too far behind to matter stops there.
    fn shared_shifted(&self, other: &Self, across: isize, down: isize, need: f32) -> f32 {
        let mut most = [0f32; FINE + 1];
        for row in (0..FINE).rev() {
            let other_row = row as isize + down;
            let theirs = if (0..FINE as isize).contains(&other_row) {
                other.rows[other_row as usize]
            } else {
                0.0
            };
            most[row] = most[row + 1] + self.rows[row].min(theirs);
        }
        if most[0] + BOUND_SLACK <= need {
            return 0.0;
        }
        let mut shared = 0f32;
        for row in 0..FINE {
            let other_row = row as isize + down;
            if (0..FINE as isize).contains(&other_row) {
                let ours = &self.cells[row * FINE..(row + 1) * FINE];
                let theirs =
                    &other.cells[other_row as usize * FINE..(other_row as usize + 1) * FINE];
                let from = (-across).max(0) as usize;
                let to = (FINE as isize - across).clamp(0, FINE as isize) as usize;
                for column in from..to {
                    shared += ours[column].min(theirs[(column as isize + across) as usize]);
                }
            }
            if shared + most[row + 1] + BOUND_SLACK <= need {
                return shared;
            }
        }
        shared
    }

    /// How much of the two grids' ink is the same ink over a ONE-CELL search, as the
    /// Sørensen-Dice coefficient — which is 1 for two grids inked alike and 0 for two that
    /// share no cell — or `floor` where no shift of them agrees better than that.
    ///
    /// The best agreement over a one-cell search, because where a mark sits in its frame is
    /// measured off its line and a line's baseline and capital height are themselves read
    /// off the marks it carries: a stroke the width of a cell, and a mark of a cell or two
    /// altogether — a full stop, a colon, a hyphen — would otherwise miss the glyph it is
    /// drawn as over a fraction of a pixel. Ink shifted out of the frame still counts
    /// against the match, since the denominator is both grids' whole ink.
    ///
    /// `floor` is the agreement the answer has to beat to be worth having exactly: two grids
    /// can share no more than all of the ink of the lesser of them, so one that cannot reach
    /// `floor` on that count alone is answered without measuring a cell, and one that falls
    /// behind part way through is answered without measuring the rest.
    fn best_shifted_above(&self, other: &Self, floor: f32) -> f32 {
        let total = self.ink + other.ink;
        if total <= 0.0 {
            return 0.0;
        }
        if 2.0 * self.ink.min(other.ink) / total <= floor {
            return floor;
        }
        let mut best = floor;
        // The grids as they STAND first, and the eight shifts of them after: a stroke lands
        // where the template lays it far more often than a cell off it, so measuring that
        // one first leaves the others a figure to beat that is usually already the answer.
        for (across, down) in [
            (0, 0),
            (-1, -1),
            (-1, 0),
            (-1, 1),
            (0, -1),
            (0, 1),
            (1, -1),
            (1, 0),
            (1, 1),
        ] {
            let shared = self.shared_shifted(other, across, down, best * total / 2.0);
            best = best.max(2.0 * shared / total);
        }
        best
    }

    /// The same measure without that search, which is what narrows the shortlist to the
    /// characters worth taking it over.
    fn dice_above(&self, other: &Self, floor: f32) -> f32 {
        let total = self.ink + other.ink;
        if total <= 0.0 {
            return 0.0;
        }
        if 2.0 * self.ink.min(other.ink) / total <= floor {
            return floor;
        }
        let shared = self.shared_shifted(other, 0, 0, floor * total / 2.0);
        floor.max(2.0 * shared / total)
    }
}

/// What a mark's ink is made of, in the counts that survive every size it is drawn at: see
/// [`shape_of`].
#[derive(Debug, Clone, Copy)]
struct Shape {
    strokes: usize,
    holes: usize,
}

impl Shape {
    /// How far this shape stands from `other`'s: a stroke or a hole of difference each.
    fn apart(&self, other: &Self) -> f32 {
        (self.strokes.abs_diff(other.strokes) + self.holes.abs_diff(other.holes)) as f32
    }
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
        let sharp = fine;
        blur(FINE, &mut fine);
        // The proportions are measured off the PIXELS rather than off the grid: a grid cell
        // covers a tenth of a capital height, and rounding a mark's extent to whole cells
        // of it would leave two marks of the same proportions a cell apart as often as not.
        //
        // How much ink a mark carries is a mean over its own rectangle, so a mark larger
        // than [`MAX_MARK_SAMPLES`] on a side is measured at one pixel in every few
        // ([`stride_over`]): what the mean is taken over is bounded, and the pixels of a
        // mark of any ordinary size are every one of them.
        let (stride_x, stride_y) = (
            stride_over((x1 + 1 - x0) as f32, MAX_MARK_SAMPLES),
            stride_over((y1 + 1 - y0) as f32, MAX_MARK_SAMPLES),
        );
        let mut ink = 0f32;
        for y in (y0..=y1).step_by(stride_y) {
            for x in (x0..=x1).step_by(stride_x) {
                ink += stroke(coverage(x as isize, y as isize));
            }
        }
        let sampled = (stride_x * stride_y) as f32;
        Self {
            sharp: Grid::of(sharp),
            // Saturating: a mark is the rectangle its own ink occupies, so the narrowest and
            // the shortest one there can be is a single pixel.
            width: (x1.saturating_sub(x0) + 1) as f32 / line.cap,
            height: (y1.saturating_sub(y0) + 1) as f32 / line.cap,
            top: (line.baseline - y0 as f32) / line.cap,
            mass: ink * sampled / (line.cap * line.cap),
            shape: shape_of(coverage, (x0, x1, y0, y1)),
            fine: Grid::of(fine),
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
    /// frames, or `floor` where they share no more of it than that
    /// ([`Grid::best_shifted_above`]).
    fn agreement_above(&self, other: &Self, floor: f32) -> f32 {
        self.fine.best_shifted_above(&other.fine, floor)
    }

    /// The same over the grids BEFORE they were blurred: how much of the two marks' ink is
    /// the same ink exactly where each lays it down.
    ///
    /// What the blur hides, since what it is for is to let a stroke that lands half a cell
    /// off its template's still meet it: the crossbar of a `t` beside the bare stem of an
    /// `l`, the flat side of a `D` beside the bowl of an `O`, the one counter of a `6`
    /// beside the two of an `8`. Two characters whose blurred grids a mark agrees with
    /// equally are told apart on this ([`settled`]).
    fn sharp_agreement_above(&self, other: &Self, floor: f32) -> f32 {
        self.sharp.best_shifted_above(&other.sharp, floor)
    }

    /// The same measure without that search: what narrows the shortlist to the characters
    /// worth taking it over.
    fn unshifted_agreement_above(&self, other: &Self, floor: f32) -> f32 {
        self.fine.dice_above(&other.fine, floor)
    }

    /// How far this mark's ink stands from `other`'s in what it is MADE of: see
    /// [`shape_of`].
    fn shape_apart(&self, other: &Self) -> f32 {
        self.shape.apart(&other.shape)
    }
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
///
/// A cell wider or taller than [`MAX_CELL_SAMPLES`] pixels is measured at one pixel in every
/// few of it ([`stride_over`]), each standing for the pixels it starts: the cell's mean is
/// then taken over a bounded number of them whatever the page's own size, and a cell of the
/// size a page's glyphs set — a fraction of a capital height — is measured pixel by pixel as
/// before.
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
    let stride_x = stride_over(step_x, MAX_CELL_SAMPLES);
    let stride_y = stride_over(step_y, MAX_CELL_SAMPLES);
    // How much of `from..to` the pixels at `at..at + stride` cover, where a pixel covers the
    // square `at..at + 1` of the page.
    let share = |at: isize, stride: usize, from: f32, to: f32| {
        let start = (at as f32).max(from);
        let end = ((at + stride as isize) as f32).min(to);
        (end - start).max(0.0)
    };
    for row in 0..grid {
        let from_y = top + row as f32 * step_y;
        let to_y = from_y + step_y;
        for column in 0..grid {
            let from_x = left + column as f32 * step_x;
            let to_x = from_x + step_x;
            let mut total = 0f32;
            let mut y = from_y.floor() as isize;
            while y < to_y.ceil() as isize {
                let down = share(y, stride_y, from_y, to_y);
                if down <= 0.0 {
                    y += stride_y as isize;
                    continue;
                }
                let mut x = from_x.floor() as isize;
                while x < to_x.ceil() as isize {
                    let across = share(x, stride_x, from_x, to_x);
                    if across > 0.0 {
                        total += down * across * stroke(coverage(x, y));
                    }
                    x += stride_x as isize;
                }
                y += stride_y as isize;
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

/// What the ink inside `x0..=x1` by `y0..=y1` is made of, as the two counts that survive
/// every size a glyph is drawn at: how many SEPARATE STROKES it falls into, and how many
/// HOLES those strokes enclose.
///
/// A character is drawn in a known number of each — one stroke for most letters, two for the
/// ones a face draws with a dot or a pair of bars (`i`, `j`, `:`, `=`, `?`), three for a `%`;
/// no hole in a `C` or a `7`, one in a `6`, a `9`, a `P` or an `A`, two in an `8` or a `B` —
/// so a mark that is one unbroken stroke around one counter is not the character a face
/// draws in two of either, whatever the two grids agree on cell by cell. That is what a grid
/// blurred a cell wide cannot see: the counter of a `6` and the two of an `8` are the same
/// ink everywhere but where the one closes.
///
/// Both counts are taken the same way on both sides of the comparison, off the pixels of the
/// mark and of the template, over the eight neighbours of an inked pixel so a stroke drawn
/// at a diagonal counts once, and over the four of a blank one so two counters that touch at
/// a corner stay two. A hole is ground the mark encloses, so ground that reaches the edge of
/// the mark's own box is not one. What a page's own ink gives is evidence rather than a rule:
/// a stroke that merged with its neighbour at the size the page was drawn at, a counter that
/// filled in, or a stroke that broke, costs its character a little ([`SHAPE_APART`]) instead
/// of ruling it out.
///
/// The scan is over at most [`MAX_MARK_SAMPLES`] pixels on a side ([`stride_over`]), as the
/// mark's own ink is, so a mark the size of a page costs no more to count than a character
/// does.
fn shape_of(
    coverage: &impl Fn(isize, isize) -> f32,
    (x0, x1, y0, y1): (usize, usize, usize, usize),
) -> Shape {
    let (stride_x, stride_y) = (
        stride_over((x1 + 1 - x0) as f32, MAX_MARK_SAMPLES),
        stride_over((y1 + 1 - y0) as f32, MAX_MARK_SAMPLES),
    );
    let columns: Vec<usize> = (x0..=x1).step_by(stride_x).collect();
    let rows: Vec<usize> = (y0..=y1).step_by(stride_y).collect();
    let inked: Vec<bool> = rows
        .iter()
        .flat_map(|y| {
            columns
                .iter()
                .map(move |x| coverage(*x as isize, *y as isize) >= super::INK_FLOOR)
        })
        .collect();
    let (wide, deep) = (columns.len(), rows.len());
    let mut taken = vec![false; inked.len()];
    let mut strokes = 0usize;
    let mut holes = 0usize;
    let mut reached: Vec<(usize, usize)> = Vec::new();
    for row in 0..deep {
        for column in 0..wide {
            if taken[row * wide + column] {
                continue;
            }
            // A region of ink is a stroke; a region of ground is a hole, unless it reaches
            // the edge of the mark's box, where it is the ground around the mark instead.
            let ink = inked[row * wide + column];
            let mut edge = false;
            taken[row * wide + column] = true;
            reached.push((column, row));
            let steps: &[(i32, i32)] = if ink {
                &[
                    (-1, -1),
                    (-1, 0),
                    (-1, 1),
                    (0, -1),
                    (0, 1),
                    (1, -1),
                    (1, 0),
                    (1, 1),
                ]
            } else {
                &[(-1, 0), (0, -1), (0, 1), (1, 0)]
            };
            while let Some((at_column, at_row)) = reached.pop() {
                edge = edge
                    || at_column == 0
                    || at_row == 0
                    || at_column + 1 == wide
                    || at_row + 1 == deep;
                for (step_row, step_column) in steps {
                    let next_column = at_column as i32 + step_column;
                    let next_row = at_row as i32 + step_row;
                    if next_column < 0
                        || next_row < 0
                        || next_column >= wide as i32
                        || next_row >= deep as i32
                    {
                        continue;
                    }
                    let cell = next_row as usize * wide + next_column as usize;
                    if inked[cell] == ink && !taken[cell] {
                        taken[cell] = true;
                        reached.push((next_column as usize, next_row as usize));
                    }
                }
            }
            match (ink, edge) {
                (true, _) => strokes += 1,
                (false, false) => holes += 1,
                (false, true) => {}
            }
        }
    }
    Shape { strokes, holes }
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
            .chain(
                OUTLINE_FACES
                    .iter()
                    .map(|(face, size)| outline_face(face, *size)),
            )
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

/// How one weight and pixel size of the PROPORTIONAL outline face draws one character,
/// measured in the frame of its own line.
///
/// The face is an outline rather than a bitmap, so each glyph is rasterised here at the size
/// asked for: `fontdue` hands back one byte of ink per pixel of the glyph's own ink box,
/// with the metrics that place that box against the line's baseline. Every glyph of one
/// size is therefore measured on ONE line, exactly as the bitmap families' are.
///
/// A face this emulator cannot parse contributes no template rather than a wrong one.
fn outline_face(bytes: &'static [u8], size: f32) -> Box<dyn Fn(char) -> Option<Mark>> {
    let settings = fontdue::FontSettings {
        scale: size,
        ..fontdue::FontSettings::default()
    };
    let Ok(font) = fontdue::Font::from_bytes(bytes, settings) else {
        return Box::new(|_| None);
    };
    let ascent = font
        .horizontal_line_metrics(size)
        .map_or(size, |line| line.ascent);
    // Where the foot of a capital falls in the frame below, and the room left above it for
    // an accent and below it for a descender: a glyph's own raster is placed against this,
    // so two glyphs of one face stand on the same row whatever each inks.
    let pad = size.ceil().max(1.0) as isize;
    let baseline = pad + ascent.round() as isize;
    let placed = move |ch: char| {
        let (metrics, bitmap) = font.rasterize(ch, size);
        if metrics.width == 0 || metrics.height == 0 {
            return None;
        }
        let (width, height) = (metrics.width, metrics.height);
        // `ymin` is how far the foot of the glyph's ink box stands above the baseline, so
        // its top row is that much more than its own height above it.
        let top = baseline - (height as isize + metrics.ymin as isize);
        let coverage = move |x: isize, y: isize| {
            let row = y - top;
            if x < 0 || row < 0 || x >= width as isize || row >= height as isize {
                return 0.0;
            }
            bitmap
                .get(row as usize * width + x as usize)
                .map_or(0.0, |ink| f32::from(*ink) / 255.0)
        };
        Some((coverage, width, (top.max(0) as usize) + height))
    };
    let line = placed('H').and_then(|(ink, width, rows)| font_line(&ink, width, rows));
    Box::new(move |ch: char| {
        let line = line?;
        let (ink, width, rows) = placed(ch)?;
        let own = extent(&ink, width, rows)?;
        Some(Mark::measure(&ink, own, line))
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

/// How wide the widest character the bank holds a template for stands, in the capital
/// heights of the line that sets it.
///
/// Measured off the bank itself rather than fixed in advance: a proportional face sets a
/// capital W a third wider than the capitals it stands beside, so a ceiling below that
/// would refuse the widest letters of the very faces this recogniser is asked to name. It
/// is therefore the hard bound on what one character can measure — ink wider than this is
/// more than one — and never a reason to refuse a character the bank could have named.
pub(super) fn widest_glyph() -> f32 {
    static WIDEST: OnceLock<f32> = OnceLock::new();
    *WIDEST.get_or_init(|| {
        bank()
            .iter()
            .flat_map(|held| held.faces.iter())
            .map(|face| face.width)
            .fold(1.0, f32::max)
    })
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
    recognise_region(ink, *glyph, line, &|_, _| true)
}

/// The same, for a mark that is one CONNECTED region of a run's ink: `own` says which
/// pixels of the mark's rectangle its own strokes reach, so the ink of a mark whose box
/// overlaps this one's is not measured as part of it.
pub(super) fn recognise_region(
    ink: &Ink,
    glyph: GlyphBox,
    line: Line,
    own: &dyn Fn(isize, isize) -> bool,
) -> Option<Named> {
    if line.cap < 1.0 {
        return None;
    }
    let page = |x: isize, y: isize| {
        if own(x, y) {
            ink.coverage(x, y)
        } else {
            0.0
        }
    };
    let mark = Mark::measure(&page, (glyph.x0, glyph.x1, glyph.y0, glyph.y1), line);
    name_mark(&mark)
}

/// What a mark could be: one character of the bank, and how well the mark's ink agreed with
/// the closest face of that character.
#[derive(Debug, Clone, Copy)]
struct Candidate {
    ch: char,
    /// Which character of the bank this is, so the ink as that character lays it down can
    /// settle a reading the blurred grids left tied without measuring every candidate's.
    held: usize,
    /// The agreement the answer is scored by, on the blurred grids and less what the two
    /// marks' shapes stand apart — or the floor it was measured against, where the character
    /// agrees no better than that ([`score_against`]).
    score: f32,
}

/// How well `mark`'s ink agrees with the character `glyph`, by the measure the answer is
/// scored by: its closest face, or `floor` where no face of it agrees better than that.
///
/// The measure takes the character's OWN best face, because a character is held in the bank
/// at several sizes and weights and the face whose blurred grid a mark meets best is not
/// always the one whose strokes land exactly where the mark's do: asking one face for both
/// this and [`sharp_against`] would score a character on a face that is a size away from the
/// page's, which is evidence about the bank rather than about the ink.
///
/// A face is measured only while it can still raise the answer: the best found so far is
/// what the next face has to beat, `floor` is what the first of them has to beat, and
/// `likeliest` — the face whose ink the mark shares most of as the grids stand — is measured
/// before the rest, so the figure the others are held against is nearly the answer from the
/// start (see [`Grid::best_shifted_above`]).
fn score_against(mark: &Mark, glyph: &Glyph, floor: f32, likeliest: usize) -> f32 {
    let mut best = floor;
    for face in glyph
        .faces
        .get(likeliest)
        .into_iter()
        .chain(glyph.faces.iter())
    {
        let apart = SHAPE_APART * mark.shape_apart(face);
        // What this face has to beat is the best found so far and the shapes' own distance
        // on top of it, and the answer is raised only where it really was beaten: a floor a
        // face failed to reach is reported as the floor itself rather than as what adding
        // and subtracting that distance rounded it to.
        let agreed = mark.agreement_above(face, best + apart);
        if agreed > best + apart {
            best = agreed - apart;
        }
    }
    best
}

/// The same on the grids BEFORE they were blurred: how much of the mark's ink and the
/// character's is the same ink exactly where each lays it down, over that character's own
/// closest face, or `floor` where no face of it shares more than that.
fn sharp_against(mark: &Mark, glyph: &Glyph, floor: f32) -> f32 {
    let mut best = floor;
    for face in &glyph.faces {
        best = best.max(mark.sharp_agreement_above(face, best));
    }
    best
}

/// Which of `candidates` a mark is named as: the character its ink agrees with most, and
/// where several agree with it as closely as the blurred grids can tell ([`TIED`]), the one
/// of those whose ink the mark really shares most of.
///
/// The blur is what lets a stroke that lands half a cell off its template's meet it at all,
/// and it is also what makes a `t` and an `l`, a `D` and an `O`, a `6` and an `8` the same
/// ink: so where it cannot separate two characters, the ink as each of them lays it down
/// does ([`sharp_against`]). The score reported is the agreement itself, which is what every
/// threshold downstream of the naming is measured in.
///
/// Only the characters the blurred grids left TIED are measured on their sharp ink, and each
/// of those only while it can still beat the sharpest found: what the ink as drawn is asked
/// is which of the tied characters it shares most of, and a character the blur already
/// separated is not one of them.
fn settled(mark: &Mark, candidates: &[Candidate]) -> Option<Candidate> {
    let best = candidates
        .iter()
        .copied()
        .reduce(|held, found| if found.score > held.score { found } else { held })?;
    let mut chosen: Option<Candidate> = None;
    let mut sharpest = 0.0f32;
    for candidate in candidates
        .iter()
        .filter(|candidate| candidate.score >= best.score - TIED)
    {
        let sharp = sharp_against(mark, &bank()[candidate.held], sharpest);
        if chosen.is_none() || sharp > sharpest {
            chosen = Some(*candidate);
            sharpest = sharp;
        }
    }
    Some(chosen.unwrap_or(best))
}

/// The character `mark` is drawn with, over the bank, by the three steps of [`recognise`].
fn name_mark(mark: &Mark) -> Option<Named> {
    if mark.fine.ink <= 0.0 {
        return None;
    }
    let named = candidates(mark);
    let best = settled(mark, &named)?;
    if best.score < MIN_MATCH {
        return None;
    }
    // Whether the shape left the reading OPEN: a bar is not a character of a word at all,
    // and where the mark is a stem the shape may not tell the stems apart — the best of the
    // others it could be scores as well.
    let open = best.ch == '|'
        || (STEMS.contains(&best.ch)
            && named
                .iter()
                .filter(|candidate| candidate.ch != best.ch && STEMS.contains(&candidate.ch))
                .any(|candidate| best.score - candidate.score <= STEM_MARGIN));
    // The best LETTER OR FIGURE the same ink could be, for an open reading inside a word to
    // be settled by: a mark whose ink agrees best with a bar, or with one bare stem over
    // another, may be a character whose own ink is nearly a stem — the `t` of a face whose
    // crossbar barely reaches past it — and that character is what such a mark reads as.
    let letter = named
        .iter()
        .filter(|candidate| candidate.ch.is_ascii_alphanumeric() && candidate.score >= MIN_MATCH)
        .reduce(|held, found| if found.score > held.score { found } else { held })
        .map(|candidate| candidate.ch);
    Some(Named {
        ch: best.ch,
        score: best.score,
        open,
        letter,
    })
}

/// The characters `mark` could be, each with how well its ink agreed with that character's
/// closest face: the [`CLOSE_LOOK`] the two narrowing steps keep.
///
/// Each step measures a character only while it can still change the step's answer: the
/// poorest character the shortlist holds is what the next one has to beat to enter it, and a
/// character that cannot reach [`NAMING_FLOOR`] enters no reading at all, so the agreement
/// kept for it is that floor rather than the exact figure.
fn candidates(mark: &Mark) -> Vec<Candidate> {
    let bank = bank();
    let mut shortlist = Ranking::<usize, SHORTLIST>::new();
    for (index, candidate) in bank.iter().enumerate() {
        let apart = candidate
            .faces
            .iter()
            .map(|face| mark.proportion_distance(face))
            .fold(f32::MAX, f32::min);
        shortlist.offer(-apart, index);
    }
    let mut close = Ranking::<(usize, usize), CLOSE_LOOK>::new();
    for index in shortlist.taken() {
        let mut scored = close.poorest().unwrap_or(0.0);
        // Which of the character's faces the mark's ink is closest to, as the grids stand:
        // the face the close look below measures FIRST, so that what every other face of
        // that character has to beat is already nearly the answer when it is measured.
        let mut closest = 0usize;
        for (at, face) in bank[index].faces.iter().enumerate() {
            let agreed = mark.unshifted_agreement_above(face, scored);
            if agreed > scored {
                scored = agreed;
                closest = at;
            }
        }
        close.offer(scored, (index, closest));
    }
    close
        .taken()
        .into_iter()
        .map(|(index, closest)| Candidate {
            ch: bank[index].ch,
            held: index,
            score: score_against(mark, &bank[index], NAMING_FLOOR, closest),
        })
        .collect()
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
    /// The best LETTER OR FIGURE this mark's ink could be, when its ink agreed with one at
    /// all: what the mark is read as where the reading is open and the mark stands inside a
    /// word, since the words of a page are set with letters and figures and a BAR is not one
    /// of them.
    pub(super) letter: Option<char>,
}

/// The characters a face may draw as nothing but a stem: the capital i, the small l, the
/// figure one and the bar. Which of them a face gives a serif, a tail or a flag differs from
/// face to face, so a mark that is a bare stem is every one of them at once.
pub(super) const STEMS: &[char] = &['I', 'l', '1', '|'];

/// How close the best of the other [`STEMS`] must score for a mark's shape to have left
/// which of them it is OPEN: a hair, since the four are the same ink only where the face
/// draws them so.
const STEM_MARGIN: f32 = 0.04;

/// How close to the ninth decile of a line's mark heights a mark must stand to count as one
/// of the line's own CAPITALS when its height is measured ([`line_of`]): within a tenth,
/// which is the pixel or two a round letter overshoots a flat capital by at the sizes a page
/// is set at.
const NEAR_CAP: f32 = 0.9;

/// The `N` best-scoring items offered to it, best first: how each step of [`recognise`]
/// hands the next one the characters worth a closer look.
struct Ranking<T, const N: usize> {
    held: Vec<(f32, T)>,
}

impl<T, const N: usize> Ranking<T, N> {
    fn new() -> Self {
        Self {
            held: Vec::with_capacity(N + 1),
        }
    }

    fn offer(&mut self, score: f32, item: T) {
        if self.held.len() == N && self.held.last().is_some_and(|(worst, _)| score <= *worst) {
            return;
        }
        let at = self.held.partition_point(|(held, _)| *held > score);
        self.held.insert(at, (score, item));
        self.held.truncate(N);
    }

    /// The score of the POOREST item held, once the ranking is full: what an item has to
    /// beat to enter it at all, so a comparison that cannot reach this need not be finished.
    fn poorest(&self) -> Option<f32> {
        (self.held.len() == N)
            .then(|| self.held.last().map(|(score, _)| *score))
            .flatten()
    }

    fn taken(self) -> Vec<T> {
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
    // Saturating: the band's own rows are what this is measured over, so a band with
    // nothing between its bounds is one row rather than a height no page has.
    let band = Line {
        baseline: (bottom + 1) as f32,
        cap: (bottom.saturating_sub(top) + 1) as f32,
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
        (Some(baseline), Some(_)) => {
            // The height of the capitals this line STANDS on the baseline: the marks whose
            // own foot is the baseline's row. A face measures its capitals by a flat letter
            // — which is what every template here is framed by — while the few marks that
            // descend through the baseline (a `J`, a `Q`, a comma) reach further down than
            // any capital of the line does.
            let standing: Vec<f32> = kept
                .iter()
                .filter(|glyph| ((glyph.y1 + 1) as f32 - baseline).abs() < 1.0)
                .map(|glyph| glyph.height() as f32)
                .collect();
            // What the TALL marks of the line agree on, rather than the tallest of them:
            // the ninth decile is a capital's height on a line that holds one, and the
            // median of the marks standing near it is the height the line's flat capitals
            // really measure — a round letter overshoots them by a pixel at the size a page
            // is set at, and framing the line by that pixel measures every mark on it
            // against a capital height no letter of it has.
            let cap = super::quantile(&standing, 0.9)
                .map(|tall| {
                    let agreed: Vec<f32> = standing
                        .iter()
                        .copied()
                        .filter(|height| {
                            *height >= NEAR_CAP * tall && *height <= tall / NEAR_CAP
                        })
                        .collect();
                    super::quantile(&agreed, 0.5).unwrap_or(tall)
                })
                .or_else(|| super::quantile(&kept_heights, 0.9))
                .unwrap_or(0.0);
            if cap >= 1.0 {
                Line { baseline, cap }
            } else {
                band
            }
        }
        _ => band,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every character of the bank is named as itself when the ink of its own face is what
    /// the recogniser is given: the comparison names a mark after the character it is drawn
    /// as, at the sizes a page is set at, including the sizes between two the bank holds
    /// templates at.
    #[test]
    fn a_glyph_of_a_face_is_named_as_the_character_it_draws() {
        // Sizes BETWEEN the rasters `OUTLINE_FACES` holds, so each row is matched against
        // templates whose stroke weight and counters are a size away from its own.
        for size in [19.0f32, 25.0, 31.0] {
            let face = outline_face(ttf_noto_sans::REGULAR, size);
            for ch in ('A'..='Z').chain('0'..='9') {
                let Some(mark) = face(ch) else {
                    panic!("case {ch:?} at {size}: the outline face must draw it");
                };
                let named = name_mark(&mark);
                assert_eq!(
                    named.map(|found| found.ch),
                    Some(ch),
                    "case {ch:?} at {size} pixels: a glyph must be named as the character it \
                     draws, got {:?}",
                    candidates(&mark)
                        .iter()
                        .map(|other| (other.ch, (other.score * 100.0) as i32))
                        .collect::<Vec<_>>()
                );
            }
        }
    }

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
                FACES.len() + PIXEL_FACES.len() + OUTLINE_FACES.len() + 1,
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
                        // Measured only as far as it can still be the closest of the other
                        // characters, which is all this asks of it: `best` is what it has to
                        // beat ([`Grid::best_shifted_above`]).
                        let scored = drawn.agreement_above(other_drawn, best);
                        if scored > best {
                            best = scored;
                            best_ch = other.ch;
                        }
                    }
                }
                let own = drawn.agreement_above(drawn, 0.0);
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
