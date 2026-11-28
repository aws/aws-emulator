// SPDX-License-Identifier: CC0-1.0

//! End-to-end behaviour of `PublishDataTransformationProfile` over the real
//! awsJson1_0 wire, read back through `GetDataTransformationProfile`.
//!
//! The rule under test: a publish SNAPSHOTS one version of a profile into a NEW
//! immutable version, numbered one past the profile's latest published version (so
//! the first publish is version 1), carrying exactly the content of the version it
//! was taken from — the DRAFT when the call omits `FromExistingVersion`, the version
//! that member names otherwise — plus that call's own `ChangeDescription`. The
//! version it copied from is left as it was, every version published earlier keeps
//! its own content, and the profile's metadata does not move. A profile or a version
//! no record holds is refused, and so is a profile that has run out of the modelled
//! `ProfileVersion` range or of the bytes this emulator retains for one profile —
//! before anything is written, so a refused publish publishes nothing. `SourceFormat`
//! names the format the published version is read as, and what it is checked against
//! is the SHAPE of the content the call would snapshot, never the format the profile
//! was created with.
//!
//! The rows differ along what the rule generalises over: the source format, how many
//! entries the published content has and how large they are, whether the call carries
//! a `ChangeDescription`, how many successive publishes a profile has had, and
//! whether the content being published is the DRAFT or an older published version.
//!
//! Every call below goes through the shared [`Profiles`] harness, i.e.
//! `MultiServiceRouter::dispatch` — the same entry point `core/router` serves — so
//! routing, deserialization, the modelled constraints and the error envelope are all
//! exercised, not bypassed.

use std::collections::BTreeMap;

use serde_json::{json, Map, Value};
use service_com_amazonaws_healthlake_healthlake::scaffolded::wire_http_status;
use service_com_amazonaws_healthlake_healthlake::state::{
    csv_profile_mapping_message, profile_version_not_found_message, CSV_PROFILE_MAPPING_FILE,
    MAX_PROFILE_RETAINED_BYTES, MAX_PROFILE_VERSION,
};

use crate::data_transformation_profile::{
    assert_well_formed_profile_id, mapping, mapping_of, modelled_message, profile_mapping_source,
    reported_mapping, Profiles,
};
use crate::fhir_datastore_lifecycle::string_at;

/// The `ProfileMapping` of an `UpdateDataTransformationProfile` request body.
///
/// `pub(crate)` so the sibling profile suites write a DRAFT through this one
/// helper rather than each growing its own copy.
pub(crate) fn request_mapping(content: &BTreeMap<String, String>) -> Value {
    let entries: Map<String, Value> = content
        .iter()
        .map(|(path, entry)| (path.clone(), Value::String(entry.clone())))
        .collect();
    Value::Object(entries)
}

/// How many BYTES of keys and values a mapping holds together — what this
/// emulator's per-profile retained-byte ceiling is measured in.
fn mapping_bytes(content: &BTreeMap<String, String>) -> usize {
    content
        .iter()
        .map(|(path, entry)| path.len() + entry.len())
        .sum()
}

/// A publish request, with the optional members present only when the row carries
/// them.
fn publish_request(
    profile_id: &str,
    source_format: &str,
    from_existing_version: Option<i32>,
    change_description: Option<&str>,
) -> Value {
    let mut request = json!({
        "ProfileId": profile_id,
        "SourceFormat": source_format,
    });
    if let Some(version) = from_existing_version {
        request["FromExistingVersion"] = json!(version);
    }
    if let Some(description) = change_description {
        request["ChangeDescription"] = json!(description);
    }
    request
}

/// A `LastUpdatedAt` timestamp as the awsJson wire carries it: epoch seconds.
fn last_updated_at(case: &str, body: &Value) -> f64 {
    body["LastUpdatedAt"]
        .as_f64()
        .unwrap_or_else(|| panic!("case {case:?}: response must carry a LastUpdatedAt: {body}"))
}

