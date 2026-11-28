// SPDX-License-Identifier: CC0-1.0

//! What a refusal of the data transformation surface QUOTES, and what it does
//! not: the value a constraint clause renders, the quota a publish ran out of, and
//! the resource a job start could not find.
//!
//! Three rules, each asserted over a SET of inputs rather than over one recorded
//! request:
//!
//! 1. **A `@length` refusal of a COLLECTION member renders no value.** A list or a
//!    map is never rendered as a value, so the clause states the member and the
//!    bound alone — `Value at '<member>' failed to satisfy constraint: Member must
//!    have length …` — while a member whose value IS a scalar keeps the framing
//!    that quotes the (length-capped) value it was sent.
//! 2. **A publish that runs out of version numbers states the quota.** The modelled
//!    `ProfileVersion` range is the bound a profile runs out of, and the
//!    `ServiceQuotaExceededException` names how many published versions a profile
//!    may hold, whichever version the publish was copying from.
//! 3. **The job surface words a missing profile its own way.** A
//!    `StartDataTransformationJob` naming a profile no record holds states the kind
//!    of resource and the identifier the request named, the template its own
//!    missing-job refusal uses, while the profile surface keeps its own wording for
//!    the same identifier.
//!
//! Every call goes through [`MultiServiceRouter::dispatch`][d] by way of the shared
//! [`Profiles`] harness — the same entry point `core/router` serves — so routing,
//! deserialization, the modelled constraints and the error envelope are all
//! exercised, not bypassed.
//!
//! Every modelled CLIENT error is asserted at the status [`wire_http_status`] gives
//! it, for the reason `tests/all/error_envelopes.rs` pins: awsJson1_0 binds nothing
//! to the transport, so this service reports an error's FAULT.
//!
//! [d]: smithy_server_runtime::MultiServiceRouter::dispatch

use serde_json::{json, Map, Value};
use service_com_amazonaws_healthlake_healthlake::scaffolded::wire_http_status;
use service_com_amazonaws_healthlake_healthlake::state::{
    profile_not_found_message, published_version_quota_message,
    transformation_profile_not_found_message, MAX_PROFILE_MAPPING_ENTRIES, MAX_PROFILE_VERSION,
    MIN_PROFILE_MAPPING_ENTRIES,
};

use crate::data_transformation_profile::{mapping, modelled_message, profile_mapping_source, Profiles};
use crate::fhir_datastore_lifecycle::{error_code, error_message, string_at};

/// A `ProfileMapping` of `entries` distinct files, as a request body carries one.
fn mapping_of(entries: usize) -> Value {
    let content: Map<String, Value> = (0..entries)
        .map(|index| {
            (
                format!("Resources/File{index}.vm"),
                Value::String("#set($x = 'content')".to_string()),
            )
        })
        .collect();
    Value::Object(content)
}

/// Create one profile and answer its `ProfileId`.
async fn profile(healthlake: &Profiles, case: &str, name: &str) -> String {
    let created = healthlake
        .ok(
            case,
            "CreateDataTransformationProfile",
            &json!({
                "ProfileName": name,
                "SourceFormat": "CCDA",
                "Source": profile_mapping_source(&mapping(&[("a.map", "Patient.id <- id")])),
            }),
        )
        .await;
    string_at(&created, "ProfileId")
}

/// One row of the constraint table: a request breaking exactly one `@length` the
/// model declares, and the clause its refusal must carry in full.
struct LengthCase {
    /// What this row varies, named in every assertion it makes.
    case: &'static str,
    operation: &'static str,
    request: Value,
    /// The dotted member path the clause names.
    path: &'static str,
    /// The requirement the clause states.
    requirement: String,
    /// The value the clause renders, or `None` for a collection, whose size is no
    /// value the caller sent.
    rendered: Option<String>,
}

impl LengthCase {
    /// The whole sentence this row's refusal must carry: the envelope, the rendered
    /// value where there is one, the member and the requirement.
    fn expected(&self) -> String {
        match &self.rendered {
            Some(value) => format!(
                "1 validation error detected: Value '{value}' at '{}' failed to satisfy \
                 constraint: {}",
                self.path, self.requirement
            ),
            None => format!(
                "1 validation error detected: Value at '{}' failed to satisfy constraint: {}",
                self.path, self.requirement
            ),
        }
    }
}

