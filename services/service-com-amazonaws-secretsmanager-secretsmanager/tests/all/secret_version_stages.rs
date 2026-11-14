// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `UpdateSecretVersionStage` and `UpdateSecret`, driven
//! over the wire through the multi-service router and read back through
//! `DescribeSecret`, `GetSecretValue` and `ListSecretVersionIds`.
//!
//! The rules under test:
//!
//! * a staging label names at most ONE version of a secret at a time, so a move
//!   takes the label off the version that held it, and a move of `AWSCURRENT`
//!   leaves `AWSPREVIOUS` on the version it came from;
//! * a label that is already attached can only be moved by a call that names its
//!   current holder in `RemoveFromVersionId`: a call that names none and one that
//!   names the wrong version are both `InvalidParameterException` — the value the
//!   caller sent is what it has to correct — and neither changes anything, which is
//!   what makes two concurrent moves of one label resolve with a single winner;
//! * `AWSCURRENT` names the version a secret's current value lives on, so it is
//!   only ever MOVED between versions: a call that asks to take it off the
//!   version holding it, naming no version to move it to, is refused and leaves
//!   every label where it was, while any other label does come off;
//! * one version carries a bounded number of the caller's own staging labels, and
//!   the labels the service moves itself do not take a slot;
//! * `UpdateSecret` applies the metadata a call names, stores a value as a new
//!   current version, and is refused by state or reported missing exactly as every
//!   other operation that resolves a `SecretId` is.
//!
//! Every test is a TABLE over the class its rule generalises: the staging label
//! (reserved and caller-named), the form of the `SecretId`, how many versions the
//! secret holds, which version the label starts on, the metadata member updated,
//! and the type of the stored value.

use std::sync::Arc;
use std::time::Instant;

use serde_json::{json, Value};
use service_com_amazonaws_secretsmanager_secretsmanager::state::{
    FORCE_DELETE_PURGE_DELAY, STAGES_PER_VERSION,
};
use service_com_amazonaws_secretsmanager_secretsmanager::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// A SigV4 header naming `secretsmanager` in `us-west-2`; the router routes on
/// the credential scope and the emulator verifies signatures only when an
/// authenticator is registered, which this harness does not do.
const AUTHORIZATION: &str = "AWS4-HMAC-SHA256 \
     Credential=AKIDEXAMPLE/20260101/us-west-2/secretsmanager/aws4_request, \
     SignedHeaders=host;x-amz-date, Signature=deadbeef";

/// The one sentence this service answers a secret it cannot find with.
const SECRET_NOT_FOUND: &str = "Secrets Manager can't find the specified secret.";

/// The sentence an operation a secret's state forbids carries.
const MARKED_FOR_DELETION: &str =
    "You can't perform this operation on the secret because it was marked for deletion.";

/// A fresh emulator holding only this service, over its own engine.
fn emulator() -> MultiServiceRouter {
    let engine = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.register_service(EmulatedService::new(engine));
    router
}

/// Send one `awsJson1_1` request and return its status and JSON body.
async fn call(router: &MultiServiceRouter, operation: &str, body: &Value) -> (u16, Value) {
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("content-type", "application/x-amz-json-1.1")
        .header("x-amz-target", format!("secretsmanager.{operation}"))
        .header("authorization", AUTHORIZATION)
        .body(bytes::Bytes::from(
            // A write that stores a value creates a version the caller has to
            // name; see `secret_client_request_token::with_staked_version`.
            serde_json::to_vec(&crate::secret_client_request_token::with_staked_version(
                operation, body,
            ))
            .expect("the request body is serializable"),
        ))
        .expect("a valid request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let parsed = if response.body().is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(response.body()).unwrap_or(Value::Null)
    };
    (status, parsed)
}

/// Send one request that must succeed, and return its body.
async fn ok(router: &MultiServiceRouter, operation: &str, body: Value, case: &str) -> Value {
    let (status, parsed) = call(router, operation, &body).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected 200 from {operation}, got {status} with body {parsed}"
    );
    parsed
}

/// Send one request that must fail with a 400, and return its wire error code and
/// message.
async fn err(
    router: &MultiServiceRouter,
    operation: &str,
    body: Value,
    case: &str,
) -> (String, String) {
    let (status, parsed) = call(router, operation, &body).await;
    assert_eq!(
        status, 400,
        "case {case:?}: expected a 400 client error from {operation}, got {status} with body \
         {parsed}"
    );
    let raw = parsed
        .get("__type")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("case {case:?}: no __type in error body {parsed}"));
    let code = raw.split('#').next_back().unwrap_or(raw).to_string();
    let message = parsed
        .get("message")
        .or_else(|| parsed.get("Message"))
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("case {case:?}: no message in error body {parsed}"))
        .to_string();
    (code, message)
}

/// A required string member of a response body.
fn text(parsed: &Value, member: &str, case: &str) -> String {
    parsed
        .get(member)
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("case {case:?}: no {member} in {parsed}"))
        .to_string()
}

/// A version id of the modelled length (`SecretVersionIdType` is 32..64), built
/// from a readable seed so a failing case names which version it meant.
fn version_id(seed: &str) -> String {
    format!("{seed:0<32}")
}

/// The staging map `DescribeSecret` reports: version id to its labels, sorted.
async fn stages(router: &MultiServiceRouter, secret_id: &str, case: &str) -> Vec<(String, Vec<String>)> {
    let described = ok(router, "DescribeSecret", json!({"SecretId": secret_id}), case).await;
    let map = described
        .get("VersionIdsToStages")
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default();
    let mut rows: Vec<(String, Vec<String>)> = map
        .into_iter()
        .map(|(version, labels)| {
            let mut labels: Vec<String> = labels
                .as_array()
                .unwrap_or_else(|| panic!("case {case:?}: VersionIdsToStages holds a list"))
                .iter()
                .map(|label| label.as_str().unwrap_or_default().to_string())
                .collect();
            labels.sort();
            (version, labels)
        })
        .collect();
    rows.sort();
    rows
}

/// The version id carrying `label` in a staging map, if any version does.
fn holder_of(rows: &[(String, Vec<String>)], label: &str) -> Option<String> {
    rows.iter()
        .find(|(_, labels)| labels.iter().any(|held| held == label))
        .map(|(version, _)| version.clone())
}

/// Create a secret with one version, and report its ARN and that version's id.
async fn create(router: &MultiServiceRouter, name: &str, case: &str) -> (String, String) {
    let created = ok(
        router,
        "CreateSecret",
        json!({
            "Name": name,
            "SecretString": "first",
            "ClientRequestToken": version_id("v0"),
        }),
        case,
    )
    .await;
    (text(&created, "ARN", case), version_id("v0"))
}

