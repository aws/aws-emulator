// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for `ListAgentRuntimeVersions`.
//!
//! THE RULE UNDER TEST: a listing of the versions of one agent runtime reports
//! one entry per version that runtime has PERSISTED — the identity members the
//! runtime itself carries, and the version number, description, instant and
//! clock-driven status of that one version — in ascending version order, paged
//! by the shared page rule (`maxResults`, resumed strictly after the opaque
//! `nextToken` the previous page ended on); a version is addressed only through
//! its runtime, so an identifier naming no runtime of this account is the
//! modelled 404 rather than an empty page.
//!
//! The tables span what the rule generalises over: how many versions have been
//! published, whether a version carries a description, which versions remain
//! after one is deleted, the page size asked for and the pages walked, and the
//! state of the runtime that holds them.
//!
//! Every test drives the service the way the router does: an `http::Request`
//! through [`MultiServiceRouter::dispatch`], so the wire protocol (`restJson1`),
//! the boundary constraint validation and the handler are all exercised.

use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_bedrockagentcorecontrol_amazonbedrockagentcorecontrol::EmulatedService;

/// The region every case signs for.
const REGION: &str = "us-east-1";

/// A `roleArn` inside `RoleArn`'s pattern, which every create and update carries.
const ROLE_ARN: &str = "arn:aws:iam::000000000000:role/VersionListingProbe";

/// An `agentRuntimeId` inside `AgentRuntimeId`'s pattern that no case creates.
const ABSENT_RUNTIME_ID: &str = "noSuchRuntime-abcdefghij";

/// The statuses a version reports: the settled one every version this emulator
/// drives reaches, and what a version of a runtime on its way out reports.
/// `AgentRuntimeStatus`'s full value list is `CREATING | CREATE_FAILED |
/// UPDATING | UPDATE_FAILED | READY | DELETING | DELETE_FAILED`
/// (`smithy_query shape=AgentRuntimeStatus`).
const DELETING: &str = "DELETING";
/// See [`DELETING`]: the status every published version settles on.
const READY: &str = "READY";

/// How long a poll loop waits for a clock-owed transition before giving up, and
/// how long it waits between two looks.
///
/// The deadline is generous: the loop waits for the CONDITION it is about, so it
/// finishes as soon as the transition is due, and the deadline only catches one
/// that never comes.
const SETTLE_DEADLINE: Duration = Duration::from_secs(60);
const POLL_INTERVAL: Duration = Duration::from_millis(25);

fn router() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::default();
    router.register_service(EmulatedService::new(engine));
    router
}

/// A SigV4 `Authorization` header for this service: what the router routes on and
/// what the handlers read the ARN region from.
fn authorization() -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20240101/{REGION}/bedrock-agentcore/\
         aws4_request, SignedHeaders=host, Signature=abc"
    )
}

async fn send(
    router: &MultiServiceRouter,
    method: &str,
    uri: &str,
    body: Option<&Value>,
) -> (u16, Value) {
    let payload = body.map_or_else(Bytes::new, |value| {
        Bytes::from(serde_json::to_vec(value).expect("the request body serializes"))
    });
    let mut request = http::Request::builder()
        .method(method)
        .uri(uri)
        .header("authorization", authorization())
        .header("host", "bedrock-agentcore-control.amazonaws.com");
    if body.is_some() {
        request = request.header("content-type", "application/json");
    }
    let request = request.body(payload).expect("a well-formed request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let parsed = if response.body().is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(response.body()).unwrap_or(Value::Null)
    };
    (status, parsed)
}

/// The error code `restJson1` carries in `__type`.
fn error_code(body: &Value) -> String {
    body.get("__type")
        .and_then(Value::as_str)
        .unwrap_or("<no __type>")
        .rsplit('#')
        .next()
        .unwrap_or_default()
        .to_string()
}

/// Create a runtime named `name`, optionally with `description`, and answer the
/// `agentRuntimeId` the service minted.
async fn create(router: &MultiServiceRouter, name: &str, description: Option<&str>) -> String {
    let mut body = json!({
        "agentRuntimeName": name,
        "agentRuntimeArtifact": {"containerConfiguration": {
            "containerUri": "public.ecr.aws/agentcore/probe:v1"
        }},
        "networkConfiguration": {"networkMode": "PUBLIC"},
        "roleArn": ROLE_ARN,
    });
    if let Some(description) = description {
        body["description"] = json!(description);
    }
    let (status, parsed) = send(router, "PUT", "/runtimes/", Some(&body)).await;
    assert_eq!(
        status, 202,
        "case {name:?}: the setup create expected 202, got {status} ({parsed})"
    );
    parsed["agentRuntimeId"]
        .as_str()
        .expect("a create answers the id it minted")
        .to_string()
}