/// One publish row: the source format of the profile, and the DRAFT content plus
/// `ChangeDescription` of each successive publish the row makes.
struct PublishCase {
    case: &'static str,
    source_format: &'static str,
    /// The content the DRAFT holds for each publish, in order: the first is what the
    /// profile is created with, each later one is written over the DRAFT by an
    /// `UpdateDataTransformationProfile` before that publish.
    drafts: Vec<BTreeMap<String, String>>,
    /// The `ChangeDescription` each publish carries, `None` for one that omits it.
    descriptions: Vec<Option<String>>,
}

/// The publish table. The rule generalises over the source format, the size and
/// count of the published entries, whether a `ChangeDescription` is carried and how
/// many versions the profile has already published, so the rows differ along exactly
/// those.
fn publish_cases() -> Vec<PublishCase> {
    // The modelled `ProfileMappingKey` bound is 1..500 characters and the modelled
    // `ProfileMappingValue` bound is 0..102400, so a 500-character key with an empty
    // value is at a published edge.
    let long_key = "p".repeat(500);
    vec![
        PublishCase {
            case: "CSV, one publish of a one-entry DRAFT, with a change description",
            source_format: "CSV",
            drafts: vec![mapping(&[("mapping-config.yaml", "tableName: patients")])],
            descriptions: vec![Some("first release of the patient mapping".to_string())],
        },
        PublishCase {
            case: "CCDA, three successive publishes, the DRAFT replaced between each",
            source_format: "CCDA",
            drafts: vec![
                mapping(&[("sections/problems.map", "Condition.code <- problem/value")]),
                mapping_of(12),
                mapping(&[
                    ("sections/vitals.map", "Observation.value[x] <- vital/value"),
                    ("sections/results.map", "Observation <- result"),
                ]),
            ],
            descriptions: vec![
                Some("problems".to_string()),
                None,
                Some("vitals and results".to_string()),
            ],
        },
        PublishCase {
            case: "CSV, two publishes of the SAME DRAFT: a version per call, not per content",
            source_format: "CSV",
            drafts: vec![
                mapping(&[("mapping-config.yaml", "tableName: patients")]),
                mapping(&[("mapping-config.yaml", "tableName: patients")]),
            ],
            descriptions: vec![None, Some("republished unchanged".to_string())],
        },
        PublishCase {
            case: "CCDA, a publish of the modelled 500-entry maximum",
            source_format: "CCDA",
            drafts: vec![mapping_of(500)],
            descriptions: vec![Some("c".repeat(1000))],
        },
        PublishCase {
            case: "CCDA, a large value and a key at the modelled maximum with an empty value",
            source_format: "CCDA",
            drafts: vec![
                BTreeMap::from([
                    ("notes.ccda.map".to_string(), "v".repeat(100_000)),
                    (
                        "sections/patients.map".to_string(),
                        "Patient.id <- patient/id".to_string(),
                    ),
                ]),
                BTreeMap::from([(long_key.clone(), String::new())]),
            ],
            descriptions: vec![Some(String::new()), None],
        },
    ]
}