/// Store another version under `token`, carrying `labels`.
async fn put(
    router: &MultiServiceRouter,
    name: &str,
    token: &str,
    labels: Value,
    case: &str,
) -> String {
    let stored = ok(
        router,
        "PutSecretValue",
        json!({
            "SecretId": name,
            "ClientRequestToken": token,
            "SecretString": format!("value-for-{token}"),
            "VersionStages": labels,
        }),
        case,
    )
    .await;
    text(&stored, "VersionId", case)
}

#[tokio::test]
async fn a_staging_label_moves_to_one_version_and_leaves_the_other() {
    // "Each staging label can be attached to only one version at a time. To add a
    // staging label to a version when it is already attached to another version,
    // Secrets Manager first removes it from the other version first and then
    // attaches it to this one." (`UpdateSecretVersionStage`, this service's
    // Smithy model.) The rows run over the label being moved — the reserved ones
    // the service also moves itself, and labels a caller named — and over the
    // form of the `SecretId`.
    for label in ["AWSCURRENT", "AWSPENDING", "blue", "release/2024-05"] {
        for by_arn in [false, true] {
            let case = format!("moving {label:?}, by ARN: {by_arn}");
            let router = emulator();
            let name = "stage-move";
            let (arn, v0) = create(&router, name, &case).await;
            let secret_id = if by_arn { arn.clone() } else { name.to_string() };
            let v1 = put(
                &router,
                name,
                &version_id("v1"),
                json!(["AWSPENDING"]),
                &case,
            )
            .await;

            // Where the label sits before the move, so the call can name it.
            let holder = match label {
                "AWSCURRENT" => Some(v0.clone()),
                "AWSPENDING" => Some(v1.clone()),
                _ => None,
            };
            let mut body = json!({
                "SecretId": secret_id.clone(),
                "VersionStage": label,
                "MoveToVersionId": v0.clone(),
            });
            if label == "AWSPENDING" {
                body.as_object_mut()
                    .expect("a request body is an object")
                    .insert("RemoveFromVersionId".to_string(), json!(v1.clone()));
            }
            // Moving a label that is already on the target version is the move
            // the caller asked for and changes nothing; every other row moves it.
            let moved = ok(&router, "UpdateSecretVersionStage", body, &case).await;
            assert_eq!(
                text(&moved, "ARN", &case),
                arn,
                "case {case:?}: expected the secret's ARN, got {moved}"
            );
            assert_eq!(
                text(&moved, "Name", &case),
                name,
                "case {case:?}: expected the secret's name, got {moved}"
            );

            // The label is on exactly one version: the one it was moved to.
            let rows = stages(&router, &secret_id, &case).await;
            let holders: Vec<&String> = rows
                .iter()
                .filter(|(_, labels)| labels.iter().any(|held| held == label))
                .map(|(version, _)| version)
                .collect();
            assert_eq!(
                holders,
                vec![&v0],
                "case {case:?}: expected {label:?} on version {v0} alone, got {rows:?}"
            );
            // Nothing else about the staging map was disturbed: the label the
            // other version kept is still there unless this call took it.
            if holder.as_deref() != Some(v1.as_str()) {
                let other: Vec<String> = rows
                    .iter()
                    .find(|(version, _)| *version == v1)
                    .map(|(_, labels)| labels.clone())
                    .unwrap_or_default();
                assert!(
                    other.iter().any(|held| held == "AWSPENDING"),
                    "case {case:?}: expected version {v1} to keep AWSPENDING, got {rows:?}"
                );
            }
        }
    }
}

#[tokio::test]
async fn moving_awscurrent_leaves_awsprevious_behind_it() {
    // "Whenever you move AWSCURRENT, Secrets Manager automatically moves the
    // label AWSPREVIOUS to the version that AWSCURRENT was removed from."
    // (`UpdateSecretVersionStage`, this service's Smithy model.) The rows run
    // over how many versions the secret holds when the label is moved, so the
    // demotion is asserted on a fresh secret and on one with a history.
    for history in [1usize, 2, 4] {
        let case = format!("{history} earlier versions");
        let router = emulator();
        let name = "stage-demote";
        let (_, v0) = create(&router, name, &case).await;
        let mut current = v0.clone();
        for index in 1..history {
            current = put(
                &router,
                name,
                &version_id(&format!("h{index}")),
                json!(["AWSCURRENT"]),
                &case,
            )
            .await;
        }
        let pending = put(
            &router,
            name,
            &version_id("pending"),
            json!(["AWSPENDING"]),
            &case,
        )
        .await;

        ok(
            &router,
            "UpdateSecretVersionStage",
            json!({
                "SecretId": name,
                "VersionStage": "AWSCURRENT",
                "MoveToVersionId": pending.clone(),
                "RemoveFromVersionId": current.clone(),
            }),
            &case,
        )
        .await;

        let rows = stages(&router, name, &case).await;
        let labels_of = |version: &str| -> Vec<String> {
            rows.iter()
                .find(|(held, _)| held == version)
                .map(|(_, labels)| labels.clone())
                .unwrap_or_default()
        };
        assert!(
            labels_of(&pending).iter().any(|held| held == "AWSCURRENT"),
            "case {case:?}: expected AWSCURRENT on version {pending}, got {rows:?}"
        );
        assert!(
            labels_of(&current).iter().any(|held| held == "AWSPREVIOUS"),
            "case {case:?}: expected AWSPREVIOUS on the version AWSCURRENT left ({current}), got \
             {rows:?}"
        );
        assert_eq!(
            rows.iter()
                .filter(|(_, labels)| labels.iter().any(|held| held == "AWSPREVIOUS"))
                .count(),
            1,
            "case {case:?}: expected AWSPREVIOUS on exactly one version, got {rows:?}"
        );
        // The promoted version is what a read with no version or stage resolves.
        let read = ok(&router, "GetSecretValue", json!({"SecretId": name}), &case).await;
        assert_eq!(
            text(&read, "VersionId", &case),
            pending,
            "case {case:?}: expected the promoted version to be current, got {read}"
        );
    }
}

