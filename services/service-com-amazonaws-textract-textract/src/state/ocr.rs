// SPDX-License-Identifier: CC0-1.0

//! Reading the text out of a RASTER document: the pixel side of
//! [`super::read_content`].
//!
//! Textract accepts a document "in PNG, JPEG, PDF, or TIFF format"; a raster in one of
//! those formats carries its text as ink rather than as characters, so it has to be
//! recognised. Nothing here is echoed from the request, keyed to a digest or
//! randomised: every line, word and character this module reports was measured off the
//! submitted pixels, and ink it cannot match to a glyph is reported as nothing at all.
//!
//! ## The pipeline
//!
//! 1. **Decode** — the `image` crate decodes the submitted bytes for the format its
//!    magic bytes name, bounded by [`MAX_RASTER_SIDE`], [`MAX_RASTER_PIXELS`] and
//!    [`MAX_RASTER_ALLOC`] so one request cannot turn a small compressed file into
//!    unbounded work.
//! 2. **Measure the ink** — Otsu's method (N. Otsu, 1979: the threshold that maximises
//!    the between-class variance of the grey histogram) splits the page into ink and
//!    ground. Ink is the SMALLER of the two classes, so a light page with dark text and a
//!    dark page with light text are both read; two classes closer together than
//!    [`MIN_INK_CONTRAST`] are sensor or compression noise on a blank page, not ink. Each
//!    pixel then carries how far it lies from the ground's own level towards the ink's,
//!    rather than a bit: the text of a page rendered at any ordinary size is
//!    anti-aliased, and a stroke of a small glyph can be nothing but its own edges.
//! 3. **Segment** — a horizontal projection profile of the ink gives the page's text
//!    lines; a vertical profile within one line gives its marks, and the way those marks
//!    are spaced — against the cell a monospaced line sets them in, or against the pitch a
//!    proportional one advances by — gives the word and column breaks ([`Separators`]).
//!    The page's RULES are struck out of its ink first ([`rules`]): a long thin straight run
//!    of ink running clear past the page's own characters is a ruling line, a border or an
//!    underline — page structure, which no face draws a character as — while a rule standing
//!    between two marks of a line is the boundary of the two CELLS they stand in.
//!    Where a line sets its FIGURES on one advance, as a proportional face does, two of them
//!    are separated by what their step covers past that advance rather than by the gap a
//!    narrow figure leaves inside its own cell ([`figure_advance`]).
//!    A run of inked columns is a candidate for ONE character, but at the size a page is
//!    drawn at it can hold two whose strokes touch, two whose boxes merely overlap — a
//!    proportional face sets letters whose ink reaches back under the letter before them
//!    ([`Ink::regions_in`]) — or be what is left of one whose thin stroke broke: which it
//!    is, is settled by NAMING ([`settle`]).
//! 4. **Recognise** — each mark is resampled into a frame anchored on the line it sits in
//!    and scored against every printable ASCII character of a bank of typefaces — the
//!    embedded 8x8 font, a pre-rasterised sans-serif face at several weights and sizes, the
//!    X11 fixed bitmap faces, and a proportional outline face rasterised at the sizes a page
//!    is set at — by how much of its ink and the character's is the same ink ([`glyphs`]).
//!    The best match above [`glyphs::MIN_MATCH`] is the character;
//!    the mean winning score is the `Confidence` the blocks report, because it is the only
//!    confidence anything here actually measured — per word, per line and per page, each
//!    over the characters that block was read from.
//! 5. **Orient and separate** — steps 3 and 4 are run over the page as submitted AND over
//!    each of its quarter turns ([`Ink::turned`]), because a document can be scanned or
//!    photographed turned. The reading that accounts for most of the page's own ink
//!    ([`PageRead::strength`]) is the page's own, and the angle it was read at is what the
//!    `WORD` blocks report as `Geometry$RotationAngle`. Each turn is read once more every
//!    way something that is NOT a character can be joining the marks of a line — a rule
//!    drawn across it, a panel of light text on a dark ground ([`Ink::variants`]) — and
//!    again the reading that accounts for most of the ink is the one reported.
//! 6. **Measure** — every line and every word also reports WHERE its ink was found, as
//!    the fraction of the SUBMITTED page it covers ([`MeasuredLine`], [`MeasuredWord`],
//!    mapped back out of the turn it was read at by [`Orientation`]), so the `Geometry`
//!    the blocks carry is the position and the size the recogniser measured rather than a
//!    typographic grid. Each measurement is both the box around the ink and its
//!    [`Outline`] — the four extreme marks themselves — so a block can report the
//!    fine-grained polygon around its item as well as the coarse box. The page's own
//!    CONTENT is measured the same way ([`Ink::content`]): the extent and the outline of
//!    every mark of ink found on it, which is what a `PAGE` block reports, so a page whose
//!    content sits in one band of the submitted canvas reports that band rather than the
//!    canvas.
//!
//! No maintained Rust crate performs offline OCR: the ones that exist either bind to a
//! C library or load neural-network weights from a file this emulator may not fetch, so
//! the recogniser itself is in-crate while the decoding (a format job) is the `image`
//! crate's, the typefaces it holds templates for are the font crates' (`font8x8`,
//! `noto-sans-mono-bitmap`, `embedded-graphics`, `ttf-noto-sans`) and the rasterising of the
//! outline face is `fontdue`'s.
//!
//! ## What it does not read
//!
//! One raster is ONE page. A multi-frame TIFF's later frames are not reported, because
//! the decoder hands this module a single image and a page nothing decoded is not a page
//! this service can describe; `DocumentMetadata.Pages` then says 1, which is what was
//! read. A PDF is not a raster and is not decoded here at all.
//!
//! ## Where it runs
//!
//! Every step here is bounded, but all of it is CPU-bound over a page whose size the
//! caller chose, so a request path reaches it through [`super::read_document`], which
//! runs it on tokio's BLOCKING pool: it occupies no async worker, and the
//! `spawn_blocking(..).await` that drives it is a real await point a timeout, a
//! cancellation or a shutdown can land on.
//!
//! One page is read several ways — a fixed four quarter turns, each read every way a rule
//! or a panel could be joining its marks, so at most a dozen readings — and each reading is
//! held to the same per-page ceilings ([`MAX_RASTER_PIXELS`], [`MAX_GLYPHS_PER_PAGE`]). The
//! work and the memory one request can cause therefore stay a fixed multiple of one page's,
//! which no request can choose.

use std::io::Cursor;

use image::{ImageFormat, Limits};

use super::Unreadable;

mod glyphs;
mod rules;
mod spacing;

use rules::Rule;

/// The largest side, in pixels, of a raster this service decodes. NOT an AWS limit: an
/// emulator-side bound so one request cannot allocate without end.
pub const MAX_RASTER_SIDE: u32 = 10_000;

/// The most pixels one raster page may hold. NOT an AWS limit: see [`MAX_RASTER_SIDE`].
/// The scan is linear in the pixel count, so this bounds the per-request CPU too.
pub const MAX_RASTER_PIXELS: u64 = 8_000_000;

/// The most bytes the decoder may allocate for one raster. NOT an AWS limit: it is what
/// stops a small compressed file from expanding into a large allocation.
pub const MAX_RASTER_ALLOC: u64 = 96 * 1024 * 1024;

/// The most glyph candidates one READING of a page may hold: the runs of inked columns its
/// lines fall into. NOT an AWS limit: it bounds the segmentation, which is linear in them.
/// A page of ordinary text holds a few thousand characters.
pub const MAX_GLYPHS_PER_PAGE: usize = 2_500;

/// The most marks one REQUEST may name, over every reading of its page. NOT an AWS limit:
/// naming one mark costs a comparison against every character of the recogniser's bank, so
/// this is what bounds the work a single request can cause.
///
/// It bounds the PRODUCT of the three factors that multiply, which no per-factor ceiling
/// does: how many marks a page's lines fall into ([`MAX_GLYPHS_PER_PAGE`]), how many pieces
/// of a run the segmentation names to settle where its characters end ([`cut_apart`],
/// [`join`]), and how many ways one page is read (four quarter turns, each read again for
/// what might be joining the marks of its lines). A page whose reading would cost more
/// naming than this is reported as too large rather than ground through: see [`Budget`].
///
/// It is set so that a page of ordinary text is read at every one of its turns and still
/// has naming to spare — a page of [`MAX_GLYPHS_PER_PAGE`] marks read at four turns is
/// 10,000 of them, with the rest for the pieces the segmentation considers — while a page
/// crafted to make every run of ink look like several characters at every turn runs out.
pub const MAX_NAMINGS_PER_PAGE: usize = 30_000;

/// How much of a page's ink one reading of it must have accounted for ([`PageRead::
/// strength`]) for that reading to settle what is ON the page: there is then nothing left
/// for a reading that takes a rule or a panel out of it to find, so those readings are not
/// taken at all. NOT an AWS limit: it is what keeps the ordinary page — read as it stands —
/// from costing the readings an obscured one needs.
const SETTLED_READING: f32 = 0.3;

/// How far apart the two Otsu classes must be, in grey levels, for the darker one to be
/// ink. Below this the page holds one tone plus noise, which is a blank page.
const MIN_INK_CONTRAST: f32 = 24.0;

/// How much ink a pixel must carry to be part of a mark at all, as a fraction of the
/// ink a stroke of this page's own text carries: the segmentation reads the page's marks
/// through this floor, so an anti-aliased edge is part of the stroke it belongs to while
/// the halftone a lossy coding leaves around it is not.
const INK_FLOOR: f32 = 0.34;

/// How much better than the page as submitted a quarter turn must read for the page to be
/// reported as read at that turn, as a fraction of the recognition score. A turn that only
/// ties with the upright reading is a coincidence, not an orientation.
const MIN_ROTATION_GAIN: f32 = 0.05;

/// How much better than the page as submitted a QUARTER turn must read for the page to be
/// reported as read at that turn, as a fraction of what each accounted for.
///
/// More than a half turn needs ([`MIN_ROTATION_GAIN`]): a quarter turn reads across the
/// page's lines rather than along them, so it describes a different page — one line of text
/// becomes a column of slivers of several — and the marks it names are the blobs those
/// slivers make rather than the characters that were drawn. A page that really was scanned
/// at a quarter turn reads far better that way than upright, where its lines of text are
/// themselves the slivers.
const MIN_QUARTER_TURN_GAIN: f32 = 0.5;

/// How many characters a WORD must hold for the ink it was read from to count towards how
/// well the page was read ([`PageRead::strength`]): two, since what a word is, as against a
/// sliver of several lines read across them, is characters side by side.
const MIN_WORD_CHARACTERS: usize = 2;

/// How much of the page's ink a QUARTER or a HALF turn of it must itself have accounted for
/// ([`PageRead::strength`]) to be the turn the page is reported as read at.
///
/// A page whose marks are no characters at all — a drawing, a page in a face nothing here
/// matches — reads badly at every turn, and the turn that reads it least badly is a
/// coincidence, not a discovery: reporting it would turn such a page sideways and split its
/// one line of marks into a column of slivers. A page that really was scanned turned reads
/// WELL at its own turn, which is what this asks for.
const MIN_ALTERNATIVE_READING: f32 = 0.2;

/// How much better than the page as it stands a reading that takes a rule or a panel out of
/// it must read for that reading to be reported, as a fraction of what was accounted for.
///
/// A page that carries no rule and no panel reads much the same either way, and the
/// difference between the two readings is then the odd mark one of them happens to join or
/// separate: taking the one that is a hair better would report a page of marks that are no
/// characters as a panel of light text, split into as many lines as its counters make. A
/// page that really does carry a rule or a panel reads far better once it is out.
const MIN_VARIANT_GAIN: f32 = 0.1;

/// How much of the line's own character pitch a gap must cover to separate two WORDS on a
/// PROPORTIONAL line whose gaps show no boundary of their own: a space leaves a gap of a
/// good part of a pitch, while a gap inside a word is a letter's side bearing, a small part
/// of one.
const WORD_GAP_PITCH: f32 = 0.4;

/// The least of the line's own character pitch a gap must cover to be a space at all, which
/// is what keeps the boundary between two clusters of BEARINGS from being read as one
/// between bearings and spaces.
const MIN_SPACE_PITCH: f32 = 0.25;

/// How many passes a line's word gap is settled in ([`Separators::word_gap`]). NOT an AWS
/// limit: each pass takes in more of the line's gaps than the last, so a few of them is
/// where the boundary stops moving on any line a page sets.
const GAP_PASSES: usize = 4;

/// How much of the line's own character pitch the step from one FIGURE to the next must
/// cover PAST the advance the face sets its figures on for a space to stand between them.
///
/// A figure's own cell is the advance, so two figures of one number step by the advance
/// give or take where each inks inside it, while two figures with a space between them step
/// by the advance plus the space's own width — about a quarter of an em, where a figure's
/// advance is well over half of one. This sits between the two: above what inking a cell
/// differently moves a figure by, and below the narrowest space a face sets.
const FIGURE_SPACE_PITCH: f32 = 0.2;

/// How many of the line's own character pitches a gap must cover to separate two COLUMNS
/// rather than two words on a PROPORTIONAL line.
const COLUMN_GAP_PITCH: f32 = 2.0;

/// How many cells a step must cover to separate two WORDS on a MONOSPACED line: a letter
/// advances one cell and a space's step covers two, so the boundary sits between them.
/// Measured between the CELLS the two marks sit in ([`GlyphBox::cell_step_from`]), which is
/// what makes a whole number of cells a whole number of cells whatever each glyph inks
/// inside its own.
const LATTICE_WORD_CELLS: f32 = 1.5;

/// How many cells a step must cover to separate two COLUMNS rather than two words on a
/// MONOSPACED line.
const LATTICE_COLUMN_CELLS: f32 = 3.0;

/// How far from a whole number of cells, in cells, a monospaced line's marks may sit on
/// average. A cell holds a glyph's ink wherever the glyph puts it, so its centre wanders
/// inside the cell; a line whose marks wander further than this sits on no lattice, which
/// is what a line whose every letter advances by its own width does.
const MAX_LATTICE_DRIFT: f32 = 0.08;

/// Marks a line must carry for a lattice fit to say anything about it: a line of two marks
/// has one step, and one step fits any cell.
const MIN_LATTICE_MARKS: usize = 3;

/// How far either side of the line's own pitch a lattice cell is searched for, in
/// sixty-fourths of that pitch: the median step of a monospaced line is its cell, give or
/// take where two glyphs put their ink inside it, and a cell free to be anything else
/// would fit a proportional line by coincidence.
const LATTICE_SEARCH: i32 = 8;

/// Rows a run of inked rows must span to be a line of text rather than speckle.
const MIN_LINE_HEIGHT: usize = 2;

/// How much of a band's own width one row's single run of ink must span for that row to be
/// a RULE drawn across the line rather than part of a character.
const RULE_ROW_SHARE: f32 = 0.6;

/// How many times wider than it is tall a band must be to be read as a line of characters
/// with something drawn ACROSS it.
///
/// A rule runs across a line, and a panel holds one, so both are longer than the line is
/// tall, while no Latin character is twice as wide as its own line: a band narrower than
/// this holds one mark of the page's own ink, whose bar, crossbar or counters are no rule
/// and no text.
const MIN_RULE_BAND_ASPECT: f32 = 2.0;

/// The least a filled region may measure on a side, in pixels, to be read as a PANEL of
/// light text: a shorter or narrower one holds no character.
const MIN_PANEL_SIDE: usize = 8;

/// How much of a line's height a column's ink may cover for that column to be a HAIRLINE:
/// a sliver of ink no character's body leaves, which is what a bridge between two
/// characters looks like.
const MAX_HAIRLINE_SHARE: f32 = 0.08;

/// How much of its own box a band's ink must fill for the band to be a filled PANEL rather
/// than a line of characters: characters leave most of their line blank, and even the
/// heaviest single character leaves more of its own box blank than this.
const MIN_PANEL_FILL: f32 = 0.6;

/// Marks the ground inside a filled region must fall into for the region to be read as a
/// panel of light text rather than as one mark of its own: the counters and notches a
/// single heavy character leaves are fewer than this.
const MIN_PANEL_MARKS: usize = 3;

/// A run no taller than this that inks more than half the page's width is a rule or a
/// border, not a line of text.
const RULE_MAX_HEIGHT: usize = 3;

/// The most indent characters a line's left margin is reported as, so a page-wide
/// margin cannot stretch one line into a long string.
const MAX_INDENT: usize = 100;

/// How far short of what the marks of its own line match their characters at a run's own
/// name may fall before the segmentation looks inside it for the characters it holds.
///
/// Two letters whose ink runs together are a run no wider than one wide capital as often as
/// not — a narrow letter set against its neighbour — so width alone does not give them away.
/// What does is the NAME: the character such a run is named as is not the ink it carries,
/// and it matches a clear margin worse than the marks of the same line that are one
/// character each. Measured against the line's own marks rather than an absolute score, so
/// a page in a face nothing here is a copy of — where every mark matches middlingly — is
/// read as it stands instead of being looked into mark by mark.
const WEAK_MARK_MATCH: f32 = 0.1;

/// How wide one mark of a line may be, in that line's capital heights, before the
/// segmentation looks for a cut inside it: no Latin character is wider than its own
/// capitals by this much, so a run of ink that is holds more than one character.
const MAX_MARK_WIDTH: f32 = 1.2;

/// How wide one mark of a line may be, as a multiple of the width of that line's OWN marks,
/// before the segmentation looks for a cut inside it.
///
/// A run has to be too wide BOTH ways to be cut: the capital height of a line is estimated
/// off the marks it carries, so a line of nothing but x-height letters sets it low and every
/// mark on such a line looks wide against it — while against the line's own marks none of
/// them does.
const MAX_MARK_PITCH: f32 = 1.6;

/// The most places inside one run of ink the segmentation considers cutting it at. NOT an
/// AWS limit: naming one piece costs a comparison against the recogniser's bank, and the
/// pieces of a run are named over every sequence of these places, so this bounds the work
/// one run of ink can cause.
const MAX_CUT_CANDIDATES: usize = 20;

/// The most CONNECTED regions one run of ink may fall into for those regions to be read as
/// its marks ([`Ink::regions_in`]). NOT an AWS limit: naming one region costs a comparison
/// against the recogniser's bank. A run of a line's own characters falls into a handful of
/// them; one that falls into more is speckle or a dither pattern, which is read as the ink
/// it stands as.
const MAX_REGIONS: usize = 8;

/// The most pixels a run of ink may cover for its connected regions to be found at all. NOT
/// an AWS limit: finding them walks the run's own pixels, so this is what keeps the work and
/// the memory one run of ink can cause bounded whatever the page's size. A run holding two
/// characters of a line covers a few thousand pixels at the sizes a page is set at; a run
/// that covers a large part of the page is ink of another kind, which the cuts read as it
/// stands.
const MAX_REGION_PIXELS: usize = 1 << 18;

/// How narrow a piece of a cut run may be, in its line's capital heights: narrower than the
/// narrowest character a line sets, so two cuts that close together would separate a
/// character's own strokes rather than two characters.
const MIN_PIECE_WIDTH: f32 = 0.12;

