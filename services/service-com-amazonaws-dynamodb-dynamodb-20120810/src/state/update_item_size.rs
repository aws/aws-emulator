// SPDX-License-Identifier: CC0-1.0

//! What an UPDATE's resulting item is charged against the documented item-size ceiling.
//!
//! # The rule this module implements
//!
//! A write is refused when what the item it leaves behind is CHARGED crosses the documented
//! item-size quota ([`MAX_ITEM_SIZE_BYTES`], "the maximum item size in DynamoDB is 400 KB").
//! What is charged is the bytes the item stores — "the lengths of its attribute names and
//! values", key attributes among them, exactly as [`item_size`] totals them — plus what the
//! SHAPE of the request stating the change costs ([`UpdateRequestShape`]).
//!
//! An update stated as an EXPRESSION is charged for that shape twice over, because such a
//! request asks for work over and above the item it leaves: a frame for each clause it states
//! ([`ParsedUpdate::request_shape_charge`]), and the part of the item it does NOT address at a
//! floor ([`UNTOUCHED_PORTION_FLOOR_BYTES`]). The attributes such a change ADDRESSES are
//! charged the bytes they hold; everything else the item carries — its partition key, its sort
//! key and every attribute the change leaves alone — is charged together, at its own bytes once
//! they fill that floor and at the floor while they do not. So an item whose untouched part is
//! shorter than the floor buys no headroom from being short, and the ceiling such an update tops
//! out at is the quota less the floor: the boundary does not move from one item to the next
//! because one is keyed by fewer bytes than the other, or carries a few bytes of attribute the
//! change never touches. An untouched part at or above the floor fills it and is charged the
//! bytes it holds, so the ceiling there is the quota itself.
//!
//! A request that states its change WITHOUT an expression — the legacy `AttributeUpdates` map,
//! and a PartiQL `UPDATE` — states no clauses and is charged the item alone, with no floor
//! under any part of it: the item charged exactly the quota is the largest one such a request
//! can leave behind.
//!
//! An item the table holds a LOCAL secondary index entry for is charged for that entry as well,
//! because the published quota counts it: "for each local secondary index on a table, there is a
//! 400 KB limit on the total size of the following: the data in the table item, plus the index key
//! values and projected attributes in the index entry" ([`Table::local_index_entry_charge`]). So an
//! update that grows an item an `ALL`-projected local index covers crosses the quota at half the
//! bytes, and the growth is measured over the item the change LEAVES, not over the bytes the
//! request carried.
//!
//! # Where it applies
//!
//! Every surface that applies a stated change to the stored item states the ceiling through
//! [`update_size_exceeded`]: `UpdateItem`, the `Update` action of `TransactWriteItems` and
//! `ExecuteTransaction`, and a PartiQL `UPDATE` through `ExecuteStatement` or
//! `BatchExecuteStatement` — so the five of them stop at the same item, each charged what the
//! shape of its own request costs. A request that CARRIES a whole item (`PutItem` and its batch
//! and transactional forms) states the item itself rather than a change to one, and is bounded
//! by the quota over the bytes it carries ([`item_size`]) and the local-index entry beside them
//! ([`Table::item_size_exceeded`]), with no shape charge of its own.

use super::*;

/// The least the part of an item an update does NOT address is charged against the item-size
/// quota when the change is stated as an EXPRESSION.
///
/// MEASURED behavior, not a modelled or documented constraint: neither the Smithy model nor the
/// Developer Guide's item-size quota ("Service, Account, and Table Quotas") states a floor, and
/// the recorded refusal boundary is what pins it — an update stated as an expression on a table
/// with a one-byte hash key name and a one-byte key value (two untouched bytes) is refused at a
/// 409,600-byte item and admitted twenty bytes under it, which is this floor less those two
/// bytes, while the same item written by a change stating no expression is admitted at the
/// 409,600 bytes themselves. The same boundary is recorded for an item carrying a further
/// attribute the change never touches: what moves it is the untouched bytes passing this floor,
/// not whether they sit in a key. It is stated here as a byte count of the RULE rather than of
/// any one item, exactly as the per-clause frames beside it are
/// ([`ParsedUpdate::request_shape_charge`]).
pub const UNTOUCHED_PORTION_FLOOR_BYTES: i64 = 22;

/// What a part of an item holding `untouched_bytes` stored bytes is charged on the path that
/// charges a floor: its own bytes once it fills [`UNTOUCHED_PORTION_FLOOR_BYTES`], and the
/// floor while it does not.
pub fn untouched_portion_charge(untouched_bytes: i64) -> i64 {
    untouched_bytes.max(UNTOUCHED_PORTION_FLOOR_BYTES)
}

/// What the SHAPE of the request stating a change costs the item-size accounting beside
/// the bytes the item itself holds.
///
/// An update stated as an EXPRESSION asks for work over and above the item it leaves —
/// it states clauses — and is charged for them: a frame per clause
/// ([`ParsedUpdate::request_shape_charge`]) and the floor above under the part of the item
/// its clauses do not address, which is what makes that path's boundary sit the same number
/// of bytes under the quota for every item whose untouched part is below the floor. A request
/// that states its change WITHOUT an expression — the legacy `AttributeUpdates` map, and a
/// PartiQL `UPDATE` — states no clauses and is charged the item alone, so the item charged
/// exactly the quota is the largest one it can leave behind.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UpdateRequestShape {
    /// A change stated as an `UpdateExpression`, charged the frames of the clauses it
    /// states, with the top-level attributes those clauses ADDRESS charged the bytes they
    /// hold and the rest of the item charged at the floor.
    StatedAsAnExpression {
        /// What the clauses this request states cost beside the item.
        clauses: i64,
        /// The top-level attributes those clauses address, in the item the change leaves.
        addressed: ::std::collections::BTreeSet<String>,
    },
    /// A change stated without an expression, charged nothing of its own.
    StatedWithoutAnExpression,
}

