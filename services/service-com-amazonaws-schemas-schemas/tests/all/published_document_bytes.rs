// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for the DOCUMENT the two read paths of a published schema
//! serve — `DescribeSchema`'s `Content` and `ExportSchema`'s conversion of it.
//!
//! RULE: a schema document is an opaque TEXT this service serves as the document it
//! holds. The document the catalogue publishes for an event type describes that event's
//! `detail` and nothing around it: its `components.schemas` are the component named
//! after the event plus whatever that component references, with no EventBridge envelope
//! and no `x-amazon-events-*` extension, served in the order the published document
//! writes them (`openapi`, `info` with its `title` before its `version`, `paths`,
//! `components`; a component's `type` before its `properties`). A conversion to another
//! schema language renders the target document's own root order (`$schema`, `title`,
//! then the `definitions` the components become) with every translated schema's keywords
//! and property names in name order, OpenAPI's `nullable` lowered to a draft-04 type
//! union, and none of OpenAPI 3.0's own keywords surviving. What a published document
//! says about the event it describes is the publisher's: for the CloudTrail-delivered
//! family that is the CloudTrail record, whose members, nested structures and formats
//! the cases below hold every member of the family to.
//!
//! Every case drives the real wire path, through the harness in
//! `registry_schema_lifecycle.rs` — nothing here touches the state struct directly.

use serde_json::Value;
use service_com_amazonaws_schemas_schemas::state::{
    aws_managed_event_types, managed_schema_title, JsonNode, ManagedEventType,
    AWS_MANAGED_REGISTRY, CLOUD_TRAIL_EVENT_FAMILY, EVENT_ENVELOPE_COMPONENT,
    JSON_SCHEMA_DRAFT4_URI,
};

use crate::registry_schema_lifecycle::{export_uri, schema_uri, Emulator, OPENAPI_ONLY_KEYWORDS};

/// The members the published document declares for a CloudTrail record, in the order it
/// writes them.
///
/// Source: the document AWS publishes for a CloudTrail-delivered event, as a recorded
/// `DescribeSchema` of `aws.events@AWSAPICallviaCloudTrail` serves it — the Smithy model
/// states nothing about a document's text, so the published document is the reference.
/// The `detail` of every member of [`CLOUD_TRAIL_EVENT_FAMILY`] is the same record
/// whichever source published it, so the rows below hold sources the issue did not cite
/// to the same set.
const PUBLISHED_RECORD_MEMBERS: &[&str] = &[
    "addendum",
    "additionalEventData",
    "annotation",
    "apiVersion",
    "awsRegion",
    "edgeDeviceDetails",
    "errorCode",
    "errorMessage",
    "eventCategory",
    "eventContext",
    "eventID",
    "eventName",
    "eventSource",
    "eventTime",
    "eventType",
    "eventVersion",
    "managementEvent",
    "readOnly",
    "recipientAccountId",
    "requestID",
    "requestParameters",
    "resources",
    "responseElements",
    "sessionCredentialFromConsole",
    "sharedEventID",
    "sourceIPAddress",
    "tlsDetails",
    "userAgent",
    "userIdentity",
];

/// The record members the published document marks `nullable`: its four free-form
/// members, whose contents are the called API's rather than CloudTrail's, which OpenAPI
/// 3.0 spells as "may be present and null". The published document marks nothing else
/// nullable. Source: the same published document.
const NULLABLE_RECORD_MEMBERS: &[&str] = &[
    "addendum",
    "additionalEventData",
    "requestParameters",
    "responseElements",
];

/// The catalogue entries the document cases run over.
///
/// Source: `crate::state::aws_managed_event_types()`. Rows differ along what the rule
/// generalises over: the publishing source, which kind of CloudTrail record the event is,
/// whether the event is a CloudTrail one or the service's own, how many components the
/// document carries and how deeply they nest. `aws.events@AWSAPICallviaCloudTrail` is
/// the entry the issue cited; every other row it did not.
fn document_rows() -> Vec<ManagedEventType> {
    let wanted = [
        "aws.events@AWSAPICallviaCloudTrail",
        "aws.s3@AWSConsoleSignInviaCloudTrail",
        "aws.lambda@AWSServiceEventviaCloudTrail",
        "aws.xray@AWSNetworkActivityEventviaCloudTrail",
        "aws.amazon.connect@AWSInsightviaCloudTrail",
        "aws.ec2@EC2InstanceStateChangeNotification",
        "aws.s3@ObjectCreated",
        "aws.health@AWSHealthEvent",
        "aws.events@ScheduledEvent",
        "aws.a4b@RoomStateChange",
    ];
    let catalogue = aws_managed_event_types();
    wanted
        .iter()
        .map(|name| {
            catalogue
                .iter()
                .find(|event| event.schema_name() == *name)
                .copied()
                .unwrap_or_else(|| panic!("case {name:?}: the catalogue must publish it"))
        })
        .collect()
}