/// How wide a mark may be, in its line's capital heights, to be a PIECE of a character
/// rather than a character: what is left of a letter whose thin stroke fell under the floor
/// that tells ink from ground.
const MAX_PIECE_WIDTH: f32 = 0.55;

/// How far apart, in its line's capital heights, two marks may be and still be pieces of
/// one character: the break a stroke leaves is a column or two, while the gap between two
/// letters is their own side bearings.
const MAX_PIECE_GAP: f32 = 0.14;

/// What a unit of ink that named no character is worth to the segmentation: a small cost, so
/// a cut or a join that leaves ink unaccounted for is taken only when what it does name is
/// worth more — and ink that is no character at all (a rule, a logo) is still left as it
/// stands rather than cut up until something matches it.
const UNNAMED_MARK: f32 = -0.08;

/// How tall a piece of a cut run must be, in its line's capital heights: as tall as a
/// character of that line, since what a cut separates is two of them.
const MIN_PIECE_HEIGHT: f32 = 0.45;

/// How much of what a run's own columns carry a column may carry and still be the
/// narrowing BETWEEN two characters: no more than they do on average, since the inner
/// strokes of an M or a W carry as much as its outer ones and a cut there would read one
/// letter as two.
const CUT_VALLEY: f32 = 1.0;

/// What cutting a run of ink at a column costs, per unit of STROKE the cut crosses there
/// over its line's own capital height: see the cut cost in [`cut_apart`]. A cut through the
/// halo between two characters crosses next to no stroke and costs next to nothing; one
/// through the middle of a character crosses all of it and costs about what naming that
/// character is worth.
const CUT_COST: f32 = 0.2;

/// How much better the pieces of a cut run must account for its ink than the whole run
/// does, per unit of that ink, for the cut to be taken: the mean match of the pieces must
/// beat the whole's by this much.
///
/// A margin rather than a hair, because a run of ink can always be cut into pieces that
/// match SOMETHING a little better than the whole matched what it is: what the margin asks
/// for is the clear improvement that separating two letters really gives.
const CUT_GAIN: f32 = 0.01;

/// How far past the column a run is cut at, in its line's capital heights, a piece's own
/// strokes are followed for the ink they continue into ([`Ink::piece_in`]): about a quarter
/// of a character's width, which is as far as the bar of a `T`, the arm of an `L` or the
/// crossbar of a `t` reaches over the character beside it.
const OVERHANG_REACH: f32 = 0.25;

/// How thick a run of inked rows may be, in its line's capital heights, to be a STROKE that
/// reaches over the character beside it ([`Ink::piece_in`]): the bar of a `T`, the arm of an
/// `L` or the hook of a `J` is a stroke of a face at any weight, while the stem, the bowl or
/// the diagonal a column of a character's own BODY crosses is thicker than this and reaches
/// over nothing.
const OVERHANG_THICK: f32 = 0.3;

/// How much better a PIECE of a cut run must agree with a character over its OWN ink than
/// over that ink together with the stroke the cut severed for the piece to be named over its
/// own ink alone ([`name_piece`]).
///
/// A margin rather than a hair: a character whose own stroke runs on over the columns of the
/// one beside it — the bar of a `T`, the arm of an `L` — is complete inside its own columns
/// and agrees with its character clearly better there than it does with the ink of its
/// neighbour taken in as well, while a character whose stroke the cut cut short agrees with
/// the character the rest of it is shaped like about as well as the whole of its stroke
/// agrees with its own.
const SEVERED_STROKE_MARGIN: f32 = 0.05;

/// How closely a mark of PUNCTUATION must be named for a division of a run that is itself
/// named as a letter or a figure to leave it ([`LookInside::punctuation`]): the ink must be
/// that mark rather than merely be shaped a little like it.
const PUNCTUATION_PIECE_MATCH: f32 = 0.78;

/// How much better than the whole run's own name a mark of PUNCTUATION a division of it
/// leaves must be named, beside [`PUNCTUATION_PIECE_MATCH`].
///
/// A letter whose own strokes a division would cut into the punctuation they are shaped like
/// — the two diagonals of an `X`, the arcs of an `S`, the bowl and tail of a `g` — is named
/// as that letter about as well as those pieces are named as their marks, while a figure set
/// tight against a mark the page really drew — a point inside an amount, a solidus inside a
/// date — leaves a mark whose ink agrees with it a clear margin better than the joined ink
/// agreed with any one character.
const PUNCTUATION_PIECE_GAIN: f32 = 0.12;

/// How much better than the run's OWN name every piece of a cut must be named for a run no
/// wider than one character of its line to be cut at all.
///
/// Such a run is cut on the evidence that it holds two characters, and the evidence is that
/// each piece reads as a character of this line better than the whole read as one. A margin
/// rather than a hair: the halves of a letter whose face sets it with a thin waist — the bowl
/// and the tail of a `g`, the two arcs of an `s` — each match some mark of punctuation about
/// as well as the whole matches its own letter, and reading a letter as two of those is a
/// worse answer than leaving it whole.
const CUT_PIECE_GAIN: f32 = 0.08;

/// A raster page's recognised text.
#[derive(Debug, Clone, PartialEq)]
pub struct Recognised {
    /// The page's text, one line per line of ink the recogniser read.
    pub text: String,
    /// The mean score of the characters recognised, as a percentage inside `Percent`'s
    /// `0..=100` range. Zero when the page carries no recognised text.
    pub confidence: f32,
    /// WHERE the recognised text sits on the page, one entry per line of `text` and in
    /// the same order, each with the words of that line in reading order.
    ///
    /// Every rectangle here was MEASURED off the submitted pixels — a word is the extent
    /// of the glyph run the line's text separates with a space or a tab, and a line the
    /// box enclosing its words — so the geometry the blocks report describes the document
    /// rather than a page template.
    pub lines: Vec<MeasuredLine>,
    /// The angle, in degrees, the page's characters were read at: `0.0` for a page read as
    /// it was submitted, and the quarter turn its characters are turned by when the page
    /// had to be turned to read them.
    pub rotation: f32,
    /// WHERE the content detected on the page sits: the box enclosing every mark of ink
    /// found on it and the outline of its extreme marks, both as fractions of the
    /// submitted page's own width and height.
    ///
    /// Measured over ALL of the page's ink, the marks no glyph was matched to included —
    /// a rule, a border, a logo is content of the page as much as a character is — so it
    /// is the extent of what was detected rather than the extent of what was recognised.
    /// `None` for a page no ink was found on at all, where nothing was detected to
    /// measure.
    pub content: Option<Marks>,
}

/// Where something sits on a page, in the page-relative fractions `BoundingBox` and
/// `Point` are expressed in: `0.0` is the page's left or top edge and `1.0` its right or
/// bottom edge.
#[derive(Debug, Clone, PartialEq)]
pub struct Rect {
    pub left: f32,
    pub top: f32,
    pub width: f32,
    pub height: f32,
}

impl Rect {
    /// The whole page: what was examined when there is no ink to bound.
    pub const fn whole_page() -> Self {
        Self {
            left: 0.0,
            top: 0.0,
            width: 1.0,
            height: 1.0,
        }
    }
}

/// One point of a page, in the same page-relative fractions as [`Rect`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Vertex {
    pub x: f32,
    pub y: f32,
}

/// The OUTLINE of a set of marks: the four extreme marks of the set — the topmost, the
/// rightmost, the bottommost and the leftmost mark — as points on the submitted page.
///
/// Each of them is a mark that was really found, so the outline is inscribed in the box
/// around the same marks and touches each of its edges, and it follows the set's own skew
/// instead of restating the box's corners: a line of text whose last glyph sits lower
/// than its first outlines a slanted quadrilateral, while a solid rectangle of ink
/// outlines its own rectangle.
pub type Outline = [Vertex; 4];

/// Where a set of marks was found on the page: the box enclosing them and their outline.
///
/// `BoundingBox` is "an axis-aligned COARSE representation of the location of the
/// recognized item" and `Polygon` is, "within the bounding box, a FINE-GRAINED polygon
/// around the recognized item", so a block needs both, measured over the same marks.
#[derive(Debug, Clone, PartialEq)]
pub struct Marks {
    pub rect: Rect,
    pub outline: Outline,
}

/// One word of a recognised line: where its ink sits, and the mean score its own
/// characters were recognised with.
#[derive(Debug, Clone, PartialEq)]
pub struct MeasuredWord {
    pub marks: Marks,
    pub confidence: f32,
}

/// One recognised line: the mean score its characters were recognised with, and its
/// words — each with its own ink — in reading order. The line's own extent is the box
/// enclosing its words, which is where the block layer takes it from.
#[derive(Debug, Clone, PartialEq)]
pub struct MeasuredLine {
    pub confidence: f32,
    pub words: Vec<MeasuredWord>,
}

/// A mean score as the percentage `Confidence` reports, inside `Percent`'s `0..=100`
/// range. Zero when nothing was recognised, because nothing was measured.
fn percentage(score_total: f32, score_count: usize) -> f32 {
    if score_count == 0 {
        0.0
    } else {
        (score_total / score_count as f32 * 100.0).clamp(0.0, 100.0)
    }
}

/// `Some(format)` when these bytes are a raster document format Textract reads on the
/// channel they were submitted on.
///
/// The format comes from the bytes' own magic numbers, never from a declared name. Which
/// formats a channel accepts is AWS's own split (see [`super::DocumentChannel`]): PNG and
/// JPEG are read whichever way the document arrived, while a TIFF is a document only an
/// Amazon S3 object may hold — `Document$Bytes` takes PNG and JPEG alone.
pub fn raster_format(bytes: &[u8], channel: super::DocumentChannel) -> Option<ImageFormat> {
    match image::guess_format(bytes) {
        Ok(format @ (ImageFormat::Png | ImageFormat::Jpeg)) => Some(format),
        Ok(ImageFormat::Tiff) if channel == super::DocumentChannel::S3Object => {
            Some(ImageFormat::Tiff)
        }
        _ => None,
    }
}

/// The name of the raster format these bytes DECLARE, whatever it is: `image`'s own
/// sniffing of the magic number at their head.
///
/// `None` says the bytes declare no raster format the sniffer knows — not that they are a
/// document. It is what lets [`super::read_content`] tell a document in a raster format
/// the service does not process from bytes that are not a raster at all.
pub fn declared_raster_format(bytes: &[u8]) -> Option<&'static str> {
    image::guess_format(bytes).ok().map(format_name)
}

/// Read the text of one raster document.
///
/// `Err` says why the document could not be read, with the variant the handlers answer
/// with the matching modelled error.
pub fn read_raster(bytes: &[u8], format: ImageFormat) -> Result<Recognised, Unreadable> {
    let name = format_name(format);
    // The header is read with the decoder's own size limits OFF, so a page over this
    // emulator's ceiling is refused as too large (what it is) rather than as unreadable.
    let (width, height) = reader(bytes, format, false)
        .into_dimensions()
        .map_err(|error| undecodable(name, &error))?;
    let pixels = u64::from(width) * u64::from(height);
    if width > MAX_RASTER_SIDE || height > MAX_RASTER_SIDE || pixels > MAX_RASTER_PIXELS {
        return Err(Unreadable::TooLarge(format!(
            "the document is {width}x{height} pixels; this emulator reads at most \
             {MAX_RASTER_SIDE} pixels on a side and {MAX_RASTER_PIXELS} pixels in all"
        )));
    }

    let decoded = reader(bytes, format, true)
        .decode()
        .map_err(|error| undecodable(name, &error))?;
    let gray = decoded.into_luma8();
    // The ceilings again, on the frame that was actually decoded: the scan, the
    // segmentation and the ink mask are all sized from THIS image, so the budget must not
    // depend on the header having agreed with it.
    if gray.width() > MAX_RASTER_SIDE
        || gray.height() > MAX_RASTER_SIDE
        || u64::from(gray.width()) * u64::from(gray.height()) > MAX_RASTER_PIXELS
    {
        return Err(Unreadable::TooLarge(format!(
            "the document decodes to {}x{} pixels; this emulator reads at most \
             {MAX_RASTER_SIDE} pixels on a side and {MAX_RASTER_PIXELS} pixels in all",
            gray.width(),
            gray.height()
        )));
    }
    let ink = Ink::of(&gray);
    // WHERE the page's content was detected, measured once off the page as it was
    // submitted: the ink of a page is the same ink whichever turn its characters are read
    // at, so the content's extent does not depend on the reading below.
    let content = ink.content(Orientation::upright(&ink));
    // What this request may spend on NAMING marks, over every reading of the page below:
    // one budget for the whole of it, so the readings cannot multiply the work (see
    // [`MAX_NAMINGS_PER_PAGE`]).
    let mut budget = Budget::new();
    // A document can be scanned or photographed turned, and a page's characters are only
    // recognisable at the angle they were drawn at, so the page is read as submitted AND at
    // each of its quarter turns. The reading of the page AS SUBMITTED stands unless the best
    // of the turns reads it clearly better, so a page with little ink, or a page of marks
    // that are no characters at all, is never reported sideways on a coincidence.
    let upright = read_every_way(&ink, Orientation::upright(&ink), &mut budget)?;
    let mut turned: Option<(u8, PageRead)> = None;
    for turns in 1..4u8 {
        let orientation = Orientation::turned(&ink, turns);
        let read = read_every_way(&ink.turned(turns), orientation, &mut budget)?;
        if turned
            .as_ref()
            .is_none_or(|(_, held)| read.strength() > held.strength())
        {
            turned = Some((turns, read));
        }
    }
    let best = match turned {
        // A HALF turn of a page of text is still a page of lines of text, so the two
        // readings are comparable mark for mark and the better of them decides. A QUARTER
        // turn reads ACROSS the page's lines instead of along them, which turns one line of
        // text into a column of slivers of several: a reading like that has to be clearly
        // better, not a little better, to be the page's own.
        Some((turns, read))
            if read.describes_better(
                &upright,
                if turns == 2 {
                    MIN_ROTATION_GAIN
                } else {
                    MIN_QUARTER_TURN_GAIN
                },
            ) =>
        {
            read
        }
        _ => upright,
    };

    Ok(Recognised {
        confidence: percentage(best.score_total, best.score_count),
        text: best.text,
        lines: best.lines,
        rotation: best.rotation,
        content,
    })
}

/// Read one turn of a page every way something that is NOT a character can be joining the
/// marks of its lines — a rule drawn across a line, a panel of light text on a dark ground
/// ([`Ink::variants`]) — and report the reading whose marks recognise best.
///
/// Every reading is of the same pixels, so a page that carries no such mark reads the same
/// either way: there is nothing in it to take out.
fn read_every_way(
    ink: &Ink,
    orientation: Orientation,
    budget: &mut Budget,
) -> Result<PageRead, Unreadable> {
    let mut best = recognise_page(ink, orientation, budget)?;
    // A page whose ink is already accounted for carries nothing to take out of it.
    if best.strength() >= SETTLED_READING {
        return Ok(best);
    }
    for variant in ink.variants() {
        let read = recognise_page(&variant, orientation, budget)?;
        // Clearly better, not a hair better: see [`MIN_VARIANT_GAIN`].
        if read.strength() > best.strength() * (1.0 + MIN_VARIANT_GAIN) {
            best = read;
        }
    }
    Ok(best)
}

/// What is left of the naming one request may do, over every reading of its page.
///
/// Every mark the recogniser names is charged here, wherever the naming happens: the marks
/// a line's runs of ink fall into, the pieces a run is cut into to settle where its
/// characters end, and the pairs a join considers. One budget serves the whole request, so
/// the factors that would otherwise multiply — marks per reading, pieces per run, readings
/// per page — are bounded by their PRODUCT, and a page that would cost more than
/// [`MAX_NAMINGS_PER_PAGE`] is reported as too large instead of being ground through.
///
/// A count rather than a clock: the same document then reads the same way on a fast machine
/// and a slow one, which a wall-clock deadline would not give.
#[derive(Debug)]
struct Budget {
    left: usize,
}

impl Budget {
    fn new() -> Self {
        Self {
            left: MAX_NAMINGS_PER_PAGE,
        }
    }

    /// Charge one naming, or say the page is too large to read.
    fn spend(&mut self) -> Result<(), Unreadable> {
        self.left = self.left.checked_sub(1).ok_or_else(|| {
            Unreadable::TooLarge(format!(
                "reading the document's marks would take more than \
                 {MAX_NAMINGS_PER_PAGE} comparisons; this emulator makes at most that many \
                 for one page"
            ))
        })?;
        Ok(())
    }
}

/// One page read at ONE orientation: its text, what it was recognised with, and where its
/// words sit on the page as it was submitted.
struct PageRead {
    text: String,
    score_total: f32,
    score_count: usize,
    /// How much of the page's ink was named, and how well: see [`PageRead::strength`].
    named_ink: f32,
    /// How much ink the page carries for its text to be read out of, which that is measured
    /// against: the ink that is left once its RULES are struck, since a ruling line is
    /// structure rather than a mark any reading could name ([`rules`]).
    page_ink: f32,
    lines: Vec<MeasuredLine>,
    rotation: f32,
}

impl PageRead {
    /// How well this reading of the page holds up as a whole, which is what one reading of
    /// a page is chosen over another by.
    ///
    /// The share of the page's OWN INK that was named, weighted by how far each mark's
    /// match beat the floor every reported character has to clear ([`glyphs::MIN_MATCH`]).
    /// Weighing the ink rather than counting the characters is what makes two readings of
    /// one page comparable: the same pixels carry the same ink whichever turn they are read
    /// at, so a page read at the turn its characters were drawn at — where nearly all of
    /// its ink is in marks that match their glyphs well — stands above the same page read
    /// sideways, where the slivers a turned line of text falls into match whatever they
    /// match barely at all, however many of them there are.
    ///
    /// Only the ink of a WORD of several characters counts ([`MIN_WORD_CHARACTERS`]):
    /// characters come in words, so a reading that falls into single marks standing apart —
    /// which is what a page read ACROSS its own lines falls into — has accounted for none of
    /// it, however well each of those marks happens to match something.
    fn strength(&self) -> f32 {
        if self.page_ink <= 0.0 {
            return 0.0;
        }
        (self.named_ink / self.page_ink).max(0.0)
    }

    /// How well the characters this reading named agree with the ink they were named from:
    /// the mean score over them, or zero where it named none.
    ///
    /// What tells a page read at the turn it was DRAWN at from the same ink read at another
    /// turn: the marks of a page read along its own characters are the letterforms of the
    /// face it was set in and agree with them closely, while the same ink turned agrees
    /// middlingly with whatever its shapes happen to resemble, however much of the page
    /// those shapes cover.
    fn agreement(&self) -> f32 {
        if self.score_count == 0 {
            return 0.0;
        }
        self.score_total / self.score_count as f32
    }

    /// Whether this reading describes the page better than the reading `than` of the same
    /// ink, which it has to by the margin `gain` to be reported in its place.
    ///
    /// Both of what a reading is judged on, because the two say different things: the share
    /// of the page's ink the reading accounted for ([`Self::strength`]), and how well the
    /// characters it named agree with that ink ([`Self::agreement`]). A reading that covers
    /// more of the page while agreeing with it WORSE has not read the page at another turn —
    /// it has named turned ink after the characters it middlingly resembles — so it does not
    /// stand in place of the reading taken along the page's own characters.
    fn describes_better(&self, than: &Self, gain: f32) -> bool {
        self.strength() >= MIN_ALTERNATIVE_READING
            && self.strength() > than.strength() * (1.0 + gain)
            && self.agreement() >= than.agreement()
    }
}

