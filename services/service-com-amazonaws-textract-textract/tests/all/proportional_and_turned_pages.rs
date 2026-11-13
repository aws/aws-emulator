// SPDX-License-Identifier: CC0-1.0

//! End-to-end coverage of the rule that a synchronous read DESCRIBES the submitted
//! document: one block per page, per line and per word that was drawn on it, each block's
//! `Text` the characters drawn in its own ink, each block's `Geometry` the extent of THAT
//! block's own pixels, and `Geometry$RotationAngle` the angle the words were read at.
//!
//! The rule, over a table of pages rather than one recorded document:
//!
//! * the words of a line are separated where the page leaves a character cell empty, so
//!   `DetectDocumentText` answers one `WORD` block per drawn word and one `LINE` block
//!   whose text is the drawn words with one space between them — whether the page sets its
//!   letters in cells of one width (a monospaced rendering, which
//!   `crate::raster_documents` draws) or advances each letter by its own width (a
//!   proportional rendering, which this suite draws, and where the space between two words
//!   is NARROWER than a letter);
//! * `AnalyzeDocument` with `FORMS` reads the same page into the same `PAGE`/`LINE`/`WORD`
//!   layer and adds the `KEY_VALUE_SET` pair of each labelled line, which exists only
//!   because the label and its value were separated into words at all;
//! * the `PAGE` block's box spans the page's CONTENT — every mark found on it, including a
//!   rule that is no line of text — rather than the first line that was read;
//! * a page submitted turned a quarter, a half or three quarters of a turn reads the same
//!   characters, reports every word where its ink sits on the page AS SUBMITTED, and
//!   reports the turn the words are drawn at as the `RotationAngle` of every `WORD`.
//!
//! Every page here is drawn in this test, so each assertion compares what the service
//! reports with WHERE THE INK WAS PUT and WHICH CHARACTERS were drawn, never with a
//! recorded answer.

use image::{GrayImage, ImageFormat, Luma};
use serde_json::{json, Value};

use crate::document_extraction::{
    assert_block_graph, block_type, blocks_of, call, harness, of_type,
};
use crate::raster_documents::raster_document;

/// The side of one glyph cell of the embedded font.
const CELL: usize = 8;

/// A rectangle of the page, in pixels.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Pixels {
    left: usize,
    top: usize,
    width: usize,
    height: usize,
}

/// A `BoundingBox` as it came back over the wire.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Rect {
    left: f64,
    top: f64,
    width: f64,
    height: f64,
}

impl Rect {
    /// Where `pixels` sits on a `width` x `height` page, as the page-relative fractions
    /// `BoundingBox` is expressed in.
    fn of(pixels: Pixels, width: usize, height: usize) -> Self {
        Self {
            left: pixels.left as f64 / width as f64,
            top: pixels.top as f64 / height as f64,
            width: pixels.width as f64 / width as f64,
            height: pixels.height as f64 / height as f64,
        }
    }

    fn right(&self) -> f64 {
        self.left + self.width
    }

    fn bottom(&self) -> f64 {
        self.top + self.height
    }

    /// Whether this box is `other`, give or take `tol` on every edge.
    fn matches(&self, other: &Rect, tol: f64) -> bool {
        (self.left - other.left).abs() <= tol
            && (self.top - other.top).abs() <= tol
            && (self.right() - other.right()).abs() <= tol
            && (self.bottom() - other.bottom()).abs() <= tol
    }

    fn union(rects: &[Rect]) -> Rect {
        let left = rects.iter().map(|rect| rect.left).fold(f64::MAX, f64::min);
        let top = rects.iter().map(|rect| rect.top).fold(f64::MAX, f64::min);
        let right = rects.iter().map(Rect::right).fold(f64::MIN, f64::max);
        let bottom = rects.iter().map(Rect::bottom).fold(f64::MIN, f64::max);
        Rect {
            left,
            top,
            width: right - left,
            height: bottom - top,
        }
    }
}

