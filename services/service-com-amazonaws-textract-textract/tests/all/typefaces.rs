// SPDX-License-Identifier: CC0-1.0

//! Reading a document set in the TYPEFACES ordinary documents are set in, end to end.
//!
//! The rule under test, over a table of inputs rather than one recorded page: the `Text` a
//! raster operation reports for a block is the characters that were DRAWN on the page it
//! read, whatever typeface and whatever size drew them, and the words it separates them
//! into are the words the page sets — including a NUMBER, whose figures a face sets on one
//! advance of its own, so the empty part of a narrow figure's cell is not a space.
//!
//! Two families of page are drawn here, because a document is set in one or the other and
//! the recogniser has to read both:
//!
//! * a PIXEL face — a page drawn a dozen pixels to the line, every stroke one pixel, which
//!   is what a small scanned or screen-rendered document carries. The rows span the rasters
//!   the X11 fixed faces are cut at from 6x9 to 10x20 (`embedded_graphics::mono_font::ascii`,
//!   the source of the list below). The two smallest cuts those faces come in, 4x6 and 5x7,
//!   and the 5x8 between them, are deliberately NOT here: a page whose capitals stand six
//!   pixels tall sets neighbouring letters a pixel apart, which no recogniser reads
//!   character for character, so asserting a round trip at those rasters would assert
//!   something this module does not promise;
//! * a PROPORTIONAL OUTLINE face — Noto Sans, rasterised here at the size each row asks
//!   for, where every letter takes the width of its own ink rather than one cell. These
//!   rows are drawn at sizes the recogniser holds NO template at, so what they pin is
//!   recognition rather than identity: an outline rasterised at 26 pixels is a different
//!   bitmap on a different pixel grid from the same outline rasterised at 18.
//!
//! The rows differ along what the rule generalises over: the family, the raster or pixel
//! size, the weight, the polarity of the ink, and the format the page is submitted in.
//!
//! The rows of one family are spread over several tests, each reading its own pages through
//! one shared helper: recognising a page is the work of reading its every mark, so a test
//! that read a whole family's table would hold up every build and every merge it runs in for
//! as long as all of those pages take, while the tests of one family run beside each other.
//! Two or three pages a test is what that comes to: the rows are the same rows and each is
//! asserted the same way, and no one of them holds a build up for the rest.

use embedded_graphics::draw_target::DrawTarget;
use embedded_graphics::geometry::{Dimensions, Point, Size};
use embedded_graphics::mono_font::{ascii, MonoFont, MonoTextStyle};
use embedded_graphics::pixelcolor::BinaryColor;
use embedded_graphics::primitives::Rectangle;
use embedded_graphics::text::Text;
use embedded_graphics::{Drawable, Pixel};
use image::{GrayImage, ImageFormat, Luma};
use serde_json::{json, Value};

use crate::document_extraction::{assert_block_graph, blocks_of, call, harness, of_type};
use crate::raster_documents::{raster_document, texts_of};

/// The lines every page here is drawn from: capitals, small letters, figures, a labelled
/// field and the punctuation a statement is set with, so more than one glyph class is read.
const LINES: &[&str] = &[
    "SIMPARITY LEDGER",
    "Account Number: 48213",
    "Total Due 1250.00",
    "Status: Settled in full",
];

/// The numbers `LINES` draws, each of which is ONE word of the page: a face sets its
/// figures on one advance, so the gap a narrow figure leaves inside its own cell is as wide
/// as a space's and a number read by its gaps alone falls apart into several words.
const NUMBERS: &[&str] = &["48213", "1250.00"];

/// How the ink and the ground of a page are drawn.
///
/// The polarity axis itself is `crate::raster_documents::Paper`'s — "one renderer serves
/// both rather than each suite growing its own" — so the levels come from there and only the
/// blending an ANTI-ALIASED stroke needs is added here: a bitmap face turns a pixel on or
/// off, while an outline rasteriser hands out how much of a stroke each pixel carries.
pub(crate) type Polarity = crate::raster_documents::Paper;

/// A page of dark ink on a light ground.
pub(crate) const fn dark_on_light() -> Polarity {
    Polarity {
        background: 245,
        ink: 0,
        ..Polarity::plain()
    }
}

/// A page of light ink on a dark ground, which Textract reads just as well.
const fn light_on_dark() -> Polarity {
    Polarity {
        background: 12,
        ink: 255,
        ..Polarity::plain()
    }
}