/// Read the lines of ink of `ink`, which is the submitted page turned as `orientation`
/// says, measuring every word back in the frame of the page as it was submitted.
///
/// The page's RULES are read first and struck out of the ink its characters are read from:
/// a ruling line is page structure rather than text ([`rules`]), so it is never named as a
/// character, never joins the marks of the row it bounds into one, and never stands in the
/// frame, the pitch or the word separation the characters beside it are measured against —
/// while a rule standing between two marks of a line is what tells that row's cells apart.
fn recognise_page(
    ink: &Ink,
    orientation: Orientation,
    budget: &mut Budget,
) -> Result<PageRead, Unreadable> {
    let ruled = ink.rules();
    let struck = ink.without(&ruled);
    let ink = struck.as_ref().unwrap_or(ink);
    let lines = ink.text_lines()?;

    let mut text = String::new();
    let mut measured: Vec<MeasuredLine> = Vec::new();
    let mut score_total = 0.0f32;
    let mut score_count = 0usize;
    let mut named_ink = 0.0f32;
    for line in &lines {
        let Some(rendered) = line.render(ink, &ruled, budget)? else {
            continue;
        };
        text.push_str(&rendered.text);
        text.push('\n');
        score_total += rendered.score_total;
        score_count += rendered.score_count;
        named_ink += rendered.named_ink;
        measured.push(rendered.measured(ink, orientation));
    }

    Ok(PageRead {
        text,
        score_total,
        score_count,
        named_ink,
        page_ink: ink.total_ink(),
        lines: measured,
        rotation: orientation.angle(),
    })
}

/// How a page was TURNED to read it, and the page's own size, which every measurement
/// taken at this orientation is expressed in.
///
/// A page read at a quarter turn is segmented and recognised in the turned frame, so every
/// rectangle it yields has to be mapped back out of that turn before it can describe the
/// document the caller submitted.
#[derive(Debug, Clone, Copy)]
struct Orientation {
    /// Clockwise quarter turns applied to the submitted page before it was segmented.
    turns: u8,
    /// The submitted page's width in pixels.
    width: usize,
    /// The submitted page's height in pixels.
    height: usize,
}

impl Orientation {
    /// Reading the page as it was submitted.
    fn upright(ink: &Ink) -> Self {
        Self::turned(ink, 0)
    }

    /// Reading the submitted page after `turns` clockwise quarter turns.
    fn turned(ink: &Ink, turns: u8) -> Self {
        Self {
            turns: turns % 4,
            width: ink.width,
            height: ink.height,
        }
    }

    /// The angle, in degrees, the characters read at this orientation are turned by ON the
    /// submitted page: turning the page clockwise to read it upright means its characters
    /// are themselves turned the other way.
    ///
    /// `Geometry$RotationAngle` is "a numerical value corresponding to the rotation of the
    /// WORD block. Possible values are 0, 90, 180, and 270", which is exactly the set of
    /// quarter turns a page is read at.
    fn angle(self) -> f32 {
        f32::from(u16::from((4 - self.turns) % 4)) * 90.0
    }

    /// Where the pixel `(x, y)` of the TURNED page sits on the submitted one.
    fn source(self, x: usize, y: usize) -> (usize, usize) {
        let last_x = self.width.saturating_sub(1);
        let last_y = self.height.saturating_sub(1);
        match self.turns {
            1 => (y, last_y.saturating_sub(x)),
            2 => (last_x.saturating_sub(x), last_y.saturating_sub(y)),
            3 => (last_x.saturating_sub(y), x),
            _ => (x, y),
        }
    }

    /// The rectangle `x0..=x1` by `y0..=y1` of the TURNED page, as a fraction of the page
    /// as it was submitted.
    fn rect(self, x0: usize, x1: usize, y0: usize, y1: usize) -> Rect {
        let (ax, ay) = self.source(x0, y0);
        let (bx, by) = self.source(x1, y1);
        let (left, right) = (ax.min(bx), ax.max(bx));
        let (top, bottom) = (ay.min(by), ay.max(by));
        let page_width = self.width.max(1) as f32;
        let page_height = self.height.max(1) as f32;
        Rect {
            left: (left as f32 / page_width).clamp(0.0, 1.0),
            top: (top as f32 / page_height).clamp(0.0, 1.0),
            width: ((right + 1 - left) as f32 / page_width).clamp(0.0, 1.0),
            height: ((bottom + 1 - top) as f32 / page_height).clamp(0.0, 1.0),
        }
    }

    /// Where the point `(u, v)` of the TURNED page — each a fraction of that page's own
    /// width and height — sits on the page as it was submitted.
    ///
    /// Turning the page back maps its corners onto the submitted page's: a quarter turn
    /// takes the turned page's top edge to one of the submitted page's sides, so a point
    /// measured while reading a turned page still names the pixel of the submitted page it
    /// was measured at.
    fn vertex(self, u: f32, v: f32) -> Vertex {
        let (x, y) = match self.turns {
            1 => (v, 1.0 - u),
            2 => (1.0 - u, 1.0 - v),
            3 => (1.0 - v, u),
            _ => (u, v),
        };
        Vertex {
            x: x.clamp(0.0, 1.0),
            y: y.clamp(0.0, 1.0),
        }
    }
}

/// The refusal for a document in an accepted container whose bytes the decoder cannot
/// read (a truncated or otherwise undecodable file).
///
/// Such a document is one no page can be read out of, which is what
/// [`Unreadable::UnsupportedFormat`] says, and that refusal carries no reason of its own:
/// what the decoder tripped over is a detail of this emulator's reader, so it is logged
/// here instead of being answered to the caller.
fn undecodable(name: &str, error: &image::ImageError) -> Unreadable {
    ::tracing::debug!(
        format = name,
        error = %error,
        "the document's bytes could not be decoded"
    );
    Unreadable::UnsupportedFormat
}

fn format_name(format: ImageFormat) -> &'static str {
    match format {
        ImageFormat::Png => "PNG",
        ImageFormat::Jpeg => "JPEG",
        ImageFormat::Tiff => "TIFF",
        // Reached through `declared_raster_format`, which names whatever format the
        // sniffer recognised so a refusal can be logged with it.
        other => other.extensions_str().first().copied().unwrap_or("raster"),
    }
}

/// A decoder for `bytes`, bounded so a small file cannot decode into a large one.
///
/// `bound_dimensions` turns the decoder's own width/height ceilings on, for the decode
/// itself; the header read leaves them off so this module can tell a page that is too
/// large from one that cannot be read at all.
fn reader(
    bytes: &[u8],
    format: ImageFormat,
    bound_dimensions: bool,
) -> image::ImageReader<Cursor<&[u8]>> {
    let mut reader = image::ImageReader::with_format(Cursor::new(bytes), format);
    // `Limits::no_limits()` rather than `default()`: the bounds below are this
    // emulator's, and starting from the crate's defaults would silently inherit them.
    let mut limits = Limits::no_limits();
    if bound_dimensions {
        limits.max_image_width = Some(MAX_RASTER_SIDE);
        limits.max_image_height = Some(MAX_RASTER_SIDE);
    }
    limits.max_alloc = Some(MAX_RASTER_ALLOC);
    reader.limits(limits);
    reader
}

// ---------------------------------------------------------------------------
// Binarisation
// ---------------------------------------------------------------------------

/// A page's ink: how much of it each pixel carries, from `0` for the page's own ground to
/// `255` for a pixel as dark as its text's own strokes.
///
/// The strength rather than a bit, because the text of a page rendered at a size at all
/// near the pixel grid is ANTI-ALIASED: the edge of every stroke is drawn part way between
/// the ink and the ground, and a stroke of a small glyph can be nothing but such edges. A
/// mark is segmented through [`INK_FLOOR`], so a stroke stays one mark, while the
/// recogniser measures the strengths themselves, so what it compares against a glyph is
/// the stroke's own weight.
struct Ink {
    width: usize,
    height: usize,
    ink: Vec<u8>,
}

/// The four EXTREME marks of a set of pixels — the topmost, the rightmost, the bottommost
/// and the leftmost — accumulated as the pixels are scanned.
///
/// One scan yields both what a block reports: the box around the marks is their bounds,
/// and the fine-grained outline of the block is the extremes themselves. Ties go to the
/// mark furthest along the page's own reading order, so one set of marks has one outline.
#[derive(Debug, Default)]
struct Extremes {
    top: Option<(usize, usize)>,
    right: Option<(usize, usize)>,
    bottom: Option<(usize, usize)>,
    left: Option<(usize, usize)>,
}

impl Extremes {
    /// Take the mark at `(x, y)` into the four extremes.
    fn mark(&mut self, x: usize, y: usize) {
        if self.top.is_none_or(|(tx, ty)| y < ty || (y == ty && x < tx)) {
            self.top = Some((x, y));
        }
        if self.right.is_none_or(|(rx, ry)| x > rx || (x == rx && y < ry)) {
            self.right = Some((x, y));
        }
        if self.bottom.is_none_or(|(bx, by)| y > by || (y == by && x > bx)) {
            self.bottom = Some((x, y));
        }
        if self.left.is_none_or(|(lx, ly)| x < lx || (x == lx && y > ly)) {
            self.left = Some((x, y));
        }
    }

    /// The pixel bounds `x0..=x1` by `y0..=y1` of every mark taken in, or `None` when none
    /// was: the leftmost and rightmost mark give the columns, the topmost and bottommost
    /// the rows.
    fn bounds(&self) -> Option<(usize, usize, usize, usize)> {
        Some((self.left?.0, self.right?.0, self.top?.1, self.bottom?.1))
    }
}

impl Ink {
    /// Measure how much ink each pixel of a grey page carries.
    ///
    /// Otsu's method splits the page into its ink and its ground; the SMALLER class is the
    /// ink, so a light page with dark text and a dark page with light text are both read.
    /// Each pixel then carries the share of the way from the ground's own level to the
    /// ink's that it lies, which is `1` for a pixel as solid as the text's strokes and a
    /// fraction for the anti-aliased edge of one.
    fn of(gray: &image::GrayImage) -> Self {
        let width = gray.width() as usize;
        let height = gray.height() as usize;
        let mut histogram = [0u64; 256];
        for pixel in gray.pixels() {
            histogram[pixel.0[0] as usize] += 1;
        }
        let total: u64 = histogram.iter().sum();
        let ink = match otsu(&histogram, total) {
            // One tone plus noise: a blank page carries no ink at all.
            None => vec![0u8; width * height],
            Some(split) => {
                let dark: u64 = histogram[..=split.threshold as usize].iter().sum();
                let ink_is_dark = dark * 2 <= total;
                let (darker, lighter) = levels(&histogram, split.threshold);
                let (ground, ink_level) = if ink_is_dark {
                    (lighter, darker)
                } else {
                    (darker, lighter)
                };
                let reach = if (ink_level - ground).abs() >= 1.0 {
                    ink_level - ground
                } else {
                    1.0
                };
                gray.pixels()
                    .map(|pixel| {
                        let value = f32::from(pixel.0[0]);
                        let strength = ((value - ground) / reach).clamp(0.0, 1.0);
                        (strength * 255.0).round() as u8
                    })
                    .collect()
            }
        };
        Self {
            width,
            height,
            ink,
        }
    }

    /// Whether `(x, y)` is part of a mark: see [`INK_FLOOR`].
    fn at(&self, x: usize, y: usize) -> bool {
        self.strength(x, y) >= INK_FLOOR
    }

    /// How much ink the pixel `(x, y)` carries, as a fraction of a solid stroke's.
    fn strength(&self, x: usize, y: usize) -> f32 {
        if x >= self.width || y >= self.height {
            return 0.0;
        }
        self.ink
            .get(y * self.width + x)
            .map_or(0.0, |ink| f32::from(*ink) / 255.0)
    }

    /// The same, for a point that may lie off the page altogether: what the recogniser
    /// measures a mark's frame through, since a frame reaches past the mark it is around.
    fn coverage(&self, x: isize, y: isize) -> f32 {
        if x < 0 || y < 0 {
            return 0.0;
        }
        self.strength(x as usize, y as usize)
    }

    /// This page turned `turns` quarter turns clockwise, so text drawn at that turn reads
    /// along the rows of the result.
    fn turned(&self, turns: u8) -> Self {
        let orientation = Orientation::turned(self, turns);
        let (width, height) = if orientation.turns % 2 == 1 {
            (self.height, self.width)
        } else {
            (self.width, self.height)
        };
        let mut ink = vec![0u8; width * height];
        for y in 0..height {
            for x in 0..width {
                let (sx, sy) = orientation.source(x, y);
                ink[y * width + x] = self.raw(sx, sy);
            }
        }
        Self {
            width,
            height,
            ink,
        }
    }

    /// The ink of one pixel as it is stored, for a copy of this page that moves it.
    fn raw(&self, x: usize, y: usize) -> u8 {
        if x >= self.width || y >= self.height {
            return 0;
        }
        self.ink.get(y * self.width + x).copied().unwrap_or(0)
    }

    /// Where the ink inside `x0..=x1` by `y0..=y1` sits on the submitted page: the box
    /// enclosing it, and the outline of the marks themselves.
    ///
    /// The outline's four points are the four EXTREME marks of that ink — the topmost, the
    /// rightmost, the bottommost and the leftmost mark found, each at the corner of its own
    /// pixel that faces away from the others. Every one of them is a mark that is really
    /// there, so the outline lies inside the box and touches each of its edges, and it
    /// follows the ink's own shape: the marks of a line whose glyphs do not all start at
    /// its top outline a slanted quadrilateral rather than the box's four corners.
    ///
    /// A rectangle with no ink in it at all cannot be outlined, so its own corners are
    /// reported: that is the whole of what was measured for it.
    fn marks(&self, x0: usize, x1: usize, y0: usize, y1: usize, orientation: Orientation) -> Marks {
        let right = x1.min(self.width.saturating_sub(1));
        let bottom = y1.min(self.height.saturating_sub(1));
        let mut extremes = Extremes::default();
        for y in y0..=bottom {
            for x in x0..=right {
                if self.at(x, y) {
                    extremes.mark(x, y);
                }
            }
        }
        Marks {
            rect: orientation.rect(x0, right, y0, bottom),
            outline: self.outline_of(&extremes, (x0, right, y0, bottom), orientation),
        }
    }

    /// Where the ink of the WHOLE page sits on it: the box enclosing every mark found on
    /// the page, and the outline of its extreme marks.
    ///
    /// What the page's own content was detected at, measured exactly as every other
    /// block's region is ([`Self::marks`]) and over every mark of the page's ink — the ink
    /// no glyph was matched to included, since a rule, a border or a logo is content of
    /// the page as much as a character is.
    ///
    /// `None` for a page that carries no ink at all: nothing was detected on it, so there
    /// is nothing to have measured.
    fn content(&self, orientation: Orientation) -> Option<Marks> {
        let mut extremes = Extremes::default();
        for y in 0..self.height {
            for x in 0..self.width {
                if self.at(x, y) {
                    extremes.mark(x, y);
                }
            }
        }
        let bounds @ (x0, x1, y0, y1) = extremes.bounds()?;
        Some(Marks {
            rect: orientation.rect(x0, x1, y0, y1),
            outline: self.outline_of(&extremes, bounds, orientation),
        })
    }

    /// The extreme marks of `extremes` as points on the submitted page, falling back to
    /// the corners of `x0..=x1` by `y0..=y1` for a rectangle no mark was found in.
    fn outline_of(
        &self,
        extremes: &Extremes,
        (x0, x1, y0, y1): (usize, usize, usize, usize),
        orientation: Orientation,
    ) -> Outline {
        let page_width = self.width.max(1) as f32;
        let page_height = self.height.max(1) as f32;
        // A pixel covers the cell `x..x+1` by `y..y+1` of the page, which is the same cell
        // `Orientation::rect` measures the box over, so a corner of the outline and the
        // edge of the box it touches are the same number.
        let corner = |(x, y): (usize, usize), rightwards: bool, downwards: bool| {
            orientation.vertex(
                (x + usize::from(rightwards)) as f32 / page_width,
                (y + usize::from(downwards)) as f32 / page_height,
            )
        };
        [
            corner(extremes.top.unwrap_or((x0, y0)), false, false),
            corner(extremes.right.unwrap_or((x1, y0)), true, false),
            corner(extremes.bottom.unwrap_or((x1, y1)), true, true),
            corner(extremes.left.unwrap_or((x0, y1)), false, true),
        ]
    }

    fn row_inked(&self, y: usize) -> bool {
        (0..self.width).any(|x| self.at(x, y))
    }

    /// The bands of inked rows of this page, top to bottom: one per line of marks.
    fn bands(&self) -> Vec<(usize, usize)> {
        let mut out = Vec::new();
        let mut y = 0usize;
        while y < self.height {
            if !self.row_inked(y) {
                y += 1;
                continue;
            }
            let top = y;
            while y < self.height && self.row_inked(y) {
                y += 1;
            }
            out.push((top, y - 1));
        }
        out
    }

    /// The leftmost and the rightmost inked column of the band `top..=bottom`.
    fn ink_columns(&self, top: usize, bottom: usize) -> Option<(usize, usize)> {
        let inked = |x: usize| (top..=bottom).any(|y| self.at(x, y));
        let left = (0..self.width).find(|x| inked(*x))?;
        let right = (left..self.width).rfind(|x| inked(*x))?;
        Some((left, right))
    }

    /// Which rows of the band `top..=bottom` carry a RULE: a single run of ink spanning
    /// most of the band's own width ([`RULE_ROW_SHARE`]).
    ///
    /// A rule drawn ACROSS a line — an underline, a cell border, a strike-through — joins
    /// every character it runs through into one mark, and a panel of light text on a dark
    /// ground is such a run in every one of its rows.
    fn rule_rows(&self, top: usize, bottom: usize, left: usize, right: usize) -> Vec<bool> {
        let span = RULE_ROW_SHARE * (right + 1 - left) as f32;
        (top..=bottom)
            .map(|y| {
                let mut longest = 0usize;
                let mut run = 0usize;
                for x in left..=right {
                    if self.at(x, y) {
                        run += 1;
                        longest = longest.max(run);
                    } else {
                        run = 0;
                    }
                }
                longest as f32 >= span
            })
            .collect()
    }

    /// The maximal runs of columns the mask `inked` marks within the rectangle
    /// `left..=right` by `top..=bottom`, left to right, each with its own extent.
    fn runs_in(
        &self,
        left: usize,
        right: usize,
        top: usize,
        bottom: usize,
        inked: impl Fn(usize, usize) -> bool,
    ) -> Vec<GlyphBox> {
        let column = |x: usize| (top..=bottom).any(|y| inked(x, y));
        let mut out = Vec::new();
        let mut x = left;
        while x <= right {
            if !column(x) {
                x += 1;
                continue;
            }
            let x0 = x;
            while x <= right && column(x) {
                x += 1;
            }
            let x1 = x - 1;
            let mut y0 = bottom;
            let mut y1 = top;
            for y in top..=bottom {
                if (x0..=x1).any(|cx| inked(cx, y)) {
                    y0 = y0.min(y);
                    y1 = y1.max(y);
                }
            }
            out.push(GlyphBox { x0, x1, y0, y1 });
        }
        out
    }

