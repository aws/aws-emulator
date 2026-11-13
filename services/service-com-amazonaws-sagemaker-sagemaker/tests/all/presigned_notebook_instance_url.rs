// SPDX-License-Identifier: CC0-1.0

//! End-to-end wire tests for SageMaker's `CreatePresignedNotebookInstanceUrl`, read back
//! through `DescribeNotebookInstance` and through the grant the call records on the
//! instance.
//!
//! Every case goes over the wire (`POST /` with `X-Amz-Target: SageMaker.<Op>`,
//! `awsJson1_1`) through the same `MultiServiceRouter` the production router uses, so
//! what these tests exercise is what a real SDK client gets. The harness is this crate's
//! shared one, from `tests/all/inference_deployment_lifecycle.rs`, and the
//! notebook-instance request shape comes from the suite that creates them.
//!
//! The tests assert the RULES over a TABLE of inputs rather than one recorded call:
//! the `AuthorizedUrl` addresses the session path on THIS emulator's own endpoint (never a
//! host of AWS's own, which no client of the emulator resolves — the address and the
//! session served at it are `tests/all/presigned_session.rs`'s subject) and carries a grant
//! token this call recorded on the instance; the URL itself is valid for the documented 5
//! minutes and opens a session of the requested duration (12 hours when the request names
//! none); every call mints its own grant; only an instance actually serving Jupyter has a
//! URL; and a name nothing created, or a session duration outside the modelled range, is
//! refused.

use std::time::Duration;

use serde_json::{json, Value};
use service_com_amazonaws_sagemaker_sagemaker::state as sm;

use super::inference_deployment_lifecycle::{error_code, message, str_field, Emulator, ACCOUNT};
use super::notebook_instance_lifecycle::{create_body, describe, PAST_IN_SERVICE_DUE};

/// Long enough to pass `state::NOTEBOOK_INSTANCE_STOPPED_AFTER` (300 ms).
const PAST_STOPPED_DUE: Duration = Duration::from_millis(600);

/// How long the URL itself stays valid, from the operation's own documentation: "The URL
/// that you get from a call to CreatePresignedNotebookInstanceUrl is valid only for 5
/// minutes."
const DOCUMENTED_URL_TTL: Duration = Duration::from_secs(300);

/// The session the URL opens when the request names no
/// `SessionExpirationDurationInSeconds`: the member's doc says "the default is 12 hours".
const DOCUMENTED_DEFAULT_SESSION_SECONDS: u64 = 12 * 60 * 60;

/// The presigned-URL grants the stored instance holds, exactly as the handler recorded
/// them in the one shared store every handler reaches.
fn grants(emulator: &Emulator, name: &str) -> Vec<sm::PresignedUrlGrant> {
    let state =
        sm::service_state(&*emulator.engine).expect("the SageMaker store is on the shared engine");
    let guard = state.lock();
    guard
        .notebook_instances
        .get(name)
        .map(|notebook| notebook.url_grants.clone())
        .unwrap_or_default()
}

/// The `authToken` query parameter of `url`, DECODED — the form the grant holds, since
/// the mint percent-encodes the token into the query.
fn token_of(url: &str) -> String {
    let encoded = url
        .split("authToken=")
        .nth(1)
        .and_then(|rest| rest.split('&').next())
        .unwrap_or_default();
    smithy_server_runtime::percent_decode(encoded)
}

/// `CreatePresignedNotebookInstanceUrl` for `name` in `region`, with whatever `extra`
/// carries beside the required member.
async fn presign(emulator: &Emulator, region: &str, name: &str, extra: &Value) -> (u16, Value) {
    let mut body = json!({ "NotebookInstanceName": name });
    for (key, value) in extra.as_object().expect("a JSON object of extra members") {
        body[key] = value.clone();
    }
    emulator
        .call_in(region, "CreatePresignedNotebookInstanceUrl", &body)
        .await
}

