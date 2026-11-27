// SPDX-License-Identifier: CC0-1.0

//! The `ReportDefinition` member set this service parses, persists and reports, end to end
//! over the awsJson1_1 wire.
//!
//! Every case drives a real `MultiServiceRouter` with this service mounted on one shared
//! engine, so each assertion is about what a client would see.
//!
//! The rule under test, stated over the SET of definitions a client can submit rather than
//! over any one recorded request: a read-back of a report definition states EVERY member of
//! the wire shape the service holds a value for — those the request named as supplied, those
//! it left unset as the service resolved them, and the service's own facts about the report
//! (the status of its last delivery and the date it was created). The member set is the whole
//! declared one, which includes `ReportCreationDate`: AWS carries that member while the
//! published model snapshot does not declare it, so this crate declares it itself (see
//! `smithy-model/LOCAL-ADDITIONS.md` and `src/ops/types/report_definition_types.rs`).
//!
//! The last test pins the local declaration against the model itself: every shape the
//! overlay restates, compared with the snapshot's own declaration of it, and the snapshot's
//! member list, which the local declaration must cover in full and which must still not
//! declare `ReportCreationDate`. A refreshed snapshot that declares the member, adds one the
//! overlay does not restate, or changes a target, value set or constraint fails there rather
//! than silently narrowing what the service accepts or reports.

use serde_json::{json, Map, Value};

use service_com_amazonaws_costandusagereportservice_awsorigamiservicegatewayservice as cur;

use cur::ops::types::{
    ADDITIONAL_ARTIFACT_VALUES, A_W_S_REGION_VALUES, COMPRESSION_FORMAT_VALUES, LAST_STATUS_VALUES,
    REPORT_FORMAT_VALUES, REPORT_VERSIONING_VALUES, SCHEMA_ELEMENT_VALUES, TIME_UNIT_VALUES,
};

use super::report_definition_lifecycle::{
    expected_read_back, is_report_timestamp, minimal_definition, read_back_of, Cur,
};

/// The published model snapshot this crate is generated from, embedded at compile time so
/// the local declaration is checked against the model rather than against a copy of it.
const SNAPSHOT: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/smithy-model/com-amazonaws-costandusagereportservice-awsorigamiservicegatewayservice.json"
));

/// The overlay model the local `ReportDefinition` declaration is generated from, embedded
/// for the same reason: what it restates of the snapshot must still match it.
const OVERLAY: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/smithy-model/local-additions.json"
));

/// The shape the local declaration is of.
const DEFINITION_SHAPE: &str = "com.amazonaws.costandusagereportservice#ReportDefinition";
/// The shape the overlay adds, which the snapshot declares nowhere.
const CREATION_DATE_SHAPE: &str = "com.amazonaws.costandusagereportservice#ReportCreationDate";
/// The one restated shape the overlay deliberately declares without its members: codegen
/// resolves a STRUCTURE member through the Rust field's type (the modelled type in
/// `shared_types`), reading only the shape's type from the model.
const STATUS_SHAPE: &str = "com.amazonaws.costandusagereportservice#ReportStatus";

/// The member this crate declares locally, which the snapshot does not.
const LOCAL_MEMBER: &str = "ReportCreationDate";

/// One value for every member the snapshot declares, so a case drives the whole modelled
/// member list through the wire.
///
/// The values come from the model: the enum members from the `*_VALUES` catalogues this
/// crate re-exports (`TimeUnit`, `ReportFormat`, `CompressionFormat`, `SchemaElement`,
/// `AWSRegion`, `AdditionalArtifact`, `ReportVersioning`, `LastStatus`), the bucket, prefix
/// and billing view in the forms their modelled patterns admit. `Format` and `Compression`
/// are the delimited-text pair on purpose: a Parquet report may not subscribe to the
/// QuickSight and Redshift manifests, and this row subscribes to every artifact there is.
fn every_member(report_name: &str) -> Value {
    let text_format = REPORT_FORMAT_VALUES
        .iter()
        .copied()
        .find(|format| *format == "textORcsv")
        .expect("the model's ReportFormat enum declares textORcsv");
    let text_compression = COMPRESSION_FORMAT_VALUES
        .iter()
        .copied()
        .find(|compression| *compression == "GZIP")
        .expect("the model's CompressionFormat enum declares GZIP");
    json!({
        "ReportName": report_name,
        "TimeUnit": TIME_UNIT_VALUES[0],
        "Format": text_format,
        "Compression": text_compression,
        "AdditionalSchemaElements": SCHEMA_ELEMENT_VALUES.to_vec(),
        "S3Bucket": "every-member.bucket_1",
        "S3Prefix": "teams/finance/cur/",
        "S3Region": A_W_S_REGION_VALUES[0],
        "AdditionalArtifacts": ADDITIONAL_ARTIFACT_VALUES.to_vec(),
        "RefreshClosedReports": false,
        "ReportVersioning": REPORT_VERSIONING_VALUES[0],
        "BillingViewArn": "arn:aws:billing::123456789012:billingview/123456789012",
        "ReportStatus": { "lastDelivery": "20240101T000000Z", "lastStatus": LAST_STATUS_VALUES[0] },
        LOCAL_MEMBER: "20200101T000000Z",
    })
}

