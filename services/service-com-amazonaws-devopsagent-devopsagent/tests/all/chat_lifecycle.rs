// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for the chat plane of one AgentSpace: `CreateChat`
//! (POST `/agents/agent-space/{agentSpaceId}/chat/create`), `GetChat` (GET
//! `.../chat/get/{executionId}`), `UpdateChat` (PATCH `.../chat/{executionId}`)
//! and `ListChats` (GET `.../chat/list`).
//!
//! Every case drives the service through the same router the emulator serves, so
//! the routes, status codes, protocol envelope and state are all exercised.
//!
//! The rules under test:
//!
//! 1. A chat execution is created in the default chat list — `archiveStatus`
//!    `ACTIVE`, no summary yet, `updatedAt` at its creation instant — and a read
//!    reports exactly what was persisted.
//! 2. Every member `UpdateChat` carries is a PATCH: what the request names is set,
//!    what it leaves out stays as it was, and a request that names nothing changes
//!    nothing.
//! 3. Archiving hides a chat execution from the DEFAULT chat list and unarchiving
//!    restores it, while `archiveStatus` on the listing selects exactly the chats
//!    in the status it names.
//! 4. While a chat execution is hidden from the default list the only change it
//!    accepts is the one that restores it: any other update is refused with HTTP
//!    409 `ConflictException` carrying the `CHAT_ARCHIVED` reason, and applies
//!    nothing.
//! 5. A repeat carrying the `clientToken` of an update already applied is ignored
//!    rather than applied a second time, and is not an error.
//! 6. An operation addressed to a chat INSIDE an agent space authorises the caller
//!    against that space first, so an `agentSpaceId` that resolves to no space
//!    fails CLOSED with HTTP 403 `AccessDeniedException`; only inside a space the
//!    caller reached is an `executionId` that names nothing HTTP 404
//!    `ResourceNotFoundException`.
//! 7. The listing reports the newest chat execution first, a page at a time, and
//!    issues a `nextToken` only while chats follow the page it served.
//! 8. A read changes nothing a later caller sees.

use std::collections::BTreeSet;
use std::sync::Arc;

use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_devopsagent_devopsagent::state::{
    chat_archived_message, chat_not_found_message, ACCESS_DENIED_MESSAGE,
    CHAT_ARCHIVE_STATUS_ACTIVE, CHAT_ARCHIVE_STATUS_ARCHIVED, CONFLICT_REASON_CHAT_ARCHIVED,
    MAX_CHATS_PER_AGENT_SPACE,
};

/// A SigV4 credential scope naming this service's signing name (`aidevops`), which
/// is how the router resolves which service claims a request. Signature
/// verification is switched off for these tests, so the signature is a placeholder.
const AUTHORIZATION: &str = concat!(
    "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/us-west-2/aidevops/aws4_request, ",
    "SignedHeaders=host, Signature=0123456789abcdef"
);

/// The published bounds of `ListChatsRequest$maxResults` (`@range(min: 1, max:
/// 20)`), from `smithy_query(query="operation", operation="ListChats")`.
const MAX_RESULTS_MIN: i64 = 1;
const MAX_RESULTS_MAX: i64 = 20;

/// The published bounds of `UpdateChatRequest$summary` (`@length(min: 1, max:
/// 256)`), from `smithy_query(query="operation", operation="UpdateChat")`.
const SUMMARY_MIN_LEN: usize = 1;
const SUMMARY_MAX_LEN: usize = 256;

/// One emulator: a single shared engine behind the production router.
fn new_router() -> MultiServiceRouter {
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_devopsagent_devopsagent::EmulatedService::new(Arc::new(
            engine::InMemoryEngine::new(),
        )),
    );
    router
}

/// Send one request and return its status and parsed body (`Value::Null` for an
/// empty body).
async fn call(
    router: &MultiServiceRouter,
    method: &str,
    uri: &str,
    body: Option<Value>,
) -> (u16, Value) {
    let payload = body
        .as_ref()
        .map(|b| Bytes::from(serde_json::to_vec(b).expect("serialize request body")))
        .unwrap_or_default();
    let mut request = http::Request::builder()
        .method(method)
        .uri(uri)
        .header("host", "aidevops.us-west-2.amazonaws.com")
        .header("authorization", AUTHORIZATION);
    if body.is_some() {
        request = request.header("content-type", "application/json");
    }
    let request = request.body(payload).expect("build request");

    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let bytes = response.into_body();
    let parsed = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap_or_else(|e| {
            panic!(
                "response body is not JSON ({e}): {}",
                String::from_utf8_lossy(&bytes)
            )
        })
    };
    (status, parsed)
}

/// The wire error code (`__type`) of an error response.
fn error_code(body: &Value) -> String {
    body.get("__type")
        .and_then(Value::as_str)
        .unwrap_or("<no __type>")
        .rsplit(['#', ':'])
        .next()
        .unwrap_or_default()
        .to_string()
}