/// The names of an object's members, in the order the document writes them.
fn member_names(node: &JsonNode) -> Vec<String> {
    match node {
        JsonNode::Object(members) => members.iter().map(|(name, _)| name.clone()).collect(),
        _ => Vec::new(),
    }
}

/// The document `answer`'s `Content` carries, read with its member order kept.
fn ordered_document(case: &str, content: &str) -> JsonNode {
    JsonNode::parse(content)
        .unwrap_or_else(|| panic!("case {case:?}: Content must be one JSON document: {content}"))
}

/// The `components.schemas` of a published document, in document order.
fn components(case: &str, document: &JsonNode) -> Vec<(String, JsonNode)> {
    let schemas = document
        .member("components")
        .and_then(|components| components.member("schemas"))
        .unwrap_or_else(|| panic!("case {case:?}: the document must carry components.schemas"));
    match schemas {
        JsonNode::Object(members) => members.clone(),
        other => panic!("case {case:?}: components.schemas must be an object, got {other:?}"),
    }
}

/// The member names the draft-04 export of the published document `source` carries at
/// its ROOT, in the order it writes them: draft-04's `$schema`, the source document's
/// title, then the `definitions` its components become.
///
/// A published document declares no event envelope, so it names no root among its
/// co-equal components and the export promotes none of them — which this also asserts,
/// because a document that DID declare one would be exported with that component's
/// keywords at the root instead.
fn expected_root_members(case: &str, source: &JsonNode) -> Vec<String> {
    let carried = components(case, source);
    assert!(
        carried
            .iter()
            .all(|(component, _)| component != EVENT_ENVELOPE_COMPONENT),
        "case {case:?}: a published document declares no {EVENT_ENVELOPE_COMPONENT} envelope, \
         got {:?}",
        carried
            .iter()
            .map(|(component, _)| component.clone())
            .collect::<Vec<String>>()
    );
    assert!(
        !carried.is_empty(),
        "case {case:?}: a published document defines the component it describes"
    );
    vec![
        "$schema".to_string(),
        "title".to_string(),
        "definitions".to_string(),
    ]
}

/// The first object in `node` whose members are NOT in the order the export renders
/// them — by name, compared without regard to case (the order the published documents
/// themselves write a schema's members in: `accountId` before `ARN`) — named by the path
/// to it. `None` when every object in the tree is ordered that way.
fn first_unordered_object(path: &str, node: &JsonNode) -> Option<String> {
    match node {
        JsonNode::Object(members) => {
            let names: Vec<&str> = members.iter().map(|(name, _)| name.as_str()).collect();
            let mut sorted = names.clone();
            sorted.sort_by_key(|name| (name.to_lowercase(), *name));
            if names != sorted {
                return Some(format!("{path}: {names:?}"));
            }
            members
                .iter()
                .find_map(|(name, member)| first_unordered_object(&format!("{path}.{name}"), member))
        }
        JsonNode::List(items) => items
            .iter()
            .enumerate()
            .find_map(|(at, item)| first_unordered_object(&format!("{path}[{at}]"), item)),
        _ => None,
    }
}