#[tokio::test]
async fn an_attached_label_moves_only_for_the_version_that_holds_it() {
    // "If the staging label you are trying to attach to one version is already
    // attached to a different version, then you must include this parameter and
    // specify the version that the label is to be removed from. If the label is
    // attached and you either do not specify this parameter, or the version ID
    // does not match, then the operation fails."
    // (`UpdateSecretVersionStageRequest$RemoveFromVersionId`, this service's
    // Smithy model.) The rows run over the label, and over what the call names in
    // `RemoveFromVersionId`: nothing, a version that does not hold the label, and
    // a version id the secret has never held.
    for label in ["AWSCURRENT", "AWSPENDING", "blue"] {
        let case = format!("label {label:?}");
        let router = emulator();
        let name = "stage-precondition";
        let (_, v0) = create(&router, name, &case).await;
        let v1 = put(&router, name, &version_id("v1"), json!(["AWSPENDING"]), &case).await;
        let v2 = put(&router, name, &version_id("v2"), json!(["blue"]), &case).await;
        // Where each label starts: AWSCURRENT on v0, AWSPENDING on v1, blue on v2.
        let holder = match label {
            "AWSCURRENT" => v0.clone(),
            "AWSPENDING" => v1.clone(),
            _ => v2.clone(),
        };
        // The version the call tries to move the label to: never the holder.
        let target = if holder == v0 { v1.clone() } else { v0.clone() };
        let before = stages(&router, name, &case).await;

        // (what RemoveFromVersionId carries, the code the refusal must use) — one
        // class for the whole rule: whether the parameter is missing or names a
        // version the label is not on, what the caller has to correct is the value it
        // sent, so every row is the parameter-value refusal.
        let rows: [(Option<String>, &str); 3] = [
            (None, "InvalidParameterException"),
            (Some(v2.clone()), "InvalidParameterException"),
            (
                Some(version_id("never-stored")),
                "InvalidParameterException",
            ),
        ];
        for (remove_from, expected_code) in rows {
            // A row that happens to name the holder is not a refusal at all.
            if remove_from.as_deref() == Some(holder.as_str()) {
                continue;
            }
            let case = format!("{case}, RemoveFromVersionId {remove_from:?}");
            let mut body = json!({
                "SecretId": name,
                "VersionStage": label,
                "MoveToVersionId": target.clone(),
            });
            if let Some(remove_from) = &remove_from {
                body.as_object_mut()
                    .expect("a request body is an object")
                    .insert("RemoveFromVersionId".to_string(), json!(remove_from));
            }
            let (code, message) = err(&router, "UpdateSecretVersionStage", body, &case).await;
            assert_eq!(
                code, expected_code,
                "case {case:?}: expected {expected_code}, got {code:?} with message {message:?}"
            );
            if remove_from.is_none() {
                let expected = format!(
                    "The parameter RemoveFromVersionId can't be empty. Staging label {label} is \
                     currently attached to version {holder}, so you must explicitly reference \
                     that version in RemoveFromVersionId."
                );
                assert_eq!(
                    message, expected,
                    "case {case:?}: expected {expected:?}, got {message:?}"
                );
            } else {
                assert!(
                    message.contains(&holder) && message.contains(label),
                    "case {case:?}: expected the refusal to name the label and its holder, got \
                     {message:?}"
                );
            }
            // The refused call changed nothing.
            assert_eq!(
                stages(&router, name, &case).await,
                before,
                "case {case:?}: expected the refused call to leave the staging map untouched"
            );
        }

        // Naming the holder is what makes the same move succeed.
        let case = format!("{case}, RemoveFromVersionId naming the holder");
        ok(
            &router,
            "UpdateSecretVersionStage",
            json!({
                "SecretId": name,
                "VersionStage": label,
                "MoveToVersionId": target.clone(),
                "RemoveFromVersionId": holder.clone(),
            }),
            &case,
        )
        .await;
        let rows = stages(&router, name, &case).await;
        let holders: Vec<&String> = rows
            .iter()
            .filter(|(_, labels)| labels.iter().any(|held| held == label))
            .map(|(version, _)| version)
            .collect();
        assert_eq!(
            holders,
            vec![&target],
            "case {case:?}: expected {label:?} to move to {target}, got {rows:?}"
        );
    }
}

#[tokio::test]
async fn two_moves_of_one_label_from_the_same_version_leave_a_single_winner() {
    // A label names one version at a time, and the `RemoveFromVersionId`
    // precondition is what resolves a race for it: whichever call lands second
    // names a version the label has already left, so it is refused and the label
    // is never on two versions. The rows run over the label and over the number of
    // contenders.
    for label in ["AWSCURRENT", "blue"] {
        for contenders in [2usize, 3, 5] {
            let case = format!("{contenders} moves of {label:?}");
            let router = emulator();
            let name = "stage-race";
            let (_, v0) = create(&router, name, &case).await;
            if label != "AWSCURRENT" {
                ok(
                    &router,
                    "UpdateSecretVersionStage",
                    json!({
                        "SecretId": name,
                        "VersionStage": label,
                        "MoveToVersionId": v0.clone(),
                    }),
                    &case,
                )
                .await;
            }
            let mut targets = Vec::new();
            for index in 0..contenders {
                targets.push(
                    put(
                        &router,
                        name,
                        &version_id(&format!("c{index}")),
                        json!([format!("candidate-{index}")]),
                        &case,
                    )
                    .await,
                );
            }

            // Every contender moves the label off the SAME source version.
            let mut winners = Vec::new();
            let mut losers = Vec::new();
            for target in &targets {
                let body = json!({
                    "SecretId": name,
                    "VersionStage": label,
                    "MoveToVersionId": target.clone(),
                    "RemoveFromVersionId": v0.clone(),
                });
                let (status, parsed) = call(&router, "UpdateSecretVersionStage", &body).await;
                if status == 200 {
                    winners.push(target.clone());
                } else {
                    let code = parsed
                        .get("__type")
                        .and_then(Value::as_str)
                        .unwrap_or_default();
                    let code = code.split('#').next_back().unwrap_or(code).to_string();
                    // What the loser has to correct is the VALUE it sent in
                    // `RemoveFromVersionId`, so the refusal is the parameter one.
                    assert_eq!(
                        code, "InvalidParameterException",
                        "case {case:?}: expected the loser to be refused for the value it sent \
                         in RemoveFromVersionId, got {code:?} with body {parsed}"
                    );
                    losers.push(target.clone());
                }
            }
            assert_eq!(
                winners.len(),
                1,
                "case {case:?}: expected exactly one winner, got {winners:?} against {losers:?}"
            );
            let rows = stages(&router, name, &case).await;
            let holders: Vec<&String> = rows
                .iter()
                .filter(|(_, labels)| labels.iter().any(|held| held == label))
                .map(|(version, _)| version)
                .collect();
            assert_eq!(
                holders,
                vec![&winners[0]],
                "case {case:?}: expected {label:?} on the winner alone, got {rows:?}"
            );
        }
    }
}