/// The rows the rule is asserted over. They differ along what it generalises over: how many
/// members the definition carries, whether it states the locally declared member at all,
/// which value it states for it, and which optional members ride along.
fn definition_rows() -> Vec<(&'static str, Value)> {
    vec![
        (
            "the modelled required members only",
            minimal_definition("local-member-report"),
        ),
        (
            // The locally declared member alone beside the required ones: it is parsed,
            // not refused, and what comes back is the service's own instant.
            "the local member claimed by the request",
            {
                let mut definition = minimal_definition("local-member-report");
                definition[LOCAL_MEMBER] = json!("20200101T000000Z");
                definition
            },
        ),
        (
            // Another value of the same member: nothing about it is fixed to one string.
            "the local member claimed with another value",
            {
                let mut definition = minimal_definition("local-member-report");
                definition[LOCAL_MEMBER] = json!("19991231T235959Z");
                definition
            },
        ),
        (
            "the local member beside the optional modelled ones",
            {
                let mut definition = minimal_definition("local-member-report");
                definition["RefreshClosedReports"] = json!(false);
                definition["ReportVersioning"] = json!(REPORT_VERSIONING_VALUES[1]);
                definition["BillingViewArn"] = json!("my-billing.view_1");
                definition[LOCAL_MEMBER] = json!("20200101T000000Z");
                definition
            },
        ),
        (
            "every member the snapshot declares, plus the local one",
            every_member("local-member-report"),
        ),
    ]
}

/// The `shapes` object of one model document.
fn model_shapes(model: &str) -> Value {
    let parsed: Value = serde_json::from_str(model).expect("a model document must be JSON");
    let shapes = parsed["shapes"]
        .as_object()
        .unwrap_or_else(|| panic!("a model document must carry a shapes object"))
        .clone();
    Value::Object(shapes)
}

/// `declaration` with every documentation trait dropped, recursively — and with a traits
/// object left empty by that dropped too, so a member the snapshot documents and nothing
/// else compares equal to the same member restated with no traits at all.
///
/// Documentation is prose: the overlay restates what codegen READS — targets, types, enum
/// values and constraint traits — so comparing declarations has to ignore the one trait that
/// carries no behaviour.
fn without_docs(declaration: &Value) -> Value {
    match declaration {
        Value::Object(members) => Value::Object(
            members
                .iter()
                .filter(|(key, _)| key.as_str() != "smithy.api#documentation")
                .map(|(key, value)| (key.clone(), without_docs(value)))
                .filter(|(key, value)| {
                    key.as_str() != "traits" || !value.as_object().is_some_and(Map::is_empty)
                })
                .collect(),
        ),
        Value::Array(items) => Value::Array(items.iter().map(without_docs).collect()),
        other => other.clone(),
    }
}

/// The values an enum shape declares, in declaration order: the `@enumValue` of each of its
/// members, which is what travels on the wire and what the member NAME (a Rust-side label
/// codegen never serializes) is free of.
fn enum_values(shape: &Value) -> Vec<String> {
    shape["members"]
        .as_object()
        .map(|members| {
            members
                .values()
                .map(|member| {
                    member["traits"]["smithy.api#enumValue"]
                        .as_str()
                        .unwrap_or_default()
                        .to_string()
                })
                .collect()
        })
        .unwrap_or_default()
}