/// The columns of the embedded font's cell that `ch` puts ink in.
///
/// `font8x8::legacy::BASIC_LEGACY` is the public-domain 8x8 bitmap font the recogniser
/// holds its templates for: one byte per row, bit 0 the leftmost pixel. Trimming the cell
/// to the ink is what makes this renderer PROPORTIONAL — every letter then takes the width
/// of its own ink instead of a cell of one width.
fn glyph_ink(ch: char) -> (Vec<u8>, usize, usize) {
    let rows = font8x8::legacy::BASIC_LEGACY[ch as usize];
    let lit = |x: usize, y: usize| rows[y] & (1 << x) != 0;
    let mut first = CELL;
    let mut last = 0usize;
    for y in 0..CELL {
        for x in 0..CELL {
            if lit(x, y) {
                first = first.min(x);
                last = last.max(x);
            }
        }
    }
    assert!(first <= last, "the font draws no ink for {ch:?}");
    (rows.to_vec(), first, last)
}

/// How a proportional page is drawn. Each knob is one axis the rule must hold along.
#[derive(Debug, Clone, Copy)]
struct Paper {
    /// Pixels per font pixel: the size the text is drawn at.
    scale: usize,
    /// Blank pixels left between two letters of one word.
    letter_gap: usize,
    /// Blank pixels left between two words.
    word_gap: usize,
    /// Blank pixels between the ink of two lines.
    line_gap: usize,
    /// Blank pixels around the text.
    margin: usize,
    /// Draw a two-pixel rule across the page under the text: a border, not a line of text.
    rule: bool,
    /// Underline every line, with the rule touching the letters it runs under — which
    /// joins every letter of the line into one mark of ink.
    underline: bool,
    /// Draw every line as LIGHT text on a dark panel: a filled rectangle around the line,
    /// with the letters left in the page's own colour.
    panel: bool,
    /// Bridge every pair of neighbouring letters with a one-pixel hairline of ink, at a
    /// height that varies along the line — what a heavy stroke or a coarse threshold leaves
    /// between two characters, and what joins a whole line into one mark.
    bridge: bool,
}

/// Blank pixels between a panel's edge and the text it holds.
const PANEL_PADDING: usize = 3;

/// A page drawn from `lines`, and where every word's ink was put on it.
struct Drawn {
    page: GrayImage,
    /// One entry per line, each the pixel extent of that line's letters in reading order.
    words: Vec<Vec<Pixels>>,
    /// The extent of every mark on the page: the letters, and the rules and panels drawn
    /// around them.
    content: Pixels,
}