/// Every row's publishes each snapshot the DRAFT into the next version, the modelled
/// reader reports every published version back with the content and description its
/// own publish gave it, and an omitted `ProfileVersion` resolves to the newest one.
/// The DRAFT is untouched by a publish.
#[tokio::test]
async fn publish_snapshots_the_draft_into_the_next_version() {
    let healthlake = Profiles::new();

    for row in publish_cases() {
        let case = row.case;
        let created = healthlake
            .ok(
                case,
                "CreateDataTransformationProfile",
                &json!({
                    "ProfileName": format!("publish-{}", row.source_format.to_lowercase()),
                    "ProfileDescription": "the profile metadata a publish leaves alone",
                    "SourceFormat": row.source_format,
                    "Source": profile_mapping_source(&row.drafts[0]),
                }),
            )
            .await;
        let profile_id = string_at(&created, "ProfileId");
        assert_well_formed_profile_id(case, &profile_id);

        // What every version published so far must still report: its content and the
        // description the publish that created it carried.
        let mut published: Vec<(i32, BTreeMap<String, String>, Option<&str>)> = Vec::new();

        for (step, draft) in row.drafts.iter().enumerate() {
            if step > 0 {
                healthlake
                    .ok(
                        case,
                        "UpdateDataTransformationProfile",
                        &json!({
                            "ProfileId": profile_id,
                            "ProfileMapping": request_mapping(draft),
                        }),
                    )
                    .await;
            }

            let description = row.descriptions[step].as_deref();
            let answered = healthlake
                .ok(
                    case,
                    "PublishDataTransformationProfile",
                    &publish_request(&profile_id, row.source_format, None, description),
                )
                .await;

            let expected_version = i64::try_from(step).expect("the step count fits") + 1;
            assert_eq!(
                answered["Version"].as_i64(),
                Some(expected_version),
                "case {case:?}: publish {step} must answer version {expected_version}, got {answered}"
            );
            assert_eq!(
                answered["ProfileId"].as_str(),
                Some(profile_id.as_str()),
                "case {case:?}: the publish reports the profile it was asked for, got {answered}"
            );
            assert_eq!(
                answered["SourceFormat"].as_str(),
                Some(row.source_format),
                "case {case:?}: the publish reports the profile's stored SourceFormat, got {answered}"
            );
            assert_eq!(
                answered["TargetFormat"], created["TargetFormat"],
                "case {case:?}: the publish reports the profile's stored TargetFormat, got {answered}"
            );
            assert_eq!(
                answered["ProfileName"], created["ProfileName"],
                "case {case:?}: a publish does not rename the profile, got {answered}"
            );
            let answered_at = last_updated_at(case, &answered);
            assert!(
                answered_at >= last_updated_at(case, &created),
                "case {case:?}: expected LastUpdatedAt at or after the create's, got {answered_at}"
            );

            let version = i32::try_from(expected_version).expect("the version fits");
            published.push((version, draft.clone(), description));

            // The version the publish created, read back through the modelled reader.
            let read = healthlake
                .ok(
                    case,
                    "GetDataTransformationProfile",
                    &json!({ "ProfileId": profile_id, "ProfileVersion": version }),
                )
                .await;
            assert_eq!(
                last_updated_at(case, &read),
                answered_at,
                "case {case:?}: the reader reports the instant the publish stamped, got {read}"
            );

            // An omitted `ProfileVersion` resolves to the latest published version,
            // which is the one this publish just created.
            let latest = healthlake
                .ok(
                    case,
                    "GetDataTransformationProfile",
                    &json!({ "ProfileId": profile_id }),
                )
                .await;
            assert_eq!(
                latest["Version"].as_i64(),
                Some(expected_version),
                "case {case:?}: the latest published version is the newest one, got {latest}"
            );
            assert_eq!(
                reported_mapping(case, &latest),
                draft.clone(),
                "case {case:?}: the latest published version carries the DRAFT's content"
            );

            // Every version published so far still carries exactly what its own
            // publish snapshotted: a published version is immutable.
            for (earlier, content, earlier_description) in &published {
                let body = healthlake
                    .ok(
                        case,
                        "GetDataTransformationProfile",
                        &json!({ "ProfileId": profile_id, "ProfileVersion": earlier }),
                    )
                    .await;
                assert_eq!(
                    &reported_mapping(case, &body),
                    content,
                    "case {case:?}: version {earlier} still carries the content it was published with"
                );
                assert_eq!(
                    body["ChangeDescription"],
                    earlier_description.map_or(Value::Null, |text| json!(text)),
                    "case {case:?}: version {earlier} carries its own ChangeDescription, got {body}"
                );
                assert_eq!(
                    body["ProfileName"], created["ProfileName"],
                    "case {case:?}: version {earlier} reports the profile's stored name, got {body}"
                );
                assert_eq!(
                    body["ProfileDescription"].as_str(),
                    Some("the profile metadata a publish leaves alone"),
                    "case {case:?}: a publish leaves the ProfileDescription alone, got {body}"
                );
            }

            // The DRAFT is the version the publish COPIED, so it still holds that
            // content — and no publish stamps it with a change description, because
            // the update that wrote it carried none.
            let draft_read = healthlake
                .ok(
                    case,
                    "GetDataTransformationProfile",
                    &json!({ "ProfileId": profile_id, "ProfileVersion": 0 }),
                )
                .await;
            assert_eq!(
                &reported_mapping(case, &draft_read),
                draft,
                "case {case:?}: the DRAFT still holds the content the publish copied"
            );
            assert!(
                draft_read["ChangeDescription"].is_null(),
                "case {case:?}: a publish stamps nothing on the DRAFT, got {draft_read}"
            );
        }
    }
}

