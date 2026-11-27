// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for the version clause of the agent runtime family's
//! not-found sentence.
//!
//! THE RULE UNDER TEST: a refusal reports only values the asking operation's
//! input can carry. A runtime lookup that resolves nothing always names the
//! agent id taken from the request path and the account the lookup ran in, and
//! it names a version ONLY on an operation whose input declares a version
//! member — the version that request supplied, or the unspecified marker when
//! such a request left the member out. An operation that declares no version
//! member has no value for a version clause, so its sentence ends after the
//! account.
//!
//! Which operations those are comes from the model
//! (`smithy_query(query="operation",operation="ListAgentRuntimeVersions,
//! GetAgentRuntime,DeleteAgentRuntime")`): `GetAgentRuntime` and
//! `DeleteAgentRuntime` declare `agentRuntimeVersion` (httpQuery `version`,
//! pattern `^([1-9][0-9]{0,4})$`), while `ListAgentRuntimeVersions` — which
//! enumerates every version of one runtime — declares only `agentRuntimeId`,
//! `maxResults` and `nextToken`.
//!
//! The tables span what the rule generalises over: the form and length of the
//! `agentRuntimeId` the request path carries, which operation asked, whether a
//! version was supplied and which one, and whether the runtime itself or only
//! the version it was asked for is missing.
//!
//! Every test drives the service the way the router does: an `http::Request`
//! through [`MultiServiceRouter::dispatch`], so the wire protocol (`restJson1`),
//! the boundary constraint validation and the handler are all exercised.

use std::sync::Arc;

use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_bedrockagentcorecontrol_amazonbedrockagentcorecontrol::EmulatedService;

/// The region every case signs for.
const REGION: &str = "us-east-1";

/// A `roleArn` inside `RoleArn`'s pattern, which every setup create carries.
const ROLE_ARN: &str = "arn:aws:iam::000000000000:role/VersionClauseProbe";

/// How a version-selecting operation reports a request that named no version.
const UNSPECIFIED: &str = "null";

fn router() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::default();
    router.register_service(EmulatedService::new(engine));
    router
}

/// A SigV4 `Authorization` header for this service: what the router routes on
/// and what the handlers read the ARN region from.
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