#[tokio::test]
async fn a_version_carries_a_bounded_number_of_caller_named_labels() {
    // `SecretVersionStagesType` — "a list of all of the staging labels currently
    // attached to this version of the secret" — is modelled with a maximum length
    // of 20, and the labels the service moves itself do not take a slot. Over the
    // bound, "the request failed because it would exceed one of the Secrets
    // Manager quotas." The rows run over whether the version also carries the
    // reserved labels, which must not consume the caller's slots.
    let expected = "You exceeded the maximum number of staging labels allowed on a secret.";
    for reserved in [true, false] {
        let case = format!("the version also holds the reserved labels: {reserved}");
        let router = emulator();
        let name = "stage-cap";
        let (_, v0) = create(&router, name, &case).await;
        // A version that carries no reserved label at all: AWSCURRENT is moved
        // off it onto a later version.
        let target = if reserved {
            v0.clone()
        } else {
            let v1 = put(&router, name, &version_id("v1"), json!(["AWSCURRENT"]), &case).await;
            assert_ne!(v1, v0, "case {case:?}: the later version has its own id");
            put(&router, name, &version_id("v2"), json!(["spare"]), &case).await
        };

        // Every slot the caller owns is filled, one call at a time.
        let filled = if reserved { 0 } else { 1 };
        for index in filled..STAGES_PER_VERSION {
            ok(
                &router,
                "UpdateSecretVersionStage",
                json!({
                    "SecretId": name,
                    "VersionStage": format!("label-{index:02}"),
                    "MoveToVersionId": target.clone(),
                }),
                &case,
            )
            .await;
        }
        let before = stages(&router, name, &case).await;

        let (code, message) = err(
            &router,
            "UpdateSecretVersionStage",
            json!({
                "SecretId": name,
                "VersionStage": "one-too-many",
                "MoveToVersionId": target.clone(),
            }),
            &case,
        )
        .await;
        assert_eq!(
            code, "LimitExceededException",
            "case {case:?}: expected LimitExceededException over the cap, got {code:?} with \
             message {message:?}"
        );
        assert_eq!(
            message, expected,
            "case {case:?}: expected {expected:?}, got {message:?}"
        );
        assert_eq!(
            stages(&router, name, &case).await,
            before,
            "case {case:?}: expected the refused attachment to change nothing"
        );

        // A reserved label still moves onto the same version: the cap is on the
        // caller's labels only.
        ok(
            &router,
            "UpdateSecretVersionStage",
            json!({
                "SecretId": name,
                "VersionStage": "AWSPENDING",
                "MoveToVersionId": target.clone(),
            }),
            &case,
        )
        .await;
        let rows = stages(&router, name, &case).await;
        let labels: Vec<String> = rows
            .iter()
            .find(|(version, _)| *version == target)
            .map(|(_, labels)| labels.clone())
            .unwrap_or_default();
        assert!(
            labels.iter().any(|held| held == "AWSPENDING"),
            "case {case:?}: expected a reserved label to attach over the cap, got {rows:?}"
        );
    }
}

#[tokio::test]
async fn removing_the_last_label_deprecates_a_version_without_deleting_it() {
    // "If this action results in the last label being removed from a version,
    // then the version is considered to be 'deprecated'"
    // (`UpdateSecretVersionStage`, this service's Smithy model): the version
    // leaves the staging map but is still addressable by its id. The rows run
    // over the label removed and over how many labels the version carried.
    for extra_labels in [0usize, 1, 3] {
        let case = format!("{extra_labels} labels beside the one removed");
        let router = emulator();
        let name = "stage-remove";
        create(&router, name, &case).await;
        let v1 = put(&router, name, &version_id("v1"), json!(["blue"]), &case).await;
        for index in 0..extra_labels {
            ok(
                &router,
                "UpdateSecretVersionStage",
                json!({
                    "SecretId": name,
                    "VersionStage": format!("extra-{index}"),
                    "MoveToVersionId": v1.clone(),
                }),
                &case,
            )
            .await;
        }

        ok(
            &router,
            "UpdateSecretVersionStage",
            json!({
                "SecretId": name,
                "VersionStage": "blue",
                "RemoveFromVersionId": v1.clone(),
            }),
            &case,
        )
        .await;

        let rows = stages(&router, name, &case).await;
        let labels: Vec<String> = rows
            .iter()
            .find(|(version, _)| *version == v1)
            .map(|(_, labels)| labels.clone())
            .unwrap_or_default();
        assert!(
            !labels.iter().any(|held| held == "blue"),
            "case {case:?}: expected the label to be gone from version {v1}, got {rows:?}"
        );
        assert_eq!(
            labels.len(),
            extra_labels,
            "case {case:?}: expected the version's other labels to survive, got {rows:?}"
        );
        // The version itself is still readable by id, deprecated or not.
        let read = ok(
            &router,
            "GetSecretValue",
            json!({"SecretId": name, "VersionId": v1.clone()}),
            &case,
        )
        .await;
        assert_eq!(
            text(&read, "VersionId", &case),
            v1,
            "case {case:?}: expected the deprecated version to still be readable, got {read}"
        );

        // Removing a label from a version that does not hold it is refused, and
        // the message says where the label is.
        let (code, message) = err(
            &router,
            "UpdateSecretVersionStage",
            json!({
                "SecretId": name,
                "VersionStage": "blue",
                "RemoveFromVersionId": v1.clone(),
            }),
            &case,
        )
        .await;
        assert_eq!(
            code, "InvalidParameterException",
            "case {case:?}: expected InvalidParameterException for a label that is not attached, \
             got {code:?} with message {message:?}"
        );
        let expected = format!("Staging label blue is not attached to version {v1}.");
        assert_eq!(
            message, expected,
            "case {case:?}: expected {expected:?}, got {message:?}"
        );
    }
}

#[tokio::test]
async fn a_stage_change_names_a_version_to_move_or_remove() {
    // `MoveToVersionId` is "the ID of the version to add the staging label to"
    // and `RemoveFromVersionId` "the ID of the version that the staging label is
    // to be removed from" (this service's Smithy model): a call naming neither
    // asks for no change. The rows run over the label named, including one no
    // version carries.
    const EXPECTED: &str = "You must specify either MoveToVersionId or RemoveFromVersionId.";
    for label in ["AWSCURRENT", "AWSPENDING", "blue"] {
        let case = format!("label {label:?} with no version named");
        let router = emulator();
        let name = "stage-no-version";
        create(&router, name, &case).await;
        let (code, message) = err(
            &router,
            "UpdateSecretVersionStage",
            json!({"SecretId": name, "VersionStage": label}),
            &case,
        )
        .await;
        assert_eq!(
            code, "InvalidParameterException",
            "case {case:?}: expected InvalidParameterException, got {code:?} with message \
             {message:?}"
        );
        assert_eq!(
            message, EXPECTED,
            "case {case:?}: expected {EXPECTED:?}, got {message:?}"
        );
    }

    // A `MoveToVersionId` the secret holds no version under is not found, and the
    // message names the version asked for.
    for absent in [version_id("absent"), version_id("another-absent")] {
        let case = format!("MoveToVersionId {absent}");
        let router = emulator();
        let name = "stage-absent-version";
        create(&router, name, &case).await;
        let (code, message) = err(
            &router,
            "UpdateSecretVersionStage",
            json!({
                "SecretId": name,
                "VersionStage": "blue",
                "MoveToVersionId": absent.clone(),
            }),
            &case,
        )
        .await;
        assert_eq!(
            code, "ResourceNotFoundException",
            "case {case:?}: expected ResourceNotFoundException, got {code:?} with message \
             {message:?}"
        );
        let expected = format!("Secrets Manager can't find the specified secret version: {absent}");
        assert_eq!(
            message, expected,
            "case {case:?}: expected {expected:?}, got {message:?}"
        );
    }
}