/// The epoch-seconds instant a reported member carries.
///
/// A chat timestamp renders as restJson1's default epoch seconds, so a case can
/// order two instants by reading them as numbers.
fn instant(case: &str, item: &Value, member: &str) -> f64 {
    item.get(member)
        .and_then(Value::as_f64)
        .unwrap_or_else(|| panic!("case {case:?}: no {member} instant in {item}"))
}

/// Create an agent space and return its id.
async fn create_space(router: &MultiServiceRouter, name: &str) -> String {
    let (status, body) = call(router, "POST", "/v1/agentspaces", Some(json!({"name": name}))).await;
    assert_eq!(status, 201, "case {name:?}: expected 201, got {status}");
    body["agentSpace"]["agentSpaceId"]
        .as_str()
        .unwrap_or_else(|| panic!("case {name:?}: no agentSpaceId in {body}"))
        .to_string()
}

/// `CreateChat` in `space`, returning the whole response.
async fn create_chat_raw(
    router: &MultiServiceRouter,
    space: &str,
    query: &str,
) -> (u16, Value) {
    let uri = format!("/agents/agent-space/{space}/chat/create{query}");
    call(router, "POST", &uri, None).await
}

/// Create a chat execution and return its minted `executionId` with the response.
async fn create_chat(router: &MultiServiceRouter, space: &str, case: &str) -> (String, Value) {
    let (status, body) = create_chat_raw(router, space, "").await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected 200 from CreateChat, got {status}: {body}"
    );
    let execution_id = body["executionId"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: no executionId in {body}"))
        .to_string();
    (execution_id, body)
}

/// `GetChat` for one chat execution.
async fn get_chat(router: &MultiServiceRouter, space: &str, execution: &str) -> (u16, Value) {
    let uri = format!("/agents/agent-space/{space}/chat/get/{execution}");
    call(router, "GET", &uri, None).await
}

/// `UpdateChat` for one chat execution, with the body the case supplies.
async fn update_chat(
    router: &MultiServiceRouter,
    space: &str,
    execution: &str,
    body: Value,
) -> (u16, Value) {
    let uri = format!("/agents/agent-space/{space}/chat/{execution}");
    call(router, "PATCH", &uri, Some(body)).await
}

/// `ListChats` for one agent space, with the query string the case supplies.
async fn list_chats(router: &MultiServiceRouter, space: &str, query: &str) -> (u16, Value) {
    let uri = format!("/agents/agent-space/{space}/chat/list{query}");
    call(router, "GET", &uri, None).await
}

/// The `executions` of a listing, as a vector.
fn executions(case: &str, body: &Value) -> Vec<Value> {
    body["executions"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: no executions list in {body}"))
        .clone()
}

/// The `executionId`s of a listing, in the order it reported them.
fn listed_ids(case: &str, body: &Value) -> Vec<String> {
    executions(case, body)
        .iter()
        .map(|execution| {
            execution["executionId"]
                .as_str()
                .unwrap_or_else(|| panic!("case {case:?}: no executionId in {execution}"))
                .to_string()
        })
        .collect()
}

/// Every chat execution of `space` the listing reports under `query`, collected by
/// walking its pages.
async fn walk_pages(
    router: &MultiServiceRouter,
    space: &str,
    max_results: Option<i64>,
    archive_status: Option<&str>,
    case: &str,
) -> Vec<String> {
    let mut ids: Vec<String> = Vec::new();
    let mut next: Option<String> = None;
    loop {
        let mut params: Vec<String> = Vec::new();
        if let Some(limit) = max_results {
            params.push(format!("maxResults={limit}"));
        }
        if let Some(status) = archive_status {
            params.push(format!("archiveStatus={status}"));
        }
        if let Some(token) = next.as_deref() {
            params.push(format!("nextToken={token}"));
        }
        let query = if params.is_empty() {
            String::new()
        } else {
            format!("?{}", params.join("&"))
        };
        let (status, body) = list_chats(router, space, &query).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200 from ListChats{query}, got {status}: {body}"
        );
        let page = listed_ids(case, &body);
        if let Some(limit) = max_results {
            assert!(
                page.len() <= limit as usize,
                "case {case:?}: page of {} entries exceeds maxResults {limit}",
                page.len()
            );
        }
        ids.extend(page);
        next = body["nextToken"].as_str().map(str::to_string);
        if next.is_none() {
            return ids;
        }
    }
}

