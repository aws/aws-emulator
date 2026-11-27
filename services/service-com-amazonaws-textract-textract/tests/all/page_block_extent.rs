// SPDX-License-Identifier: CC0-1.0

//! End-to-end coverage of WHAT a `PAGE` block's `Geometry` describes: the content detected
//! on the submitted page, measured off its own pixels.
//!
//! `BoundingBox` is "the bounding box around the DETECTED page, text, key-value pair,
//! table, table cell, or selection element on a document page", with Left/Top "as a ratio
//! of overall document page width/height", and `Polygon` is "within the bounding box, a
//! fine-grained polygon around the recognized item". So a `PAGE` block reports where the
//! page was detected on what was submitted — the extent of the ink found on it, normalised
//! by the submitted page's own width and height, outlined by the extreme marks of that ink
//! — and not the submitted canvas by construction: a document photographed or scanned
//! inside a larger frame occupies part of that frame. A page nothing at all was detected
//! on is the one page reported as the whole of what was submitted, because that is the
//! whole of what was examined for it.
//!
//! The rows of the table differ along what the rule generalises over: how much of the page
//! the ink covers (a word in a corner of a mostly blank page, lines filling the page, a
//! page with no ink at all), WHERE on the page it sits, the page's proportions, the
//! polarity of the ink, whether the page carries ink that is no character at all (a rule),
//! the quarter turn the page is submitted at, the read path it is read through
//! (`DetectDocumentText` and `AnalyzeDocument` with each feature set), and whether the
//! page's text carries measured geometry at all.
//!
//! Every page here is drawn in this test, and the extent the assertions compare against is
//! measured by this test off the very pixels it submitted — never taken from a recorded
//! answer.

use image::{GrayImage, ImageFormat, Luma};
use serde_json::{json, Value};
use service_com_amazonaws_textract_textract::state;

use crate::block_geometry::{bbox_of, polygon_of, Rect};
use crate::block_outlines::assert_inscribed;
use crate::document_extraction::{
    assert_block_graph, block_id, block_type, blocks_of, call, harness, of_type,
};
use crate::raster_documents::{raster_document, render, Paper};

/// One row of the table: the page that is submitted, and how it is read.
struct Case {
    name: &'static str,
    lines: Vec<&'static str>,
    paper: Paper,
    /// The size of the page the drawn ink is placed on, in pixels, when it is to be
    /// bigger than the ink: `None` leaves the drawn page as it is.
    page: Option<(u32, u32)>,
    /// Where the drawn ink is placed on that page, as a fraction of the room left over,
    /// so `(0.0, 0.0)` is its top-left corner and `(1.0, 1.0)` its bottom-right one.
    at: (f32, f32),
    /// Clockwise quarter turns applied to the page before it is submitted.
    turns: u8,
    operation: &'static str,
    /// The `FeatureTypes` of an `AnalyzeDocument` row; empty for text detection.
    features: Vec<&'static str>,
}

impl Case {
    /// A row read through `DetectDocumentText`, which is the plain text-detection path.
    fn detected(name: &'static str, lines: Vec<&'static str>) -> Self {
        Self {
            name,
            lines,
            paper: Paper::plain(),
            page: None,
            at: (0.0, 0.0),
            turns: 0,
            operation: "DetectDocumentText",
            features: Vec::new(),
        }
    }
}

/// The lines a row that needs no text of its own is drawn from: a heading, a labelled
/// field and a total, so the analysis rows have a key/value pair to derive.
fn sample_lines() -> Vec<&'static str> {
    vec!["SIMPARITY LEDGER", "Account Number: 48213", "Total Due 1250.00"]
}

