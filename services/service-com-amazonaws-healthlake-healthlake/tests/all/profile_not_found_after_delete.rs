// SPDX-License-Identifier: CC0-1.0

//! How this service WORDS its refusals for a data transformation profile it no
//! longer holds — the sequence a client runs when it deletes a profile and keeps
//! reading it — over the real awsJson1_0 wire.
//!
//! The rule under test: a data-transformation refusal that cannot resolve the
//! profile a request names IDENTIFIES it from the request. A call that resolves the
//! PROFILE is refused `Profile '<ProfileId>' not found`; one that resolves a
//! particular VERSION of it is refused `Profile '<ProfileId>' version <n> not
//! found`, where `<n>` is the version the request named — the DRAFT, version 0, when
//! it named none. The fixed text is the same for every such call and the variable
//! parts are the request's, so two refusals differ exactly where the two requests
//! did and no refusal is a request-independent sentence.
//!
//! What a delete LEAVES — nothing readable, nothing writable — is the sibling
//! suite's rule, so the rows and the create-publish-delete walk come from
//! [`deletion_cases`] and [`build_profile`] there rather than from a second copy
//! here. What this suite adds is the WORDING, asserted against the sentences written
//! out below instead of against the crate's own copy of them, over the versions a
//! later read can name (the DRAFT, every version the profile had published, the one
//! past the last of them and the modelled `ProfileVersion` maximum), through every
//! operation of the refusal family — including the version listing, which the
//! sibling suite does not reach — and over two profiles deleted in one store, so a
//! sentence quoting nothing from the request could not pass.
//!
//! Every call below goes through the shared [`Profiles`] harness, i.e.
//! `MultiServiceRouter::dispatch` — the same entry point `core/router` serves — so
//! routing, deserialization, the modelled constraints and the error envelope are all
//! exercised, not bypassed.
//!
//! The same rule for an id no record EVER held, over the whole data-transformation
//! surface, is `tests/all/data_transformation_refusal_wording.rs`; a reader looking
//! for a place to assert this wording wants one of these two suites rather than a
//! third.

use std::collections::BTreeMap;

use serde_json::{json, Value};
use service_com_amazonaws_healthlake_healthlake::scaffolded::wire_http_status;

use crate::data_transformation_profile::{
    mapping, modelled_message, profile_mapping_source, Profiles,
};
use crate::delete_data_transformation_profile::{
    build_profile, deletion_cases, request_mapping, Deletion,
};
use crate::fhir_datastore_lifecycle::string_at;

/// The largest version a read may name: the modelled `ProfileVersion` range is 0..99
/// (`smithy_query(query="shape",shape="ProfileVersion")`), so a version past this one
/// is refused at the boundary and never reaches a handler that could word a
/// not-found answer for it.
const MAX_MODELLED_VERSION: i64 = 99;

/// The sentence a call that resolved a whole profile no record holds is refused
/// with: the fixed text `Profile '` and `' not found` around the `ProfileId` the
/// request asked about.
///
/// Written out here rather than read back from the crate, so what this suite asserts
/// is the WORDING and not the emulator's own copy of it.
fn aws_profile_not_found(profile_id: &str) -> String {
    format!("Profile '{profile_id}' not found")
}

/// The sentence a call that resolved one VERSION of a profile no record holds is
/// refused with: the same template with the requested version as a second variable
/// part. Written out here for the reason [`aws_profile_not_found`] is.
fn aws_profile_version_not_found(profile_id: &str, version: i64) -> String {
    format!("Profile '{profile_id}' version {version} not found")
}

/// Assert `operation` on `request` is refused 404 `ResourceNotFoundException`
/// stating exactly `expected`.
async fn assert_refused_with(
    healthlake: &Profiles,
    case: &str,
    operation: &str,
    request: &Value,
    expected: &str,
) {
    let (status, code, payload) = healthlake.refused_body(case, operation, request).await;
    assert_eq!(
        (status, code.as_str(), modelled_message(&payload)),
        (
            wire_http_status(404),
            "ResourceNotFoundException",
            Some(expected)
        ),
        "case {case:?}: {operation} on {request} must be refused with {expected:?}, \
         got {status}: {payload}"
    );
}

/// The versions a profile holds, read through the listing, lowest first.
async fn listed_versions(healthlake: &Profiles, case: &str, profile_id: &str) -> Vec<i64> {
    let body = healthlake
        .ok(
            case,
            "ListDataTransformationProfileVersions",
            &json!({ "ProfileId": profile_id }),
        )
        .await;
    let items = body["Items"]
        .as_array()
        .cloned()
        .unwrap_or_else(|| panic!("case {case:?}: the listing carries an Items list: {body}"));
    let mut versions: Vec<i64> = items
        .iter()
        .map(|item| {
            item["Version"]
                .as_i64()
                .unwrap_or_else(|| panic!("case {case:?}: every summary carries a Version: {item}"))
        })
        .collect();
    versions.sort_unstable();
    versions
}

