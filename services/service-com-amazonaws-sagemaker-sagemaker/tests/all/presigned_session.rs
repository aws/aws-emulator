// SPDX-License-Identifier: CC0-1.0

//! End-to-end wire tests for the URLs this service hands a caller to FETCH, and for the
//! session each of them opens.
//!
//! Three operations answer such a URL — `CreatePresignedNotebookInstanceUrl`,
//! `CreatePartnerAppPresignedUrl` and `CreatePresignedMlflowAppUrl` — and the RULE under
//! test is the one that covers all three: the URL addresses an endpoint THIS emulator
//! serves (the endpoint the request arrived on, which
//! `smithy_server_runtime::OperationContext::endpoint` reports), the emulator SERVES that
//! address, and it authenticates a session there only while the grant the minting call
//! recorded is live — a token no grant holds, and one whose window the clock has closed,
//! authenticate nothing.
//!
//! Every case goes over the wire through the same `MultiServiceRouter` the production
//! router uses, on the shared `Emulator` harness in `inference_deployment_lifecycle`: the
//! mint as an `awsJson1_1` call, and the published URL fetched the way a browser fetches
//! it (`Emulator::fetch`, a plain `GET` of the path and query the URL carries).
//!
//! In this harness the router is not served on a port, so the base it publishes is
//! `smithy_server_runtime::UNSERVED_URL_BASE` — the same value `ctx.endpoint()` answers
//! in-process, where a served emulator answers its data port's base. What the rows pin is
//! that the published URL is built on THAT base and on the session path, never on a host
//! of AWS's own: a client of the emulator resolves no `*.sagemaker.aws` name.

use std::time::{Duration, SystemTime};

use serde_json::{json, Value};
use service_com_amazonaws_sagemaker_sagemaker::state as sm;

use crate::inference_deployment_lifecycle::{str_field, Emulator};
use crate::mlflow_app_lifecycle::{minimal, mlflow_app_id};
use crate::notebook_instance_lifecycle::{create_body as notebook_body, PAST_IN_SERVICE_DUE};
use crate::partner_app_creation::{
    app_id_of, create_body as partner_app_body, subscribe, PARTNER_APP_TYPES,
};

/// The region every case signs for; a session address carries no region of its own, so
/// one region is enough and the ARNs the sessions report name it.
const REGION: &str = "us-east-1";

/// Which presigned-URL operation a row exercises.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Subject {
    /// `CreatePresignedNotebookInstanceUrl`, addressed by `NotebookInstanceName`.
    NotebookInstance,
    /// `CreatePartnerAppPresignedUrl`, addressed by the app's ARN.
    PartnerApp,
    /// `CreatePresignedMlflowAppUrl`, addressed by the app's ARN.
    MlflowApp,
}

impl Subject {
    /// The operation that mints the URL.
    fn operation(self) -> &'static str {
        match self {
            Self::NotebookInstance => "CreatePresignedNotebookInstanceUrl",
            Self::PartnerApp => "CreatePartnerAppPresignedUrl",
            Self::MlflowApp => "CreatePresignedMlflowAppUrl",
        }
    }

    /// The output member the URL travels in: `AuthorizedUrl` for both presigned
    /// `AuthorizedUrl` shapes, `Url` for the partner app
    /// (`smithy_query(query="operation",operation="CreatePresignedNotebookInstanceUrl,CreatePartnerAppPresignedUrl,CreatePresignedMlflowAppUrl")`).
    fn url_member(self) -> &'static str {
        match self {
            Self::PartnerApp => "Url",
            _ => "AuthorizedUrl",
        }
    }

    /// The ARN resource kind the session address names this subject by.
    fn kind(self) -> &'static str {
        match self {
            Self::NotebookInstance => "notebook-instance",
            Self::PartnerApp => "partner-app",
            Self::MlflowApp => "mlflow-app",
        }
    }
}

/// One prepared resource of a subject kind: what the mint is asked about, what the
/// session address names it by, and the ARN its session reports.
struct Prepared {
    subject: Subject,
    /// The value the mint request carries (a notebook name, or an app's ARN).
    identifier: String,
    /// The resource's own identifier in the session address.
    resource: String,
    arn: String,
}

