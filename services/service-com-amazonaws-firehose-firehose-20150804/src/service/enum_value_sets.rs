// SPDX-License-Identifier: CC0-1.0

//! WHICH of the stated value-set orders this service takes, WHICH of its enumerated types
//! hold their values in an order this checkout's model does not carry, and the evidence
//! for both.
//!
//! A constraint clause about a member of an enumerated type states the values that type
//! admits (`Member must satisfy enum value set: [<values>]`), and a front door is seen
//! listing them in one of three orders: lexicographically, as a set the service publishes,
//! or in the order the hash table the values are held in is walked. All three live in
//! `service_common::validation_message`; this service's refusals show the third
//! ([`::service_common::validation_message::in_hashed_enum_order`]) — the values' own
//! buckets ascending, a shared bucket read from the head each value was pushed onto, so
//! that of two values sharing a bucket the one pushed LATER is stated first.
//!
//! The bucket is computed from a value's own text, so it belongs to the enumerated TYPE:
//! one type is stated alike under every member targeting it — any operation, any depth, any
//! list index — and a type this model gains needs nothing added here. What the derivation
//! cannot read off a value is the order the service's own SET was built in, which decides
//! the listing wherever two of a type's values share one bucket. That order is the order
//! the values were pushed in, which is neither the sorted catalogue the generated schema
//! presents a type's values in nor, for every type, the order the model declares them in;
//! the one type of this service whose listing turns on it is published below
//! ([`BUILT_ENUM_VALUE_SETS`]), as the service's API reference publishes that member's
//! valid values.
//!
//! The evidence for the listing is the two sets this service's refusals are recorded
//! stating: `ProcessorType`'s, which the derivation reproduces from the values alone, and
//! `CompressionFormat`'s, which it reproduces from the published order below.

use ::service_common::validation_message::{
    in_hashed_enum_order, in_stated_enum_order, StatedEnumValueSets,
};
use smithy_server_runtime::Violation;

/// The order this service's refusal states `ProcessorType`'s value set in.
///
/// Held here as the EVIDENCE for the order chosen above, not as a table the renderer
/// reads: the renderer derives the listing from the values, and this is the one listing
/// a refusal of this service is known to state, which that derivation has to reproduce.
#[cfg(test)]
const PROCESSOR_TYPE_VALUE_SET: [&str; 6] = [
    "Decompression",
    "Lambda",
    "RecordDeAggregation",
    "MetadataExtraction",
    "AppendDelimiterToRecord",
    "CloudWatchLogProcessing",
];

/// The order this service's refusal states `CompressionFormat`'s value set in.
///
/// The second piece of evidence for the listing, and the one that pins the order the set
/// below is built in: `GZIP` and `UNCOMPRESSED` share a bucket, so which of the two is
/// stated first is decided by which was pushed later.
#[cfg(test)]
const COMPRESSION_FORMAT_VALUE_SET: [&str; 5] =
    ["ZIP", "HADOOP_SNAPPY", "Snappy", "GZIP", "UNCOMPRESSED"];

/// The order the set of compression formats is built in: the order this service's API
/// reference publishes the valid values of a `CompressionFormat` member in, which is the
/// order the model declares them in as well (`smithy_query(query="shape",
/// shape="CompressionFormat")`: `UNCOMPRESSED`, `GZIP`, `ZIP`, `Snappy`, `HADOOP_SNAPPY`).
///
/// It has to be published here because the values do NOT reach a refusal in that order: the
/// generated schema carries a type's values as a sorted catalogue
/// (`crate::ops::types::shared_types::COMPRESSION_FORMAT_VALUES`), which pushes
/// `UNCOMPRESSED` after `GZIP`. Only the relative order of values that share a bucket is
/// observable in a listing, so the one thing this row decides is that `UNCOMPRESSED` was
/// pushed before `GZIP` — and the stated set that follows from it is the one the service's
/// refusal is recorded stating ([`COMPRESSION_FORMAT_VALUE_SET`]).
const COMPRESSION_FORMAT_BUILT_ORDER: &[&str] =
    &["UNCOMPRESSED", "GZIP", "ZIP", "Snappy", "HADOOP_SNAPPY"];