/// The page level for a pixel that already carries `ground` and takes `coverage` of a
/// stroke's ink, drawn at `polarity`.
fn level(polarity: Polarity, ground: u8, coverage: u8) -> u8 {
    if polarity.ink >= polarity.background {
        ground.max(polarity.background.saturating_add(coverage))
    } else {
        ground.min(polarity.background.saturating_sub(coverage))
    }
}

/// The smallest draw target there is: it records which of its pixels a face turned on.
///
/// `embedded_graphics` draws a bitmap face's glyphs rather than handing out their bits, so
/// a page set in one is drawn into this and read off it.
struct Canvas {
    width: usize,
    height: usize,
    lit: Vec<bool>,
}

impl Dimensions for Canvas {
    fn bounding_box(&self) -> Rectangle {
        Rectangle::new(
            Point::zero(),
            Size::new(self.width as u32, self.height as u32),
        )
    }
}

impl DrawTarget for Canvas {
    type Color = BinaryColor;
    type Error = core::convert::Infallible;

    fn draw_iter<I>(&mut self, pixels: I) -> Result<(), Self::Error>
    where
        I: IntoIterator<Item = Pixel<Self::Color>>,
    {
        for Pixel(point, colour) in pixels {
            let inside = point.x >= 0
                && point.y >= 0
                && (point.x as usize) < self.width
                && (point.y as usize) < self.height;
            if inside && colour == BinaryColor::On {
                self.lit[point.y as usize * self.width + point.x as usize] = true;
            }
        }
        Ok(())
    }
}

/// Draw `lines` with one of the X11 fixed bitmap faces, at that face's own raster.
///
/// `pub(crate)`: `crate::page_turns_of_a_pixel_face` draws its pages with this same
/// renderer rather than growing one of its own.
pub(crate) fn pixel_page(
    lines: &[&str],
    font: &MonoFont<'static>,
    polarity: Polarity,
) -> GrayImage {
    let cell = font.character_size;
    let pitch = cell.height as usize + 5;
    let margin = 12usize;
    let columns = lines
        .iter()
        .map(|line| line.chars().count())
        .max()
        .unwrap_or(0);
    let width = margin * 2 + columns * cell.width.max(1) as usize;
    let height = margin * 2 + lines.len() * pitch;
    let mut canvas = Canvas {
        width,
        height,
        lit: vec![false; width * height],
    };
    let style = MonoTextStyle::new(font, BinaryColor::On);
    for (row, line) in lines.iter().enumerate() {
        let at = Point::new(
            margin as i32,
            (margin + row * pitch + cell.height as usize) as i32,
        );
        // `Infallible`: the target above cannot fail, so this cannot.
        let _ = Text::new(line, at, style).draw(&mut canvas);
    }
    let mut page = GrayImage::from_pixel(width as u32, height as u32, Luma([polarity.background]));
    for y in 0..height {
        for x in 0..width {
            if canvas.lit[y * width + x] {
                page.put_pixel(x as u32, y as u32, Luma([polarity.ink]));
            }
        }
    }
    page
}

/// Draw `lines` with the PROPORTIONAL outline face `face`, rasterised at `size` pixels.
///
/// `fontdue` rasterises an outline into one byte of ink per pixel of the glyph's own ink
/// box, with the metrics that place that box against the line's baseline and advance the
/// pen by the glyph's own width — so the page carries anti-aliased strokes set
/// proportionally, which is what a document rendered from an outline face looks like.
pub(crate) fn outline_page(
    lines: &[&str],
    face: &'static [u8],
    size: f32,
    polarity: Polarity,
) -> GrayImage {
    let font = fontdue::Font::from_bytes(face, fontdue::FontSettings::default())
        .expect("the embedded outline face parses");
    let metrics = font
        .horizontal_line_metrics(size)
        .expect("the outline face sets its lines");
    let pitch = metrics.new_line_size.ceil() as usize;
    let margin = (size * 1.5).ceil() as usize;
    let width = margin * 2
        + lines
            .iter()
            .map(|line| {
                line.chars()
                    .map(|ch| font.metrics(ch, size).advance_width.ceil() as usize)
                    .sum::<usize>()
            })
            .max()
            .unwrap_or(0);
    let height = margin * 2 + lines.len() * pitch;
    let mut page = GrayImage::from_pixel(width as u32, height as u32, Luma([polarity.background]));
    for (row, line) in lines.iter().enumerate() {
        let baseline = margin + row * pitch + metrics.ascent.ceil() as usize;
        let mut pen = margin as f32;
        for ch in line.chars() {
            let (glyph, bitmap) = font.rasterize(ch, size);
            for y in 0..glyph.height {
                for x in 0..glyph.width {
                    let coverage = bitmap[y * glyph.width + x];
                    if coverage == 0 {
                        continue;
                    }
                    // `ymin` is how far the foot of the glyph's ink box stands above the
                    // baseline, so its top row is that much more than its height above it.
                    let at_x = pen as isize + glyph.xmin as isize + x as isize;
                    let above = glyph.height as isize + glyph.ymin as isize;
                    let at_y = baseline as isize - above + y as isize;
                    if at_x < 0 || at_y < 0 || at_x >= width as isize || at_y >= height as isize {
                        continue;
                    }
                    let (at_x, at_y) = (at_x as u32, at_y as u32);
                    let ground = page.get_pixel(at_x, at_y).0[0];
                    page.put_pixel(at_x, at_y, Luma([level(polarity, ground, coverage)]));
                }
            }
            pen += font.metrics(ch, size).advance_width;
        }
    }
    page
}

