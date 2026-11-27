// SPDX-License-Identifier: CC0-1.0

//! At which TURN a page set in a pixel face is read, end to end.
//!
//! The rule under test, over a table of pages rather than one recorded document: the turn a
//! raster operation reports for a page's words (`Geometry$RotationAngle`, which the `Block`
//! docs state is "a numerical value corresponding to the rotation of the WORD block.
//! Possible values are 0, 90, 180, and 270") is the turn the page's characters were DRAWN
//! at, and the text it reports is the text they were drawn with — so the reading taken of
//! the page as it was submitted stands unless one of the page's turns reads it clearly
//! better, and a reading that already accounts for as much of the page's ink as any reading
//! can is not displaced by any of them.
//!
//! The rows differ along what the rule generalises over: the quarter turn the page was
//! submitted at, and the raster the pixel face sets its characters at. Every page is drawn
//! here from known text and then turned, so what is asserted is a round trip through real
//! pixels: a page drawn upright must come back upright with its own text, and the same
//! pixels turned must come back reporting the turn they were submitted at — with the same
//! text, since the characters are the same characters whichever way the sheet was fed.

use embedded_graphics::mono_font::{ascii, MonoFont};
use image::ImageFormat;
use serde_json::{json, Value};

use crate::document_extraction::{assert_block_graph, blocks_of, call, harness, of_type};
use crate::page_block_extent::turned;
use crate::raster_documents::{raster_document, texts_of};
use crate::typefaces::{dark_on_light, pixel_page};

/// The lines every page here is drawn from: a heading of capitals and a labelled figure, so
/// a line of the page carries more than one glyph class and the page carries more than one
/// line.
const LINES: &[&str] = &["SIM2REAL LEDGER", "Account 48213"];

/// Read each of `cases` — a page set in a pixel face, submitted after a number of quarter
/// turns — and assert the turn and the text it comes back with.
async fn pages_are_read_at_the_turn_they_were_drawn_at(cases: &[(&str, &MonoFont<'static>, u8)]) {
    let router = harness();
    for (case, font, turns) in cases {
        let page = turned(&pixel_page(LINES, font, dark_on_light()), *turns);
        let request = json!({ "Document": raster_document(&page, ImageFormat::Png) });
        let (status, body) = call(&router, "DetectDocumentText", &request).await;
        assert_eq!(
            status, 200,
            "case {case:?}: a page in a format Textract reads must be read, got {status} \
             with {body}"
        );
        let blocks = blocks_of(&body);
        assert_block_graph(case, &body, &blocks, None);
        let words = of_type(&blocks, "WORD");
        assert!(
            !words.is_empty(),
            "case {case:?}: a page of ink must be read as the words drawn on it, got {body}"
        );
        let expected = f64::from(u16::from(turns % 4)) * 90.0;
        for word in &words {
            let reported = word
                .get("Geometry")
                .and_then(|geometry| geometry.get("RotationAngle"))
                .and_then(Value::as_f64);
            assert_eq!(
                reported,
                Some(expected),
                "case {case:?}: the turn a page's characters are drawn at is the \
                 RotationAngle of every WORD, got {reported:?} on {:?}",
                word.get("Text")
            );
        }
        assert_eq!(
            texts_of(&blocks, "LINE"),
            LINES.iter().map(ToString::to_string).collect::<Vec<_>>(),
            "case {case:?}: the LINE blocks must report the lines the page was drawn from"
        );
    }
}

/// A page drawn UPRIGHT is read upright, at either raster: the reading taken of the page as
/// it was submitted is the page's own, and the turns of it do not displace it.
#[tokio::test]
async fn a_page_set_in_a_pixel_face_drawn_upright_is_read_upright() {
    pages_are_read_at_the_turn_they_were_drawn_at(&[
        ("9x18, upright", &ascii::FONT_9X18, 0),
        ("10x20, upright", &ascii::FONT_10X20, 0),
    ])
    .await;
}

/// The same pixels fed in UPSIDE DOWN are read at the half turn they were submitted at: the
/// reading taken along the page's own characters is still the one that describes it, and it
/// is found by reading the page at that turn.
#[tokio::test]
async fn a_page_set_in_a_pixel_face_submitted_upside_down_is_read_at_a_half_turn() {
    pages_are_read_at_the_turn_they_were_drawn_at(&[
        ("9x18, submitted upside down", &ascii::FONT_9X18, 2),
        ("10x20, submitted upside down", &ascii::FONT_10X20, 2),
    ])
    .await;
}

/// The same fed in SIDEWAYS, each way round, where reading the page as submitted reads
/// across its lines of text instead of along them.
#[tokio::test]
async fn a_page_set_in_a_pixel_face_submitted_sideways_is_read_at_a_quarter_turn() {
    pages_are_read_at_the_turn_they_were_drawn_at(&[
        ("9x18, submitted a quarter turn clockwise", &ascii::FONT_9X18, 1),
        (
            "9x18, submitted three quarters of a turn clockwise",
            &ascii::FONT_9X18,
            3,
        ),
    ])
    .await;
}
