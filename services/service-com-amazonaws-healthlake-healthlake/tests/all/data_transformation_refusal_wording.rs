// SPDX-License-Identifier: CC0-1.0

//! How the data transformation surface WORDS its answers on the awsJson1_0 wire, and
//! what a create and the read after it carry.
//!
//! Four rules, each asserted over a SET of inputs rather than over one recorded
//! request:
//!
//! 1. **A not-found refusal NAMES what the call could not find.** A call that resolves
//!    a profile states `Profile '<ProfileId>' not found`; one that resolves a
//!    particular VERSION of a profile states `Profile '<ProfileId>' version <n> not
//!    found`; a transformation-job read states `Transformation job not found:
//!    <JobId>`. The variable part comes from the request, so two refusals of the same
//!    operation differ exactly where the request did.
//! 2. **A boundary-validation refusal states ONE sentence under BOTH message keys.**
//!    The modelled `ValidationException` declares `Message`
//!    (`smithy_query(query="shape",shape="ValidationException")`) and AWS's validation
//!    framework words the same sentence in the lowercase `message`; both reach the
//!    wire carrying the same text. In it, a `@pattern` clause names the member and the
//!    pattern WITHOUT the `^`/`$` anchors and echoes no value, and an enum clause
//!    names the member and the value set the service reports.
//! 3. **A value that is not an ARN at all is refused by the ARN PARSER**, not by the
//!    member's `@pattern`: `com.amazon.arn.ARNSyntaxException: ARNs must start with
//!    'arn:': <value>`, so no refusal pastes the service's resource-ARN regex into a
//!    response. A value that IS an ARN is resolved against state as before.
//! 4. **A create answers the protocol's 200 with the new profile's status**, and a
//!    read naming no version serves the latest published version — the DRAFT while
//!    there is none — so create -> get and create -> update -> get read back the
//!    content the caller just wrote.
//!
//! Every call goes through [`MultiServiceRouter::dispatch`][d] by way of the shared
//! [`Profiles`] harness — the same entry point `core/router` serves — so routing,
//! deserialization, the modelled constraints and the error envelope are all
//! exercised, not bypassed.
//!
//! [d]: smithy_server_runtime::MultiServiceRouter::dispatch

use std::collections::BTreeMap;

use serde_json::{json, Value};
use service_com_amazonaws_healthlake_healthlake::scaffolded::wire_http_status;
use service_com_amazonaws_healthlake_healthlake::state::{
    transformation_job_not_found_message, ACTIVE_PROFILE_STATUS, REPORTED_SOURCE_FORMATS,
};

use crate::data_transformation_profile::{
    expected_profile_not_found, mapping, modelled_message, profile_mapping_source,
    reported_mapping, Profiles,
};
use crate::fhir_datastore_lifecycle::{error_code, string_at};

/// The success status this service answers every operation with: awsJson1_0 binds
/// nothing to the transport, so `CreateDataTransformationProfile`'s modelled
/// `@http(code: 201)` is inert and a success is the protocol's 200.
const SUCCESS_STATUS: u16 = 200;

/// Create one profile and return its `ProfileId`, asserting the create's own half of
/// RULE 4 as it goes: the protocol status and the status of the profile it minted.
async fn create(healthlake: &Profiles, case: &str, request: &Value) -> String {
    let (status, created) = healthlake
        .call("CreateDataTransformationProfile", request)
        .await;
    assert_eq!(
        status, SUCCESS_STATUS,
        "case {case:?}: the create answers the protocol's status, got {status}: {created}"
    );
    assert_eq!(
        created["ProfileStatus"].as_str(),
        Some(ACTIVE_PROFILE_STATUS),
        "case {case:?}: the create reports the new profile's status, got {created}"
    );
    string_at(&created, "ProfileId")
}

