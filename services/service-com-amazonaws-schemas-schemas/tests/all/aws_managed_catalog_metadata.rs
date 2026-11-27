// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for WHAT the AWS-managed `aws.events` catalogue contains and
//! what it reports about each member:
//!
//! 1. a source the catalogue publishes CloudTrail-delivered schemas for publishes the
//!    whole family, not the API-call event alone, and every member of it is named
//!    after its `detail-type`;
//! 2. a published schema's `LastModified` is the SCHEMA's own instant — fixed, at
//!    whole-second precision and the same in every store — not a reading of the clock;
//! 3. a published schema's `VersionCount` is the number of versions the catalogue
//!    publishes for it, and every one of them is readable;
//! 4. a schema AWS publishes is no customer resource, so it carries no ARN;
//! 5. a published document titles the EVENT it describes, and the draft-04 export
//!    carries that title, the component the document roots at as its own root, every
//!    other component as a definition, and OpenAPI's `nullable` lowered to a draft-04
//!    type union.
//!
//! Every case drives the real wire path, through the harness in
//! `registry_schema_lifecycle.rs` — nothing here touches the state struct directly.

use std::collections::BTreeSet;

use serde_json::{json, Value};
use service_com_amazonaws_schemas_schemas::state::{
    aws_managed_schemas, managed_schema_title, managed_version_count, ManagedSchema,
    AWS_MANAGED_REGISTRY, AWS_SERVICE_EVENT_TYPES,
    CLOUD_TRAIL_EVENT_FAMILY, EVENT_ENVELOPE_COMPONENT,
};

use crate::registry_schema_lifecycle::{
    create_registry, export_uri, schema_names, schema_uri, Answer, Emulator,
    OPENAPI_ONLY_KEYWORDS,
};

/// The listing of the AWS-managed registry's schemas.
fn catalogue_uri() -> String {
    format!("/v1/registries/name/{AWS_MANAGED_REGISTRY}/schemas")
}

/// The catalogue entry published under `name`.
fn entry(name: &str) -> &'static ManagedSchema {
    aws_managed_schemas()
        .iter()
        .find(|entry| entry.name == name)
        .unwrap_or_else(|| panic!("case {name:?}: the catalogue must hold it"))
}

/// The schema names the catalogue publishes for one event source.
async fn names_of_source(emulator: &Emulator, source: &str) -> Vec<String> {
    let listed = emulator
        .call(
            "GET",
            &format!("{}?schemaNamePrefix={source}@", catalogue_uri()),
            None,
        )
        .await;
    assert_eq!(
        listed.status, 200,
        "case {source:?}: expected 200, got {} {:?}",
        listed.status, listed.body
    );
    schema_names(&listed)
}

/// One member of a `Schemas` listing, by schema name.
fn summary_of<'a>(answer: &'a Answer, name: &str) -> &'a Value {
    answer.body["Schemas"]
        .as_array()
        .unwrap_or_else(|| panic!("expected a Schemas array in {:?}", answer.body))
        .iter()
        .find(|item| item["SchemaName"].as_str() == Some(name))
        .unwrap_or_else(|| panic!("expected {name:?} among {:?}", answer.body))
}

/// The timestamp a response member carries, as it went on the wire.
fn timestamp_at(answer: &Answer, member: &str) -> String {
    answer
        .value_at(member)
        .as_str()
        .unwrap_or_else(|| panic!("expected `{member}` to be a timestamp in {:?}", answer.body))
        .to_string()
}

/// The event sources the membership cases run over.
///
/// Source: `crate::state::AWS_EVENT_SOURCES`, the sources the catalogue publishes
/// CloudTrail-delivered schemas for — which is not every source it publishes anything
/// for (`schema_name_collation.rs` covers a source that publishes its own event types
/// and no CloudTrail-delivered schema, and `catalogue_publications.rs` owns which
/// sources carry the family at all). The first two rows are the sources the issue
/// cited; the rest are sources it did not, including ones whose name is not a single
/// dotted word (`aws.amazon.connect`) and ones that publish nothing beyond the
/// CloudTrail family.
fn source_rows() -> Vec<&'static str> {
    vec![
        "aws.a4b",
        "aws.events",
        "aws.act",
        "aws.abuse",
        "aws.amazon.connect",
        "aws.amazonmq",
        "aws.amazonq",
        "aws.ec2",
        "aws.xray",
    ]
}