/// RULE 1: a create persists the chat execution in the default chat list, and a
/// read reports exactly what was persisted — whatever the (ignored) identity
/// members the create carried.
#[tokio::test]
async fn created_chat_is_reported_unchanged_by_get() {
    let router = new_router();
    let space = create_space(&router, "chat-create").await;

    // The rows differ along what the rule generalises over: the identity members
    // the create may carry. `userId` is deprecated and ignored, and `userType` is
    // the authentication type of the caller's identity, so neither changes the
    // chat that is created. The `userType` values are the model's enum
    // (`smithy_query(query="shape", shape="UserType")`).
    let rows: [(&str, &str); 6] = [
        ("no identity members", ""),
        ("deprecated userId", "?userId=u-1"),
        ("userType IAM", "?userType=IAM"),
        ("userType IDC", "?userType=IDC"),
        ("userType IDP", "?userType=IDP"),
        ("both", "?userId=u.2_x-y&userType=IDC"),
    ];

    let mut minted: BTreeSet<String> = BTreeSet::new();
    for (case, query) in rows {
        let (status, created) = create_chat_raw(&router, &space, query).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200 from CreateChat, got {status}: {created}"
        );
        let execution_id = created["executionId"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: no executionId in {created}"))
            .to_string();
        assert!(
            minted.insert(execution_id.clone()),
            "case {case:?}: executionId {execution_id} was minted twice"
        );

        let (status, read) = get_chat(&router, &space, &execution_id).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200 from GetChat, got {status}: {read}"
        );
        assert_eq!(
            read["executionId"],
            json!(execution_id),
            "case {case:?}: GetChat reports {:?}, not the created {execution_id}",
            read["executionId"]
        );
        assert_eq!(
            read["createdAt"], created["createdAt"],
            "case {case:?}: GetChat reports createdAt {:?}, not the created {:?}",
            read["createdAt"], created["createdAt"]
        );
        assert_eq!(
            read["updatedAt"], created["createdAt"],
            "case {case:?}: a chat nothing has updated reports updatedAt {:?}, not its creation instant {:?}",
            read["updatedAt"], created["createdAt"]
        );
        assert_eq!(
            read["archiveStatus"],
            json!(CHAT_ARCHIVE_STATUS_ACTIVE),
            "case {case:?}: a created chat reports archiveStatus {:?}, not {CHAT_ARCHIVE_STATUS_ACTIVE}",
            read["archiveStatus"]
        );
        assert!(
            read.get("summary").is_none_or(Value::is_null),
            "case {case:?}: a created chat carries a summary: {read}"
        );
    }
}

/// RULE 2: every member an update names is set and every member it leaves out
/// stays as it was; an update that names nothing changes nothing at all.
#[tokio::test]
async fn update_chat_patches_only_the_members_it_names() {
    let router = new_router();
    let space = create_space(&router, "chat-update").await;

    // The rows differ along what the rule generalises over: which members the
    // patch carries, each value of the `ChatArchiveStatus` enum
    // (`smithy_query(query="shape", shape="ChatArchiveStatus")`), and both
    // published bounds of `summary`'s length.
    let shortest = "s".repeat(SUMMARY_MIN_LEN);
    let longest = "l".repeat(SUMMARY_MAX_LEN);
    let rows: [(&str, Option<&str>, Option<&str>); 7] = [
        ("neither member", None, None),
        ("summary only", Some("release triage"), None),
        ("shortest summary", Some(shortest.as_str()), None),
        ("longest summary", Some(longest.as_str()), None),
        ("archive only", None, Some(CHAT_ARCHIVE_STATUS_ARCHIVED)),
        ("status it already has", None, Some(CHAT_ARCHIVE_STATUS_ACTIVE)),
        (
            "both members",
            Some("oncall handover"),
            Some(CHAT_ARCHIVE_STATUS_ARCHIVED),
        ),
    ];

    for (case, summary, archive_status) in rows {
        let (execution_id, created) = create_chat(&router, &space, case).await;
        let mut body = json!({});
        if let Some(summary) = summary {
            body["summary"] = json!(summary);
        }
        if let Some(archive_status) = archive_status {
            body["archiveStatus"] = json!(archive_status);
        }
        let (status, updated) = update_chat(&router, &space, &execution_id, body).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200 from UpdateChat, got {status}: {updated}"
        );

        // What the update answers is the chat as it now stands: the members it set,
        // and the stored values of the ones it did not.
        let expected_summary = summary.map_or(Value::Null, |summary| json!(summary));
        let expected_status = json!(archive_status.unwrap_or(CHAT_ARCHIVE_STATUS_ACTIVE));
        assert_eq!(
            updated["summary"], expected_summary,
            "case {case:?}: UpdateChat reports summary {:?}, not {expected_summary:?}",
            updated["summary"]
        );
        assert_eq!(
            updated["archiveStatus"], expected_status,
            "case {case:?}: UpdateChat reports archiveStatus {:?}, not {expected_status:?}",
            updated["archiveStatus"]
        );

        // And a read of the chat agrees with it, so the update was persisted.
        let (status, read) = get_chat(&router, &space, &execution_id).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200 from GetChat, got {status}: {read}"
        );
        assert_eq!(
            read["summary"], expected_summary,
            "case {case:?}: GetChat reports summary {:?}, not {expected_summary:?}",
            read["summary"]
        );
        assert_eq!(
            read["archiveStatus"], expected_status,
            "case {case:?}: GetChat reports archiveStatus {:?}, not {expected_status:?}",
            read["archiveStatus"]
        );
        assert_eq!(
            read["createdAt"], created["createdAt"],
            "case {case:?}: an update moved createdAt to {:?} from {:?}",
            read["createdAt"], created["createdAt"]
        );
        assert_eq!(
            read["updatedAt"], updated["updatedAt"],
            "case {case:?}: GetChat reports updatedAt {:?}, not the update's {:?}",
            read["updatedAt"], updated["updatedAt"]
        );

        // An update that changed something is the chat's instant of last change; an
        // update that named nothing left that instant where it was.
        let created_at = instant(case, &created, "createdAt");
        let updated_at = instant(case, &read, "updatedAt");
        if summary.is_none() && archive_status.is_none() {
            assert_eq!(
                read["updatedAt"], created["createdAt"],
                "case {case:?}: an update that named nothing moved updatedAt to {:?} from {:?}",
                read["updatedAt"], created["createdAt"]
            );
        } else {
            assert!(
                updated_at >= created_at,
                "case {case:?}: the chat was updated at {updated_at}, before it was created at {created_at}"
            );
        }
    }
}