fn cases() -> Vec<Case> {
    vec![
        // The crux of the rule: one word on a page many times its size. The ink covers a
        // few per cent of the page, and the PAGE block reports those few per cent.
        Case {
            page: Some((320, 240)),
            ..Case::detected("one word in the top-left corner of a large page", vec!["LEDGER"])
        },
        // The same ink, moved. The PAGE block moves with it, which is what a measurement
        // does and a placeholder cannot.
        Case {
            page: Some((320, 240)),
            at: (1.0, 1.0),
            ..Case::detected("the same word in the bottom-right corner", vec!["LEDGER"])
        },
        Case {
            page: Some((320, 240)),
            at: (0.5, 0.5),
            ..Case::detected("the same word in the middle of the page", vec!["LEDGER"])
        },
        // The other extreme: ink over nearly the whole page, which still stops at the
        // page's own margins.
        Case::detected("lines filling the page", sample_lines()),
        // A page with no ink at all: one PAGE block, and nothing was detected on it to
        // measure, so it reports the whole of what was submitted.
        Case {
            page: Some((160, 120)),
            ..Case::detected("a page carrying no ink at all", Vec::new())
        },
        // Proportions: the box's members are ratios of the page's own width and height,
        // so the same ink on a wide page and on a tall one is measured against each.
        Case {
            page: Some((480, 120)),
            at: (0.5, 0.5),
            ..Case::detected("a page far wider than it is tall", vec!["LEDGER"])
        },
        Case {
            page: Some((160, 640)),
            at: (0.5, 0.5),
            ..Case::detected("a page far taller than it is wide", vec!["LEDGER"])
        },
        // Light text on a dark ground: the ink is the lighter class, and it is the ink
        // that is measured.
        Case {
            paper: Paper {
                background: 20,
                ink: 240,
                ..Paper::plain()
            },
            page: Some((400, 300)),
            at: (0.25, 0.75),
            ..Case::detected("light ink on a dark ground", sample_lines())
        },
        // A rule drawn across the page is page structure, no line of text — and it is
        // content of the page, so the detected extent reaches as far as the rule does.
        Case {
            paper: Paper {
                rule: true,
                ..Paper::plain()
            },
            ..Case::detected("a page carrying a rule", sample_lines())
        },
        // The quarter turns: the extent is measured in the frame the page was SUBMITTED
        // in, and the polygon starts at the corner the characters were read from.
        Case {
            turns: 1,
            page: Some((400, 300)),
            at: (0.2, 0.8),
            ..Case::detected("a page submitted a quarter turn clockwise", sample_lines())
        },
        Case {
            turns: 2,
            ..Case::detected("a page submitted a half turn", sample_lines())
        },
        Case {
            turns: 3,
            page: Some((400, 300)),
            ..Case::detected("a page submitted three quarters of a turn", sample_lines())
        },
        // The analysis path, which assembles its own response around the same blocks, with
        // each feature set it derives blocks for.
        Case {
            operation: "AnalyzeDocument",
            features: vec!["FORMS"],
            page: Some((400, 300)),
            at: (0.9, 0.1),
            ..Case::detected("the analysis path over a labelled page", sample_lines())
        },
        Case {
            operation: "AnalyzeDocument",
            features: vec!["TABLES"],
            ..Case::detected(
                "the analysis path over a page carrying a table",
                vec!["Item      Price", "Pen       2.50", "Pad       11.75"],
            )
        },
        Case {
            operation: "AnalyzeDocument",
            features: vec!["FORMS", "TABLES"],
            page: Some((420, 320)),
            at: (0.1, 0.9),
            ..Case::detected("the analysis path over both feature sets", sample_lines())
        },
    ]
}

/// `drawn` placed on a `width` x `height` page whose ground is `background`, at `at` of
/// the room left over — so a page much larger than its ink can be drawn with that ink
/// anywhere on it.
fn on_page(drawn: &GrayImage, (width, height): (u32, u32), background: u8, at: (f32, f32)) -> GrayImage {
    let mut page = GrayImage::from_pixel(width.max(drawn.width()), height.max(drawn.height()), Luma([background]));
    let room = |page: u32, ink: u32, at: f32| -> u32 {
        let left = page.saturating_sub(ink);
        (left as f32 * at.clamp(0.0, 1.0)).round() as u32
    };
    let (dx, dy) = (
        room(page.width(), drawn.width(), at.0),
        room(page.height(), drawn.height(), at.1),
    );
    for y in 0..drawn.height() {
        for x in 0..drawn.width() {
            page.put_pixel(x + dx, y + dy, *drawn.get_pixel(x, y));
        }
    }
    page
}

