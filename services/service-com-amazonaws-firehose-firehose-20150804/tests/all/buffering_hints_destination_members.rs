// SPDX-License-Identifier: CC0-1.0

//! WHICH `BufferingHints` decide a buffer's window: the ones of the destination member the
//! stream was created with, whichever member that is.
//!
//! The rule under test is the one the `buffering_hints` suite states and whose table-driver
//! these rows reuse — a buffer is delivered once the FIRST of the two conditions its
//! destination's own `BufferingHints` state is met, `IntervalInSeconds` after it opened or
//! as soon as it holds `SizeInMBs`, with the documented defaults (300 s, 5 MiB) where the
//! configuration names neither. What these rows vary is the MEMBER carrying those hints:
//! every destination member with an Amazon S3 location that every ingested record passes
//! through buffers by its own hints — the plain `S3DestinationConfiguration`, the extended
//! one, and the `S3Configuration` of an Amazon Redshift destination ("Firehose first
//! delivers data to the S3 bucket and then issues an Amazon Redshift COPY command").
//!
//! So a record put into a destination configured with a minutes-long interval is in NO
//! member's bucket at the reading taken right after that put, however the destination was
//! spelled, and reaches every one of them once the span this emulator holds a buffer for has
//! passed — the hints being hints the service may improve on.

use crate::buffering_hints::{
    assert_window_rule, Due, Member, Put, Row, BIG_RECORD_BYTES, LONG_INTERVAL, SHORT_INTERVAL,
};

/// The bucket every case here delivers into. Its own, so these rows and the sibling suite's
/// read independent destinations.
const BUCKET: &str = "buffering-member-bucket";

/// The cases.
///
/// The rows differ along what the rule generalises over: WHICH member carries the Amazon S3
/// location and its hints, the configured `IntervalInSeconds` (its published minimum 0, a
/// short one, a minutes-long one, its published maximum 900, and the member left out for the
/// documented default of 300), the configured `SizeInMBs` (1, its published maximum 128, and
/// the member left out for the documented default of 5), whether the records put CROSS that
/// size, how many records there are and how big each is, and whether they are put in one
/// batch or one call each.
fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "the plain Amazon S3 member at a minutes-long interval under the largest \
                   SizeInMBs it admits, with one tiny record put on its own",
            member: Member::PlainS3,
            hints: Some((128, LONG_INTERVAL)),
            records: (1, 0),
            put: Put::OneCallEach,
            due: Due::AtTheCeiling,
        },
        Row {
            case: "the plain Amazon S3 member with no BufferingHints at all, so the \
                   documented 300 s default decides",
            member: Member::PlainS3,
            hints: None,
            records: (2, 0),
            put: Put::OneBatch,
            due: Due::AtTheCeiling,
        },
        Row {
            case: "the plain Amazon S3 member at a short interval, with a payload far below \
                   SizeInMBs",
            member: Member::PlainS3,
            hints: Some((128, SHORT_INTERVAL)),
            records: (3, 0),
            put: Put::OneCallEach,
            due: Due::AfterItsInterval,
        },
        Row {
            case: "the plain Amazon S3 member at the shortest interval it admits",
            member: Member::PlainS3,
            hints: Some((1, 0)),
            records: (1, 0),
            put: Put::OneCallEach,
            due: Due::AtOnce,
        },
        Row {
            case: "the plain Amazon S3 member whose records cross SizeInMBs=1 inside the \
                   longest interval it admits",
            member: Member::PlainS3,
            hints: Some((1, 900)),
            records: (2, BIG_RECORD_BYTES),
            put: Put::OneCallEach,
            due: Due::AtOnce,
        },
        Row {
            case: "the extended Amazon S3 member at a minutes-long interval under the \
                   largest SizeInMBs it admits",
            member: Member::ExtendedS3,
            hints: Some((128, LONG_INTERVAL)),
            records: (1, 0),
            put: Put::OneCallEach,
            due: Due::AtTheCeiling,
        },
        Row {
            case: "an Amazon Redshift destination's S3 configuration at a minutes-long \
                   interval under the largest SizeInMBs it admits",
            member: Member::RedshiftS3,
            hints: Some((128, LONG_INTERVAL)),
            records: (1, 0),
            put: Put::OneBatch,
            due: Due::AtTheCeiling,
        },
        Row {
            case: "an Amazon Redshift destination's S3 configuration at a short interval",
            member: Member::RedshiftS3,
            hints: Some((1, SHORT_INTERVAL)),
            records: (2, 0),
            put: Put::OneCallEach,
            due: Due::AfterItsInterval,
        },
        Row {
            case: "an Amazon Redshift destination's S3 configuration whose records cross \
                   SizeInMBs=1 inside the longest interval it admits",
            member: Member::RedshiftS3,
            hints: Some((1, 900)),
            records: (2, BIG_RECORD_BYTES),
            put: Put::OneCallEach,
            due: Due::AtOnce,
        },
    ]
}

#[tokio::test]
async fn every_destination_members_own_buffering_hints_decide_its_window() {
    assert_window_rule(BUCKET, "member-buffering-stream", rows()).await;
}