/// RULE 2 (bound): a summary past the published length is refused by the modelled
/// constraint, and the refused update applies nothing.
#[tokio::test]
async fn update_chat_refuses_a_summary_past_its_published_bound() {
    let router = new_router();
    let space = create_space(&router, "chat-summary-bound").await;
    let (execution_id, _) = create_chat(&router, &space, "summary bound").await;

    // One past each published bound of `summary` (`@length(min: 1, max: 256)`):
    // the empty string below the minimum, and one character past the maximum.
    let too_long = "x".repeat(SUMMARY_MAX_LEN + 1);
    for (case, summary) in [("empty summary", ""), ("one past the maximum", too_long.as_str())] {
        let (status, body) =
            update_chat(&router, &space, &execution_id, json!({"summary": summary})).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected 400, got {status}: {body}"
        );
        assert_eq!(
            error_code(&body),
            "ValidationException",
            "case {case:?}: expected ValidationException, got {body}"
        );
        let (_, read) = get_chat(&router, &space, &execution_id).await;
        assert!(
            read.get("summary").is_none_or(Value::is_null),
            "case {case:?}: the refused update stored a summary: {read}"
        );
    }
}

/// RULE 3: archiving hides a chat execution from the DEFAULT chat list and
/// unarchiving restores it, while `archiveStatus` selects exactly the chats in the
/// status it names.
#[tokio::test]
async fn archiving_hides_a_chat_from_the_default_list_until_it_is_restored() {
    let router = new_router();
    let space = create_space(&router, "chat-archive-filter").await;

    let mut created: Vec<String> = Vec::new();
    for n in 0..4 {
        let (execution_id, _) = create_chat(&router, &space, &format!("chat-{n}")).await;
        created.push(execution_id);
    }
    // Archive the first and third, so neither the archived nor the active set is
    // a contiguous run of the creation order.
    for index in [0, 2] {
        let (status, body) = update_chat(
            &router,
            &space,
            &created[index],
            json!({"archiveStatus": CHAT_ARCHIVE_STATUS_ARCHIVED}),
        )
        .await;
        assert_eq!(
            status, 200,
            "case archive {index}: expected 200, got {status}: {body}"
        );
    }
    let archived: BTreeSet<String> = [created[0].clone(), created[2].clone()].into();
    let active: BTreeSet<String> = [created[1].clone(), created[3].clone()].into();

    // The rows differ along what the rule generalises over: the listing with no
    // filter (the default chat list) and the listing with each value of the
    // `ChatArchiveStatus` enum.
    let rows: [(&str, Option<&str>, &BTreeSet<String>); 3] = [
        ("default list", None, &active),
        ("ACTIVE filter", Some(CHAT_ARCHIVE_STATUS_ACTIVE), &active),
        (
            "ARCHIVED filter",
            Some(CHAT_ARCHIVE_STATUS_ARCHIVED),
            &archived,
        ),
    ];
    for (case, filter, expected) in rows {
        let listed: BTreeSet<String> = walk_pages(&router, &space, None, filter, case)
            .await
            .into_iter()
            .collect();
        assert_eq!(
            &listed, expected,
            "case {case:?}: the listing reports {listed:?}, not {expected:?}"
        );
    }

    // Unarchiving restores the chat to the default chat list, and nothing else.
    let (status, body) = update_chat(
        &router,
        &space,
        &created[0],
        json!({"archiveStatus": CHAT_ARCHIVE_STATUS_ACTIVE}),
    )
    .await;
    assert_eq!(
        status, 200,
        "case unarchive: expected 200, got {status}: {body}"
    );
    let restored: BTreeSet<String> = walk_pages(&router, &space, None, None, "unarchive")
        .await
        .into_iter()
        .collect();
    let expected: BTreeSet<String> = [
        created[0].clone(),
        created[1].clone(),
        created[3].clone(),
    ]
    .into();
    assert_eq!(
        restored, expected,
        "case unarchive: the default list reports {restored:?}, not {expected:?}"
    );
}