/// A `ProfileMapping` update request body.
fn request_mapping(content: &BTreeMap<String, String>) -> Value {
    Value::Object(
        content
            .iter()
            .map(|(path, text)| (path.clone(), Value::String(text.clone())))
            .collect(),
    )
}

/// RULE 1: every not-found refusal of this surface names the resource the request
/// asked for, through the template of the call that resolved it.
///
/// The rows span the OPERATION (every data-transformation operation that resolves a
/// profile or a job, plus the ARN-keyed tag reader) and, for each, the FORM of the
/// identifier: ids differing in their digits, a case variant, and — for the job
/// reader, whose `DataTransformationJobId` admits far more than a 32-hex id — a short
/// id, a long one and a hyphenated one.
#[tokio::test]
async fn every_not_found_refusal_names_the_resource_it_could_not_find() {
    let healthlake = Profiles::new();

    // One live profile, so the refusals below are about the resource asked for and
    // not about a surface that refuses everything.
    let live = create(
        &healthlake,
        "the control profile",
        &json!({
            "ProfileName": "refusal-control",
            "SourceFormat": "CCDA",
            "Source": profile_mapping_source(&mapping(&[("a.map", "Patient.id <- id")])),
        }),
    )
    .await;

    // Well-formed `ProfileIdString` values (32 lowercase hex) that no record holds.
    let unheld: [&str; 4] = [
        "ffffffffffffffffffffffffffffffff",
        "00000000000000000000000000000000",
        "0123456789abcdef0123456789abcdef",
        "deadbeefdeadbeefdeadbeefdeadbeef",
    ];
    for profile_id in unheld {
        assert_ne!(
            profile_id, live,
            "case {profile_id:?}: the row must not name the live profile"
        );
        let requests: Vec<(&str, Value)> = vec![
            (
                "GetDataTransformationProfile",
                json!({ "ProfileId": profile_id }),
            ),
            (
                "GetDataTransformationProfile",
                json!({ "ProfileId": profile_id, "ProfileVersion": 0 }),
            ),
            (
                "GetDataTransformationProfile",
                json!({ "ProfileId": profile_id, "ProfileVersion": 99 }),
            ),
            (
                "ListDataTransformationProfileVersions",
                json!({ "ProfileId": profile_id }),
            ),
            (
                "DeleteDataTransformationProfile",
                json!({ "ProfileId": profile_id }),
            ),
            (
                "UpdateDataTransformationProfile",
                json!({
                    "ProfileId": profile_id,
                    "ProfileMapping": request_mapping(&mapping(&[("a.map", "Patient.id <- id")])),
                }),
            ),
            (
                "PublishDataTransformationProfile",
                json!({ "ProfileId": profile_id, "SourceFormat": "CCDA" }),
            ),
            (
                "PublishDataTransformationProfile",
                json!({
                    "ProfileId": profile_id,
                    "SourceFormat": "CCDA",
                    "FromExistingVersion": 3,
                }),
            ),
        ];
        for (operation, request) in requests {
            let case = format!("{operation} on {profile_id} as {request}");
            let (status, code, payload) = healthlake
                .refused_body(&case, operation, &request).await;
            let expected = expected_profile_not_found(operation, &request);
            assert_eq!(
                (status, code.as_str(), modelled_message(&payload)),
                (
                    wire_http_status(404),
                    "ResourceNotFoundException",
                    Some(expected.as_str())
                ),
                "case {case:?}: expected {expected:?}, got {payload}"
            );
            assert!(
                modelled_message(&payload).is_some_and(|text| text.contains(profile_id)),
                "case {case:?}: the refusal must name the profile asked for, got {payload}"
            );
        }
    }

    // The job reader's own template, over the forms its modelled
    // `DataTransformationJobId` admits (1..32, `^[a-zA-Z0-9-]+$`): a single
    // character, a hyphenated name, an all-digit id, a case variant and one at the
    // modelled maximum length.
    let job_ids: [String; 5] = [
        "a".to_string(),
        "sim2real-job-00000000".to_string(),
        "0123456789".to_string(),
        "SIM2REAL-JOB-00000000".to_string(),
        "j".repeat(32),
    ];
    for job_id in &job_ids {
        let (status, code, payload) = healthlake
            .refused_body(
                job_id,
                "DescribeDataTransformationJob",
                &json!({ "JobId": job_id }),
            )
            .await;
        let expected = transformation_job_not_found_message(job_id);
        assert_eq!(
            (status, code.as_str(), modelled_message(&payload)),
            (
                wire_http_status(404),
                "ResourceNotFoundException",
                Some(expected.as_str())
            ),
            "case {job_id:?}: expected {expected:?}, got {payload}"
        );
    }

    // The live profile still reads back, so the refusals are about identity.
    let read = healthlake
        .ok(
            "the control profile",
            "GetDataTransformationProfile",
            &json!({ "ProfileId": &live }),
        )
        .await;
    assert_eq!(
        read["ProfileId"].as_str(),
        Some(live.as_str()),
        "the live profile must still read back, got {read}"
    );
}

