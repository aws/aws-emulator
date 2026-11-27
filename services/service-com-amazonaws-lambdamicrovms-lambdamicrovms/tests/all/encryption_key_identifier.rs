// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the ENCRYPTION KEY a caller names for the resources of this
//! service: `kmsKeyIdentifier`.
//!
//! RULE: `kmsKeyIdentifier` is caller-supplied configuration of the resource the call
//! creates — an image VERSION for `CreateMicrovmImage`/`UpdateMicrovmImage`, a MicroVM for
//! `RunMicrovm` — so the value the request named is stored on that resource and reported,
//! byte for byte, by every response that reports it, and a request that names none creates
//! a resource that "uses an Amazon Web Services owned key" and therefore reports no
//! identifier at all
//! (`smithy_query(query="doc",shape="CreateMicrovmImageRequest",member="kmsKeyIdentifier",
//! model_path="services/service-com-amazonaws-lambdamicrovms-lambdamicrovms/smithy-model/com-amazonaws-lambdamicrovms-lambdamicrovms.json")`).
//!
//! The responses that declare the member are `CreateMicrovmImageResponse`,
//! `UpdateMicrovmImageResponse`, `GetMicrovmImageVersionOutput`,
//! `MicrovmImageVersionSummary`, `UpdateMicrovmImageVersionResponse`,
//! `RunMicrovmResponse` and `GetMicrovmResponse`
//! (`smithy_query(query="shape",shape="CreateMicrovmImageResponse,UpdateMicrovmImageResponse,GetMicrovmImageVersionOutput,MicrovmImageVersionSummary,UpdateMicrovmImageVersionResponse,RunMicrovmResponse,GetMicrovmResponse",
//! model_path="services/service-com-amazonaws-lambdamicrovms-lambdamicrovms/smithy-model/com-amazonaws-lambdamicrovms-lambdamicrovms.json")`),
//! and each test walks every one of them that reports the resource it created.
//!
//! Each test states that rule and exercises it over a SET of inputs; the values any one
//! row happens to carry are examples, not the specification. The wire harness
//! (`emulator`, `call`, `image_body`, …) is the one `microvm_lifecycle` owns.