/// RULE 4: while a chat execution is archived the only change it accepts is the
/// one that restores it to the default chat list; every other update is refused
/// with the modelled conflict and applies nothing.
#[tokio::test]
async fn an_archived_chat_refuses_every_update_but_the_one_that_restores_it() {
    let router = new_router();
    let space = create_space(&router, "chat-archived-conflict").await;

    // The rows differ along what the rule generalises over: which members the
    // refused patch carries, including the status the chat already has and a patch
    // that names nothing.
    let refused: [(&str, Value); 4] = [
        ("summary only", json!({"summary": "late edit"})),
        (
            "archive again",
            json!({"archiveStatus": CHAT_ARCHIVE_STATUS_ARCHIVED}),
        ),
        (
            "summary and archive",
            json!({"summary": "late edit", "archiveStatus": CHAT_ARCHIVE_STATUS_ARCHIVED}),
        ),
        ("neither member", json!({})),
    ];

    for (case, patch) in refused {
        let (execution_id, _) = create_chat(&router, &space, case).await;
        let (status, body) = update_chat(
            &router,
            &space,
            &execution_id,
            json!({"summary": "before archiving", "archiveStatus": CHAT_ARCHIVE_STATUS_ARCHIVED}),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200 archiving the chat, got {status}: {body}"
        );

        let (status, refusal) = update_chat(&router, &space, &execution_id, patch.clone()).await;
        assert_eq!(
            status, 409,
            "case {case:?}: expected 409 for {patch}, got {status}: {refusal}"
        );
        assert_eq!(
            error_code(&refusal),
            "ConflictException",
            "case {case:?}: expected ConflictException, got {refusal}"
        );
        assert_eq!(
            refusal["reason"],
            json!(CONFLICT_REASON_CHAT_ARCHIVED),
            "case {case:?}: the refusal reports reason {:?}, not {CONFLICT_REASON_CHAT_ARCHIVED}",
            refusal["reason"]
        );
        assert_eq!(
            refusal["message"],
            json!(chat_archived_message(&execution_id)),
            "case {case:?}: the refusal reports message {:?}, not {:?}",
            refusal["message"],
            chat_archived_message(&execution_id)
        );

        // The refused update applied nothing: the chat still carries the summary it
        // was archived with, and is still archived.
        let (_, read) = get_chat(&router, &space, &execution_id).await;
        assert_eq!(
            read["summary"],
            json!("before archiving"),
            "case {case:?}: the refused update changed the summary to {:?}",
            read["summary"]
        );
        assert_eq!(
            read["archiveStatus"],
            json!(CHAT_ARCHIVE_STATUS_ARCHIVED),
            "case {case:?}: the refused update changed archiveStatus to {:?}",
            read["archiveStatus"]
        );

        // The one accepted change restores it, and may carry the rest of the update
        // with it.
        let (status, restored) = update_chat(
            &router,
            &space,
            &execution_id,
            json!({"summary": "after restoring", "archiveStatus": CHAT_ARCHIVE_STATUS_ACTIVE}),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200 restoring the chat, got {status}: {restored}"
        );
        assert_eq!(
            restored["summary"],
            json!("after restoring"),
            "case {case:?}: the restore reports summary {:?}",
            restored["summary"]
        );
        assert_eq!(
            restored["archiveStatus"],
            json!(CHAT_ARCHIVE_STATUS_ACTIVE),
            "case {case:?}: the restore reports archiveStatus {:?}",
            restored["archiveStatus"]
        );
    }
}