/// RULE: a published document is served as it is published — `openapi`, then `info`
/// (`title` before `version`), then `paths`, then `components`, and a component's `type`
/// before its `properties` — because `Content` is text a client caches, diffs and hashes,
/// not a value whose members may be rearranged.
#[tokio::test]
async fn a_published_document_is_served_in_the_order_it_is_published() {
    let emulator = Emulator::new();

    for event in document_rows() {
        let name = event.schema_name();
        let title = managed_schema_title(event.source, event.detail_type);
        let described = emulator
            .call("GET", &schema_uri(AWS_MANAGED_REGISTRY, &name), None)
            .await;
        assert_eq!(
            described.status, 200,
            "case {name:?}: expected 200, got {} {:?}",
            described.status, described.body
        );
        let content = described.str_at("Content");

        // The document's opening bytes, as the published document writes them: the
        // dialect, the title, the empty `paths`, then the components it describes the
        // event through.
        let opening = format!(
            r#"{{"openapi":"3.0.0","info":{{"title":{},"version":"1.0.0"}},"paths":{{}},"components":{{"schemas":{{"#,
            serde_json::to_string(&title).expect("a title encodes")
        );
        assert!(
            content.starts_with(&opening),
            "case {name:?}: the document must open as published ({opening}), got {content}"
        );

        let document = ordered_document(&name, &content);
        assert_eq!(
            member_names(&document),
            vec!["openapi", "info", "paths", "components"],
            "case {name:?}: the published member order, got {content}"
        );
        assert_eq!(
            document.member("info").map(member_names),
            Some(vec!["title".to_string(), "version".to_string()]),
            "case {name:?}: `info` titles the event before it versions the document, \
             got {content}"
        );

        for (component, schema) in components(&name, &document) {
            let names = member_names(&schema);
            let type_at = names.iter().position(|member| member == "type");
            let properties_at = names.iter().position(|member| member == "properties");
            assert_eq!(
                type_at,
                Some(0),
                "case {name:?}: component {component:?} must declare its `type` first, \
                 got {names:?}"
            );
            if let Some(properties_at) = properties_at {
                assert!(
                    properties_at > 0,
                    "case {name:?}: component {component:?} must declare `type` before \
                     `properties`, got {names:?}"
                );
            }
        }

        // Reading it again answers the same bytes: a document is not re-rendered per
        // read.
        let again = emulator
            .call("GET", &schema_uri(AWS_MANAGED_REGISTRY, &name), None)
            .await;
        assert_eq!(
            again.str_at("Content"),
            content,
            "case {name:?}: two reads must answer one document"
        );
    }
}

/// RULE: a published document describes the event type it is published for — the
/// component named after the event and whatever that component references, and nothing
/// else: no EventBridge envelope and no `x-amazon-events-*` extension, which the
/// published document states nowhere.
#[tokio::test]
async fn a_published_document_describes_the_event_and_nothing_around_it() {
    let emulator = Emulator::new();

    for event in document_rows() {
        let name = event.schema_name();
        let described = emulator
            .call("GET", &schema_uri(AWS_MANAGED_REGISTRY, &name), None)
            .await;
        assert_eq!(
            described.status, 200,
            "case {name:?}: expected 200, got {} {:?}",
            described.status, described.body
        );
        let content = described.str_at("Content");
        let document = ordered_document(&name, &content);

        // The document defines the component named after the event, and every other
        // component it carries is one the document itself references.
        let carried: Vec<String> = components(&name, &document)
            .into_iter()
            .map(|(component, _)| component)
            .collect();
        assert!(
            carried.iter().any(|component| component == event.event_name),
            "case {name:?}: the document defines the component it describes, got {carried:?}"
        );
        for component in carried.iter().filter(|component| *component != event.event_name) {
            assert!(
                content.contains(&format!("#/components/schemas/{component}")),
                "case {name:?}: the extra component {component:?} must be one the document \
                 references, got {content}"
            );
        }
        assert!(
            !carried.iter().any(|component| component == EVENT_ENVELOPE_COMPONENT),
            "case {name:?}: a published document carries no event envelope, got {carried:?}"
        );
        assert!(
            !content.contains("x-amazon-events-"),
            "case {name:?}: a published document declares no EventBridge extension, got {content}"
        );
    }
}