/// One row of the boundary table: a request breaking exactly one rule the model
/// declares, and what the refusal must say about it.
struct BoundaryCase {
    /// What this row varies, named in every assertion it makes.
    case: &'static str,
    operation: &'static str,
    request: Value,
    /// The clause the sentence must carry, in full.
    clause: String,
}

/// RULE 2: a boundary-validation refusal carries ONE sentence under both message
/// keys, with the pattern clause unanchored and unechoed and the enum clause naming
/// the reported value set.
///
/// The rows span the constraint CLASS (a `@pattern`, an enum value set) and the
/// MEMBER and OPERATION carrying it — a job id, a profile id, a client token and the
/// source format of both the create and the listing — so the rule is not keyed to one
/// recorded request. Every expected pattern is read from the shipped model, so no
/// regex is written out here.
#[tokio::test]
async fn a_boundary_refusal_states_one_sentence_under_both_message_keys() {
    let healthlake = Profiles::new();
    let source = profile_mapping_source(&mapping(&[("a.map", "Patient.id <- id")]));

    let rows: Vec<BoundaryCase> = vec![
        BoundaryCase {
            case: "a JobId outside its declared pattern",
            operation: "DescribeDataTransformationJob",
            request: json!({ "JobId": "sim2real_job_1" }),
            clause: pattern_clause("jobId", "DataTransformationJobId"),
        },
        BoundaryCase {
            case: "a JobId outside its declared pattern another way",
            operation: "DescribeDataTransformationJob",
            request: json!({ "JobId": "jobs/one" }),
            clause: pattern_clause("jobId", "DataTransformationJobId"),
        },
        BoundaryCase {
            case: "a ProfileId outside its declared pattern",
            operation: "GetDataTransformationProfile",
            request: json!({ "ProfileId": "ZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZ" }),
            clause: pattern_clause("profileId", "ProfileIdString"),
        },
        BoundaryCase {
            case: "a ClientToken outside its declared pattern",
            operation: "CreateDataTransformationProfile",
            request: json!({
                "ProfileName": "bad-token",
                "SourceFormat": "CCDA",
                "ClientToken": "not a token",
                "Source": source.clone(),
            }),
            clause: pattern_clause("clientToken", "ClientToken"),
        },
        BoundaryCase {
            case: "a SourceFormat outside the enum, on the create",
            operation: "CreateDataTransformationProfile",
            request: json!({
                "ProfileName": "bad-format",
                "SourceFormat": "PARQUET",
                "Source": source.clone(),
            }),
            clause: enum_clause("sourceFormat", REPORTED_SOURCE_FORMATS),
        },
        BoundaryCase {
            case: "a SourceFormat outside the enum, on the listing",
            operation: "ListDataTransformationProfiles",
            request: json!({ "SourceFormat": "PARQUET" }),
            clause: enum_clause("sourceFormat", REPORTED_SOURCE_FORMATS),
        },
        BoundaryCase {
            case: "a TargetFormat-shaped value on the source format member",
            operation: "ListDataTransformationProfiles",
            request: json!({ "SourceFormat": "FHIR_R4" }),
            clause: enum_clause("sourceFormat", REPORTED_SOURCE_FORMATS),
        },
    ];

    for BoundaryCase {
        case,
        operation,
        request,
        clause,
    } in rows
    {
        let (status, payload) = healthlake.call(operation, &request).await;
        assert_eq!(
            status, 400,
            "case {case:?} on {operation}: expected HTTP 400, got {status}: {payload}"
        );
        assert_eq!(
            error_code(&payload),
            "ValidationException",
            "case {case:?} on {operation}: expected __type ValidationException, got {payload}"
        );
        let expected = format!("1 validation error detected: {clause}");
        for key in ["Message", "message"] {
            let actual = payload.get(key).and_then(Value::as_str);
            assert_eq!(
                actual,
                Some(expected.as_str()),
                "case {case:?} on {operation}: the {key} member must state {expected:?}, \
                 got {actual:?}"
            );
        }
        assert!(
            !expected.contains('^') && !expected.contains('$'),
            "case {case:?} on {operation}: the clause must carry no anchors, got {expected:?}"
        );
        assert!(
            !expected.contains("Value '"),
            "case {case:?} on {operation}: the clause must echo no rejected value, \
             got {expected:?}"
        );
    }
}