fn message(body: &Value) -> String {
    body.get("message")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

/// The sentence for a lookup that named no version at all, as an operation with
/// no version member words it.
fn version_free(id: &str) -> String {
    format!(
        "Agent with agentId: {id}, accountId: {} not found!",
        engine::emulator_account_id()
    )
}

/// The sentence for a lookup an operation that SELECTS a version ran: the same
/// text with the version clause appended.
fn version_qualified(id: &str, version: &str) -> String {
    format!(
        "Agent with agentId: {id}, accountId: {}, and version: {version} not found!",
        engine::emulator_account_id()
    )
}

/// `agentRuntimeId`s inside `AgentRuntimeId`'s pattern
/// (`^[a-zA-Z][a-zA-Z0-9_]{0,99}-[a-zA-Z0-9]{10}$`) that no case creates, spanning
/// the forms the label admits: the shortest prefix the pattern allows, a mixed
/// prefix with underscores and digits, and the longest prefix it allows.
fn absent_ids() -> Vec<String> {
    vec![
        "a-0123456789".to_string(),
        "Agent_probe_2-kR7vPq2wXn".to_string(),
        format!("l{}-ZZ99zz00aA", "o".repeat(99)),
    ]
}

/// Create a runtime named `name` and answer the `agentRuntimeId` the service
/// minted, so a case can address a runtime that really exists.
async fn create(router: &MultiServiceRouter, name: &str) -> String {
    let body = json!({
        "agentRuntimeName": name,
        "agentRuntimeArtifact": {"containerConfiguration": {
            "containerUri": "public.ecr.aws/agentcore/probe:v1"
        }},
        "networkConfiguration": {"networkMode": "PUBLIC"},
        "roleArn": ROLE_ARN,
    });
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

/// RULE: `ListAgentRuntimeVersions` selects no version, so its 404 for a runtime
/// this account does not hold ends after the account id — whatever form the id
/// takes, and whatever paging members the query carries.
#[tokio::test]
async fn the_version_listing_refuses_an_unheld_runtime_without_a_version_clause() {
    let router = router();

    // (case, the query the request carries beyond the path)
    let queries: [(&str, &str); 4] = [
        ("no query at all", ""),
        ("a page size asked for", "?maxResults=1"),
        ("the largest modelled page size", "?maxResults=100"),
        // A `version` query is not a member of this operation, so the wire drops
        // it: it can no more reach the sentence than paging can.
        ("a version query the operation does not model", "?version=3"),
    ];

    for id in absent_ids() {
        for (query, suffix) in queries {
            let case = format!("ListAgentRuntimeVersions on {id:?} with {query}");
            let (status, body) =
                send(&router, "POST", &format!("/runtimes/{id}/versions/{suffix}"), None).await;
            assert_eq!(
                (status, error_code(&body).as_str()),
                (404, "ResourceNotFoundException"),
                "case {case:?}: expected 404 ResourceNotFoundException, got {status} ({body})"
            );
            let expected = version_free(&id);
            assert_eq!(
                message(&body),
                expected,
                "case {case:?}: expected {expected:?}, got {:?}",
                message(&body)
            );
            assert!(
                !message(&body).contains("version"),
                "case {case:?}: expected no version clause at all, got {:?}",
                message(&body)
            );
        }
    }
}

/// RULE: an operation whose input DECLARES a version member keeps the version
/// clause — filled with the version the request supplied, and with the
/// unspecified marker when that request named none.
#[tokio::test]
async fn a_version_selecting_lookup_keeps_the_version_clause() {
    let router = router();

    // (case, method, the query beyond the path, the version the clause reports)
    let rows: [(&str, &str, &str, &str); 8] = [
        ("GetAgentRuntime, no version named", "GET", "", UNSPECIFIED),
        ("GetAgentRuntime, the first version", "GET", "?version=1", "1"),
        ("GetAgentRuntime, a far later version", "GET", "?version=412", "412"),
        // `AgentRuntimeVersion` is `^([1-9][0-9]{0,4})$`: five digits is the most
        // the wire carries.
        (
            "GetAgentRuntime, the highest modelled version",
            "GET",
            "?version=99999",
            "99999",
        ),
        (
            "DeleteAgentRuntime, no version named",
            "DELETE",
            "",
            UNSPECIFIED,
        ),
        (
            "DeleteAgentRuntime, the first version",
            "DELETE",
            "?version=1",
            "1",
        ),
        (
            "DeleteAgentRuntime, a far later version",
            "DELETE",
            "?version=412",
            "412",
        ),
        (
            "DeleteAgentRuntime, the highest modelled version",
            "DELETE",
            "?version=99999",
            "99999",
        ),
    ];

    for id in absent_ids() {
        for (row, method, query, version) in rows {
            let case = format!("{row} on {id:?}");
            let (status, body) =
                send(&router, method, &format!("/runtimes/{id}/{query}"), None).await;
            assert_eq!(
                (status, error_code(&body).as_str()),
                (404, "ResourceNotFoundException"),
                "case {case:?}: expected 404 ResourceNotFoundException, got {status} ({body})"
            );
            let expected = version_qualified(&id, version);
            assert_eq!(
                message(&body),
                expected,
                "case {case:?}: expected {expected:?}, got {:?}",
                message(&body)
            );
        }
    }
}

/// RULE: the two sentences differ only in the clause, and only by which
/// operation asked — one runtime that EXISTS answers the version listing, while
/// a version it never published is still the version-qualified refusal from the
/// operation that selects one.
#[tokio::test]
async fn one_runtime_answers_both_sentences_by_who_asked() {
    let router = router();
    let id = create(&router, "versionClauseProbe").await;

    // The listing of a runtime that IS held is served, so the refusal above is
    // about the runtime's existence rather than about the operation.
    let (status, body) = send(&router, "POST", &format!("/runtimes/{id}/versions/"), None).await;
    let case = "ListAgentRuntimeVersions on a runtime this account holds";
    assert_eq!(
        status, 200,
        "case {case:?}: expected 200, got {status} ({body})"
    );
    assert_eq!(
        body["agentRuntimes"].as_array().map(Vec::len),
        Some(1),
        "case {case:?}: expected the one published version, got {body}"
    );

    // A version that runtime never published: the asking operation declares a
    // version member and the request filled it, so the clause reports it.
    for version in ["2", "77", "99999"] {
        let case = format!("GetAgentRuntime on a held runtime at unpublished version {version}");
        let (status, body) =
            send(&router, "GET", &format!("/runtimes/{id}/?version={version}"), None).await;
        assert_eq!(
            (status, error_code(&body).as_str()),
            (404, "ResourceNotFoundException"),
            "case {case:?}: expected 404 ResourceNotFoundException, got {status} ({body})"
        );
        let expected = version_qualified(&id, version);
        assert_eq!(
            message(&body),
            expected,
            "case {case:?}: expected {expected:?}, got {:?}",
            message(&body)
        );
    }
}
