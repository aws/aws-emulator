// SPDX-License-Identifier: CC0-1.0

//! End-to-end behaviour of `DeleteDataTransformationProfile` over the real
//! awsJson1_0 wire, read back through every operation that reports a profile.
//!
//! The rule under test: a delete removes the whole data transformation profile the
//! call names — its DRAFT (version 0) and every version published from it — reports
//! that profile's id and stored name with the instant it went, and leaves every
//! other profile alone; afterwards nothing about that profile is readable or
//! writable, so every operation that addresses it answers this service's fixed
//! profile not-found refusal, and the slot it held in the store is given back. A
//! profile no record holds — including one an earlier call already deleted — is
//! refused with the same text and nothing else is touched.
//!
//! The rows differ along what the rule generalises over: the source format, the
//! stored name's length at both ends of the modelled `ProfileNameString` range, how
//! many and how large the DRAFT's mapping entries are, how many versions the profile
//! had published (none, one, several), and whether it carried tags.
//!
//! Every call below goes through the shared [`Profiles`] harness, i.e.
//! `MultiServiceRouter::dispatch` — the same entry point `core/router` serves — so
//! routing, deserialization, the modelled constraints and the error envelope are all
//! exercised, not bypassed.

use std::collections::BTreeMap;

use serde_json::{json, Map, Value};
use service_com_amazonaws_healthlake_healthlake::scaffolded::wire_http_status;
use service_com_amazonaws_healthlake_healthlake::state::MAX_PROFILES;

use crate::data_transformation_profile::{
    assert_well_formed_profile_id, mapping, mapping_of, modelled_message, profile_mapping_source,
    reported_mapping, Profiles,
};
use crate::fhir_datastore_lifecycle::string_at;

/// A `ProfileMapping` request member from stored content.
///
/// `pub(crate)` so the sibling post-delete wording suite writes one through the same
/// helper rather than keeping a second copy of it.
pub(crate) fn request_mapping(content: &BTreeMap<String, String>) -> Value {
    let entries: Map<String, Value> = content
        .iter()
        .map(|(path, entry)| (path.clone(), Value::String(entry.clone())))
        .collect();
    Value::Object(entries)
}

/// A timestamp member as the awsJson wire carries it: epoch seconds.
fn timestamp_at(case: &str, body: &Value, member: &str) -> f64 {
    body[member]
        .as_f64()
        .unwrap_or_else(|| panic!("case {case:?}: response must carry a {member}: {body}"))
}

/// One deletion row: the profile to build, how many versions to publish from it,
/// and the tags it carries.
///
/// `pub(crate)` with [`deletion_cases`] and [`build_profile`], so the sibling suite
/// that asserts how the post-delete refusals are WORDED drives the same rows through
/// the same builder instead of restating the table and the create-publish-delete walk.
pub(crate) struct Deletion {
    pub(crate) case: &'static str,
    pub(crate) source_format: &'static str,
    pub(crate) profile_name: String,
    pub(crate) initial: BTreeMap<String, String>,
    pub(crate) publishes: usize,
    pub(crate) tags: Option<BTreeMap<String, String>>,
}