/// The requirement clause a `@length` maximum states, as the validation framework
/// words it.
fn at_most(bound: usize) -> String {
    format!("Member must have length less than or equal to {bound}")
}

/// The requirement clause a `@length` minimum states.
fn at_least(bound: usize) -> String {
    format!("Member must have length greater than or equal to {bound}")
}

/// RULE 1: a collection outside its `@length` is refused without a rendered value,
/// and a scalar outside its own `@length` still quotes what was sent.
///
/// The rows span the collection KIND (a map, a list), the BOUND broken (the maximum
/// and the minimum), the OPERATION and MEMBER carrying it (the profile content of
/// an update and of a create, the tag map of a create, the tag list of a tagging
/// call) and, as the complement, two scalar members whose refusal does quote the
/// value. Every bound is read from this crate's own constants or from the model, so
/// no limit is written out twice.
#[tokio::test]
async fn a_collection_length_refusal_renders_no_value() {
    let healthlake = Profiles::new();
    let profile_id = profile(&healthlake, "the profile the updates address", "length-wording").await;
    let datastore_arn = format!(
        "arn:aws:healthlake:us-east-1:{}:dataTransformationProfile/{profile_id}",
        engine::emulator_account_id()
    );
    // The tag bounds the model declares for the two tag members
    // (`smithy_query(query="operation",operation="TagResource")` -> `Tags` length
    // 0..200; `CreateDataTransformationProfile` -> `Tags` length 0..50).
    const MAX_TAG_LIST: usize = 200;
    const MAX_TAG_MAP: usize = 50;
    // `ClientToken` is length 1..64 and `ProfileNameString` length 1..256, from the
    // same model query: the two scalar members the complement rows below break, each
    // with a value short enough to be quoted in full rather than elided.
    const MAX_CLIENT_TOKEN: usize = 64;
    // `DataTransformationJobId` is length 1..32 over `^[a-zA-Z0-9-]+$`, so a longer
    // value of that alphabet breaks the length alone.
    const MAX_JOB_ID: usize = 32;

    let over_long_token = "t".repeat(MAX_CLIENT_TOKEN + 6);
    let over_long_job_id = "j".repeat(MAX_JOB_ID + 8);
    let rows: Vec<LengthCase> = vec![
        LengthCase {
            case: "a profile mapping one entry past the modelled maximum",
            operation: "UpdateDataTransformationProfile",
            request: json!({
                "ProfileId": &profile_id,
                "ProfileMapping": mapping_of(MAX_PROFILE_MAPPING_ENTRIES + 1),
            }),
            path: "profileMapping",
            requirement: at_most(MAX_PROFILE_MAPPING_ENTRIES),
            rendered: None,
        },
        LengthCase {
            case: "a profile mapping far past the modelled maximum",
            operation: "UpdateDataTransformationProfile",
            request: json!({
                "ProfileId": &profile_id,
                "ProfileMapping": mapping_of(MAX_PROFILE_MAPPING_ENTRIES * 2),
            }),
            path: "profileMapping",
            requirement: at_most(MAX_PROFILE_MAPPING_ENTRIES),
            rendered: None,
        },
        LengthCase {
            case: "a profile mapping below the modelled minimum",
            operation: "UpdateDataTransformationProfile",
            request: json!({
                "ProfileId": &profile_id,
                "ProfileMapping": mapping_of(0),
            }),
            path: "profileMapping",
            requirement: at_least(MIN_PROFILE_MAPPING_ENTRIES),
            rendered: None,
        },
        LengthCase {
            case: "a tag MAP one entry past its modelled maximum",
            operation: "CreateDataTransformationProfile",
            request: json!({
                "ProfileName": "over-tagged",
                "SourceFormat": "CCDA",
                "Source": profile_mapping_source(&mapping(&[("a.map", "Patient.id <- id")])),
                "Tags": tag_map(MAX_TAG_MAP + 1),
            }),
            path: "tags",
            requirement: at_most(MAX_TAG_MAP),
            rendered: None,
        },
        LengthCase {
            case: "a tag LIST one entry past its modelled maximum",
            operation: "TagResource",
            request: json!({
                "ResourceARN": &datastore_arn,
                "Tags": tag_list(MAX_TAG_LIST + 1),
            }),
            path: "tags",
            requirement: at_most(MAX_TAG_LIST),
            rendered: None,
        },
        LengthCase {
            case: "a scalar member past its modelled maximum, which IS quoted",
            operation: "CreateDataTransformationProfile",
            request: json!({
                "ProfileName": "over-tokened",
                "SourceFormat": "CCDA",
                "Source": profile_mapping_source(&mapping(&[("a.map", "Patient.id <- id")])),
                "ClientToken": &over_long_token,
            }),
            path: "clientToken",
            requirement: at_most(MAX_CLIENT_TOKEN),
            rendered: Some(over_long_token.clone()),
        },
        LengthCase {
            case: "a scalar member below its modelled minimum, whose empty value IS quoted",
            operation: "CreateDataTransformationProfile",
            request: json!({
                "ProfileName": "",
                "SourceFormat": "CCDA",
                "Source": profile_mapping_source(&mapping(&[("a.map", "Patient.id <- id")])),
            }),
            path: "profileName",
            requirement: at_least(1),
            rendered: Some(String::new()),
        },
        LengthCase {
            case: "another scalar member past its modelled maximum",
            operation: "DescribeDataTransformationJob",
            request: json!({ "JobId": &over_long_job_id }),
            path: "jobId",
            requirement: at_most(MAX_JOB_ID),
            rendered: Some(over_long_job_id.clone()),
        },
    ];

    for row in &rows {
        let (status, code, payload) = healthlake
            .refused_body(row.case, row.operation, &row.request)
            .await;
        assert_eq!(
            (status, code.as_str()),
            (wire_http_status(400), "ValidationException"),
            "case {:?}: the request must be refused as a constraint violation, got \
             {status}: {payload}",
            row.case
        );
        let expected = row.expected();
        assert_eq!(
            modelled_message(&payload),
            Some(expected.as_str()),
            "case {:?}: expected {expected:?}, got {payload}",
            row.case
        );
        if row.rendered.is_none() {
            assert!(
                !error_message(&payload).contains(" entries'")
                    && !error_message(&payload).contains(" items'"),
                "case {:?}: a collection's size is no value the caller sent, got {payload}",
                row.case
            );
        }
    }

    // The profile is untouched by every refusal above: its content is still the one
    // entry the create wrote, so nothing was stored past a bound.
    let read = healthlake
        .ok(
            "the profile the updates address",
            "GetDataTransformationProfile",
            &json!({ "ProfileId": &profile_id }),
        )
        .await;
    assert_eq!(
        read["ProfileMapping"]
            .as_object()
            .map(serde_json::Map::len),
        Some(1),
        "the refused updates must have stored nothing, got {read}"
    );
}