/// Publish another version of `id`, optionally with `description`, and answer the
/// version number the service published.
async fn update(router: &MultiServiceRouter, id: &str, description: Option<&str>) -> String {
    let mut body = json!({
        "agentRuntimeArtifact": {"containerConfiguration": {
            "containerUri": "public.ecr.aws/agentcore/probe:v2"
        }},
        "networkConfiguration": {"networkMode": "PUBLIC"},
        "roleArn": ROLE_ARN,
    });
    if let Some(description) = description {
        body["description"] = json!(description);
    }
    let (status, parsed) =
        send(router, "PUT", &format!("/runtimes/{id}/"), Some(&body)).await;
    assert_eq!(
        status, 202,
        "case {id:?}: the setup update expected 202, got {status} ({parsed})"
    );
    parsed["agentRuntimeVersion"]
        .as_str()
        .expect("an update answers the version it published")
        .to_string()
}

/// One `ListAgentRuntimeVersions` call, with `query` appended to the route.
async fn list(router: &MultiServiceRouter, id: &str, query: &str) -> (u16, Value) {
    send(
        router,
        "POST",
        &format!("/runtimes/{id}/versions/{query}"),
        None,
    )
    .await
}

/// The entries of a listing that answered 200, as the response carries them.
async fn entries(router: &MultiServiceRouter, id: &str, query: &str) -> Vec<Value> {
    let (status, body) = list(router, id, query).await;
    assert_eq!(
        status, 200,
        "case {id:?} {query:?}: expected 200, got {status} ({body})"
    );
    body["agentRuntimes"]
        .as_array()
        .unwrap_or_else(|| panic!("case {id:?} {query:?}: expected agentRuntimes, got {body}"))
        .clone()
}

/// A member of one listed entry, as a string.
fn member(entry: &Value, name: &str) -> String {
    entry[name]
        .as_str()
        .unwrap_or_else(|| panic!("expected {name} on {entry}"))
        .to_string()
}

/// The version numbers a listing reported, in the order it reported them.
fn versions_of(entries: &[Value]) -> Vec<String> {
    entries
        .iter()
        .map(|entry| member(entry, "agentRuntimeVersion"))
        .collect()
}

/// List the versions of `id` until every one of them reports [`READY`], or fail
/// once the generous deadline passes.
///
/// A freshly published version reports its transient status until the transition
/// the clock owes it comes due, so this looks AGAIN for the condition rather than
/// waiting a fixed time: nothing here depends on how fast the host runs.
async fn entries_once_settled(router: &MultiServiceRouter, id: &str) -> Vec<Value> {
    let deadline = std::time::Instant::now() + SETTLE_DEADLINE;
    loop {
        let listed = entries(router, id, "").await;
        if listed
            .iter()
            .all(|entry| member(entry, "status") == READY)
        {
            return listed;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "case {id:?}: the published versions never settled, last answer {listed:?}"
        );
        tokio::time::sleep(POLL_INTERVAL).await;
    }
}

/// RULE: the listing reports one entry per version the runtime has published, in
/// ascending version order, each carrying the runtime's own identity and that
/// one version's number and description.
#[tokio::test]
async fn one_entry_per_published_version_in_ascending_order() {
    let router = router();

    // (case, the description of each version in publish order)
    let rows: Vec<(&str, Vec<Option<&str>>)> = vec![
        ("a runtime that was only created", vec![Some("the first")]),
        (
            "a runtime updated once",
            vec![Some("the first"), Some("the second")],
        ),
        (
            "a version published without a description",
            vec![Some("the first"), None],
        ),
        (
            "a runtime updated four times",
            vec![
                None,
                Some("the second"),
                Some("the third"),
                None,
                Some("the fifth"),
            ],
        ),
    ];

    for (index, (case, descriptions)) in rows.into_iter().enumerate() {
        let name = format!("versionRows{index}");
        let id = create(&router, &name, descriptions[0]).await;
        for description in &descriptions[1..] {
            update(&router, &id, *description).await;
        }

        // Every version the clock has settled, so the status each entry reports is
        // pinned rather than "whatever a live version may be".
        let listed = entries_once_settled(&router, &id).await;
        let expected_versions: Vec<String> = (1..=descriptions.len())
            .map(|version| version.to_string())
            .collect();
        assert_eq!(
            versions_of(&listed),
            expected_versions,
            "case {case:?}: expected {expected_versions:?}, got {:?}",
            versions_of(&listed)
        );
        for (position, description) in descriptions.iter().enumerate() {
            let entry = &listed[position];
            // `description` is `@required` on the entry, so a version published
            // without one reports the empty string rather than omitting it.
            let expected = description.unwrap_or_default();
            assert_eq!(
                member(entry, "description"),
                expected,
                "case {case:?} version {}: expected description {expected:?}, got {:?}",
                position + 1,
                member(entry, "description")
            );
            assert_eq!(
                member(entry, "agentRuntimeId"),
                id,
                "case {case:?} version {}: expected the runtime's id {id:?}, got {:?}",
                position + 1,
                member(entry, "agentRuntimeId")
            );
            assert_eq!(
                member(entry, "agentRuntimeName"),
                name,
                "case {case:?} version {}: expected the runtime's name {name:?}, got {:?}",
                position + 1,
                member(entry, "agentRuntimeName")
            );
            let expected_arn = format!(
                "arn:aws:bedrock-agentcore:{REGION}:{}:runtime/{id}",
                engine::emulator_account_id()
            );
            assert_eq!(
                member(entry, "agentRuntimeArn"),
                expected_arn,
                "case {case:?} version {}: expected the runtime's ARN {expected_arn:?}, got {:?}",
                position + 1,
                member(entry, "agentRuntimeArn")
            );
            let status = member(entry, "status");
            assert_eq!(
                status, READY,
                "case {case:?} version {}: expected {READY}, got {status:?}",
                position + 1
            );
            assert!(
                !entry["lastUpdatedAt"].is_null(),
                "case {case:?} version {}: expected a lastUpdatedAt timestamp, got {}",
                position + 1,
                entry["lastUpdatedAt"]
            );
        }
    }
}