/// RULE: a published document describes the event its publisher publishes — for every
/// member of the CloudTrail-delivered family, the CloudTrail record: its member set, the
/// nested structures it declares inline, the members that may arrive null, and the
/// format and pattern of the instant it carries.
#[tokio::test]
async fn a_cloud_trail_document_describes_the_published_record() {
    let emulator = Emulator::new();

    // Rows: the whole CloudTrail family, published by sources the issue did not cite
    // beside the one it did — the record is the same whichever source and whichever kind
    // of record it is.
    let sources = ["aws.events", "aws.s3", "aws.amazon.connect", "aws.xray"];
    for source in sources {
        for (detail_type, event_name) in CLOUD_TRAIL_EVENT_FAMILY {
            let name = format!("{source}@{event_name}");
            let described = emulator
                .call("GET", &schema_uri(AWS_MANAGED_REGISTRY, &name), None)
                .await;
            assert_eq!(
                described.status, 200,
                "case {name:?}: expected 200, got {} {:?}",
                described.status, described.body
            );
            let content = described.str_at("Content");
            let document: Value = serde_json::from_str(&content)
                .unwrap_or_else(|e| panic!("case {name:?}: Content must be JSON: {e}"));
            let record = &document["components"]["schemas"][*event_name];
            // Every member SET below is read with the document's own order kept (a
            // `serde_json` map holds members by name or by insertion depending on the
            // build's features, so it could not pin an order), and asserted in the order
            // the published document writes them.
            let ordered = ordered_document(&name, &content);
            let ordered_record = ordered
                .member("components")
                .and_then(|components| components.member("schemas"))
                .and_then(|schemas| schemas.member(event_name))
                .unwrap_or_else(|| panic!("case {name:?}: the document declares its event"));

            let declared = ordered_record
                .member("properties")
                .map(member_names)
                .unwrap_or_else(|| panic!("case {name:?}: the record declares properties"));
            assert_eq!(
                declared, PUBLISHED_RECORD_MEMBERS,
                "case {name:?}: the published record member set, got {declared:?}"
            );

            // The record's nested structures are declared inline, so the document
            // carries the event's component and no other.
            let carried: Vec<String> = components(&name, &ordered)
                .into_iter()
                .map(|(component, _)| component)
                .collect();
            assert_eq!(
                carried,
                vec![(*event_name).to_string()],
                "case {name:?}: the published document's components, got {carried:?}"
            );
            assert_eq!(
                document["info"]["title"],
                serde_json::json!(format!("{detail_type} published by {source}")),
                "case {name:?}: the document titles the event it describes, got {content}"
            );

            for member in NULLABLE_RECORD_MEMBERS {
                assert_eq!(
                    record["properties"][*member]["nullable"],
                    serde_json::json!(true),
                    "case {name:?}: {member} may arrive null, got {:?}",
                    record["properties"][*member]
                );
            }
            for member in PUBLISHED_RECORD_MEMBERS {
                let declared = &record["properties"][*member];
                let nullable = NULLABLE_RECORD_MEMBERS.contains(member);
                assert_eq!(
                    declared["nullable"] == serde_json::json!(true),
                    nullable,
                    "case {name:?}: {member} is {}nullable in the published document, \
                     got {declared:?}",
                    if nullable { "" } else { "not " }
                );
            }

            // The record's instant: the type, the format and the pattern CloudTrail
            // writes it in, which is what a generated binding parses it with.
            let event_time = &record["properties"]["eventTime"];
            assert_eq!(
                event_time["type"],
                serde_json::json!("string"),
                "case {name:?}: eventTime is a string, got {event_time:?}"
            );
            assert_eq!(
                event_time["format"],
                serde_json::json!("date-time"),
                "case {name:?}: eventTime carries the date-time format, got {event_time:?}"
            );
            assert_eq!(
                event_time["pattern"],
                serde_json::json!("yyyy-MM-dd'T'HH:mm:ss'Z'"),
                "case {name:?}: eventTime carries the record's pattern, got {event_time:?}"
            );

            // A resource of `resources`, the TLS details and the identity are declared
            // where the record carries them.
            let resource = &record["properties"]["resources"]["items"];
            let resource_members = ordered_record
                .member("properties")
                .and_then(|properties| properties.member("resources"))
                .and_then(|resources| resources.member("items"))
                .and_then(|items| items.member("properties"))
                .map(member_names)
                .unwrap_or_else(|| panic!("case {name:?}: a resource declares properties"));
            assert_eq!(
                resource_members,
                vec!["accountId", "ARN", "ARNPrefix", "type"],
                "case {name:?}: a resource of the record, got {resource:?}"
            );
            let tls_members = ordered_record
                .member("properties")
                .and_then(|properties| properties.member("tlsDetails"))
                .and_then(|tls| tls.member("properties"))
                .map(member_names)
                .unwrap_or_else(|| panic!("case {name:?}: tlsDetails declares properties"));
            assert_eq!(
                tls_members,
                vec![
                    "cipherSuite",
                    "clientProvidedHostHeader",
                    "keyExchange",
                    "tlsVersion"
                ],
                "case {name:?}: the record's TLS details, got {tls_members:?}"
            );
            let identity_members = ordered_record
                .member("properties")
                .and_then(|properties| properties.member("userIdentity"))
                .and_then(|identity| identity.member("properties"))
                .map(member_names)
                .unwrap_or_else(|| panic!("case {name:?}: userIdentity declares properties"));
            assert_eq!(
                identity_members,
                vec![
                    "accessKeyId",
                    "accountId",
                    "arn",
                    "credentialId",
                    "identityProvider",
                    "inScopeOf",
                    "invokedBy",
                    "invokedByDelegate",
                    "onBehalfOf",
                    "principalId",
                    "sessionContext",
                    "type",
                    "userName"
                ],
                "case {name:?}: the record's identity, got {identity_members:?}"
            );
            let on_behalf_of = ordered_record
                .member("properties")
                .and_then(|properties| properties.member("userIdentity"))
                .and_then(|identity| identity.member("properties"))
                .and_then(|members| members.member("onBehalfOf"))
                .and_then(|on_behalf_of| on_behalf_of.member("properties"))
                .map(member_names)
                .unwrap_or_else(|| panic!("case {name:?}: onBehalfOf declares properties"));
            assert_eq!(
                on_behalf_of,
                vec!["identityStoreArn", "userId"],
                "case {name:?}: the identity the record acted on behalf of, got {on_behalf_of:?}"
            );
        }
    }
}

