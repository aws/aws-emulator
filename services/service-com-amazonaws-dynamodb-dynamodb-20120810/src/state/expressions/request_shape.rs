// SPDX-License-Identifier: CC0-1.0

//! What the SHAPE of an update's request costs the item-size accounting.
//!
//! An update states what to CHANGE rather than the item to store, so what it asks for is
//! not the item it leaves behind: a request is sized against the documented 400 KB item
//! ceiling over that item PLUS a frame for each clause it states. Which means the largest
//! operand one write can carry MOVES with the clauses stated beside it — by a fixed amount
//! per clause KIND, the same in every Region and the same whatever the clause's operand
//! holds — rather than only with the bytes the result stores.
//!
//! Each frame is read off exactly that boundary: the amount by which the largest accepted
//! operand shrinks when one clause of that kind is stated beside the one assignment every
//! such write already carries. Those shrinks are RECORDED behaviour rather than a published
//! figure — AWS documents the 400 KB item ceiling ("Service, Account, and Table Quotas":
//! "The maximum item size in DynamoDB is 400 KB") and says nothing about what a clause
//! costs on top of the item — so each constant below names the shrink it was read from, and
//! an operand length is the only thing a caller has to vary to read it back
//! (`tests/all/request_shape_validation_classes.rs` does exactly that, one row per clause
//! kind).
//!
//! A clause that stores something of its own moves the boundary by its frame AND by those
//! bytes, which the item the write leaves already accounts for, so the frames below are that
//! shrink NET of them — which is why a clause writing a computed number carries a frame two
//! bytes under the shrink it is observed to cause, and an assignment reaching into a
//! document carries the plain assignment's frame beside the bytes the document it builds
//! stores.
//!
//! What this does NOT settle is where the boundary sits in ABSOLUTE terms for a plain
//! single-assignment update: that is the item ceiling's own accounting
//! ([`super::update_charged_size`], which charges the part of the item the change does not
//! address at [`super::UNTOUCHED_PORTION_FLOOR_BYTES`]), and the frames are differences
//! invariant under it.
//! Each is read off the boundary of a write that already carries ONE assignment, so that
//! assignment is the baseline the differences are counted from and costs nothing of its own
//! ([`BASELINE_CLAUSE_CHARGE`]): a single-clause update is sized against the item ceiling over
//! what its item is CHARGED and nothing besides.
//!
//! Nothing of the request's text is read for a frame, so two requests stating the same
//! clauses are sized alike; the one exception is a `REMOVE`, whose ask IS the path it names
//! because it writes nothing at all.
//!
//! [`super`] re-exports them, so every caller still names them `crate::state::…`.

use super::*;

/// The frame one further `SET` assignment costs — `SET v = :v, v2 = :v2` accepts an
/// operand this much shorter than `SET v = :v` does, over and above the bytes the second
/// attribute stores. An assignment whose target reaches INSIDE an attribute carries the
/// same frame; what moves the boundary further there is the document it builds.
pub(super) const SET_CLAUSE_CHARGE: i64 = 19;

/// The frame the ONE clause every measured write already carried: a plain assignment, the
/// baseline each of the frames here is a difference FROM.
///
/// It is taken back out of an update's charge ([`ParsedUpdate::request_shape_charge`]), so
/// the frames stay the differences they were read as and the absolute boundary of a
/// single-clause update is the item ceiling itself.
pub(super) const BASELINE_CLAUSE_CHARGE: i64 = SET_CLAUSE_CHARGE;

/// The frame an assignment whose operand is ARITHMETIC costs: `+` or `-` over two
/// operands, each of which the write has to read before it can store anything. The
/// boundary such a clause is observed to move is 71 bytes, the widest of the OPERANDS it
/// reads accounting for the rest — which is what the accounting charges in place of the
/// number the clause computes ([`ParsedUpdate::arithmetic_operand_charge`]), so the boundary
/// stays where it is whichever result the arithmetic carries into.
pub(super) const SET_ARITHMETIC_CLAUSE_CHARGE: i64 = 69;

/// The frame an assignment whose operand is a FUNCTION CALL costs — `if_not_exists` or
/// `list_append`, the two the member defines. The boundary such a clause is observed to
/// move is 76 bytes, what it stores accounting for the rest.
pub(super) const SET_FUNCTION_CLAUSE_CHARGE: i64 = 74;

/// The frame one `ADD` action costs. The boundary such a clause is observed to move is 21
/// bytes, the number it stores accounting for the rest — the same frame a plain assignment
/// carries, which is what an `ADD` is beside the item it leaves.
pub(super) const ADD_CLAUSE_CHARGE: i64 = 19;

/// The frame one `DELETE` action carries: none of its own. The action takes members out of
/// a set the item already holds — it writes no location and asks for no operand the write
/// has to read beyond the members it subtracts — so what moves the boundary beside it is
/// the set the item is left holding, which the stored item already accounts for.
pub(super) const DELETE_CLAUSE_CHARGE: i64 = 0;

/// The frame one `REMOVE` action costs BEFORE the path it names: a removal writes
/// nothing, so what it asks for is the path itself, and the path's own length is added to
/// this — whether or not the item holds anything there.
pub(super) const REMOVE_CLAUSE_CHARGE: i64 = 1;

/// The frame ONE `SET` assignment stated beside another costs, by the shape of its
/// operand: a single path or value, an arithmetic pair, or a call to one of the member's
/// two functions.
pub(super) fn set_clause_charge(value: &SetValue) -> i64 {
    match value {
        SetValue::Sum(..) | SetValue::Difference(..) => SET_ARITHMETIC_CLAUSE_CHARGE,
        SetValue::Single(operand) => match operand {
            SetOperand::IfNotExists(..) | SetOperand::ListAppend(..) => {
                SET_FUNCTION_CLAUSE_CHARGE
            }
            SetOperand::Path(_) | SetOperand::Value(_) => SET_CLAUSE_CHARGE,
        },
    }
}