/// RULE: for a source the catalogue publishes CloudTrail-delivered schemas for, it
/// holds one per KIND of CloudTrail record EventBridge delivers — not the API call
/// alone — each published under `<source>@<EventName>` where `<EventName>` is the
/// event's `detail-type` with its spaces removed.
#[tokio::test]
async fn the_catalogue_publishes_the_whole_cloud_trail_family_of_a_source_it_holds_it_for() {
    let emulator = Emulator::new();

    // The naming rule the table is held to: the catalogue names a schema after the
    // event's detail-type with the spaces taken out.
    for (detail_type, event_name) in CLOUD_TRAIL_EVENT_FAMILY {
        assert_eq!(
            *event_name,
            detail_type.replace(' ', ""),
            "case {detail_type:?}: an EventName is its detail-type without spaces, \
             got {event_name:?}"
        );
    }
    assert!(
        CLOUD_TRAIL_EVENT_FAMILY.len() > 1,
        "the CloudTrail family is more than the API-call event, got {CLOUD_TRAIL_EVENT_FAMILY:?}"
    );

    for source in source_rows() {
        let published = names_of_source(&emulator, source).await;
        for (detail_type, event_name) in CLOUD_TRAIL_EVENT_FAMILY {
            let expected = format!("{source}@{event_name}");
            assert!(
                published.contains(&expected),
                "case {source:?}/{detail_type:?}: the catalogue must publish \
                 {expected:?}, got {published:?}"
            );

            // And it is readable: the document describes the event the name says.
            let described = emulator
                .call("GET", &schema_uri(AWS_MANAGED_REGISTRY, &expected), None)
                .await;
            assert_eq!(
                described.status, 200,
                "case {expected:?}: expected 200, got {} {:?}",
                described.status, described.body
            );
            let document: Value = serde_json::from_str(&described.str_at("Content"))
                .unwrap_or_else(|e| panic!("case {expected:?}: Content must be JSON: {e}"));
            let component = &document["components"]["schemas"][*event_name];
            assert!(
                component["properties"].is_object(),
                "case {expected:?}: the document must describe the event it is named \
                 after, got {document:?}"
            );
            // The document does not restate the event's identity anywhere in the
            // components; it titles the event it describes.
            assert_eq!(
                document["info"]["title"],
                json!(format!("{detail_type} published by {source}")),
                "case {expected:?}: the document must title the event it describes, \
                 got {document:?}"
            );
        }
    }
}

/// RULE (an invariant): a published schema's `LastModified` is a property OF THE
/// PUBLISHED SCHEMA — the same value in every store and across every reset, at
/// whole-second precision — and not of the moment the store was built, so two stores
/// built at two different moments report the same catalogue; and it is the property of
/// THAT schema, so entries different publications of the catalogue wrote report different
/// instants. WHICH instant each entry reports (the instant of the publication that wrote
/// it) is `catalogue_publications.rs`.
#[tokio::test]
async fn a_published_schema_reports_its_own_fixed_last_modified() {
    let first = Emulator::new();
    // A SECOND store, built later in this process: a value read off the clock would
    // differ between the two.
    let second = Emulator::new();

    // Rows: members of the catalogue that differ along the publishing source, whether
    // the event type is a CloudTrail one or the service's own, and how many versions
    // the catalogue publishes for it.
    let rows = [
        "aws.a4b@AWSAPICallviaCloudTrail",
        "aws.a4b@RoomStateChange",
        "aws.acm@ACMCertificateAvailable",
        "aws.events@AWSServiceEventviaCloudTrail",
        "aws.events@ScheduledEvent",
        "aws.ec2@EC2InstanceStateChangeNotification",
        "aws.xray@AWSInsightviaCloudTrail",
    ];

    let mut seen: BTreeSet<String> = BTreeSet::new();
    for name in rows {
        let here = first
            .call("GET", &schema_uri(AWS_MANAGED_REGISTRY, name), None)
            .await;
        assert_eq!(
            here.status, 200,
            "case {name:?}: expected 200, got {} {:?}",
            here.status, here.body
        );
        let there = second
            .call("GET", &schema_uri(AWS_MANAGED_REGISTRY, name), None)
            .await;
        let modified = timestamp_at(&here, "LastModified");
        assert_eq!(
            modified,
            timestamp_at(&there, "LastModified"),
            "case {name:?}: two stores must report the same instant for one published \
             schema"
        );
        assert!(
            !modified.contains('.'),
            "case {name:?}: a published instant is whole-second, got {modified:?}"
        );
        // Reading it twice from the same store cannot move it either.
        let again = first
            .call("GET", &schema_uri(AWS_MANAGED_REGISTRY, name), None)
            .await;
        assert_eq!(
            modified,
            timestamp_at(&again, "LastModified"),
            "case {name:?}: a read must not move the instant"
        );
        seen.insert(modified.clone());

        // The listing reports the same instant as the read of the schema.
        let listed = first
            .call(
                "GET",
                &format!("{}?schemaNamePrefix={name}", catalogue_uri()),
                None,
            )
            .await;
        assert_eq!(
            summary_of(&listed, name)["LastModified"],
            json!(modified),
            "case {name:?}: the summary and the read must report one instant"
        );
    }

    // Each row above was written by a publication of its OWN — three members of the
    // CloudTrail-delivered family, each written across every source that carries it, and
    // four sources' own event types — so the catalogue reports one instant per row and
    // does not collapse all of them onto one. WHICH instant each one reports is
    // `catalogue_publications.rs`.
    assert_eq!(
        seen.len(),
        rows.len(),
        "every row is an entry of a publication of its own, so the catalogue reports \
         {} instants for them, got {seen:?}",
        rows.len()
    );
}