/// RULE 1, on the surface that raises its OWN collection violations: the
/// data-transformation endpoint's REST tagging surface reads its collections itself,
/// and a collection outside its `@length` there is stated the same way — the member
/// and the bound, with no value and no claim that the member was absent.
///
/// The rows differ along the collection (the tag MAP a merge carries, the tag KEY
/// LIST a removal carries) and the bound broken (the maximum of one, the minimum of
/// the other), so the rule is asserted across both of that surface's collections.
#[tokio::test]
async fn the_rest_tagging_surface_states_a_collection_bound_without_a_value() {
    // The bounds the modelled members declare, as `tests/all/rest_tagging_surface.rs`
    // reads them: `TagMap` and `TagKeyList` are both @length(min: 0|1, max: 50).
    const MAX_TAGS: usize = 50;
    const MIN_TAG_KEYS: usize = 1;

    let service = crate::fhir_rest_and_validate_source::HealthLake::new();
    let (status, created) = service
        .rpc(
            "CreateFHIRDatastore",
            &json!({ "DatastoreTypeVersion": "R4", "DatastoreName": "rest tag bounds" }),
        )
        .await;
    assert_eq!(
        status, 200,
        "the data store the rows tag must be created, got {status}: {created}"
    );
    let arn = string_at(&created, "DatastoreArn");
    // An `@httpLabel` is one path segment, so the ARN's delimiters are
    // percent-encoded on the way into the path.
    let path_arn = arn
        .replace('%', "%25")
        .replace(':', "%3A")
        .replace('/', "%2F");

    let many_keys: Vec<Value> = (0..=MAX_TAGS)
        .map(|index| Value::String(format!("key{index}")))
        .collect();
    let rows: [(&str, &str, Value, String, String); 3] = [
        (
            "a tag map one entry past its maximum",
            "tags",
            json!({ "Tags": tag_map(MAX_TAGS + 1) }),
            "tags".to_string(),
            at_most(MAX_TAGS),
        ),
        (
            "a tag key list one element past its maximum",
            "untag",
            json!({ "TagKeys": Value::Array(many_keys) }),
            "tagKeys".to_string(),
            at_most(MAX_TAGS),
        ),
        (
            "a tag key list below its minimum",
            "untag",
            json!({ "TagKeys": [] }),
            "tagKeys".to_string(),
            at_least(MIN_TAG_KEYS),
        ),
    ];

    for (case, segment, body, path, requirement) in rows {
        let (status, answered) = service
            .rest(
                "POST",
                &format!("/{segment}/{path_arn}"),
                "application/json",
                serde_json::to_vec(&body).expect("the request body serializes"),
            )
            .await;
        assert_eq!(
            (status, error_code(&answered).as_str()),
            (400, "ValidationException"),
            "case {case:?}: the request must be refused as a constraint violation, got \
             {status}: {answered}"
        );
        let expected = format!(
            "1 validation error detected: Value at '{path}' failed to satisfy constraint: \
             {requirement}"
        );
        assert_eq!(
            modelled_message(&answered),
            Some(expected.as_str()),
            "case {case:?}: expected {expected:?}, got {answered}"
        );
    }
}

