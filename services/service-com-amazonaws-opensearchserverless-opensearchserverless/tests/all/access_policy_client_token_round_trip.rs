// SPDX-License-Identifier: CC0-1.0

//! A data access policy created with an idempotency token, end to end over the wire: the
//! create, the retry of it, the read, the write and the delete.
//!
//! Every case drives the real request path (`MultiServiceRouter::dispatch` with `POST /`,
//! `X-Amz-Target: OpenSearchServerless.<Operation>` and an `awsJson1_0` body) through the
//! harness this crate's `collection_lifecycle` suite owns, so the boundary validator, the
//! protocol codec and the handler all take part exactly as they do for an SDK client.
//!
//! The rule under test: `clientToken` only decides whether a repeated create is a RETRY of
//! the first one — it changes nothing about what the create STORES. A create that carries one
//! leaves exactly the policy a create without one leaves, the retry answers that same stored
//! policy and neither creates a second one nor drops the first, and every later call of that
//! `(type, name)` pair — `GetAccessPolicy`, `ListAccessPolicies`, `UpdateAccessPolicy`,
//! `DeleteAccessPolicy` — is served exactly as it is for a create that carried no token. Once
//! the delete is served the pair is gone, and the same token may name a new policy.
//!
//! The rows span what the rule generalises over: the token's presence and its SIZE at both
//! bounds of `ClientToken` (`@length(min: 1, max: 512)`, `@idempotencyToken` —
//! `smithy_query(query="shape", shape="ClientToken")`), the SDK-generated UUID form a client
//! sends without being asked, the number of retries, whether the retry's body is identical,
//! and both spellings of a data access policy document (one statement object and the list of
//! statements AWS's own documents are written as).

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::collection_lifecycle::{at, call, emulator, text, CLIENT_ERROR};

/// One statement, as a data access policy document is written: the LIST of statements form.
const LIST_DOCUMENT: &str = r#"[{"Rules":[{"ResourceType":"collection","Resource":["collection/*"],"Permission":["aoss:DescribeCollectionItems"]}],"Principal":["arn:aws:iam::000000000000:root"]}]"#;

/// The same statement written as a bare object, which this API accepts too.
const OBJECT_DOCUMENT: &str = r#"{"Rules":[{"ResourceType":"index","Resource":["index/books/*"],"Permission":["aoss:ReadDocument"]}],"Principal":["arn:aws:iam::000000000000:role/Reader"]}"#;

/// The description every policy below is created with.
const DESCRIPTION: &str = "an idempotent clientToken probe";

/// One row: what the create carries as its `clientToken`, how many times the create is
/// retried under it, and the document the policy is created with.
struct Row {
    case: &'static str,
    token: Option<String>,
    retries: usize,
    document: &'static str,
}

/// The rows: the token forms a client really sends, at both bounds of `ClientToken`, with
/// the two document spellings and one and two retries.
fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "no clientToken at all",
            token: None,
            retries: 0,
            document: LIST_DOCUMENT,
        },
        Row {
            case: "a clientToken at the 1-character minimum",
            token: Some("x".to_string()),
            retries: 1,
            document: LIST_DOCUMENT,
        },
        Row {
            case: "an SDK-generated UUID clientToken",
            token: Some("3f2504e0-4f89-11d3-9a0c-0305e82c3301".to_string()),
            retries: 2,
            document: LIST_DOCUMENT,
        },
        Row {
            case: "a clientToken at the 512-character maximum",
            token: Some("t".repeat(512)),
            retries: 1,
            document: OBJECT_DOCUMENT,
        },
        Row {
            case: "an SDK-generated UUID clientToken on the object document",
            token: Some("16fd2706-8baf-433b-82eb-8c7fada847da".to_string()),
            retries: 1,
            document: OBJECT_DOCUMENT,
        },
    ]
}

