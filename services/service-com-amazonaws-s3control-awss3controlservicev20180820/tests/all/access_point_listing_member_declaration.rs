// SPDX-License-Identifier: CC0-1.0

//! The members a listing entry reports an access point with, pinned against the
//! published model snapshot.
//!
//! `AccessPoint` — the shape `ListAccessPoints` and
//! `ListAccessPointsForDirectoryBuckets` report their entries as — is declared in
//! this crate's own source rather than rendered from the snapshot, because the
//! wire carries a member the snapshot does not declare (`AccessPointType`, which
//! `GetAccessPoint` reports for one access point too). A hand-maintained
//! declaration can fall behind the model, and a refresh of the snapshot is
//! exactly when it would: a member added to `AccessPoint` upstream would simply
//! stop being reported, with the crate still compiling and every other test still
//! passing.
//!
//! So the rule under test is the declaration's own: an entry reports every member
//! the snapshot declares for the shape, in the order the snapshot declares them,
//! and then the one member the wire carries beyond it. The member list is read
//! off the model file rather than copied here, and the entry is written through
//! the same `restXml` codec a response is serialized by.
//!
//! What each member CARRIES for a stored access point, and that a read and a
//! listing agree about it, is driven end to end over HTTP by the sibling suites
//! `access_point_reads_and_refusals` and `access_point_listing_bucket_scope`.

use aws_smithy_schema::codec::FinishSerializer;
use aws_smithy_schema::serde::ShapeSerializer;
use aws_smithy_schema_server::xml::XmlShapeSerializer;
use serde_json::Value;

use service_com_amazonaws_s3control_awss3controlservicev20180820::ops::types::{
    AccessPoint, VpcConfiguration,
};

/// The shape under test, as the snapshot keys it.
const ACCESS_POINT_SHAPE: &str = "com.amazonaws.s3control#AccessPoint";

/// The member the WIRE carries beyond the snapshot's member list, reported last.
/// See `src/ops/types/access_point_wire.rs`.
const UNMODELLED_MEMBER: &str = "AccessPointType";

/// The members the snapshot declares for `AccessPoint`, in declaration order —
/// which is the order the codec writes them in, each member's index being its
/// position in that list.
fn snapshot_members() -> Vec<String> {
    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/smithy-model/com-amazonaws-s3control-awss3controlservicev20180820.json"
    );
    let text = std::fs::read_to_string(path)
        .unwrap_or_else(|error| panic!("the model file reads back: {error}"));
    let parsed: Value =
        serde_json::from_str(&text).unwrap_or_else(|error| panic!("the model parses: {error}"));
    parsed["shapes"][ACCESS_POINT_SHAPE]["members"]
        .as_object()
        .unwrap_or_else(|| panic!("the snapshot declares {ACCESS_POINT_SHAPE} with members"))
        .keys()
        .cloned()
        .collect()
}

/// One access point with EVERY member set, so the document it is written to
/// carries them all: what is under test is the member list and its order, not
/// what a stored access point happens to carry.
fn fully_populated() -> AccessPoint {
    AccessPoint {
        name: "every-member".to_string(),
        network_origin: "VPC".to_string(),
        vpc_configuration: Some(VpcConfiguration {
            vpc_id: "vpc-0123456789abcdef0".to_string(),
        }),
        bucket: "every-member-bucket".to_string(),
        access_point_arn: Some(
            "arn:aws:s3:us-west-2:000000000000:accesspoint/every-member".to_string(),
        ),
        alias: Some("every-member-alias".to_string()),
        bucket_account_id: Some("000000000000".to_string()),
        data_source_id: Some("data-source-id".to_string()),
        data_source_type: Some("data-source-type".to_string()),
        access_point_type: Some("Regular".to_string()),
    }
}

/// The names of the elements one element DIRECTLY contains, in document order: a
/// member of the written structure, where a nested structure's own children are
/// not.
fn member_elements(document: &str) -> Vec<String> {
    let mut names = Vec::new();
    let mut depth = 0usize;
    let mut rest = document;
    while let Some(open) = rest.find('<') {
        let after = &rest[open + 1..];
        let Some(end) = after.find('>') else {
            panic!("an element with no closing angle bracket in {document}")
        };
        let tag = &after[..end];
        if tag.starts_with('/') {
            depth = depth.saturating_sub(1);
        } else if let Some(empty) = tag.strip_suffix('/') {
            // A member written in the empty form is still that member.
            if depth == 1 {
                names.push(empty.trim().to_string());
            }
        } else {
            depth += 1;
            if depth == 2 {
                names.push(tag.to_string());
            }
        }
        rest = &after[end + 1..];
    }
    names
}

#[test]
fn a_listing_entry_carries_the_snapshots_members_in_order_and_the_recorded_one_last() {
    let mut expected = snapshot_members();
    assert!(
        !expected.contains(&UNMODELLED_MEMBER.to_string()),
        "the snapshot now declares {UNMODELLED_MEMBER} itself: drop the local declaration in \
         src/ops/types/access_point_wire.rs and let shared_types.rs render the shape again, \
         got {expected:?}"
    );
    expected.push(UNMODELLED_MEMBER.to_string());

    let mut serializer = XmlShapeSerializer::new(None);
    serializer
        .write_struct(&AccessPoint::SCHEMA, &fully_populated())
        .unwrap_or_else(|error| panic!("the entry serializes: {error}"));
    let document = String::from_utf8(serializer.finish())
        .unwrap_or_else(|error| panic!("the document is UTF-8: {error}"));

    let written = member_elements(&document);
    assert_eq!(
        written, expected,
        "expected an entry to carry the snapshot's members in order and {UNMODELLED_MEMBER} \
         last, got {written:?} in {document}"
    );
}