/// RULE 3: a value that is not an ARN at all is refused by the ARN parser, in the
/// parser's own words, from every operation that takes a resource ARN.
///
/// The rows span the FORM of the value — a bare name, a path, an ARN-like string
/// whose prefix is wrong, one that merely contains `arn:`, and a long value — and the
/// OPERATION, which is every awsJson operation of this service whose input declares a
/// resource ARN. The control row is an ARN the parser accepts: it is resolved against
/// state and refused as not found, so this rule is about parsing and not about
/// refusing everything.
#[tokio::test]
async fn a_value_that_is_not_an_arn_is_refused_by_the_arn_parser() {
    let healthlake = Profiles::new();

    let long_value = "x".repeat(200);
    let not_arns: Vec<&str> = vec![
        "not-a-valid-arn",
        "healthlake:us-east-1:000000000000:datastore/fhir/x",
        "ARN:aws:healthlake:us-east-1:000000000000:datastore/fhir/x",
        "/arn:aws:healthlake:us-east-1:000000000000:datastore/fhir/x",
        "my-arn:thing",
        long_value.as_str(),
    ];

    for value in not_arns {
        for (operation, request) in arn_requests(value) {
            let case = format!("{operation} on {value:?}");
            let (status, payload) = healthlake.call(operation, &request).await;
            assert_eq!(
                status, 400,
                "case {case:?}: expected HTTP 400, got {status}: {payload}"
            );
            let expected =
                format!("com.amazon.arn.ARNSyntaxException: ARNs must start with 'arn:': {value}");
            let actual = modelled_message(&payload);
            assert_eq!(
                actual,
                Some(expected.as_str()),
                "case {case:?}: expected {expected:?}, got {actual:?}"
            );
            assert!(
                !payload.to_string().contains("satisfy regular expression"),
                "case {case:?}: the refusal must not paste the resource-ARN pattern, \
                 got {payload}"
            );
        }
    }

    // A value past the length a message echoes is ELIDED rather than pasted back, so
    // a long request cannot amplify itself into a long response. The bound is the
    // framework's own (`MAX_ECHOED_VALUE`, 256 characters).
    let past_the_echo_bound = "y".repeat(400);
    let (status, payload) = healthlake
        .call(
            "ListTagsForResource",
            &json!({ "ResourceARN": &past_the_echo_bound }),
        )
        .await;
    let message = modelled_message(&payload).unwrap_or_default().to_string();
    assert_eq!(
        status, 400,
        "a value past the echo bound is still refused, got {status}: {payload}"
    );
    assert!(
        message.starts_with("com.amazon.arn.ARNSyntaxException: ARNs must start with 'arn:': y")
            && message.len() < past_the_echo_bound.len(),
        "a value past the echo bound is elided in the parser's refusal, got {message:?}"
    );

    // The control: a value the parser ACCEPTS is resolved against state instead.
    let held = "arn:aws:healthlake:us-east-1:000000000000:datastore/fhir/\
                00000000000000000000000000000000";
    for (operation, request) in arn_requests(held) {
        let case = format!("{operation} on a well-formed ARN");
        let (status, payload) = healthlake.call(operation, &request).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status}: {payload}"
        );
        assert_eq!(
            error_code(&payload),
            "ResourceNotFoundException",
            "case {case:?}: a parseable ARN is resolved against state, got {payload}"
        );
    }
}