/// The words of `lines`, line by line, as they were drawn.
fn drawn_words(lines: &[&str]) -> Vec<Vec<String>> {
    lines
        .iter()
        .map(|line| line.split_whitespace().map(ToString::to_string).collect())
        .collect()
}

/// The LINE blocks' texts, as words.
fn read_words(blocks: &[Value]) -> Vec<Vec<String>> {
    texts_of(blocks, "LINE")
        .iter()
        .map(|line| line.split_whitespace().map(ToString::to_string).collect())
        .collect()
}

/// Read `page` through `DetectDocumentText` and hand back its blocks.
async fn read_page(
    router: &smithy_server_runtime::MultiServiceRouter,
    case: &str,
    page: &GrayImage,
    format: ImageFormat,
) -> Vec<Value> {
    let request = json!({ "Document": raster_document(page, format) });
    let (status, body) = call(router, "DetectDocumentText", &request).await;
    assert_eq!(
        status, 200,
        "case {case:?}: a page in a format Textract reads must be read, got {status} with \
         {body}"
    );
    let blocks = blocks_of(&body);
    assert_block_graph(case, &body, &blocks, None);
    assert!(
        !of_type(&blocks, "PAGE").is_empty(),
        "case {case:?}: a page that was read reports a PAGE block, got {body}"
    );
    blocks
}