/// RULE: a published schema's `VersionCount` is the number of versions the catalogue
/// publishes for it, every one of those versions is readable by number, and the version
/// a read without `SchemaVersion` answers is the newest of them.
#[tokio::test]
async fn a_published_schema_reports_every_version_it_publishes() {
    let emulator = Emulator::new();

    // Rows: entries published at one version and entries published at several — what
    // the rule generalises over is the COUNT. `aws.acm@ACMCertificateAvailable` is the
    // one the issue cited; the rest it did not.
    let rows = [
        "aws.acm@ACMCertificateAvailable",
        "aws.a4b@RoomStateChange",
        "aws.a4b@AWSAPICallviaCloudTrail",
        "aws.events@ScheduledEvent",
        "aws.s3@ObjectCreated",
    ];

    for name in rows {
        let published = managed_version_count(name);
        let listed = emulator
            .call(
                "GET",
                &format!("{}?schemaNamePrefix={name}", catalogue_uri()),
                None,
            )
            .await;
        let summary = summary_of(&listed, name);
        assert_eq!(
            summary["VersionCount"],
            json!(published),
            "case {name:?}: the summary must report every published version, \
             got {summary:?}"
        );

        let described = emulator
            .call("GET", &schema_uri(AWS_MANAGED_REGISTRY, name), None)
            .await;
        assert_eq!(
            described.str_at("SchemaVersion"),
            published.to_string(),
            "case {name:?}: a read without a version answers the newest one"
        );

        // Every version the count claims is readable, and reports its own number and
        // the published document.
        for version in 1..=published {
            let pinned = emulator
                .call(
                    "GET",
                    &format!(
                        "{}?schemaVersion={version}",
                        schema_uri(AWS_MANAGED_REGISTRY, name)
                    ),
                    None,
                )
                .await;
            assert_eq!(
                pinned.status, 200,
                "case {name:?} version={version}: expected 200, got {} {:?}",
                pinned.status, pinned.body
            );
            assert_eq!(
                pinned.str_at("SchemaVersion"),
                version.to_string(),
                "case {name:?} version={version}: the read answers the version asked for"
            );
            assert_eq!(
                pinned.str_at("Content"),
                entry(name).content,
                "case {name:?} version={version}: the catalogue's document is what is \
                 served"
            );
        }
        // And one past the count is not a version the catalogue holds.
        let past = emulator
            .call(
                "GET",
                &format!(
                    "{}?schemaVersion={}",
                    schema_uri(AWS_MANAGED_REGISTRY, name),
                    published + 1
                ),
                None,
            )
            .await;
        assert_eq!(
            past.status, 404,
            "case {name:?}: version {} is not published, got {} {:?}",
            published + 1,
            past.status,
            past.body
        );

        // The version listing agrees with the count.
        let versions = emulator
            .call(
                "GET",
                &format!("{}/versions", schema_uri(AWS_MANAGED_REGISTRY, name)),
                None,
            )
            .await;
        let reported = versions.body["SchemaVersions"]
            .as_array()
            .unwrap_or_else(|| panic!("case {name:?}: expected SchemaVersions in {:?}", versions.body))
            .len();
        assert_eq!(
            u64::try_from(reported).unwrap_or(u64::MAX),
            published,
            "case {name:?}: the version listing must hold every published version"
        );
    }
}