/// One row of the authoring-loop table: what the create stores and what the update
/// replaces it with, for one source format.
struct AuthoringCase {
    /// What this row varies, named in every assertion it makes.
    case: &'static str,
    source_format: &'static str,
    /// The content the create's `Source` carries.
    initial: BTreeMap<String, String>,
    /// The content the update replaces it with.
    replacement: BTreeMap<String, String>,
}

/// RULE 4: a create answers with the new profile's status, and a read naming no
/// version serves the latest published version — the DRAFT while there is none — so
/// the authoring loop reads back what it just wrote.
///
/// The rows differ along the create's SOURCE FORMAT and along the SHAPE of the
/// content (one template entry, the one CSV mapping document), and each row walks
/// create -> get, then update -> get, then publish -> get, so the omitted-version
/// read is asserted at every stage a profile passes through.
#[tokio::test]
async fn a_read_naming_no_version_serves_the_latest_version_the_profile_holds() {
    let healthlake = Profiles::new();

    let rows: Vec<AuthoringCase> = vec![
        AuthoringCase {
            case: "one entry, CCDA",
            source_format: "CCDA",
            initial: mapping(&[("Resources/Patient.vm", "initial-content")]),
            replacement: mapping(&[("Resources/Patient.vm", "updated-content")]),
        },
        AuthoringCase {
            case: "the CSV mapping document, CSV",
            source_format: "CSV",
            initial: mapping(&[("mapping-config.yaml", "tableName: patients")]),
            replacement: mapping(&[("mapping-config.yaml", "tableName: people")]),
        },
    ];

    for AuthoringCase {
        case,
        source_format,
        initial,
        replacement,
    } in rows
    {
        let profile_id = create(
            &healthlake,
            case,
            &json!({
                "ProfileName": format!("draft-readable-{source_format}"),
                "SourceFormat": source_format,
                "Source": profile_mapping_source(&initial),
            }),
        )
        .await;

        // create -> get, with no ProfileVersion: the DRAFT the create wrote.
        let draft = healthlake
            .ok(
                case,
                "GetDataTransformationProfile",
                &json!({ "ProfileId": &profile_id }),
            )
            .await;
        assert_eq!(
            (draft["Version"].as_i64(), reported_mapping(case, &draft)),
            (Some(0), initial.clone()),
            "case {case:?}: an unpublished profile reads back at its DRAFT, got {draft}"
        );
        assert_eq!(
            draft["SourceFormat"].as_str(),
            Some(source_format),
            "case {case:?}: the DRAFT reports the stored source format, got {draft}"
        );
        assert_eq!(
            draft["TargetFormat"].as_str(),
            Some("FHIR_R4"),
            "case {case:?}: the only modelled TargetFormat is FHIR_R4, got {draft}"
        );
        assert!(
            draft["ProfileName"].is_string() && draft["LastUpdatedAt"].as_f64().is_some(),
            "case {case:?}: the read carries every required output member, got {draft}"
        );

        // create -> update -> get: the content the update wrote, still the DRAFT.
        healthlake
            .ok(
                case,
                "UpdateDataTransformationProfile",
                &json!({
                    "ProfileId": &profile_id,
                    "ProfileMapping": request_mapping(&replacement),
                }),
            )
            .await;
        let updated = healthlake
            .ok(
                case,
                "GetDataTransformationProfile",
                &json!({ "ProfileId": &profile_id }),
            )
            .await;
        assert_eq!(
            (
                updated["Version"].as_i64(),
                reported_mapping(case, &updated)
            ),
            (Some(0), replacement.clone()),
            "case {case:?}: the DRAFT an update wrote is what a read naming no version \
             serves, got {updated}"
        );

        // And once a version IS published, that version is what the same read serves.
        let published = healthlake
            .ok(
                case,
                "PublishDataTransformationProfile",
                &json!({ "ProfileId": &profile_id, "SourceFormat": source_format }),
            )
            .await;
        assert_eq!(
            published["Version"].as_i64(),
            Some(1),
            "case {case:?}: the first publish is version 1, got {published}"
        );
        let latest = healthlake
            .ok(
                case,
                "GetDataTransformationProfile",
                &json!({ "ProfileId": &profile_id }),
            )
            .await;
        assert_eq!(
            (latest["Version"].as_i64(), reported_mapping(case, &latest)),
            (Some(1), replacement.clone()),
            "case {case:?}: a published version is what a read naming no version serves, \
             got {latest}"
        );
    }
}