/// `page` after `turns` clockwise quarter turns, which is how the same sheet reaches the
/// service when it was fed in turned.
///
/// `pub(crate)`: `crate::page_turns_of_a_pixel_face` submits its pages at each of their
/// quarter turns through this same helper rather than growing one of its own.
pub(crate) fn turned(page: &GrayImage, turns: u8) -> GrayImage {
    match turns % 4 {
        1 => image::imageops::rotate90(page),
        2 => image::imageops::rotate180(page),
        3 => image::imageops::rotate270(page),
        _ => page.clone(),
    }
}

/// The page one row submits.
fn page_of(case: &Case) -> GrayImage {
    let drawn = if case.lines.is_empty() {
        GrayImage::from_pixel(1, 1, Luma([case.paper.background]))
    } else {
        render(&case.lines, case.paper)
    };
    let placed = match case.page {
        Some(size) => on_page(&drawn, size, case.paper.background, case.at),
        None => drawn,
    };
    turned(&placed, case.turns)
}

/// Whether the pixel `(x, y)` of a page whose ground is `background` carries ink. Every
/// page here is drawn in two levels, so a pixel is either the ground or a stroke.
fn is_ink(page: &GrayImage, background: u8, x: u32, y: u32) -> bool {
    page.get_pixel(x, y).0[0] != background
}

/// The four corners of the whole page, clockwise from its top-left one, as the
/// page-relative points `Polygon` is expressed in.
const PAGE_CORNERS: [(f64, f64); 4] = [(0.0, 0.0), (1.0, 0.0), (1.0, 1.0), (0.0, 1.0)];

/// The whole page, as the box reported for a page nothing was detected on.
const WHOLE_PAGE: Rect = Rect {
    left: 0.0,
    top: 0.0,
    width: 1.0,
    height: 1.0,
};

/// This test's OWN measurement of the content it drew: the extent of the ink of the page
/// it submitted, as the page-relative fractions `BoundingBox` is expressed in, or `None`
/// for a page it drew no ink on.
fn drawn_extent(page: &GrayImage, background: u8) -> Option<Rect> {
    let mut bounds: Option<(u32, u32, u32, u32)> = None;
    for y in 0..page.height() {
        for x in 0..page.width() {
            if !is_ink(page, background, x, y) {
                continue;
            }
            bounds = Some(match bounds {
                None => (x, x, y, y),
                Some((x0, x1, y0, y1)) => (x0.min(x), x1.max(x), y0.min(y), y1.max(y)),
            });
        }
    }
    let (x0, x1, y0, y1) = bounds?;
    let (width, height) = (f64::from(page.width()), f64::from(page.height()));
    Some(Rect {
        left: f64::from(x0) / width,
        top: f64::from(y0) / height,
        width: f64::from(x1 + 1 - x0) / width,
        height: f64::from(y1 + 1 - y0) / height,
    })
}

/// How far a reported edge may sit from the drawn one: one pixel of the submitted page,
/// since the recogniser reads the ink through a floor of its own and an edge pixel of a
/// stroke can fall either side of it.
fn tolerance(page: &GrayImage) -> f64 {
    1.0 / f64::from(page.width().min(page.height()).max(1))
}

fn assert_same_extent(case: &str, what: &str, reported: &Rect, expected: &Rect, tol: f64) {
    assert!(
        (reported.left - expected.left).abs() <= tol
            && (reported.top - expected.top).abs() <= tol
            && (reported.width - expected.width).abs() <= 2.0 * tol
            && (reported.height - expected.height).abs() <= 2.0 * tol,
        "case {case:?}: the PAGE block's BoundingBox must be {what} {expected:?}, got \
         {reported:?}"
    );
}