impl UpdateRequestShape {
    /// What this request's clauses cost beside the item.
    fn clause_charge(&self) -> i64 {
        match self {
            Self::StatedAsAnExpression { clauses, .. } => *clauses,
            Self::StatedWithoutAnExpression => 0,
        }
    }

    /// The bytes of the part of `item` this request does NOT address: every attribute its
    /// clauses name nothing of, the key attributes among them — a key is addressed by no
    /// clause, because an update may not write one — each weighed the way every part of an
    /// item is ([`item_size`]).
    ///
    /// A request that states no clause at all addresses nothing, so the whole item is its
    /// untouched part; it is charged no floor under it either
    /// ([`Self::untouched_portion_charge`]), so the two together charge such a request the
    /// item exactly as it stands.
    fn untouched_bytes(&self, item: &Item) -> i64 {
        let addressed = match self {
            Self::StatedAsAnExpression { addressed, .. } => addressed,
            Self::StatedWithoutAnExpression => return item_size(item),
        };
        sum_of(
            item.iter()
                .filter(|(attribute, _)| !addressed.contains(*attribute))
                .map(|(attribute, value)| attribute_size(attribute, value)),
        )
    }

    /// What a part holding `untouched_bytes` stored bytes is charged under this shape: the
    /// floor on the expression path, and the bytes it actually holds where the request states
    /// no clauses at all.
    fn untouched_portion_charge(&self, untouched_bytes: i64) -> i64 {
        match self {
            Self::StatedAsAnExpression { .. } => untouched_portion_charge(untouched_bytes),
            Self::StatedWithoutAnExpression => untouched_bytes,
        }
    }
}

/// What `item` is charged against [`MAX_ITEM_SIZE_BYTES`] on the update path: the bytes of the
/// attributes the change ADDRESSES, plus what the part of the item it leaves alone is charged
/// ([`UpdateRequestShape::untouched_portion_charge`]), plus the local-index entry the same
/// quota counts beside the item ([`Table::local_index_entry_charge`]), plus what the request's
/// own shape costs (nothing, for a change stated without an expression).
///
/// Every part of the item is weighed the way every other part of it is ([`item_size`]), so a
/// composite key is charged across both of its attributes and an untouched attribute beside
/// them in the same total.
///
/// A READ of the table and the item it is handed: nothing is stored and nothing is modified.
pub fn update_charged_size(table: &Table, item: &Item, shape: &UpdateRequestShape) -> i64 {
    let untouched_bytes = shape.untouched_bytes(item);
    item_size(item)
        .saturating_sub(untouched_bytes)
        .saturating_add(shape.untouched_portion_charge(untouched_bytes))
        .saturating_add(table.local_index_entry_charge(item))
        .saturating_add(shape.clause_charge())
}

/// Whether the item an update would leave behind, charged as [`update_charged_size`] charges
/// it, crosses the documented item-size quota — the one question every update surface asks
/// before it writes.
///
/// A READ of the item it is handed: nothing is stored and nothing is modified, so asking
/// twice answers twice the same.
pub fn update_size_exceeded(table: &Table, item: &Item, shape: &UpdateRequestShape) -> bool {
    update_charged_size(table, item, shape) > MAX_ITEM_SIZE_BYTES
}

#[cfg(test)]
mod tests {
    use super::*;

    /// RULE: an untouched portion under the floor is charged the floor, so the headroom an
    /// update has over the attributes it writes is the same for every such item; one at or
    /// above it is charged the bytes it holds.
    #[test]
    fn the_untouched_portion_is_charged_at_its_floor() {
        // Untouched portions below the floor, each of which must be charged the floor itself:
        // the smallest a table can have (a one-byte key name beside a one-byte value), the two
        // sizes whose one-byte difference moved the recorded boundary, and the byte under the
        // floor.
        for untouched_bytes in [2i64, 9, 10, UNTOUCHED_PORTION_FLOOR_BYTES - 1] {
            let charged = untouched_portion_charge(untouched_bytes);
            assert_eq!(
                charged, UNTOUCHED_PORTION_FLOOR_BYTES,
                "case \"{untouched_bytes}-byte untouched portion\": expected the floor to be \
                 charged, got {charged}"
            );
        }
        // The floor itself and portions past it, each charged its own bytes.
        for untouched_bytes in [
            UNTOUCHED_PORTION_FLOOR_BYTES,
            UNTOUCHED_PORTION_FLOOR_BYTES + 1,
            300,
        ] {
            let charged = untouched_portion_charge(untouched_bytes);
            assert_eq!(
                charged, untouched_bytes,
                "case \"{untouched_bytes}-byte untouched portion\": expected its own bytes to be \
                 charged, got {charged}"
            );
        }
    }
}