    /// This page read a second way, where something that is NOT a character joins the
    /// marks of a line into one: a rule drawn across the line, a hairline of ink spread
    /// between two characters, or a panel of light text on a dark ground.
    ///
    /// Each is a reading of the same pixels, not a guess about them: the caller reads the
    /// page every way and keeps the reading whose marks recognise best, so a page that
    /// carries none of them is unaffected by all.
    fn variants(&self) -> Vec<Self> {
        let mut out = Vec::new();
        out.extend(self.without_rules());
        out.extend(self.without_hairlines());
        out.extend(self.panels_read());
        out
    }

    /// This page with the HAIRLINES that bridge the marks of a line cleared, or `None` when
    /// it carries none.
    ///
    /// A column of a line carrying ink over only a sliver of the line's height
    /// ([`MAX_HAIRLINE_SHARE`]) is no part of a character's body: it is what ink spread
    /// between two characters leaves — the bridge a heavy stroke, a coarse threshold or a
    /// lossy coding puts between them — and it joins them into one mark. Whether such a
    /// column IS a bridge is established the way a rule is: by clearing it, and keeping the
    /// result only where what remains falls into more marks than before.
    ///
    /// A bridge stands BETWEEN two characters, so what makes a sliver one is a character's
    /// own stroke on both sides of it inside the same run of ink. A sliver at the edge of a
    /// run is part of the character it belongs to — the bar of a `T`, the crossbar of a `t`,
    /// the arm of an `L`, a serif — and clearing it would trim that character to the one its
    /// stem alone is shaped like rather than separate anything.
    fn without_hairlines(&self) -> Option<Self> {
        let mut cleared: Option<Vec<u8>> = None;
        for (top, bottom) in self.bands() {
            let Some((left, right)) = self.ink_columns(top, bottom) else {
                continue;
            };
            let band = bottom + 1 - top;
            let sliver = MAX_HAIRLINE_SHARE * band as f32;
            // Measured once per column of the band, not once per look: the segmentation
            // below reads the mask a column at a time, over a page whose size the caller
            // chose.
            let inked: Vec<bool> = (left..=right)
                .map(|x| (top..=bottom).any(|y| self.at(x, y)))
                .collect();
            let slivers: Vec<bool> = (left..=right)
                .map(|x| {
                    let ink = (top..=bottom).filter(|y| self.at(x, *y)).count();
                    ink > 0 && (ink as f32) <= sliver
                })
                .collect();
            let hairlines = bridges(&inked, &slivers);
            if !hairlines.iter().any(|bridge| *bridge) {
                continue;
            }
            let joined = self.runs_in(left, right, top, bottom, |x, y| self.at(x, y));
            let apart = self.runs_in(left, right, top, bottom, |x, y| {
                self.at(x, y) && !hairlines[x - left]
            });
            if apart.len() <= joined.len() {
                continue;
            }
            let cleared_ink = cleared.get_or_insert_with(|| self.ink.clone());
            for (offset, _) in hairlines.iter().enumerate().filter(|(_, bridge)| **bridge) {
                for y in top..=bottom {
                    cleared_ink[y * self.width + left + offset] = 0;
                }
            }
        }
        cleared.map(|ink| Self {
            width: self.width,
            height: self.height,
            ink,
        })
    }

    /// This page with the RULES drawn across its lines taken out, or `None` when it carries
    /// none.
    ///
    /// Whether a long run of ink is a rule BETWEEN characters is established by taking it
    /// out: if what remains falls into more marks than before, the run was joining
    /// characters; if it does not, the ink was part of one — the bar of a 'T', the crossbar
    /// of an 'H' — and it stays. A band too narrow to hold a line of characters
    /// ([`MIN_RULE_BAND_ASPECT`]) carries no rule at all, for the same reason.
    ///
    /// This is the case [`rules`] cannot measure, which is why both survive: a rule that is
    /// told from a character by its own SHAPE — far out of square, and running clear past the
    /// page's characters — is struck off the page before it is ever segmented, while the ink
    /// taken out here is a run too short or too heavy for its shape to say, whose being a
    /// rule is established only by what taking it out does to the marks of its own band. This
    /// is offered as one READING of the page beside the page as it stands ([`Self::variants`])
    /// rather than settled in advance, because that is all its evidence supports.
    fn without_rules(&self) -> Option<Self> {
        let mut cleaned: Option<Vec<u8>> = None;
        for (top, bottom) in self.bands() {
            let Some((left, right)) = self.ink_columns(top, bottom) else {
                continue;
            };
            let band = bottom + 1 - top;
            if ((right + 1 - left) as f32) < MIN_RULE_BAND_ASPECT * band as f32 {
                continue;
            }
            let rules = self.rule_rows(top, bottom, left, right);
            if !rules.iter().any(|rule| *rule) {
                continue;
            }
            let joined = self.runs_in(left, right, top, bottom, |x, y| self.at(x, y));
            let apart = self.runs_in(left, right, top, bottom, |x, y| {
                self.at(x, y) && !rules[y - top]
            });
            if apart.len() <= joined.len() {
                continue;
            }
            let cleaned_ink = cleaned.get_or_insert_with(|| self.ink.clone());
            for (offset, _) in rules.iter().enumerate().filter(|(_, rule)| **rule) {
                for x in left..=right {
                    cleaned_ink[(top + offset) * self.width + x] = 0;
                }
            }
        }
        cleaned.map(|ink| Self {
            width: self.width,
            height: self.height,
            ink,
        })
    }

    /// This page with its PANELS read as the light text they hold, or `None` when it
    /// carries none.
    ///
    /// A band whose ink FILLS most of its own box ([`MIN_PANEL_FILL`]) is not a line of
    /// characters: characters leave most of their line blank. It is a filled panel, and the
    /// characters of such a panel are the ground it leaves inside its own edge — light text
    /// on a dark ground is text all the same — so the panel is read by taking its ground for
    /// its ink, and only where that ground falls into marks of its own.
    fn panels_read(&self) -> Option<Self> {
        let mut read: Option<Vec<u8>> = None;
        for (top, bottom) in self.bands() {
            let Some((left, right)) = self.ink_columns(top, bottom) else {
                continue;
            };
            let (band, width) = (bottom + 1 - top, right + 1 - left);
            if band < MIN_PANEL_SIDE || width < MIN_PANEL_SIDE {
                continue;
            }
            // A panel holds a LINE of text, so it is wider than it is tall. A band that is
            // not is one mark of the page's own ink — a heavy character, a mark of a
            // drawing — whose ground is the shape of that mark rather than text.
            if (width as f32) < MIN_RULE_BAND_ASPECT * band as f32 {
                continue;
            }
            let inked = (top..=bottom)
                .map(|y| (left..=right).filter(|x| self.at(*x, y)).count())
                .sum::<usize>();
            if (inked as f32) < MIN_PANEL_FILL * (band * width) as f32 {
                continue;
            }
            let (inner_left, inner_right) = (left + 1, right - 1);
            let (inner_top, inner_bottom) = (top + 1, bottom - 1);
            let ground = self.runs_in(inner_left, inner_right, inner_top, inner_bottom, |x, y| {
                !self.at(x, y)
            });
            if ground.len() < MIN_PANEL_MARKS {
                continue;
            }
            // The panel's own ground, inked as strongly as the panel was: light text on a
            // dark ground is drawn with the contrast between the two, which is the contrast
            // the page was measured by.
            let read_ink = read.get_or_insert_with(|| self.ink.clone());
            for y in top..=bottom {
                for x in left..=right {
                    let inside = (inner_left..=inner_right).contains(&x)
                        && (inner_top..=inner_bottom).contains(&y);
                    read_ink[y * self.width + x] = if inside && !self.at(x, y) {
                        u8::MAX
                    } else {
                        0
                    };
                }
            }
        }
        read.map(|ink| Self {
            width: self.width,
            height: self.height,
            ink,
        })
    }

    /// The page's lines of text, top to bottom, with the glyph candidates of each.
    fn text_lines(&self) -> Result<Vec<InkLine>, Unreadable> {
        let mut lines = Vec::new();
        let mut glyph_budget = MAX_GLYPHS_PER_PAGE;
        for (top, bottom) in self.bands() {
            if bottom + 1 - top < MIN_LINE_HEIGHT {
                continue;
            }
            let glyphs = self.glyphs_between(top, bottom);
            if glyphs.is_empty() {
                continue;
            }
            // A short, page-wide run of ink is a rule or a border, not text.
            let inked_width = glyphs.last().map_or(0, |g| g.x1) + 1 - glyphs[0].x0;
            if bottom + 1 - top <= RULE_MAX_HEIGHT && inked_width * 2 > self.width {
                continue;
            }
            if glyphs.len() > glyph_budget {
                return Err(Unreadable::TooLarge(format!(
                    "the document holds more than {MAX_GLYPHS_PER_PAGE} marks to recognise; \
                     this emulator reads at most that many on one page"
                )));
            }
            glyph_budget -= glyphs.len();
            lines.push(InkLine {
                top,
                bottom,
                glyphs,
            });
        }
        Ok(lines)
    }

    /// How much ink this page carries in all: the sum of every pixel's own strength.
    ///
    /// The same pixels carry the same ink whichever turn they are read at, which is what
    /// lets [`PageRead::strength`] compare two readings of one page.
    fn total_ink(&self) -> f32 {
        self.ink.iter().map(|ink| f32::from(*ink) / 255.0).sum()
    }

    /// How much ink the rectangle of `glyph` carries.
    fn ink_in(&self, glyph: &GlyphBox) -> f32 {
        let right = glyph.x1.min(self.width.saturating_sub(1));
        let bottom = glyph.y1.min(self.height.saturating_sub(1));
        (glyph.y0..=bottom)
            .map(|y| (glyph.x0..=right).map(|x| self.strength(x, y)).sum::<f32>())
            .sum()
    }

    /// The glyph candidates of the band `top..=bottom`: the maximal runs of inked
    /// columns, left to right, each with its own ink extent.
    fn glyphs_between(&self, top: usize, bottom: usize) -> Vec<GlyphBox> {
        self.runs_in(0, self.width.saturating_sub(1), top, bottom, |x, y| {
            self.at(x, y)
        })
    }

    /// The mark the columns `x0..=x1` of `band` hold: the rectangle their ink occupies, or
    /// `None` when they carry none.
    fn mark_in(&self, x0: usize, x1: usize, band: &InkLine) -> Option<GlyphBox> {
        let right = x1.min(self.width.saturating_sub(1));
        let bottom = band.bottom.min(self.height.saturating_sub(1));
        let mut found: Option<GlyphBox> = None;
        for y in band.top..=bottom {
            for x in x0..=right {
                if !self.at(x, y) {
                    continue;
                }
                found = Some(match found {
                    None => GlyphBox {
                        x0: x,
                        x1: x,
                        y0: y,
                        y1: y,
                    },
                    Some(box_so_far) => GlyphBox {
                        x0: box_so_far.x0.min(x),
                        x1: box_so_far.x1.max(x),
                        y0: box_so_far.y0.min(y),
                        y1: box_so_far.y1.max(y),
                    },
                });
            }
        }
        found
    }

    /// How much STROKE one column of `band` carries: the ink past the floor that tells a
    /// page's ink from its ground, which is what a cut through that column crosses.
    fn stroke_in_column(&self, x: usize, band: &InkLine) -> f32 {
        let bottom = band.bottom.min(self.height.saturating_sub(1));
        (band.top..=bottom)
            .map(|y| ((self.strength(x, y) - INK_FLOOR) / (1.0 - INK_FLOOR)).clamp(0.0, 1.0))
            .sum()
    }

    /// The ink one PIECE of the run `within` is named from: the ink of its own columns
    /// `x0..=x1`, together with the ink of the STROKES that cross either end of it and belong
    /// to it, as far as `reach` columns of the rest of the run.
    ///
    /// A cut down a column separates two characters whose strokes touch, and a face sets
    /// characters whose strokes reach over their neighbour's columns — the bar of a `T`, the
    /// arm of an `L`, the hook of a `J` — so the column a cut is taken at leaves part of one
    /// character's own stroke standing in the other's columns. That stroke belongs to the
    /// character it is part of: a piece is therefore named over it as well, or the ink of a
    /// `T` whose bar was cut short is read as the character the rest of it is shaped like.
    ///
    /// What is followed is a STROKE and only a stroke: a run of inked rows no thicker than
    /// `follow`'s own measure, carried on column by column through the rows it reaches — its
    /// own rows and their immediate neighbours, since a stroke drawn at a slant moves a row
    /// per column — and dropped where the ink it reaches is no stroke but the BODY of a
    /// character, which reaches over nothing: a stem, a bowl or a diagonal standing at the cut
    /// is the piece's own ink up to the cut and no further, so the character beside it is not
    /// measured as part of it. `None` when the piece's own columns carry no ink at all.
    fn piece_in(
        &self,
        x0: usize,
        x1: usize,
        within: &GlyphBox,
        band: &InkLine,
        follow: Follow,
    ) -> Option<Region> {
        let right = x1.min(self.width.saturating_sub(1));
        let bottom = band.bottom.min(self.height.saturating_sub(1));
        if x0 > right || band.top > bottom {
            return None;
        }
        let mut pixels: Vec<(usize, usize)> = Vec::new();
        for y in band.top..=bottom {
            for x in x0..=right {
                if self.at(x, y) {
                    pixels.push((x, y));
                }
            }
        }
        if pixels.is_empty() {
            return None;
        }
        // Each end of the piece, and the way out of it: a stroke that stands in the column
        // beyond that end, against the piece's own ink there, is followed out as far as its
        // own ink runs on. It is the ink of the column BEYOND that has to be a stroke rather
        // than the piece's own, because the cut can fall anywhere across a character: the
        // crossbar of a `t` set tight against the letter after it runs on past the stem the
        // cut leaves standing at the piece's edge.
        for (edge, outward, limit) in [(x0, -1isize, within.x0), (right, 1isize, within.x1)] {
            let beyond = if outward < 0 {
                edge.checked_sub(1).filter(|next| *next >= limit)
            } else {
                (edge < limit && edge + 1 < self.width).then_some(edge + 1)
            };
            let Some(beyond) = beyond else {
                continue;
            };
            let own = self.row_runs(edge, band.top, bottom);
            for stroke in self.row_runs(beyond, band.top, bottom) {
                if stroke.1 + 1 - stroke.0 > follow.thick {
                    continue;
                }
                // Against the piece's own ink at its edge: a stroke that stands clear of it
                // is another character's, not the continuation of one of this piece's.
                if !own
                    .iter()
                    .any(|(above, below)| stroke.0 <= below + 1 && *above <= stroke.1 + 1)
                {
                    continue;
                }
                pixels.extend((stroke.0..=stroke.1).map(|y| (beyond, y)));
                pixels.extend(
                    self.along_stroke(
                        beyond,
                        stroke,
                        outward,
                        limit,
                        (band.top, bottom),
                        follow.one_column_in(),
                    )
                    .0,
                );
            }
        }
        Some(Region::of(&pixels))
    }

    /// The ink the stroke `rows` of column `from` runs on into, stepping `step` columns at a
    /// time: the pixels found and how many columns it was followed over.
    ///
    /// The walk stops at `limit`, after `follow`'s own reach of columns, where the next column
    /// carries no ink in the rows the stroke reaches, and where the ink it reaches there is
    /// thicker than a stroke — see [`Self::piece_in`].
    fn along_stroke(
        &self,
        from: usize,
        rows: (usize, usize),
        step: isize,
        limit: usize,
        (top, bottom): (usize, usize),
        follow: Follow,
    ) -> (Vec<(usize, usize)>, usize) {
        let mut found: Vec<(usize, usize)> = Vec::new();
        let mut held = rows;
        let mut at = from;
        let mut steps = 0usize;
        for _ in 0..follow.reach {
            let next = match step {
                step if step < 0 => match at.checked_sub(1) {
                    Some(next) if next >= limit => next,
                    _ => break,
                },
                _ if at < limit && at + 1 < self.width => at + 1,
                _ => break,
            };
            let Some(along) = self
                .row_runs(next, top, bottom)
                .into_iter()
                .filter(|(from, to)| to + 1 - from <= follow.thick)
                .find(|(from, to)| *from <= held.1 + 1 && held.0 <= to + 1)
            else {
                break;
            };
            found.extend((along.0..=along.1).map(|y| (next, y)));
            held = along;
            at = next;
            steps += 1;
        }
        (found, steps)
    }

    /// The contiguous runs of INKED ROWS column `x` carries inside `top..=bottom`, each as
    /// `(first, last)`: the strokes of the characters that column crosses.
    fn row_runs(&self, x: usize, top: usize, bottom: usize) -> Vec<(usize, usize)> {
        let mut runs: Vec<(usize, usize)> = Vec::new();
        let mut from: Option<usize> = None;
        for y in top..=bottom {
            match (self.at(x, y), from) {
                (true, None) => from = Some(y),
                (false, Some(start)) => {
                    runs.push((start, y - 1));
                    from = None;
                }
                _ => {}
            }
        }
        if let Some(start) = from {
            runs.push((start, bottom));
        }
        runs
    }

    /// The CONNECTED regions of `run`'s ink inside `band`, in reading order: the marks its
    /// ink really falls into.
    ///
    /// A run of inked columns is the ink of one column range, which is not the same thing as
    /// the ink of one character: a proportional face sets letters whose ink reaches back
    /// under the letter before them — the hook of a `J`, the tail of a `Q` — so two
    /// characters' boxes overlap in columns while their strokes never touch, and no cut down
    /// a column separates them. What separates them is CONNECTION: a mark is the ink its own
    /// strokes reach, taken here over the eight neighbours of each inked pixel so that a
    /// stroke drawn at a diagonal stays one region.
    ///
    /// The regions are offered to the segmentation as one reading of the run beside the cuts
    /// ([`cut_apart`]); nothing is split unless the pieces are NAMED better than the whole.
    /// A run whose ink falls into more than [`MAX_REGIONS`] of them is left to the cuts: that
    /// is speckle or a dither pattern rather than a line's characters, and so is a run that
    /// covers more of the page than [`MAX_REGION_PIXELS`], which is more than the characters
    /// of one line of text ever do.
    fn regions_in(&self, run: &GlyphBox, band: &InkLine) -> Vec<Region> {
        let right = run.x1.min(self.width.saturating_sub(1));
        let bottom = band.bottom.min(self.height.saturating_sub(1));
        if run.x0 > right || band.top > bottom {
            return Vec::new();
        }
        let (columns, rows) = (right + 1 - run.x0, bottom + 1 - band.top);
        if columns.saturating_mul(rows) > MAX_REGION_PIXELS {
            return Vec::new();
        }
        let cell = |x: usize, y: usize| (y - band.top) * columns + (x - run.x0);
        let mut taken = vec![false; columns * rows];
        let mut found: Vec<Vec<(usize, usize)>> = Vec::new();
        let mut reached: Vec<(usize, usize)> = Vec::new();
        for y in band.top..=bottom {
            for x in run.x0..=right {
                if !self.at(x, y) || taken[cell(x, y)] {
                    continue;
                }
                taken[cell(x, y)] = true;
                reached.push((x, y));
                let mut region: Vec<(usize, usize)> = Vec::new();
                while let Some((at_x, at_y)) = reached.pop() {
                    region.push((at_x, at_y));
                    for (step_y, step_x) in [
                        (-1i32, -1i32),
                        (-1, 0),
                        (-1, 1),
                        (0, -1),
                        (0, 1),
                        (1, -1),
                        (1, 0),
                        (1, 1),
                    ] {
                        let next_x = at_x as i32 + step_x;
                        let next_y = at_y as i32 + step_y;
                        let inside = next_x >= run.x0 as i32
                            && next_x <= right as i32
                            && next_y >= band.top as i32
                            && next_y <= bottom as i32;
                        if !inside {
                            continue;
                        }
                        let (next_x, next_y) = (next_x as usize, next_y as usize);
                        if self.at(next_x, next_y) && !taken[cell(next_x, next_y)] {
                            taken[cell(next_x, next_y)] = true;
                            reached.push((next_x, next_y));
                        }
                    }
                }
                found.push(region);
                if found.len() > MAX_REGIONS {
                    return Vec::new();
                }
            }
        }
        let mut regions: Vec<Region> = found.iter().map(|pixels| Region::of(pixels)).collect();
        regions.sort_by_key(|region| (region.glyph.x0, region.glyph.y0));
        regions
    }