/// RULE 5: a repeat carrying the `clientToken` of an update already applied is
/// ignored rather than applied again, and is not an error; a different token is a
/// different request.
#[tokio::test]
async fn update_chat_ignores_a_repeat_of_the_same_client_token() {
    let router = new_router();
    let space = create_space(&router, "chat-idempotency").await;
    let (execution_id, _) = create_chat(&router, &space, "idempotent update").await;

    let (status, first) = update_chat(
        &router,
        &space,
        &execution_id,
        json!({"summary": "first", "clientToken": "tok-chat-1"}),
    )
    .await;
    assert_eq!(status, 200, "case first update: got {status}: {first}");

    // The same token, a different body: the service "ignores the request, but does
    // not return an error", so what it answers is the chat the first call left.
    let (status, replayed) = update_chat(
        &router,
        &space,
        &execution_id,
        json!({"summary": "second", "clientToken": "tok-chat-1"}),
    )
    .await;
    assert_eq!(status, 200, "case replay: got {status}: {replayed}");
    assert_eq!(
        replayed, first,
        "case replay: the repeat answered {replayed}, not the first update's {first}"
    );
    let (_, read) = get_chat(&router, &space, &execution_id).await;
    assert_eq!(
        read["summary"],
        json!("first"),
        "case replay: the ignored repeat changed the summary to {:?}",
        read["summary"]
    );

    // A different token is a different request, so it applies.
    let (status, applied) = update_chat(
        &router,
        &space,
        &execution_id,
        json!({"summary": "second", "clientToken": "tok-chat-2"}),
    )
    .await;
    assert_eq!(status, 200, "case second token: got {status}: {applied}");
    assert_eq!(
        applied["summary"],
        json!("second"),
        "case second token: the update reports summary {:?}",
        applied["summary"]
    );

    // And so is the FIRST token presented for another chat execution: the chat a
    // request names is part of the request, so this one applies rather than being
    // read as a repeat of the update to the chat above.
    let (other, _) = create_chat(&router, &space, "another chat").await;
    let (status, elsewhere) = update_chat(
        &router,
        &space,
        &other,
        json!({"summary": "other chat", "clientToken": "tok-chat-1"}),
    )
    .await;
    assert_eq!(
        status, 200,
        "case token reused on another chat: got {status}: {elsewhere}"
    );
    assert_eq!(
        elsewhere["summary"],
        json!("other chat"),
        "case token reused on another chat: the update reports summary {:?}",
        elsewhere["summary"]
    );
    let (_, read) = get_chat(&router, &space, &execution_id).await;
    assert_eq!(
        read["summary"],
        json!("second"),
        "case token reused on another chat: the first chat's summary became {:?}",
        read["summary"]
    );
}

/// RULE 6: every chat operation authorises the caller against the agent space
/// first, so a space that resolves to nothing fails CLOSED with the wholly fixed
/// `Access denied`, disclosing no identifier.
#[tokio::test]
async fn chat_operations_fail_closed_for_a_space_that_resolves_to_nothing() {
    let router = new_router();
    let space = create_space(&router, "chat-fail-closed").await;
    let (execution_id, _) = create_chat(&router, &space, "fail closed").await;

    // The rows differ in length (both published bounds of `AgentSpaceIdentifier`,
    // `@length(min: 1, max: 2048)`), character class and form, and include a
    // well-formed minted identifier that names no stored space.
    let longest = "a".repeat(2048);
    let absent_spaces = [
        "a",
        "as-ffffffffffffffff",
        "AS-0000000000000000",
        "not.a.space_id-9",
        longest.as_str(),
    ];
    for absent in absent_spaces {
        for (case, method, uri, body) in [
            (
                "CreateChat",
                "POST",
                format!("/agents/agent-space/{absent}/chat/create"),
                None,
            ),
            (
                "ListChats",
                "GET",
                format!("/agents/agent-space/{absent}/chat/list"),
                None,
            ),
            (
                "GetChat",
                "GET",
                format!("/agents/agent-space/{absent}/chat/get/{execution_id}"),
                None,
            ),
            (
                "UpdateChat",
                "PATCH",
                format!("/agents/agent-space/{absent}/chat/{execution_id}"),
                Some(json!({"summary": "unreachable"})),
            ),
        ] {
            let (status, refusal) = call(&router, method, &uri, body).await;
            assert_eq!(
                status, 403,
                "case {case:?} in space {absent:?}: expected 403, got {status}: {refusal}"
            );
            assert_eq!(
                error_code(&refusal),
                "AccessDeniedException",
                "case {case:?} in space {absent:?}: expected AccessDeniedException, got {refusal}"
            );
            assert_eq!(
                refusal["message"],
                json!(ACCESS_DENIED_MESSAGE),
                "case {case:?} in space {absent:?}: the refusal reports message {:?}, not {ACCESS_DENIED_MESSAGE:?}",
                refusal["message"]
            );
        }
    }

    // The same chat is served inside the space that owns it, so the rows above are
    // a decision about the space and not a blanket refusal.
    let (status, served) = get_chat(&router, &space, &execution_id).await;
    assert_eq!(status, 200, "case served: got {status}: {served}");
}