#[tokio::test]
async fn a_secret_marked_for_deletion_refuses_a_stage_change_and_an_update() {
    // A secret inside its recovery window still EXISTS, so an operation it
    // forbids is refused by its state: "The secret is scheduled for deletion."
    // (`InvalidRequestException`, this service's Smithy model.) The rows run over
    // each documented bound of the recovery window and a value between them, and
    // over the mutation attempted.
    for window in [7i64, 19, 30] {
        let case = format!("a recovery window of {window} days");
        let router = emulator();
        let name = "stage-deleted";
        let (_, v0) = create(&router, name, &case).await;
        ok(
            &router,
            "DeleteSecret",
            json!({"SecretId": name, "RecoveryWindowInDays": window}),
            &case,
        )
        .await;

        let mutations: [(&str, Value); 3] = [
            (
                "UpdateSecretVersionStage",
                json!({
                    "SecretId": name,
                    "VersionStage": "blue",
                    "MoveToVersionId": v0.clone(),
                }),
            ),
            (
                "UpdateSecretVersionStage",
                json!({
                    "SecretId": name,
                    "VersionStage": "AWSCURRENT",
                    "RemoveFromVersionId": v0.clone(),
                }),
            ),
            (
                "UpdateSecret",
                json!({"SecretId": name, "Description": "while deleted"}),
            ),
        ];
        for (operation, body) in mutations {
            let case = format!("{case}: {operation} {body}");
            let (code, message) = err(&router, operation, body, &case).await;
            assert_eq!(
                code, "InvalidRequestException",
                "case {case:?}: expected InvalidRequestException for a secret marked for \
                 deletion, got {code:?} with message {message:?}"
            );
            assert_eq!(
                message, MARKED_FOR_DELETION,
                "case {case:?}: expected {MARKED_FOR_DELETION:?}, got {message:?}"
            );
        }

        // Restoring the secret is what makes the same calls work again.
        ok(&router, "RestoreSecret", json!({"SecretId": name}), &case).await;
        ok(
            &router,
            "UpdateSecret",
            json!({"SecretId": name, "Description": "after restore"}),
            &case,
        )
        .await;
        let described = ok(&router, "DescribeSecret", json!({"SecretId": name}), &case).await;
        assert_eq!(
            text(&described, "Description", &case),
            "after restore",
            "case {case:?}: expected the update to be stored after the restore, got {described}"
        );
    }
}

#[tokio::test]
async fn update_secret_applies_the_metadata_a_call_names() {
    // "Modifies the details of a secret, including metadata and the secret
    // value." (`UpdateSecret`, this service's Smithy model.) A member the call
    // does not name keeps the value already stored, and the change is visible to
    // the next `DescribeSecret`. The rows run over the member updated and the
    // value it carries, including the documented empty `KmsKeyId`.
    //
    // (the request's members, the member read back, the value expected)
    let rows: [(Value, &str, &str); 5] = [
        (json!({"Description": "a new description"}), "Description", "a new description"),
        (
            json!({"KmsKeyId": "arn:aws:kms:us-west-2:111122223333:key/abcd"}),
            "KmsKeyId",
            "arn:aws:kms:us-west-2:111122223333:key/abcd",
        ),
        (json!({"KmsKeyId": "alias/my-key"}), "KmsKeyId", "alias/my-key"),
        // "If you set this to an empty string, Secrets Manager uses the Amazon
        // Web Services managed key aws/secretsmanager."
        (json!({"KmsKeyId": ""}), "KmsKeyId", "alias/aws/secretsmanager"),
        (json!({"Type": "ExamplePartner"}), "Type", "ExamplePartner"),
    ];

    for (update, member, expected) in rows {
        let case = format!("update {update} read back as {member}");
        let router = emulator();
        let name = "update-metadata";
        let (arn, v0) = create(&router, name, &case).await;
        let before = ok(&router, "DescribeSecret", json!({"SecretId": name}), &case).await;

        let mut body = update.clone();
        body.as_object_mut()
            .expect("every row's update is an object")
            .insert("SecretId".to_string(), json!(name));
        let updated = ok(&router, "UpdateSecret", body, &case).await;
        assert_eq!(
            text(&updated, "ARN", &case),
            arn,
            "case {case:?}: expected the secret's ARN, got {updated}"
        );
        assert_eq!(
            text(&updated, "Name", &case),
            name,
            "case {case:?}: expected the secret's name, got {updated}"
        );
        assert!(
            updated.get("VersionId").is_none(),
            "case {case:?}: expected no VersionId when no value was stored, got {updated}"
        );

        let described = ok(&router, "DescribeSecret", json!({"SecretId": name}), &case).await;
        assert_eq!(
            described.get(member).and_then(Value::as_str),
            Some(expected),
            "case {case:?}: expected {member} to read back as {expected:?}, got {described}"
        );
        // The value and its staging labels are untouched by a metadata update.
        let read = ok(&router, "GetSecretValue", json!({"SecretId": name}), &case).await;
        assert_eq!(
            text(&read, "VersionId", &case),
            v0,
            "case {case:?}: expected the current version to be unchanged, got {read}"
        );
        assert_eq!(
            described.get("VersionIdsToStages"),
            before.get("VersionIdsToStages"),
            "case {case:?}: expected the staging map to be unchanged, got {described}"
        );
    }
}