use serde_json::{json, Value};
use service_com_amazonaws_lambdamicrovms_lambdamicrovms::state::{
    managed_base_image_arn, FIRST_IMAGE_VERSION, MANAGED_BASE_IMAGE_IDS, VERSION_STATE_SUCCESSFUL,
    VERSION_STATUS_ACTIVE,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::image_build_outcome::get_version;
use crate::image_version_listing::list_versions;
use crate::image_version_status_publication::publish_status;
use crate::microvm_lifecycle::{
    as_uri_value, call, emulator, image_body, poll_until, run_microvm, REGION,
};

/// The member every case here is about.
const KEY_MEMBER: &str = "kmsKeyIdentifier";

/// The forms `KmsKeyIdentifier` admits, as its own constraint spells them: `@length(1,
/// 2048)` and `@pattern
/// ^(arn:aws[a-z-]*:kms:[a-z0-9-]+:[0-9]{12}:(key|alias)/[a-zA-Z0-9:/_-]+|alias/[a-zA-Z0-9:/_-]+|[a-zA-Z0-9-]+)$`
/// (`smithy_query(query="shape",shape="KmsKeyIdentifier",
/// model_path="services/service-com-amazonaws-lambdamicrovms-lambdamicrovms/smithy-model/com-amazonaws-lambdamicrovms-lambdamicrovms.json")`).
/// The rows differ along the FORM of the reference — key ARN, alias ARN, bare alias, bare
/// key id, another partition's ARN, the shortest value the length admits — because the
/// rule is about storing and reporting whatever reference the caller named, not about any
/// one spelling of it.
fn admitted_key_forms() -> Vec<(&'static str, String)> {
    vec![
        (
            "a key ARN",
            format!("arn:aws:kms:{REGION}:123456789012:key/0123abcd-ef45-6789-abcd-0123456789ab"),
        ),
        (
            "an alias ARN",
            format!("arn:aws:kms:{REGION}:123456789012:alias/microvm-images"),
        ),
        ("a bare alias", "alias/microvm-images".to_string()),
        (
            "a bare key id",
            "0123abcd-ef45-6789-abcd-0123456789ab".to_string(),
        ),
        (
            "a key ARN of another partition",
            "arn:aws-us-gov:kms:us-gov-west-1:123456789012:key/0123abcd-ef45".to_string(),
        ),
        ("the shortest value the length admits", "k".to_string()),
    ]
}

/// The AWS-managed base image every build here builds on, in the region the harness signs
/// for (`state::MANAGED_BASE_IMAGE_IDS`).
fn base_image_arn() -> String {
    managed_base_image_arn(REGION, MANAGED_BASE_IMAGE_IDS[0])
}

/// `PUT /2025-09-09/microvm-images/{imageIdentifier}` — the whole configuration, plus
/// whatever `extra` adds.
async fn update_image(
    router: &MultiServiceRouter,
    identifier: &str,
    extra: Value,
) -> (u16, Value) {
    let mut body = json!({
        "baseImageArn": base_image_arn(),
        "buildRoleArn": "arn:aws:iam::123456789012:role/microvm-rebuild",
        "codeArtifact": {"uri": "s3://artifacts/app-v2.zip"},
    });
    if let Value::Object(members) = extra {
        let map = body.as_object_mut().expect("the body is an object");
        for (key, value) in members {
            map.insert(key, value);
        }
    }
    call(
        router,
        "PUT",
        &format!("/2025-09-09/microvm-images/{}", as_uri_value(identifier)),
        Some(body),
    )
    .await
}

/// `GET /2025-09-09/microvms/{microvmIdentifier}`.
async fn get_microvm(router: &MultiServiceRouter, identifier: &str) -> (u16, Value) {
    call(
        router,
        "GET",
        &format!("/2025-09-09/microvms/{}", as_uri_value(identifier)),
        None,
    )
    .await
}

/// What `body` reports under [`KEY_MEMBER`]: the stored reference, or `None` for a
/// resource that rests on an AWS owned key and reports no identifier.
fn reported_key(body: &Value) -> Option<&str> {
    body[KEY_MEMBER].as_str()
}

/// Look at the version `image_version` of the image `name` until its build has settled on
/// the success branch, so a case reads a version whose record is final.
async fn poll_until_version_built(router: &MultiServiceRouter, case: &str, name: &str, image_version: &str) {
    poll_until(
        case,
        &format!("version {image_version} of {name} reports {VERSION_STATE_SUCCESSFUL}"),
        || async {
            let (status, body) = get_version(router, name, image_version).await;
            status == 200 && body["state"].as_str() == Some(VERSION_STATE_SUCCESSFUL)
        },
    )
    .await;
}

/// One row of the version listing of `name` for the version `image_version`.
async fn listed_version(router: &MultiServiceRouter, case: &str, name: &str, image_version: &str) -> Value {
    let (status, body) = list_versions(router, name, "").await;
    assert_eq!(
        status, 200,
        "case {case:?}: the version listing of {name} answered {status} {body}"
    );
    body["items"]
        .as_array()
        .unwrap_or(&Vec::new())
        .iter()
        .find(|row| row["imageVersion"].as_str() == Some(image_version))
        .cloned()
        .unwrap_or_else(|| {
            panic!("case {case:?}: the listing of {name} carries version {image_version}, got {body}")
        })
}

/// RULE: the `kmsKeyIdentifier` a create names is stored on the version it mints and
/// reported unchanged by every response that reports that version — the create itself, the
/// version's own read, its row in the version listing and a status update of it — and a
/// create that names none mints a version that reports no identifier.
#[tokio::test]
async fn a_versions_encryption_key_is_reported_wherever_that_version_is() {
    let router = emulator();

    // Rows: every form the member's constraint admits, plus the request that names no key
    // at all — the version that "uses an Amazon Web Services owned key".
    let mut rows: Vec<(String, Option<String>)> = admitted_key_forms()
        .into_iter()
        .map(|(case, key)| (case.to_string(), Some(key)))
        .collect();
    rows.push(("no key at all".to_string(), None));

    // Phase 1: create every row, and assert the create's own answer. One asynchronous
    // build span covers them all, so the reads come after every create.
    let mut created: Vec<(String, String, Option<String>)> = Vec::new();
    for (index, (case, key)) in rows.into_iter().enumerate() {
        let name = format!("kms-version-{index}");
        let extra = match &key {
            Some(key) => json!({KEY_MEMBER: key}),
            None => json!({}),
        };
        let (status, body) = call(
            &router,
            "POST",
            "/2025-09-09/microvm-images",
            Some(image_body(&name, extra)),
        )
        .await;
        assert_eq!(
            status, 201,
            "case {case:?}: a create naming {key:?} is accepted, got {status} {body}"
        );
        assert_eq!(
            reported_key(&body),
            key.as_deref(),
            "case {case:?}: the create reports the key it was given, got {:?}",
            body[KEY_MEMBER]
        );
        created.push((case, name, key));
    }

    // Phase 2: every other answer about the same version reports the same reference.
    for (case, name, key) in created {
        poll_until_version_built(&router, &case, &name, FIRST_IMAGE_VERSION).await;

        let (status, read) = get_version(&router, &name, FIRST_IMAGE_VERSION).await;
        assert_eq!(
            status, 200,
            "case {case:?}: the version read answered {status} {read}"
        );
        assert_eq!(
            reported_key(&read),
            key.as_deref(),
            "case {case:?}: the version read reports the stored key, got {:?}",
            read[KEY_MEMBER]
        );

        let row = listed_version(&router, &case, &name, FIRST_IMAGE_VERSION).await;
        assert_eq!(
            reported_key(&row),
            key.as_deref(),
            "case {case:?}: the listing row reports the stored key, got {row}"
        );
        // The same row also carries the member this service answers with beside the
        // modelled ones, so one page serves both halves of a settled version.
        assert!(
            row["versionStateTimeBucket"].is_string(),
            "case {case:?}: a settled version's row reports its state-time bucket, got {row}"
        );

        let (status, published) =
            publish_status(&router, &name, FIRST_IMAGE_VERSION, VERSION_STATUS_ACTIVE).await;
        assert_eq!(
            status, 200,
            "case {case:?}: the status update answered {status} {published}"
        );
        assert_eq!(
            reported_key(&published),
            key.as_deref(),
            "case {case:?}: the status update reports the stored key, got {:?}",
            published[KEY_MEMBER]
        );
    }
}

/// RULE: the key is configuration of ONE version, so an update records its own request's
/// key on the version it mints and leaves every earlier version reporting the key that
/// version was created with — including an update that names no key, whose version rests
/// on an AWS owned key however the earlier one was encrypted.
#[tokio::test]
async fn an_update_records_its_own_key_on_the_version_it_mints() {
    let router = emulator();

    let created_key =
        format!("arn:aws:kms:{REGION}:123456789012:key/11111111-2222-3333-4444-555555555555");

    // Rows: what the update names, against the same create. Each row gets its own image,
    // so the earlier version it must not disturb is its own.
    let rows: Vec<(&str, Option<String>)> = vec![
        (
            "an update naming another key",
            Some("alias/microvm-images-v2".to_string()),
        ),
        (
            "an update naming the same key",
            Some(created_key.clone()),
        ),
        ("an update naming no key", None),
    ];

    for (index, (case, updated_key)) in rows.into_iter().enumerate() {
        let name = format!("kms-update-{index}");
        let (status, body) = call(
            &router,
            "POST",
            "/2025-09-09/microvm-images",
            Some(image_body(&name, json!({KEY_MEMBER: created_key}))),
        )
        .await;
        assert_eq!(
            status, 201,
            "case {case:?}: the create is accepted, got {status} {body}"
        );
        poll_until_version_built(&router, case, &name, FIRST_IMAGE_VERSION).await;

        let extra = match &updated_key {
            Some(key) => json!({KEY_MEMBER: key}),
            None => json!({}),
        };
        let (status, updated) = update_image(&router, &name, extra).await;
        assert_eq!(
            status, 200,
            "case {case:?}: the update is accepted, got {status} {updated}"
        );
        assert_eq!(
            reported_key(&updated),
            updated_key.as_deref(),
            "case {case:?}: the update reports its OWN key, got {:?}",
            updated[KEY_MEMBER]
        );

        let minted = updated["imageVersion"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: the update reports the version it minted, got {updated}"))
            .to_string();
        assert_ne!(
            minted, FIRST_IMAGE_VERSION,
            "case {case:?}: the update mints a version of its own, got {updated}"
        );
        poll_until_version_built(&router, case, &name, &minted).await;

        let (status, new_version) = get_version(&router, &name, &minted).await;
        assert_eq!(
            status, 200,
            "case {case:?}: the new version's read answered {status} {new_version}"
        );
        assert_eq!(
            reported_key(&new_version),
            updated_key.as_deref(),
            "case {case:?}: the minted version reports the update's key, got {:?}",
            new_version[KEY_MEMBER]
        );

        let (status, first_version) = get_version(&router, &name, FIRST_IMAGE_VERSION).await;
        assert_eq!(
            status, 200,
            "case {case:?}: the first version's read answered {status} {first_version}"
        );
        assert_eq!(
            reported_key(&first_version),
            Some(created_key.as_str()),
            "case {case:?}: the earlier version keeps the key it was created with, got {:?}",
            first_version[KEY_MEMBER]
        );
    }
}

/// RULE: a MicroVM's `kmsKeyIdentifier` is the RUN's own configuration — "the key that
/// encrypts your MicroVM configuration at rest" — so it is reported by both the run and
/// every later read of that MicroVM, it is never taken from the image version the MicroVM
/// was launched from, and a run that names none reports no identifier.
#[tokio::test]
async fn a_microvms_encryption_key_is_the_runs_own() {
    let router = emulator();

    // The image is created with a key of its own, so a MicroVM reporting the image's key
    // instead of the run's would be visible here.
    let image_key = format!("arn:aws:kms:{REGION}:123456789012:alias/image-level-key");
    let name = "kms-microvm-image";
    let (status, body) = call(
        &router,
        "POST",
        "/2025-09-09/microvm-images",
        Some(image_body(name, json!({KEY_MEMBER: image_key}))),
    )
    .await;
    assert_eq!(status, 201, "the create is accepted, got {status} {body}");
    poll_until_version_built(&router, "the image the runs launch", name, FIRST_IMAGE_VERSION).await;

    // Rows: every form the member admits, plus the run that names none.
    let mut rows: Vec<(String, Option<String>)> = admitted_key_forms()
        .into_iter()
        .map(|(case, key)| (case.to_string(), Some(key)))
        .collect();
    rows.push(("no key at all".to_string(), None));

    for (case, key) in rows {
        let mut request = json!({"imageIdentifier": name});
        if let Some(key) = &key {
            request
                .as_object_mut()
                .expect("the body is an object")
                .insert(KEY_MEMBER.to_string(), json!(key));
        }
        let (status, run) = run_microvm(&router, request).await;
        assert_eq!(
            status, 200,
            "case {case:?}: a run naming {key:?} is accepted, got {status} {run}"
        );
        assert_eq!(
            reported_key(&run),
            key.as_deref(),
            "case {case:?}: the run reports the key it was given, got {:?}",
            run[KEY_MEMBER]
        );

        let microvm_id = run["microvmId"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: the run reports the id it minted, got {run}"))
            .to_string();
        let (status, read) = get_microvm(&router, &microvm_id).await;
        assert_eq!(
            status, 200,
            "case {case:?}: the MicroVM read answered {status} {read}"
        );
        assert_eq!(
            reported_key(&read),
            key.as_deref(),
            "case {case:?}: the MicroVM read reports the stored key, got {:?}",
            read[KEY_MEMBER]
        );
    }
}

/// RULE: a `kmsKeyIdentifier` outside the member's own constraint is refused with HTTP 400
/// `ValidationException` before anything is created, and the refusal names the member — so
/// a caller learns which value it has to fix rather than getting a resource encrypted with
/// something the service could not read.
#[tokio::test]
async fn an_encryption_key_outside_the_modelled_constraint_is_refused() {
    let router = emulator();

    // Rows: each way a value falls outside `KmsKeyIdentifier` — the pattern (whitespace,
    // punctuation the expression does not admit, an ARN of another service) and the
    // published length maximum, one character past it.
    let rows: Vec<(&str, String)> = vec![
        ("a value carrying a space", "not a key".to_string()),
        (
            "punctuation the pattern does not admit",
            "key!0123".to_string(),
        ),
        (
            "an ARN of another service",
            format!("arn:aws:s3:{REGION}:123456789012:key/abc"),
        ),
        (
            "one character past the length maximum",
            "k".repeat(2049),
        ),
    ];

    for (index, (case, key)) in rows.into_iter().enumerate() {
        let name = format!("kms-refused-{index}");
        let (status, body) = call(
            &router,
            "POST",
            "/2025-09-09/microvm-images",
            Some(image_body(&name, json!({KEY_MEMBER: key}))),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: {key:?} is outside the constraint, got {status} {body}"
        );
        assert_eq!(
            body["__type"].as_str(),
            Some("ValidationException"),
            "case {case:?}: expected ValidationException, got {body}"
        );
        let message = body["message"].as_str().unwrap_or_default();
        assert!(
            message.contains(KEY_MEMBER),
            "case {case:?}: the refusal names the member at fault, got {message:?}"
        );

        // The refusal came BEFORE the resource: nothing was created under that name.
        let (status, after) = call(
            &router,
            "GET",
            &format!("/2025-09-09/microvm-images/{}", as_uri_value(&name)),
            None,
        )
        .await;
        assert_ne!(
            status, 200,
            "case {case:?}: a refused create leaves no image behind, got {after}"
        );
    }
}

/// RULE: every value a response reports under this member is one the request named, so a
/// listing page of versions created with different keys reports each version's own — the
/// rows of one page never share a key.
#[tokio::test]
async fn one_page_of_versions_reports_each_versions_own_key() {
    let router = emulator();
    let name = "kms-per-row";

    let first_key = format!("arn:aws:kms:{REGION}:123456789012:key/aaaaaaaa-1111-2222-3333-444444444444");
    let (status, created) = call(
        &router,
        "POST",
        "/2025-09-09/microvm-images",
        Some(image_body(name, json!({KEY_MEMBER: first_key}))),
    )
    .await;
    assert_eq!(status, 201, "the create is accepted, got {status} {created}");
    poll_until_version_built(&router, "the first version", name, FIRST_IMAGE_VERSION).await;

    // Each update mints one more version, each with a key of its own — and one with none.
    let later_keys: Vec<Option<String>> = vec![
        Some("alias/second-key".to_string()),
        None,
        Some("0123abcd-ef45-6789-abcd-0123456789ab".to_string()),
    ];
    let mut expected: Vec<(String, Option<String>)> =
        vec![(FIRST_IMAGE_VERSION.to_string(), Some(first_key.clone()))];
    for (index, key) in later_keys.into_iter().enumerate() {
        let case = format!("update {index}");
        let extra = match &key {
            Some(key) => json!({KEY_MEMBER: key}),
            None => json!({}),
        };
        let (status, updated) = update_image(&router, name, extra).await;
        assert_eq!(
            status, 200,
            "case {case:?}: the update is accepted, got {status} {updated}"
        );
        let minted = updated["imageVersion"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: the update reports its version, got {updated}"))
            .to_string();
        poll_until_version_built(&router, &case, name, &minted).await;
        expected.push((minted, key));
    }

    for (image_version, key) in expected {
        let case = format!("version {image_version}");
        let row = listed_version(&router, &case, name, &image_version).await;
        assert_eq!(
            reported_key(&row),
            key.as_deref(),
            "case {case:?}: the row reports the key its own version was built with, got {row}"
        );
    }
}