/// The `Geometry$RotationAngle` of the first `WORD` of an answer, or `0.0` for a page that
/// was read into no words at all.
fn rotation_of(blocks: &[Value]) -> f64 {
    of_type(blocks, "WORD")
        .first()
        .and_then(|word| {
            word.get("Geometry")
                .and_then(|geometry| geometry.get("RotationAngle"))
                .and_then(Value::as_f64)
        })
        .unwrap_or(0.0)
}

/// Whether `(x, y)` — a point of a `Polygon`, so a page-relative fraction — names a corner
/// of an inked pixel of the page that was submitted.
///
/// A polygon point is a corner of the pixel one extreme mark was found at, so the pixel
/// it belongs to is one of the four the point touches.
fn on_a_mark(page: &GrayImage, background: u8, (x, y): (f64, f64)) -> bool {
    let column = (x * f64::from(page.width())).round() as i64;
    let row = (y * f64::from(page.height())).round() as i64;
    for cx in [column - 1, column] {
        for cy in [row - 1, row] {
            if cx < 0 || cy < 0 || cx >= i64::from(page.width()) || cy >= i64::from(page.height()) {
                continue;
            }
            if is_ink(page, background, cx as u32, cy as u32) {
                return true;
            }
        }
    }
    false
}

#[tokio::test]
async fn a_page_block_reports_the_content_measured_on_the_submitted_page() {
    let router = harness();

    // Every row's PAGE box, to state at the end what no row can state on its own: the box
    // is a measurement, so it varies with the document instead of being one constant.
    let mut page_boxes: Vec<(&str, Rect)> = Vec::new();

    for case in cases() {
        let page = page_of(&case);
        let document = raster_document(&page, ImageFormat::Png);
        let request = if case.features.is_empty() {
            json!({ "Document": document })
        } else {
            json!({ "Document": document, "FeatureTypes": case.features })
        };
        let (status, body) = call(&router, case.operation, &request).await;
        assert_eq!(
            status, 200,
            "case {:?}: a readable page must answer 200, got {status} with {body}",
            case.name
        );
        let blocks = blocks_of(&body);
        assert_block_graph(case.name, &body, &blocks, None);

        let pages = of_type(&blocks, "PAGE");
        assert_eq!(
            pages.len(),
            1,
            "case {:?}: one submitted page is one PAGE block, got {} of them",
            case.name,
            pages.len()
        );
        let page_box = bbox_of(case.name, pages[0]);
        let tol = tolerance(&page);
        let drawn = drawn_extent(&page, case.paper.background);
        match &drawn {
            // The page carries ink: the box is that ink's extent, measured here off the
            // submitted pixels.
            Some(expected) => {
                assert_same_extent(case.name, "the extent of the ink drawn on the page", &page_box, expected, tol);
            }
            // Nothing was detected on the page, so the whole of what was submitted is
            // reported.
            None => assert_same_extent(
                case.name,
                "the whole of what was submitted",
                &page_box,
                &WHOLE_PAGE,
                1e-6,
            ),
        }
        page_boxes.push((case.name, page_box));

        // The polygon outlines the same content: four points inside that box, each
        // touching one of its edges (`assert_inscribed`), each a corner of a pixel of the
        // page's own ink, listed from the corner the characters were read from.
        assert_inscribed(case.name, pages[0]);
        let reported = polygon_of(case.name, pages[0]);
        assert_eq!(
            reported.len(),
            4,
            "case {:?}: a PAGE outlines its content with four points, got {reported:?}",
            case.name
        );
        if drawn.is_some() {
            for (index, point) in reported.iter().enumerate() {
                assert!(
                    on_a_mark(&page, case.paper.background, *point),
                    "case {:?}: a PAGE Polygon point must be a corner of a pixel the page \
                     carries ink at, got {reported:?} whose point {index} ({point:?}) is on \
                     no mark",
                    case.name
                );
            }
        } else {
            assert_eq!(
                reported,
                PAGE_CORNERS.to_vec(),
                "case {:?}: a page nothing was detected on outlines the whole of what was \
                 submitted, got {reported:?}",
                case.name
            );
        }
        // Which corner the list starts at: a page read as it was submitted is outlined
        // from the corner nearest the page's top-left one, and a page read upside down
        // from the one furthest from it.
        let rotation = rotation_of(&blocks);
        let diagonal = |point: &(f64, f64)| point.0 + point.1;
        let nearest = reported.iter().map(diagonal).fold(f64::MAX, f64::min);
        let furthest = reported.iter().map(diagonal).fold(f64::MIN, f64::max);
        match rotation as i64 {
            0 => assert!(
                (diagonal(&reported[0]) - nearest).abs() < 1e-6,
                "case {:?}: a page read as it was submitted is outlined from the corner \
                 nearest its top-left one, got {reported:?}",
                case.name
            ),
            180 => assert!(
                (diagonal(&reported[0]) - furthest).abs() < 1e-6,
                "case {:?}: a page read upside down is outlined from the corner furthest \
                 from its top-left one, got {reported:?}",
                case.name
            ),
            _ => {}
        }

        // Every block read off the page sits inside the content that was detected on it:
        // the ink a block was read from is part of that content.
        for block in &blocks {
            if block_type(block) == "PAGE" {
                continue;
            }
            let block_box = bbox_of(case.name, block);
            assert!(
                block_box.left >= page_box.left - tol
                    && block_box.top >= page_box.top - tol
                    && block_box.right() <= page_box.right() + tol
                    && block_box.bottom() <= page_box.bottom() + tol,
                "case {:?}: {} block {} at {block_box:?} must sit inside its PAGE \
                 {page_box:?}",
                case.name,
                block_type(block),
                block_id(block)
            );
        }
    }

    // And the rows really did differ: the first three draw the SAME word in three places of
    // the same page, so three measurements must report three boxes — which a box fixed by
    // construction could not do.
    let moved: Vec<(&str, Rect)> = page_boxes
        .iter()
        .filter(|(case, _)| case.contains("corner") || case.contains("middle"))
        .cloned()
        .collect();
    assert!(
        moved.len() >= 2,
        "the table must submit the same ink in more than one place, got {moved:?}"
    );
    for pair in moved.windows(2) {
        assert!(
            (pair[0].1.left - pair[1].1.left).abs() > 1e-3
                || (pair[0].1.top - pair[1].1.top).abs() > 1e-3,
            "the ink moved between {:?} and {:?}, so their PAGE boxes must differ: {:?} \
             against {:?}",
            pair[0].0,
            pair[1].0,
            pair[0].1,
            pair[1].1
        );
    }
    // None of them is the whole canvas: a page whose ink covers part of it is detected on
    // part of it.
    for (case, page_box) in &moved {
        assert!(
            page_box.width < 0.9 && page_box.height < 0.9,
            "case {case:?}: one word on a page many times its size must be detected on part \
             of it, got {page_box:?}"
        );
    }
}