/// RULE 6: inside a space the caller reached, an `executionId` that names no chat
/// is HTTP 404 naming the resource that was resolved — including one minted in
/// another space, which is as absent as one that never existed.
#[tokio::test]
async fn chat_operations_refuse_an_execution_id_that_names_nothing() {
    let router = new_router();
    let space = create_space(&router, "chat-not-found").await;
    let other_space = create_space(&router, "chat-not-found-other").await;
    let (elsewhere, _) = create_chat(&router, &other_space, "another space").await;
    let (present, _) = create_chat(&router, &space, "present").await;

    // The rows differ in form, digit case and value — every one satisfying
    // `ResourceId` (`^[a-zA-Z0-9_.-]+$`, 1..128), so the modelled constraint admits
    // them and the lookup is what refuses them — and the last is a chat this
    // service minted in ANOTHER space.
    let absent = [
        "chat-ffffffffffffffff",
        "chat-FFFFFFFFFFFFFFFF",
        "chat-0",
        "0001",
        "task-0000000000000001",
        elsewhere.as_str(),
    ];
    for case in absent {
        for (operation, method, uri, body) in [
            (
                "GetChat",
                "GET",
                format!("/agents/agent-space/{space}/chat/get/{case}"),
                None,
            ),
            (
                "UpdateChat",
                "PATCH",
                format!("/agents/agent-space/{space}/chat/{case}"),
                Some(json!({"summary": "no such chat"})),
            ),
        ] {
            let (status, refusal) = call(&router, method, &uri, body).await;
            assert_eq!(
                status, 404,
                "case {case:?} on {operation}: expected 404, got {status}: {refusal}"
            );
            assert_eq!(
                error_code(&refusal),
                "ResourceNotFoundException",
                "case {case:?} on {operation}: expected ResourceNotFoundException, got {refusal}"
            );
            assert_eq!(
                refusal["message"],
                json!(chat_not_found_message(case)),
                "case {case:?} on {operation}: the refusal reports message {:?}, not {:?}",
                refusal["message"],
                chat_not_found_message(case)
            );
        }
    }

    // The chat that IS in this space is served, and the one in the other space is
    // served there.
    for (case, space_id, execution) in [
        ("present here", space.as_str(), present.as_str()),
        ("present there", other_space.as_str(), elsewhere.as_str()),
    ] {
        let (status, served) = get_chat(&router, space_id, execution).await;
        assert_eq!(status, 200, "case {case:?}: got {status}: {served}");
    }
}

/// RULE 7: the listing reports the newest chat execution first, pages at the size
/// the request asks for, and issues a `nextToken` only while chats follow the page
/// it served — so walking the pages enumerates every chat exactly once.
#[tokio::test]
async fn list_chats_pages_newest_first() {
    let router = new_router();
    let space = create_space(&router, "chat-paging").await;

    let mut created: Vec<String> = Vec::new();
    for n in 0..7 {
        let (execution_id, _) = create_chat(&router, &space, &format!("page-{n}")).await;
        created.push(execution_id);
    }
    // Newest first: the reverse of the order they were created in.
    let newest_first: Vec<String> = created.iter().rev().cloned().collect();

    // The rows differ in page size: both published bounds of `maxResults`, a size
    // that divides the stored count, one that does not, and no bound at all.
    for max_results in [
        Some(MAX_RESULTS_MIN),
        Some(2),
        Some(3),
        Some(MAX_RESULTS_MAX),
        None,
    ] {
        let case = format!("maxResults={max_results:?}");
        let walked = walk_pages(&router, &space, max_results, None, &case).await;
        assert_eq!(
            walked, newest_first,
            "case {case:?}: walking the pages reported {walked:?}, not {newest_first:?}"
        );
    }

    // A page carries no token once it served the last chat.
    let (status, body) = list_chats(&router, &space, "?maxResults=20").await;
    assert_eq!(status, 200, "case last page: got {status}: {body}");
    assert!(
        body["nextToken"].is_null(),
        "case last page: a page that served every chat still carries {:?}",
        body["nextToken"]
    );

    // A `maxResults` outside the published range is refused by the modelled
    // constraint.
    for case in [MAX_RESULTS_MIN - 1, MAX_RESULTS_MAX + 1] {
        let (status, body) = list_chats(&router, &space, &format!("?maxResults={case}")).await;
        assert_eq!(
            status, 400,
            "case maxResults={case}: expected 400, got {status}: {body}"
        );
        assert_eq!(
            error_code(&body),
            "ValidationException",
            "case maxResults={case}: expected ValidationException, got {body}"
        );
    }

    // A `nextToken` this service did not issue names no position in the listing.
    for case in ["not-a-token", "v1:zzzz", "v1:6368617431", ""] {
        let (status, body) = list_chats(&router, &space, &format!("?nextToken={case}")).await;
        assert_eq!(
            status, 400,
            "case nextToken={case:?}: expected 400, got {status}: {body}"
        );
        assert_eq!(
            error_code(&body),
            "ValidationException",
            "case nextToken={case:?}: expected ValidationException, got {body}"
        );
    }
}