#[tokio::test]
async fn update_secret_stores_a_value_as_the_new_current_version() {
    // "If you include SecretString or SecretBinary to create a new secret
    // version, Secrets Manager automatically moves the staging label AWSCURRENT
    // to the new version. Then it attaches the label AWSPREVIOUS to the version
    // that AWSCURRENT was removed from." (`UpdateSecret`, this service's Smithy
    // model.) The rows run over the type and size of the value and over whether
    // the call names the new version's id itself.
    let values: [(&str, Value); 4] = [
        ("a short string", json!({"SecretString": "second"})),
        ("a long string", json!({"SecretString": "y".repeat(8192)})),
        ("a JSON document", json!({"SecretString": "{\"user\":\"b\"}"})),
        ("binary", json!({"SecretBinary": "AAECAwQ="})),
    ];
    for (label, value) in values {
        for named_token in [false, true] {
            let case = format!("{label}, the caller names the version: {named_token}");
            let router = emulator();
            let name = "update-value";
            let (_, v0) = create(&router, name, &case).await;

            let mut body = value.clone();
            let members = body.as_object_mut().expect("a request body is an object");
            members.insert("SecretId".to_string(), json!(name));
            if named_token {
                members.insert(
                    "ClientRequestToken".to_string(),
                    json!(version_id("update")),
                );
            }
            let updated = ok(&router, "UpdateSecret", body.clone(), &case).await;
            let version = text(&updated, "VersionId", &case);
            if named_token {
                assert_eq!(
                    version,
                    version_id("update"),
                    "case {case:?}: expected the named token to be the new version's id, got \
                     {updated}"
                );
            }
            assert_ne!(
                version, v0,
                "case {case:?}: expected a NEW version rather than the one already stored"
            );

            // The new version is what a read resolves, and it carries the value.
            let read = ok(&router, "GetSecretValue", json!({"SecretId": name}), &case).await;
            assert_eq!(
                text(&read, "VersionId", &case),
                version,
                "case {case:?}: expected the new version to be current, got {read}"
            );
            for member in ["SecretString", "SecretBinary"] {
                assert_eq!(
                    read.get(member),
                    value.get(member),
                    "case {case:?}: expected {member} to read back as stored, got {read}"
                );
            }
            // The version AWSCURRENT left carries AWSPREVIOUS.
            let rows = stages(&router, name, &case).await;
            let previous: Vec<&String> = rows
                .iter()
                .filter(|(_, labels)| labels.iter().any(|held| held == "AWSPREVIOUS"))
                .map(|(version, _)| version)
                .collect();
            assert_eq!(
                previous,
                vec![&v0],
                "case {case:?}: expected AWSPREVIOUS on the version AWSCURRENT left, got {rows:?}"
            );

            if named_token {
                // The token is an @idempotencyToken: the same token with the
                // same value is the same request and changes nothing.
                let repeated = ok(&router, "UpdateSecret", body.clone(), &case).await;
                assert_eq!(
                    text(&repeated, "VersionId", &case),
                    version,
                    "case {case:?}: expected an idempotent repeat to report the same version, got \
                     {repeated}"
                );
                let versions = ok(
                    &router,
                    "ListSecretVersionIds",
                    json!({"SecretId": name, "IncludeDeprecated": true}),
                    &case,
                )
                .await;
                let count = versions
                    .get("Versions")
                    .and_then(Value::as_array)
                    .map(Vec::len)
                    .unwrap_or_default();
                assert_eq!(
                    count, 2,
                    "case {case:?}: expected the repeat to create no third version, got {versions}"
                );

                // The same token over a DIFFERENT value would modify an existing
                // version, which this operation never does.
                let mut conflicting = body.clone();
                let members = conflicting
                    .as_object_mut()
                    .expect("a request body is an object");
                members.remove("SecretBinary");
                members.insert("SecretString".to_string(), json!("a different value"));
                let (code, message) = err(&router, "UpdateSecret", conflicting, &case).await;
                assert_eq!(
                    code, "ResourceExistsException",
                    "case {case:?}: expected ResourceExistsException for a reused token, got \
                     {code:?} with message {message:?}"
                );
            }
        }
    }
}

#[tokio::test]
async fn update_secret_refuses_both_value_members_in_one_call() {
    // "Either SecretBinary or SecretString must have a value, but not both."
    // (`UpdateSecretRequest$SecretString`, this service's Smithy model.) The rows
    // run over the size of each value, since the refusal is of the two members
    // APPEARING together.
    const EXPECTED: &str =
        "You can't specify both a binary secret value and a string secret value in the same \
         secret.";
    let pairs: [(&str, Value); 3] = [
        ("short values", json!({"SecretString": "a", "SecretBinary": "AAECAwQ="})),
        (
            "a long string",
            json!({"SecretString": "z".repeat(4096), "SecretBinary": "AAECAwQ="}),
        ),
        (
            "a long blob",
            json!({"SecretString": "a", "SecretBinary": "AA".repeat(512)}),
        ),
    ];
    for (label, value) in pairs {
        let case = label.to_string();
        let router = emulator();
        let name = "update-both-values";
        let (_, v0) = create(&router, name, &case).await;
        let mut body = value.clone();
        body.as_object_mut()
            .expect("a request body is an object")
            .insert("SecretId".to_string(), json!(name));
        let (code, message) = err(&router, "UpdateSecret", body, &case).await;
        assert_eq!(
            code, "InvalidParameterException",
            "case {case:?}: expected InvalidParameterException, got {code:?} with message \
             {message:?}"
        );
        assert_eq!(
            message, EXPECTED,
            "case {case:?}: expected {EXPECTED:?}, got {message:?}"
        );
        // The refused call stored nothing.
        let read = ok(&router, "GetSecretValue", json!({"SecretId": name}), &case).await;
        assert_eq!(
            text(&read, "VersionId", &case),
            v0,
            "case {case:?}: expected the refused call to leave the current version alone, got \
             {read}"
        );
    }
}

