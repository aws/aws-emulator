// SPDX-License-Identifier: CC0-1.0

//! Reading the characters a page sets TIGHT: the marks of punctuation a line draws against
//! its figures, and the letters whose own stroke reaches over the character beside them.
//!
//! The rule under test, over a table of pages rather than one recorded page: `Block$Text` is
//! "the word or line of text that's recognized by Amazon Textract", so for ANY ASCII drawn on
//! a page every `LINE` and `WORD` Text spells exactly the characters drawn. Two classes of ink
//! make one run of inked columns out of two characters, and both are the recogniser's to
//! divide:
//!
//! * a mark of PUNCTUATION a face sets tight against the figure or letter beside it — the
//!   solidus of a date, the point of an amount, the colon after a label, the hyphen inside an
//!   identifier — whose ink touches its neighbour's, so no vertical slice of the page separates
//!   the two and a reading that cannot divide them names the joined ink after whatever single
//!   character it resembles;
//! * a letter whose own STROKE runs on over the columns of the character beside it — the bar of
//!   a `T`, the arm of an `L`, the crossbar of a `t` — so wherever the two divide, part of one
//!   character's stroke stands in the other's columns, and a piece named over its own columns
//!   alone is named after what the rest of it is shaped like.
//!
//! The rows differ along what the rule generalises over: which mark of punctuation the line
//! sets and where in a token it stands, which letter's stroke overhangs its neighbour, the
//! size the page is set at, the weight of the face, and the operation the page is read through
//! (`DetectDocumentText` and `AnalyzeDocument`, which share the one recogniser). Every page is
//! rendered here from known text, so each assertion is a round trip through real pixels.

use image::ImageFormat;
use serde_json::json;

use crate::document_extraction::{blocks_of, call, harness};
use crate::raster_documents::{raster_document, texts_of};
use crate::typefaces::{dark_on_light, outline_page};

/// The characters of `text`, with the spaces left out: where a line is broken into words is
/// `crate::word_breaks`' rule, not this one's.
fn characters(text: &str) -> String {
    text.chars().filter(|ch| !ch.is_whitespace()).collect()
}

/// One row of the table: the lines the page is set from, and the face and size it is set at.
struct Page {
    case: &'static str,
    lines: Vec<&'static str>,
    face: &'static [u8],
    size: f32,
}

/// The pages of the table, each drawing punctuation set tight against figures and letters
/// whose strokes overhang their neighbours.
fn pages() -> Vec<Page> {
    vec![
        Page {
            case: "a date whose solidus stands between two figures",
            lines: vec!["DATE 01/02/2023"],
            face: ttf_noto_sans::REGULAR,
            size: 23.0,
        },
        Page {
            case: "the same date set larger",
            lines: vec!["DATE 01/02/2023"],
            face: ttf_noto_sans::REGULAR,
            size: 31.0,
        },
        Page {
            case: "an amount and a label, a point and a colon inside the line",
            lines: vec!["TOTAL $12.34", "Invoice Number: INV-99887"],
            face: ttf_noto_sans::REGULAR,
            size: 24.0,
        },
        Page {
            case: "the letters whose bars overhang, in capitals",
            lines: vec!["RECEIPT TOTAL DATE", "ATLAS LATTE"],
            face: ttf_noto_sans::REGULAR,
            size: 24.0,
        },
        Page {
            case: "the same, set in the bold cut",
            lines: vec!["RECEIPT TOTAL DATE"],
            face: ttf_noto_sans::BOLD,
            size: 26.0,
        },
        Page {
            case: "a labelled field and a date, in mixed case",
            lines: vec!["Name: John Smith", "Date: 01/02/2024"],
            face: ttf_noto_sans::REGULAR,
            size: 27.0,
        },
    ]
}

/// RULE: the characters read are the characters drawn, for the punctuation a page sets tight
/// against its figures and for the letters whose strokes overhang their neighbours — read
/// through `DetectDocumentText`.
#[tokio::test]
async fn a_page_set_tight_is_read_as_the_characters_drawn() {
    let router = harness();
    for page in pages() {
        let drawn = outline_page(&page.lines, page.face, page.size, dark_on_light());
        let (status, body) = call(
            &router,
            "DetectDocumentText",
            &json!({ "Document": raster_document(&drawn, ImageFormat::Png) }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {:?}: a page in a format Textract reads must be read, got {status} with \
             {body}",
            page.case
        );
        let read = texts_of(&blocks_of(&body), "LINE");
        assert_eq!(
            read.len(),
            page.lines.len(),
            "case {:?}: a page of {} lines of ink is read as that many LINE blocks, got \
             {read:?}",
            page.case,
            page.lines.len()
        );
        for (line, (read, drawn)) in read.iter().zip(page.lines.iter()).enumerate() {
            assert_eq!(
                characters(read),
                characters(drawn),
                "case {:?}: line {line} was drawn {drawn:?}, got {read:?}",
                page.case
            );
        }
    }
}

/// RULE: the same characters come back through `AnalyzeDocument`, which reads the page with
/// the same recogniser — so the marks of punctuation and the overhanging strokes of a page
/// read for FORMS spell what was drawn, in the WORD blocks the feature blocks are built from.
#[tokio::test]
async fn the_words_of_a_page_read_for_forms_spell_what_was_drawn() {
    let router = harness();
    let lines = vec!["Name: John Jones", "Date: 01/02/2024", "TOTAL $12.34"];
    let drawn = outline_page(&lines, ttf_noto_sans::REGULAR, 27.0, dark_on_light());
    let (status, body) = call(
        &router,
        "AnalyzeDocument",
        &json!({
            "Document": raster_document(&drawn, ImageFormat::Png),
            "FeatureTypes": ["FORMS"],
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case a form-like page: AnalyzeDocument must read it, got {status} with {body}"
    );
    let blocks = blocks_of(&body);
    let words = texts_of(&blocks, "WORD");
    // Every word drawn, in the order the page draws them: the WORD blocks of the page are
    // the words of its lines, each spelling its own drawn characters.
    let expected: Vec<String> = lines
        .iter()
        .flat_map(|line| line.split_whitespace())
        .map(ToString::to_string)
        .collect();
    assert_eq!(
        words, expected,
        "case a form-like page: the WORD blocks must spell the words drawn, got {words:?}"
    );
}