/// The deletion table. The modelled bounds the rows sit on come from the operation's
/// own shapes: `ProfileNameString` is len 1..256, `ProfileMapping` holds 1..500
/// entries with keys 1..500 characters and values 0..102400, and `ProfileVersion`
/// runs 0..99, so a profile with several published versions is well inside it.
pub(crate) fn deletion_cases() -> Vec<Deletion> {
    let longest_name = "n".repeat(256);
    let long_key = "d".repeat(500);
    vec![
        Deletion {
            case: "CSV, a DRAFT only, no published version and no tags",
            source_format: "CSV",
            profile_name: "delete-draft-only".to_string(),
            initial: mapping(&[("mapping-config.yaml", "tableName: patients")]),
            publishes: 0,
            tags: None,
        },
        Deletion {
            case: "CCDA, one published version, tagged",
            source_format: "CCDA",
            profile_name: "delete-one-version".to_string(),
            initial: mapping(&[
                ("sections/problems.map", "Condition.code <- problem/value"),
                ("sections/vitals.map", "Observation.value[x] <- vital/value"),
            ]),
            publishes: 1,
            tags: Some(BTreeMap::from([
                ("env".to_string(), "test".to_string()),
                ("team".to_string(), "clinical-data".to_string()),
            ])),
        },
        Deletion {
            case: "CCDA, four published versions, a name at the modelled 1-character minimum",
            source_format: "CCDA",
            profile_name: "d".to_string(),
            initial: mapping_of(12),
            publishes: 4,
            tags: Some(BTreeMap::from([("only".to_string(), String::new())])),
        },
        Deletion {
            case: "CCDA, a mapping at the modelled 500-entry maximum, two published versions",
            source_format: "CCDA",
            profile_name: longest_name,
            initial: mapping_of(500),
            publishes: 2,
            tags: None,
        },
        Deletion {
            case: "CCDA, a large value and a key at the modelled maximum with an empty value",
            source_format: "CCDA",
            profile_name: "delete-large-content".to_string(),
            initial: BTreeMap::from([
                (long_key, String::new()),
                ("sections/notes.map".to_string(), "v".repeat(100_000)),
            ]),
            publishes: 1,
            tags: Some(BTreeMap::from([("size".to_string(), "large".to_string())])),
        },
    ]
}

/// The create request one row builds its profile from.
fn create_request(row: &Deletion) -> Value {
    let mut request = json!({
        "ProfileName": row.profile_name,
        "ProfileDescription": "the profile a delete removes whole",
        "SourceFormat": row.source_format,
        "Source": profile_mapping_source(&row.initial),
    });
    if let Some(tags) = &row.tags {
        request["Tags"] = Value::Object(
            tags.iter()
                .map(|(key, value)| (key.clone(), Value::String(value.clone())))
                .collect(),
        );
    }
    request
}

/// Build one row's profile and return `(ProfileId, the versions it holds)`: version
/// 0, the DRAFT, plus one per publish.
pub(crate) async fn build_profile(healthlake: &Profiles, row: &Deletion) -> (String, Vec<i64>) {
    let case = row.case;
    let created = healthlake
        .ok(case, "CreateDataTransformationProfile", &create_request(row))
        .await;
    let profile_id = string_at(&created, "ProfileId");
    assert_well_formed_profile_id(case, &profile_id);

    let mut versions = vec![0i64];
    for publish in 0..row.publishes {
        let published = healthlake
            .ok(
                case,
                "PublishDataTransformationProfile",
                &json!({
                    "ProfileId": profile_id,
                    "SourceFormat": row.source_format,
                    "ChangeDescription": format!("release {publish}"),
                }),
            )
            .await;
        versions.push(published["Version"].as_i64().unwrap_or_else(|| {
            panic!("case {case:?}: a publish reports the Version it wrote: {published}")
        }));
    }
    (profile_id, versions)
}

/// The ARN of the profile `profile_id`, built from an ARN this emulator minted so no
/// test names a partition, region or account of its own.
async fn profile_arn(healthlake: &Profiles, profile_id: &str) -> String {
    let created = healthlake
        .ok(
            "arn",
            "CreateFHIRDatastore",
            &json!({ "DatastoreTypeVersion": "R4", "DatastoreName": "arn-source" }),
        )
        .await;
    crate::resource_tagging::profile_arn(&string_at(&created, "DatastoreArn"), profile_id)
}

/// Assert `request` on `operation` is refused with this service's profile
/// not-found answer, naming what the call could not find.
async fn assert_profile_not_found(
    healthlake: &Profiles,
    case: &str,
    operation: &str,
    request: &Value,
) {
    let (status, code, payload) = healthlake.refused_body(case, operation, request).await;
    let expected = crate::data_transformation_profile::expected_profile_not_found(
        operation, request,
    );
    assert_eq!(
        (status, code.as_str(), modelled_message(&payload)),
        (
            wire_http_status(404),
            "ResourceNotFoundException",
            Some(expected.as_str())
        ),
        "case {case:?}: {operation} on a profile no record holds is refused with the \
         sentence naming it, got {payload}"
    );
}