/// RULE: a schema in a service-owned registry is not a customer resource and carries no
/// ARN — the responses about one report `SchemaArn` empty and the listing summaries
/// leave the member out — while a schema a caller created keeps the ARN its create
/// minted.
#[tokio::test]
async fn a_schema_aws_publishes_carries_no_arn() {
    let emulator = Emulator::new();

    // Rows: entries differing in source and in whether the event type is a CloudTrail
    // one or the service's own.
    let rows = [
        "aws.events@AWSAPICallviaCloudTrail",
        "aws.events@AWSNetworkActivityEventviaCloudTrail",
        "aws.acm@ACMCertificateAvailable",
        "aws.ecs@ECSTaskStateChange",
    ];

    for name in rows {
        let described = emulator
            .call("GET", &schema_uri(AWS_MANAGED_REGISTRY, name), None)
            .await;
        assert_eq!(
            described.str_at("SchemaArn"),
            "",
            "case {name:?}: DescribeSchema reports no ARN for a schema AWS publishes"
        );

        let exported = emulator
            .call(
                "GET",
                &export_uri(AWS_MANAGED_REGISTRY, name, "JSONSchemaDraft4"),
                None,
            )
            .await;
        assert_eq!(
            exported.str_at("SchemaArn"),
            "",
            "case {name:?}: ExportSchema reports no ARN for a schema AWS publishes"
        );

        let listed = emulator
            .call(
                "GET",
                &format!("{}?schemaNamePrefix={name}", catalogue_uri()),
                None,
            )
            .await;
        assert!(
            summary_of(&listed, name)["SchemaArn"].is_null(),
            "case {name:?}: a listing summary of a published schema carries no \
             SchemaArn member, got {:?}",
            summary_of(&listed, name)
        );

        let versions = emulator
            .call(
                "GET",
                &format!("{}/versions", schema_uri(AWS_MANAGED_REGISTRY, name)),
                None,
            )
            .await;
        assert!(
            versions.body["SchemaVersions"][0]["SchemaArn"].is_null(),
            "case {name:?}: a version summary of a published schema carries no \
             SchemaArn member, got {:?}",
            versions.body
        );
    }

    // The contrast: a caller's own schema IS a resource, and reports the ARN its
    // create minted from every reader.
    create_registry(&emulator, "mine").await;
    let created = emulator
        .call(
            "POST",
            &schema_uri("mine", "own"),
            Some(json!({
                "Content": r#"{"openapi":"3.0.0","info":{"title":"own","version":"1"}}"#,
                "Type": "OpenApi3",
            })),
        )
        .await;
    assert_eq!(
        created.status, 201,
        "expected 201 from CreateSchema, got {} {:?}",
        created.status, created.body
    );
    let minted = created.str_at("SchemaArn");
    assert!(
        minted.contains(":schema/mine/own"),
        "a caller's schema is minted an ARN, got {minted:?}"
    );
    let described = emulator.call("GET", &schema_uri("mine", "own"), None).await;
    assert_eq!(
        described.str_at("SchemaArn"),
        minted,
        "a caller's schema reports the ARN its create minted"
    );
    let listed = emulator
        .call("GET", "/v1/registries/name/mine/schemas", None)
        .await;
    assert_eq!(
        summary_of(&listed, "own")["SchemaArn"],
        json!(minted),
        "a summary of a caller's schema carries its ARN"
    );
}