/// Draw `lines` with each letter advanced by its OWN width, so the space between two words
/// is narrower than a letter — which is what a page set in a proportional font looks like,
/// and what no cell of one width describes.
fn draw(lines: &[&str], paper: Paper) -> Drawn {
    let glyph_height = CELL * paper.scale;
    // Measure first, so the page is exactly as big as the text plus its margins.
    let mut widths: Vec<usize> = Vec::new();
    for line in lines {
        let mut width = 0usize;
        for (index, word) in line.split(' ').enumerate() {
            if index > 0 {
                width += paper.word_gap;
            }
            for (letter, ch) in word.chars().enumerate() {
                if letter > 0 {
                    width += paper.letter_gap;
                }
                let (_, first, last) = glyph_ink(ch);
                width += (last + 1 - first) * paper.scale;
            }
        }
        widths.push(width);
    }
    let text_width = widths.iter().copied().max().unwrap_or(0);
    let text_height = lines.len() * glyph_height + lines.len().saturating_sub(1) * paper.line_gap;
    let frame = if paper.panel { PANEL_PADDING + 1 } else { 0 };
    let width = paper.margin * 2 + text_width + frame * 2;
    let height = paper.margin * 2
        + text_height
        + frame * 2 * lines.len()
        + if paper.rule { paper.margin } else { 0 }
        + if paper.underline { 2 * lines.len() } else { 0 };
    let mut page = GrayImage::from_pixel(width as u32, height as u32, Luma([255]));
    let ink = if paper.panel { 255u8 } else { 0u8 };

    let mut words: Vec<Vec<Pixels>> = Vec::new();
    let mut marks: Vec<Pixels> = Vec::new();
    let mut top = paper.margin;
    for line in lines {
        let line_left = paper.margin + frame;
        if paper.panel {
            // The panel first, so the letters are left in the page's own colour inside it.
            let panel = Pixels {
                left: paper.margin,
                top,
                width: text_width + frame * 2,
                height: glyph_height + frame * 2,
            };
            for y in panel.top..panel.top + panel.height {
                for x in panel.left..panel.left + panel.width {
                    page.put_pixel(x as u32, y as u32, Luma([0]));
                }
            }
            marks.push(panel);
        }
        let glyph_top = top + frame;
        let mut x = line_left;
        let mut drawn_words: Vec<Pixels> = Vec::new();
        let mut letters: Vec<(usize, usize)> = Vec::new();
        for (index, word) in line.split(' ').enumerate() {
            if index > 0 {
                x += paper.word_gap;
            }
            let word_left = x;
            let mut ink_top = glyph_top + glyph_height;
            let mut ink_bottom = glyph_top;
            for (letter, ch) in word.chars().enumerate() {
                if letter > 0 {
                    x += paper.letter_gap;
                }
                let (rows, first, last) = glyph_ink(ch);
                for (gy, bits) in rows.iter().enumerate() {
                    for gx in first..=last {
                        if bits & (1 << gx) == 0 {
                            continue;
                        }
                        ink_top = ink_top.min(glyph_top + gy * paper.scale);
                        ink_bottom = ink_bottom.max(glyph_top + (gy + 1) * paper.scale);
                        for dy in 0..paper.scale {
                            for dx in 0..paper.scale {
                                page.put_pixel(
                                    (x + (gx - first) * paper.scale + dx) as u32,
                                    (glyph_top + gy * paper.scale + dy) as u32,
                                    Luma([ink]),
                                );
                            }
                        }
                    }
                }
                let letter_width = (last + 1 - first) * paper.scale;
                letters.push((x, x + letter_width - 1));
                x += letter_width;
            }
            let word_box = Pixels {
                left: word_left,
                top: ink_top,
                width: x - word_left,
                height: ink_bottom - ink_top,
            };
            marks.push(word_box);
            drawn_words.push(word_box);
        }
        if paper.bridge {
            // A hairline of ink in the gap between two letters, at a height that varies
            // along the line, so no row of the line is one long run and every column of it
            // carries ink: the whole line is one mark.
            let base = drawn_words
                .iter()
                .map(|word| word.top + word.height)
                .max()
                .unwrap_or(glyph_top);
            for (pair, gap) in letters.windows(2).enumerate() {
                let y = base - 1 - (pair % 3) * paper.scale;
                for mark in gap[0].1 + 1..gap[1].0 {
                    page.put_pixel(mark as u32, y as u32, Luma([ink]));
                }
            }
        }
        if paper.underline {
            // A rule touching the letters it runs under, which inks every column of the
            // line and so joins its letters into one mark.
            let bottom = drawn_words
                .iter()
                .map(|word| word.top + word.height)
                .max()
                .unwrap_or(glyph_top);
            let right = drawn_words
                .last()
                .map_or(line_left, |word| word.left + word.width);
            for y in bottom..bottom + 2 {
                for mark in line_left..right {
                    page.put_pixel(mark as u32, y as u32, Luma([ink]));
                }
            }
            marks.push(Pixels {
                left: line_left,
                top: bottom,
                width: right - line_left,
                height: 2,
            });
        }
        words.push(drawn_words);
        top = glyph_top + glyph_height + frame + paper.line_gap
            + if paper.underline { 2 } else { 0 };
    }

    if paper.rule {
        let y = height - 3;
        for x in 0..width {
            for dy in 0..2 {
                page.put_pixel(x as u32, (y + dy) as u32, Luma([0]));
            }
        }
        marks.push(Pixels {
            left: 0,
            top: y,
            width,
            height: 2,
        });
    }

    let left = marks.iter().map(|mark| mark.left).min().unwrap_or(0);
    let right = marks
        .iter()
        .map(|mark| mark.left + mark.width)
        .max()
        .unwrap_or(0);
    let content_top = marks.iter().map(|mark| mark.top).min().unwrap_or(0);
    let bottom = marks
        .iter()
        .map(|mark| mark.top + mark.height)
        .max()
        .unwrap_or(0);
    Drawn {
        page,
        words,
        content: Pixels {
            left,
            top: content_top,
            width: right - left,
            height: bottom - content_top,
        },
    }
}

