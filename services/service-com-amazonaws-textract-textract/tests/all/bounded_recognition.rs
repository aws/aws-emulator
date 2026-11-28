// SPDX-License-Identifier: CC0-1.0

//! What a page is read as does not depend on HOW MUCH of the comparison was measured.
//!
//! A mark is named by comparing it against a bank of templates and taking the character of
//! the template whose ink it shares most of. The comparison is measured only as far as its
//! answer can still change: every template carries the agreement it has to beat to matter,
//! and one that the cells it has left cannot carry past that is dropped where it stands.
//!
//! The rule under test, over a table of pages rather than one: the `Text` a raster read path
//! reports for a page is the characters that were DRAWN on it, and the SAME characters
//! whichever read path reports them — however well or poorly the page's own ink agrees with
//! any template the bank holds. A comparison cut short where it could still have won would
//! show here as a character named after another template, as a word falling apart, or as two
//! read paths over the same pixels disagreeing.
//!
//! The rows differ along what the rule generalises over: the family the page is set in (a
//! PIXEL face, whose glyphs are a few strokes wide and whose ink a template is a copy of, and
//! a PROPORTIONAL OUTLINE face at sizes the bank holds no template at, where every
//! comparison is of ink no template is a copy of), the size, the polarity of the ink and the
//! format the page is submitted in.
//!
//! The exact assertion of the set is that the two read paths AGREE, which holds whatever the
//! page is set in; what each path reports is then held against the page's own text — character
//! for character where a template is a copy of the ink, and word for word where it is not,
//! which is `crate::typefaces`' own distinction.

use image::{GrayImage, ImageFormat};
use serde_json::json;

use crate::document_extraction::{blocks_of, call, harness, of_type};
use crate::raster_documents::{raster_document, texts_of, Paper};
use crate::typefaces::{dark_on_light, outline_page, pixel_page, Polarity};

/// The lines every page here is drawn from: capitals, small letters, a labelled field and a
/// number, so more than one glyph class is compared, and two lines rather than a page of
/// them, since what the rows span is the comparison and not the page.
const LINES: &[&str] = &["SIMPARITY LEDGER", "Account Number: 48213"];

/// A page of light ink on a dark ground, which Textract reads just as well.
const fn light_on_dark() -> Polarity {
    Polarity {
        background: 12,
        ink: 255,
        ..Paper::plain()
    }
}

/// The characters of `lines`, with the spaces left out: where a page breaks a line into
/// words is `crate::word_breaks`' rule, not this one.
fn characters(lines: &[String]) -> String {
    lines
        .iter()
        .flat_map(|line| line.chars())
        .filter(|character| !character.is_whitespace())
        .collect()
}

/// The LINE and WORD texts a read path reports for `page`.
///
/// `operation` is the read path: the detection path takes the document on its own, the
/// analysis path takes it with the features asked of it, and both report the blocks the
/// page's own pixels were read as.
async fn read_both_ways(
    router: &smithy_server_runtime::MultiServiceRouter,
    case: &str,
    page: &GrayImage,
    format: ImageFormat,
) -> Vec<(&'static str, Vec<String>, Vec<String>)> {
    let document = raster_document(page, format);
    let paths = [
        ("DetectDocumentText", json!({ "Document": document })),
        (
            "AnalyzeDocument",
            json!({ "Document": raster_document(page, format), "FeatureTypes": ["FORMS"] }),
        ),
    ];
    let mut out = Vec::new();
    for (operation, request) in paths {
        let (status, body) = call(router, operation, &request).await;
        assert_eq!(
            status, 200,
            "case {case:?} through {operation}: a page in a format Textract reads must be \
             read, got {status} with {body}"
        );
        let blocks = blocks_of(&body);
        assert!(
            !of_type(&blocks, "PAGE").is_empty(),
            "case {case:?} through {operation}: a page that was read reports a PAGE block, got \
             {body}"
        );
        out.push((
            operation,
            texts_of(&blocks, "LINE"),
            texts_of(&blocks, "WORD"),
        ));
    }
    out
}

