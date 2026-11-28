// SPDX-License-Identifier: CC0-1.0

//! Why a comparison measured only as far as it can still change the answer gives the answer
//! an unbounded one gives.
//!
//! Every comparison in [`super`] carries the agreement it has to beat to matter — the best
//! face found so far, the poorest character a shortlist still holds, or [`super::NAMING_FLOOR`],
//! below which a character enters no reading at all — and a template that cannot be carried
//! past it by the cells left is dropped where it stands. What that must never do is change
//! the character a mark is named: a template is dropped only once the ink it has left is too
//! little to overtake the one that is winning.
//!
//! The rows differ along what the rule generalises over: the character the mark is drawn
//! with, the size it is drawn at, the floor the comparison is given, and which face of the
//! character it is told to measure first.
//!
//! TODO: this file belongs at `src/state/ocr/glyphs/bounds.rs`, which is the module it is
//! declared as; it is reached through a `#[path]` override only because the tooling that
//! added it could create a file but not rename one. Rename it and drop the override on the
//! next touch of this module.

use super::*;

/// How far a bounded comparison may fall short of an unbounded one.
///
/// A bound is allowed a hair of slack ([`BOUND_SLACK`]), so that a bound rounded a fraction
/// of a cell short can never drop a template that was about to win; the cost of that slack
/// is that the answer may be that hair low. It is orders of magnitude under the margins the
/// reading is decided by — [`TIED`] and [`STEM_MARGIN`] are hundredths — so no reading turns
/// on it.
const SLACK: f32 = 1e-4;

/// The marks the rows are measured from: one glyph each of the letterform classes a floor
/// could have cut short — a flat capital, a letter with a descender, a figure drawn around
/// two counters, a bare stem and a mark of a cell or two — drawn by the proportional outline
/// face at sizes the bank holds no template at, so the mark is not a copy of any template.
fn marks() -> Vec<(String, Mark)> {
    let mut out = Vec::new();
    for size in [17.0f32, 29.0] {
        let face = outline_face(ttf_noto_sans::REGULAR, size);
        for ch in ['A', 'g', '8', 'l', ':'] {
            let Some(mark) = face(ch) else {
                panic!("case {ch:?} at {size} pixels: the outline face must draw it");
            };
            out.push((format!("{ch:?} at {size} pixels"), mark));
        }
    }
    out
}

/// The floors a comparison is held against here: none at all, the floor the naming itself
/// uses, the poorest reading there can be, and one no mark reaches.
const FLOORS: &[f32] = &[0.0, NAMING_FLOOR, MIN_MATCH, 0.95];

/// For every character of the bank, the agreement a BOUNDED comparison reports is the
/// agreement an unbounded one reports wherever that agreement stands above the floor, and at
/// most the floor wherever it does not — whichever face it is told to measure first.
#[test]
fn a_comparison_measured_against_a_floor_answers_as_an_unbounded_one() {
    for (case, mark) in marks() {
        for glyph in bank() {
            let unbounded = score_against(&mark, glyph, 0.0, 0);
            let faces = glyph.faces.len();
            // The face measured first is a HINT: the one the mark's ink is closest to as the
            // grids stand, a face at the far end of the character's own list, and an index
            // the character has no face at.
            for first in [0, faces - 1, faces] {
                for floor in FLOORS {
                    let bounded = score_against(&mark, glyph, *floor, first);
                    if unbounded > *floor {
                        assert!(
                            (bounded - unbounded).abs() <= SLACK,
                            "case {case} against {:?}, floor {floor}, first face {first}: a \
                             comparison bounded below an agreement it reaches must report \
                             that agreement, expected {unbounded} and got {bounded}",
                            glyph.ch
                        );
                    } else {
                        assert!(
                            bounded <= *floor,
                            "case {case} against {:?}, floor {floor}, first face {first}: a \
                             comparison bounded above the agreement it reaches must report no \
                             more than its floor, expected at most {floor} and got {bounded}",
                            glyph.ch
                        );
                    }
                }
            }
        }
    }
}

/// The same of the comparison on the grids BEFORE they were blurred, which is what settles a
/// reading the blurred grids left tied: it too answers as an unbounded comparison wherever
/// its answer stands above the floor it was given.
#[test]
fn the_sharp_comparison_answers_as_an_unbounded_one_too() {
    for (case, mark) in marks() {
        for glyph in bank() {
            let unbounded = sharp_against(&mark, glyph, 0.0);
            for floor in FLOORS {
                let bounded = sharp_against(&mark, glyph, *floor);
                if unbounded > *floor {
                    assert!(
                        (bounded - unbounded).abs() <= SLACK,
                        "case {case} against {:?}, floor {floor}: the sharp comparison must \
                         report the agreement it reaches, expected {unbounded} and got \
                         {bounded}",
                        glyph.ch
                    );
                } else {
                    assert!(
                        bounded <= *floor,
                        "case {case} against {:?}, floor {floor}: the sharp comparison must \
                         report no more than its floor, expected at most {floor} and got \
                         {bounded}",
                        glyph.ch
                    );
                }
            }
        }
    }
}

/// A grid shares with another exactly the ink the two lay in the same cells, whatever floor
/// the comparison is given and at every shift it searches: the bounded search over the
/// one-cell neighbourhood reports the best shift's own Sørensen-Dice coefficient.
///
/// Measured against the coefficient worked out here from the cells themselves rather than
/// against the module's own helpers, so what is pinned is the measure and not merely that
/// two of its callers agree.
#[test]
fn the_search_reports_the_best_shift_of_the_two_grids() {
    for (case, mark) in marks() {
        for glyph in bank().iter().take(12) {
            for face in &glyph.faces {
                let mut best = 0f32;
                for down in -1..=1isize {
                    for across in -1..=1isize {
                        let total = mark.fine.ink + face.fine.ink;
                        let mut shared = 0f32;
                        for row in 0..FINE as isize {
                            for column in 0..FINE as isize {
                                let (their_row, their_column) = (row + down, column + across);
                                if !(0..FINE as isize).contains(&their_row)
                                    || !(0..FINE as isize).contains(&their_column)
                                {
                                    continue;
                                }
                                shared += mark.fine.cells[row as usize * FINE + column as usize]
                                    .min(
                                        face.fine.cells
                                            [their_row as usize * FINE + their_column as usize],
                                    );
                            }
                        }
                        if total > 0.0 {
                            best = best.max(2.0 * shared / total);
                        }
                    }
                }
                let measured = mark.agreement_above(face, 0.0);
                assert!(
                    (measured - best).abs() <= SLACK,
                    "case {case} against {:?}: the search must report the best shift's own \
                     coefficient, expected {best} and got {measured}",
                    glyph.ch
                );
            }
        }
    }
}