/// One row of the table: a page, the text it was drawn from, how it was drawn, the quarter
/// turns it was submitted at, and the read path it is read through.
struct Case {
    name: &'static str,
    lines: Vec<&'static str>,
    paper: Paper,
    /// Clockwise quarter turns applied to the drawn page before it was submitted.
    turns: u8,
    operation: &'static str,
    /// The `FeatureTypes` of an `AnalyzeDocument` row; empty for text detection.
    features: Vec<&'static str>,
}

impl Paper {
    /// Letters a pixel or two apart and a word space NARROWER than one letter: a page no
    /// cell of one width describes.
    const fn tight() -> Self {
        Self {
            scale: 2,
            letter_gap: 1,
            word_gap: 7,
            line_gap: 6,
            margin: 5,
            rule: false,
            underline: false,
            panel: false,
            bridge: false,
        }
    }
}

/// Mixed case, digits, punctuation and a labelled line, with letters of several widths so
/// no cell of one width fits the page and no two words of a line share a box.
fn sample_lines() -> Vec<&'static str> {
    vec!["SIMPARITY LEDGER", "Account Number: 48213", "Total Due 1250.00"]
}

fn cases() -> Vec<Case> {
    vec![
        Case {
            name: "tight letters and a space narrower than one letter",
            lines: sample_lines(),
            paper: Paper::tight(),
            turns: 0,
            operation: "DetectDocumentText",
            features: Vec::new(),
        },
        Case {
            name: "the same page drawn small",
            lines: sample_lines(),
            paper: Paper {
                scale: 1,
                letter_gap: 1,
                word_gap: 4,
                line_gap: 4,
                ..Paper::tight()
            },
            turns: 0,
            operation: "DetectDocumentText",
            features: Vec::new(),
        },
        Case {
            name: "the same page drawn large, with wide letter spacing",
            lines: sample_lines(),
            paper: Paper {
                scale: 4,
                letter_gap: 4,
                word_gap: 22,
                line_gap: 12,
                margin: 30,
                ..Paper::tight()
            },
            turns: 0,
            operation: "DetectDocumentText",
            features: Vec::new(),
        },
        Case {
            name: "a single word, which no space divides",
            lines: vec!["SIMPARITY"],
            paper: Paper::tight(),
            turns: 0,
            operation: "DetectDocumentText",
            features: Vec::new(),
        },
        Case {
            name: "a one-letter word beside longer ones",
            lines: vec!["A BC 7"],
            // A space as wide as a letter: a line whose words are one letter long carries
            // hardly a gap inside a word to measure its spaces against, so the space
            // itself has to stand as wide as the letters do.
            paper: Paper {
                word_gap: 12,
                ..Paper::tight()
            },
            turns: 0,
            operation: "DetectDocumentText",
            features: Vec::new(),
        },
        Case {
            name: "a page carrying a rule, which is content but no line of text",
            lines: sample_lines(),
            paper: Paper {
                rule: true,
                ..Paper::tight()
            },
            turns: 0,
            operation: "DetectDocumentText",
            features: Vec::new(),
        },
        Case {
            name: "underlined lines, where the rule joins every letter into one mark",
            lines: sample_lines(),
            paper: Paper {
                underline: true,
                ..Paper::tight()
            },
            turns: 0,
            operation: "DetectDocumentText",
            features: Vec::new(),
        },
        Case {
            name: "light text on a dark panel",
            lines: sample_lines(),
            paper: Paper {
                panel: true,
                // A wide margin, so the dark panels are the smaller part of the page and
                // the page's own ground stays its ground.
                margin: 40,
                ..Paper::tight()
            },
            turns: 0,
            operation: "DetectDocumentText",
            features: Vec::new(),
        },
        Case {
            name: "letters bridged by hairlines, which join the line into one mark",
            lines: sample_lines(),
            paper: Paper {
                bridge: true,
                scale: 4,
                letter_gap: 2,
                word_gap: 14,
                ..Paper::tight()
            },
            turns: 0,
            operation: "DetectDocumentText",
            features: Vec::new(),
        },
        Case {
            name: "an underlined page turned a half turn",
            lines: vec!["Account Number: 48213"],
            paper: Paper {
                underline: true,
                ..Paper::tight()
            },
            turns: 2,
            operation: "DetectDocumentText",
            features: Vec::new(),
        },
        Case {
            name: "a quarter turn clockwise",
            lines: sample_lines(),
            paper: Paper::tight(),
            turns: 1,
            operation: "DetectDocumentText",
            features: Vec::new(),
        },
        Case {
            name: "a half turn",
            lines: sample_lines(),
            paper: Paper::tight(),
            turns: 2,
            operation: "DetectDocumentText",
            features: Vec::new(),
        },
        Case {
            name: "three quarters of a turn clockwise",
            lines: sample_lines(),
            paper: Paper::tight(),
            turns: 3,
            operation: "DetectDocumentText",
            features: Vec::new(),
        },
        Case {
            name: "the analysis path over a labelled line",
            lines: vec!["Account Number: 48213"],
            paper: Paper::tight(),
            turns: 0,
            operation: "AnalyzeDocument",
            features: vec!["FORMS"],
        },
        Case {
            name: "the analysis path over a page turned a half turn",
            lines: sample_lines(),
            paper: Paper::tight(),
            turns: 2,
            operation: "AnalyzeDocument",
            features: vec!["FORMS"],
        },
    ]
}