/// RULE: a `PAGE` block spans the content detected on the page on the fallback layout too —
/// a document whose text carries no measured geometry at all, where every block is laid out
/// on the character grid the text's own lines and columns place it on, so the content of the
/// page is exactly the blocks it was read into.
///
/// Driven through `state::blocks`, the one block builder every read path of this service
/// answers from, because no submitted page produces unmeasured text: a raster is measured.
#[test]
fn a_page_block_spans_the_content_of_a_page_that_carries_no_measurement() {
    // (case, the document's pages)
    let cases: Vec<(&str, Vec<String>)> = vec![
        ("one short line", vec!["LEDGER\n".to_string()]),
        (
            "a labelled line and a table row",
            vec!["Account Number: 48213\nItem\tPrice\nPen\t2.50\n".to_string()],
        ),
        (
            "two pages of different lengths",
            vec![
                "SIMPARITY LEDGER\n".to_string(),
                "Total Due 1250.00\nAccount Number: 48213\n".to_string(),
            ],
        ),
        ("a page with no text on it at all", vec![String::new()]),
    ];

    for (case, pages) in cases {
        let page_count = pages.len();
        let content = state::DocumentContent {
            pages,
            confidence: 90.0,
            measured: Vec::new(),
        };
        let blocks = state::blocks(
            "a-document-key",
            &content,
            state::FeatureSet {
                forms: true,
                tables: true,
            },
        );
        let page_blocks: Vec<_> = blocks
            .iter()
            .filter(|block| block.block_type.as_deref() == Some("PAGE"))
            .collect();
        assert_eq!(
            page_blocks.len(),
            page_count,
            "case {case:?}: one PAGE block per page of the document, got {}",
            page_blocks.len()
        );
        for page_block in page_blocks {
            let page_number = page_block.page.unwrap_or_default();
            let geometry = page_block
                .geometry
                .as_ref()
                .unwrap_or_else(|| panic!("case {case:?}: a PAGE must carry a Geometry"));
            let bbox = geometry
                .bounding_box
                .as_ref()
                .unwrap_or_else(|| panic!("case {case:?}: a PAGE must carry a BoundingBox"));
            let reported = Rect {
                left: f64::from(bbox.left),
                top: f64::from(bbox.top),
                width: f64::from(bbox.width),
                height: f64::from(bbox.height),
            };
            // The content of such a page is the blocks it was read into, so the box around
            // them is what the PAGE reports.
            let mut spanned: Option<Rect> = None;
            for block in blocks.iter().filter(|block| {
                block.page == Some(page_number) && block.block_type.as_deref() != Some("PAGE")
            }) {
                let Some(box_of) = block
                    .geometry
                    .as_ref()
                    .and_then(|geometry| geometry.bounding_box.as_ref())
                else {
                    continue;
                };
                let rect = Rect {
                    left: f64::from(box_of.left),
                    top: f64::from(box_of.top),
                    width: f64::from(box_of.width),
                    height: f64::from(box_of.height),
                };
                spanned = Some(match spanned {
                    None => rect,
                    Some(held) => {
                        let left = held.left.min(rect.left);
                        let top = held.top.min(rect.top);
                        Rect {
                            left,
                            top,
                            width: held.right().max(rect.right()) - left,
                            height: held.bottom().max(rect.bottom()) - top,
                        }
                    }
                });
            }
            let expected = spanned.unwrap_or(WHOLE_PAGE);
            assert!(
                (reported.left - expected.left).abs() < 1e-6
                    && (reported.top - expected.top).abs() < 1e-6
                    && (reported.width - expected.width).abs() < 1e-6
                    && (reported.height - expected.height).abs() < 1e-6,
                "case {case:?}: the PAGE block must span the content of page {page_number} \
                 {expected:?}, got {reported:?}"
            );
            let polygon = geometry
                .polygon
                .as_ref()
                .unwrap_or_else(|| panic!("case {case:?}: a PAGE must carry a Polygon"));
            let points: Vec<(f64, f64)> = polygon
                .iter()
                .map(|point| (f64::from(point.x), f64::from(point.y)))
                .collect();
            assert_eq!(
                points.len(),
                4,
                "case {case:?}: a PAGE outlines its content with four points, got {points:?}"
            );
            for (index, point) in points.iter().enumerate() {
                assert!(
                    point.0 >= expected.left - 1e-6
                        && point.0 <= expected.right() + 1e-6
                        && point.1 >= expected.top - 1e-6
                        && point.1 <= expected.bottom() + 1e-6,
                    "case {case:?}: a PAGE Polygon must lie within the content it spans \
                     {expected:?}, got {points:?} differing at point {index}"
                );
            }
        }
    }
}