/// The `CreateAccessPolicy` body for `row`, under `name`.
fn create_body(row: &Row, name: &str) -> Value {
    let mut body = json!({
        "type": "data",
        "name": name,
        "description": DESCRIPTION,
        "policy": row.document,
    });
    if let Some(token) = &row.token {
        body["clientToken"] = json!(token);
    }
    body
}

/// The names `ListAccessPolicies` reports for the `data` type.
async fn listed_names(router: &MultiServiceRouter, case: &str) -> Vec<String> {
    let (status, code, answer) = call(router, "ListAccessPolicies", json!({ "type": "data" })).await;
    assert_eq!(
        status, 200,
        "case {case}: expected 200 from ListAccessPolicies, got {status} {code} with {answer}"
    );
    at(&answer, "accessPolicySummaries")
        .as_array()
        .map(|summaries| {
            summaries
                .iter()
                .map(|summary| text(summary, "name"))
                .collect()
        })
        .unwrap_or_default()
}

#[tokio::test]
async fn a_token_bearing_create_leaves_the_policy_every_later_call_addresses() {
    for row in rows() {
        // One store per row, so what a row observes is what that row's own calls left.
        let router = emulator();
        // `PolicyName` is @length(min: 3, max: 32) with `^[a-z][a-z0-9-]+$`.
        let name = "probe-idem-policy";
        let case = row.case;

        let (status, code, created) = call(&router, "CreateAccessPolicy", create_body(&row, name)).await;
        assert_eq!(
            status, 200,
            "case {case}: expected 200 from the create, got {status} {code} with {created}"
        );
        let stored = at(&created, "accessPolicyDetail").clone();
        let version = text(&stored, "policyVersion");

        // Every retry under the SAME token is a retry of that create: the same record, and
        // nothing else created or dropped.
        for attempt in 1..=row.retries {
            let (status, code, replay) =
                call(&router, "CreateAccessPolicy", create_body(&row, name)).await;
            assert_eq!(
                status, 200,
                "case {case}, retry {attempt}: expected 200, got {status} {code} with {replay}"
            );
            assert_eq!(
                at(&replay, "accessPolicyDetail"),
                &stored,
                "case {case}, retry {attempt}: a replayed clientToken answers the record the \
                 first create stored, got {replay}"
            );
        }

        // The read: the policy the create reported is the policy that is there.
        let (status, code, read) = call(
            &router,
            "GetAccessPolicy",
            json!({ "type": "data", "name": name }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case}: expected 200 from GetAccessPolicy after the create, got {status} \
             {code} with {read}"
        );
        let reported = at(&read, "accessPolicyDetail").clone();
        assert_eq!(
            reported, stored,
            "case {case}: the read must report the policy the create stored, got {read}"
        );
        for (member, expected) in [
            ("type", "data".to_string()),
            ("name", name.to_string()),
            ("description", DESCRIPTION.to_string()),
            ("policyVersion", version.clone()),
        ] {
            assert_eq!(
                text(&reported, member),
                expected,
                "case {case}: expected {member} {expected:?} in the read, got {reported}"
            );
        }
        assert_eq!(
            at(&reported, "policy"),
            &serde_json::from_str::<Value>(row.document).expect("the row's document is JSON"),
            "case {case}: the read must report the document the create was given, got {reported}"
        );
        assert!(
            at(&reported, "createdDate").is_i64() && at(&reported, "lastModifiedDate").is_i64(),
            "case {case}: the read must report both epoch stamps, got {reported}"
        );

        // The listing reports exactly that one policy.
        assert_eq!(
            listed_names(&router, case).await,
            vec![name.to_string()],
            "case {case}: the listing must report the created policy and nothing else"
        );

        // The write: addressed at the version the create minted, which only exists because
        // the create stored it.
        let (status, code, written) = call(
            &router,
            "UpdateAccessPolicy",
            json!({
                "type": "data",
                "name": name,
                "policyVersion": version,
                "description": "written after the idempotent create",
            }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case}: expected 200 from UpdateAccessPolicy, got {status} {code} with {written}"
        );
        assert_eq!(
            text(at(&written, "accessPolicyDetail"), "description"),
            "written after the idempotent create",
            "case {case}: the write must report what it wrote, got {written}"
        );

        // The delete: a caller can clean up what it created.
        let (status, code, deleted) = call(
            &router,
            "DeleteAccessPolicy",
            json!({ "type": "data", "name": name }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case}: expected 200 from DeleteAccessPolicy, got {status} {code} with {deleted}"
        );

        // And the pair is gone: the read is refused, the listing is empty.
        let (status, code, gone) = call(
            &router,
            "GetAccessPolicy",
            json!({ "type": "data", "name": name }),
        )
        .await;
        assert_eq!(
            (status, code.as_str()),
            (CLIENT_ERROR, "ResourceNotFoundException"),
            "case {case}: expected {CLIENT_ERROR} ResourceNotFoundException after the delete, \
             got {status} {code} with {gone}"
        );
        assert!(
            listed_names(&router, case).await.is_empty(),
            "case {case}: the listing must report no policy after the delete"
        );

        // The token outlives nothing: spent again after the delete it names a new policy,
        // rather than replaying the deleted one.
        let (status, code, again) = call(&router, "CreateAccessPolicy", create_body(&row, name)).await;
        assert_eq!(
            status, 200,
            "case {case}: expected 200 from the create after the delete, got {status} {code} \
             with {again}"
        );
        let recreated = at(&again, "accessPolicyDetail").clone();
        assert_ne!(
            text(&recreated, "policyVersion"),
            version,
            "case {case}: the policy created after the delete is a new one, so it carries its \
             own version, got {recreated}"
        );
        let (status, code, read_again) = call(
            &router,
            "GetAccessPolicy",
            json!({ "type": "data", "name": name }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case}: expected 200 from GetAccessPolicy after the second create, got \
             {status} {code} with {read_again}"
        );
        assert_eq!(
            at(&read_again, "accessPolicyDetail"),
            &recreated,
            "case {case}: the read must report the policy the second create stored, got \
             {read_again}"
        );
    }
}