/// Both read paths report the characters the page was drawn with, and report the same ones.
///
/// `exactly` says whether the characters must come back one for one: a page set in a face the
/// bank holds a copy of is read character for character, while a page rasterised from an
/// outline at a size no template is held at is ink no template is a copy of, so what is asked
/// of it is the words the page sets and most of their characters.
async fn a_page_reads_the_same_either_way(
    router: &smithy_server_runtime::MultiServiceRouter,
    case: &str,
    page: &GrayImage,
    format: ImageFormat,
    exactly: bool,
) {
    let read = read_both_ways(router, case, page, format).await;
    let drawn: Vec<String> = LINES.iter().map(ToString::to_string).collect();
    let drawn_words: Vec<String> = LINES
        .iter()
        .flat_map(|line| line.split_whitespace())
        .map(ToString::to_string)
        .collect();
    for (operation, lines, words) in &read {
        assert_eq!(
            lines.len(),
            drawn.len(),
            "case {case:?} through {operation}: a page of {} lines of ink is read as that many \
             LINE blocks, got {lines:?}",
            drawn.len()
        );
        assert_eq!(
            words.len(),
            drawn_words.len(),
            "case {case:?} through {operation}: the page sets {} words, got {words:?}",
            drawn_words.len()
        );
        if exactly {
            assert_eq!(
                lines, &drawn,
                "case {case:?} through {operation}: the LINE blocks must report the lines the \
                 page was drawn from"
            );
            assert_eq!(
                words, &drawn_words,
                "case {case:?} through {operation}: the WORD blocks must report the words the \
                 page was drawn from"
            );
        } else {
            // How many of the characters drawn came back as themselves: ink an outline
            // rasteriser laid down is no copy of any template, so what the rule asks of it is
            // the page's own text rather than two faces agreeing exactly.
            let read = characters(lines);
            let drawn = characters(&drawn);
            let right = read
                .chars()
                .zip(drawn.chars())
                .filter(|(read, drawn)| read == drawn)
                .count();
            assert!(
                right * 10 >= drawn.chars().count() * 9,
                "case {case:?} through {operation}: the characters read must be the characters \
                 drawn, got {right} of {} in {lines:?}",
                drawn.chars().count()
            );
        }
    }
    // The two paths read the same pixels, so they report the same characters: this is what a
    // comparison cut short in one place and not another would break.
    let (first, firsts_lines, firsts_words) = &read[0];
    for (operation, lines, words) in read.iter().skip(1) {
        assert_eq!(
            lines, firsts_lines,
            "case {case:?}: {operation} and {first} read the same pixels, so they must report \
             the same LINE texts"
        );
        assert_eq!(
            words, firsts_words,
            "case {case:?}: {operation} and {first} read the same pixels, so they must report \
             the same WORD texts"
        );
    }
}

/// A page set in a PIXEL face — ink a template of the bank is a copy of — reads character for
/// character down both read paths, whichever way its ink is drawn.
#[tokio::test]
async fn a_pixel_page_reads_the_same_characters_down_both_read_paths() {
    let router = harness();
    for (case, font, polarity, format) in [
        (
            "6x10, dark ink on a light page",
            &embedded_graphics::mono_font::ascii::FONT_6X10,
            dark_on_light(),
            ImageFormat::Png,
        ),
        (
            "8x13, light ink on a dark page",
            &embedded_graphics::mono_font::ascii::FONT_8X13,
            light_on_dark(),
            ImageFormat::Png,
        ),
    ] {
        let page = pixel_page(LINES, font, polarity);
        a_page_reads_the_same_either_way(&router, case, &page, format, true).await;
    }
}

/// A page set in a PROPORTIONAL OUTLINE face at a size the bank holds no template at — ink
/// every comparison measures against templates that are a size away from it — reads as the
/// text it was drawn with down both read paths, at either polarity and in either format.
#[tokio::test]
async fn an_outline_page_reads_the_same_characters_down_both_read_paths() {
    let router = harness();
    for (case, size, polarity, format) in [
        ("regular, 19 pixels", 19.0f32, dark_on_light(), ImageFormat::Png),
        (
            "regular, 27 pixels, submitted as a JPEG",
            27.0,
            dark_on_light(),
            ImageFormat::Jpeg,
        ),
    ] {
        let page = outline_page(LINES, ttf_noto_sans::REGULAR, size, polarity);
        a_page_reads_the_same_either_way(&router, case, &page, format, false).await;
    }
}