/// RULE: the draft-04 export is the published rendering of the same document — draft-04
/// named first, then the source document's title, then the definitions its components
/// become, with every translated schema's keywords and property names in name order,
/// OpenAPI's `nullable` lowered to a type union, every component ref following the
/// component into `definitions`, and no keyword of OpenAPI 3.0's own vocabulary
/// surviving.
#[tokio::test]
async fn the_draft_4_export_is_rendered_as_it_is_published() {
    let emulator = Emulator::new();

    for event in document_rows() {
        let name = event.schema_name();
        let title = managed_schema_title(event.source, event.detail_type);
        let exported = emulator
            .call(
                "GET",
                &export_uri(AWS_MANAGED_REGISTRY, &name, "JSONSchemaDraft4"),
                None,
            )
            .await;
        assert_eq!(
            exported.status, 200,
            "case {name:?}: expected 200, got {} {:?}",
            exported.status, exported.body
        );
        let content = exported.str_at("Content");

        // The source document's components are the definitions, and nothing of
        // OpenAPI's own vocabulary survives.
        let described = emulator
            .call("GET", &schema_uri(AWS_MANAGED_REGISTRY, &name), None)
            .await;
        let source = ordered_document(&name, &described.str_at("Content"));

        // The document's opening bytes: the dialect, the title, then the definitions.
        let root_members = expected_root_members(&name, &source);
        let opening = format!(
            r#"{{"$schema":{},"title":{},"definitions":{{"#,
            serde_json::to_string(JSON_SCHEMA_DRAFT4_URI).expect("the dialect encodes"),
            serde_json::to_string(&title).expect("a title encodes")
        );
        assert!(
            content.starts_with(&opening),
            "case {name:?}: the export must open as published ({opening}), got {content}"
        );

        let converted = ordered_document(&name, &content);
        assert_eq!(
            member_names(&converted),
            root_members,
            "case {name:?}: the published root order, got {content}"
        );
        let definitions = converted
            .member("definitions")
            .unwrap_or_else(|| panic!("case {name:?}: the export carries definitions"));
        if let Some(unordered) = first_unordered_object("definitions", definitions) {
            panic!(
                "case {name:?}: the export renders every schema in name order, \
                 but {unordered} is not"
            );
        }

        let expected: Vec<String> = components(&name, &source)
            .into_iter()
            .map(|(component, _)| component)
            .collect();
        let mut expected_in_name_order: Vec<String> = expected.clone();
        expected_in_name_order.sort_by_key(|component| component.to_lowercase());
        assert_eq!(
            member_names(definitions),
            expected_in_name_order,
            "case {name:?}: every component becomes a definition, got {content}"
        );
        assert!(
            !content.contains("#/components/schemas/"),
            "case {name:?}: no OpenAPI component ref may survive, got {content}"
        );
        for keyword in OPENAPI_ONLY_KEYWORDS {
            assert!(
                !content.contains(&format!("\"{keyword}\"")),
                "case {name:?}: draft-04 defines no {keyword:?} keyword, got {content}"
            );
        }

        // A nullable member of the source is a union with "null" in the export, one the
        // source did not mark keeps its single type, and a member named for an
        // OpenAPI-only keyword is not in the export at all.
        let source_value: Value =
            serde_json::from_str(&described.str_at("Content")).expect("the source is JSON");
        let converted_value: Value = serde_json::from_str(&content).expect("the export is JSON");
        for component in expected {
            let Some(properties) =
                source_value["components"]["schemas"][&component]["properties"].as_object()
            else {
                continue;
            };
            let rendered = &converted_value["definitions"][&component];
            for (member, declared) in properties {
                let exported_member = &rendered["properties"][member];
                if OPENAPI_ONLY_KEYWORDS.contains(&member.as_str()) {
                    assert!(
                        exported_member.is_null(),
                        "case {name:?}: {component}.{member} is named for an OpenAPI-only \
                         keyword and does not survive, got {exported_member:?}"
                    );
                    continue;
                }
                let Some(declared_type) = declared["type"].as_str() else {
                    continue;
                };
                let expected_type = if declared["nullable"] == serde_json::json!(true) {
                    serde_json::json!([declared_type, "null"])
                } else {
                    serde_json::json!(declared_type)
                };
                assert_eq!(
                    exported_member["type"], expected_type,
                    "case {name:?}: {component}.{member} must export as {expected_type:?}, \
                     got {exported_member:?}"
                );
            }
        }

        // The language the document is already stored in is no export target, so asking
        // for it is refused on the request; the stored document itself is what the
        // reader answers, byte for byte.
        let same = emulator
            .call(
                "GET",
                &export_uri(AWS_MANAGED_REGISTRY, &name, "OpenApi3"),
                None,
            )
            .await;
        assert_eq!(
            same.status, 400,
            "case {name:?}: the stored language is no export target, got {} {:?}",
            same.status, same.body
        );
    }
}