/// `FromExistingVersion` republishes a version the profile already holds as its new
/// latest version — the rollback case — leaving the version it copied, and the DRAFT,
/// exactly as they were. Version 0 names the DRAFT, which is what an omitted member
/// resolves to.
#[tokio::test]
async fn publishing_from_an_existing_version_rolls_the_content_forward() {
    let healthlake = Profiles::new();
    let case = "a rollback by republishing an earlier version";
    let first = mapping(&[("sections/patients.map", "Patient.id <- patient/id")]);
    let second = mapping(&[
        ("sections/patients.map", "Patient.id <- patient/mrn"),
        ("sections/encounters.map", "Encounter.id <- encounter/id"),
    ]);

    let created = healthlake
        .ok(
            case,
            "CreateDataTransformationProfile",
            &json!({
                "ProfileName": "publish-rollback",
                "SourceFormat": "CCDA",
                "Source": profile_mapping_source(&first),
            }),
        )
        .await;
    let profile_id = string_at(&created, "ProfileId");

    // Version 1 holds the created DRAFT.
    healthlake
        .ok(
            case,
            "PublishDataTransformationProfile",
            &publish_request(&profile_id, "CCDA", None, Some("v1")),
        )
        .await;
    // Version 2 holds the replaced DRAFT.
    healthlake
        .ok(
            case,
            "UpdateDataTransformationProfile",
            &json!({
                "ProfileId": profile_id,
                "ProfileMapping": request_mapping(&second),
            }),
        )
        .await;
    healthlake
        .ok(
            case,
            "PublishDataTransformationProfile",
            &publish_request(&profile_id, "CCDA", None, Some("v2")),
        )
        .await;

    // The rollback: republish version 1 as the new latest version.
    let rolled_back = healthlake
        .ok(
            case,
            "PublishDataTransformationProfile",
            &publish_request(&profile_id, "CCDA", Some(1), Some("rolled back to v1")),
        )
        .await;
    assert_eq!(
        rolled_back["Version"].as_i64(),
        Some(3),
        "case {case:?}: a rollback publishes a NEW version, got {rolled_back}"
    );

    // Explicit version 0 is the DRAFT, the same content an omitted member publishes.
    let from_draft = healthlake
        .ok(
            case,
            "PublishDataTransformationProfile",
            &publish_request(&profile_id, "CCDA", Some(0), None),
        )
        .await;
    assert_eq!(
        from_draft["Version"].as_i64(),
        Some(4),
        "case {case:?}: publishing the DRAFT explicitly is the next version, got {from_draft}"
    );

    // Every version holds the content it was published with, the rollback included.
    let expected: [(i32, &BTreeMap<String, String>, Option<&str>); 5] = [
        (0, &second, None),
        (1, &first, Some("v1")),
        (2, &second, Some("v2")),
        (3, &first, Some("rolled back to v1")),
        (4, &second, None),
    ];
    for (version, content, description) in expected {
        let body = healthlake
            .ok(
                case,
                "GetDataTransformationProfile",
                &json!({ "ProfileId": profile_id, "ProfileVersion": version }),
            )
            .await;
        assert_eq!(
            &reported_mapping(case, &body),
            content,
            "case {case:?}: version {version} carries the content it was published with"
        );
        assert_eq!(
            body["ChangeDescription"],
            description.map_or(Value::Null, |text| json!(text)),
            "case {case:?}: version {version} carries its own ChangeDescription, got {body}"
        );
    }

    let latest = healthlake
        .ok(
            case,
            "GetDataTransformationProfile",
            &json!({ "ProfileId": profile_id }),
        )
        .await;
    assert_eq!(
        latest["Version"].as_i64(),
        Some(4),
        "case {case:?}: the latest published version is the newest one, got {latest}"
    );
}