#[tokio::test]
async fn a_mutation_addressed_to_a_secret_this_service_does_not_hold_is_not_found() {
    // A mutation gets the SAME specific answer a read gets for a `SecretId` that
    // names nothing: "Secrets Manager can't find the resource that you asked
    // for." (`ResourceNotFoundException`, this service's Smithy model.) The rows
    // run over the operation, over the form of the id, and over a secret that
    // never existed against one whose deletion skipped the recovery window.
    let account = engine::EMULATOR_ACCOUNT_ID;
    let never_created = [
        "never-created".to_string(),
        "folder/path/never-created".to_string(),
        format!("arn:aws:secretsmanager:us-west-2:{account}:secret:never-created-AbCdEf"),
        format!("arn:aws:secretsmanager:us-west-2:{account}:secret:never-created"),
    ];
    let mutations: [(&str, Value); 3] = [
        ("UpdateSecret", json!({"Description": "post-mortem"})),
        ("UpdateSecret", json!({"SecretString": "a value"})),
        (
            "UpdateSecretVersionStage",
            json!({"VersionStage": "AWSCURRENT", "MoveToVersionId": version_id("v0")}),
        ),
    ];

    for (operation, extra) in &mutations {
        for secret_id in &never_created {
            let case = format!("{operation} on {secret_id:?}");
            let router = emulator();
            // A secret that DOES exist, so the store is not merely empty.
            create(&router, "an-existing-secret", &case).await;
            let mut body = extra.clone();
            body.as_object_mut()
                .expect("every row's members are an object")
                .insert("SecretId".to_string(), json!(secret_id));
            let (code, message) = err(&router, operation, body, &case).await;
            assert_eq!(
                code, "ResourceNotFoundException",
                "case {case:?}: expected ResourceNotFoundException, got {code:?} with message \
                 {message:?}"
            );
            assert_eq!(
                message, SECRET_NOT_FOUND,
                "case {case:?}: expected the constant sentence {SECRET_NOT_FOUND:?}, got \
                 {message:?}"
            );
        }
    }

    // A deletion that skipped the recovery window is permanent once its short
    // asynchronous delay has elapsed, and the mutation then reports the secret
    // missing with the same sentence.
    for (operation, extra) in &mutations {
        let case = format!("{operation} on a force-deleted secret");
        let router = emulator();
        let name = "force-deleted";
        create(&router, name, &case).await;
        let started = Instant::now();
        ok(
            &router,
            "DeleteSecret",
            json!({"SecretId": name, "ForceDeleteWithoutRecovery": true}),
            &case,
        )
        .await;
        // While the delay has not elapsed the secret still exists and is refused
        // by its state; past it, it is gone. Both are the documented answers, so
        // the assertion is on whichever the clock makes due.
        let mut body = extra.clone();
        body.as_object_mut()
            .expect("every row's members are an object")
            .insert("SecretId".to_string(), json!(name));
        let (code, message) = err(&router, operation, body, &case).await;
        if started.elapsed() < FORCE_DELETE_PURGE_DELAY {
            assert_eq!(
                code, "InvalidRequestException",
                "case {case:?}: expected the secret to still exist inside its purge delay, got \
                 {code:?} with message {message:?}"
            );
            assert_eq!(
                message, MARKED_FOR_DELETION,
                "case {case:?}: expected {MARKED_FOR_DELETION:?}, got {message:?}"
            );
            continue;
        }
        assert_eq!(
            code, "ResourceNotFoundException",
            "case {case:?}: expected ResourceNotFoundException once the deletion is final, got \
             {code:?} with message {message:?}"
        );
        assert_eq!(
            message, SECRET_NOT_FOUND,
            "case {case:?}: expected the constant sentence {SECRET_NOT_FOUND:?}, got {message:?}"
        );
    }
}

#[tokio::test]
async fn awscurrent_moves_between_versions_and_never_comes_off_the_secret() {
    // `AWSCURRENT` names the version a `GetSecretValue` that asks for no version
    // answers with, so every secret has exactly one version carrying it for as
    // long as the secret exists. A call that names `RemoveFromVersionId` and no
    // `MoveToVersionId` asks to take a label off that version, which for this
    // label would leave the secret with no current value: it is refused and no
    // label moves. Moving it to another version is the same call WITH
    // `MoveToVersionId`, and that still works. The rows run over how many
    // versions the secret holds, whether the label sits on the version the
    // secret was created with or on one a later put promoted, and the form of
    // the `SecretId`.
    const EXPECTED: &str = "You can only move staging label AWSCURRENT to a different secret \
                            version. It can't be completely removed.";
    // (versions stored beside the first, whether a later put promotes AWSCURRENT,
    //  whether the call addresses the secret by ARN)
    let rows = [
        (0usize, false, false),
        (1, false, true),
        (1, true, false),
        (3, true, true),
    ];
    for (extra_versions, promote, by_arn) in rows {
        let case = format!(
            "{extra_versions} extra version(s), promoted={promote}, addressed by \
             {}",
            if by_arn { "ARN" } else { "name" }
        );
        let router = emulator();
        let name = "stage-current-removal";
        let (arn, _) = create(&router, name, &case).await;
        for index in 0..extra_versions {
            let token = version_id(&format!("side{index}"));
            put(&router, name, &token, json!([format!("side-{index}")]), &case).await;
        }
        if promote {
            put(
                &router,
                name,
                &version_id("promoted"),
                json!(["AWSCURRENT"]),
                &case,
            )
            .await;
        }
        let secret_id = if by_arn { arn.clone() } else { name.to_string() };

        let before = stages(&router, &secret_id, &case).await;
        let current = holder_of(&before, "AWSCURRENT")
            .unwrap_or_else(|| panic!("case {case:?}: no version holds AWSCURRENT in {before:?}"));

        let (code, message) = err(
            &router,
            "UpdateSecretVersionStage",
            json!({
                "SecretId": secret_id.clone(),
                "VersionStage": "AWSCURRENT",
                "RemoveFromVersionId": current.clone(),
            }),
            &case,
        )
        .await;
        assert_eq!(
            code, "InvalidParameterException",
            "case {case:?}: expected InvalidParameterException for a removal of AWSCURRENT, got \
             {code:?} with message {message:?}"
        );
        assert_eq!(
            message, EXPECTED,
            "case {case:?}: expected {EXPECTED:?}, got {message:?}"
        );

        let after = stages(&router, &secret_id, &case).await;
        assert_eq!(
            after, before,
            "case {case:?}: expected the refused removal to change no label, got {after:?} after \
             {before:?}"
        );

        // The same label DOES move: the call that names a version to move it to
        // is accepted, and the secret's current value follows the label.
        let target = put(
            &router,
            name,
            &version_id("next"),
            json!(["staged"]),
            &case,
        )
        .await;
        ok(
            &router,
            "UpdateSecretVersionStage",
            json!({
                "SecretId": secret_id.clone(),
                "VersionStage": "AWSCURRENT",
                "MoveToVersionId": target.clone(),
                "RemoveFromVersionId": current.clone(),
            }),
            &case,
        )
        .await;
        let read = ok(
            &router,
            "GetSecretValue",
            json!({"SecretId": secret_id.clone()}),
            &case,
        )
        .await;
        assert_eq!(
            text(&read, "VersionId", &case),
            target,
            "case {case:?}: expected the current value to follow the moved label, got {read}"
        );
    }
}