/// RULE: the `AuthorizedUrl` addresses the session path on this emulator's own endpoint
/// and carries an opaque grant token the call recorded on that instance; the grant is
/// valid for the documented 5 minutes and opens a session of the requested duration,
/// defaulting to the documented 12 hours.
#[tokio::test]
async fn a_presigned_url_addresses_the_instances_own_jupyter_host_with_a_recorded_grant() {
    // Rows differ along what the rule generalises over: whether the request names a
    // session duration and which one (each published bound of `range 1800..43200` and one
    // in between), the signed region, and the name's shape (including the 63-character
    // modelled maximum).
    let longest = format!("{}b", "a".repeat(62));
    let cases: Vec<(&str, &str, &str, Value, u64)> = vec![
        (
            "default-session",
            "us-east-1",
            "nb",
            json!({}),
            DOCUMENTED_DEFAULT_SESSION_SECONDS,
        ),
        (
            "session-on-the-lower-bound",
            "us-east-1",
            "nb-1800",
            json!({ "SessionExpirationDurationInSeconds": 1800 }),
            1800,
        ),
        (
            "session-in-between",
            "eu-west-1",
            "demo-notebook-with-hyphens",
            json!({ "SessionExpirationDurationInSeconds": 3600 }),
            3600,
        ),
        (
            "session-on-the-upper-bound",
            "ap-southeast-2",
            longest.as_str(),
            json!({ "SessionExpirationDurationInSeconds": 43200 }),
            43200,
        ),
    ];

    for (case, region, name, extra, session_seconds) in cases {
        let emulator = Emulator::new();
        let (status, created) = emulator
            .call_in(region, "CreateNotebookInstance", &create_body(name, &json!({})))
            .await;
        assert_eq!(status, 200, "case {case}: CreateNotebookInstance {created}");
        tokio::time::sleep(PAST_IN_SERVICE_DUE).await;

        let (status, minted) = presign(&emulator, region, name, &extra).await;
        assert_eq!(
            status, 200,
            "case {case}: expected HTTP 200 from CreatePresignedNotebookInstanceUrl, got \
             {status} {minted}"
        );
        let url = str_field(&minted, "AuthorizedUrl");

        // The instance's own `Url` is what the READER reports, as AWS reports it; the
        // URL a caller is told to FETCH is not built on it — no client of this emulator
        // resolves an AWS host — so it addresses the session path on the endpoint this
        // emulator publishes.
        let (status, described) = emulator
            .call_in(
                region,
                "DescribeNotebookInstance",
                &json!({ "NotebookInstanceName": name }),
            )
            .await;
        assert_eq!(status, 200, "case {case}: DescribeNotebookInstance {described}");
        let host = str_field(&described, "Url");
        assert_eq!(
            host,
            format!("{name}.notebook.{region}.sagemaker.aws"),
            "case {case}: the instance's Jupyter host, got {described:?}"
        );
        let expected = format!(
            "{}/_aws/sagemaker/notebook-instance/{name}/auth?authToken=",
            smithy_server_runtime::UNSERVED_URL_BASE
        );
        assert!(
            url.starts_with(&expected),
            "case {case}: expected the URL to address {expected:?}, got {url:?}"
        );

        // The token is a grant the instance holds, not a value invented for the response.
        let token = token_of(&url);
        assert!(
            !token.is_empty(),
            "case {case}: the URL must carry a grant token, got {url:?}"
        );
        let held = grants(&emulator, name);
        let grant = held
            .iter()
            .find(|grant| grant.token == token)
            .unwrap_or_else(|| {
                panic!("case {case}: the token in the URL must be a grant the instance holds, got {held:?} for {url:?}")
            });
        let url_ttl = grant
            .expires_at
            .duration_since(grant.issued_at)
            .expect("the URL expiry follows the instant it was issued");
        assert_eq!(
            url_ttl, DOCUMENTED_URL_TTL,
            "case {case}: the URL's own validity is the documented 5 minutes, got \
             {url_ttl:?}"
        );
        let session = grant
            .session_expires_at
            .duration_since(grant.issued_at)
            .expect("the session expiry follows the instant it was issued");
        assert_eq!(
            session,
            Duration::from_secs(session_seconds),
            "case {case}: the session must last the requested duration, got {session:?}"
        );
    }
}