/// RULE: the export orders a schema's member names WITHOUT regard to case. What pins
/// the rule is the ORDER ITSELF, not a list written here: the expectation below is the
/// stored document's own member names re-ordered case-insensitively, and the case asserts
/// that this order is NOT the one a case-sensitive comparison produces — so a conversion
/// that compared names by their bytes would fail it. (The published document's own order,
/// `accountId` before `ARN` before `ARNPrefix`, is what says the comparison ignores case:
/// every upper-case letter sorts before every lower-case one.)
#[tokio::test]
async fn the_export_orders_member_names_without_regard_to_case() {
    let emulator = Emulator::new();

    // Rows: two publishing sources and two kinds of CloudTrail record, whose shared
    // record carries the one mixed-case member set — a resource of `resources`.
    for name in [
        "aws.events@AWSAPICallviaCloudTrail",
        "aws.s3@AWSServiceEventviaCloudTrail",
    ] {
        let described = emulator
            .call("GET", &schema_uri(AWS_MANAGED_REGISTRY, name), None)
            .await;
        let document = ordered_document(name, &described.str_at("Content"));
        let event_name = name.split_once('@').map(|(_, event)| event).unwrap_or("");
        let resource_of = |schemas: &JsonNode, record: &str| {
            schemas
                .member(record)
                .and_then(|record| record.member("properties"))
                .and_then(|properties| properties.member("resources"))
                .and_then(|resources| resources.member("items"))
                .and_then(|items| items.member("properties"))
                .map(member_names)
                .unwrap_or_else(|| panic!("case {name:?}: a resource declares properties"))
        };
        let schemas = document
            .member("components")
            .and_then(|components| components.member("schemas"))
            .unwrap_or_else(|| panic!("case {name:?}: the document carries components"));
        let stored = resource_of(schemas, event_name);

        // What the export must write: the stored names, ordered without regard to case.
        let mut expected = stored.clone();
        expected.sort_by_key(|member| (member.to_lowercase(), member.clone()));
        // And what a case-sensitive comparison would write instead — a different order,
        // so this case cannot pass under one.
        let mut by_bytes = stored.clone();
        by_bytes.sort();
        assert_ne!(
            expected, by_bytes,
            "case {name:?}: this member set must distinguish the two orders, got {stored:?}"
        );

        let exported = emulator
            .call(
                "GET",
                &export_uri(AWS_MANAGED_REGISTRY, name, "JSONSchemaDraft4"),
                None,
            )
            .await;
        let converted = ordered_document(name, &exported.str_at("Content"));
        let definitions = converted
            .member("definitions")
            .unwrap_or_else(|| panic!("case {name:?}: the export carries definitions"));
        let rendered = resource_of(definitions, event_name);
        assert_eq!(
            rendered, expected,
            "case {name:?}: the export orders member names without regard to case, \
             got {rendered:?}"
        );
    }
}

/// The members the published document declares for the session a temporary credential
/// belongs to (`userIdentity.sessionContext`), in the order it writes them, each with the
/// type it declares.
///
/// Source: the session block of a CloudTrail record's identity (CloudTrail User Guide,
/// "CloudTrail userIdentity element"), which the published document declares inline like
/// the record's other nested structures — a recorded `ExportSchema` of a
/// CloudTrail-delivered catalogue entry renders its first member as
/// `"assumedRoot":{"type":"string"}`.
const PUBLISHED_SESSION_MEMBERS: &[(&str, &str)] = &[
    ("assumedRoot", "string"),
    ("attributes", "object"),
    ("ec2RoleDelivery", "string"),
    ("sessionIssuer", "object"),
    ("sourceIdentity", "string"),
    ("webIdFederationData", "object"),
];