/// The DRAFT a refused publish must leave unpublished and untouched.
const UNTOUCHED: &[(&str, &str)] = &[("sections/problems.map", "Condition.code <- problem/value")];

/// Every publish this service refuses, and what each is refused with. `None` for the
/// expected code is a row refused by the modelled constraints at the boundary, where
/// only the status is this slice's to assert.
fn refusal_cases(profile_id: &str) -> Vec<(&'static str, Value, u16, Option<&'static str>)> {
    vec![
        (
            "a well-formed ProfileId no profile has",
            publish_request("ffffffffffffffffffffffffffffffff", "CCDA", None, None),
            wire_http_status(404),
            Some("ResourceNotFoundException"),
        ),
        (
            "a ProfileId that is not 32 hexadecimal characters",
            publish_request("not-a-profile-id", "CCDA", None, None),
            400,
            None,
        ),
        (
            "a FromExistingVersion the profile has never published",
            publish_request(profile_id, "CCDA", Some(7), None),
            wire_http_status(404),
            Some("ResourceNotFoundException"),
        ),
        (
            // The modelled `ProfileVersion` range is 0..99.
            "a FromExistingVersion one past the modelled 99 maximum",
            publish_request(profile_id, "CCDA", Some(100), None),
            400,
            None,
        ),
        (
            "a FromExistingVersion one below the modelled 0 minimum",
            publish_request(profile_id, "CCDA", Some(-1), None),
            400,
            None,
        ),
        (
            "a SourceFormat the modelled enum does not declare",
            publish_request(profile_id, "HL7V2", None, None),
            400,
            None,
        ),
        (
            // The DRAFT holds a template mapping, which is not a mapping of the
            // comma-separated-values format: that format's mapping is the ONE
            // `mapping-config.yaml` document.
            "a SourceFormat whose mapping shape the DRAFT does not have",
            publish_request(profile_id, "CSV", None, None),
            400,
            Some("ValidationException"),
        ),
        (
            "no SourceFormat at all, which the model requires",
            json!({ "ProfileId": profile_id }),
            400,
            None,
        ),
        (
            "a ChangeDescription one character past its modelled 1000-character maximum",
            publish_request(profile_id, "CCDA", None, Some(&"c".repeat(1001))),
            400,
            None,
        ),
    ]
}

/// Every refused publish is refused with the modelled status and publishes nothing:
/// the profile still has no published version and its DRAFT is the content that was
/// there before.
#[tokio::test]
async fn refused_publishes_publish_nothing() {
    let healthlake = Profiles::new();
    let created = healthlake
        .ok(
            "refusals",
            "CreateDataTransformationProfile",
            &json!({
                "ProfileName": "publish-refusals",
                "SourceFormat": "CCDA",
                "Source": profile_mapping_source(&mapping(UNTOUCHED)),
            }),
        )
        .await;
    let profile_id = string_at(&created, "ProfileId");

    for (case, request, expected_status, expected_code) in refusal_cases(&profile_id) {
        let (status, code, payload) = healthlake
            .refused_body(case, "PublishDataTransformationProfile", &request)
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
        }

        // Nothing was published, so a read naming no version still resolves to the
        // DRAFT — the content the refused publish left untouched.
        let unpublished = healthlake
            .ok(
                case,
                "GetDataTransformationProfile",
                &json!({ "ProfileId": profile_id }),
            )
            .await;
        assert_eq!(
            unpublished["Version"].as_i64(),
            Some(0),
            "case {case:?}: a refused publish leaves the profile unpublished, got {unpublished}"
        );

        let draft = healthlake
            .ok(
                case,
                "GetDataTransformationProfile",
                &json!({ "ProfileId": profile_id, "ProfileVersion": 0 }),
            )
            .await;
        assert_eq!(
            reported_mapping(case, &draft),
            mapping(UNTOUCHED),
            "case {case:?}: a refused publish writes nothing"
        );
    }

    // The two 404 rows state the sentence of the version each publish resolved: a
    // publish promotes a VERSION, so each names the one it could not find.
    let (_, _, unknown_profile) = healthlake
        .refused_body(
            "an unknown profile",
            "PublishDataTransformationProfile",
            &publish_request("ffffffffffffffffffffffffffffffff", "CCDA", None, None),
        )
        .await;
    assert_eq!(
        modelled_message(&unknown_profile),
        Some(
            profile_version_not_found_message("ffffffffffffffffffffffffffffffff", 0).as_str()
        ),
        "an unknown profile is named beside the DRAFT the publish resolved, \
         got {unknown_profile}"
    );
    let (_, _, unknown_version) = healthlake
        .refused_body(
            "an unknown version",
            "PublishDataTransformationProfile",
            &publish_request(&profile_id, "CCDA", Some(7), None),
        )
        .await;
    assert_eq!(
        modelled_message(&unknown_version),
        Some(profile_version_not_found_message(&profile_id, 7).as_str()),
        "a version the profile lacks is named in its own sentence, got {unknown_version}"
    );

    // A publish still works after every refusal above, and it is version 1: none of
    // them consumed a version number.
    let case = "a publish after the refusals";
    let published = healthlake
        .ok(
            case,
            "PublishDataTransformationProfile",
            &publish_request(&profile_id, "CCDA", None, None),
        )
        .await;
    assert_eq!(
        published["Version"].as_i64(),
        Some(1),
        "case {case:?}: the refusals consumed no version number, got {published}"
    );
}

