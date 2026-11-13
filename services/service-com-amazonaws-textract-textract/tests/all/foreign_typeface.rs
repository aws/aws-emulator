// SPDX-License-Identifier: CC0-1.0

//! Reading a document set in a typeface the recogniser holds no template for, end to end.
//!
//! The rule under test, over a table of inputs rather than one recorded page: the text a
//! raster operation reports is the text the pixels were DRAWN with, whatever typeface drew
//! them, and every feature derived from that text follows from the page's own layout —
//! `FORMS` pairs a label with the value beside it whether the label is punctuated with a
//! colon or set in the next column, `AnalyzeExpense` reports a summary field for a receipt
//! line that ends in an amount, and a `QUERIES` query is answered with the value the
//! document holds against the label the query asks about.
//!
//! The other raster suites draw their pages with the same public-domain 8x8 font the
//! recogniser holds templates for, which makes that round trip partly self-referential.
//! Every page here is drawn at a raster the recogniser holds NO template at: the bank takes
//! the pre-rasterised sans-serif face at its 16- and 32-pixel rasters, and these pages are
//! drawn at its 20- and 24-pixel ones, whose glyphs are a different bitmap on a different
//! pixel grid — anti-aliased ink no template is a copy of. So what these rows pin is
//! recognition rather than identity. The rows differ along what the rule generalises over:
//! the weight and raster the text is drawn at, the scale it is rendered at, how the label of
//! a field is separated from its value, and which operation reads the page.

use std::io::Cursor;

use image::{GrayImage, ImageFormat, Luma};
use noto_sans_mono_bitmap::{get_raster, FontWeight, RasterHeight};
use serde_json::{json, Value};

use crate::document_extraction::{
    block_id, block_type, blocks_of, call, harness, of_type,
};
use crate::raster_documents::texts_of;

/// How a page is drawn: the face's weight and raster, and the pixels per font pixel.
///
/// Every raster named here is one the recogniser holds NO template at (see the module
/// header): its templates come from this face's 16- and 32-pixel rasters, so a page drawn at
/// the 20- or 24-pixel one is ink no template is a copy of.
#[derive(Debug, Clone, Copy)]
struct Face {
    weight: FontWeight,
    height: RasterHeight,
    scale: usize,
}

impl Face {
    const fn plain() -> Self {
        Self {
            weight: FontWeight::Regular,
            height: RasterHeight::Size24,
            scale: 1,
        }
    }
}

/// Draw `lines` with the pre-rasterised face, `gap` blank glyph cells wide wherever the
/// text holds a tab, as a grey page.
///
/// `noto_sans_mono_bitmap` hands out one byte of ink per pixel per glyph, so the strokes
/// are anti-aliased: the edge of every one of them lies part way between the ink and the
/// ground, which is what a page rendered at any ordinary size carries.
fn draw(lines: &[&str], face: Face) -> GrayImage {
    let cell = face.height.val();
    let pitch = cell * 3 / 2;
    let margin = 10usize;
    let advance = |ch: char| match ch {
        // A tab is the wide gap a page sets its second COLUMN at.
        '\t' => cell * 3,
        other => get_raster(other, face.weight, face.height).map_or(cell / 2, |it| it.width()),
    };
    let columns = lines
        .iter()
        .map(|line| line.chars().map(advance).sum::<usize>())
        .max()
        .unwrap_or(0);
    let width = ((margin * 2 + columns * face.scale) as u32).max(1);
    let depth = ((margin * 2 + lines.len() * pitch * face.scale) as u32).max(1);
    let mut page = GrayImage::from_pixel(width, depth, Luma([255]));
    for (row, line) in lines.iter().enumerate() {
        let mut at = margin;
        for ch in line.chars() {
            if let Some(raster) = get_raster(ch, face.weight, face.height) {
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
                        for dy in 0..face.scale {
                            for dx in 0..face.scale {
                                let px = ((at + x) * face.scale + dx) as u32;
                                let py = ((margin + row * pitch + y) * face.scale + dy) as u32;
                                if px < width && py < depth {
                                    page.put_pixel(px, py, Luma([255 - ink]));
                                }
                            }
                        }
                    }
                }
            }
            at += advance(ch);
        }
    }
    page
}

