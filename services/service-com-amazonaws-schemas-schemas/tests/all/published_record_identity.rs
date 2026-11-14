// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for WHO a published CloudTrail document says made the call: the
//! `userIdentity` the record of a CloudTrail-delivered event carries.
//!
//! RULE: the document a catalogue entry publishes declares its event's record WHOLE — a
//! structure the record carries is declared with every member that record states for it,
//! so a client that generates a binding or feeds a validator from the document knows every
//! member of a delivered event. For the CloudTrail-delivered family the identity element
//! of a record states, beside the credential and the session facts, WHAT KIND of principal
//! made the call and the friendly NAME of a principal that has one of its own; both are
//! members of the record and so both are declared. The draft-04 export of the same
//! document carries each of them, rendered in name order like every other schema it
//! translates.
//!
//! Every case drives the real wire path, through the harness in
//! `registry_schema_lifecycle.rs` — nothing here touches the state struct directly.

use service_com_amazonaws_schemas_schemas::state::{
    JsonNode, AWS_MANAGED_REGISTRY, CLOUD_TRAIL_EVENT_FAMILY,
};

use crate::registry_schema_lifecycle::{export_uri, schema_uri, Emulator};

/// Every member the published document declares for a record's identity element, in the
/// order the document writes them, each with the type it is declared as.
///
/// Source: the identity element of a CloudTrail record (CloudTrail User Guide, "CloudTrail
/// userIdentity element") — the credential it was signed with (`accessKeyId`,
/// `credentialId`), whose it is (`accountId`, `arn`, `principalId`), who or what stood in
/// for it (`identityProvider`, `inScopeOf`, `invokedBy`, `invokedByDelegate`,
/// `onBehalfOf`), the session a temporary credential belongs to (`sessionContext`), the
/// KIND of principal it is (`type`) and the friendly NAME of a principal that has one
/// (`userName`). A record this emulator itself writes carries that kind and that name
/// (the shared CloudTrail record writer in `services/common`), so a document that did not
/// declare them would not describe the events the emulator delivers.
const PUBLISHED_IDENTITY_MEMBERS: &[(&str, &str)] = &[
    ("accessKeyId", "string"),
    ("accountId", "string"),
    ("arn", "string"),
    ("credentialId", "string"),
    ("identityProvider", "string"),
    ("inScopeOf", "object"),
    ("invokedBy", "string"),
    ("invokedByDelegate", "object"),
    ("onBehalfOf", "object"),
    ("principalId", "string"),
    ("sessionContext", "object"),
    ("type", "string"),
    ("userName", "string"),
];

/// The sources the cases run the rule over: the one the issue cited and three it did not.
/// The `detail` of a CloudTrail-delivered event is the same record whichever source
/// published it, so each row must declare the same identity.
const PUBLISHING_SOURCES: &[&str] = &["aws.events", "aws.s3", "aws.amazon.connect", "aws.xray"];

/// The names of an object's members, in the order the document writes them.
fn member_names(node: &JsonNode) -> Vec<String> {
    match node {
        JsonNode::Object(members) => members.iter().map(|(name, _)| name.clone()).collect(),
        _ => Vec::new(),
    }
}

/// `members` in the order the draft-04 export renders them: by name, without regard to
/// case.
fn in_name_order(members: &[&str]) -> Vec<String> {
    let mut ordered: Vec<String> = members.iter().map(|member| (*member).to_string()).collect();
    ordered.sort_by_key(|member| (member.to_lowercase(), member.clone()));
    ordered
}

/// The node `path` names in `node`, or a panic naming the case and the path.
fn at<'a>(case: &str, node: &'a JsonNode, path: &[&str]) -> &'a JsonNode {
    let mut walked = node;
    for step in path {
        walked = walked.member(step).unwrap_or_else(|| {
            panic!("case {case:?}: the document declares {path:?}, missing at {step:?}: {node:?}")
        });
    }
    walked
}

