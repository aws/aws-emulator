// SPDX-License-Identifier: CC0-1.0

//! The ECR registry permissions policy, end to end over the wire.
//!
//! The rule under test: this registry carries at most ONE permissions policy.
//! `PutRegistryPolicy` validates the document as a resource-based IAM policy and then
//! stores it, replacing whatever was there; `GetRegistryPolicy` reports the
//! stored document, as often as it is asked, and changes nothing;
//! `DeleteRegistryPolicy` removes it and reports what it removed; and a read or a delete
//! of a registry that carries no policy is refused with `RegistryPolicyNotFoundException`
//! (HTTP 400). A document that is not a usable registry policy is refused with
//! `InvalidParameterException` (HTTP 400) and leaves the stored policy as it was.
//!
//! The document the three operations report is the submitted one in the canonical
//! IAM-policy rendering, so what is compared here is the policy's CONTENT
//! ([`crate::policy_document_canonical_form::document_content`]) plus the three
//! operations agreeing on the same text; the rendering itself is pinned byte for byte in
//! `policy_document_canonical_form.rs`.
//!
//! Every case goes through the router, so it exercises routing, deserialization, the
//! handler, the persisted state and serialization — the same path a client takes.

use serde_json::{json, Value};

use crate::policy_document_canonical_form::document_content;
use crate::wire::{call, emulator, ok, refused, REGISTRY_ID};

/// The documented ceiling on a registry policy document: `RegistryPolicyText` is
/// modelled `@length(min: 0, max: 10240)` — `smithy_query(query="operation",
/// operation="PutRegistryPolicy")`.
const MAX_POLICY_TEXT: usize = 10_240;

/// A policy document the registry accepts, used wherever a case only needs one stored.
const A_STORED_POLICY: &str = r#"{"Version":"2012-10-17","Statement":[{"Sid":"Baseline","Effect":"Allow","Principal":{"AWS":"arn:aws:iam::210987654321:root"},"Action":["ecr:ReplicateImage"],"Resource":"*"}]}"#;

/// A document written across several lines, whose own layout the rendering replaces.
const PRETTY_PRINTED_POLICY: &str = r#"{
  "Version": "2012-10-17",
  "Statement": [
    {
      "Sid": "ReplicateToPartner",
      "Effect": "Allow",
      "Principal": { "AWS": "arn:aws:iam::123456789012:root" },
      "Action": [ "ecr:ReplicateImage" ],
      "Resource": "*"
    }
  ]
}"#;

/// A document whose text is not ASCII: the `Sid` and a condition value carry characters
/// from several scripts, including one outside the basic multilingual plane.
const UNICODE_POLICY: &str = r#"{"Version":"2012-10-17","Statement":[{"Sid":"réplicaçãoAndDéjàVu日本語🚀","Effect":"Allow","Principal":{"AWS":"arn:aws:iam::123456789012:root"},"Action":["ecr:ReplicateImage"],"Resource":"*","Condition":{"StringEquals":{"aws:RequestTag/équipe":"ação-日本語-🚀"}}}]}"#;

/// A policy document of exactly `size` characters: the skeleton of a one-statement
/// registry policy, with its `Sid` padded out to fill the rest.
fn policy_of_size(size: usize) -> String {
    let document = |sid: &str| {
        format!(
            "{{\"Version\":\"2012-10-17\",\"Statement\":[{{\"Sid\":\"{sid}\",\"Effect\":\"Allow\",\
             \"Principal\":{{\"AWS\":\"arn:aws:iam::123456789012:root\"}},\
             \"Action\":[\"ecr:ReplicateImage\"],\"Resource\":\"*\"}}]}}"
        )
    };
    let skeleton = document("").chars().count();
    assert!(
        size >= skeleton,
        "a {size}-character policy cannot hold the {skeleton}-character skeleton"
    );
    document(&"a".repeat(size - skeleton))
}