/// Read a page set in each of `cases` and assert it comes back character for character.
///
/// The rows of the three tables below are run through this, so each of them asserts the same
/// thing of its own rasters: the LINE blocks report the lines the page was drawn from and the
/// WORD blocks the words it sets. The table is split across tests rather than read in one of
/// them because recognising a page is the work of reading its every mark, and a test that
/// reads a dozen pages holds up every build it runs in for as long as all of them take.
async fn pixel_pages_read_the_characters_they_were_drawn_with(
    cases: &[(&str, &MonoFont<'static>, Polarity, ImageFormat)],
) {
    let router = harness();
    for (case, font, polarity, format) in cases {
        let page = pixel_page(LINES, font, *polarity);
        let blocks = read_page(&router, case, &page, *format).await;
        assert_eq!(
            texts_of(&blocks, "LINE"),
            LINES.iter().map(ToString::to_string).collect::<Vec<_>>(),
            "case {case:?}: the LINE blocks must report the lines the page was drawn from"
        );
        assert_eq!(
            texts_of(&blocks, "WORD"),
            drawn_words(LINES).concat(),
            "case {case:?}: the WORD blocks must report the words the page was drawn from"
        );
    }
}

/// A page set in one of the SMALLEST pixel faces asserted here is read character for
/// character.
///
/// The rasters are the X11 fixed faces' own (`embedded_graphics::mono_font::ascii`): a page
/// set in one of them is what a document rendered at a dozen pixels to the line carries.
#[tokio::test]
async fn a_page_set_in_a_pixel_face_reads_the_characters_it_was_drawn_with() {
    pixel_pages_read_the_characters_they_were_drawn_with(&[
        ("6x9", &ascii::FONT_6X9, dark_on_light(), ImageFormat::Png),
        ("6x10", &ascii::FONT_6X10, dark_on_light(), ImageFormat::Png),
    ])
    .await;
}

/// The same of the MIDDLING rasters those faces are cut at: twelve and thirteen pixels to
/// the line, where a capital stands nine or ten pixels tall.
#[tokio::test]
async fn a_page_set_in_a_middling_pixel_face_reads_the_characters_it_was_drawn_with() {
    pixel_pages_read_the_characters_they_were_drawn_with(&[
        ("6x12", &ascii::FONT_6X12, dark_on_light(), ImageFormat::Png),
        ("6x13", &ascii::FONT_6X13, dark_on_light(), ImageFormat::Png),
        ("7x13", &ascii::FONT_7X13, dark_on_light(), ImageFormat::Png),
    ])
    .await;
}

/// The same of the LARGER rasters those faces are cut at, up to the largest of them.
#[tokio::test]
async fn a_page_set_in_a_large_pixel_face_reads_the_characters_it_was_drawn_with() {
    pixel_pages_read_the_characters_they_were_drawn_with(&[
        ("7x14", &ascii::FONT_7X14, dark_on_light(), ImageFormat::Png),
        ("8x13", &ascii::FONT_8X13, dark_on_light(), ImageFormat::Png),
        ("9x15", &ascii::FONT_9X15, dark_on_light(), ImageFormat::Png),
        ("9x18", &ascii::FONT_9X18, dark_on_light(), ImageFormat::Png),
        ("10x20", &ascii::FONT_10X20, dark_on_light(), ImageFormat::Png),
    ])
    .await;
}

/// The same whichever way the page's ink is drawn and whatever format it is submitted in: a
/// page of light ink on a dark ground reads as the page it is, and so does one coded as a
/// JPEG, whose coding leaves a halo around every stroke.
#[tokio::test]
async fn a_page_set_in_a_pixel_face_reads_it_however_its_ink_and_format_are() {
    pixel_pages_read_the_characters_they_were_drawn_with(&[
        (
            "9x18, light ink on a dark page",
            &ascii::FONT_9X18,
            light_on_dark(),
            ImageFormat::Png,
        ),
        (
            "9x18, submitted as a JPEG",
            &ascii::FONT_9X18,
            dark_on_light(),
            ImageFormat::Jpeg,
        ),
    ])
    .await;
}

/// Read a page set in each of `cases` and assert the text it was drawn with comes back: one
/// line per line of ink, the words the page sets, and nearly every word character for
/// character.
///
/// The glyph identities are not all asserted: ink drawn by an outline rasteriser is no copy
/// of any template, so what the rule asks for is that the page's own text comes back, not
/// that two faces' ink agrees exactly.
///
/// The rows of the four tests below are run through this, each with its own sizes, weight,
/// polarity or format: a page in this family is read twice over to pin that the blocks come
/// from its pixels, so a table of all of them in one test would hold up every build it runs
/// in for as long as sixteen readings take.
async fn outline_pages_read_the_text_they_were_drawn_with(
    cases: &[(&str, &'static [u8], f32, Polarity, ImageFormat)],
) {
    let router = harness();
    for (case, face, size, polarity, format) in cases {
        let page = outline_page(LINES, face, *size, *polarity);
        let blocks = read_page(&router, case, &page, *format).await;
        let read = read_words(&blocks);
        let drawn = drawn_words(LINES);
        assert_eq!(
            read.len(),
            drawn.len(),
            "case {case:?}: a page of {} lines of ink is read as that many lines, got {:?}",
            drawn.len(),
            texts_of(&blocks, "LINE")
        );
        for (line, (read_line, drawn_line)) in read.iter().zip(drawn.iter()).enumerate() {
            assert_eq!(
                read_line.len(),
                drawn_line.len(),
                "case {case:?}: line {line} sets {} words, got {read_line:?} against \
                 {drawn_line:?}",
                drawn_line.len()
            );
        }
        // How many of the words drawn came back character for character. A page in a face
        // no template is a copy of is measured rather than matched by identity, so what the
        // rule asks for is that most of its words are the words drawn.
        let right = read
            .concat()
            .iter()
            .zip(drawn.concat().iter())
            .filter(|(read, drawn)| read == drawn)
            .count();
        let drawn_count = drawn.concat().len();
        assert!(
            right * 4 >= drawn_count * 3,
            "case {case:?}: the words read must be the words drawn, got {right} of \
             {drawn_count} in {:?}",
            texts_of(&blocks, "LINE")
        );
        // Every block that reports a confidence reports one inside `Percent`'s range: each
        // is the score measured over the characters THAT block was read from.
        for block in &blocks {
            let Some(confidence) = block.get("Confidence").and_then(Value::as_f64) else {
                continue;
            };
            assert!(
                confidence > 0.0 && confidence <= 100.0,
                "case {case:?}: a recognised block reports the confidence it was recognised \
                 with, got {confidence} on {block}"
            );
        }
        // The same pixels answer the same way: the blocks come from what was read, not
        // from a roll.
        let again = read_page(&router, case, &page, *format).await;
        assert_eq!(
            again, blocks,
            "case {case:?}: the same page must answer with the same blocks"
        );
    }
}

/// A page set in a PROPORTIONAL outline face, at a size the recogniser holds no template
/// at, is read as the text it was drawn with.
#[tokio::test]
async fn a_page_set_in_a_proportional_outline_face_reads_the_text_it_was_drawn_with() {
    outline_pages_read_the_text_they_were_drawn_with(&[
        (
            "regular, 22 pixels",
            ttf_noto_sans::REGULAR,
            22.0,
            dark_on_light(),
            ImageFormat::Png,
        ),
        (
            "regular, 26 pixels",
            ttf_noto_sans::REGULAR,
            26.0,
            dark_on_light(),
            ImageFormat::Png,
        ),
    ])
    .await;
}

/// The same at the LARGER sizes a document is set at, where an outline is rasterised onto a
/// coarser fraction of its own strokes.
#[tokio::test]
async fn a_page_set_in_a_large_proportional_outline_face_reads_the_text_it_was_drawn_with() {
    outline_pages_read_the_text_they_were_drawn_with(&[
        (
            "regular, 32 pixels",
            ttf_noto_sans::REGULAR,
            32.0,
            dark_on_light(),
            ImageFormat::Png,
        ),
        (
            "regular, 36 pixels",
            ttf_noto_sans::REGULAR,
            36.0,
            dark_on_light(),
            ImageFormat::Png,
        ),
    ])
    .await;
}

/// The same of a page set in the BOLD cut of that face, whose heavier stroke fills the
/// counters a letter is told apart by.
#[tokio::test]
async fn a_page_set_in_a_bold_proportional_outline_face_reads_the_text_it_was_drawn_with() {
    outline_pages_read_the_text_they_were_drawn_with(&[
        (
            "bold, 24 pixels",
            ttf_noto_sans::BOLD,
            24.0,
            dark_on_light(),
            ImageFormat::Png,
        ),
        (
            "bold, 32 pixels",
            ttf_noto_sans::BOLD,
            32.0,
            dark_on_light(),
            ImageFormat::Png,
        ),
    ])
    .await;
}

/// The same whichever way the page's ink is drawn and whatever format it is submitted in.
#[tokio::test]
async fn a_page_set_in_a_proportional_outline_face_reads_it_however_its_ink_and_format_are() {
    outline_pages_read_the_text_they_were_drawn_with(&[
        (
            "regular, 26 pixels, light ink on a dark page",
            ttf_noto_sans::REGULAR,
            26.0,
            light_on_dark(),
            ImageFormat::Png,
        ),
        (
            "regular, 26 pixels, submitted as a JPEG",
            ttf_noto_sans::REGULAR,
            26.0,
            dark_on_light(),
            ImageFormat::Jpeg,
        ),
    ])
    .await;
}

/// The figures of a NUMBER are one word, however narrow a figure is and whatever advance
/// the face sets its figures on — and two numbers the page really does separate stay two.
///
/// A proportional face sets every figure on the same advance, so a `1` inks a fraction of
/// its own cell and leaves a gap as wide as a space's either side of it. A number separated
/// by those gaps alone falls into several words, which no page drew.
///
/// The rows of the three tests below are run through this, each with its own sizes and
/// weight: reading a page is the work of reading its every mark, so a table of every size in
/// one test would hold up every build it runs in for as long as all of them take.
async fn the_figures_of_a_number_are_one_word(cases: &[(&str, &'static [u8], f32)]) {
    let router = harness();
    for (case, face, size) in cases {
        let page = outline_page(LINES, face, *size, dark_on_light());
        let blocks = read_page(&router, case, &page, ImageFormat::Png).await;
        let words = texts_of(&blocks, "WORD");
        for number in NUMBERS {
            assert!(
                words.iter().any(|word| word == number),
                "case {case:?}: the figures of {number:?} are one word, got {words:?}"
            );
        }
        // The rule does not swallow the spaces a page really sets between two numbers: a
        // line of two separated numbers is read as two words of the lengths drawn.
        let separated = ["Invoice 4821 3307"];
        let page = outline_page(&separated, face, *size, dark_on_light());
        let blocks = read_page(&router, case, &page, ImageFormat::Png).await;
        let read: Vec<usize> = texts_of(&blocks, "WORD")
            .iter()
            .map(|word| word.chars().count())
            .collect();
        assert_eq!(
            read,
            vec![7, 4, 4],
            "case {case:?}: {separated:?} sets three words, got {:?}",
            texts_of(&blocks, "WORD")
        );
    }
}

/// A number's figures are one word on a page set at the sizes a document's own text is set
/// at.
#[tokio::test]
async fn the_figures_of_a_number_are_read_as_one_word() {
    the_figures_of_a_number_are_one_word(&[
        ("regular, 22 pixels", ttf_noto_sans::REGULAR, 22.0),
        ("regular, 26 pixels", ttf_noto_sans::REGULAR, 26.0),
    ])
    .await;
}

/// The same at the LARGER sizes, where the gap a narrow figure leaves inside its own cell is
/// several pixels wider than at the sizes above.
#[tokio::test]
async fn the_figures_of_a_number_are_read_as_one_word_at_a_large_size() {
    the_figures_of_a_number_are_one_word(&[
        ("regular, 32 pixels", ttf_noto_sans::REGULAR, 32.0),
        ("regular, 36 pixels", ttf_noto_sans::REGULAR, 36.0),
    ])
    .await;
}

/// The same in the BOLD cut, whose heavier stroke inks more of a figure's own cell.
#[tokio::test]
async fn the_figures_of_a_number_are_read_as_one_word_in_a_bold_face() {
    the_figures_of_a_number_are_one_word(&[("bold, 24 pixels", ttf_noto_sans::BOLD, 24.0)]).await;
}

/// The TOKENS a page carries are read character for character, at every size a document is
/// set at.
///
/// A token is a run of capitals and figures with no word in it, so nothing but the ink names
/// each character: the classes that share a letterform are what it is drawn from — `B` beside
/// `8`, `5` beside `S`, `0` beside `O` and `Q`, `1` beside `I`, `2` beside `Z`, `U` beside
/// `V` — and a reading that takes one for the other is a misread character rather than a
/// different word. The rows span the sizes a page is set at, which is what the recogniser
/// holds its templates at: a size between two held rasters is matched against glyphs whose
/// stroke weight and counters are a size away from the page's own.
///
/// What is asserted is the CHARACTERS, so the line's own spaces are left out of the
/// comparison: where a page breaks a line into words is [`the_figures_of_a_number_are_read_as_one_word`]'s
/// rule, not this one.
#[tokio::test]
async fn a_page_of_tokens_is_read_character_for_character() {
    let router = harness();
    // Each token draws from the classes that share a letterform, so no row is read by
    // elimination from the shapes around it.
    let tokens = ["EZUB9FL5", "7XUBY3LV", "B8S5OQ01", "MWK4GTD2"];
    for token in tokens {
        for size in [18.0f32, 24.0, 28.0, 32.0] {
            let case = format!("{token} at {size} pixels");
            let line = format!("TOKEN {token}");
            let page = outline_page(&[&line], ttf_noto_sans::REGULAR, size, dark_on_light());
            let blocks = read_page(&router, &case, &page, ImageFormat::Png).await;
            let read: String = texts_of(&blocks, "LINE")
                .join("")
                .chars()
                .filter(|character| !character.is_whitespace())
                .collect();
            let drawn: String = line
                .chars()
                .filter(|character| !character.is_whitespace())
                .collect();
            assert_eq!(
                read, drawn,
                "case {case:?}: the characters drawn must be the characters read, got {:?}",
                texts_of(&blocks, "LINE")
            );
        }
    }
}

/// Two characters a proportional face sets as ONE run of inked columns are read as the two
/// characters they are, on a page set at `size` pixels.
///
/// The rule under test: the `Text` a raster operation reports is the characters the page was
/// drawn with, and where one character ends is settled by the ink — not by the column a
/// slice down the page happens to find. A proportional face gives the segmentation two ways
/// that a run of inked columns is not one character, and a page of TOKENS is where they
/// show, since a token is no word and nothing but the ink names each of its characters:
///
/// * a letter whose ink reaches BACK UNDER the letter before it — the hook of a `J`, the
///   tail of a `Q` — so the two boxes overlap in columns although the strokes never touch,
///   and the pair stands no wider than one wide capital;
/// * a wide capital set against a narrow one — a `W` beside a `V` — whose strokes do touch,
///   where the widest letter a face sets is half again as wide as the capitals it stands
///   with, so a `W` must be allowed to be one character.
///
/// A third class is what the ink of ONE character is made of: the characters that are the
/// same ink but for where one of them closes — a `6` against an `8`, an `A` against an `8` —
/// which a page set in any face has to be read apart.
///
/// The rows differ along what the rule generalises over: which character reaches under its
/// neighbour, and which letterform classes the token draws from. The SIZE the page is set at
/// is the axis the three tests below differ along, one size each, because reading a page is
/// the work of reading its every mark: a test that read every token at every size would hold
/// up every build it runs in for as long as all of them take.
async fn tokens_set_as_one_run_of_ink_are_read_as_two(size: f32) {
    let router = harness();
    let tokens = [
        // A `J` after a capital, a figure and a round letter.
        "XSJMK34S",
        "18JK73EU",
        "MOHJHG7D",
        // A `J` beside the `1` it shares a stem with.
        "5A91JT85",
        // A `Q`, whose tail reaches under the letter after it.
        "4QJ7QX2W",
        // The wide capitals against the narrow ones.
        "VWMWXY7K",
        "AVWX8MW2",
        // The characters drawn around two counters against the ones drawn around one and
        // none: an 8 and a B against a 6, a 9, an A and a 0, against a 5 and a 7.
        "68B9A057",
    ];
    for token in tokens {
        let case = format!("{token} at {size} pixels");
        let drawn = format!("SIM2REAL {token}");
        let page = outline_page(&[&drawn], ttf_noto_sans::REGULAR, size, dark_on_light());
        let blocks = read_page(&router, &case, &page, ImageFormat::Png).await;
        let bare =
            |text: &str| -> String { text.chars().filter(|ch| !ch.is_whitespace()).collect() };
        assert_eq!(
            bare(&texts_of(&blocks, "LINE").join("")),
            bare(&drawn),
            "case {case:?}: the characters drawn must be the characters read, got {:?}",
            texts_of(&blocks, "LINE")
        );
        // The token is ONE word: two characters whose boxes overlap are not two words,
        // and neither are two whose strokes touch.
        assert!(
            texts_of(&blocks, "WORD").iter().any(|word| word == token),
            "case {case:?}: the token is one word of the page, got {:?}",
            texts_of(&blocks, "WORD")
        );
    }
}

/// Every token of the table, on a page set SMALL.
#[tokio::test]
async fn two_characters_set_as_one_run_of_ink_are_read_as_two() {
    tokens_set_as_one_run_of_ink_are_read_as_two(18.0).await;
}

/// The same at the middling size a document's own text is set at.
#[tokio::test]
async fn two_characters_set_as_one_run_of_ink_are_read_as_two_at_a_middling_size() {
    tokens_set_as_one_run_of_ink_are_read_as_two(24.0).await;
}

/// The same at a size whose strokes are several pixels wide.
#[tokio::test]
async fn two_characters_set_as_one_run_of_ink_are_read_as_two_at_a_large_size() {
    tokens_set_as_one_run_of_ink_are_read_as_two(32.0).await;
}

/// The analysis path reads the same page the same way: the words a number is read as are
/// the words every read path over that document reports.
#[tokio::test]
async fn the_analysis_path_reads_a_numbers_figures_as_one_word() {
    let router = harness();
    let page = outline_page(LINES, ttf_noto_sans::REGULAR, 32.0, dark_on_light());
    let request = json!({
        "Document": raster_document(&page, ImageFormat::Png),
        "FeatureTypes": ["FORMS"],
    });
    let (status, body) = call(&router, "AnalyzeDocument", &request).await;
    assert_eq!(
        status, 200,
        "case analysis of an outline page: a readable page must be analysed, got {status} \
         with {body}"
    );
    let blocks = blocks_of(&body);
    let words = texts_of(&blocks, "WORD");
    for number in NUMBERS {
        assert!(
            words.iter().any(|word| word == number),
            "case analysis of an outline page: the figures of {number:?} are one word, got \
             {words:?}"
        );
    }
}