/// `pixels` of the drawn page, as a fraction of the page SUBMITTED after `turns` clockwise
/// quarter turns of it.
fn turned(pixels: Pixels, turns: u8, width: usize, height: usize) -> Rect {
    match turns % 4 {
        // A clockwise turn sends the drawn page's left edge to the submitted page's top.
        1 => Rect::of(
            Pixels {
                left: height - pixels.top - pixels.height,
                top: pixels.left,
                width: pixels.height,
                height: pixels.width,
            },
            height,
            width,
        ),
        2 => Rect::of(
            Pixels {
                left: width - pixels.left - pixels.width,
                top: height - pixels.top - pixels.height,
                width: pixels.width,
                height: pixels.height,
            },
            width,
            height,
        ),
        3 => Rect::of(
            Pixels {
                left: pixels.top,
                top: width - pixels.left - pixels.width,
                width: pixels.height,
                height: pixels.width,
            },
            height,
            width,
        ),
        _ => Rect::of(pixels, width, height),
    }
}

fn text_of(block: &Value) -> String {
    block
        .get("Text")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

fn bbox_of(case: &str, block: &Value) -> Rect {
    let bbox = block
        .get("Geometry")
        .and_then(|geometry| geometry.get("BoundingBox"))
        .unwrap_or_else(|| panic!("case {case:?}: a block carries no BoundingBox, got {block}"));
    let number = |member: &str| bbox.get(member).and_then(Value::as_f64).unwrap_or(f64::NAN);
    Rect {
        left: number("Left"),
        top: number("Top"),
        width: number("Width"),
        height: number("Height"),
    }
}

fn rotation_of(block: &Value) -> Option<f64> {
    block
        .get("Geometry")
        .and_then(|geometry| geometry.get("RotationAngle"))
        .and_then(Value::as_f64)
}

#[tokio::test]
async fn a_pages_blocks_describe_the_words_that_were_drawn_on_it() {
    let router = harness();

    for case in cases() {
        let drawn = draw(&case.lines, case.paper);
        let (drawn_width, drawn_height) = (
            drawn.page.width() as usize,
            drawn.page.height() as usize,
        );
        let submitted = match case.turns % 4 {
            1 => image::imageops::rotate90(&drawn.page),
            2 => image::imageops::rotate180(&drawn.page),
            3 => image::imageops::rotate270(&drawn.page),
            _ => drawn.page.clone(),
        };
        let document = raster_document(&submitted, ImageFormat::Png);
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

        // Two pixels of slack per font pixel: a binarisation decides an edge pixel, and the
        // assertion is about WHERE the ink is, not about which of its edge pixels is ink.
        let tol = 2.0 * case.paper.scale as f64 / drawn_width.min(drawn_height) as f64;

        // One block per page, per drawn line and per drawn word — and, on the analysis
        // path, the KEY_VALUE_SET pair of each labelled line on top of exactly that.
        let drawn_words: Vec<&str> = case
            .lines
            .iter()
            .flat_map(|line| line.split(' '))
            .collect();
        let lines = of_type(&blocks, "LINE");
        let words = of_type(&blocks, "WORD");
        assert_eq!(
            lines.len(),
            case.lines.len(),
            "case {:?}: expected one LINE per drawn line, got {:?}",
            case.name,
            lines.iter().map(|line| text_of(line)).collect::<Vec<_>>()
        );
        assert_eq!(
            words.len(),
            drawn_words.len(),
            "case {:?}: expected one WORD per drawn word ({drawn_words:?}), got {:?}",
            case.name,
            words.iter().map(|word| text_of(word)).collect::<Vec<_>>()
        );
        let text_layer = blocks
            .iter()
            .filter(|block| matches!(block_type(block), "PAGE" | "LINE" | "WORD"))
            .count();
        assert_eq!(
            text_layer,
            1 + case.lines.len() + drawn_words.len(),
            "case {:?}: the PAGE/LINE/WORD layer must be the page, its lines and their \
             words and nothing else, got {:?}",
            case.name,
            blocks.iter().map(block_type).collect::<Vec<_>>()
        );

        // The characters that came back are the characters that were drawn.
        assert_eq!(
            words.iter().map(|word| text_of(word)).collect::<Vec<_>>(),
            drawn_words,
            "case {:?}: every WORD must report the characters drawn in its own ink",
            case.name
        );
        assert_eq!(
            lines.iter().map(|line| text_of(line)).collect::<Vec<_>>(),
            case.lines,
            "case {:?}: every LINE must report the words drawn on it, one space apart",
            case.name
        );

        // Every WORD sits where ITS OWN ink was put, so no two words of a line share a box.
        let mut word_boxes: Vec<Rect> = Vec::new();
        let mut reported = words.iter();
        for (row, line) in drawn.words.iter().enumerate() {
            for (index, pixels) in line.iter().enumerate() {
                let block = reported.next().expect("one WORD per drawn word");
                let expected = turned(*pixels, case.turns, drawn_width, drawn_height);
                let reported_box = bbox_of(case.name, block);
                word_boxes.push(reported_box);
                assert!(
                    reported_box.matches(&expected, tol),
                    "case {:?}: WORD {:?} (line {row}, word {index}) was drawn at \
                     {expected:?}, but reports {reported_box:?}",
                    case.name,
                    text_of(block)
                );
                assert_eq!(
                    rotation_of(block),
                    Some(f64::from(u16::from(case.turns % 4)) * 90.0),
                    "case {:?}: WORD {:?} must report the turn its characters are drawn \
                     at as its RotationAngle",
                    case.name,
                    text_of(block)
                );
            }
        }

        // The PAGE spans the page's content: every mark found on it, the rule included.
        let page_box = bbox_of(case.name, of_type(&blocks, "PAGE")[0]);
        let content = turned(drawn.content, case.turns, drawn_width, drawn_height);
        assert!(
            page_box.matches(&content, tol),
            "case {:?}: the PAGE box must span the page's content ({content:?}), got \
             {page_box:?}",
            case.name
        );
        let spanned = Rect::union(&word_boxes);
        assert!(
            page_box.width + tol >= spanned.width && page_box.height + tol >= spanned.height,
            "case {:?}: the PAGE box must span every word of the page, got {page_box:?} \
             against {spanned:?}",
            case.name
        );

        // A labelled line is read into a KEY and a VALUE, which exists only because the
        // label and its value were separated into words at all. A line labels its value
        // either by punctuating the label or by ending in an amount.
        if case.features.contains(&"FORMS") {
            let labelled = case
                .lines
                .iter()
                .filter(|line| {
                    let punctuated = line.split(' ').any(|word| word.ends_with(':'));
                    let amount = line.split(' ').next_back().is_some_and(|last| {
                        last.chars().any(|c| c.is_ascii_digit())
                            && last.chars().all(|c| c.is_ascii_digit() || ".,".contains(c))
                    });
                    (punctuated || amount) && line.contains(' ')
                })
                .count();
            assert_eq!(
                of_type(&blocks, "KEY_VALUE_SET").len(),
                2 * labelled,
                "case {:?}: expected a KEY and a VALUE per labelled line ({labelled}), got \
                 {:?}",
                case.name,
                of_type(&blocks, "KEY_VALUE_SET")
                    .iter()
                    .map(|block| text_of(block))
                    .collect::<Vec<_>>()
            );
        }
    }
}