/// RULE: every call mints its own grant, and the instance keeps the ones whose session
/// could still be in use; minting a URL changes nothing a later reader sees.
#[tokio::test]
async fn each_call_mints_its_own_grant_and_leaves_the_instance_as_it_was() {
    let emulator = Emulator::new();
    emulator
        .ok("CreateNotebookInstance", &create_body("repeat", &json!({})))
        .await;
    tokio::time::sleep(PAST_IN_SERVICE_DUE).await;
    let before = describe(&emulator, "repeat").await;

    // Rows: the same instance asked repeatedly, with and without a session duration.
    let requests = [
        json!({}),
        json!({ "SessionExpirationDurationInSeconds": 1800 }),
        json!({ "SessionExpirationDurationInSeconds": 43200 }),
        json!({}),
    ];
    let mut tokens: Vec<String> = Vec::new();
    for (index, extra) in requests.iter().enumerate() {
        let case = format!("call-{index}");
        let (status, minted) = presign(&emulator, "us-east-1", "repeat", extra).await;
        assert_eq!(
            status, 200,
            "case {case}: expected HTTP 200, got {status} {minted}"
        );
        let token = token_of(&str_field(&minted, "AuthorizedUrl"));
        assert!(
            !tokens.contains(&token),
            "case {case}: each call mints its own grant, got the earlier {token:?}"
        );
        tokens.push(token);

        let held = grants(&emulator, "repeat");
        assert_eq!(
            held.len(),
            index + 1,
            "case {case}: the instance keeps every unexpired grant, got {held:?}"
        );
        for issued in &tokens {
            assert!(
                held.iter().any(|grant| &grant.token == issued),
                "case {case}: the instance must still hold {issued:?}, got {held:?}"
            );
        }
    }

    // The mint is about the grant, not about the instance: nothing a reader reports moved.
    let after = describe(&emulator, "repeat").await;
    assert_eq!(
        after, before,
        "case reader-unchanged: minting a presigned URL must leave what a later reader \
         sees untouched"
    );
}

/// RULE: the URL connects its holder to the Jupyter server ON the instance, so only an
/// instance that is serving one has a URL; from every other status the call is refused
/// with a 400 `ValidationException` naming the status it found, and no grant is recorded.
#[tokio::test]
async fn a_presigned_url_is_minted_only_for_an_instance_serving_jupyter() {
    // Rows: every status this slice can place an instance in — still coming up, serving,
    // a stop in flight, and stopped.
    let cases = [
        ("still-pending", false, false, false),
        ("in-service", true, false, false),
        ("stop-in-flight", true, true, false),
        ("stopped", true, true, true),
    ];

    for (case, wait_for_in_service, stop_it, wait_out_the_stop) in cases {
        let emulator = Emulator::new();
        emulator
            .ok("CreateNotebookInstance", &create_body(case, &json!({})))
            .await;
        if wait_for_in_service {
            tokio::time::sleep(PAST_IN_SERVICE_DUE).await;
        }
        if stop_it {
            let (status, body) = emulator
                .call("StopNotebookInstance", &json!({ "NotebookInstanceName": case }))
                .await;
            assert_eq!(status, 200, "case {case}: StopNotebookInstance {status} {body}");
        }
        if wait_out_the_stop {
            tokio::time::sleep(PAST_STOPPED_DUE).await;
        }

        let (status, answered) = presign(&emulator, "us-east-1", case, &json!({})).await;
        if case == "in-service" {
            assert_eq!(
                status, 200,
                "case {case}: a serving instance has a URL, got {status} {answered}"
            );
            continue;
        }
        // A row that did not wait out a span is normally still in the transient status;
        // on a loaded box the clock may have settled it, which the rule covers too, so the
        // outcome is read rather than demanded of the clock.
        let settled_early = status == 200;
        if settled_early {
            let found = str_field(&describe(&emulator, case).await, "NotebookInstanceStatus");
            assert_eq!(
                found, "InService",
                "case {case}: a URL was minted, so the instance must be serving, got \
                 {found}"
            );
            continue;
        }
        assert_eq!(
            status, 400,
            "case {case}: an instance serving no Jupyter server has no URL, got {status} \
             {answered}"
        );
        assert_eq!(
            error_code(&answered),
            "ValidationException",
            "case {case}: error code, got {answered:?}"
        );
        let refusal = message(&answered);
        assert!(
            refusal.contains(&format!("{ACCOUNT}:notebook-instance/{case}"))
                && ["Pending", "Stopping", "Stopped"]
                    .iter()
                    .any(|status| refusal.contains(status))
                && refusal.contains("InService"),
            "case {case}: the refusal must name the instance, the status it found and the \
             status it needs, got {refusal:?}"
        );
        assert!(
            answered.get("AuthorizedUrl").is_none(),
            "case {case}: a refusal carries no URL, got {answered:?}"
        );
        let held = grants(&emulator, case);
        assert!(
            held.is_empty(),
            "case {case}: a refused call records no grant, got {held:?}"
        );
    }
}