/// The members the published document declares for each structure nested in that session
/// block, in the order it writes them. Source: the same identity element — the session's
/// own attributes, the entity that issued it and the data of a web-identity federation.
const PUBLISHED_SESSION_STRUCTURES: &[(&str, &[&str])] = &[
    ("attributes", &["creationDate", "mfaAuthenticated"]),
    (
        "sessionIssuer",
        &["accountId", "arn", "principalId", "type", "userName"],
    ),
    ("webIdFederationData", &["attributes", "federatedProvider"]),
];

/// The node `path` names in `node`, or a panic naming the case and the path.
fn at<'a>(case: &str, node: &'a JsonNode, path: &[&str]) -> &'a JsonNode {
    let mut walked = node;
    for step in path {
        walked = walked.member(step).unwrap_or_else(|| {
            panic!(
                "case {case:?}: the document must declare {} (at {step:?}), got {:?}",
                path.join("."),
                member_names(walked)
            )
        });
    }
    walked
}

/// The `member_names` of `node`, as owned strings — the expectation an assertion compares
/// a declared member list against.
fn owned(members: &[&str]) -> Vec<String> {
    members.iter().map(|member| (*member).to_string()).collect()
}

/// `members` in the order the draft-04 export renders them: by name, without regard to
/// case.
fn in_name_order(members: &[&str]) -> Vec<String> {
    let mut ordered = owned(members);
    ordered.sort_by_key(|member| (member.to_lowercase(), member.clone()));
    ordered
}

/// RULE: a nested structure of the record a published document describes is declared
/// INLINE with its own members — which holds for the session a temporary credential was
/// issued for exactly as it does for the record's other nested structures, so a client
/// that generates a binding or validates a delivered event from either read path
/// constrains the session's members too, and only a structure whose members the published
/// document states nowhere (the attributes an external identity provider supplies) is an
/// object and no further.
#[tokio::test]
async fn a_cloud_trail_document_declares_the_session_a_credential_belongs_to() {
    let emulator = Emulator::new();

    // Rows: the whole CloudTrail-delivered family, published by four sources — three of
    // which no issue cited. The `detail` of every one of them is the same record, so the
    // session block is declared identically whichever source published it and whichever
    // kind of record it is.
    let sources = ["aws.events", "aws.s3", "aws.amazon.connect", "aws.xray"];
    for source in sources {
        for (_, event_name) in CLOUD_TRAIL_EVENT_FAMILY {
            let name = format!("{source}@{event_name}");

            // The describe path: the document the catalogue holds.
            let described = emulator
                .call("GET", &schema_uri(AWS_MANAGED_REGISTRY, &name), None)
                .await;
            assert_eq!(
                described.status, 200,
                "case {name:?}: expected 200, got {} {:?}",
                described.status, described.body
            );
            let content = described.str_at("Content");
            let document = ordered_document(&name, &content);
            let session = at(
                &name,
                &document,
                &[
                    "components",
                    "schemas",
                    event_name,
                    "properties",
                    "userIdentity",
                    "properties",
                    "sessionContext",
                ],
            );
            assert_eq!(
                member_names(session),
                vec!["type", "properties"],
                "case {name:?}: the session block declares its type before its members, \
                 got {content}"
            );
            assert_eq!(
                session.member("type"),
                Some(&JsonNode::text("object")),
                "case {name:?}: the session block is an object, got {session:?}"
            );
            let declared = member_names(at(&name, session, &["properties"]));
            let expected: Vec<String> = PUBLISHED_SESSION_MEMBERS
                .iter()
                .map(|(member, _)| (*member).to_string())
                .collect();
            assert_eq!(
                declared, expected,
                "case {name:?}: the published session members, got {declared:?}"
            );
            for (member, declared_type) in PUBLISHED_SESSION_MEMBERS {
                let node = at(&name, session, &["properties", member]);
                assert_eq!(
                    node.member("type"),
                    Some(&JsonNode::text(declared_type)),
                    "case {name:?}: sessionContext.{member} is declared {declared_type:?}, \
                     got {node:?}"
                );
            }
            for (structure, members) in PUBLISHED_SESSION_STRUCTURES {
                let nested = at(&name, session, &["properties", structure, "properties"]);
                assert_eq!(
                    member_names(nested),
                    owned(members),
                    "case {name:?}: sessionContext.{structure} declares the record's members, \
                     got {:?}",
                    member_names(nested)
                );
            }
            // The attributes an external identity provider supplies are the provider's
            // own, so the published document constrains them to be an object and no
            // further — the one structure of the session block declared without members.
            let provider_attributes = at(
                &name,
                session,
                &["properties", "webIdFederationData", "properties", "attributes"],
            );
            assert_eq!(
                member_names(provider_attributes),
                vec!["type"],
                "case {name:?}: the federation attributes are an object and no further, \
                 got {provider_attributes:?}"
            );

            // The export path: the draft-04 rendering of that same document, which keeps
            // every declared member and renders each schema's names in name order.
            let exported = emulator
                .call(
                    "GET",
                    &export_uri(AWS_MANAGED_REGISTRY, &name, "JSONSchemaDraft4"),
                    None,
                )
                .await;
            assert_eq!(
                exported.status, 200,
                "case {name:?}: expected 200 from the export, got {} {:?}",
                exported.status, exported.body
            );
            let export_content = exported.str_at("Content");
            let converted = ordered_document(&name, &export_content);
            let exported_session = at(
                &name,
                &converted,
                &[
                    "definitions",
                    event_name,
                    "properties",
                    "userIdentity",
                    "properties",
                    "sessionContext",
                ],
            );
            assert_eq!(
                member_names(exported_session),
                vec!["properties", "type"],
                "case {name:?}: the export renders the session block in name order, \
                 got {export_content}"
            );
            let session_members: Vec<&str> = PUBLISHED_SESSION_MEMBERS
                .iter()
                .map(|(member, _)| *member)
                .collect();
            let exported_members = member_names(at(&name, exported_session, &["properties"]));
            assert_eq!(
                exported_members,
                in_name_order(&session_members),
                "case {name:?}: the export carries every session member, got {exported_members:?}"
            );
            for (member, declared_type) in PUBLISHED_SESSION_MEMBERS {
                let node = at(&name, exported_session, &["properties", member]);
                assert_eq!(
                    node.member("type"),
                    Some(&JsonNode::text(declared_type)),
                    "case {name:?}: the export declares sessionContext.{member} \
                     {declared_type:?}, got {node:?}"
                );
            }
            for (structure, members) in PUBLISHED_SESSION_STRUCTURES {
                let nested = at(
                    &name,
                    exported_session,
                    &["properties", structure, "properties"],
                );
                assert_eq!(
                    member_names(nested),
                    in_name_order(members),
                    "case {name:?}: the export carries sessionContext.{structure}'s members, \
                     got {:?}",
                    member_names(nested)
                );
            }
        }
    }
}