#[tokio::test]
async fn one_token_spent_on_two_pairs_creates_both_policies() {
    let router = emulator();
    let token = "a-token-two-creates-share";

    // The same token spent on a DIFFERENT (type, name) pair is a request for that other
    // policy: it is created on its own terms, and the first one is left exactly where it is.
    for name in ["probe-first-policy", "probe-second-policy"] {
        let (status, code, created) = call(
            &router,
            "CreateAccessPolicy",
            json!({
                "type": "data",
                "name": name,
                "description": DESCRIPTION,
                "policy": LIST_DOCUMENT,
                "clientToken": token,
            }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case CreateAccessPolicy({name}) under a shared token: expected 200, got {status} \
             {code} with {created}"
        );
        assert_eq!(
            text(at(&created, "accessPolicyDetail"), "name"),
            name,
            "case CreateAccessPolicy({name}) under a shared token: expected the policy it \
             asked for, got {created}"
        );
    }

    let mut listed = listed_names(&router, "two pairs under one token").await;
    listed.sort();
    assert_eq!(
        listed,
        vec![
            "probe-first-policy".to_string(),
            "probe-second-policy".to_string()
        ],
        "case two pairs under one token: both policies must be stored"
    );

    // And each is readable and deletable on its own.
    for name in ["probe-first-policy", "probe-second-policy"] {
        let (status, code, read) = call(
            &router,
            "GetAccessPolicy",
            json!({ "type": "data", "name": name }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case GetAccessPolicy({name}) under a shared token: expected 200, got {status} \
             {code} with {read}"
        );
        let (status, code, deleted) = call(
            &router,
            "DeleteAccessPolicy",
            json!({ "type": "data", "name": name }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case DeleteAccessPolicy({name}) under a shared token: expected 200, got {status} \
             {code} with {deleted}"
        );
    }
}