/// Every row's delete reports the profile it removed and leaves nothing of it
/// readable: not its DRAFT, not any version published from it, and not its tags.
#[tokio::test]
async fn delete_removes_the_profile_and_every_version_of_it() {
    let healthlake = Profiles::new();

    for row in deletion_cases() {
        let case = row.case;
        let (profile_id, versions) = build_profile(&healthlake, &row).await;
        let arn = profile_arn(&healthlake, &profile_id).await;

        // Every version is readable before the delete, so the delete below is shown to
        // have removed something.
        for version in &versions {
            let read = healthlake
                .ok(
                    case,
                    "GetDataTransformationProfile",
                    &json!({ "ProfileId": profile_id, "ProfileVersion": version }),
                )
                .await;
            assert_eq!(
                read["Version"].as_i64(),
                Some(*version),
                "case {case:?}: version {version} is readable before the delete, got {read}"
            );
            assert_eq!(
                reported_mapping(case, &read),
                row.initial,
                "case {case:?}: version {version} holds the content it was built from"
            );
        }
        let before = healthlake
            .ok(case, "ListTagsForResource", &json!({ "ResourceARN": arn }))
            .await;
        assert_eq!(
            crate::list_tags_for_resource::reported_tags(case, &before),
            row.tags.clone().unwrap_or_default(),
            "case {case:?}: the profile's tags are readable before the delete"
        );

        let deleted = healthlake
            .ok(
                case,
                "DeleteDataTransformationProfile",
                &json!({ "ProfileId": profile_id }),
            )
            .await;
        assert_eq!(
            deleted["ProfileId"].as_str(),
            Some(profile_id.as_str()),
            "case {case:?}: the delete reports the profile it was asked for, got {deleted}"
        );
        assert_eq!(
            deleted["ProfileName"].as_str(),
            Some(row.profile_name.as_str()),
            "case {case:?}: the delete reports the deleted profile's stored name, got {deleted}"
        );
        let deletion_time = timestamp_at(case, &deleted, "DeletionTime");
        assert!(
            deletion_time > 0.0,
            "case {case:?}: expected a DeletionTime after the epoch, got {deletion_time}"
        );

        // Nothing of the profile survives: no version of it, by number or by the
        // omitted-version form, and no tag set.
        for version in &versions {
            assert_profile_not_found(
                &healthlake,
                case,
                "GetDataTransformationProfile",
                &json!({ "ProfileId": profile_id, "ProfileVersion": version }),
            )
            .await;
        }
        assert_profile_not_found(
            &healthlake,
            case,
            "GetDataTransformationProfile",
            &json!({ "ProfileId": profile_id }),
        )
        .await;
        assert_profile_not_found(
            &healthlake,
            case,
            "ListTagsForResource",
            &json!({ "ResourceARN": arn }),
        )
        .await;
        // Nor can it be written: an update, a publish or a second delete all find no
        // record at all.
        assert_profile_not_found(
            &healthlake,
            case,
            "UpdateDataTransformationProfile",
            &json!({
                "ProfileId": profile_id,
                "ProfileMapping": request_mapping(&mapping(&[("after.map", "Patient.id <- id")])),
            }),
        )
        .await;
        assert_profile_not_found(
            &healthlake,
            case,
            "PublishDataTransformationProfile",
            &json!({ "ProfileId": profile_id, "SourceFormat": row.source_format }),
        )
        .await;
        assert_profile_not_found(
            &healthlake,
            case,
            "DeleteDataTransformationProfile",
            &json!({ "ProfileId": profile_id }),
        )
        .await;
        assert_profile_not_found(
            &healthlake,
            case,
            "TagResource",
            &json!({
                "ResourceARN": arn,
                "Tags": crate::resource_tagging::tag_list(&[("after", "delete")]),
            }),
        )
        .await;
    }
}