/// Create a resource of `subject` named `name` through this service's own API and bring it
/// into the status its presign requires.
async fn prepare(emulator: &Emulator, subject: Subject, name: &str) -> Prepared {
    match subject {
        Subject::NotebookInstance => {
            let (status, created) = emulator
                .call_in(REGION, "CreateNotebookInstance", &notebook_body(name, &json!({})))
                .await;
            assert_eq!(
                status, 200,
                "case {name}: CreateNotebookInstance must succeed, got {status} {created}"
            );
            // A URL is minted only for an instance that is serving Jupyter, which is the
            // transition the clock owes after the create.
            tokio::time::sleep(PAST_IN_SERVICE_DUE).await;
            Prepared {
                subject,
                identifier: name.to_string(),
                resource: name.to_string(),
                arn: str_field(&created, "NotebookInstanceArn"),
            }
        }
        Subject::PartnerApp => {
            subscribe(emulator, &PARTNER_APP_TYPES);
            let (status, created) = emulator
                .call_in(
                    REGION,
                    "CreatePartnerApp",
                    &partner_app_body(name, PARTNER_APP_TYPES[0], &json!({})),
                )
                .await;
            assert_eq!(
                status, 200,
                "case {name}: CreatePartnerApp must succeed, got {status} {created}"
            );
            let arn = str_field(&created, "Arn");
            // A URL is minted only for an `Available` app.
            tokio::time::sleep(sm::PARTNER_APP_AVAILABLE_AFTER + Duration::from_millis(300)).await;
            Prepared {
                subject,
                resource: app_id_of(&arn).to_string(),
                identifier: arn.clone(),
                arn,
            }
        }
        Subject::MlflowApp => {
            let (status, created) = emulator
                .call_in(REGION, "CreateMlflowApp", &minimal(name))
                .await;
            assert_eq!(
                status, 200,
                "case {name}: CreateMlflowApp must succeed, got {status} {created}"
            );
            let arn = str_field(&created, "Arn");
            Prepared {
                subject,
                resource: mlflow_app_id(&arn).to_string(),
                identifier: arn.clone(),
                arn,
            }
        }
    }
}

/// Mint a presigned URL for `prepared`, with whatever `extra` members the row names.
async fn presign(emulator: &Emulator, prepared: &Prepared, extra: &Value) -> (u16, Value) {
    let mut body = match prepared.subject {
        Subject::NotebookInstance => json!({ "NotebookInstanceName": prepared.identifier }),
        _ => json!({ "Arn": prepared.identifier }),
    };
    for (member, value) in extra.as_object().expect("a JSON object of extra members") {
        body[member] = value.clone();
    }
    emulator
        .call_in(REGION, prepared.subject.operation(), &body)
        .await
}

/// The URL a successful mint published.
async fn published_url(emulator: &Emulator, case: &str, prepared: &Prepared, extra: &Value) -> String {
    let (status, minted) = presign(emulator, prepared, extra).await;
    assert_eq!(
        status, 200,
        "case {case:?}: {} must succeed, got {status} {minted}",
        prepared.subject.operation()
    );
    str_field(&minted, prepared.subject.url_member())
}

/// The grants the stored resource holds, exactly as the handler recorded them in the one
/// shared store every handler reaches.
fn grants(emulator: &Emulator, prepared: &Prepared) -> Vec<sm::PresignedUrlGrant> {
    let state =
        sm::service_state(&*emulator.engine).expect("the SageMaker store is on the shared engine");
    let guard = state.lock();
    match prepared.subject {
        Subject::NotebookInstance => guard
            .notebook_instances
            .get(&prepared.resource)
            .map(|notebook| notebook.url_grants.clone()),
        Subject::PartnerApp => guard
            .partner_apps
            .get(&prepared.resource)
            .map(|app| app.grants.clone()),
        Subject::MlflowApp => guard
            .mlflow_apps
            .get(&prepared.arn)
            .map(|app| app.grants.clone()),
    }
    .unwrap_or_default()
}

