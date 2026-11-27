// SPDX-License-Identifier: CC0-1.0

//! WHICH stream of WHICH table a DynamoDB `EventSourceArn` names.
//!
//! An Amazon DynamoDB event source is "the ARN of the stream" (`EventSourceArn`), and such a
//! stream is a stream OF A TABLE: its resource field is that table's own continued with
//! `/stream/<label>`, and the label is what tells one stream of a table from another — "the
//! combination of … table name, StreamLabel is guaranteed to be unique"
//! (`StreamDescription$StreamLabel`). So the value resolves to a PAIR, and only a value naming
//! exactly one table and exactly one label of it resolves to a stream at all: a resource field
//! carrying a further `/` in either half addresses something other than one stream of one
//! table, exactly as one that stops at the table addresses the table rather than any of its
//! streams.
//!
//! The reading itself belongs to the service that OWNS those streams and is read FROM it
//! ([`::service_com_amazonaws_dynamodbstreams_dynamodbstreams_20120810::state::table_stream_resource`]),
//! not restated here: a value this service admits as naming a stream is then exactly one that
//! service can look up, and the two cannot drift. What this module states is that the member's
//! own reading of which values name an event source at all ([`super::names_event_source`]) is
//! held to it — in ONE place, rather than once per question asked about such a value.
//!
//! A reading of the VALUE alone: it resolves what the ARN names and reaches no store, which is
//! why a value it says no to names no stream in any account or Region however long a caller
//! waits.

use ::service_com_amazonaws_dynamodbstreams_dynamodbstreams_20120810::state as streams;

/// The table and the stream label the resource field `resource` of a DynamoDB ARN names, or
/// `None` for a value that names no one stream of one table (see the module docs).
///
/// The halves are returned as they were written, so a caller that resolves either of them
/// against a store compares the very text the request carried.
#[must_use]
pub fn table_stream_named(resource: &str) -> Option<(&str, &str)> {
    streams::table_stream_resource(resource)
}

/// Whether the resource field `resource` of a DynamoDB ARN names ONE stream of ONE table,
/// which is what [`table_stream_named`] resolving says.
#[must_use]
pub fn names_one_table_stream(resource: &str) -> bool {
    table_stream_named(resource).is_some()
}

#[cfg(test)]
mod tests {
    use super::table_stream_named;

    /// One row: the case, the resource field, and the pair it names.
    type Row = (
        &'static str,
        &'static str,
        Option<(&'static str, &'static str)>,
    );

    /// RULE: a DynamoDB stream event source resolves to the pair `table/<name>/stream/<label>`
    /// names, and to nothing at all where either half is missing, empty or carries a further
    /// segment.
    ///
    /// The rows differ along what the rule generalises over: how the resource field is written
    /// — a stream of a table, a label in the form the owning service mints and a label in
    /// another form (the rule is about the shape of the halves, not about which label), a label
    /// carrying a further segment, no label, no table, a table carrying a further segment, the
    /// table itself, another sub-resource of the table, and another resource type entirely.
    /// The reading is the owning service's, so this is what the values an `EventSourceArn` may
    /// carry resolve to through it.
    #[test]
    fn a_dynamodb_stream_arn_resolves_to_one_table_and_one_label() {
        // (case, the resource field, the pair it names)
        let rows: Vec<Row> = vec![
            (
                "a stream of a table, labelled as the owning service mints one",
                "table/orders/stream/2026-10-23T11:46:49.000",
                Some(("orders", "2026-10-23T11:46:49.000")),
            ),
            (
                "a stream of a table, labelled in another form",
                "table/orders/stream/ab12cd34ef56",
                Some(("orders", "ab12cd34ef56")),
            ),
            (
                "a label carrying a further segment",
                "table/orders/stream/label/nonexistent-stream-label",
                None,
            ),
            ("no label at all", "table/orders/stream/", None),
            (
                "no table at all",
                "table//stream/2026-10-23T11:46:49.000",
                None,
            ),
            (
                "a table half carrying a further segment",
                "table/orders/replica/stream/2026-10-23T11:46:49.000",
                None,
            ),
            ("the table itself", "table/orders", None),
            (
                "another sub-resource of the table",
                "table/orders/backup/01700000000000-a1b2c3d4",
                None,
            ),
            ("another resource type", "stream/orders", None),
        ];
        for (case, resource, expected) in rows {
            let named = table_stream_named(resource);
            assert_eq!(
                named, expected,
                "case {case:?}: {resource} names the pair {expected:?}, got {named:?}"
            );
        }
    }
}