fn document(lines: &[&str], face: Face) -> Value {
    let page = draw(lines, face);
    let mut bytes = Cursor::new(Vec::new());
    page.write_to(&mut bytes, ImageFormat::Png)
        .expect("a grey page encodes as a PNG");
    json!({ "Bytes": aws_smithy_types::base64::encode(bytes.into_inner()) })
}

/// The characters of `text`, without the spacing: what separates two words is measured off
/// the line's own gaps, so a character count is taken over the characters themselves.
fn bare(text: &str) -> String {
    text.chars().filter(|ch| !ch.is_whitespace()).collect()
}

/// How many of the characters drawn came back, as a share of them.
fn read_share(drawn: &[&str], read: &[String]) -> f32 {
    let drawn = bare(&drawn.join(""));
    let read = bare(&read.join(""));
    let right = drawn
        .chars()
        .zip(read.chars())
        .filter(|(drawn, read)| drawn == read)
        .count();
    right as f32 / drawn.chars().count().max(1) as f32
}

/// Every character of the lines a page was drawn from comes back from its pixels, at every
/// weight, size and scale the same lines are drawn at.
#[tokio::test]
async fn detect_document_text_reads_the_face_it_was_drawn_in() {
    let router = harness();
    let lines = [
        "SIM2REAL LEDGER",
        "Invoice Number: INV-99887",
        "Total Due $12.34",
    ];
    let cases: Vec<(&str, Face)> = vec![
        ("regular strokes", Face::plain()),
        (
            "light strokes",
            Face {
                weight: FontWeight::Light,
                ..Face::plain()
            },
        ),
        (
            "bold strokes",
            Face {
                weight: FontWeight::Bold,
                ..Face::plain()
            },
        ),
        (
            "a smaller raster",
            Face {
                height: RasterHeight::Size20,
                ..Face::plain()
            },
        ),
        (
            "drawn at twice the size",
            Face {
                scale: 2,
                ..Face::plain()
            },
        ),
    ];
    for (case, face) in cases {
        let (status, body) = call(
            &router,
            "DetectDocumentText",
            &json!({ "Document": document(&lines, face) }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: a readable raster is read whatever face it is set in, got \
             {status} with {body}"
        );
        let blocks = blocks_of(&body);
        let read = texts_of(&blocks, "LINE");
        assert_eq!(
            read.len(),
            lines.len(),
            "case {case:?}: a page of {} lines of ink is read as that many lines, got \
             {read:?}",
            lines.len()
        );
        let share = read_share(&lines, &read);
        assert!(
            share >= 0.8,
            "case {case:?}: the characters reported must be the characters drawn, got \
             {read:?} against {lines:?} ({share:.2} of them)"
        );
        // Every word of every line is a WORD block of its own, so the text is reported as
        // words rather than as one run of characters.
        let words = texts_of(&blocks, "WORD");
        assert!(
            words.len() >= lines.iter().map(|line| line.split(' ').count()).sum::<usize>() - 1,
            "case {case:?}: every word drawn is a WORD block, got {words:?}"
        );
    }
}

/// A labelled line is reported as a KEY and a VALUE whichever way the page separates the
/// label from the value: punctuated with a colon, or set in the next column.
#[tokio::test]
async fn forms_pairs_a_label_with_the_value_the_page_sets_beside_it() {
    let router = harness();
    let cases: Vec<(&str, Vec<&str>, &str, &str)> = vec![
        (
            "a colon between the label and the value",
            vec!["NAME: JOHN SMITH"],
            "NAME",
            "JOHN SMITH",
        ),
        (
            "the label in one column and the value in the next",
            vec!["NAME\tJOHN SMITH"],
            "NAME",
            "JOHN SMITH",
        ),
        (
            "a colon, with the value in the next column",
            vec!["INVOICE NUMBER:\tINV-99887"],
            "INVOICE NUMBER",
            "INV-99887",
        ),
    ];
    for (case, lines, label, value) in cases {
        let (status, body) = call(
            &router,
            "AnalyzeDocument",
            &json!({
                "Document": document(&lines, Face::plain()),
                "FeatureTypes": ["FORMS"],
            }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: the page must be analysed, got {status} with {body}"
        );
        let blocks = blocks_of(&body);
        let pairs = of_type(&blocks, "KEY_VALUE_SET");
        let entity = |block: &Value, kind: &str| {
            block["EntityTypes"]
                .as_array()
                .is_some_and(|types| types.iter().any(|it| it.as_str() == Some(kind)))
        };
        let keys: Vec<&&Value> = pairs.iter().filter(|block| entity(block, "KEY")).collect();
        assert_eq!(
            keys.len(),
            1,
            "case {case:?}: the page holds one labelled line, so it holds one KEY, got {:?}",
            texts_of(&blocks, "KEY_VALUE_SET")
        );
        // The KEY's own words are the label's, and its VALUE relationship resolves to the
        // block whose words are the value's.
        let words: Vec<&Value> = of_type(&blocks, "WORD").into_iter().collect();
        let text_of = |ids: &[&str]| -> String {
            words
                .iter()
                .filter(|word| ids.contains(&block_id(word)))
                .map(|word| word["Text"].as_str().unwrap_or_default())
                .collect::<Vec<&str>>()
                .join(" ")
        };
        let related = |block: &Value, kind: &str| -> Vec<String> {
            block["Relationships"]
                .as_array()
                .map(|all| {
                    all.iter()
                        .filter(|it| it["Type"].as_str() == Some(kind))
                        .filter_map(|it| it["Ids"].as_array())
                        .flatten()
                        .filter_map(|id| id.as_str().map(ToString::to_string))
                        .collect()
                })
                .unwrap_or_default()
        };
        let key = keys[0];
        let children = related(key, "CHILD");
        let key_text = text_of(&children.iter().map(String::as_str).collect::<Vec<&str>>());
        assert!(
            read_share(&[label], std::slice::from_ref(&key_text)) >= 0.75,
            "case {case:?}: the KEY's own words are the label's, got {key_text:?} against \
             {label:?}"
        );
        let answered = related(key, "VALUE");
        let valued = pairs
            .iter()
            .find(|block| answered.contains(&block_id(block).to_string()))
            .unwrap_or_else(|| {
                panic!("case {case:?}: the KEY's VALUE relationship must resolve to a block")
            });
        assert!(
            entity(valued, "VALUE"),
            "case {case:?}: the block a KEY's VALUE relationship names is the VALUE, got \
             {valued}"
        );
        let value_children = related(valued, "CHILD");
        let value_text = text_of(
            &value_children
                .iter()
                .map(String::as_str)
                .collect::<Vec<&str>>(),
        );
        assert!(
            read_share(&[value], std::slice::from_ref(&value_text)) >= 0.75,
            "case {case:?}: the VALUE's own words are the value's, got {value_text:?} \
             against {value:?}"
        );
        assert!(
            of_type(&blocks, "PAGE")
                .first()
                .is_some_and(|page| block_type(page) == "PAGE"),
            "case {case:?}: the answer carries the page it was read from"
        );
    }
}

/// A receipt line that ends in an AMOUNT is a summary field, with no colon on the page.
#[tokio::test]
async fn analyze_expense_reports_a_summary_field_for_a_line_that_ends_in_an_amount() {
    let router = harness();
    let cases: Vec<(&str, Vec<&str>, &str, &str)> = vec![
        (
            "a total written with a currency sign",
            vec!["RECEIPT", "TOTAL $12.34"],
            "TOTAL",
            "12.34",
        ),
        (
            "a total written without one",
            vec!["RECEIPT", "TOTAL 12.34"],
            "TOTAL",
            "12.34",
        ),
        (
            "a label of several words",
            vec!["RECEIPT", "AMOUNT DUE 42.50"],
            "AMOUNT DUE",
            "42.50",
        ),
    ];
    for (case, lines, label, amount) in cases {
        let (status, body) = call(
            &router,
            "AnalyzeExpense",
            &json!({ "Document": document(&lines, Face::plain()) }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: the receipt must be analysed, got {status} with {body}"
        );
        let documents = body["ExpenseDocuments"]
            .as_array()
            .cloned()
            .unwrap_or_default();
        assert_eq!(
            documents.len(),
            1,
            "case {case:?}: one page is one expense document, got {body}"
        );
        let fields = documents[0]["SummaryFields"]
            .as_array()
            .cloned()
            .unwrap_or_default();
        assert!(
            !fields.is_empty(),
            "case {case:?}: a line that ends in an amount is a summary field, got {}",
            documents[0]
        );
        let detected = |field: &Value, which: &str| -> String {
            field[which]["Text"].as_str().unwrap_or_default().to_string()
        };
        let found = fields
            .iter()
            .map(|field| {
                (
                    detected(field, "LabelDetection"),
                    detected(field, "ValueDetection"),
                )
            })
            .collect::<Vec<(String, String)>>();
        assert!(
            found.iter().any(|(read_label, read_value)| {
                read_share(&[label], std::slice::from_ref(read_label)) >= 0.6
                    && read_value.contains(&amount.chars().next().unwrap_or('0').to_string())
            }),
            "case {case:?}: the label and the amount the line holds must be the field's, \
             got {found:?} against {label:?} and {amount:?}"
        );
    }
}

/// A query is answered with the value the page holds against the label it asks about, and
/// the QUERY block is linked to that answer by an ANSWER relationship.
#[tokio::test]
async fn a_query_is_answered_from_the_label_the_page_holds() {
    let router = harness();
    let cases: Vec<(&str, Vec<&str>, &str)> = vec![
        (
            "a colon-punctuated label",
            vec!["INVOICE NUMBER: INV-99887"],
            "What is the invoice number?",
        ),
        (
            "a label in its own column",
            vec!["INVOICE NUMBER\tINV-99887"],
            "What is the invoice number?",
        ),
    ];
    for (case, lines, question) in cases {
        let (status, body) = call(
            &router,
            "AnalyzeDocument",
            &json!({
                "Document": document(&lines, Face::plain()),
                "FeatureTypes": ["QUERIES"],
                "QueriesConfig": { "Queries": [{ "Text": question, "Alias": "INVOICE_NUMBER" }] },
            }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: the page must be analysed, got {status} with {body}"
        );
        let blocks = blocks_of(&body);
        let queries = of_type(&blocks, "QUERY");
        assert_eq!(
            queries.len(),
            1,
            "case {case:?}: one query asked is one QUERY block, got {blocks:?}"
        );
        let answers: Vec<String> = queries[0]["Relationships"]
            .as_array()
            .map(|all| {
                all.iter()
                    .filter(|it| it["Type"].as_str() == Some("ANSWER"))
                    .filter_map(|it| it["Ids"].as_array())
                    .flatten()
                    .filter_map(|id| id.as_str().map(ToString::to_string))
                    .collect()
            })
            .unwrap_or_default();
        assert_eq!(
            answers.len(),
            1,
            "case {case:?}: a query the page answers carries an ANSWER relationship, got {:?}",
            queries[0]
        );
        let result = of_type(&blocks, "QUERY_RESULT")
            .into_iter()
            .find(|block| answers.contains(&block_id(block).to_string()))
            .unwrap_or_else(|| {
                panic!("case {case:?}: the ANSWER must name a QUERY_RESULT block")
            });
        let answer = result["Text"].as_str().unwrap_or_default().to_string();
        assert!(
            read_share(&["INV-99887"], std::slice::from_ref(&answer)) >= 0.6,
            "case {case:?}: the answer is the value the page holds against that label, got \
             {answer:?}"
        );
        assert!(
            result["Confidence"].as_f64().is_some_and(|it| it > 0.0),
            "case {case:?}: the answer reports the confidence it was read with, got {result}"
        );
    }
}