/// Every enumerated type of this service whose own set is built in an order other than the
/// one this checkout's model declares: the values as the model declares them, and the order
/// the service's set holds them in.
///
/// A row is found by the value set the violation carries, matched as a SET
/// (`service_common::validation_message::stated_enum_value_set`), so one row covers every
/// member of every operation that targets that type, at whatever depth it sits. A type
/// whose model order is the service's needs no row: the listing follows the order the values
/// were pushed in only where two of them share a bucket.
const BUILT_ENUM_VALUE_SETS: &StatedEnumValueSets<'static> = &[(
    crate::ops::types::shared_types::COMPRESSION_FORMAT_VALUES,
    COMPRESSION_FORMAT_BUILT_ORDER,
)];

/// `violation` with an enumerated type's value set stated the way this service states it:
/// the type's own set ([`BUILT_ENUM_VALUE_SETS`]) in place of the order the schema presented
/// it in, listed bucket by bucket of the table holding it
/// ([`::service_common::validation_message::in_hashed_enum_order`]).
///
/// Only the values move: the member path, the subject, the echoed value and the
/// requirement's wording all keep coming from the one place that writes them, and every
/// other violation class comes back untouched.
pub(super) fn as_this_service_states(violation: &Violation) -> Violation {
    in_hashed_enum_order(&in_stated_enum_order(violation, BUILT_ENUM_VALUE_SETS))
}

#[cfg(test)]
mod tests {
    use super::*;
    use smithy_server_runtime::ViolationKind;

    /// One enum-constraint violation carrying `declared` as the values the type admits, as
    /// the framework's walk reports one.
    fn refused(declared: &[&str]) -> Violation {
        Violation {
            path: "extendedS3DestinationConfiguration.compressionFormat".to_string(),
            kind: ViolationKind::EnumValue(
                declared.iter().map(|value| (*value).to_string()).collect(),
            ),
            value: Some("NotAModelledValue".to_string()),
            sensitive: false,
        }
    }

    /// The values a restated violation lists, in the order it lists them.
    fn listed(violation: &Violation) -> Vec<String> {
        match &violation.kind {
            ViolationKind::EnumValue(values) => values.clone(),
            other => panic!("expected an enum value set, got {other:?}"),
        }
    }

    /// The shared, value-derived order reproduces the set this service's refusal states for
    /// a type the model declares in the service's own order — which is neither the model's
    /// declaration order nor a sort of the values, so taking it is a choice this pin holds
    /// the renderer to.
    #[test]
    fn the_hashed_order_reproduces_the_set_this_service_states() {
        // `smithy_query(query="shape", shape="ProcessorType")` declares the values in this
        // order; `crate::ops::types::shared_types::PROCESSOR_TYPE_VALUES` holds the same
        // six sorted, and the assertions below hold the stated listing apart from both.
        let declared = [
            "RecordDeAggregation",
            "Decompression",
            "CloudWatchLogProcessing",
            "Lambda",
            "MetadataExtraction",
            "AppendDelimiterToRecord",
        ];
        let stated = ::service_common::validation_message::hashed_enum_order(&declared);
        assert_eq!(
            stated,
            PROCESSOR_TYPE_VALUE_SET.to_vec(),
            "expected {PROCESSOR_TYPE_VALUE_SET:?}, got {stated:?}"
        );
        assert_ne!(
            stated,
            declared.to_vec(),
            "the stated set must not be the model's declaration order, got {stated:?}"
        );
        let mut sorted = declared.to_vec();
        sorted.sort_unstable();
        assert_ne!(
            stated, sorted,
            "the stated set must not be a sort of the values, got {stated:?}"
        );

        // The listing is the model's membership re-ordered, so it cannot drift from the
        // type the model declares.
        let mut sorted_stated = stated.clone();
        sorted_stated.sort_unstable();
        let modelled: Vec<&str> = {
            let mut values = crate::ops::types::shared_types::PROCESSOR_TYPE_VALUES.to_vec();
            values.sort_unstable();
            values
        };
        assert_eq!(
            sorted_stated, modelled,
            "the stated set must hold exactly the modelled values, got {stated:?}"
        );

        // A type with no row of its own is listed straight from the order the schema
        // presented it in, so the seam states exactly that listing.
        let through_the_seam = listed(&as_this_service_states(&refused(&declared)));
        assert_eq!(
            through_the_seam, stated,
            "a type with no published order must be listed as the derivation lists it, got \
             {through_the_seam:?}"
        );
    }