    /// How much ink `region`'s own pixels carry.
    fn ink_in_region(&self, region: &Region) -> f32 {
        (region.glyph.y0..=region.glyph.y1)
            .map(|y| {
                (region.glyph.x0..=region.glyph.x1)
                    .filter(|x| region.holds(*x as isize, y as isize))
                    .map(|x| self.strength(x, y))
                    .sum::<f32>()
            })
            .sum()
    }

    /// The columns INSIDE `run` where it might be cut: the places its ink narrows, which is
    /// what two characters whose strokes touch leave between them.
    ///
    /// A candidate is a column that carries no more ink than the columns either side of it
    /// — a valley of the run's own profile — and carries little enough of what the run's own
    /// columns carry ([`CUT_VALLEY`]) to be the narrowing between two characters rather than
    /// the thin middle of one, as the inner strokes of an M or a W are. The list is the
    /// deepest `most` of those (and never more than [`MAX_CUT_CANDIDATES`]), no two closer
    /// together than a character of this line could be narrow, in column order; a cut nearer
    /// an end of the run than that separates no character from the rest, so the ends are left
    /// out by the same measure ([`MIN_PIECE_WIDTH`]).
    fn cut_candidates(
        &self,
        run: &GlyphBox,
        band: &InkLine,
        line: glyphs::Line,
        most: usize,
    ) -> Vec<usize> {
        // At least one column either side: what a cut separates is two characters, so the
        // narrowest piece there can be is a column, and the column before the first of them
        // is the one a valley is measured against.
        let margin = (MIN_PIECE_WIDTH * line.cap).ceil().max(1.0) as usize;
        let from = run.x0 + margin;
        let Some(to) = run.x1.checked_sub(margin) else {
            return Vec::new();
        };
        if from >= to {
            return Vec::new();
        }
        let bottom = band.bottom.min(self.height.saturating_sub(1));
        let profile = |x: usize| -> f32 { (band.top..=bottom).map(|y| self.strength(x, y)).sum() };
        let Some(typical) = median_of(&(run.x0..=run.x1).map(profile).collect::<Vec<f32>>()) else {
            return Vec::new();
        };
        let mut valleys: Vec<(f32, usize)> = (from..=to)
            .filter(|x| {
                let (left, here, right) = (profile(x - 1), profile(*x), profile(x + 1));
                // A real NARROWING: no more ink than either side of it and less than one of
                // them. A run whose profile is flat — a bar, a rule, a block of filled ink —
                // narrows nowhere, so it offers no cut at all rather than one at every
                // column of it.
                here <= left && here <= right && (here < left || here < right)
                    && here <= CUT_VALLEY * typical
            })
            .map(|x| (profile(x), x))
            .collect();
        // Deepest first, so the places the ink narrows most are the ones kept.
        valleys.sort_by(|one, other| {
            one.0
                .total_cmp(&other.0)
                .then_with(|| one.1.cmp(&other.1))
        });
        let apart = (MIN_PIECE_WIDTH * line.cap).max(1.0) as usize;
        let most = most.min(MAX_CUT_CANDIDATES);
        let mut kept: Vec<usize> = Vec::new();
        for (_, x) in valleys {
            if kept.len() >= most {
                break;
            }
            if kept.iter().any(|held| held.abs_diff(x) < apart) {
                continue;
            }
            kept.push(x);
        }
        kept.sort_unstable();
        kept
    }
}

/// Which of a band's SLIVER columns bridge two characters: the ones with a character's own
/// stroke on both sides of them inside the same run of ink.
///
/// `inked` and `slivers` are index-aligned, one entry per column of the band:
/// `inked[column]` says the column carries ink at all, `slivers[column]` that all it carries
/// is a sliver ([`MAX_HAIRLINE_SHARE`]). One pass each way, so a band costs its own columns
/// rather than their square.
fn bridges(inked: &[bool], slivers: &[bool]) -> Vec<bool> {
    let mut before = vec![false; inked.len()];
    let mut stroke = false;
    for (column, inked) in inked.iter().enumerate() {
        if !inked {
            stroke = false;
            continue;
        }
        before[column] = stroke;
        if !slivers[column] {
            stroke = true;
        }
    }
    let mut after = vec![false; inked.len()];
    let mut stroke = false;
    for (column, inked) in inked.iter().enumerate().rev() {
        if !inked {
            stroke = false;
            continue;
        }
        after[column] = stroke;
        if !slivers[column] {
            stroke = true;
        }
    }
    (0..inked.len())
        .map(|column| slivers[column] && before[column] && after[column])
        .collect()
}

/// The threshold Otsu's method picks, and how far apart the classes it splits are.
struct Split {
    threshold: u8,
    separation: f32,
}

/// Otsu's threshold for a grey histogram, or `None` when the two classes it would split
/// are closer together than [`MIN_INK_CONTRAST`] — one tone plus noise, so no ink.
fn otsu(histogram: &[u64; 256], total: u64) -> Option<Split> {
    if total == 0 {
        return None;
    }
    let total = total as f64;
    let sum: f64 = histogram
        .iter()
        .enumerate()
        .map(|(value, count)| value as f64 * *count as f64)
        .sum();
    let mut below_weight = 0f64;
    let mut below_sum = 0f64;
    let mut best: Option<Split> = None;
    let mut best_variance = -1f64;
    for (value, count) in histogram.iter().enumerate().take(255) {
        below_weight += *count as f64;
        below_sum += value as f64 * *count as f64;
        let above_weight = total - below_weight;
        if below_weight == 0.0 || above_weight == 0.0 {
            continue;
        }
        let below_mean = below_sum / below_weight;
        let above_mean = (sum - below_sum) / above_weight;
        let delta = above_mean - below_mean;
        let variance = below_weight * above_weight * delta * delta;
        if variance > best_variance {
            best_variance = variance;
            best = Some(Split {
                threshold: u8::try_from(value).unwrap_or(u8::MAX),
                separation: delta as f32,
            });
        }
    }
    best.filter(|split| split.separation >= MIN_INK_CONTRAST)
}

/// The mean grey level of the two classes `threshold` splits a histogram into: the darker
/// class's and the lighter class's.
///
/// They are the levels the page's ground and its ink are really drawn at, which is what a
/// pixel's ink strength is measured between. A class with no pixels in it reports the
/// level it is bounded by, so a page of one tone measures no ink either way.
fn levels(histogram: &[u64; 256], threshold: u8) -> (f32, f32) {
    let mean = |range: std::ops::RangeInclusive<usize>, fallback: f32| {
        let mut weight = 0f64;
        let mut total = 0f64;
        for value in range {
            let count = histogram[value] as f64;
            weight += count;
            total += value as f64 * count;
        }
        if weight > 0.0 {
            (total / weight) as f32
        } else {
            fallback
        }
    };
    let split = threshold as usize;
    (
        mean(0..=split, f32::from(threshold)),
        mean(split + 1..=255, f32::from(threshold)),
    )
}

// ---------------------------------------------------------------------------
// Segmentation
// ---------------------------------------------------------------------------

/// One candidate mark: the rectangle its ink occupies.
#[derive(Debug, Clone, Copy)]
struct GlyphBox {
    x0: usize,
    x1: usize,
    y0: usize,
    y1: usize,
}

impl GlyphBox {
    fn width(&self) -> usize {
        self.x1 + 1 - self.x0
    }

    fn height(&self) -> usize {
        self.y1 + 1 - self.y0
    }

    /// How far this mark's left edge stands past `before`'s: the STEP from one mark to the
    /// next, which is what a line that sets its characters on one advance separates them
    /// by.
    fn glyph_step_from(&self, before: &Self) -> f32 {
        self.x0.saturating_sub(before.x0) as f32
    }

    /// How far this mark's own CENTRE stands past `before`'s: the step from the cell one
    /// mark sits in to the cell the next sits in.
    ///
    /// What a MONOSPACED line separates its marks by, because a cell holds a glyph's ink
    /// wherever the glyph puts it: the step between two marks' left EDGES is a cell give or
    /// take the bearings the two glyphs leave inside their cells, which on a line that sets
    /// a narrow glyph beside a wide one is a good part of one.
    fn cell_step_from(&self, before: &Self) -> f32 {
        ((self.x0 + self.x1) as f32 - (before.x0 + before.x1) as f32) / 2.0
    }
}

/// A line of ink on the page.
struct InkLine {
    top: usize,
    bottom: usize,
    glyphs: Vec<GlyphBox>,
}

/// How far a STROKE that crosses a cut is followed, and what still counts as a stroke: the
/// measure [`Ink::piece_in`] reads a piece's ink by, in the pixels of the line it is on.
#[derive(Debug, Clone, Copy)]
struct Follow {
    /// How many columns past the cut the stroke is followed: see [`OVERHANG_REACH`].
    reach: usize,
    /// How many rows of ink one column of it may carry and still be a stroke rather than the
    /// body of a character: see [`OVERHANG_THICK`].
    thick: usize,
}

impl Follow {
    /// What a line of capital height `cap` follows a stroke by.
    fn of(cap: f32) -> Self {
        Self {
            reach: (OVERHANG_REACH * cap).round().max(0.0) as usize,
            thick: (OVERHANG_THICK * cap).round().max(1.0) as usize,
        }
    }

    /// The same measure one column further on: what is left of the reach once the column
    /// beyond the cut has been taken.
    fn one_column_in(self) -> Self {
        Self {
            reach: self.reach.saturating_sub(1),
            ..self
        }
    }
}

/// One CONNECTED region of a run's ink: the box it occupies, and which pixels of that box
/// its own strokes reach.
///
/// Two characters' boxes can overlap while their strokes never touch (see
/// [`Ink::regions_in`]), so a region carries the mask as well as the box: the ink of the
/// mark beside it stands inside the same rectangle and is not this mark's.
#[derive(Debug, Clone)]
struct Region {
    glyph: GlyphBox,
    /// Row-major over `glyph`'s own rectangle.
    own: Vec<bool>,
}

impl Region {
    /// The region `pixels` make up: the box around them, and the mask of which of that
    /// box's pixels they are.
    fn of(pixels: &[(usize, usize)]) -> Self {
        let mut extremes = Extremes::default();
        for (x, y) in pixels {
            extremes.mark(*x, *y);
        }
        // Every region comes from at least one pixel, so it has bounds; a region without
        // them is the empty box at the origin, which holds nothing.
        let (x0, x1, y0, y1) = extremes.bounds().unwrap_or((0, 0, 0, 0));
        let glyph = GlyphBox { x0, x1, y0, y1 };
        let mut own = vec![false; glyph.width() * glyph.height()];
        for (x, y) in pixels {
            own[(y - y0) * glyph.width() + (x - x0)] = true;
        }
        Self { glyph, own }
    }

    /// Whether `(x, y)` is one of this region's own pixels.
    fn holds(&self, x: isize, y: isize) -> bool {
        let inside = (self.glyph.x0 as isize..=self.glyph.x1 as isize).contains(&x)
            && (self.glyph.y0 as isize..=self.glyph.y1 as isize).contains(&y);
        inside
            && self.own[(y as usize - self.glyph.y0) * self.glyph.width()
                + (x as usize - self.glyph.x0)]
    }
}

/// One mark of a line as the segmentation settled it: its rectangle, the ink that rectangle
/// holds, and the character it was named with, if any.
#[derive(Debug, Clone, Copy)]
struct Settled {
    glyph: GlyphBox,
    ink: f32,
    named: Option<glyphs::Named>,
    /// Which run of ink this mark was CUT out of, where the cut settled that run's
    /// characters: the index of that run among the line's own.
    ///
    /// Such a cut was taken because every piece of it names a character of this line better
    /// than the whole run named one ([`LookInside`]), so the pieces are not weighed against
    /// each other again by [`join`]: putting them back together would be the same comparison
    /// answered the other way.
    piece: Option<usize>,
}

impl Settled {
    /// What this mark is worth to the segmentation: how far its match beat the floor every
    /// reported character has to clear, over the INK the mark was named from, and a small
    /// cost per unit of ink that named nothing.
    ///
    /// Weighed by the ink rather than counted per mark, because cutting a run of ink into
    /// pieces and joining pieces into one mark both leave the ink where it was: what the
    /// segmentation compares is therefore how well the same ink is accounted for either way,
    /// and it cannot win by naming more marks, each from less of it.
    fn value(&self) -> f32 {
        self.ink
            * self
                .named
                .map_or(UNNAMED_MARK, |found| found.score - glyphs::MIN_MATCH)
    }
}

/// Name every mark of `band`, cutting a run of ink whose strokes run together and joining
/// the pieces of a stroke that broke apart.
///
/// A run of inked columns is a candidate for ONE character, but at the size a page is
/// drawn at that candidate can be wrong every way: neighbouring letters set tight, or
/// spread by a heavy weight or a lossy coding, touch and come out as one run; a letter whose
/// ink reaches back under the one before it shares that one's columns without touching it at
/// all; and a thin stroke — the arch of an 'n', the waist of an 's' — can fall under the
/// floor that tells ink from ground and leave a letter in pieces. All three are settled by
/// NAMING: a cut or a split into the run's own connected regions is kept only where the
/// pieces are named better than the whole was, and a join only where the whole is named
/// better than its pieces, so a page that needs none of them is read exactly as its runs
/// stand.
///
/// Which runs are looked inside at all is settled by naming too: the ones too wide to be one
/// character of their line, and the ones whose own name falls a clear margin short of what
/// the line's other marks match at ([`WEAK_MARK_MATCH`]).
fn settle(
    ink: &Ink,
    band: &InkLine,
    line: glyphs::Line,
    budget: &mut Budget,
) -> Result<Vec<Settled>, Unreadable> {
    // How wide one mark of THIS line is: a run several times that holds more than one
    // character, whatever the line's capitals measure.
    let typical = median(band.glyphs.iter().map(GlyphBox::width)).unwrap_or(1.0);
    let too_wide = (MAX_MARK_WIDTH * line.cap).max(MAX_MARK_PITCH * typical);
    // Every run named as it stands, which is what the pieces of a cut are weighed against.
    let whole: Vec<Settled> = band
        .glyphs
        .iter()
        .map(|run| name(ink, *run, line, budget))
        .collect::<Result<_, _>>()?;
    // What the marks of THIS line match their characters at: the runs that fall a clear
    // margin short of it are the ones whose ink is not one character of the face the rest
    // of the line is set in.
    let matched = median_of(
        &whole
            .iter()
            .map(|mark| mark.named.map_or(0.0, |found| found.score))
            .collect::<Vec<f32>>(),
    )
    .unwrap_or(0.0);
    let mut cut: Vec<Settled> = Vec::new();
    for (index, (run, whole)) in band.glyphs.iter().zip(whole).enumerate() {
        // Which readings of this run are worth taking. A run TOO WIDE to be one character of
        // its line holds more than one, and where its characters' strokes touch only a cut
        // through the narrowing between them separates them: every place such a run narrows
        // is offered to the segmentation. A run whose own name falls SHORT of what the marks
        // of the same line match at may hold two characters that are no wider than one — two
        // narrow letters set tight read as the one letter their joined strokes are shaped
        // like — and there the single deepest narrowing of its ink is offered: that is where
        // two such characters divide, and offering one place rather than every place is what
        // keeps a middling mark of a page set in a face nothing here is a copy of from being
        // ground into the punctuation its pieces are shaped like. A run whose name is a clear
        // margin worse than its line's ([`WEAK_MARK_MATCH`]) is read by its own connected
        // regions too, since ink that is not joined is evidence of two marks.
        let short = whole.named.is_none_or(|found| found.score < matched);
        let weak = whole
            .named
            .is_none_or(|found| found.score < matched - WEAK_MARK_MATCH);
        let wide = (run.width() as f32) > too_wide;
        let look = LookInside {
            cuts: if wide {
                MAX_CUT_CANDIDATES
            } else {
                usize::from(short)
            },
            // A cut taken on a run that is NOT too wide is taken on the evidence that both
            // of its pieces are characters of this line, so each of them has to be named as
            // well as this line's own marks are ([`WEAK_MARK_MATCH`]) AND a clear margin
            // better than the whole run was ([`CUT_PIECE_GAIN`]): without that, the one
            // narrowing of a letter the face sets with a thin waist — a `g`, an `s` — would
            // be read as the two marks of punctuation its halves are shaped like. A run too
            // wide to be one character holds more than one whatever its pieces are named,
            // and is cut on the width.
            floor: if wide {
                0.0
            } else {
                (matched - WEAK_MARK_MATCH)
                    .max(whole.named.map_or(0.0, |found| found.score) + CUT_PIECE_GAIN)
            },
            // What a cut separates is two characters of a WORD, and a word is set with
            // letters and figures: a run the recogniser names as one of those is a letter
            // of the line's text, and a letter does not divide into the marks of
            // PUNCTUATION its own strokes are shaped like — the two diagonals of an `X`,
            // the arcs of an `S`, the bowl and tail of a `g` — unless the ink of such a mark
            // really is one, which is what a close name a clear margin better than the whole
            // run's own says ([`PUNCTUATION_PIECE_MATCH`], [`PUNCTUATION_PIECE_GAIN`]): a
            // figure set tight against a point or a solidus the page drew leaves exactly
            // that. A run that names punctuation itself, or names nothing at all, is ink of
            // another kind and may be divided into whatever its pieces are.
            punctuation: if names_a_character_of_a_word(&whole) {
                (whole.named.map_or(0.0, |found| found.score) + PUNCTUATION_PIECE_GAIN)
                    .max(PUNCTUATION_PIECE_MATCH)
            } else {
                0.0
            },
            settles: (!wide).then_some(index),
            regions: wide || weak,
        };
        cut_apart(ink, band, whole, line, look, budget, &mut cut)?;
    }
    join(ink, band, cut, line, budget)
}