/// The document `Content` carries, read with its member order kept.
fn ordered_document(case: &str, content: &str) -> JsonNode {
    JsonNode::parse(content)
        .unwrap_or_else(|| panic!("case {case:?}: Content must be one JSON document: {content}"))
}

/// The identity member names a case expects, in the published order.
fn expected_identity_members() -> Vec<String> {
    PUBLISHED_IDENTITY_MEMBERS
        .iter()
        .map(|(member, _)| (*member).to_string())
        .collect()
}

/// RULE (the describe path): a published CloudTrail document declares the record's
/// identity element whole — every member the record states for it, in the order the
/// document writes them, each with the type the record carries.
#[tokio::test]
async fn a_published_record_declares_its_identity_whole() {
    let emulator = Emulator::new();

    // Rows: every member of the CloudTrail-delivered family (which kind of record the
    // event is) published by four different sources — the identity is the same in each.
    for source in PUBLISHING_SOURCES {
        for (_, event_name) in CLOUD_TRAIL_EVENT_FAMILY {
            let case = format!("{source}@{event_name}");
            let described = emulator
                .call("GET", &schema_uri(AWS_MANAGED_REGISTRY, &case), None)
                .await;
            assert_eq!(
                described.status, 200,
                "case {case:?}: expected 200, got {} {:?}",
                described.status, described.body
            );
            let content = described.str_at("Content");
            let document = ordered_document(&case, &content);
            let identity = at(
                &case,
                &document,
                &[
                    "components",
                    "schemas",
                    event_name,
                    "properties",
                    "userIdentity",
                ],
            );
            assert_eq!(
                member_names(identity),
                vec!["type", "properties"],
                "case {case:?}: the identity declares its type before its members, got {content}"
            );
            let declared = member_names(at(&case, identity, &["properties"]));
            assert_eq!(
                declared,
                expected_identity_members(),
                "case {case:?}: the published identity members, got {declared:?}"
            );
            for (member, declared_type) in PUBLISHED_IDENTITY_MEMBERS {
                let node = at(&case, identity, &["properties", member]);
                assert_eq!(
                    node.member("type"),
                    Some(&JsonNode::text(declared_type)),
                    "case {case:?}: userIdentity.{member} is declared {declared_type:?}, \
                     got {node:?}"
                );
            }
        }
    }
}

/// RULE (the export path): the draft-04 rendering of that document carries every member
/// of the identity the source document declares, rendered in name order.
#[tokio::test]
async fn the_draft4_export_carries_every_identity_member() {
    let emulator = Emulator::new();

    let members: Vec<&str> = PUBLISHED_IDENTITY_MEMBERS
        .iter()
        .map(|(member, _)| *member)
        .collect();
    for source in PUBLISHING_SOURCES {
        for (_, event_name) in CLOUD_TRAIL_EVENT_FAMILY {
            let case = format!("{source}@{event_name}");
            let exported = emulator
                .call(
                    "GET",
                    &export_uri(AWS_MANAGED_REGISTRY, &case, "JSONSchemaDraft4"),
                    None,
                )
                .await;
            assert_eq!(
                exported.status, 200,
                "case {case:?}: expected 200 from the export, got {} {:?}",
                exported.status, exported.body
            );
            let content = exported.str_at("Content");
            let converted = ordered_document(&case, &content);
            let identity = at(
                &case,
                &converted,
                &[
                    "definitions",
                    event_name,
                    "properties",
                    "userIdentity",
                    "properties",
                ],
            );
            let declared = member_names(identity);
            assert_eq!(
                declared,
                in_name_order(&members),
                "case {case:?}: the export carries every identity member, got {declared:?}"
            );
            for (member, declared_type) in PUBLISHED_IDENTITY_MEMBERS {
                let node = at(&case, identity, &[member]);
                assert_eq!(
                    node.member("type"),
                    Some(&JsonNode::text(declared_type)),
                    "case {case:?}: the export declares userIdentity.{member} \
                     {declared_type:?}, got {node:?}"
                );
            }
        }
    }
}