/// The policy documents a put must accept and a get must report back with its content
/// intact. The rows
/// differ along what the rule generalises over: the shape of `Statement` (one object or
/// an array), the policy version, the principal form, the action element, the document's
/// whitespace, its character set and its size.
fn accepted_documents() -> Vec<(&'static str, String)> {
    vec![
        (
            "a single statement in an array",
            r#"{"Version":"2012-10-17","Statement":[{"Sid":"Replicate","Effect":"Allow","Principal":{"AWS":"arn:aws:iam::123456789012:root"},"Action":["ecr:ReplicateImage"],"Resource":"*"}]}"#
                .to_string(),
        ),
        (
            "one statement written as an object rather than an array",
            r#"{"Version":"2012-10-17","Statement":{"Effect":"Allow","Principal":{"AWS":"arn:aws:iam::123456789012:root"},"Action":"ecr:ReplicateImage","Resource":"*"}}"#
                .to_string(),
        ),
        (
            // The legacy version the IAM policy reference documents beside 2012-10-17.
            "the legacy policy version",
            r#"{"Version":"2008-10-17","Statement":[{"Effect":"Allow","Principal":{"AWS":"arn:aws:iam::123456789012:root"},"Action":"ecr:ReplicateImage","Resource":"*"}]}"#
                .to_string(),
        ),
        (
            "no Version element, which a policy need not declare",
            r#"{"Statement":[{"Effect":"Allow","Principal":{"AWS":"arn:aws:iam::123456789012:root"},"Action":"ecr:ReplicateImage","Resource":"*"}]}"#
                .to_string(),
        ),
        (
            "an Allow and a Deny, over a list of principals",
            r#"{"Version":"2012-10-17","Statement":[{"Sid":"AllowTwo","Effect":"Allow","Principal":{"AWS":["arn:aws:iam::123456789012:root","arn:aws:iam::123456789012:role/replicator"]},"Action":["ecr:ReplicateImage","ecr:BatchImportUpstreamImage"],"Resource":"*"},{"Sid":"DenyOne","Effect":"Deny","Principal":{"AWS":"arn:aws:iam::999988887777:root"},"Action":"ecr:ReplicateImage","Resource":"*"}]}"#
                .to_string(),
        ),
        (
            "a service principal rather than an account",
            r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Principal":{"Service":"replication.ecr.amazonaws.com"},"Action":"ecr:ReplicateImage","Resource":"*"}]}"#
                .to_string(),
        ),
        (
            "NotAction in place of Action",
            r#"{"Version":"2012-10-17","Statement":[{"Effect":"Deny","Principal":{"AWS":"arn:aws:iam::123456789012:root"},"NotAction":["ecr:ReplicateImage"],"Resource":"*"}]}"#
                .to_string(),
        ),
        (
            "a pretty-printed document, whose own layout the rendering replaces",
            PRETTY_PRINTED_POLICY.to_string(),
        ),
        ("a document whose text is not ASCII", UNICODE_POLICY.to_string()),
        (
            "a document at the modelled size ceiling",
            policy_of_size(MAX_POLICY_TEXT),
        ),
        (
            "a document one character inside the ceiling",
            policy_of_size(MAX_POLICY_TEXT - 1),
        ),
    ]
}