/// A profile runs out of versions at the modelled `ProfileVersion` maximum: the
/// publish that would pass it is refused as the quota it is, and the versions
/// published before it are all still readable.
#[tokio::test]
async fn a_profile_runs_out_of_versions_at_the_modelled_maximum() {
    let healthlake = Profiles::new();
    let case = "every version the modelled range allows";
    let content = mapping(&[("mapping-config.yaml", "tableName: patients")]);
    let created = healthlake
        .ok(
            case,
            "CreateDataTransformationProfile",
            &json!({
                "ProfileName": "publish-version-ceiling",
                "SourceFormat": "CSV",
                "Source": profile_mapping_source(&content),
            }),
        )
        .await;
    let profile_id = string_at(&created, "ProfileId");

    // The DRAFT is version 0, so the published versions run 1..=MAX_PROFILE_VERSION.
    for version in 1..=MAX_PROFILE_VERSION {
        let published = healthlake
            .ok(
                case,
                "PublishDataTransformationProfile",
                &publish_request(&profile_id, "CSV", None, None),
            )
            .await;
        assert_eq!(
            published["Version"].as_i64(),
            Some(i64::from(version)),
            "case {case:?}: publish number {version} is version {version}, got {published}"
        );
    }

    let (status, code, payload) = healthlake
        .refused_body(
            case,
            "PublishDataTransformationProfile",
            &publish_request(&profile_id, "CSV", None, None),
        )
        .await;
    assert_eq!(
        (status, code.as_str()),
        (wire_http_status(400), "ServiceQuotaExceededException"),
        "case {case:?}: the publish past version {MAX_PROFILE_VERSION} is refused as a quota, got {payload}"
    );

    let latest = healthlake
        .ok(
            case,
            "GetDataTransformationProfile",
            &json!({ "ProfileId": profile_id }),
        )
        .await;
    assert_eq!(
        latest["Version"].as_i64(),
        Some(i64::from(MAX_PROFILE_VERSION)),
        "case {case:?}: the refused publish left the latest version where it was, got {latest}"
    );
    assert_eq!(
        reported_mapping(case, &latest),
        content,
        "case {case:?}: the last version published still carries its content"
    );
}