/// Whether this mark names a character a WORD is set with: a letter or a figure, or a bare
/// stem one of them is what it would be read as ([`glyphs::Named::letter`]).
fn names_a_character_of_a_word(mark: &Settled) -> bool {
    mark.named.is_some_and(|found| {
        found.ch.is_ascii_alphanumeric() || (found.open && found.letter.is_some())
    })
}

/// Whether `mark` is a character a division of a run may leave: a letter or a figure, or a
/// mark of punctuation whose ink really is that mark ([`LookInside::punctuation`]).
fn divides_into(mark: &Settled, punctuation: f32) -> bool {
    match mark.named {
        Some(found) if found.ch.is_ascii_alphanumeric() => true,
        Some(found) => found.score >= punctuation,
        None => punctuation <= 0.0,
    }
}

/// Which readings of a run of ink the segmentation takes beside the run as it stands: the
/// cuts through the places its ink narrows, the run's own connected regions, or neither.
#[derive(Debug, Clone, Copy)]
struct LookInside {
    /// How many places inside the run — the deepest narrowings of its ink — the segmentation
    /// may cut it at. Zero leaves the run uncut.
    cuts: usize,
    /// How well every piece of a cut must be named for that cut to be taken at all.
    floor: f32,
    /// Whether every piece of a division must be a LETTER OR A FIGURE, and how closely a
    /// piece that is neither must be named for the division to be taken anyway.
    ///
    /// What a division separates is two characters of a word, and the words of a page are set
    /// with letters and figures: ink that divides into the marks of PUNCTUATION its pieces are
    /// shaped like — the diagonals of a `V`, the arcs of an `s`, the bowl and tail of a `g` —
    /// is one character, which is what it stays. A page sets punctuation of its own too, and
    /// sets it tight against the figures and letters beside it, so a piece that is a mark of
    /// punctuation is admitted where its ink really is that mark: it has to be named this
    /// closely, which is a clear margin better than the whole run was named as the character
    /// it is not ([`PUNCTUATION_PIECE_MATCH`], [`PUNCTUATION_PIECE_GAIN`]). Zero admits any
    /// piece, which is what a run that names no character of a word is divided under.
    punctuation: f32,
    /// The run this look is into, where a cut taken here SETTLES that run's characters, so
    /// that [`join`] does not weigh its pieces against each other again: `Some` where the
    /// cut would be taken on the strength of the pieces' own names rather than on the run's
    /// width.
    settles: Option<usize>,
    regions: bool,
}

/// Name one mark, against this request's naming budget.
fn name(
    ink: &Ink,
    glyph: GlyphBox,
    line: glyphs::Line,
    budget: &mut Budget,
) -> Result<Settled, Unreadable> {
    budget.spend()?;
    Ok(Settled {
        named: glyphs::recognise(ink, &glyph, line),
        ink: ink.ink_in(&glyph),
        glyph,
        piece: None,
    })
}

/// Name one connected region of a run's ink, measuring the region's OWN pixels and not the
/// ink a mark beside it leaves inside the same rectangle.
fn name_region(
    ink: &Ink,
    region: &Region,
    line: glyphs::Line,
    budget: &mut Budget,
) -> Result<Settled, Unreadable> {
    budget.spend()?;
    Ok(Settled {
        named: glyphs::recognise_region(ink, region.glyph, line, &|x, y| region.holds(x, y)),
        ink: ink.ink_in_region(region),
        glyph: region.glyph,
        piece: None,
    })
}

/// Name one PIECE a cut leaves of the run `within`: the mark the piece's own columns hold,
/// named over the ink its strokes continue into past the cut as well ([`Ink::piece_in`]).
///
/// What the piece is WORTH to the segmentation is still measured over the ink of its own
/// columns, and the rectangle it reports is still that ink's: the continuation says which
/// character the piece is, not how much of the run it accounts for, so a cut cannot win by
/// counting the ink either side of it twice.
///
/// Which of the two readings names the piece: the one that takes in the crossing stroke,
/// since the stroke a cut severed is part of the character it was cut from — unless the
/// piece's OWN ink agrees with a character a clear margin better ([`SEVERED_STROKE_MARGIN`]),
/// which is what the ink beside a complete character says. A `T` whose bar runs on into the
/// letter after it is a complete `T` either way and agrees with it best over its own ink,
/// while a `T` whose bar was cut short agrees with the character the rest of it is shaped
/// like no better than the whole of its bar agrees with a `T`. Where the piece's own ink
/// leaves the reading OPEN — a bare stem, which is what a severed cross-stroke leaves — the
/// crossing stroke settles it.
fn name_piece(
    ink: &Ink,
    glyph: GlyphBox,
    within: &GlyphBox,
    band: &InkLine,
    line: glyphs::Line,
    budget: &mut Budget,
) -> Result<Settled, Unreadable> {
    budget.spend()?;
    let own = glyphs::recognise(ink, &glyph, line);
    let crossing = ink
        .piece_in(glyph.x0, glyph.x1, within, band, Follow::of(line.cap))
        .filter(|piece| piece.glyph.width() > glyph.width())
        .and_then(|piece| {
            glyphs::recognise_region(ink, piece.glyph, line, &|x, y| piece.holds(x, y))
        });
    let named = match (own, crossing) {
        // A piece whose own ink is a BARE STEM is the ink of several characters at once, and
        // the ink of one whose cross-stroke the cut severed is a bare stem too: so where the
        // stroke that crosses the cut names a character, that is what the piece is. A piece
        // that names a character with a stroke of its own keeps that reading unless taking
        // the crossing stroke in agrees as closely ([`SEVERED_STROKE_MARGIN`]).
        (Some(own), Some(crossing))
            if glyphs::STEMS.contains(&own.ch)
                || own.open
                || own.score <= crossing.score + SEVERED_STROKE_MARGIN =>
        {
            Some(crossing)
        }
        (Some(own), _) => Some(own),
        (None, crossing) => crossing,
    };
    Ok(Settled {
        named,
        ink: ink.ink_in(&glyph),
        glyph,
        piece: None,
    })
}

/// Read `whole`'s run apart into the marks that account for its ink best, and push what it
/// settles as onto `out`.
///
/// A run `look`ed inside may hold more than one character in either of two ways. Its
/// characters' strokes may TOUCH, with nothing but a narrowing of its ink between them — the
/// letters set tight, or the halo around them run together — and where exactly is settled by
/// NAMING: the columns where the run's ink narrows are the places it could be cut
/// ([`Ink::cut_candidates`]), and the run is cut into the sequence of pieces whose names
/// account for its ink best of all such sequences — found once, over the whole run, rather
/// than by halving it, since where one letter ends depends on where the letters after it do.
/// Or its characters may share COLUMNS without touching at all, which no cut separates, and
/// then the run's own connected regions are the marks ([`Ink::regions_in`]).
///
/// `whole` is the run named as it stands, and it is the reading that stands unless one of
/// the other two accounts for its ink better: the same ink either way, so what they are
/// compared on is how well it is named.
fn cut_apart(
    ink: &Ink,
    band: &InkLine,
    whole: Settled,
    line: glyphs::Line,
    look: LookInside,
    budget: &mut Budget,
    out: &mut Vec<Settled>,
) -> Result<(), Unreadable> {
    // Nothing about this run says it holds more than one character: it is the mark it was
    // named as.
    if look.cuts == 0 && !look.regions {
        out.push(whole);
        return Ok(());
    }
    let run = whole.glyph;
    // What its own CONNECTED regions name as, where its ink falls into more than one of
    // them: two characters whose boxes overlap but whose strokes never touch. A run that is
    // one region is the one mark `whole` already named.
    //
    // Every region has to stand as tall as a character of its line, for the reason a cut's
    // pieces do: ink that does not is part of ONE character — the dot of an `i`, the piece a
    // stroke that broke left behind — and reading such a run by its regions would report a
    // letter as the marks of punctuation its pieces are shaped like. What puts a broken
    // letter back together is [`join`].
    let mut regions: Vec<Settled> = Vec::new();
    let found = ink.regions_in(&run, band);
    let apart = found.len() > 1
        && found
            .iter()
            .all(|region| (region.glyph.height() as f32) >= MIN_PIECE_HEIGHT * line.cap);
    if look.regions && apart {
        for region in &found {
            regions.push(name_region(ink, region, line, budget)?);
        }
        // A run the recogniser names as a letter or a figure is a character of the line's
        // own text, and such a character does not divide into the marks of PUNCTUATION its
        // own strokes are shaped like — whether it divides at a narrowing of its ink or at a
        // break in it. So where a cut of this run would have to leave characters of a word
        // ([`LookInside::punctuation`]), so must a reading by its regions: the two diagonals
        // an `X` falls into when its crossing thins out are no two characters of a word.
        if !regions
            .iter()
            .all(|region| divides_into(region, look.punctuation))
        {
            regions.clear();
        }
    }
    let regions_value = if regions.is_empty() {
        f32::MIN
    } else {
        regions.iter().map(Settled::value).sum()
    };
    // Where each piece may start: the run's own left edge, each place it could be cut, and
    // one past its right edge, so the piece from `starts[i]` to `starts[j] - 1` is the ink
    // between two of them.
    let mut starts = vec![run.x0];
    if look.cuts > 0 {
        starts.extend(
            ink.cut_candidates(&run, band, line, look.cuts)
                .into_iter()
                .map(|at| at + 1),
        );
    }
    starts.push(run.x1 + 1);
    let last = starts.len() - 1;
    let widest = glyphs::widest_glyph() * line.cap;
    // The best-named sequence of pieces from each start onwards, found from the right so
    // each piece is named once.
    let mut best = vec![f32::MIN; starts.len()];
    let mut then = vec![last; starts.len()];
    let mut named: Vec<Option<Settled>> = vec![None; starts.len()];
    best[last] = 0.0;
    // A run with no place to be cut inside it leaves `best[0]` where it started, so the cut
    // reading of such a run is worth nothing and the other two settle it.
    if starts.len() >= 3 {
        for from in (0..last).rev() {
            for to in from + 1..=last {
                let width = (starts[to] - starts[from]) as f32;
                // Too wide to be one character: and, since the pieces only grow from here,
                // so is every piece after it — unless nothing narrower was found at all, in
                // which case this one piece carries the rest of the run.
                if width > widest && to > from + 1 {
                    break;
                }
                let Some(piece) = ink.mark_in(starts[from], starts[to] - 1, band) else {
                    continue;
                };
                // A cut separates two characters of the line's own text, and both of them
                // stand as tall as such a character: a piece shorter than that is part of
                // ONE character — the bar of a T, the arm of an L — and cutting there would
                // read a letter as two marks of punctuation.
                if (piece.height() as f32) < MIN_PIECE_HEIGHT * line.cap {
                    continue;
                }
                let piece = name_piece(ink, piece, &run, band, line, budget)?;
                // A cut separates two characters of this line, and a character of this line
                // is named as well as the line's own marks are, and is a character a word is
                // set with where the cut is taken on the naming alone: a piece that is
                // neither is no character of a word, so the cut that would leave it is not
                // taken (see [`LookInside::floor`] and [`LookInside::punctuation`]).
                let character = piece.named.is_some_and(|found| found.score >= look.floor)
                    && divides_into(&piece, look.punctuation);
                if !character {
                    continue;
                }
                // What the cut at the END of this piece costs: the STROKE it crosses. Two
                // characters whose ink runs together are bridged by the halo anti-aliasing
                // leaves around their strokes, which is little ink past the floor that tells
                // ink from ground; the inside of one character — the middle of an M, the
                // waist of an S — is a stroke, which is all of it. Charging the cut for what
                // it crosses is what keeps a letter whole where the page gives both.
                //
                // Over the line's own CAPITAL HEIGHT, because that is what a piece is worth
                // measured in: a piece's value is the ink of a whole character ([`Settled::
                // value`]), which grows with the square of the size the page is set at,
                // while the stroke one column crosses grows with the size itself. Charging
                // the column alone would make a cut through the middle of a character free
                // on a page set large and dear on the same page set small, so a crossing
                // stroke is charged as the ink it would lay down over a character's own
                // height.
                let crossed = if to == last {
                    0.0
                } else {
                    CUT_COST * ink.stroke_in_column(starts[to] - 1, band) * line.cap
                };
                let value = piece.value() - crossed + best[to];
                if value > best[from] {
                    best[from] = value;
                    then[from] = to;
                    named[from] = Some(piece);
                }
            }
        }
    }
    let cuts_value = best[0];
    // The run stands as it is unless being read apart accounts for its ink better, by the
    // margin a cut has to gain ([`CUT_GAIN`]) — the same ink either way, so what the three
    // readings are compared on is how well they name it.
    let standing = whole.value() + CUT_GAIN * whole.ink;
    if standing >= cuts_value.max(regions_value) {
        out.push(whole);
        return Ok(());
    }
    if regions_value >= cuts_value {
        out.extend(regions);
        return Ok(());
    }
    let mut at = 0usize;
    while at < last {
        let Some(piece) = named[at] else {
            break;
        };
        out.push(Settled {
            // Where this cut settles the run's characters, its pieces carry the run they came
            // out of, so [`join`] does not put them back together.
            piece: look.settles,
            ..piece
        });
        at = then[at];
    }
    Ok(())
}

/// Join the neighbouring pieces of `marks` that are one character's ink: what a stroke that
/// broke apart leaves.
///
/// Two marks closer than a letter's own side bearing, at least one of them too narrow to be
/// a character of this line, can be one character whose stroke broke. Where BOTH are too
/// narrow to be characters the pieces are joined wherever the whole accounts for their ink
/// better than either does; where only one is, the narrow one may be a narrow CHARACTER set
/// tight against its neighbour instead, and what tells the two apart is whether the joined
/// ink NAMES a character at least as well as the worse of the two is named ([`joined`]).
///
/// A piece belongs to the character it broke off, and that character stands on EITHER side of
/// it: the upper arc a `3` leaves when its waist falls under the floor that tells ink from
/// ground stands before the rest of that figure, and the arm of a `Y` whose apex thinned out
/// stands after the rest of that letter. So the pairs are weighed against each other rather
/// than read left to right: the pair whose joined ink accounts for most of itself is joined,
/// then the pairs its join changed are weighed again, until no pair is left to join. A pass
/// that only ever joined a piece to the mark BEFORE it would report the piece of a letter
/// that broke on its left as the mark of punctuation that piece is shaped like.
///
/// Each pair is named once and re-named only when a join changes it, so the naming this
/// costs grows with the marks of the line rather than with their square.
fn join(
    ink: &Ink,
    band: &InkLine,
    marks: Vec<Settled>,
    line: glyphs::Line,
    budget: &mut Budget,
) -> Result<Vec<Settled>, Unreadable> {
    let mut marks = marks;
    // What each neighbouring pair is worth joined, `pairs[at]` being the pair of `marks[at]`
    // and the mark after it: `None` where that pair is not one character's ink.
    let mut pairs: Vec<Option<Joined>> = Vec::with_capacity(marks.len());
    for at in 0..marks.len().saturating_sub(1) {
        pairs.push(joined(ink, band, &marks[at], &marks[at + 1], line, budget)?);
    }
    while let Some(at) = best_join(&pairs) {
        // The pair stands as one mark, and the pairs either side of it are pairs of that
        // mark now, so they are weighed again.
        let Some(found) = pairs[at].take() else {
            break;
        };
        marks[at] = found.mark;
        marks.remove(at + 1);
        pairs.remove(at);
        if at > 0 {
            pairs[at - 1] = joined(ink, band, &marks[at - 1], &marks[at], line, budget)?;
        }
        if at < pairs.len() {
            pairs[at] = joined(ink, band, &marks[at], &marks[at + 1], line, budget)?;
        }
    }
    Ok(marks)
}

/// Two neighbouring marks read as ONE mark, and how much of itself that reading accounts
/// for beside the better of the two.
#[derive(Debug, Clone, Copy)]
struct Joined {
    mark: Settled,
    gain: f32,
}

/// Which pair of [`join`]'s accounts for most of its own ink, or `None` where no pair is to
/// be joined at all.
fn best_join(pairs: &[Option<Joined>]) -> Option<usize> {
    pairs
        .iter()
        .enumerate()
        .filter_map(|(at, found)| found.map(|found| (at, found.gain)))
        .fold(None, |held: Option<(usize, f32)>, (at, gain)| {
            match held {
                Some((_, best)) if best >= gain => held,
                _ => Some((at, gain)),
            }
        })
        .map(|(at, _)| at)
}

/// `before` and `after` named as one mark, where their ink is one character's.
fn joined(
    ink: &Ink,
    band: &InkLine,
    before: &Settled,
    after: &Settled,
    line: glyphs::Line,
    budget: &mut Budget,
) -> Result<Option<Joined>, Unreadable> {
    // Only PIECES are joined: two marks each as wide as a character of this line are two
    // characters, and two marks a letter's own side bearing apart are two marks. Which of
    // the two is the piece is not asked — a stroke breaks off either side of a letter.
    //
    // Two pieces one CUT settled are not weighed against each other again: that cut was taken
    // because each of them names a character of this line better than the whole run named
    // one, which is the same comparison this would answer the other way (see
    // [`Settled::piece`]).
    let settled = before.piece.is_some() && before.piece == after.piece;
    let narrow = |mark: &Settled| (mark.glyph.width() as f32) <= MAX_PIECE_WIDTH * line.cap;
    let piece = narrow(before) || narrow(after);
    let close = (after.glyph.x0.saturating_sub(before.glyph.x1 + 1) as f32)
        <= MAX_PIECE_GAP * line.cap;
    let fits = ((after.glyph.x1 + 1).saturating_sub(before.glyph.x0) as f32)
        <= glyphs::widest_glyph() * line.cap;
    if settled || !(piece && close && fits) {
        return Ok(None);
    }
    let Some(together) = ink.mark_in(before.glyph.x0, after.glyph.x1, band) else {
        return Ok(None);
    };
    let together = name(ink, together, line, budget)?;
    // How much of the joined ink the one mark accounts for, beside the marks it is made of:
    // the same ink either way, so what the two readings are compared on is how well that ink
    // is named ([`Settled::value`]).
    //
    // Which marks it is weighed against is what the two readings ARE. Where both marks name a
    // CHARACTER OF A WORD — a letter or a figure, or a stem a word's own letter is what it
    // would be read as ([`glyphs::Named::letter`]) — both readings are readings of the whole
    // ink, one character or two, so the join is weighed against BOTH of them: weighing it
    // against the better one alone would join any two characters whose ink one character
    // middlingly fits, since one mark named over twice the ink is worth more than either of
    // them however well each was named. Where a mark names no character of a word — a
    // diagonal, a brace, ink that named nothing at all — it is no reading of its own but a
    // piece of something, and the join is weighed against the better of the two, which is
    // what tells a letter whose stroke broke from a narrow character the face set tight
    // against its neighbour.
    let character = names_a_character_of_a_word;
    let gain = if character(before) && character(after) {
        together.value() - (before.value() + after.value())
    } else {
        together.value() - before.value().max(after.value())
    };
    // Where BOTH marks are too narrow to be characters of this line, the ink can be one
    // character's, broken in two — the diagonals a `V` falls into when its apex thins out,
    // the bar and the stem of a broken `T` — and the pieces are joined wherever the whole
    // accounts for that ink better than the marks it is made of do.
    if narrow(before) && narrow(after) && gain > 0.0 {
        return Ok(Some(Joined {
            mark: together,
            gain,
        }));
    }
    // Where only ONE of them is narrow, that one is either a narrow CHARACTER the face set
    // tight against its neighbour — an `I`, a `1`, a point — or a piece that broke off it.
    // Two things have to hold for it to be a piece. The joined ink must be named at least as
    // well as the worse of the two is: the pieces of a letter whose stroke broke name that
    // letter at least that well, while two characters the face set tight are each named well
    // and their joined ink is named worse than both, since one character's ink is not read as
    // well as either of two characters' is. Going by the score of the pieces alone would
    // report a mark the page never drew: what a thin stroke falling under the floor that
    // tells ink from ground leaves is the ends of the strokes it joined, and those agree
    // closely with the punctuation they are shaped like. And the join must account for the
    // ink BETTER than the two marks do, the same test the pieces of a broken stroke are
    // joined by: without it a point the face set tight against the figure beside it — the
    // point of an amount — would be swallowed into that figure, which is named well over
    // nearly all of the joined ink however little of it the point is.
    //
    // A part that names nothing counts as named at nothing, so ink that matched no glyph is
    // joined into the character beside it wherever the joined ink names one.
    let scored = |mark: &Settled| mark.named.map_or(0.0, |found| found.score);
    let whole = together
        .named
        .is_some_and(|found| found.score >= scored(before).min(scored(after)));
    if !whole || gain <= 0.0 {
        return Ok(None);
    }
    Ok(Some(Joined {
        mark: together,
        gain,
    }))
}