/// RULE 7: a page cursor names a POSITION in the newest-first order rather than an
/// index into one snapshot of it, so a walk stays correct while chats are created
/// around it.
#[tokio::test]
async fn a_chat_page_cursor_names_a_position_not_an_index() {
    let router = new_router();
    let space = create_space(&router, "chat-cursor").await;

    let mut created: Vec<String> = Vec::new();
    for n in 0..4 {
        let (execution_id, _) = create_chat(&router, &space, &format!("cursor-{n}")).await;
        created.push(execution_id);
    }

    let (status, first) = list_chats(&router, &space, "?maxResults=2").await;
    assert_eq!(status, 200, "case first page: got {status}: {first}");
    let page = listed_ids("first page", &first);
    assert_eq!(
        page,
        vec![created[3].clone(), created[2].clone()],
        "case first page: reported {page:?}"
    );
    let token = first["nextToken"]
        .as_str()
        .unwrap_or_else(|| panic!("case first page: no nextToken in {first}"))
        .to_string();

    // A chat created after the first page was served is NEWER than the cursor, so
    // the rest of the walk is unaffected by it: the second page carries exactly the
    // chats that followed the cursor.
    let (newer, _) = create_chat(&router, &space, "created mid-walk").await;
    let (status, second) = list_chats(&router, &space, &format!("?maxResults=2&nextToken={token}"))
        .await;
    assert_eq!(status, 200, "case second page: got {status}: {second}");
    let page = listed_ids("second page", &second);
    assert_eq!(
        page,
        vec![created[1].clone(), created[0].clone()],
        "case second page: reported {page:?}"
    );
    assert!(
        !page.contains(&newer),
        "case second page: the chat created mid-walk appeared behind the cursor: {page:?}"
    );
    assert!(
        second["nextToken"].is_null(),
        "case second page: the last page carries {:?}",
        second["nextToken"]
    );

    // A fresh walk sees the new chat at the front.
    let walked = walk_pages(&router, &space, Some(2), None, "fresh walk").await;
    assert_eq!(
        walked.first(),
        Some(&newer),
        "case fresh walk: reported {walked:?}"
    );
}

/// RULE: the chat store one agent space holds is bounded — a create past the cap
/// is refused with the modelled quota error and the store stops growing.
#[tokio::test]
async fn chat_creates_are_refused_at_the_store_bound() {
    let router = new_router();
    let space = create_space(&router, "chat-cap").await;

    for n in 0..MAX_CHATS_PER_AGENT_SPACE {
        create_chat(&router, &space, &format!("cap-{n}")).await;
    }
    let (status, body) = create_chat_raw(&router, &space, "").await;
    assert_eq!(
        status, 402,
        "case chat cap: expected 402, got {status}: {body}"
    );
    assert_eq!(
        error_code(&body),
        "ServiceQuotaExceededException",
        "case chat cap: expected ServiceQuotaExceededException, got {body}"
    );

    // The refused create grew nothing, and the chats already stored are still
    // served.
    let listed = walk_pages(&router, &space, Some(MAX_RESULTS_MAX), None, "chat cap").await;
    assert_eq!(
        listed.len(),
        MAX_CHATS_PER_AGENT_SPACE,
        "case chat cap: the refused create grew the store to {}",
        listed.len()
    );
}

/// RULE 8: a read reports the store and changes nothing a later caller sees.
#[tokio::test]
async fn chat_reads_change_nothing_a_later_caller_sees() {
    let router = new_router();
    let space = create_space(&router, "chat-reads").await;
    let (execution_id, _) = create_chat(&router, &space, "read twice").await;
    let (status, body) = update_chat(
        &router,
        &space,
        &execution_id,
        json!({"summary": "read me twice"}),
    )
    .await;
    assert_eq!(status, 200, "case setup update: got {status}: {body}");

    for case in ["first read", "second read"] {
        let (status, read) = get_chat(&router, &space, &execution_id).await;
        assert_eq!(status, 200, "case {case:?}: got {status}: {read}");
        assert_eq!(
            read["summary"],
            json!("read me twice"),
            "case {case:?}: reports summary {:?}",
            read["summary"]
        );
        assert_eq!(
            read["archiveStatus"],
            json!(CHAT_ARCHIVE_STATUS_ACTIVE),
            "case {case:?}: reports archiveStatus {:?}",
            read["archiveStatus"]
        );
    }

    let (status, before) = list_chats(&router, &space, "").await;
    assert_eq!(status, 200, "case list before: got {status}: {before}");
    let (status, after) = list_chats(&router, &space, "").await;
    assert_eq!(status, 200, "case list after: got {status}: {after}");
    assert_eq!(
        before, after,
        "case repeated listing: the second listing reported {after}, not {before}"
    );
}