/// A tag MAP of `entries` distinct keys, as `CreateDataTransformationProfile`
/// carries one.
fn tag_map(entries: usize) -> Value {
    Value::Object(
        (0..entries)
            .map(|index| (format!("key{index}"), Value::String(format!("value{index}"))))
            .collect(),
    )
}

/// A tag LIST of `entries` distinct tags, as `TagResource` carries one.
fn tag_list(entries: usize) -> Value {
    Value::Array(
        (0..entries)
            .map(|index| json!({ "Key": format!("key{index}"), "Value": format!("value{index}") }))
            .collect(),
    )
}

/// RULE 2: the publish that would pass the modelled version ceiling is refused as
/// the quota it is, stating how many published versions a profile may hold.
///
/// The rows differ along HOW the exhausting publish is spelled — copying the DRAFT
/// (the default) and republishing an already published version — because the quota
/// is about the version it would MINT and not about the one it copies from.
#[tokio::test]
async fn the_version_quota_refusal_states_the_ceiling() {
    for (case, from_existing_version) in [
        ("the publish that copies the DRAFT", None),
        ("the publish that republishes version 1", Some(1)),
    ] {
        let healthlake = Profiles::new();
        let content = mapping(&[("mapping-config.yaml", "tableName: patients")]);
        let created = healthlake
            .ok(
                case,
                "CreateDataTransformationProfile",
                &json!({
                    "ProfileName": "version-quota",
                    "SourceFormat": "CSV",
                    "Source": profile_mapping_source(&content),
                }),
            )
            .await;
        let profile_id = string_at(&created, "ProfileId");

        // The DRAFT is version 0, so the published versions run 1..=MAX_PROFILE_VERSION
        // and the next publish is the one past the ceiling.
        for version in 1..=MAX_PROFILE_VERSION {
            let published = healthlake
                .ok(
                    case,
                    "PublishDataTransformationProfile",
                    &json!({ "ProfileId": &profile_id, "SourceFormat": "CSV" }),
                )
                .await;
            assert_eq!(
                published["Version"].as_i64(),
                Some(i64::from(version)),
                "case {case:?}: publish number {version} is version {version}, got {published}"
            );
        }

        let mut request = json!({ "ProfileId": &profile_id, "SourceFormat": "CSV" });
        if let Some(from) = from_existing_version {
            request["FromExistingVersion"] = json!(from);
        }
        let (status, code, payload) = healthlake
            .refused_body(case, "PublishDataTransformationProfile", &request)
            .await;
        let expected = published_version_quota_message();
        assert_eq!(
            (status, code.as_str(), modelled_message(&payload)),
            (
                wire_http_status(400),
                "ServiceQuotaExceededException",
                Some(expected.as_str())
            ),
            "case {case:?}: expected {expected:?}, got {payload}"
        );
        assert!(
            expected.contains(&MAX_PROFILE_VERSION.to_string()),
            "case {case:?}: the sentence states the ceiling, got {expected:?}"
        );

        // The refused publish consumed no version: the latest is still the last one
        // published.
        let latest = healthlake
            .ok(
                case,
                "GetDataTransformationProfile",
                &json!({ "ProfileId": &profile_id }),
            )
            .await;
        assert_eq!(
            latest["Version"].as_i64(),
            Some(i64::from(MAX_PROFILE_VERSION)),
            "case {case:?}: the refused publish left the latest version where it was, got {latest}"
        );
    }
}

