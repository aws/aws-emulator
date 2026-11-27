// SPDX-License-Identifier: CC0-1.0

//! The ink one PIECE of a cut run is named from: the stroke the cut SEVERED, followed out of
//! the piece's own columns ([`super::Ink::piece_in`]).
//!
//! A cut down a column separates two characters whose strokes touch, and a face sets
//! characters whose strokes reach over the columns of the character beside them — the bar of a
//! `T`, the arm of an `L`, the crossbar of a `t`. Wherever the cut falls, part of one
//! character's own stroke is then left standing in the other's columns, and a piece named over
//! its own columns alone is named after whatever the rest of it is shaped like: a `T` whose bar
//! was cut short reads as a stem with a flag.
//!
//! The cases here are laid out as pixels rather than drawn by a rasteriser, so what is under
//! test is the rule itself: which ink a piece is named over. The rows differ along what the
//! rule generalises over — where the cut falls across the stroke, which way the stroke runs out
//! of the piece, how thick the stroke is, and whether the ink beyond the cut is a stroke at all
//! or the BODY of the character beside it, which reaches over nothing.
//!
//! TODO: this file belongs at `src/state/ocr/severed_strokes.rs`, which is the module it is
//! declared as; it is reached through a `#[path]` override only because the tooling that added
//! it could create a file but not rename one. Rename it and drop the override on the next touch
//! of this module.

use super::*;

/// How tall the pages below are, and where their one line of ink stands.
const ROWS: usize = 16;

/// The page the rows are laid out on: `ink` is one string per row, `#` for a pixel of a
/// stroke and anything else for the ground.
fn page(rows: &[&str]) -> Ink {
    let width = rows.iter().map(|row| row.len()).max().unwrap_or(0);
    let mut gray = image::GrayImage::from_pixel(width as u32, rows.len() as u32, image::Luma([255]));
    for (y, row) in rows.iter().enumerate() {
        for (x, cell) in row.chars().enumerate() {
            if cell == '#' {
                gray.put_pixel(x as u32, y as u32, image::Luma([0]));
            }
        }
    }
    Ink::of(&gray)
}

/// The band of rows that page's line of ink stands in.
fn band(ink: &Ink) -> InkLine {
    let glyphs = ink.glyphs_between(0, ROWS - 1);
    InkLine {
        top: 0,
        bottom: ROWS - 1,
        glyphs,
    }
}

/// Which columns the piece `x0..=x1` of the run `run` is named over, as the box of the ink
/// [`Ink::piece_in`] hands back.
fn named_over(rows: &[&str], run: (usize, usize), piece: (usize, usize)) -> (usize, usize) {
    let ink = page(rows);
    let band = band(&ink);
    let within = GlyphBox {
        x0: run.0,
        x1: run.1,
        y0: 0,
        y1: ROWS - 1,
    };
    let found = ink
        .piece_in(piece.0, piece.1, &within, &band, Follow { reach: 4, thick: 3 })
        .expect("the piece carries ink");
    (found.glyph.x0, found.glyph.x1)
}

/// A `T` whose bar was cut short is named over the whole of its bar; a stem the cut leaves
/// standing beside the BODY of the character before it is named over its own columns alone.
#[test]
fn a_piece_is_named_over_the_stroke_the_cut_severed() {
    // A `T` set against a letter before it, their ink touching: the bar runs on over that
    // letter's columns, so a cut anywhere across the bar leaves part of it on the other side.
    let bar_over_a_neighbour: &[&str] = &[
        "...........",
        "...........",
        "..#######..",
        "..#...#....",
        "..#...#....",
        "..#...#....",
        "..#...#....",
        "..#####....",
        "......#....",
        "......#....",
        "......#....",
        "...........",
        "...........",
        "...........",
        "...........",
        "...........",
    ];
    // The same ink without the bar running on: the character beside the piece is a body (a
    // stem of its own), which reaches over no columns at all.
    let body_beside_the_piece: &[&str] = &[
        "...........",
        "...........",
        "..##...##..",
        "..##...##..",
        "..##...##..",
        "..##...##..",
        "..##...##..",
        "..##...##..",
        "..##...##..",
        "..##...##..",
        "..##...##..",
        "...........",
        "...........",
        "...........",
        "...........",
        "...........",
    ];
    /// One row of the table: the page's ink, the run of inked columns its characters stand in,
    /// the piece of that run a cut leaves, and the columns that piece must be named over.
    struct Row {
        case: &'static str,
        rows: &'static [&'static str],
        run: (usize, usize),
        piece: (usize, usize),
        named_over: (usize, usize),
    }
    let cases: Vec<Row> = vec![
        Row {
            // The bar runs on left of the cut as far as the body of the letter before it,
            // which is where the stroke ends and that letter's own ink begins.
            case: "the bar cut at its middle, the piece to its right",
            rows: bar_over_a_neighbour,
            run: (2, 8),
            piece: (5, 8),
            named_over: (3, 8),
        },
        Row {
            case: "the bar cut one column further right",
            rows: bar_over_a_neighbour,
            run: (2, 8),
            piece: (6, 8),
            named_over: (3, 8),
        },
        Row {
            // Beyond this piece stands the BODY of the character after it — a stem the full
            // height of the line — which reaches over no columns at all.
            case: "the piece to the LEFT of the cut, a body beyond it",
            rows: bar_over_a_neighbour,
            run: (2, 8),
            piece: (2, 5),
            named_over: (2, 5),
        },
        Row {
            case: "a body beside the piece, which reaches over nothing",
            rows: body_beside_the_piece,
            run: (2, 8),
            piece: (5, 8),
            named_over: (7, 8),
        },
    ];
    for row in cases {
        let over = named_over(row.rows, row.run, row.piece);
        assert_eq!(
            over, row.named_over,
            "case {:?}: the piece {:?} of the run {:?} must be named over the columns {:?}, \
             got {over:?}",
            row.case, row.piece, row.run, row.named_over
        );
    }
}

/// The ink the stroke of one column runs on into is the stroke's own: a column of the page's
/// ground stops the walk, and so does the body of a character.
#[test]
fn a_stroke_is_followed_only_while_its_own_ink_runs_on() {
    let rows: &[&str] = &[
        "..............",
        "..............",
        "..####..####..",
        "..............",
        "..............",
        "..............",
        "..............",
        "..............",
        "..............",
        "..............",
        "..............",
        "..............",
        "..............",
        "..............",
        "..............",
        "..............",
    ];
    let ink = page(rows);
    let (found, steps) = ink.along_stroke(
        5,
        (2, 2),
        1,
        13,
        (0, ROWS - 1),
        Follow { reach: 8, thick: 3 },
    );
    assert_eq!(
        steps, 0,
        "case a gap beyond the stroke: the walk must stop at the ground, got {found:?}"
    );
    let (found, steps) = ink.along_stroke(
        2,
        (2, 2),
        1,
        13,
        (0, ROWS - 1),
        Follow { reach: 8, thick: 3 },
    );
    assert_eq!(
        (steps, found.len()),
        (3, 3),
        "case the stroke's own columns: the walk must follow it to its end, got {found:?}"
    );
}