/// What codegen reads from a shape declaration: the shape's type, the constraint traits on
/// it, the values of an enum and the element target of a list. Comparing this is what makes
/// the overlay's restatement checkable against the snapshot without pinning the prose or the
/// enum member labels.
fn codegen_view(shape: &Value) -> Value {
    let mut view = Map::new();
    view.insert("type".to_string(), shape["type"].clone());
    let traits = without_docs(&shape["traits"]);
    if traits.as_object().is_some_and(|traits| !traits.is_empty()) {
        view.insert("traits".to_string(), traits);
    }
    if shape["type"] == json!("enum") {
        let mut values = enum_values(shape);
        values.sort();
        view.insert("values".to_string(), json!(values));
    }
    if let Some(member) = shape.get("member") {
        view.insert("member".to_string(), member["target"].clone());
    }
    Value::Object(view)
}

/// The `members` object of the snapshot's `ReportDefinition` shape.
fn snapshot_definition_members() -> Map<String, Value> {
    model_shapes(SNAPSHOT)[DEFINITION_SHAPE]["members"]
        .as_object()
        .unwrap_or_else(|| panic!("the snapshot must declare ReportDefinition with members"))
        .clone()
}

#[tokio::test]
async fn every_read_of_a_definition_states_the_locally_declared_member() {
    for (case, definition) in definition_rows() {
        // Through the create path...
        let cur = Cur::new();
        let (status, body) = cur.put(&definition, None).await;
        assert_eq!(
            status, 200,
            "case {case:?} via PutReportDefinition: expected HTTP 200, got {status} {body:?}"
        );
        let listed = cur.describe_all().await;
        let first = listed.first().cloned().unwrap_or(Value::Null);
        let created = first[LOCAL_MEMBER].as_str().unwrap_or_default();
        assert!(
            is_report_timestamp(created),
            "case {case:?} via PutReportDefinition: expected {LOCAL_MEMBER} stated as \
             yyyymmddThhmmssZ, got {first:?}"
        );
        // The value is the service's own fact about the report, so a request that claimed
        // one is not what the read states.
        assert_ne!(
            Some(created),
            definition[LOCAL_MEMBER].as_str(),
            "case {case:?} via PutReportDefinition: expected the creation date the service \
             holds, got {first:?}"
        );
        assert_eq!(
            read_back_of(&first, case),
            expected_read_back(&with_no_local_member(&definition)),
            "case {case:?} via PutReportDefinition: expected the stored definition plus the \
             service-populated members, got {first:?}"
        );

        // ...and through the update path, which takes the same shape.
        let cur = Cur::new();
        let (status, body) = cur.put(&minimal_definition("local-member-report"), None).await;
        assert_eq!(status, 200, "case {case:?}: setup got {body:?}");
        let (status, body) = cur
            .call(
                "ModifyReportDefinition",
                &json!({
                    "ReportName": "local-member-report",
                    "ReportDefinition": definition,
                }),
            )
            .await;
        assert_eq!(
            status, 200,
            "case {case:?} via ModifyReportDefinition: expected HTTP 200, got {status} {body:?}"
        );
        let listed = cur.read_back_all(case).await;
        assert_eq!(
            listed,
            vec![expected_read_back(&with_no_local_member(&definition))],
            "case {case:?} via ModifyReportDefinition: expected the replacement definition \
             plus the service-populated members, got {listed:?}"
        );

        // A reporting call does not move the date: every read answers the stored instant.
        let repeated = cur.read_back_all(case).await;
        assert_eq!(
            repeated, listed,
            "case {case:?}: a read must not move the creation date, got {repeated:?}"
        );
    }
}

/// `definition` without the locally declared member: the service fills that member from the
/// report it holds, so what a read-back states for it is checked by [`read_back_of`] rather
/// than compared against the request.
fn with_no_local_member(definition: &Value) -> Value {
    let mut definition = definition.clone();
    definition
        .as_object_mut()
        .expect("a report definition is an object")
        .remove(LOCAL_MEMBER);
    definition
}