/// Every refusal of the post-delete read path names the profile the request asked
/// about, and the version it asked about wherever the call resolved one.
#[tokio::test]
async fn every_refusal_of_a_deleted_profile_names_what_it_could_not_find() {
    let healthlake = Profiles::new();

    // A live profile, so the refusals below are about the resource asked for and not
    // about a surface that refuses everything.
    let control = healthlake
        .ok(
            "the control profile",
            "CreateDataTransformationProfile",
            &json!({
                "ProfileName": "post-delete-control",
                "SourceFormat": "CCDA",
                "Source": profile_mapping_source(&mapping(&[("a.map", "Patient.id <- id")])),
            }),
        )
        .await;
    let live = string_at(&control, "ProfileId");

    for row in deletion_cases() {
        let case = row.case;
        let (profile_id, versions) = build_profile(&healthlake, &row).await;

        // The profile holds every one of those versions before the delete, so the
        // refusals after it are shown to be about a profile that went.
        assert_eq!(
            listed_versions(&healthlake, case, &profile_id).await,
            versions,
            "case {case:?}: the versions the profile holds are listed before the delete"
        );

        healthlake
            .ok(
                case,
                "DeleteDataTransformationProfile",
                &json!({ "ProfileId": profile_id }),
            )
            .await;

        // A call that resolves the PROFILE names the profile.
        let for_profile = aws_profile_not_found(&profile_id);
        for operation in [
            "GetDataTransformationProfile",
            "ListDataTransformationProfileVersions",
            "DeleteDataTransformationProfile",
        ] {
            assert_refused_with(
                &healthlake,
                case,
                operation,
                &json!({ "ProfileId": profile_id }),
                &for_profile,
            )
            .await;
        }

        // A call that resolves one VERSION names that version too: the DRAFT, every
        // version the profile had published, the one past the last of them and the
        // modelled maximum.
        let mut probed = versions.clone();
        probed.push(versions.len() as i64);
        probed.push(MAX_MODELLED_VERSION);
        for version in probed {
            let for_version = aws_profile_version_not_found(&profile_id, version);
            assert_refused_with(
                &healthlake,
                case,
                "GetDataTransformationProfile",
                &json!({ "ProfileId": profile_id, "ProfileVersion": version }),
                &for_version,
            )
            .await;
            assert_refused_with(
                &healthlake,
                case,
                "PublishDataTransformationProfile",
                &json!({
                    "ProfileId": profile_id,
                    "SourceFormat": row.source_format,
                    "FromExistingVersion": version,
                }),
                &for_version,
            )
            .await;
        }

        // A publish naming no source version would have copied the DRAFT, and an
        // update edits it, so both name version 0.
        let for_draft = aws_profile_version_not_found(&profile_id, 0);
        assert_refused_with(
            &healthlake,
            case,
            "PublishDataTransformationProfile",
            &json!({ "ProfileId": profile_id, "SourceFormat": row.source_format }),
            &for_draft,
        )
        .await;
        assert_refused_with(
            &healthlake,
            case,
            "UpdateDataTransformationProfile",
            &json!({
                "ProfileId": profile_id,
                "ProfileMapping": request_mapping(&mapping(&[("after.map", "Patient.id <- id")])),
            }),
            &for_draft,
        )
        .await;
    }

    // The control profile is untouched by all of it.
    let read = healthlake
        .ok(
            "the control profile",
            "GetDataTransformationProfile",
            &json!({ "ProfileId": live }),
        )
        .await;
    assert_eq!(
        read["ProfileId"].as_str(),
        Some(live.as_str()),
        "the live profile must still read back, got {read}"
    );
}

/// Two profiles deleted in one store are refused with their OWN ids: the variable
/// part of the sentence comes from the request, so neither refusal could be the
/// other's and neither could be a request-independent constant.
#[tokio::test]
async fn two_deleted_profiles_are_refused_with_their_own_identifiers() {
    let healthlake = Profiles::new();
    let case = "two profiles deleted in one store";

    let mut gone: Vec<String> = Vec::new();
    for index in 0..2u32 {
        let row = Deletion {
            case,
            source_format: "CSV",
            profile_name: format!("deleted-neighbour-{index}"),
            initial: BTreeMap::from([(
                "mapping-config.yaml".to_string(),
                "tableName: patients".to_string(),
            )]),
            publishes: 1,
            tags: None,
        };
        let (profile_id, _) = build_profile(&healthlake, &row).await;
        healthlake
            .ok(
                case,
                "DeleteDataTransformationProfile",
                &json!({ "ProfileId": profile_id }),
            )
            .await;
        gone.push(profile_id);
    }
    assert_ne!(
        gone[0], gone[1],
        "case {case:?}: the two profiles must have distinct ids, got {gone:?}"
    );

    let mut answered: Vec<String> = Vec::new();
    for profile_id in &gone {
        let (_, _, payload) = healthlake
            .refused_body(
                case,
                "GetDataTransformationProfile",
                &json!({ "ProfileId": profile_id }),
            )
            .await;
        let message = modelled_message(&payload)
            .unwrap_or_else(|| panic!("case {case:?}: the refusal carries a Message: {payload}"))
            .to_string();
        assert_eq!(
            message,
            aws_profile_not_found(profile_id),
            "case {case:?}: the refusal for {profile_id} must name it, got {message:?}"
        );
        answered.push(message);
    }
    assert_ne!(
        answered[0], answered[1],
        "case {case:?}: two refusals must differ where the requests did, got {answered:?}"
    );
}