/// A profile also runs out of the BYTES this emulator retains for one profile: the
/// publish whose snapshot would pass that ceiling is refused, and every version
/// published under it is still readable.
#[tokio::test]
async fn a_profile_runs_out_of_retained_bytes() {
    let healthlake = Profiles::new();
    let case = "the per-profile retained-byte ceiling";
    // Ten entries, each a 100 000-character value (inside the modelled 102 400
    // maximum), so one version is just under the 1 MiB a single mapping may hold and
    // a handful of versions reach the per-profile ceiling.
    let bulk: BTreeMap<String, String> = (0..10)
        .map(|index| {
            (
                format!("sections/bulk-{index:04}.map"),
                "v".repeat(100_000),
            )
        })
        .collect();
    let version_bytes = mapping_bytes(&bulk);

    let created = healthlake
        .ok(
            case,
            "CreateDataTransformationProfile",
            &json!({
                "ProfileName": "publish-byte-ceiling",
                "SourceFormat": "CCDA",
                "Source": profile_mapping_source(&bulk),
            }),
        )
        .await;
    let profile_id = string_at(&created, "ProfileId");

    // The DRAFT already counts against the ceiling, so the publishes that fit are
    // the ones whose snapshot keeps the total inside it.
    let publishes_that_fit = MAX_PROFILE_RETAINED_BYTES / version_bytes - 1;
    assert!(
        publishes_that_fit > 0,
        "case {case:?}: the ceiling must admit at least one publish, got {publishes_that_fit}"
    );
    for version in 1..=publishes_that_fit {
        let published = healthlake
            .ok(
                case,
                "PublishDataTransformationProfile",
                &publish_request(&profile_id, "CCDA", None, None),
            )
            .await;
        assert_eq!(
            published["Version"].as_i64(),
            Some(i64::try_from(version).expect("the version fits")),
            "case {case:?}: publish number {version} must be stored, got {published}"
        );
    }

    let (status, code, payload) = healthlake
        .refused_body(
            case,
            "PublishDataTransformationProfile",
            &publish_request(&profile_id, "CCDA", None, None),
        )
        .await;
    assert_eq!(
        (status, code.as_str()),
        (wire_http_status(400), "ServiceQuotaExceededException"),
        "case {case:?}: the publish past {MAX_PROFILE_RETAINED_BYTES} retained bytes is refused as a quota, got {payload}"
    );

    let latest = healthlake
        .ok(
            case,
            "GetDataTransformationProfile",
            &json!({ "ProfileId": profile_id }),
        )
        .await;
    assert_eq!(
        latest["Version"].as_i64(),
        Some(i64::try_from(publishes_that_fit).expect("the version fits")),
        "case {case:?}: the refused publish left the latest version where it was, got {latest}"
    );
    assert_eq!(
        reported_mapping(case, &latest),
        bulk,
        "case {case:?}: the last version published still carries its content"
    );
}

/// One row of the publish-format table: the mapping a profile's DRAFT holds, the
/// format it was created with, and whether a publish naming each format is served.
struct FormatCase {
    case: &'static str,
    /// The format the profile is CREATED with, which the publish rule does not read.
    created_as: &'static str,
    content: BTreeMap<String, String>,
    /// Whether a publish naming the comma-separated-values format is served.
    publishes_as_csv: bool,
}

/// The publish-format table. The rule is about the SHAPE of the content a publish
/// would snapshot, so the rows differ along that shape — the one
/// `mapping-config.yaml` document, that document beside another file, a single file
/// under another name, a set of templates, and an empty-valued entry — and along the
/// format the profile was created with, which the rule does not read.
fn format_cases() -> Vec<FormatCase> {
    vec![
        FormatCase {
            case: "the one CSV mapping document, in a profile created as CSV",
            created_as: "CSV",
            content: mapping(&[(CSV_PROFILE_MAPPING_FILE, "tableName: patients")]),
            publishes_as_csv: true,
        },
        FormatCase {
            case: "the one CSV mapping document, in a profile created as CCDA",
            created_as: "CCDA",
            content: mapping(&[(CSV_PROFILE_MAPPING_FILE, "tableName: patients")]),
            publishes_as_csv: true,
        },
        FormatCase {
            case: "the CSV mapping document with an empty value",
            created_as: "CSV",
            content: mapping(&[(CSV_PROFILE_MAPPING_FILE, "")]),
            publishes_as_csv: true,
        },
        FormatCase {
            case: "the CSV mapping document beside another file",
            created_as: "CSV",
            content: mapping(&[
                (CSV_PROFILE_MAPPING_FILE, "tableName: patients"),
                ("notes.md", "the table this profile maps"),
            ]),
            publishes_as_csv: false,
        },
        FormatCase {
            case: "one mapping file under another name",
            created_as: "CSV",
            content: mapping(&[("mapping-config.yml", "tableName: patients")]),
            publishes_as_csv: false,
        },
        FormatCase {
            case: "a set of C-CDA templates",
            created_as: "CCDA",
            content: mapping(&[
                ("Resources/Patient.vm", "#set($p = $patient)"),
                ("Resources/Observation.vm", "#set($o = $observation)"),
            ]),
            publishes_as_csv: false,
        },
        FormatCase {
            case: "many template files",
            created_as: "CCDA",
            content: mapping_of(12),
            publishes_as_csv: false,
        },
    ]
}