/// One line's recognised text, and the scores it was recognised with.
struct RenderedLine {
    text: String,
    score_total: f32,
    score_count: usize,
    /// How much of the line's ink was named, and how well: the ink of each named mark of a
    /// word of several characters, weighted by how far its match beat
    /// [`glyphs::MIN_MATCH`].
    named_ink: f32,
    /// The line's words, in reading order: one per run of glyphs the text does NOT
    /// separate with a space or a tab, so `words[k]` is the k-th whitespace-separated
    /// word of `text`.
    words: Vec<WordRun>,
}

/// One word's measured ink, in page pixels, and the scores its characters matched with.
struct WordRun {
    x0: usize,
    x1: usize,
    y0: usize,
    y1: usize,
    score_total: f32,
    score_count: usize,
    /// How much of the page's ink this word's characters were named from, and how well:
    /// each mark's ink weighted by how far its match beat [`glyphs::MIN_MATCH`].
    named_ink: f32,
}

impl WordRun {
    /// A word starting at `glyph`.
    fn starting_at(glyph: &GlyphBox, score: f32, named_ink: f32) -> Self {
        Self {
            x0: glyph.x0,
            x1: glyph.x1,
            y0: glyph.y0,
            y1: glyph.y1,
            score_total: score,
            score_count: 1,
            named_ink,
        }
    }

    /// Take `glyph` into this word.
    fn extend(&mut self, glyph: &GlyphBox, score: f32, named_ink: f32) {
        self.x0 = self.x0.min(glyph.x0);
        self.x1 = self.x1.max(glyph.x1);
        self.y0 = self.y0.min(glyph.y0);
        self.y1 = self.y1.max(glyph.y1);
        self.score_total += score;
        self.score_count += 1;
        self.named_ink += named_ink;
    }

    /// This word's ink as it sits on the page it was submitted as, whichever turn of that
    /// page it was read at: the box enclosing the ink, and the outline of the marks.
    fn marks(&self, ink: &Ink, orientation: Orientation) -> Marks {
        ink.marks(self.x0, self.x1, self.y0, self.y1, orientation)
    }
}

impl RenderedLine {
    /// This line's measurements, as fractions of the page as it was SUBMITTED: the extent
    /// and the outline of each word's own ink, and the score each was recognised with.
    fn measured(&self, ink: &Ink, orientation: Orientation) -> MeasuredLine {
        MeasuredLine {
            confidence: percentage(self.score_total, self.score_count),
            words: self
                .words
                .iter()
                .map(|run| MeasuredWord {
                    marks: run.marks(ink, orientation),
                    confidence: percentage(run.score_total, run.score_count),
                })
                .collect(),
        }
    }
}

/// The characters of the named marks of one line, with the ones a BARE STEM could be
/// resolved by the characters beside them.
///
/// A stem with nothing else to it is the same ink in `I`, `l`, `1` and `|`: which of them a
/// face draws that way differs from face to face — a serif on the capital, a tail on the
/// letter l, a flag on the figure one, none of them on the bar — so a mark that is nothing
/// but a stem cannot be told apart by its shape, and the comparison that named it had to
/// pick one of the four. What tells them apart is what stands beside it in its own word,
/// which is how a reader tells them apart too: a stem next to figures is the figure, a stem
/// next to small letters is the small letter — except at the start of a word, where it is
/// the capital that begins it — and a stem next to capitals is the capital. A stem standing
/// in no word at all is left as it was named: nothing says it is a letter.
///
/// A mark INSIDE a word is a letter or a figure, since that is what the words of a page are
/// set with — a BAR is not one of them, it is what a rule or a separator is drawn as — so an
/// open reading there is read as the best letter or figure the mark's ink could be
/// ([`glyphs::Named::letter`]). Only where that is itself one of the stems do the characters
/// beside it settle which of them it is: a mark whose ink names a character of its own is
/// that character, however nearly a stem the face draws it — the `t` of a face whose crossbar
/// barely reaches past its own stem.
fn settle_stems(marks: &[Settled], separated: &[(usize, Separation)]) -> Vec<(char, f32)> {
    let mut named: Vec<(char, f32)> = separated
        .iter()
        .filter_map(|(at, _)| marks[*at].named)
        .map(|found| (found.ch, found.score))
        .collect();
    let letters: Vec<Option<char>> = separated
        .iter()
        .filter_map(|(at, _)| marks[*at].named)
        .map(|found| found.letter)
        .collect();
    let open: Vec<bool> = separated
        .iter()
        .filter_map(|(at, _)| marks[*at].named)
        .map(|found| found.open)
        .collect();
    // The words of the line: runs of marks nothing separates.
    let mut from = 0usize;
    for step in 1..=separated.len() {
        if step < separated.len() && separated[step].1 == Separation::None {
            continue;
        }
        let word = from..step;
        from = step;
        if word.len() < 2 {
            continue;
        }
        for at in word.clone() {
            // Only a mark whose own shape left the reading open is read off its
            // neighbours: where the face told the stems apart, the shape decides.
            if !open[at] {
                continue;
            }
            // A mark inside a word is a letter or a figure, so the reading is taken off the
            // best of those the mark's ink could be. Where that is a character of its own
            // rather than one of the stems, it IS the character: nothing about the
            // neighbours says otherwise. Where it is a stem, the shape's own reading stands
            // for the neighbours to settle below, since what the shape said about which
            // stem — a tail on the l, a serif on the I, neither on the bar — is what the
            // position is read against.
            let Some(letter) = letters[at] else {
                continue;
            };
            if !glyphs::STEMS.contains(&letter) {
                named[at].0 = letter;
                continue;
            }
            // What stands NEAREST it, either side, skipping the stems whose own reading is
            // just as open as this one's: the closer a character is, the more it says about
            // which of the four this one is.
            let nearest = |mut over: Box<dyn Iterator<Item = usize>>| -> Option<char> {
                over.find(|other| !open[*other]).map(|other| named[other].0)
            };
            let before = nearest(Box::new((word.start..at).rev()));
            let after = nearest(Box::new(at + 1..word.end));
            let beside: Vec<char> = before.into_iter().chain(after).collect();
            let any = |which: fn(&char) -> bool| beside.iter().any(which);
            named[at].0 = if any(|ch| ch.is_ascii_digit()) {
                '1'
            } else if any(|ch| ch.is_lowercase()) {
                // Among small letters the mark is a letter, but which of the two letters it
                // is the shape may still have had an opinion about — a tail on the l, a
                // serif on the I — so that opinion stands. Where it named neither letter,
                // the position decides: a stem that BEGINS a word of small letters is the
                // capital that begins it, and one inside such a word is the small letter.
                match named[at].0 {
                    letter @ ('I' | 'l') => letter,
                    _ if at == word.start => 'I',
                    _ => 'l',
                }
            } else if any(|ch| ch.is_uppercase()) {
                'I'
            } else {
                named[at].0
            };
        }
    }
    named
}

/// Whether the line's `step`-th named mark was read as a FIGURE.
fn is_figure(named: &[(char, f32)], step: usize) -> bool {
    named
        .get(step)
        .is_some_and(|(ch, _)| ch.is_ascii_digit())
}

/// The advance this line sets its FIGURES on, in pixels, or `None` for a line that sets no
/// two figures side by side.
///
/// The median step from one figure to the next over the pairs of neighbouring figures the
/// line's own gaps do not separate: those are figures of ONE number, so their step is the
/// advance the face sets every figure on, whatever each of them inks inside it.
///
/// A line whose every pair of figures its gaps DO separate still sets them on that advance —
/// a number of nothing but narrow figures leaves a gap like a space's at every one of its
/// own steps — and there the smallest step two of its figures stand apart by is the estimate,
/// since no two figures of one face stand closer together than its advance.
fn figure_advance(
    marks: &[Settled],
    separated: &[(usize, Separation)],
    named: &[(char, f32)],
) -> Option<f32> {
    let step_between = |step: usize| {
        let before = marks[separated[step - 1].0].glyph;
        let after = marks[separated[step].0].glyph;
        after.glyph_step_from(&before)
    };
    let figures: Vec<usize> = (1..separated.len())
        .filter(|step| is_figure(named, *step) && is_figure(named, step - 1))
        .collect();
    let together: Vec<f32> = figures
        .iter()
        .filter(|step| separated[**step].1 == Separation::None)
        .map(|step| step_between(*step))
        .collect();
    median_of(&together).or_else(|| {
        figures
            .iter()
            .map(|step| step_between(*step))
            .reduce(f32::min)
    })
}

/// How one line's named marks fall into WORDS: the separator rule the line's own marks set,
/// what separates each named mark from the one before it, and the character each was read
/// as.
///
/// [`Separated::marks`] and [`Separated::named`] are index-aligned, and both hold one entry
/// per NAMED mark of the line in reading order: a mark that matched no glyph contributes no
/// character and separates nothing.
struct Separated {
    separators: Separators,
    marks: Vec<(usize, Separation)>,
    named: Vec<(char, f32)>,
}

/// Separate one line's `marks` into its words, measured off THAT line's own marks.
///
/// The ONE place a line's word and column breaks are decided, so the text, the measured
/// words and the reading of its bare stems cannot disagree about where its words are.
///
/// Two marks are separated by what they stand apart PAST the gap the characters they were
/// named as leave when a proportional face sets them side by side ([`spacing::Letterforms`]),
/// which is what a word space measures; a pair with no letterforms to measure against, and a
/// line set in one CELL per character, are separated by the line's own gaps and steps
/// ([`Separators`]).
fn separate(marks: &[Settled], line: glyphs::Line) -> Separated {
    let boxes: Vec<GlyphBox> = marks.iter().map(|mark| mark.glyph).collect();
    let median_width = median(boxes.iter().map(GlyphBox::width)).unwrap_or(1.0);
    let gaps: Vec<usize> = boxes
        .windows(2)
        .map(|pair| pair[1].x0.saturating_sub(pair[0].x1 + 1))
        .collect();
    // The STEP from each mark to the next: the pitch the line's characters are set at,
    // which is what its word and column separators are measured against.
    let steps: Vec<usize> = boxes
        .windows(2)
        .map(|pair| pair[1].x0.saturating_sub(pair[0].x0))
        .collect();
    // Which marks a line separates into words, and which into COLUMNS, is a property of
    // THIS line's own marks: see [`Separators`].
    let separators = Separators::of(&boxes, &gaps, &steps, median_width);
    // What the characters this line's marks were named as leave between them, which is what
    // its gaps are measured against where it sets each of them in its own width.
    let letterforms = match separators {
        Separators::Monospaced { .. } => None,
        Separators::Proportional { .. } => spacing::Letterforms::of(marks, line),
    };
    let mut separated: Vec<(usize, Separation)> = Vec::new();
    let mut previous: Option<(&GlyphBox, char)> = None;
    for (at, mark) in marks.iter().enumerate() {
        let Some(named) = mark.named else {
            continue;
        };
        let separation = match previous {
            None => Separation::Word,
            Some((before, before_ch)) => match separators.between(before, &mark.glyph) {
                // A gap of several character pitches is the boundary of two COLUMNS
                // whatever the letterforms either side of it leave, which no space covers.
                Separation::Column => Separation::Column,
                measured => letterforms
                    .and_then(|set| set.between(before, before_ch, &mark.glyph, named.ch))
                    .unwrap_or(measured),
            },
        };
        separated.push((at, separation));
        previous = Some((&mark.glyph, named.ch));
    }
    let named = settle_stems(marks, &separated);
    // A face sets its FIGURES on one advance, so the gap a narrow figure leaves inside its
    // own cell is as wide as a space's: where the line shows that advance, a mark that
    // FOLLOWS a figure is separated by what its step covers past that advance rather than by
    // the gap the figure's empty cell left (see [`Separators::between_set_on`]). It is the
    // left mark that has to be the figure, since what the step covers is the advance of the
    // mark it starts from — which is why the point of an amount stays part of it. The stems
    // are then read again, since what a bare stem is depends on the word it stands in.
    let named = match figure_advance(marks, &separated, &named) {
        None => named,
        Some(advance) => {
            for step in 1..separated.len() {
                if !is_figure(&named, step - 1) {
                    continue;
                }
                let before = marks[separated[step - 1].0].glyph;
                let after = marks[separated[step].0].glyph;
                separated[step].1 = separators.between_set_on(&before, &after, advance);
            }
            settle_stems(marks, &separated)
        }
    };
    Separated {
        separators,
        marks: separated,
        named,
    }
}

impl Separated {
    /// The words this line was separated into, in reading order: a word is a run of named
    /// marks nothing separates.
    #[cfg(test)]
    fn words(&self) -> Vec<String> {
        let mut out: Vec<String> = Vec::new();
        for (step, (_, separation)) in self.marks.iter().enumerate() {
            let ch = self.named[step].0;
            match out.last_mut() {
                Some(word) if *separation == Separation::None => word.push(ch),
                _ => out.push(String::from(ch)),
            }
        }
        out
    }
}

impl InkLine {
    /// How many CELLS the rules `crossing` this line separate the mark ending at column
    /// `before` from the mark starting at column `after` by.
    ///
    /// A ruling line drawn between two marks of a row is the boundary of the cells they
    /// stand in, whatever the gap between them measures, and two rules with no mark between
    /// them bound a cell nothing was drawn in — so the count is the number of rules crossed
    /// rather than whether any was.
    ///
    /// `crossing` is in column order, so the rules standing in one gap are a RUN of it: the
    /// run is found by a search and walked only as far as the gap reaches. Reading a line
    /// therefore costs its own marks and the rules that really stand between them rather than
    /// the product of the two counts.
    ///
    /// A cell at either END of a row is bounded by one rule and the table's own border
    /// rather than by two marks, so the ends are counted on their own
    /// ([`Self::outside`]).
    fn divided(crossing: &[&Rule], before: usize, after: usize) -> usize {
        let from = crossing.partition_point(|rule| rule.left() <= before);
        crossing
            .get(from..)
            .unwrap_or_default()
            .iter()
            .take_while(|rule| rule.left() < after)
            .filter(|rule| rule.divides(before, after))
            .count()
    }

    /// How many CELLS of a ruled row stand OUTSIDE its marks on one side: the rules standing
    /// clear of them, less the outermost, which is the table's own border.
    ///
    /// A row whose first or last cell was left empty is bounded there by a rule and that
    /// border rather than by two marks, so counting only the rules BETWEEN two marks would
    /// report one cell too few and shift every `ColumnIndex` after it. `beyond` says which
    /// side is meant: the rules that end before the row's first mark, or the ones that begin
    /// after its last.
    fn outside(crossing: &[&Rule], beyond: impl Fn(&Rule) -> bool) -> usize {
        crossing
            .iter()
            .filter(|rule| beyond(rule))
            .count()
            .saturating_sub(1)
    }

    /// Recognise this line: its characters, and the word and column breaks between them.
    /// `Ok(None)` when nothing in the line matched a glyph, so the line carries no text.
    ///
    /// `ruled` are the rules drawn on the page, which delimit this line's cells.
    fn render(
        &self,
        ink: &Ink,
        ruled: &[Rule],
        budget: &mut Budget,
    ) -> Result<Option<RenderedLine>, Unreadable> {
        // The rules that cross THIS line, in column order: taken once for the line rather
        // than once per mark of it.
        let crossing: Vec<&Rule> = ruled
            .iter()
            .filter(|rule| rule.crosses(self.top, self.bottom))
            .collect();
        // Where this line sets its characters, which is the frame each of its marks is
        // measured and named in.
        let line = glyphs::line_of(&self.glyphs, self.top, self.bottom);
        // What its marks ARE: a run of inked columns is one mark only until the naming
        // says otherwise (see [`settle`]).
        let marks = settle(ink, self, line, budget)?;
        // Where this line's words are, and what each of its marks was read as.
        let Separated {
            separators,
            marks: separated,
            named,
        } = separate(&marks, line);
        // The character cell the line's left margin is reported in, so a line that
        // starts half way across the page is laid out half way across its text.
        let cell = separators.cell();

        let mut text = String::new();
        let mut score_total = 0.0f32;
        let mut score_count = 0usize;
        let mut words: Vec<WordRun> = Vec::new();

        for (step, (at, separation)) in separated.iter().enumerate() {
            let glyph = &marks[*at].glyph;
            let (ch, score) = named[step];
            // Whether this glyph starts a new word is decided by the same gaps the text
            // is separated on, so the measured words and the words of the text are one
            // and the same list.
            let starts_word = if step == 0 {
                let indent = ((glyph.x0 as f32 / cell).round() as usize).min(MAX_INDENT);
                text.push_str(&" ".repeat(indent));
                // The cells the row's ruling lines bound to the LEFT of its first mark, each
                // one a cell nothing was drawn in.
                let leading = Self::outside(&crossing, |rule| rule.right() < glyph.x0);
                for _ in 0..leading.min(MAX_INDENT) {
                    text.push('\t');
                }
                true
            } else {
                // A rule drawn down the page between this mark and the one before it is the
                // boundary of two CELLS of a ruled row, so it separates them into columns
                // whatever their own gap measures.
                let before = &marks[separated[step - 1].0].glyph;
                match Self::divided(&crossing, before.x1, glyph.x0) {
                    0 => match separation {
                        Separation::Column => {
                            text.push('\t');
                            true
                        }
                        Separation::Word => {
                            text.push(' ');
                            true
                        }
                        Separation::None => false,
                    },
                    crossed => {
                        for _ in 0..crossed.min(MAX_INDENT) {
                            text.push('\t');
                        }
                        true
                    }
                }
            };
            text.push(ch);
            let named_ink = (score - glyphs::MIN_MATCH) * marks[*at].ink;
            match words.last_mut() {
                Some(word) if !starts_word => word.extend(glyph, score, named_ink),
                _ => words.push(WordRun::starting_at(glyph, score, named_ink)),
            }
            score_total += score;
            score_count += 1;
        }
        if score_count == 0 {
            return Ok(None);
        }
        // The cells the row's ruling lines bound to the RIGHT of its last mark, counted the
        // same way as the ones to the left of its first.
        if let Some(last) = separated.last().map(|(at, _)| marks[*at].glyph) {
            let trailing = Self::outside(&crossing, |rule| rule.left() > last.x1);
            for _ in 0..trailing.min(MAX_INDENT) {
                text.push('\t');
            }
        }
        Ok(Some(RenderedLine {
            text,
            score_total,
            score_count,
            // Only the ink of a WORD of several characters counts towards how well the page
            // was read: see [`PageRead::strength`].
            named_ink: words
                .iter()
                .filter(|word| word.score_count >= MIN_WORD_CHARACTERS)
                .map(|word| word.named_ink)
                .sum(),
            words,
        }))
    }
}