/// Move every grant of the stored resource `shift` into the past, so the window the mint
/// recorded has elapsed without the test waiting for it. The clock is what closes a
/// window, so a test that drives it this way needs no sleep and no wall-clock assertion.
fn backdate_grants(emulator: &Emulator, prepared: &Prepared, shift: Duration) {
    let state =
        sm::service_state(&*emulator.engine).expect("the SageMaker store is on the shared engine");
    let mut guard = state.lock();
    let held: Option<&mut Vec<sm::PresignedUrlGrant>> = match prepared.subject {
        Subject::NotebookInstance => guard
            .notebook_instances
            .get_mut(&prepared.resource)
            .map(|notebook| &mut notebook.url_grants),
        Subject::PartnerApp => guard
            .partner_apps
            .get_mut(&prepared.resource)
            .map(|app| &mut app.grants),
        Subject::MlflowApp => guard
            .mlflow_apps
            .get_mut(&prepared.arn)
            .map(|app| &mut app.grants),
    };
    for grant in held.expect("the prepared resource is in the store") {
        grant.issued_at -= shift;
        grant.expires_at -= shift;
        grant.session_expires_at -= shift;
    }
}

/// The `authToken` a published URL carries, DECODED — the form the grant holds, since the
/// mint percent-encodes the token into the query.
fn token_of(url: &str) -> String {
    let encoded = url
        .split("authToken=")
        .nth(1)
        .and_then(|rest| rest.split('&').next())
        .unwrap_or_default();
    smithy_server_runtime::percent_decode(encoded)
}

/// The address a session of `kind`/`resource` is published at: the base this emulator
/// publishes, the reserved `_aws` namespace, this service's plane, the resource, and the
/// `auth` segment the URL signs in at. Spelled out here rather than taken from the
/// service, so the suite pins the address instead of echoing it.
fn expected_prefix(kind: &str, resource: &str) -> String {
    format!(
        "{}/_aws/sagemaker/{kind}/{resource}/auth?authToken=",
        smithy_server_runtime::UNSERVED_URL_BASE
    )
}

/// The session JSON an open address answers.
fn parsed_session(case: &str, body: &str) -> Value {
    serde_json::from_str(body)
        .unwrap_or_else(|error| panic!("case {case:?}: the session must be JSON, got {body:?}: {error}"))
}

/// The rows the rule is asserted over: every operation that publishes a URL, each with
/// resource shapes and session windows that differ along what the rule generalises over —
/// which operation mints it, the resource's name shape and length, and which validity
/// members the request names (each published bound of `ExpiresInSeconds` 5..300 and
/// `SessionExpirationDurationInSeconds` 1800..43200, and neither).
fn rows() -> Vec<(&'static str, Subject, String, Value)> {
    let longest_notebook = format!("{}b", "a".repeat(62));
    vec![
        (
            "a notebook instance, no session window named",
            Subject::NotebookInstance,
            "session-nb".to_string(),
            json!({}),
        ),
        (
            "a notebook instance at the lower session bound",
            Subject::NotebookInstance,
            "session-nb-1800".to_string(),
            json!({ "SessionExpirationDurationInSeconds": 1800 }),
        ),
        (
            "a notebook instance with the longest name the model admits",
            Subject::NotebookInstance,
            longest_notebook,
            json!({ "SessionExpirationDurationInSeconds": 43200 }),
        ),
        (
            "a partner app, no validity members named",
            Subject::PartnerApp,
            "SessionPartnerApp".to_string(),
            json!({}),
        ),
        (
            "a partner app at both lower bounds",
            Subject::PartnerApp,
            "SessionPartnerAppBounds".to_string(),
            json!({ "ExpiresInSeconds": 5, "SessionExpirationDurationInSeconds": 1800 }),
        ),
        (
            "an MLflow App, no validity members named",
            Subject::MlflowApp,
            "session-mlflow".to_string(),
            json!({}),
        ),
        (
            "an MLflow App at both upper bounds",
            Subject::MlflowApp,
            "session-mlflow-bounds".to_string(),
            json!({ "ExpiresInSeconds": 300, "SessionExpirationDurationInSeconds": 43200 }),
        ),
    ]
}