    /// The order does not depend on the order the values reached the clause in, for a type
    /// whose values fall in distinct buckets: the same six, presented the other way round,
    /// are stated the same way. (A type two of whose values share a bucket is stated from
    /// the order its own set was built in, which is what [`BUILT_ENUM_VALUE_SETS`] carries
    /// for the one such type whose set the model mis-orders.)
    #[test]
    fn the_order_is_presentation_independent_for_a_collision_free_type() {
        let mut reversed = crate::ops::types::shared_types::PROCESSOR_TYPE_VALUES.to_vec();
        reversed.reverse();
        let stated = ::service_common::validation_message::hashed_enum_order(&reversed);
        assert_eq!(
            stated,
            PROCESSOR_TYPE_VALUE_SET.to_vec(),
            "presented as {reversed:?}: expected {PROCESSOR_TYPE_VALUE_SET:?}, got {stated:?}"
        );
    }

    /// A type whose own set is built in an order the values do not reach the clause in is
    /// stated from THAT order: the seam reproduces the set this service's refusal is
    /// recorded stating, whichever order the schema presented the values in.
    #[test]
    fn a_published_set_is_stated_from_the_order_the_service_built_it_in() {
        // (case, the order the values reached the clause in)
        let presented: [(&str, Vec<&str>); 4] = [
            (
                "the order this service states them in",
                COMPRESSION_FORMAT_VALUE_SET.to_vec(),
            ),
            (
                "that order reversed",
                COMPRESSION_FORMAT_VALUE_SET.iter().rev().copied().collect(),
            ),
            (
                "the generated catalogue's sorted order",
                crate::ops::types::shared_types::COMPRESSION_FORMAT_VALUES.to_vec(),
            ),
            (
                // The built order below, which is also the order the model declares (the
                // doc comment on that constant cites the `smithy_query` it comes from), so
                // this one case covers both presentations while they coincide.
                "the order the service's own set is built in",
                COMPRESSION_FORMAT_BUILT_ORDER.to_vec(),
            ),
        ];

        for (case, values) in presented {
            let actual = listed(&as_this_service_states(&refused(&values)));
            assert_eq!(
                actual,
                COMPRESSION_FORMAT_VALUE_SET.to_vec(),
                "case {case:?}: expected {COMPRESSION_FORMAT_VALUE_SET:?}, got {actual:?}"
            );
        }

        // The published order is the modelled membership re-ordered, so a model revision
        // that adds or drops a value is caught here rather than stating a set of its own.
        let mut published = COMPRESSION_FORMAT_BUILT_ORDER.to_vec();
        published.sort_unstable();
        let mut modelled = crate::ops::types::shared_types::COMPRESSION_FORMAT_VALUES.to_vec();
        modelled.sort_unstable();
        assert_eq!(
            published, modelled,
            "the published order must hold exactly the modelled values, got \
             {COMPRESSION_FORMAT_BUILT_ORDER:?}"
        );
    }

    /// Nothing but the value set moves: the member path, the echoed value and every other
    /// violation class come back as the framework reported them.
    #[test]
    fn only_the_value_set_moves() {
        let reported = refused(&COMPRESSION_FORMAT_VALUE_SET);
        let restated = as_this_service_states(&reported);
        assert_eq!(
            (restated.path.as_str(), restated.value.as_deref()),
            (reported.path.as_str(), reported.value.as_deref()),
            "expected the path and the value untouched, got {restated:?}"
        );

        let other = Violation {
            path: "deliveryStreamName".to_string(),
            kind: ViolationKind::MinLength(1),
            value: Some(String::new()),
            sensitive: false,
        };
        assert_eq!(
            as_this_service_states(&other).message(),
            other.message(),
            "a violation that is not an enum value set must be left alone"
        );
    }
}