/// RULE: a publish validates the SHAPE of the content it would snapshot against the
/// `SourceFormat` the CALL names — a mapping of the comma-separated-values format is
/// the one `mapping-config.yaml` document, and a version holding anything else cannot
/// be published as that format — and never against the format the profile was created
/// with, which a publish may name either way. A refused publish creates no version.
#[tokio::test]
async fn a_publish_validates_the_mapping_against_the_format_it_names() {
    let healthlake = Profiles::new();

    for row in format_cases() {
        let case = row.case;
        let created = healthlake
            .ok(
                case,
                "CreateDataTransformationProfile",
                &json!({
                    "ProfileName": "publish-format-rule",
                    "SourceFormat": row.created_as,
                    "Source": profile_mapping_source(&row.content),
                }),
            )
            .await;
        let profile_id = string_at(&created, "ProfileId");

        if row.publishes_as_csv {
            let published = healthlake
                .ok(
                    case,
                    "PublishDataTransformationProfile",
                    &publish_request(&profile_id, "CSV", None, None),
                )
                .await;
            assert_eq!(
                published["Version"].as_i64(),
                Some(1),
                "case {case:?}: a CSV publish of a CSV mapping is the first version, \
                 got {published}"
            );
        } else {
            let (status, code, payload) = healthlake
                .refused_body(
                    case,
                    "PublishDataTransformationProfile",
                    &publish_request(&profile_id, "CSV", None, None),
                )
                .await;
            assert_eq!(
                (status, code.as_str()),
                (400, "ValidationException"),
                "case {case:?}: a CSV publish of a mapping that is not one must be refused, \
                 got {payload}"
            );
            assert_eq!(
                modelled_message(&payload),
                Some(csv_profile_mapping_message().as_str()),
                "case {case:?}: the refusal must state the key a CSV mapping requires, \
                 got {payload}"
            );
            // The refused publish created nothing: a read naming no version still
            // resolves to the DRAFT, and version 1 is a version the profile does not
            // hold.
            let unpublished = healthlake
                .ok(
                    case,
                    "GetDataTransformationProfile",
                    &json!({ "ProfileId": profile_id }),
                )
                .await;
            assert_eq!(
                unpublished["Version"].as_i64(),
                Some(0),
                "case {case:?}: a refused publish leaves the profile unpublished, \
                 got {unpublished}"
            );
            let (_, code, payload) = healthlake
                .refused_body(
                    case,
                    "GetDataTransformationProfile",
                    &json!({ "ProfileId": profile_id, "ProfileVersion": 1 }),
                )
                .await;
            assert_eq!(
                code.as_str(),
                "ResourceNotFoundException",
                "case {case:?}: the version a refused publish would have created must not \
                 be held, got {payload}"
            );
        }

        // Whatever its mapping's shape, every version of it publishes as the template
        // format — which fixes no file names — so the formats are not alternatives a
        // profile is locked into by its create.
        let as_ccda = healthlake
            .ok(
                case,
                "PublishDataTransformationProfile",
                &publish_request(&profile_id, "CCDA", None, None),
            )
            .await;
        assert_eq!(
            as_ccda["Version"].as_i64(),
            Some(if row.publishes_as_csv { 2 } else { 1 }),
            "case {case:?}: a CCDA publish snapshots the DRAFT whatever its shape, \
             got {as_ccda}"
        );
    }
}