/// RULE (an invariant over the whole catalogue): every entry the catalogue publishes is
/// served as a published document and exports to a published draft-04 rendering — not
/// only the entries a case names.
#[tokio::test]
async fn every_catalogue_entry_is_served_and_exported_as_published() {
    let emulator = Emulator::new();
    let catalogue = aws_managed_event_types();

    // A bounded, deterministic sample of the whole catalogue: every 131st entry, so the
    // rows span the sources, the record kinds and the service-specific event types
    // without replaying thousands of HTTP calls. The stride is coprime with the family
    // size (6), so the sample does not land on one kind of record.
    let sampled: Vec<&ManagedEventType> = catalogue.iter().step_by(131).collect();
    assert!(
        sampled.len() > 5,
        "the sample must span the catalogue, got {} of {}",
        sampled.len(),
        catalogue.len()
    );

    for event in sampled {
        let name = event.schema_name();
        let title = managed_schema_title(event.source, event.detail_type);
        let described = emulator
            .call("GET", &schema_uri(AWS_MANAGED_REGISTRY, &name), None)
            .await;
        assert_eq!(
            described.status, 200,
            "case {name:?}: expected 200, got {} {:?}",
            described.status, described.body
        );
        let content = described.str_at("Content");
        let document = ordered_document(&name, &content);
        assert_eq!(
            member_names(&document),
            vec!["openapi", "info", "paths", "components"],
            "case {name:?}: the published member order, got {content}"
        );
        assert_eq!(
            document
                .member("info")
                .and_then(|info| info.member("title")),
            Some(&JsonNode::Text(title.clone())),
            "case {name:?}: the document titles its event, got {content}"
        );

        let exported = emulator
            .call(
                "GET",
                &export_uri(AWS_MANAGED_REGISTRY, &name, "JSONSchemaDraft4"),
                None,
            )
            .await;
        assert_eq!(
            exported.status, 200,
            "case {name:?}: expected 200 from the export, got {} {:?}",
            exported.status, exported.body
        );
        let converted = ordered_document(&name, &exported.str_at("Content"));
        assert_eq!(
            member_names(&converted),
            expected_root_members(&name, &document),
            "case {name:?}: the published root order, got {:?}",
            exported.str_at("Content")
        );
    }
}