/// RULE: a `NotebookInstanceName` the store does not hold is refused with a 400
/// `ValidationException` naming the ARN it could not find, in the region the caller
/// signed for.
#[tokio::test]
async fn an_absent_notebook_instance_is_refused_with_a_400_validation_exception() {
    // Rows: legal name shapes nothing created, across signed regions.
    let longest = format!("{}b", "a".repeat(62));
    let names = ["absent", "a", "never-created-notebook", longest.as_str()];
    for region in ["us-east-1", "eu-central-1"] {
        for name in names {
            let emulator = Emulator::new();
            let case = format!("region={region} name={name}");
            let (status, parsed) = presign(&emulator, region, name, &json!({})).await;
            assert_eq!(
                status, 400,
                "case {case}: expected HTTP 400 for an absent instance, got {status} \
                 {parsed}"
            );
            assert_eq!(
                error_code(&parsed),
                "ValidationException",
                "case {case}: error code, got {parsed:?}"
            );
            assert!(
                message(&parsed).contains(&format!(
                    "arn:aws:sagemaker:{region}:{ACCOUNT}:notebook-instance/{name}"
                )),
                "case {case}: the refusal must name the instance it could not find, got \
                 {parsed:?}"
            );
        }
    }

    // An unsigned call names no region, so the ARN it reports carries the emulator's
    // default one.
    let emulator = Emulator::new();
    let (status, parsed) = emulator
        .call_unsigned(
            "CreatePresignedNotebookInstanceUrl",
            &json!({ "NotebookInstanceName": "absent-unsigned" }),
        )
        .await;
    assert_eq!(status, 400, "case unsigned: expected HTTP 400, got {status} {parsed}");
    assert!(
        message(&parsed).contains(&format!(
            "arn:aws:sagemaker:us-east-1:{ACCOUNT}:notebook-instance/absent-unsigned"
        )),
        "case unsigned: the refusal must fall back to the default region, got {parsed:?}"
    );
}

/// RULE: a request outside a member's modelled constraint is refused at the boundary with
/// HTTP 400 before the handler runs; a request ON each published bound is served.
#[tokio::test]
async fn a_request_outside_the_modelled_constraints_is_refused() {
    let emulator = Emulator::new();
    emulator
        .ok("CreateNotebookInstance", &create_body("bounds", &json!({})))
        .await;
    tokio::time::sleep(PAST_IN_SERVICE_DUE).await;

    // Rows: one past each published bound — `SessionExpirationDurationInSeconds` is
    // `range 1800..43200` and `NotebookInstanceName` is `len 0..63` with pattern
    // `^[a-zA-Z0-9](-*[a-zA-Z0-9])*$` (smithy_query
    // operation=CreatePresignedNotebookInstanceUrl).
    let too_long = "a".repeat(64);
    let refused: Vec<(&str, Value)> = vec![
        (
            "session-below-1800",
            json!({ "NotebookInstanceName": "bounds", "SessionExpirationDurationInSeconds": 1799 }),
        ),
        (
            "session-above-43200",
            json!({ "NotebookInstanceName": "bounds", "SessionExpirationDurationInSeconds": 43201 }),
        ),
        (
            "session-of-zero",
            json!({ "NotebookInstanceName": "bounds", "SessionExpirationDurationInSeconds": 0 }),
        ),
        ("name-past-63", json!({ "NotebookInstanceName": too_long })),
        (
            "name-with-an-illegal-character",
            json!({ "NotebookInstanceName": "not a notebook name" }),
        ),
        ("no-name-at-all", json!({})),
    ];
    for (case, body) in refused {
        let (status, parsed) = emulator
            .call("CreatePresignedNotebookInstanceUrl", &body)
            .await;
        assert_eq!(
            status, 400,
            "case {case}: the request must be refused, got {status} {parsed}"
        );
        assert!(
            grants(&emulator, "bounds").is_empty(),
            "case {case}: a refused request records no grant"
        );
    }

    // ON the bounds: each published extreme is served.
    for seconds in [1800, 43200] {
        let (status, minted) = presign(
            &emulator,
            "us-east-1",
            "bounds",
            &json!({ "SessionExpirationDurationInSeconds": seconds }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case session-on-{seconds}: expected HTTP 200, got {status} {minted}"
        );
        assert!(
            !token_of(&str_field(&minted, "AuthorizedUrl")).is_empty(),
            "case session-on-{seconds}: the URL must carry a grant token, got {minted:?}"
        );
    }
}