/// RULE: a published URL addresses an endpoint this emulator serves — never a host of
/// AWS's own — carries a grant token the minting call recorded on the resource, and
/// fetching it opens the session that grant describes.
#[tokio::test]
async fn a_published_url_addresses_this_emulator_and_opens_the_recorded_session() {
    for (case, subject, name, extra) in rows() {
        let emulator = Emulator::new();
        let prepared = prepare(&emulator, subject, &name).await;
        let url = published_url(&emulator, case, &prepared, &extra).await;

        // The address: this emulator's own published base and the session path, with
        // nothing of AWS's own naming in it.
        let prefix = expected_prefix(subject.kind(), &prepared.resource);
        assert!(
            url.starts_with(&prefix),
            "case {case:?}: expected a URL starting {prefix:?}, got {url:?}"
        );
        assert!(
            !url.contains("sagemaker.aws") && !url.contains("amazonaws.com"),
            "case {case:?}: a published URL must not name a host AWS serves, got {url:?}"
        );

        // The token: a grant the resource holds, not a value invented for the response.
        let token = token_of(&url);
        let held = grants(&emulator, &prepared);
        let grant = held
            .iter()
            .find(|grant| grant.token == token)
            .unwrap_or_else(|| {
                panic!("case {case:?}: the token in {url:?} must be a grant the resource holds, got {held:?}")
            });

        // Fetching it opens the session that grant describes.
        let (status, body) = emulator.fetch(&url).await;
        assert_eq!(
            status, 200,
            "case {case:?}: fetching the published URL must open a session, got {status} {body:?}"
        );
        let session = parsed_session(case, &body);
        assert_eq!(
            str_field(&session, "ResourceArn"),
            prepared.arn,
            "case {case:?}: the session must name the resource it belongs to, got {session}"
        );
        let expires_at = grant
            .session_expires_at
            .duration_since(SystemTime::UNIX_EPOCH)
            .expect("a session expiry after the epoch")
            .as_secs_f64();
        let reported = session
            .get("SessionExpiresAt")
            .and_then(Value::as_f64)
            .unwrap_or_default();
        // The instant travels as a JSON number, so the comparison allows the rounding a
        // decimal rendering of an f64 introduces — a millisecond, where the windows under
        // test are half an hour and longer.
        assert!(
            (reported - expires_at).abs() < 0.001,
            "case {case:?}: the session must end when the grant says ({expires_at}), got \
             {reported}"
        );
    }
}

/// RULE: the URL authenticates only while the grant the mint recorded is live — a window
/// the clock has closed stops authenticating, whatever the resource's own status still
/// reports.
#[tokio::test]
async fn a_url_whose_window_has_elapsed_no_longer_authenticates() {
    for (case, subject, name, extra) in rows() {
        let emulator = Emulator::new();
        let prepared = prepare(&emulator, subject, &name).await;
        let url = published_url(&emulator, case, &prepared, &extra).await;

        // The control: while the grant is live the address authenticates.
        let (status, body) = emulator.fetch(&url).await;
        assert_eq!(
            status, 200,
            "case {case:?}: a fresh URL must authenticate, got {status} {body:?}"
        );

        // The clock, driven rather than waited for: every window the mint recorded is now
        // in the past. Half a day covers the widest window the model admits
        // (`SessionExpirationDurationInSeconds` 1800..43200).
        backdate_grants(&emulator, &prepared, Duration::from_secs(43_201));
        let (status, body) = emulator.fetch(&url).await;
        assert_eq!(
            status, 403,
            "case {case:?}: a URL past its window must not authenticate, got {status} {body:?}"
        );
    }
}

/// RULE: only the grant a mint recorded authenticates — a token nothing minted, a
/// tampered one, a missing one and another resource's own are each refused, and the
/// address of a resource this account does not hold authenticates nothing.
#[tokio::test]
async fn only_a_recorded_grant_authenticates_a_session() {
    for (case, subject, name, extra) in rows() {
        let emulator = Emulator::new();
        let prepared = prepare(&emulator, subject, &name).await;
        let url = published_url(&emulator, case, &prepared, &extra).await;
        let prefix = expected_prefix(subject.kind(), &prepared.resource);
        // The token as the URL carries it (percent-encoded), without any further query
        // field the operation's own URL appends.
        let encoded = url
            .strip_prefix(&prefix)
            .unwrap_or_default()
            .split('&')
            .next()
            .unwrap_or_default()
            .to_string();

        // Rows differ along HOW the presented token fails: nothing minted it, it was
        // tampered with, it is absent, and it belongs to another resource of the same
        // kind. The last row addresses a resource this account holds nothing under.
        let other = prepare(&emulator, subject, "othersession").await;
        let other_url = published_url(&emulator, case, &other, &extra).await;
        let other_token = other_url
            .strip_prefix(&expected_prefix(subject.kind(), &other.resource))
            .unwrap_or_default()
            .split('&')
            .next()
            .unwrap_or_default()
            .to_string();
        let attempts: Vec<(&str, String)> = vec![
            ("a token nothing minted", format!("{prefix}not-a-grant")),
            (
                "a tampered token",
                format!("{prefix}{}", tampered(&encoded)),
            ),
            ("no token at all", prefix.trim_end_matches("?authToken=").to_string()),
            (
                "another resource's token",
                format!("{prefix}{other_token}"),
            ),
            (
                "a resource this account does not hold",
                format!(
                    "{}x",
                    expected_prefix(subject.kind(), &format!("{}-absent", prepared.resource))
                ),
            ),
        ];
        for (attempt, address) in attempts {
            let (status, body) = emulator.fetch(&address).await;
            assert_eq!(
                status, 403,
                "case {case:?}/{attempt}: expected HTTP 403 for {address:?}, got {status} \
                 {body:?}"
            );
        }

        // The grant itself still authenticates: a refused attempt consumed nothing.
        let (status, body) = emulator.fetch(&url).await;
        assert_eq!(
            status, 200,
            "case {case:?}: the recorded grant must still authenticate, got {status} {body:?}"
        );
    }
}