#[tokio::test]
async fn the_declared_member_set_covers_the_snapshot_and_the_member_it_lacks() {
    // Half one: the local declaration covers the snapshot's own member list, and adds
    // exactly the member the snapshot does not declare.
    let snapshot_members = snapshot_definition_members();
    assert!(
        !snapshot_members.contains_key(LOCAL_MEMBER),
        "the snapshot now declares {LOCAL_MEMBER}, so the local declaration should give way \
         to the modelled member; snapshot members {:?}",
        snapshot_members.keys().collect::<Vec<_>>()
    );
    let submitted = every_member("every-member-report");
    let submitted_members: Vec<String> = submitted
        .as_object()
        .expect("the row is an object")
        .keys()
        .cloned()
        .collect();
    for member in snapshot_members.keys() {
        assert!(
            submitted_members.contains(member),
            "the snapshot declares {member}, which this suite must drive through the wire; \
             submitted {submitted_members:?}"
        );
    }
    assert_eq!(
        submitted_members.len(),
        snapshot_members.len() + 1,
        "the declared set is the snapshot's members plus {LOCAL_MEMBER} only; submitted \
         {submitted_members:?}, snapshot {:?}",
        snapshot_members.keys().collect::<Vec<_>>()
    );

    // Half two: that whole set survives the trip, and the read-back states it in full.
    let cur = Cur::new();
    let (status, body) = cur.put(&submitted, None).await;
    assert_eq!(
        status, 200,
        "every declared member must be accepted, got {status} {body:?}"
    );
    let listed = cur.describe_all().await;
    let first = listed.first().cloned().unwrap_or(Value::Null);
    let reported: Vec<String> = first
        .as_object()
        .unwrap_or_else(|| panic!("a read-back is an object, got {first:?}"))
        .keys()
        .cloned()
        .collect();
    for member in snapshot_members.keys() {
        assert!(
            reported.contains(member),
            "the read-back must state the modelled member {member}, got {reported:?}"
        );
    }
    assert!(
        reported.contains(&LOCAL_MEMBER.to_string()),
        "the read-back must state the locally declared member {LOCAL_MEMBER}, got {reported:?}"
    );
    assert_eq!(
        reported.len(),
        snapshot_members.len() + 1,
        "the read-back states the snapshot's members plus {LOCAL_MEMBER} only, got {reported:?}"
    );
    assert_eq!(
        read_back_of(&first, "every declared member"),
        expected_read_back(&with_no_local_member(&submitted)),
        "every declared member must survive the trip, got {first:?}"
    );
}

#[test]
fn the_overlay_restates_the_snapshot_exactly_but_for_the_member_it_adds() {
    // The overlay is what the local declaration is generated from, so anything it restates
    // of the snapshot — a member's target, a shape's type, an enum's values, a constraint
    // trait — must still BE what the snapshot says, or the type on the wire would be
    // narrower or wider than the model without any other test noticing.
    let snapshot = model_shapes(SNAPSHOT);
    let overlay = model_shapes(OVERLAY);
    let restated = overlay
        .as_object()
        .expect("the overlay carries a shapes object");

    for (shape_id, declared) in restated {
        if shape_id == CREATION_DATE_SHAPE {
            assert!(
                snapshot[shape_id].is_null(),
                "the snapshot now declares {shape_id}, so the local declaration should give \
                 way to the modelled shape"
            );
            continue;
        }
        let modelled = &snapshot[shape_id];
        assert!(
            !modelled.is_null(),
            "the overlay restates {shape_id}, which the snapshot no longer declares"
        );

        if shape_id == DEFINITION_SHAPE {
            // The one shape that differs on purpose: the snapshot's members, restated as
            // the snapshot declares them, plus the member AWS carries and it does not.
            let declared_members = declared["members"]
                .as_object()
                .expect("the overlay declares the definition members");
            let modelled_members = modelled["members"]
                .as_object()
                .expect("the snapshot declares the definition members");
            for (member, modelled_member) in modelled_members {
                assert_eq!(
                    without_docs(&declared_members[member]),
                    without_docs(modelled_member),
                    "member {member} of {shape_id} must be restated as the snapshot declares \
                     it, got {:?}",
                    declared_members.get(member)
                );
            }
            let added: Vec<&String> = declared_members
                .keys()
                .filter(|member| !modelled_members.contains_key(*member))
                .collect();
            assert_eq!(
                added,
                vec![LOCAL_MEMBER],
                "{shape_id} may add {LOCAL_MEMBER} and nothing else, added {added:?}"
            );
            continue;
        }

        if shape_id == STATUS_SHAPE {
            // Declared without its members on purpose (see the constant): only the type is
            // read from the model, and the modelled Rust type carries the members.
            assert_eq!(
                declared["type"], modelled["type"],
                "{shape_id} must be restated with the type the snapshot gives it"
            );
            assert!(
                declared["members"].as_object().is_some_and(Map::is_empty),
                "{shape_id} is restated for its type alone, so it must declare no members"
            );
            continue;
        }

        assert_eq!(
            codegen_view(declared),
            codegen_view(modelled),
            "{shape_id} must be restated as the snapshot declares it"
        );
    }
}