/// Every awsJson request of this service that carries a resource ARN, with `value` in
/// that member.
fn arn_requests(value: &str) -> Vec<(&'static str, Value)> {
    vec![
        ("ListTagsForResource", json!({ "ResourceARN": value })),
        (
            "TagResource",
            json!({ "ResourceARN": value, "Tags": [{ "Key": "env", "Value": "test" }] }),
        ),
        (
            "UntagResource",
            json!({ "ResourceARN": value, "TagKeys": ["env"] }),
        ),
    ]
}

/// The clause this service states for a value outside the `@pattern` of `shape`, at
/// the member path `path`: the pattern as the shipped model declares it, without the
/// anchors, and no echo of the rejected value.
fn pattern_clause(path: &str, shape: &str) -> String {
    let pattern = modelled_pattern(shape);
    let unanchored = pattern
        .strip_prefix('^')
        .and_then(|body| body.strip_suffix('$'))
        .unwrap_or(&pattern);
    format!(
        "Value at '{path}' failed to satisfy constraint: Member must satisfy regular \
         expression pattern: {unanchored}"
    )
}

/// The clause this service states for a value outside an enum's value set.
fn enum_clause(path: &str, allowed: &[&str]) -> String {
    format!(
        "Value at '{path}' failed to satisfy constraint: Member must satisfy enum value \
         set: [{}]",
        allowed.join(", ")
    )
}

/// The `@pattern` the shipped Smithy model declares on `shape`.
///
/// Read out of the model at test time, so the expectation's source is the model
/// itself rather than a regex written here that could drift from it.
fn modelled_pattern(shape: &str) -> String {
    const MODEL_PATH: &str = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/smithy-model/com-amazonaws-healthlake-healthlake.json"
    );
    let model = std::fs::read_to_string(MODEL_PATH).expect("this crate ships its Smithy model");
    let model: Value = serde_json::from_str(&model).expect("the Smithy model is JSON");
    let shapes = model
        .get("shapes")
        .and_then(Value::as_object)
        .expect("a Smithy model has shapes");
    shapes
        .iter()
        .find(|(id, _)| id.rsplit('#').next() == Some(shape))
        .and_then(|(_, declared)| {
            declared
                .get("traits")?
                .get("smithy.api#pattern")?
                .as_str()
                .map(str::to_string)
        })
        .unwrap_or_else(|| panic!("the model declares a @pattern on {shape}"))
}