/// The documents a put must refuse. Each row is a way a document fails the policy
/// grammar: it is not JSON, not an object, carries no statements, or carries a statement
/// missing or misusing one of the elements a resource-based policy needs — plus the one
/// rule a REGISTRY policy adds, that it may not grant to every principal.
fn refused_documents() -> Vec<(&'static str, &'static str)> {
    vec![
        ("an empty document", ""),
        ("text that is not JSON at all", "not a policy"),
        (
            "JSON cut off mid-document",
            r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow""#,
        ),
        ("a JSON array rather than an object", "[]"),
        ("a bare JSON string", r#""a policy""#),
        ("an object with no Statement element", r#"{"Version":"2012-10-17"}"#),
        (
            "a Statement element holding no statements",
            r#"{"Version":"2012-10-17","Statement":[]}"#,
        ),
        (
            "a statement that is not an object",
            r#"{"Version":"2012-10-17","Statement":["ecr:ReplicateImage"]}"#,
        ),
        (
            "a policy version the policy language does not define",
            r#"{"Version":"2019-01-01","Statement":[{"Effect":"Allow","Principal":{"AWS":"arn:aws:iam::123456789012:root"},"Action":"ecr:ReplicateImage"}]}"#,
        ),
        (
            "a statement with no Effect",
            r#"{"Version":"2012-10-17","Statement":[{"Principal":{"AWS":"arn:aws:iam::123456789012:root"},"Action":"ecr:ReplicateImage"}]}"#,
        ),
        (
            "an Effect that is neither Allow nor Deny",
            r#"{"Version":"2012-10-17","Statement":[{"Effect":"Permit","Principal":{"AWS":"arn:aws:iam::123456789012:root"},"Action":"ecr:ReplicateImage"}]}"#,
        ),
        (
            "a statement naming no action",
            r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Principal":{"AWS":"arn:aws:iam::123456789012:root"},"Resource":"*"}]}"#,
        ),
        (
            "an empty Action list",
            r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Principal":{"AWS":"arn:aws:iam::123456789012:root"},"Action":[]}]}"#,
        ),
        (
            "a statement naming no principal",
            r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Action":"ecr:ReplicateImage","Resource":"*"}]}"#,
        ),
        (
            "an empty Principal",
            r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Principal":{},"Action":"ecr:ReplicateImage"}]}"#,
        ),
        (
            "a wildcard principal",
            r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Principal":"*","Action":"ecr:ReplicateImage","Resource":"*"}]}"#,
        ),
        (
            "a wildcard AWS principal",
            r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Principal":{"AWS":"*"},"Action":"ecr:ReplicateImage","Resource":"*"}]}"#,
        ),
        (
            "a wildcard beside a named principal",
            r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Principal":{"AWS":["arn:aws:iam::123456789012:root","*"]},"Action":"ecr:ReplicateImage","Resource":"*"}]}"#,
        ),
    ]
}

/// The `policyText` a call reported, or `Value::Null` when it reported none.
fn policy_text(body: &Value) -> Value {
    body["policyText"].clone()
}

/// The policy document a call reported, as a comparison sees it.
fn reported_content(body: &Value) -> Value {
    document_content(
        body["policyText"]
            .as_str()
            .unwrap_or_else(|| panic!("the answer must carry a policyText, got {body:?}")),
    )
}

/// A document the registry accepts is read back with its content intact however many
/// times it is asked for, and replaced — not merged — by the next put.
#[tokio::test]
async fn a_stored_policy_document_round_trips_and_the_last_put_wins() {
    let router = emulator();

    for (case, document) in accepted_documents() {
        let put = ok(
            &router,
            case,
            "PutRegistryPolicy",
            &json!({"policyText": document}),
        )
        .await;
        assert_eq!(
            reported_content(&put),
            document_content(&document),
            "case {case:?}: the put reports the document it stored, got {:?}",
            policy_text(&put)
        );
        assert_eq!(
            put["registryId"],
            json!(REGISTRY_ID),
            "case {case:?}: the put names this registry, got {:?}",
            put["registryId"]
        );

        // Two reads: the document comes back unchanged, and reading it does not consume
        // it — the second read sees exactly what the first did.
        for read in 1..=2 {
            let got = ok(&router, case, "GetRegistryPolicy", &json!({})).await;
            assert_eq!(
                policy_text(&got),
                policy_text(&put),
                "case {case:?}: read {read} must report the text the put did, got {:?}",
                policy_text(&got)
            );
            assert_eq!(
                reported_content(&got),
                document_content(&document),
                "case {case:?}: read {read} must report the stored document, got {:?}",
                policy_text(&got)
            );
            assert_eq!(
                got["registryId"],
                json!(REGISTRY_ID),
                "case {case:?}: read {read} names this registry, got {:?}",
                got["registryId"]
            );
        }
    }

    // The registry holds ONE policy: after that run of puts the stored document is the
    // last one put, not any of the ones before it.
    let last = accepted_documents()
        .pop()
        .expect("the accepted-document table is not empty")
        .1;
    let got = ok(&router, "after every put", "GetRegistryPolicy", &json!({})).await;
    assert_eq!(
        reported_content(&got),
        document_content(&last),
        "the stored policy is the last one put, got {:?}",
        policy_text(&got)
    );

    // And the delete reports the document it removed, in the same text the read did.
    let deleted = ok(&router, "after every put", "DeleteRegistryPolicy", &json!({})).await;
    assert_eq!(
        policy_text(&deleted),
        policy_text(&got),
        "the delete reports the text the read did, got {:?}",
        policy_text(&deleted)
    );
    assert_eq!(
        deleted["registryId"],
        json!(REGISTRY_ID),
        "the delete names this registry, got {:?}",
        deleted["registryId"]
    );
}

/// A document that is not a usable registry policy is refused, and the registry keeps
/// the policy it already had.
#[tokio::test]
async fn a_document_that_is_not_a_registry_policy_is_refused_and_stores_nothing() {
    let router = emulator();
    ok(
        &router,
        "the policy under protection",
        "PutRegistryPolicy",
        &json!({"policyText": A_STORED_POLICY}),
    )
    .await;

    for (case, document) in refused_documents() {
        refused(
            &router,
            case,
            "PutRegistryPolicy",
            &json!({"policyText": document}),
            "InvalidParameterException",
        )
        .await;

        let got = ok(&router, case, "GetRegistryPolicy", &json!({})).await;
        assert_eq!(
            reported_content(&got),
            document_content(A_STORED_POLICY),
            "case {case:?}: a refused put leaves the stored policy as it was, got {:?}",
            policy_text(&got)
        );
    }

    // One character past the modelled ceiling is refused at the boundary, before the
    // handler runs — with the client-input error this service models, naming the member
    // and the bound (`override_refusal` in `src/service.rs`) — and stores nothing either.
    let too_long = policy_of_size(MAX_POLICY_TEXT + 1);
    refused(
        &router,
        "a document one character past the modelled ceiling",
        "PutRegistryPolicy",
        &json!({"policyText": too_long}),
        "InvalidParameterException",
    )
    .await;
    let got = ok(&router, "past the ceiling", "GetRegistryPolicy", &json!({})).await;
    assert_eq!(
        reported_content(&got),
        document_content(A_STORED_POLICY),
        "an over-long document stores nothing, got {:?}",
        policy_text(&got)
    );
}

/// A registry that carries no policy has none to report and none to delete: both calls
/// are refused with `RegistryPolicyNotFoundException`, before a put and after a delete.
#[tokio::test]
async fn reading_or_deleting_a_policy_that_is_not_there_is_refused() {
    let router = emulator();

    for operation in ["GetRegistryPolicy", "DeleteRegistryPolicy"] {
        refused(
            &router,
            "before any put",
            operation,
            &json!({}),
            "RegistryPolicyNotFoundException",
        )
        .await;
    }

    ok(
        &router,
        "the policy to delete",
        "PutRegistryPolicy",
        &json!({"policyText": A_STORED_POLICY}),
    )
    .await;
    let deleted = ok(&router, "the delete", "DeleteRegistryPolicy", &json!({})).await;
    assert_eq!(
        reported_content(&deleted),
        document_content(A_STORED_POLICY),
        "the delete reports the document it removed, got {:?}",
        policy_text(&deleted)
    );

    // The delete removed it: reading and deleting again are refused the same way.
    for operation in ["GetRegistryPolicy", "DeleteRegistryPolicy"] {
        refused(
            &router,
            "after the delete",
            operation,
            &json!({}),
            "RegistryPolicyNotFoundException",
        )
        .await;
    }

    // And a put after the delete stores a policy again, from scratch.
    ok(
        &router,
        "a put after the delete",
        "PutRegistryPolicy",
        &json!({"policyText": A_STORED_POLICY}),
    )
    .await;
    let got = ok(&router, "a put after the delete", "GetRegistryPolicy", &json!({})).await;
    assert_eq!(
        reported_content(&got),
        document_content(A_STORED_POLICY),
        "a put after a delete stores the policy again, got {:?}",
        policy_text(&got)
    );
}

/// One of the documents the concurrent puts put, told apart by the account its statement
/// names.
fn concurrent_policy(index: usize) -> String {
    format!(
        "{{\"Version\":\"2012-10-17\",\"Statement\":[{{\"Sid\":\"Writer{index}\",\
         \"Effect\":\"Allow\",\"Principal\":{{\"AWS\":\"arn:aws:iam::12345678901{index}:root\"}},\
         \"Action\":[\"ecr:ReplicateImage\"],\"Resource\":\"*\"}}]}}"
    )
}

/// Puts driven concurrently all succeed, and the registry is left holding exactly one of
/// the documents they put — never a mix of them, and never nothing.
#[tokio::test]
async fn concurrent_puts_leave_exactly_one_of_the_documents_they_put() {
    let router = emulator();
    let documents: Vec<String> = (0..4).map(concurrent_policy).collect();
    let bodies: Vec<Value> = documents
        .iter()
        .map(|document| json!({"policyText": document}))
        .collect();

    // `join!` polls the four calls together, so they interleave at every await point
    // rather than running one after another.
    let (first, second, third, fourth) = tokio::join!(
        call(&router, "PutRegistryPolicy", &bodies[0]),
        call(&router, "PutRegistryPolicy", &bodies[1]),
        call(&router, "PutRegistryPolicy", &bodies[2]),
        call(&router, "PutRegistryPolicy", &bodies[3]),
    );

    for (index, answer) in [first, second, third, fourth].into_iter().enumerate() {
        assert_eq!(
            answer.status, 200,
            "case {index}: a concurrent put must succeed, got {} with {:?}",
            answer.status, answer.body
        );
        assert_eq!(
            reported_content(&answer.body),
            document_content(&documents[index]),
            "case {index}: each put reports its OWN document, got {:?}",
            policy_text(&answer.body)
        );
    }

    let got = ok(&router, "after the concurrent puts", "GetRegistryPolicy", &json!({})).await;
    let stored = reported_content(&got);
    assert!(
        documents
            .iter()
            .any(|document| stored == document_content(document)),
        "the stored policy must be one of the documents put, got {:?}",
        policy_text(&got)
    );
}
