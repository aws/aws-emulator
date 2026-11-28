// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the FORM of the identifier this service mints for a build
//! request, driven over the same router the emulator boots.
//!
//! RULE: `MicrovmImageBuildSummary$buildId` and `GetMicrovmImageBuildOutput$buildId` are
//! "the build request ID" — a free-form `NonBlankString` the model fixes no concrete form
//! for, which a request id is reported in: a bare lowercase hyphenated 8-4-4-4-12 UUID,
//! drawn once per build request, carrying no service-chosen prefix and nothing of the
//! request or of how many builds the store minted before it. One mint writes the record,
//! so the listing row, the single-build read and the builds path label all speak the one
//! identifier.
//!
//! Each test states that rule and exercises it over a SET of inputs; the values any one
//! row happens to carry are examples, not the specification. The wire harness
//! (`emulator`, `call`, `image_body`, …) is the one `microvm_lifecycle` owns, and the
//! build-read accessors are the ones `image_build_read_path` owns.

use std::collections::BTreeSet;

use serde_json::{json, Value};
use service_com_amazonaws_lambdamicrovms_lambdamicrovms::state::{
    image_state_admits_update, managed_base_image_arn, FIRST_IMAGE_VERSION,
    MANAGED_BASE_IMAGE_IDS,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::image_build_read_path::{create_image, get_build, list_builds, page_build_ids};
use crate::microvm_image_and_instance_paths::get_image;
use crate::microvm_lifecycle::{as_uri_value, call, emulator, poll_until, REGION};

/// The character positions of a hyphenated 8-4-4-4-12 UUID the FORM itself fixes rather
/// than draws: the version nibble that opens the third group and the variant nibble that
/// opens the fourth (RFC 4122, the layout `uuid::Builder::from_random_bytes` writes).
const VERSION_NIBBLE: usize = 14;
const VARIANT_NIBBLE: usize = 19;

/// Whether `value` is a build request id in the form this service reports one in: 36
/// characters in the lowercase hyphenated `8-4-4-4-12` groups of a version-4 UUID —
/// hexadecimal throughout, the version nibble `4`, the variant nibble one of `8`, `9`,
/// `a`, `b` — and so carrying no prefix, no counter and no whitespace (the member's
/// `@pattern ^[^\s]+$`).
fn is_build_request_id(value: &str) -> bool {
    let groups: Vec<&str> = value.split('-').collect();
    let lengths: Vec<usize> = groups.iter().map(|group| group.len()).collect();
    if lengths != vec![8, 4, 4, 4, 12] {
        return false;
    }
    let hexadecimal = groups
        .iter()
        .all(|group| group.chars().all(|c| matches!(c, '0'..='9' | 'a'..='f')));
    hexadecimal && groups[2].starts_with('4') && groups[3].starts_with(['8', '9', 'a', 'b'])
}

/// Every `buildId` the build listing of `version` of the image `identifier` serves, in the
/// order the page carries them, with each one asserted to be in the request-id form.
pub(crate) async fn listed_build_ids(
    router: &MultiServiceRouter,
    identifier: &str,
    version: &str,
    case: &str,
) -> Vec<String> {
    let (status, body) = list_builds(router, identifier, version, "").await;
    assert_eq!(
        status, 200,
        "case {case:?}: the build listing got {status} {body}"
    );
    let ids = page_build_ids(&body, case);
    assert!(
        !ids.is_empty(),
        "case {case:?}: one build request at least, got {body}"
    );
    for build_id in &ids {
        assert!(
            is_build_request_id(build_id),
            "case {case:?}: expected a bare lowercase 8-4-4-4-12 UUID, got {build_id:?}"
        );
    }
    ids
}

/// Read each of `build_ids` back through `GetMicrovmImageBuild`, addressed BY the
/// identifier the listing published, and fail unless the read echoes it byte for byte.
pub(crate) async fn read_each_build_back(
    router: &MultiServiceRouter,
    identifier: &str,
    version: &str,
    build_ids: &[String],
    case: &str,
) {
    for build_id in build_ids {
        let (status, body) = get_build(router, identifier, version, build_id).await;
        assert_eq!(
            status, 200,
            "case {case:?}: the read of {build_id:?} got {status} {body}"
        );
        assert_eq!(
            body["buildId"].as_str(),
            Some(build_id.as_str()),
            "case {case:?}: the read echoes the identifier it was addressed by \
             ({build_id:?}), got {body}"
        );
    }
}

/// Fail unless no DRAWN character position of `ids` is fixed: an identifier rendered from a
/// counter leaves its leading positions constant for every build a store ever mints, while
/// a drawn one varies everywhere the UUID form does not fix the character.
fn every_drawn_position_varies(ids: &[String], case: &str) {
    assert!(
        ids.len() > 1,
        "case {case:?}: the variation is read over several mints, got {ids:?}"
    );
    for position in 0..ids[0].chars().count() {
        if position == VERSION_NIBBLE || position == VARIANT_NIBBLE {
            continue;
        }
        let seen: BTreeSet<Option<char>> = ids.iter().map(|id| id.chars().nth(position)).collect();
        if seen == BTreeSet::from([Some('-')]) {
            continue;
        }
        assert!(
            seen.len() > 1,
            "case {case:?}: expected the drawn character at position {position} to vary over \
             {} mints, got {seen:?} from {ids:?}",
            ids.len()
        );
    }
}

/// RULE: every build the create's fan-out mints publishes its `buildId` as a bare drawn
/// request id, distinct from every other build's of the store, and that identifier is the
/// one handle on the build: the single-build read addressed by it echoes it unchanged.
#[tokio::test]
async fn every_created_builds_id_is_a_bare_drawn_request_id() {
    let router = emulator();

    // Rows differ in what the create asks to be built — which is what the fan-out, and so
    // the number of identifiers one call mints, generalises over — and in which optional
    // members the request carries, because the identifier is drawn from none of them.
    let rows: Vec<(&str, &str, Value)> = vec![
        ("the architecture the service supplies", "drawn-supplied", json!({})),
        (
            "the one architecture modelled, named once",
            "drawn-named-once",
            json!({"cpuConfigurations": [{"architecture": "ARM_64"}]}),
        ),
        (
            "the same architecture named twice",
            "drawn-named-twice",
            json!({"cpuConfigurations": [
                {"architecture": "ARM_64"},
                {"architecture": "ARM_64"},
            ]}),
        ),
        (
            "a sized, described and tagged create",
            "drawn-every-member",
            json!({
                "description": "every optional member",
                "resources": [{"minimumMemoryInMiB": 2048}],
                "environmentVariables": {"STAGE": "prod"},
                "tags": {"team": "platform"},
            }),
        ),
    ];

    let mut everything: Vec<String> = Vec::new();
    for (case, name, extra) in rows {
        let image_arn = create_image(&router, name, extra).await;
        let ids = listed_build_ids(&router, &image_arn, FIRST_IMAGE_VERSION, case).await;

        let distinct: BTreeSet<&String> = ids.iter().collect();
        assert_eq!(
            distinct.len(),
            ids.len(),
            "case {case:?}: each build request gets its own id, got {ids:?}"
        );
        read_each_build_back(&router, &image_arn, FIRST_IMAGE_VERSION, &ids, case).await;

        // The listing is a pure report: it publishes the stored identifiers, so reading it
        // again answers the same ones in the same order.
        assert_eq!(
            listed_build_ids(&router, &image_arn, FIRST_IMAGE_VERSION, case).await,
            ids,
            "case {case:?}: the listing reports the ids it published, got {ids:?}"
        );
        everything.extend(ids);
    }

    // Across every build of the store: no identifier repeats, and no drawn position of one
    // is fixed — the identifier is the build's own rather than its position in the store.
    let distinct: BTreeSet<&String> = everything.iter().collect();
    assert_eq!(
        distinct.len(),
        everything.len(),
        "no two builds of one store share an id, got {everything:?}"
    );
    every_drawn_position_varies(&everything, "every build of the store");
}

/// RULE: the identifier form belongs to the MINT, not to one operation, so the builds an
/// `UpdateMicrovmImage` triggers for the version it adds carry the same bare drawn request
/// ids — distinct from the ids of the version the create built — and each is readable by
/// the single-build read of that version.
#[tokio::test]
async fn an_updates_builds_carry_the_same_request_id_form() {
    let router = emulator();
    let name = "drawn-updated";
    let image_arn = create_image(&router, name, json!({})).await;
    let created = listed_build_ids(&router, &image_arn, FIRST_IMAGE_VERSION, "the create").await;

    // An update builds the next version of a record whose own lifecycle work has settled
    // (`state::image_state_admits_update`), so the case waits for that CONDITION rather
    // than for a duration.
    poll_until("the update", "the image's create has settled", || async {
        let (status, body) = get_image(&router, &image_arn).await;
        status == 200 && body["state"].as_str().is_some_and(image_state_admits_update)
    })
    .await;

    let (status, body) = call(
        &router,
        "PUT",
        &format!("/2025-09-09/microvm-images/{}", as_uri_value(&image_arn)),
        Some(json!({
            "baseImageArn": managed_base_image_arn(REGION, MANAGED_BASE_IMAGE_IDS[0]),
            "buildRoleArn": "arn:aws:iam::123456789012:role/microvm-build",
            "codeArtifact": {"uri": "s3://artifacts/app-2.zip"},
        })),
    )
    .await;
    assert_eq!(status, 200, "the update got {status} {body}");
    let updated_version = body["imageVersion"]
        .as_str()
        .unwrap_or_else(|| panic!("the update answers the version it minted, got {body}"))
        .to_string();
    assert_ne!(
        updated_version, FIRST_IMAGE_VERSION,
        "the update mints a further version, got {body}"
    );

    let case = "the update";
    let updated = listed_build_ids(&router, &image_arn, &updated_version, case).await;
    read_each_build_back(&router, &image_arn, &updated_version, &updated, case).await;

    let shared: Vec<&String> = updated
        .iter()
        .filter(|build_id| created.contains(build_id))
        .collect();
    assert!(
        shared.is_empty(),
        "case {case:?}: each build request of each version gets its own id, got {shared:?} in \
         both of {created:?} and {updated:?}"
    );
}