/// RULE 3: a job start naming a profile no record holds states the KIND of resource
/// it could not find and the identifier the request named; the profile surface
/// words the same identifier its own way.
///
/// The rows span the FORM of the identifier — ids differing in their digits, all
/// zeroes, all `f`s — and each is asserted on both surfaces, so the two templates
/// are pinned against each other rather than one being asserted alone.
#[tokio::test]
async fn the_job_surface_words_a_missing_profile_its_own_way() {
    let healthlake = Profiles::new();
    // One live profile, so the refusals below are about the profile asked for and
    // not about a surface that refuses everything.
    let live = profile(&healthlake, "the control profile", "job-not-found").await;

    let unheld: [&str; 4] = [
        "00000000000000000000000000000000",
        "ffffffffffffffffffffffffffffffff",
        "0123456789abcdef0123456789abcdef",
        "deadbeefdeadbeefdeadbeefdeadbeef",
    ];
    for (index, profile_id) in unheld.into_iter().enumerate() {
        assert_ne!(
            profile_id, live,
            "case {profile_id:?}: the row must not name the live profile"
        );
        let request = json!({
            "ProfileId": profile_id,
            "ClientToken": format!("token-missing-{index}"),
            "JobName": format!("job-missing-{index}"),
            "DataAccessRoleArn": "arn:aws:iam::111122223333:role/HealthLakeTransform",
            "InputDataConfig": { "S3Uri": "s3://transform-in-bucket" },
            "OutputDataConfig": { "S3Configuration": {
                "S3Uri": "s3://transform-out-bucket",
                "KmsKeyId": "1234abcd-12ab-34cd-56ef-1234567890ab",
            } },
        });
        let (status, code, payload) = healthlake
            .refused_body(profile_id, "StartDataTransformationJob", &request)
            .await;
        let expected = transformation_profile_not_found_message(profile_id);
        assert_eq!(
            (status, code.as_str(), modelled_message(&payload)),
            (
                wire_http_status(404),
                "ResourceNotFoundException",
                Some(expected.as_str())
            ),
            "case {profile_id:?}: expected {expected:?}, got {payload}"
        );

        // The profile surface answers the SAME identifier in its own wording, so the
        // job surface's sentence is this operation's and not a crate-wide constant.
        let (_status, _code, read) = healthlake
            .refused_body(
                profile_id,
                "GetDataTransformationProfile",
                &json!({ "ProfileId": profile_id }),
            )
            .await;
        let profile_surface = profile_not_found_message(profile_id);
        assert_eq!(
            modelled_message(&read),
            Some(profile_surface.as_str()),
            "case {profile_id:?}: expected the profile surface's own {profile_surface:?}, \
             got {read}"
        );
        assert_ne!(
            expected, profile_surface,
            "case {profile_id:?}: the two surfaces word the refusal differently"
        );
    }

    // The live profile still starts a job, so the refusals were about identity.
    let started = healthlake
        .ok(
            "the control profile",
            "StartDataTransformationJob",
            &json!({
                "ProfileId": &live,
                "ClientToken": "token-live",
                "JobName": "job-live",
                "DataAccessRoleArn": "arn:aws:iam::111122223333:role/HealthLakeTransform",
                "InputDataConfig": { "S3Uri": "s3://transform-in-bucket" },
                "OutputDataConfig": { "S3Configuration": {
                    "S3Uri": "s3://transform-out-bucket",
                    "KmsKeyId": "1234abcd-12ab-34cd-56ef-1234567890ab",
                } },
            }),
        )
        .await;
    assert_eq!(
        started["JobStatus"].as_str(),
        Some("SUBMITTED"),
        "the live profile must start a job, got {started}"
    );
}