/// RULE: the page holds at most `maxResults` entries and the rest is resumed
/// strictly after the opaque token the page ended on, so walking every page of
/// any page size yields each published version exactly once, in ascending order.
#[tokio::test]
async fn every_page_size_walks_each_version_exactly_once() {
    let router = router();
    let id = create(&router, "versionPaging", Some("the first")).await;
    for version in 2..=6 {
        update(&router, &id, Some(&format!("version {version}"))).await;
    }
    let all: Vec<String> = (1..=6).map(|version| version.to_string()).collect();

    // The page sizes a caller can ask for, inside `maxResults`'s modelled range
    // 1..100: under, at and over the number of versions held, and the modelled
    // ceiling itself.
    for page_size in [1_usize, 2, 5, 6, 7, 100] {
        let mut walked: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        let mut pages = 0;
        loop {
            let query = match &token {
                None => format!("?maxResults={page_size}"),
                Some(token) => format!("?maxResults={page_size}&nextToken={token}"),
            };
            let (status, body) = list(&router, &id, &query).await;
            assert_eq!(
                status, 200,
                "case page size {page_size}, page {pages}: expected 200, got {status} ({body})"
            );
            let page = body["agentRuntimes"]
                .as_array()
                .unwrap_or_else(|| {
                    panic!("case page size {page_size}: expected agentRuntimes, got {body}")
                })
                .clone();
            assert!(
                page.len() <= page_size,
                "case page size {page_size}, page {pages}: expected at most {page_size} \
                 entries, got {}",
                page.len()
            );
            walked.extend(versions_of(&page));
            pages += 1;
            assert!(
                pages <= all.len() + 1,
                "case page size {page_size}: the walk did not end, got {walked:?}"
            );
            match body["nextToken"].as_str() {
                None => break,
                Some(next) => token = Some(next.to_string()),
            }
        }
        assert_eq!(
            walked, all,
            "case page size {page_size}: expected {all:?} across the walk, got {walked:?}"
        );
    }
}

/// RULE: the entries are the versions the runtime PERSISTS, so a version that
/// was deleted is not reported and the ones that remain keep their own numbers —
/// the listing is never a range of numbers nothing was stored for.
#[tokio::test]
async fn a_deleted_version_is_not_reported() {
    let router = router();
    let id = create(&router, "versionDeletion", Some("the first")).await;
    for version in 2..=4 {
        update(&router, &id, Some(&format!("version {version}"))).await;
    }

    // (case, the version deleted, what the listing reports afterwards)
    let rows: [(&str, &str, &[&str]); 3] = [
        ("a version in the middle", "2", &["1", "3", "4"]),
        ("the oldest version", "1", &["3", "4"]),
        ("the newest version", "4", &["3"]),
    ];
    for (case, version, remaining) in rows {
        let (status, body) =
            send(&router, "DELETE", &format!("/runtimes/{id}/?version={version}"), None).await;
        assert_eq!(
            status, 202,
            "case {case:?}: the setup delete expected 202, got {status} ({body})"
        );
        let listed = versions_of(&entries(&router, &id, "").await);
        assert_eq!(
            listed, remaining,
            "case {case:?}: expected {remaining:?}, got {listed:?}"
        );
    }
}