#[tokio::test]
async fn a_staging_label_other_than_awscurrent_comes_off_the_version_holding_it() {
    // Removing a label is what a call with no `MoveToVersionId` does — "to
    // remove a label from a version, then do not specify this parameter"
    // (`UpdateSecretVersionStageRequest$MoveToVersionId`, this service's Smithy
    // model) — and only the label naming the secret's current value is exempt.
    // The rows run over the label removed: the two reserved ones the service
    // also moves itself, and labels a caller named.
    for label in ["AWSPREVIOUS", "AWSPENDING", "blue", "rotation-candidate"] {
        let case = format!("removing {label:?}");
        let router = emulator();
        let name = "stage-other-removal";
        create(&router, name, &case).await;
        // `AWSPREVIOUS` lands on the version `AWSCURRENT` left, so it is staged
        // by promoting a second version rather than by naming it directly.
        let held_by = if label == "AWSPREVIOUS" {
            let first = holder_of(&stages(&router, name, &case).await, "AWSCURRENT")
                .unwrap_or_else(|| panic!("case {case:?}: the new secret has a current version"));
            put(
                &router,
                name,
                &version_id("promoted"),
                json!(["AWSCURRENT"]),
                &case,
            )
            .await;
            first
        } else {
            put(&router, name, &version_id("staged"), json!([label]), &case).await
        };
        let before = stages(&router, name, &case).await;
        assert_eq!(
            holder_of(&before, label).as_deref(),
            Some(held_by.as_str()),
            "case {case:?}: expected {label} to start on version {held_by}, got {before:?}"
        );

        ok(
            &router,
            "UpdateSecretVersionStage",
            json!({
                "SecretId": name,
                "VersionStage": label,
                "RemoveFromVersionId": held_by.clone(),
            }),
            &case,
        )
        .await;

        let after = stages(&router, name, &case).await;
        assert_eq!(
            holder_of(&after, label),
            None,
            "case {case:?}: expected no version to hold {label} after the removal, got {after:?}"
        );
        // The secret still has a current value: taking another label off never
        // touches the one that names it.
        assert!(
            holder_of(&after, "AWSCURRENT").is_some(),
            "case {case:?}: expected AWSCURRENT to survive the removal, got {after:?}"
        );
        // The version is still addressable by its id, label or no label.
        let read = ok(
            &router,
            "GetSecretValue",
            json!({"SecretId": name, "VersionId": held_by.clone()}),
            &case,
        )
        .await;
        assert_eq!(
            text(&read, "VersionId", &case),
            held_by,
            "case {case:?}: expected the version to stay readable by id, got {read}"
        );
    }
}

#[tokio::test]
async fn a_remove_from_version_id_that_is_not_where_the_label_is_is_a_parameter_refusal() {
    // "If the label is attached and you either do not specify this parameter, or
    // the version ID does not match, then the operation fails."
    // (`UpdateSecretVersionStageRequest$RemoveFromVersionId`, this service's
    // Smithy model.) What fails is the VALUE the request carried — the same call
    // naming the version the label is really on is served — so the whole class is
    // the parameter refusal, `InvalidParameterException`, whether the parameter is
    // missing or names the wrong version.
    //
    // The rows span what the rule generalises over: the label (both reserved ones
    // and a caller's own), what `RemoveFromVersionId` names (a version the secret
    // holds but the label is not on, a version id the secret never held, and the
    // version the label is being moved TO), and whether the call also names a
    // `MoveToVersionId` at all.
    for label in ["AWSCURRENT", "AWSPENDING", "blue"] {
        let router = emulator();
        let name = "stale-remove-from";
        let case = format!("label {label:?}");
        let (_, v0) = create(&router, name, &case).await;
        let v1 = put(&router, name, &version_id("v1"), json!(["AWSPENDING"]), &case).await;
        let v2 = put(&router, name, &version_id("v2"), json!(["blue"]), &case).await;
        // Where each label starts: AWSCURRENT on v0, AWSPENDING on v1, blue on v2.
        let holder = match label {
            "AWSCURRENT" => v0.clone(),
            "AWSPENDING" => v1.clone(),
            _ => v2.clone(),
        };
        // The version the label is moved to: never the one that holds it.
        let target = if holder == v0 { v1.clone() } else { v0.clone() };
        let before = stages(&router, name, &case).await;

        let stale: Vec<(&str, String)> = vec![
            (
                "a version the secret holds but the label is not on",
                if holder == v2 { v1.clone() } else { v2.clone() },
            ),
            (
                "a version id the secret has never held",
                version_id("never-stored"),
            ),
            ("the version the label is being moved to", target.clone()),
        ];
        for (row, named) in stale {
            // A row that happens to name the holder is not a refusal at all.
            if named == holder {
                continue;
            }
            let case = format!("{case}, RemoveFromVersionId naming {row}");
            let (code, message) = err(
                &router,
                "UpdateSecretVersionStage",
                json!({
                    "SecretId": name,
                    "VersionStage": label,
                    "MoveToVersionId": target.clone(),
                    "RemoveFromVersionId": named.clone(),
                }),
                &case,
            )
            .await;
            assert_eq!(
                code, "InvalidParameterException",
                "case {case:?}: expected InvalidParameterException, got {code:?} with message \
                 {message:?}"
            );
            let expected = format!(
                "When you move staging label {label}, if you specify RemoveFromVersionId, it \
                 must be set to the version that currently has the staging label {holder}."
            );
            assert_eq!(
                message, expected,
                "case {case:?}: expected {expected:?}, got {message:?}"
            );
            assert_eq!(
                stages(&router, name, &case).await,
                before,
                "case {case:?}: expected the refused call to leave the staging map untouched"
            );
        }

        // The same class with no `MoveToVersionId` at all: a removal that names a
        // version the label is not on. `AWSCURRENT` is refused before this rule —
        // it is only ever moved — so the removal rows are the other labels'.
        if label != "AWSCURRENT" {
            let named = if holder == v2 { v1.clone() } else { v2.clone() };
            let case = format!("{case}, a removal naming a version the label is not on");
            let (code, message) = err(
                &router,
                "UpdateSecretVersionStage",
                json!({
                    "SecretId": name,
                    "VersionStage": label,
                    "RemoveFromVersionId": named.clone(),
                }),
                &case,
            )
            .await;
            assert_eq!(
                code, "InvalidParameterException",
                "case {case:?}: expected InvalidParameterException, got {code:?} with message \
                 {message:?}"
            );
            assert!(
                message.contains(label) && message.contains(&holder),
                "case {case:?}: expected the refusal to name the label and where it is, got \
                 {message:?}"
            );
            assert_eq!(
                stages(&router, name, &case).await,
                before,
                "case {case:?}: expected the refused call to leave the staging map untouched"
            );
        }

        // Naming the holder is what makes the same move succeed, so the refusals
        // above are about the value, not about the move.
        let case = format!("{case}, RemoveFromVersionId naming the holder");
        ok(
            &router,
            "UpdateSecretVersionStage",
            json!({
                "SecretId": name,
                "VersionStage": label,
                "MoveToVersionId": target.clone(),
                "RemoveFromVersionId": holder.clone(),
            }),
            &case,
        )
        .await;
        let after = stages(&router, name, &case).await;
        let holders: Vec<&String> = after
            .iter()
            .filter(|(_, labels)| labels.iter().any(|held| held == label))
            .map(|(version, _)| version)
            .collect();
        assert_eq!(
            holders,
            vec![&target],
            "case {case:?}: expected {label:?} to end up on {target}, got {after:?}"
        );
    }
}