/// RULE: a published document titles the EVENT it describes — the event's `detail-type`
/// and the source publishing it — and the draft-04 export is computed from that
/// document: draft-04 named at the root, the source's title carried onto it, the
/// component the document roots at promoted to the root, every other component a
/// definition with its references following, and OpenAPI 3.0's `nullable` lowered to
/// draft-04's union of the member's type with `"null"`.
#[tokio::test]
async fn the_published_document_titles_its_event_and_exports_to_draft_4() {
    let emulator = Emulator::new();

    // Rows: one entry per kind of CloudTrail record (whose documents carry nullable
    // members and nested component references), plus every service-specific event type
    // the catalogue publishes — they differ in how many components they carry and how
    // deeply those reference each other.
    let mut rows: Vec<(String, String, String)> = CLOUD_TRAIL_EVENT_FAMILY
        .iter()
        .map(|(detail_type, event_name)| {
            (
                format!("aws.events@{event_name}"),
                "aws.events".to_string(),
                (*detail_type).to_string(),
            )
        })
        .collect();
    rows.extend(AWS_SERVICE_EVENT_TYPES.iter().map(|event| {
        (
            event.schema_name(),
            event.source.to_string(),
            event.detail_type.to_string(),
        )
    }));

    for (name, source, detail_type) in rows {
        let expected_title = managed_schema_title(&source, &detail_type);
        let described = emulator
            .call("GET", &schema_uri(AWS_MANAGED_REGISTRY, &name), None)
            .await;
        assert_eq!(
            described.status, 200,
            "case {name:?}: expected 200, got {} {:?}",
            described.status, described.body
        );
        let document: Value = serde_json::from_str(&described.str_at("Content"))
            .unwrap_or_else(|e| panic!("case {name:?}: Content must be JSON: {e}"));
        assert_eq!(
            document["info"]["title"],
            json!(expected_title),
            "case {name:?}: the document titles the event it describes, got {:?}",
            document["info"]
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
            "case {name:?}: expected 200, got {} {:?}",
            exported.status, exported.body
        );
        let converted: Value = serde_json::from_str(&exported.str_at("Content"))
            .unwrap_or_else(|e| panic!("case {name:?}: the export must be JSON: {e}"));
        assert_eq!(
            converted["$schema"],
            json!("http://json-schema.org/draft-04/schema#"),
            "case {name:?}: the export names draft-04, got {converted:?}"
        );
        assert_eq!(
            converted["title"],
            json!(expected_title),
            "case {name:?}: the export carries the document's title, got {converted:?}"
        );

        let components = document["components"]["schemas"]
            .as_object()
            .unwrap_or_else(|| panic!("case {name:?}: the document has components"));
        for component in components.keys() {
            assert!(
                converted["definitions"][component].is_object(),
                "case {name:?}: component {component:?} must be a definition, \
                 got {converted:?}"
            );
        }
        assert!(
            document["components"]["schemas"][EVENT_ENVELOPE_COMPONENT].is_null(),
            "case {name:?}: a published document declares no event envelope, got {document:?}"
        );
        assert!(
            converted["required"].is_null() && converted["properties"].is_null(),
            "case {name:?}: the export promotes no component where the document names \
             no root, got {converted:?}"
        );
        let rendered = serde_json::to_string(&converted).unwrap_or_default();
        assert!(
            !rendered.contains("#/components/schemas/"),
            "case {name:?}: no OpenAPI component ref may survive the conversion: \
             {converted:?}"
        );
        assert!(
            !rendered.contains("\"nullable\""),
            "case {name:?}: draft-04 has no `nullable` keyword: {converted:?}"
        );

        // Every member the source marked nullable is a union with "null" in the
        // export, and every member it did not keeps its single type.
        for (component, schema) in components {
            let Some(properties) = schema["properties"].as_object() else {
                continue;
            };
            let rendered = &converted["definitions"][component];
            for (member, declared) in properties {
                let exported_member = &rendered["properties"][member];
                // A member named for one of OpenAPI 3.0's own keywords is the one kind
                // of member the conversion does not carry over — draft-04 defines none
                // of them; every other member must be there, widened or as it stands.
                if OPENAPI_ONLY_KEYWORDS.contains(&member.as_str()) {
                    assert!(
                        exported_member.is_null(),
                        "case {name:?}: {component}.{member} is named for an \
                         OpenAPI-only keyword and does not survive, got {exported_member:?}"
                    );
                    continue;
                }
                let Some(declared_type) = declared["type"].as_str() else {
                    continue;
                };
                let expected_type = if declared["nullable"] == json!(true) {
                    json!([declared_type, "null"])
                } else {
                    json!(declared_type)
                };
                assert_eq!(
                    exported_member["type"], expected_type,
                    "case {name:?}: {component}.{member} must export as \
                     {expected_type:?}, got {exported_member:?}"
                );
            }
        }
    }
}