/// A delete removes the ONE profile it names: every other stored profile keeps its
/// own content, its own versions and its own tags.
#[tokio::test]
async fn delete_removes_only_the_profile_it_names() {
    let healthlake = Profiles::new();
    let case = "three profiles, the middle one deleted";
    let mut stored: Vec<(String, BTreeMap<String, String>)> = Vec::new();

    for index in 0..3u32 {
        let content = mapping(&[(
            "mapping-config.yaml",
            format!("tableName: patients_{index}").as_str(),
        )]);
        let created = healthlake
            .ok(
                case,
                "CreateDataTransformationProfile",
                &json!({
                    "ProfileName": format!("neighbour-{index}"),
                    "SourceFormat": "CSV",
                    "Source": profile_mapping_source(&content),
                    "Tags": { "index": format!("{index}") },
                }),
            )
            .await;
        let profile_id = string_at(&created, "ProfileId");
        healthlake
            .ok(
                case,
                "PublishDataTransformationProfile",
                &json!({ "ProfileId": profile_id, "SourceFormat": "CSV" }),
            )
            .await;
        stored.push((profile_id, content));
    }

    let (target_id, _) = stored.remove(1);
    healthlake
        .ok(
            case,
            "DeleteDataTransformationProfile",
            &json!({ "ProfileId": target_id }),
        )
        .await;

    for (profile_id, expected) in &stored {
        for version in [0, 1] {
            let read = healthlake
                .ok(
                    case,
                    "GetDataTransformationProfile",
                    &json!({ "ProfileId": profile_id, "ProfileVersion": version }),
                )
                .await;
            assert_eq!(
                &reported_mapping(case, &read),
                expected,
                "case {case:?}: profile {profile_id} version {version} survives the delete"
            );
        }
        let arn = profile_arn(&healthlake, profile_id).await;
        let tags = healthlake
            .ok(case, "ListTagsForResource", &json!({ "ResourceARN": arn }))
            .await;
        assert!(
            !crate::list_tags_for_resource::reported_tags(case, &tags).is_empty(),
            "case {case:?}: profile {profile_id} keeps its tags after the delete, got {tags}"
        );
    }
    assert_profile_not_found(
        &healthlake,
        case,
        "GetDataTransformationProfile",
        &json!({ "ProfileId": target_id, "ProfileVersion": 0 }),
    )
    .await;
}

/// Every delete this service refuses, and what each is refused with. `None` for the
/// expected code is a row refused by the modelled constraints at the boundary, where
/// only the status is this slice's to assert: the modelled `ProfileIdString` is
/// exactly 32 characters matching `^[a-f0-9]{32}$`, so anything else never reaches
/// the handler.
fn refusal_cases() -> Vec<(&'static str, Value, u16, Option<&'static str>)> {
    vec![
        (
            "a well-formed ProfileId no profile has",
            json!({ "ProfileId": "ffffffffffffffffffffffffffffffff" }),
            wire_http_status(404),
            Some("ResourceNotFoundException"),
        ),
        (
            "a ProfileId of 32 characters that are not all hexadecimal",
            json!({ "ProfileId": "zzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzz" }),
            400,
            None,
        ),
        (
            "a ProfileId one character short of the modelled 32",
            json!({ "ProfileId": "f".repeat(31) }),
            400,
            None,
        ),
        (
            "a ProfileId one character past the modelled 32",
            json!({ "ProfileId": "f".repeat(33) }),
            400,
            None,
        ),
        (
            "an uppercase ProfileId, outside the modelled pattern",
            json!({ "ProfileId": "FFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFF" }),
            400,
            None,
        ),
        (
            "no ProfileId at all, though the model requires it",
            json!({}),
            400,
            None,
        ),
    ]
}