/// The value `share` of the way along `values` in order, or `None` when there are none.
///
/// The ONE order statistic this module takes: the median of a line's marks, of a run's own
/// columns, and the decile a line's capital height is read off ([`glyphs::line_of`]) are all
/// this, so none of them is a second implementation of sorting and indexing.
pub(super) fn quantile(values: &[f32], share: f32) -> Option<f32> {
    if values.is_empty() {
        return None;
    }
    let mut sorted = values.to_vec();
    sorted.sort_by(f32::total_cmp);
    let at = ((sorted.len() - 1) as f32 * share).round() as usize;
    sorted.get(at).copied()
}

/// The median of `values`, or `None` when there are none.
fn median_of(values: &[f32]) -> Option<f32> {
    quantile(values, 0.5)
}

/// The median of `values`, or `None` when there are none.
fn median(values: impl Iterator<Item = usize>) -> Option<f32> {
    median_of(&values.map(|value| value as f32).collect::<Vec<f32>>())
}

/// What separates two neighbouring marks of one line.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Separation {
    /// Nothing: they are two marks of one word.
    None,
    /// A word space.
    Word,
    /// A column break — the gap between two cells of a table row.
    Column,
}

/// How one line's marks are separated into words and into columns, measured off THAT
/// line's own marks.
///
/// A word space is a character cell nothing was drawn in, so what identifies one is the
/// cell, and a line is set in one of two ways:
///
/// * MONOSPACED — every mark sits a whole number of cells from the line's first mark
///   ([`Separators::lattice_cell`]). The gaps a letter's own side bearings leave on such a
///   line are a large part of its cell and no narrower than a space's, so what separates
///   two words here is a STEP that skips a cell.
/// * PROPORTIONAL — the marks fit no cell, because each letter advances by its own width.
///   The side bearings are then a small part of the pitch while a space is most of one, so
///   the GAP between two marks separates them: the line's own gaps fall into a cluster of
///   bearings and a cluster of spaces, and the boundary between the two is where the line
///   separates its words ([`Separators::clustered_gap`]). A line that shows no such
///   boundary is separated at a fixed part of its pitch ([`WORD_GAP_PITCH`]), which is
///   what a line of one word needs so that nothing inside it is read as a space; and
///   either way the boundary stands clear of the line's own typical gap by the width of
///   the narrowest space a face sets ([`MIN_SPACE_PITCH`]), since a space is a bearing and
///   a space's own advance where a bearing is the bearing alone. Its
///   FIGURES are the exception, since a proportional face sets those on one advance all the
///   same: they are separated by their STEP past that advance
///   ([`Separators::between_set_on`]).
///
/// Measuring against the line's pitch rather than against an absolute width is what reads
/// text drawn small and text drawn large by the one rule.
#[derive(Debug)]
enum Separators {
    /// The line is set on a lattice of this cell, in pixels.
    Monospaced { cell: f32 },
    /// The line's marks fit no cell; its characters are set at this pitch, in pixels, and
    /// a gap of `word` pixels or more separates two of its words.
    Proportional { pitch: f32, word: f32 },
}

impl Separators {
    /// The separators of the line `glyphs` are the marks of, with the gaps and the steps
    /// between them already taken.
    fn of(glyphs: &[GlyphBox], gaps: &[usize], steps: &[usize], median_width: f32) -> Self {
        // The pitch is the line's median step, except where more of its marks stand apart
        // than together (a line of single-letter words): there the median step is a WORD's
        // step rather than a character's, and the median mark plus the median gap is the
        // narrower estimate.
        let median_gap = median(gaps.iter().copied()).unwrap_or(0.0);
        let typical = median_width + median_gap;
        let pitch = median(steps.iter().copied())
            .unwrap_or(typical)
            .min(typical)
            .max(1.0);
        match Self::lattice_cell(glyphs, pitch) {
            Some(cell) => Self::Monospaced { cell },
            None => Self::Proportional {
                pitch,
                word: Self::word_gap(gaps, pitch),
            },
        }
    }

    /// The gap at which a PROPORTIONAL line separates its words: the boundary its own gaps
    /// cluster at ([`clustered_gap`](Self::clustered_gap)), or a fixed part of its pitch
    /// where they show none ([`WORD_GAP_PITCH`]), held clear of the BEARINGS it separates.
    ///
    /// A space's gap is a bearing AND a space's own advance, where a bearing is the bearing
    /// alone, so the boundary is never below the typical bearing of this line plus the
    /// narrowest space a face sets ([`MIN_SPACE_PITCH`]). Without that a line whose narrow
    /// letters leave wide bearings — an `l`, an `i`, a point, each inking a sliver of its own
    /// advance — has those bearings read as spaces, which breaks its words in two.
    ///
    /// Which gaps the bearings ARE is what the boundary says, so raising it takes more gaps
    /// into them and the two are settled together: the boundary is raised until it stands
    /// clear of the bearings below it, which is where it stops moving. A fixed few passes
    /// ([`GAP_PASSES`]), since each one takes in more gaps than the last and a line has as
    /// many distinct gaps as it has marks.
    fn word_gap(gaps: &[usize], pitch: f32) -> f32 {
        let mut word = Self::clustered_gap(gaps, pitch).unwrap_or(WORD_GAP_PITCH * pitch);
        for _ in 0..GAP_PASSES {
            let bearings: Vec<f32> = gaps
                .iter()
                .map(|gap| *gap as f32)
                .filter(|gap| *gap < word)
                .collect();
            let Some(typical) = median_of(&bearings) else {
                break;
            };
            let clear = typical + MIN_SPACE_PITCH * pitch;
            if clear <= word {
                break;
            }
            word = clear;
        }
        word
    }

    /// The gap at which a PROPORTIONAL line separates its words, read off the clusters its
    /// own gaps fall into: the bearings inside its words, and the spaces between them.
    ///
    /// The boundary is the lowest jump in the line's sorted gaps that both doubles the gaps
    /// below it and leaves a gap of at least [`MIN_SPACE_PITCH`] of the line's pitch above
    /// it — a space takes a part of a character cell, while a bearing is the sliver a
    /// letter's own shape leaves. Reading the boundary off the line rather than fixing it
    /// is what separates the words of a line set TIGHT, whose spaces are narrow but still
    /// stand clear of its bearings.
    ///
    /// `None` for a line whose gaps show no such boundary, which is what a line of one word
    /// looks like.
    fn clustered_gap(gaps: &[usize], pitch: f32) -> Option<f32> {
        let mut sorted: Vec<usize> = gaps.to_vec();
        sorted.sort_unstable();
        sorted.dedup();
        sorted
            .windows(2)
            .find(|pair| {
                let (bearing, space) = (pair[0], pair[1]);
                space >= 2 * bearing.max(1) && space as f32 >= MIN_SPACE_PITCH * pitch
            })
            .map(|pair| (pair[0] + pair[1]) as f32 / 2.0)
    }

    /// The cell a line's marks are set in, when they sit on a LATTICE: the cell that leaves
    /// every mark closest to a whole number of cells from the line's first mark, when even
    /// the closest leaves them no further out than [`MAX_LATTICE_DRIFT`].
    ///
    /// `None` for a line whose marks fit no cell — a proportional rendering, where each
    /// letter advances by its own width — and for a line of too few marks
    /// ([`MIN_LATTICE_MARKS`]) for the fit to say anything.
    ///
    /// The marks are placed by their CENTRES, which is where a cell holds them whatever ink
    /// the glyph puts in it, and the cell is searched for around `pitch`, the step most of
    /// the line's marks are a cell apart by.
    ///
    /// Both the distance of each mark from the line's FIRST and the STEP from each mark to
    /// the next have to come out a whole number of cells, because a line set in the width of
    /// its own characters fits a cell by coincidence as often as not: the steps of a short
    /// line of such a face — a wide letter, then a narrow one — can cancel each other out
    /// over the line while no two neighbouring marks stand a cell apart at all.
    fn lattice_cell(glyphs: &[GlyphBox], pitch: f32) -> Option<f32> {
        if glyphs.len() < MIN_LATTICE_MARKS || pitch < 2.0 {
            return None;
        }
        let centres: Vec<f32> = glyphs
            .iter()
            .map(|glyph| (glyph.x0 + glyph.x1) as f32 / 2.0)
            .collect();
        let mut best: Option<(f32, f32)> = None;
        for offset in -LATTICE_SEARCH..=LATTICE_SEARCH {
            let cell = pitch * (1.0 + offset as f32 / 64.0);
            let drift = Self::lattice_drift(&centres, cell)
                .max(Self::step_drift(&centres, cell));
            if best.is_none_or(|(lowest, _)| drift < lowest) {
                best = Some((drift, cell));
            }
        }
        best.filter(|(drift, _)| *drift <= MAX_LATTICE_DRIFT)
            .map(|(_, cell)| cell)
    }

    /// How far from a whole number of `cell`s the STEP from each of `centres` to the next
    /// sits, in cells, on average over the line.
    ///
    /// What the drift from the line's first mark cannot see: a step of half a cell followed
    /// by one of one and a half leaves both marks a whole number of cells from the first,
    /// and no face sets its characters that way.
    fn step_drift(centres: &[f32], cell: f32) -> f32 {
        let steps: Vec<f32> = centres
            .windows(2)
            .map(|pair| (pair[1] - pair[0]) / cell)
            .collect();
        if steps.is_empty() {
            return 0.0;
        }
        steps
            .iter()
            .map(|cells| (cells - cells.round()).abs())
            .sum::<f32>()
            / steps.len() as f32
    }

    /// How far from a whole number of `cell`s the marks at `centres` sit, in cells, on
    /// average over the line.
    fn lattice_drift(centres: &[f32], cell: f32) -> f32 {
        let first = centres.first().copied().unwrap_or_default();
        let total: f32 = centres
            .iter()
            .map(|centre| {
                let cells = (centre - first) / cell;
                (cells - cells.round()).abs()
            })
            .sum();
        total / centres.len() as f32
    }

    /// What separates the mark `before` from the mark `after`, which follows it on the
    /// same line.
    fn between(&self, before: &GlyphBox, after: &GlyphBox) -> Separation {
        let (measured, word, column) = match *self {
            Self::Monospaced { cell } => (
                after.cell_step_from(before),
                LATTICE_WORD_CELLS * cell,
                LATTICE_COLUMN_CELLS * cell,
            ),
            Self::Proportional { pitch, word } => (
                after.x0.saturating_sub(before.x1 + 1) as f32,
                word,
                COLUMN_GAP_PITCH * pitch,
            ),
        };
        Self::verdict(measured, word, column)
    }

    /// What separates two marks the line sets on ONE ADVANCE of `advance` pixels, whatever
    /// each of them inks inside it: the step past that advance, rather than the gap their
    /// own shapes leave.
    ///
    /// This is how a line separates its FIGURES. A face sets its figures on a single
    /// advance — tabular figures, so columns of them line up — so a narrow figure leaves
    /// the gap of its own empty cell either side of it, as wide as a space's, while the
    /// step from one figure to the next is the advance whether the figure is a `1` or an
    /// `8`. Measuring the gap there reads a number as several words; measuring the step
    /// past the advance reads a space between two figures only where the line really left
    /// one.
    ///
    /// A MONOSPACED line already measures every separation by the step it skips, so there
    /// is nothing of its own for a figure to be measured by.
    fn between_set_on(&self, before: &GlyphBox, after: &GlyphBox, advance: f32) -> Separation {
        match *self {
            Self::Monospaced { .. } => self.between(before, after),
            Self::Proportional { pitch, .. } => Self::verdict(
                after.glyph_step_from(before) - advance,
                FIGURE_SPACE_PITCH * pitch,
                COLUMN_GAP_PITCH * pitch,
            ),
        }
    }

    /// What a separation of `measured` pixels is, against the widths this line separates
    /// its words and its columns at.
    fn verdict(measured: f32, word: f32, column: f32) -> Separation {
        if measured >= column {
            Separation::Column
        } else if measured >= word {
            Separation::Word
        } else {
            Separation::None
        }
    }

    /// The character cell this line's left margin is reported in, so a line that starts
    /// half way across the page is laid out half way across its text.
    fn cell(&self) -> f32 {
        match *self {
            Self::Monospaced { cell } => cell,
            // A proportional line has no cell; its characters take a pitch each.
            Self::Proportional { pitch, .. } => pitch,
        }
        .max(1.0)
    }
}

#[cfg(test)]
mod probe;

/// The ink one piece of a cut run is named from: the stroke the cut severed.
#[cfg(test)]
#[path = "ocr/trial.rs"]
mod severed_strokes;

/// Reading the characters of a token off a page set in a proportional face.
#[cfg(test)]
#[path = "ocr/diag.rs"]
mod tokens;

/// How a line that sets its characters on one advance is separated into words.
#[cfg(test)]
#[path = "ocr/scratch.rs"]
mod figures;

#[cfg(test)]
mod tests {
    use super::*;

    /// The naming one request may do is bounded whatever the page: the budget admits exactly
    /// [`MAX_NAMINGS_PER_PAGE`] namings, wherever in the reading they happen, and then says
    /// the page is too large rather than letting the readings, the runs and the pieces of a
    /// run multiply the work between them.
    #[test]
    fn the_naming_one_request_may_do_is_bounded() {
        let mut budget = Budget::new();
        for spent in 0..MAX_NAMINGS_PER_PAGE {
            assert!(
                budget.spend().is_ok(),
                "case naming {spent}: a request may name {MAX_NAMINGS_PER_PAGE} marks"
            );
        }
        let refused = budget.spend().expect_err("the budget must run out");
        let Unreadable::TooLarge(message) = &refused else {
            panic!("case the budget is spent: the page must be refused as too large, got {refused:?}");
        };
        assert!(
            message.contains(&MAX_NAMINGS_PER_PAGE.to_string()),
            "case the budget is spent: the refusal must name the ceiling it is, got {message:?}"
        );
        assert!(
            budget.spend().is_err(),
            "case past the budget: it stays spent"
        );
    }

    /// The side of one glyph cell of the 8x8 font these tests draw with.
    const CELL: usize = glyphs::FONT_CELL;

    /// The columns of the font's cell that `ch` puts ink in.
    fn glyph_extent(ch: char) -> (usize, usize) {
        let rows = font8x8::legacy::BASIC_LEGACY[ch as usize];
        let mut first = CELL;
        let mut last = 0usize;
        for bits in &rows {
            for bit in 0..CELL {
                if bits & (1 << bit) != 0 {
                    first = first.min(bit);
                    last = last.max(bit);
                }
            }
        }
        (first, last)
    }

    /// The marks of a line drawn PROPORTIONALLY: each letter takes the width of its own
    /// ink, and the space between two words is narrower than a letter.
    fn proportional(line: &str, scale: usize, letter_gap: usize, word_gap: usize) -> Vec<GlyphBox> {
        let mut out = Vec::new();
        let mut x = 0usize;
        for (index, word) in line.split(' ').enumerate() {
            if index > 0 {
                x += word_gap;
            }
            for (letter, ch) in word.chars().enumerate() {
                if letter > 0 {
                    x += letter_gap;
                }
                let (first, last) = glyph_extent(ch);
                let width = (last + 1 - first) * scale;
                out.push(GlyphBox {
                    x0: x,
                    x1: x + width - 1,
                    y0: 0,
                    y1: CELL * scale,
                });
                x += width;
            }
        }
        out
    }

    /// The marks of a line drawn MONOSPACED: every character in a cell of one width, where
    /// the gaps a letter's own ink leaves inside its cell are as wide as a space's.
    fn monospaced(line: &str, scale: usize) -> Vec<GlyphBox> {
        line.chars()
            .enumerate()
            .filter(|(_, ch)| *ch != ' ')
            .map(|(column, ch)| {
                let (first, last) = glyph_extent(ch);
                GlyphBox {
                    x0: (column * CELL + first) * scale,
                    x1: (column * CELL + last) * scale + scale - 1,
                    y0: 0,
                    y1: CELL * scale,
                }
            })
            .collect()
    }

    /// How the separators read one line's marks: one entry per pair of neighbours.
    fn separations(glyphs: &[GlyphBox]) -> Vec<Separation> {
        let median_width = median(glyphs.iter().map(GlyphBox::width)).unwrap_or(1.0);
        let gaps: Vec<usize> = glyphs
            .windows(2)
            .map(|pair| pair[1].x0.saturating_sub(pair[0].x1 + 1))
            .collect();
        let steps: Vec<usize> = glyphs
            .windows(2)
            .map(|pair| pair[1].x0.saturating_sub(pair[0].x0))
            .collect();
        let separators = Separators::of(glyphs, &gaps, &steps, median_width);
        glyphs
            .windows(2)
            .map(|pair| separators.between(&pair[0], &pair[1]))
            .collect()
    }

    /// What `line`'s own spaces separate its marks into.
    fn drawn(line: &str) -> Vec<Separation> {
        let mut out = Vec::new();
        for (index, word) in line.split(' ').enumerate() {
            if index > 0 {
                out.push(Separation::Word);
            }
            out.extend((1..word.chars().count()).map(|_| Separation::None));
        }
        out
    }

    /// The marks of a line are separated into the words the line was drawn from, whether it
    /// sets its letters in cells of one width or advances each letter by its own width, and
    /// at every size it is drawn at.
    #[test]
    fn a_lines_marks_are_separated_into_the_words_it_was_drawn_from() {
        let labelled = "Account Number: 48213";
        let capitals = "SIMPARITY LEDGER";
        let cases: Vec<(&str, &str, Vec<GlyphBox>)> = vec![
            (
                "proportional, drawn small",
                labelled,
                proportional(labelled, 1, 1, 4),
            ),
            ("proportional", labelled, proportional(labelled, 2, 1, 7)),
            (
                "proportional, drawn large with wide letter spacing",
                labelled,
                proportional(labelled, 4, 4, 22),
            ),
            (
                "proportional capitals, whose widths hardly differ",
                capitals,
                proportional(capitals, 2, 1, 7),
            ),
            (
                "monospaced, drawn small",
                labelled,
                monospaced(labelled, 1),
            ),
            ("monospaced", labelled, monospaced(labelled, 3)),
            ("monospaced capitals", capitals, monospaced(capitals, 2)),
        ];
        for (case, line, glyphs) in cases {
            assert_eq!(
                separations(&glyphs),
                drawn(line),
                "case {case:?}: the marks of {line:?} must be separated into its own words"
            );
        }
    }
}