/// RULE: opening a session is a READ — the resource reads back exactly as it did before,
/// and the same URL opens the same session again.
#[tokio::test]
async fn opening_a_session_changes_nothing_a_later_read_reports() {
    // Rows: one resource of each subject kind, read back through its own describe.
    let rows = [
        (
            Subject::NotebookInstance,
            "stable-nb",
            "DescribeNotebookInstance",
        ),
        (Subject::PartnerApp, "StableApp", "DescribePartnerApp"),
        (Subject::MlflowApp, "stable-mlflow", "DescribeMlflowApp"),
    ];
    for (subject, name, reader) in rows {
        let emulator = Emulator::new();
        let prepared = prepare(&emulator, subject, name).await;
        let url = published_url(&emulator, name, &prepared, &json!({})).await;
        let read_body = match subject {
            Subject::NotebookInstance => json!({ "NotebookInstanceName": prepared.identifier }),
            _ => json!({ "Arn": prepared.identifier }),
        };
        let before = emulator.ok(reader, &read_body).await;

        for attempt in ["first fetch", "second fetch"] {
            let (status, body) = emulator.fetch(&url).await;
            assert_eq!(
                status, 200,
                "case {name}/{attempt}: the session must open, got {status} {body:?}"
            );
            let after = emulator.ok(reader, &read_body).await;
            assert_eq!(
                after, before,
                "case {name}/{attempt}: opening a session must leave what a later reader \
                 sees untouched"
            );
            assert!(
                !after.to_string().contains(&token_of(&url)),
                "case {name}/{attempt}: the grant token must be reported by no read, got \
                 {after}"
            );
        }
    }
}

/// RULE: the session is served by the resource, so one that no longer serves it — a
/// notebook instance stopped after its URL was minted — authenticates nothing, even
/// though the grant's own window is still open.
#[tokio::test]
async fn a_resource_that_no_longer_serves_a_session_authenticates_nothing() {
    let emulator = Emulator::new();
    let prepared = prepare(&emulator, Subject::NotebookInstance, "stopped-nb").await;
    let url = published_url(&emulator, "stopped-nb", &prepared, &json!({})).await;
    let (status, body) = emulator.fetch(&url).await;
    assert_eq!(
        status, 200,
        "case stopped-nb: a serving instance must authenticate, got {status} {body:?}"
    );

    let (status, stopped) = emulator
        .call_in(
            REGION,
            "StopNotebookInstance",
            &json!({ "NotebookInstanceName": prepared.identifier }),
        )
        .await;
    assert_eq!(
        status, 200,
        "case stopped-nb: StopNotebookInstance must succeed, got {status} {stopped}"
    );
    let (status, body) = emulator.fetch(&url).await;
    assert_eq!(
        status, 403,
        "case stopped-nb: an instance serving no Jupyter server authenticates nothing, got \
         {status} {body:?}"
    );
}

/// `token` with one character changed, so the presented value is a token of the same shape
/// that no grant holds.
fn tampered(token: &str) -> String {
    let mut tampered: Vec<char> = token.chars().collect();
    if let Some(first) = tampered.first_mut() {
        *first = if *first == 'A' { 'B' } else { 'A' };
    }
    tampered.into_iter().collect()
}