/// Every refused delete is refused with the modelled status and removes nothing, so
/// the stored profile is still readable afterwards.
#[tokio::test]
async fn refused_deletes_remove_nothing() {
    let healthlake = Profiles::new();
    let content = mapping(&[("sections/problems.map", "Condition.code <- problem/value")]);
    let created = healthlake
        .ok(
            "refusals",
            "CreateDataTransformationProfile",
            &json!({
                "ProfileName": "delete-refusals",
                "SourceFormat": "CCDA",
                "Source": profile_mapping_source(&content),
            }),
        )
        .await;
    let profile_id = string_at(&created, "ProfileId");

    for (case, request, expected_status, expected_code) in refusal_cases() {
        let (status, code, payload) = healthlake
            .refused_body(case, "DeleteDataTransformationProfile", &request)
            .await;
        assert_eq!(
            status, expected_status,
            "case {case:?}: expected HTTP {expected_status}, got {status}: {payload}"
        );
        if let Some(expected_code) = expected_code {
            assert_eq!(
                code.as_str(),
                expected_code,
                "case {case:?}: expected {expected_code}, got {payload}"
            );
            assert_eq!(
                modelled_message(&payload),
                Some(
                    crate::data_transformation_profile::expected_profile_not_found(
                        "DeleteDataTransformationProfile",
                        &request
                    )
                    .as_str()
                ),
                "case {case:?}: an unknown profile is named in the not-found sentence, \
                 got {payload}"
            );
        }

        let read = healthlake
            .ok(
                case,
                "GetDataTransformationProfile",
                &json!({ "ProfileId": profile_id, "ProfileVersion": 0 }),
            )
            .await;
        assert_eq!(
            reported_mapping(case, &read),
            content,
            "case {case:?}: a refused delete removes nothing"
        );
    }

    // A delete still works after every refusal above.
    let case = "a delete after the refusals";
    healthlake
        .ok(
            case,
            "DeleteDataTransformationProfile",
            &json!({ "ProfileId": profile_id }),
        )
        .await;
    assert_profile_not_found(
        &healthlake,
        case,
        "GetDataTransformationProfile",
        &json!({ "ProfileId": profile_id, "ProfileVersion": 0 }),
    )
    .await;
}

/// A deleted profile gives its slot back: with the store full, a create is refused as
/// the quota it is, and the create that follows a delete succeeds.
#[tokio::test]
async fn a_delete_gives_the_profile_slot_back() {
    let healthlake = Profiles::new();
    let case = "a store filled to its profile ceiling";
    let mut ids = Vec::new();
    for index in 0..MAX_PROFILES {
        let created = healthlake
            .ok(
                case,
                "CreateDataTransformationProfile",
                &json!({
                    "ProfileName": format!("filler-{index:04}"),
                    "SourceFormat": "CSV",
                    "Source": profile_mapping_source(&mapping(&[(
                        "patients.csv.map",
                        "Patient.id <- patient_id",
                    )])),
                }),
            )
            .await;
        ids.push(string_at(&created, "ProfileId"));
    }

    let one_more = json!({
        "ProfileName": "one-past-the-ceiling",
        "SourceFormat": "CSV",
        "Source": profile_mapping_source(&mapping(&[("a.map", "Patient.id <- id")])),
    });
    let (status, code) = healthlake
        .refused(case, "CreateDataTransformationProfile", &one_more)
        .await;
    assert_eq!(
        (status, code.as_str()),
        (400, "ServiceQuotaExceededException"),
        "case {case:?}: a create past the {MAX_PROFILES}-profile ceiling is refused as a quota"
    );

    healthlake
        .ok(
            case,
            "DeleteDataTransformationProfile",
            &json!({ "ProfileId": ids[0] }),
        )
        .await;
    let created = healthlake
        .ok(case, "CreateDataTransformationProfile", &one_more)
        .await;
    assert_well_formed_profile_id(case, &string_at(&created, "ProfileId"));
}