/// RULE: a version is reported exactly as a read of that version reports it, so
/// the listing and `GetAgentRuntime?version=` cannot disagree about one version's
/// number, description or status.
#[tokio::test]
async fn each_entry_agrees_with_the_read_of_that_version() {
    let router = router();
    let id = create(&router, "versionRoundTrip", Some("the first")).await;
    update(&router, &id, Some("the second")).await;
    update(&router, &id, None).await;

    let listed = entries_once_settled(&router, &id).await;
    assert_eq!(
        versions_of(&listed),
        vec!["1", "2", "3"],
        "case \"the settled listing\": expected every published version, got {:?}",
        versions_of(&listed)
    );
    for entry in &listed {
        let version = member(entry, "agentRuntimeVersion");
        let (status, read) =
            send(&router, "GET", &format!("/runtimes/{id}/?version={version}"), None).await;
        assert_eq!(
            status, 200,
            "case \"the read of version {version}\": expected 200, got {status} ({read})"
        );
        for name in ["agentRuntimeArn", "agentRuntimeId", "agentRuntimeName", "status"] {
            assert_eq!(
                member(entry, name),
                member(&read, name),
                "case \"version {version}\": expected the read's {name} {:?}, got {:?}",
                member(&read, name),
                member(entry, name)
            );
        }
        // The read omits a description the version never carried; the entry
        // reports it as the empty string, the one difference the shapes impose.
        let read_description = read["description"].as_str().unwrap_or_default();
        assert_eq!(
            member(entry, "description"),
            read_description,
            "case \"version {version}\": expected the read's description \
             {read_description:?}, got {:?}",
            member(entry, "description")
        );
        assert_eq!(
            entry["lastUpdatedAt"], read["lastUpdatedAt"],
            "case \"version {version}\": expected the read's lastUpdatedAt {}, got {}",
            read["lastUpdatedAt"], entry["lastUpdatedAt"]
        );
    }
}

/// RULE: the status an entry carries is the runtime's at that version, so once
/// the runtime's own deletion has been accepted every version it still holds is
/// reported as going away.
#[tokio::test]
async fn a_runtime_being_deleted_reports_every_version_as_deleting() {
    let router = router();
    let id = create(&router, "versionDeletingRuntime", Some("the first")).await;
    update(&router, &id, Some("the second")).await;

    let (status, body) = send(&router, "DELETE", &format!("/runtimes/{id}/"), None).await;
    assert_eq!(
        status, 202,
        "case \"the setup delete of the runtime\": expected 202, got {status} ({body})"
    );

    let listed = entries(&router, &id, "").await;
    assert_eq!(
        versions_of(&listed),
        vec!["1", "2"],
        "case \"a runtime whose deletion is under way\": expected both versions, got {:?}",
        versions_of(&listed)
    );
    for entry in &listed {
        let version = member(entry, "agentRuntimeVersion");
        assert_eq!(
            member(entry, "status"),
            DELETING,
            "case \"version {version} of a runtime being deleted\": expected {DELETING}, got {:?}",
            member(entry, "status")
        );
    }
}

/// RULE: a version is addressed only through its runtime, so an identifier that
/// names no runtime of this account is the modelled 404 — not an empty page —
/// and a `nextToken` this service did not issue is the modelled 400.
#[tokio::test]
async fn the_listing_refuses_an_unheld_runtime_and_a_foreign_token() {
    let router = router();
    let id = create(&router, "versionRefusals", Some("the first")).await;

    let (status, body) = list(&router, ABSENT_RUNTIME_ID, "").await;
    let case = "an addressable runtime the account does not hold";
    assert_eq!(
        (status, error_code(&body).as_str()),
        (404, "ResourceNotFoundException"),
        "case {case:?}: expected 404 ResourceNotFoundException, got {status} ({body})"
    );
    let expected = format!(
        "Agent with agentId: {ABSENT_RUNTIME_ID}, accountId: {} not found!",
        engine::emulator_account_id()
    );
    assert_eq!(
        body["message"].as_str().unwrap_or_default(),
        expected,
        "case {case:?}: expected {expected:?}, got {:?}",
        body["message"]
    );

    // (case, the token the request carries)
    let tokens: [(&str, &str); 3] = [
        ("a token that is not hex at all", "notAToken"),
        ("a token of another service's form", "eyJuZXh0IjoiMiJ9"),
        ("a token with an odd number of hex digits", "abc"),
    ];
    for (case, token) in tokens {
        let (status, body) = list(&router, &id, &format!("?nextToken={token}")).await;
        assert_eq!(
            (status, error_code(&body).as_str()),
            (400, "ValidationException"),
            "case {case:?}: expected 400 ValidationException, got {status} ({body})"
        );
        assert!(
            body["message"]
                .as_str()
                .unwrap_or_default()
                .contains("nextToken")
                || body
                    .pointer("/fieldList/0/name")
                    .and_then(Value::as_str)
                    .is_some_and(|name| name == "nextToken"),
            "case {case:?}: expected the refusal to name nextToken, got {body}"
        );
    }
}
