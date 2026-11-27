// SPDX-License-Identifier: CC0-1.0

//! Glue resource policies: `PutResourcePolicy`, `GetResourcePolicy`,
//! `DeleteResourcePolicy` and `GetResourcePolicies`.
//!
//! The rule under test: a resource policy is ONE record per Glue resource, addressed by
//! `ResourceArn` alone — this account's Data Catalog when the request names none — which a
//! put stores whole, a read reports with the hash the put answered and the instants the
//! attachment was created and last written, and a delete removes. The DOCUMENT a read
//! reports is the one the put carried, re-emitted in the service's own layout rather than
//! echoed back byte for byte, and the `PolicyHash` is an opaque token minted for each write.
//! A resource carrying no policy is a miss for the read and the delete alike (400
//! `EntityNotFoundException`, "Policy not found"), each condition a request carries is
//! checked BEFORE anything is stored, and `GetResourcePolicies` enumerates every policy
//! the account has attached — the account-level catalog policy among them — as a walk whose
//! opening page reports no entry and the token the rest of the walk follows.
//!
//! Each suite is a TABLE whose rows differ along what the rule generalises over: which
//! resource the policy is attached to, which document it carries (size, statement count,
//! ordering, spelling), which condition the request states, and how the page is cut. The
//! recorded case — a catalog policy put, read, conditionally rewritten and deleted — is
//! one row among them.
//!
//! Every call goes over the wire through the shared [`crate::wire`] harness.

use std::collections::BTreeSet;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::wire::{call, emulator, error_code, error_message, ok, ACCOUNT_ID};

/// The ARN of this account's Data Catalog, as served in the Region the harness signs its
/// requests for (`us-east-1`): the resource a `PutResourcePolicy` that names no
/// `ResourceArn` attaches its policy to.
const CATALOG_ARN: &str = "arn:aws:glue:us-east-1:000000000000:catalog";

/// ARNs of individual Glue resources of this account and Region, one per documented
/// resource portion (`database/<name>`, `table/<database>/<name>`, `connection/<name>`,
/// `crawler/<name>`) — among the ARNs whose policies `GetResourcePolicies` enumerates
/// beside the catalog's own.
const DATABASE_ARN: &str = "arn:aws:glue:us-east-1:000000000000:database/sales";
const TABLE_ARN: &str = "arn:aws:glue:us-east-1:000000000000:table/sales/orders";
const CONNECTION_ARN: &str = "arn:aws:glue:us-east-1:000000000000:connection/warehouse";
const CRAWLER_ARN: &str = "arn:aws:glue:us-east-1:000000000000:crawler/nightly";

/// The `Message` a read or a delete of a resource that carries no policy answers with.
/// Recorded wording, which names neither the resource nor the ARN addressed.
const POLICY_NOT_FOUND: &str = "Policy not found";

/// The `Message` a condition failure answers with when the policy is absent or its hash
/// is not the one the request stated. Recorded wording, covering both cases.
const POLICY_CONDITION_FAILED: &str = "Policy not exists or policy hash mismatch";

/// The `Message` a `PolicyExistsCondition` of `NOT_EXIST` earns against a resource that
/// already carries a policy.
const POLICY_ALREADY_EXISTS: &str = "Policy already exists";

/// The published values of the modelled `ExistCondition` enum, which are the only ones
/// `PutResourcePolicyRequest$PolicyExistsCondition` accepts.
///
/// Source: `smithy_query(query="shape", shape="ExistCondition")` on this crate's model,
/// whose `values` are exactly these three.
const PUBLISHED_EXIST_CONDITIONS: [&str; 3] = ["MUST_EXIST", "NONE", "NOT_EXIST"];

/// The published bounds of `GetResourcePoliciesRequest$MaxResults`.
///
/// Source: `smithy_query(query="operation", operation="GetResourcePolicies")`, whose
/// `MaxResults` targets `PageSize` with `range` 1..1000.
const MIN_PAGE_SIZE: i64 = 1;
const MAX_PAGE_SIZE: i64 = 1_000;

/// A policy document granting `actions` on `resource` to this account's root principal —
/// the shape of a Glue resource policy, built per case so two rows differ only in what
/// the rule generalises over.
fn policy_document(actions: &[&str], resource: &str) -> String {
    json!({
        "Version": "2012-10-17",
        "Statement": [{
            "Effect": "Allow",
            "Principal": { "AWS": format!("arn:aws:iam::{ACCOUNT_ID}:root") },
            "Action": actions,
            "Resource": [resource]
        }]
    })
    .to_string()
}

/// The `PutResourcePolicy` request body for `document`, attached to the resource `arn`
/// names (`None` lets the `ResourceArn` default to the Data Catalog).
fn put_request(document: &str, arn: Option<&str>) -> Value {
    let mut request = json!({ "PolicyInJson": document });
    if let Some(resource_arn) = arn {
        request["ResourceArn"] = json!(resource_arn);
    }
    request
}

/// A request body addressing the resource `arn` names, or the Data Catalog when it names
/// none — the body a read and a delete take.
fn arn_request(arn: Option<&str>) -> Value {
    match arn {
        Some(resource_arn) => json!({ "ResourceArn": resource_arn }),
        None => json!({}),
    }
}

/// Attach `document` to the resource `arn` names and answer the `PolicyHash` the put
/// reported.
async fn attach(
    router: &MultiServiceRouter,
    case: &str,
    document: &str,
    arn: Option<&str>,
) -> String {
    let answer = ok(router, "PutResourcePolicy", &put_request(document, arn)).await;
    answer["PolicyHash"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case}: PutResourcePolicy must report a PolicyHash, got {answer}"))
        .to_string()
}

/// The policy the resource `arn` names carries, as `GetResourcePolicy` reports it.
async fn attached(router: &MultiServiceRouter, case: &str, arn: Option<&str>) -> Value {
    let (status, body) = call(router, "GetResourcePolicy", &arn_request(arn)).await;
    assert_eq!(
        status, 200,
        "case {case}: GetResourcePolicy must answer 200 for an attached policy, got {status} \
         with {body}"
    );
    body
}

/// Assert that the resource `arn` names carries no policy: the read and the delete both
/// answer 400 `EntityNotFoundException` with the recorded wording.
async fn assert_no_policy(router: &MultiServiceRouter, case: &str, arn: Option<&str>) {
    for operation in ["GetResourcePolicy", "DeleteResourcePolicy"] {
        let (status, body) = call(router, operation, &arn_request(arn)).await;
        assert_eq!(
            status, 400,
            "case {case} / {operation}: a resource with no policy must be refused with 400, \
             got {status} with {body}"
        );
        assert!(
            error_code(&body).contains("EntityNotFoundException"),
            "case {case} / {operation}: expected EntityNotFoundException, got {body}"
        );
        assert_eq!(
            error_message(&body),
            POLICY_NOT_FOUND,
            "case {case} / {operation}: expected the message {POLICY_NOT_FOUND:?}, got {body}"
        );
    }
}

/// The `GetResourcePoliciesResponseList` of one page, entry by entry as it stands on the
/// wire.
fn listed_entries(page: &Value, case: &str) -> Vec<Value> {
    page["GetResourcePoliciesResponseList"]
        .as_array()
        .unwrap_or_else(|| {
            panic!("case {case}: GetResourcePolicies must report a list, got {page}")
        })
        .clone()
}

/// Walk the whole enumeration the way a paginator does and answer every entry the pages
/// reported, in the order they reported them.
///
/// The walk opens with a request carrying no `NextToken` — which reports no policy and the
/// token to resume from, the recorded shape of this operation's opening page (the suites
/// below assert the ENTRIES against what the store holds, so a handler that reported
/// nothing anywhere would fail them) — and then follows each page's token until a page
/// carries none. `page_size` is the `MaxResults` every request states, or none to let the
/// service choose; `allowed` bounds how many pages a terminating walk may take.
async fn walk_entries(
    router: &MultiServiceRouter,
    case: &str,
    page_size: Option<i64>,
    allowed: usize,
) -> Vec<Value> {
    let mut walked: Vec<Value> = Vec::new();
    let mut token: Option<String> = None;
    let mut pages = 0usize;
    loop {
        let mut request = json!({});
        if let Some(size) = page_size {
            request["MaxResults"] = json!(size);
        }
        if let Some(cursor) = &token {
            request["NextToken"] = json!(cursor);
        }
        let page = ok(router, "GetResourcePolicies", &request).await;
        pages += 1;
        assert!(
            pages <= allowed,
            "case {case}: the walk must terminate within {allowed} pages, got {pages}"
        );
        let entries = listed_entries(&page, case);
        if pages == 1 {
            assert!(
                entries.is_empty(),
                "case {case}: the walk's opening page reports no policy, got {page}"
            );
            assert!(
                page.get("NextToken").and_then(Value::as_str).is_some(),
                "case {case}: the walk's opening page answers the token to resume from, got \
                 {page}"
            );
        }
        if let Some(size) = page_size {
            assert!(
                entries.len() as i64 <= size,
                "case {case}: a page holds at most MaxResults entries, got {page}"
            );
        }
        walked.extend(entries);
        match page.get("NextToken").and_then(Value::as_str) {
            Some(cursor) => token = Some(cursor.to_string()),
            None => break,
        }
    }
    walked
}

/// [`walk_entries`] as the documents the walked entries carry.
async fn walk_documents(
    router: &MultiServiceRouter,
    case: &str,
    page_size: Option<i64>,
    allowed: usize,
) -> Vec<Value> {
    walk_entries(router, case, page_size, allowed)
        .await
        .iter()
        .map(|policy| {
            let text = policy["PolicyInJson"].as_str().unwrap_or_else(|| {
                panic!("case {case}: every listed policy carries its document, got {policy}")
            });
            parsed(text, case)
        })
        .collect()
}

/// `document` as a JSON value, so two spellings of one policy compare equal.
fn parsed(document: &str, case: &str) -> Value {
    serde_json::from_str(document).unwrap_or_else(|error| {
        panic!("case {case}: {document:?} must be a JSON document, got {error}")
    })
}

/// Whether `hash` is shaped the way a `PolicyHash` is: the base64 of 128 bits, i.e. 24
/// characters of the base64 alphabet ending in the two padding characters that 16 bytes
/// leave.
///
/// The SHAPE is what the recorded member pins — the token itself is opaque, minted per write
/// — so this is what a client validating the form of the hash it stores sees.
fn is_policy_hash_shaped(hash: &str) -> bool {
    hash.len() == 24
        && hash.ends_with("==")
        && hash[..22]
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'+' || byte == b'/')
}

/// Assert that `reported` carries the policy `submitted`: the SAME JSON document, member for
/// member and value for value.
///
/// The two are compared as JSON values rather than as text, because what a read reports is
/// the document and not the client's bytes — the service re-emits it in its own layout, which
/// [`a_policy_document_is_reported_in_the_services_own_layout`] pins against literal text.
fn assert_reports_document(reported: &Value, submitted: &str, case: &str) {
    let text = reported["PolicyInJson"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case}: the read reports a PolicyInJson, got {reported}"));
    assert_eq!(
        parsed(text, case),
        parsed(submitted, case),
        "case {case}: the reported document must be the one submitted; expected {submitted}, \
         got {text}"
    );
}

/// A resource that carries no policy is a miss for the read and the delete, whichever
/// resource it is — and a refused read stores nothing, so the next read is the same miss.
#[tokio::test]
async fn a_resource_with_no_policy_is_a_miss_for_the_read_and_the_delete() {
    // (case, the ResourceArn addressed)
    let cases: Vec<(&str, Option<&str>)> = vec![
        ("the Data Catalog, by default", None),
        ("the Data Catalog, named", Some(CATALOG_ARN)),
        ("a database", Some(DATABASE_ARN)),
        ("a table", Some(TABLE_ARN)),
        ("a connection", Some(CONNECTION_ARN)),
        ("a crawler", Some(CRAWLER_ARN)),
    ];

    let router = emulator();
    for (case, arn) in &cases {
        assert_no_policy(&router, case, *arn).await;
        // Reporting, not mutating: four refused calls later the answer is unchanged.
        assert_no_policy(&router, case, *arn).await;
    }

    // Nothing above attached anything, so the whole enumeration reports no policy.
    let walked = walk_documents(&router, "after only refused calls", None, 3).await;
    assert_eq!(
        walked,
        Vec::<Value>::new(),
        "case after only refused calls: expected no policies enumerated, got {walked:?}"
    );
}

/// Every policy document a put carries is reported back as the document it is, with a hash
/// of its own and the two instants of the attachment — and two reads with no write between
/// them answer the same.
#[tokio::test]
async fn every_policy_document_round_trips_with_a_hash_of_its_own() {
    // (case, the document put)
    let cases: Vec<(&str, String)> = vec![
        (
            // The recorded case: one read action granted on the catalog.
            "one action on the catalog",
            policy_document(&["glue:GetDatabase"], CATALOG_ARN),
        ),
        (
            "several actions",
            policy_document(
                &["glue:GetDatabase", "glue:GetTable", "glue:GetPartitions"],
                CATALOG_ARN,
            ),
        ),
        (
            "the same actions in the other order",
            policy_document(
                &["glue:GetPartitions", "glue:GetTable", "glue:GetDatabase"],
                CATALOG_ARN,
            ),
        ),
        (
            "a document naming a different resource",
            policy_document(&["glue:GetDatabase"], DATABASE_ARN),
        ),
        (
            "two statements",
            json!({
                "Version": "2012-10-17",
                "Statement": [
                    {
                        "Sid": "Reads",
                        "Effect": "Allow",
                        "Principal": { "AWS": format!("arn:aws:iam::{ACCOUNT_ID}:root") },
                        "Action": ["glue:GetTable"],
                        "Resource": [TABLE_ARN]
                    },
                    {
                        "Sid": "Denies",
                        "Effect": "Deny",
                        "Principal": { "AWS": "*" },
                        "Action": ["glue:DeleteTable"],
                        "Resource": [TABLE_ARN]
                    }
                ]
            })
            .to_string(),
        ),
        (
            // Spelling, not meaning: a document with its own whitespace is stored as
            // sent rather than re-serialised.
            "a document carrying its own whitespace",
            format!(
                "{{\n  \"Version\": \"2012-10-17\",\n  \"Statement\": [\n    {{\"Effect\": \
                 \"Allow\", \"Principal\": {{\"AWS\": \"arn:aws:iam::{ACCOUNT_ID}:root\"}}, \
                 \"Action\": [\"glue:GetTables\"], \"Resource\": [\"{CATALOG_ARN}\"]}}\n  ]\n}}"
            ),
        ),
        (
            "a statement list with nothing in it",
            json!({ "Version": "2012-10-17", "Statement": [] }).to_string(),
        ),
        (
            "a long document",
            policy_document(
                &[
                    "glue:GetDatabase",
                    "glue:GetDatabases",
                    "glue:GetTable",
                    "glue:GetTables",
                    "glue:GetPartition",
                    "glue:GetPartitions",
                    "glue:GetUserDefinedFunction",
                    "glue:GetUserDefinedFunctions",
                    "glue:GetConnection",
                    "glue:GetConnections",
                ],
                CATALOG_ARN,
            ),
        ),
    ];

    let mut hashes: Vec<(String, String)> = Vec::new();
    for (case, document) in &cases {
        let router = emulator();
        let put_hash = attach(&router, case, document, None).await;
        assert!(
            !put_hash.is_empty() && put_hash.chars().count() <= 255,
            "case {case}: a PolicyHash is a HashString of 1..255 characters, got {put_hash:?}"
        );
        assert!(
            is_policy_hash_shaped(&put_hash),
            "case {case}: a PolicyHash is the recorded shape — 128 bits, base64 — got \
             {put_hash:?}"
        );

        let reported = attached(&router, case, None).await;
        assert_reports_document(&reported, document, case);
        assert_eq!(
            reported["PolicyHash"],
            json!(put_hash),
            "case {case}: the read reports the hash the put answered ({put_hash}), got {reported}"
        );
        let create_time = reported["CreateTime"].as_f64();
        let update_time = reported["UpdateTime"].as_f64();
        assert!(
            create_time.is_some_and(|at| at > 0.0),
            "case {case}: expected a populated CreateTime, got {reported}"
        );
        assert_eq!(
            update_time, create_time,
            "case {case}: an attachment nothing has rewritten reports UpdateTime equal to \
             CreateTime, got {reported}"
        );

        // Stored, not computed per call: a second read answers the same record.
        let again = attached(&router, case, None).await;
        assert_eq!(
            again, reported,
            "case {case}: two reads of one attachment must agree; got {reported} then {again}"
        );

        hashes.push((case.to_string(), put_hash));
    }

    // The hash identifies the WRITE that attached the policy, so no two of these
    // attachments — each in a store of its own — may share one.
    let distinct: BTreeSet<&String> = hashes.iter().map(|(_, hash)| hash).collect();
    assert_eq!(
        distinct.len(),
        hashes.len(),
        "two attachments must not share a PolicyHash, got {hashes:?}"
    );

    // ...and a second write of the SAME document mints another one, because what the hash
    // tells a client is whether the policy has been written since it last read it.
    let (case, document) = &cases[0];
    let elsewhere = emulator();
    let first = attach(&elsewhere, case, document, Some(TABLE_ARN)).await;
    let again = attach(&elsewhere, case, document, Some(TABLE_ARN)).await;
    assert_ne!(
        again, first,
        "case {case}: a second write of one document must mint a new PolicyHash, got {first} \
         twice"
    );
    let reported = ok(
        &elsewhere,
        "GetResourcePolicy",
        &arn_request(Some(TABLE_ARN)),
    )
    .await;
    assert_eq!(
        reported["PolicyHash"],
        json!(again),
        "case {case}: the read reports the hash the last write answered, got {reported}"
    );
}

/// A second put replaces the document whole, keeps the attachment's `CreateTime` and mints
/// a new `PolicyHash` for the write.
#[tokio::test]
async fn a_second_put_replaces_the_document_and_keeps_the_create_time() {
    // (case, the first document, the second)
    let cases: Vec<(&str, String, String)> = vec![
        (
            "a narrower grant",
            policy_document(&["glue:GetDatabase", "glue:GetTable"], CATALOG_ARN),
            policy_document(&["glue:GetDatabase"], CATALOG_ARN),
        ),
        (
            "a wider grant",
            policy_document(&["glue:GetDatabase"], CATALOG_ARN),
            policy_document(&["glue:GetDatabase", "glue:GetPartitions"], CATALOG_ARN),
        ),
        (
            "an emptied statement list",
            policy_document(&["glue:GetDatabase"], CATALOG_ARN),
            json!({ "Version": "2012-10-17", "Statement": [] }).to_string(),
        ),
        (
            "the very same document again",
            policy_document(&["glue:GetTable"], TABLE_ARN),
            policy_document(&["glue:GetTable"], TABLE_ARN),
        ),
    ];

    for (case, first, second) in &cases {
        let router = emulator();
        let first_hash = attach(&router, case, first, None).await;
        let before = attached(&router, case, None).await;

        let second_hash = attach(&router, case, second, None).await;
        let after = attached(&router, case, None).await;

        assert_reports_document(&after, second, case);
        assert_eq!(
            after["PolicyHash"],
            json!(second_hash),
            "case {case}: the read reports the hash the second put answered, got {after}"
        );
        assert_ne!(
            second_hash, first_hash,
            "case {case}: every write mints its own hash; got {first_hash} twice"
        );
        assert_eq!(
            after["CreateTime"], before["CreateTime"],
            "case {case}: a rewrite does not move CreateTime, got {:?} then {:?}",
            before["CreateTime"], after["CreateTime"]
        );
        let (created, updated) = (after["CreateTime"].as_f64(), after["UpdateTime"].as_f64());
        assert!(
            updated >= created && updated.is_some(),
            "case {case}: a rewritten attachment reports UpdateTime no earlier than \
             CreateTime, got {after}"
        );
    }
}

/// The `ResourceArn` a request names and the one it lets default address the SAME record
/// when that ARN is the Data Catalog's own.
#[tokio::test]
async fn the_default_resource_arn_and_the_catalog_arn_name_one_record() {
    // (case, the ARN the put names, the ARN the read names)
    let cases: Vec<(&str, Option<&str>, Option<&str>)> = vec![
        ("put defaulted, read defaulted", None, None),
        ("put defaulted, read named", None, Some(CATALOG_ARN)),
        ("put named, read defaulted", Some(CATALOG_ARN), None),
        ("put named, read named", Some(CATALOG_ARN), Some(CATALOG_ARN)),
    ];

    for (case, put_arn, read_arn) in &cases {
        let router = emulator();
        let document = policy_document(&["glue:GetDatabase"], CATALOG_ARN);
        let hash = attach(&router, case, &document, *put_arn).await;

        let reported = attached(&router, case, *read_arn).await;
        assert_reports_document(&reported, &document, case);
        assert_eq!(
            reported["PolicyHash"],
            json!(hash),
            "case {case}: both spellings report one hash, got {reported}"
        );

        // And the delete under the other spelling removes that one record.
        ok(
            &router,
            "DeleteResourcePolicy",
            &arn_request(*read_arn),
        )
        .await;
        assert_no_policy(&router, case, *put_arn).await;
    }
}

/// Each resource keeps its OWN policy: a put on one leaves the others alone, and so does
/// a delete.
#[tokio::test]
async fn each_resource_keeps_its_own_policy() {
    let router = emulator();
    // (case, the ARN addressed, the document attached to it)
    let attachments: Vec<(&str, Option<&str>, String)> = vec![
        (
            "the catalog",
            None,
            policy_document(&["glue:GetDatabase"], CATALOG_ARN),
        ),
        (
            "a database",
            Some(DATABASE_ARN),
            policy_document(&["glue:GetTables"], DATABASE_ARN),
        ),
        (
            "a table",
            Some(TABLE_ARN),
            policy_document(&["glue:GetTable"], TABLE_ARN),
        ),
        (
            "a connection",
            Some(CONNECTION_ARN),
            policy_document(&["glue:GetConnection"], CONNECTION_ARN),
        ),
    ];

    for (case, arn, document) in &attachments {
        attach(&router, case, document, *arn).await;
    }
    for (case, arn, document) in &attachments {
        let reported = attached(&router, case, *arn).await;
        assert_reports_document(&reported, document, case);
    }

    // Detaching one leaves every other attachment readable.
    ok(
        &router,
        "DeleteResourcePolicy",
        &arn_request(Some(DATABASE_ARN)),
    )
    .await;
    assert_no_policy(&router, "a detached database", Some(DATABASE_ARN)).await;
    for (case, arn, document) in &attachments {
        if *arn == Some(DATABASE_ARN) {
            continue;
        }
        let reported = attached(&router, case, *arn).await;
        assert_reports_document(&reported, document, case);
    }
}

/// `PolicyExistsCondition` decides whether the put is served, and a put it refuses
/// stores nothing.
#[tokio::test]
async fn the_exists_condition_decides_whether_the_put_is_served() {
    let attached_document = policy_document(&["glue:GetDatabase"], CATALOG_ARN);
    let new_document = policy_document(&["glue:GetTables"], CATALOG_ARN);

    // (case, whether a policy is attached first, the condition, the refusal message the
    // put earns — `None` when it is served)
    let cases: Vec<(&str, bool, Option<&str>, Option<&str>)> = vec![
        ("no policy, MUST_EXIST", false, Some("MUST_EXIST"), Some(POLICY_CONDITION_FAILED)),
        ("no policy, NOT_EXIST", false, Some("NOT_EXIST"), None),
        ("no policy, NONE", false, Some("NONE"), None),
        ("no policy, no condition", false, None, None),
        ("a policy, MUST_EXIST", true, Some("MUST_EXIST"), None),
        ("a policy, NOT_EXIST", true, Some("NOT_EXIST"), Some(POLICY_ALREADY_EXISTS)),
        ("a policy, NONE", true, Some("NONE"), None),
        ("a policy, no condition", true, None, None),
    ];

    for (case, attach_first, condition, refusal) in &cases {
        let router = emulator();
        if *attach_first {
            attach(&router, case, &attached_document, None).await;
        }

        let mut request = put_request(&new_document, None);
        if let Some(condition) = condition {
            request["PolicyExistsCondition"] = json!(condition);
        }
        let (status, body) = call(&router, "PutResourcePolicy", &request).await;

        match refusal {
            Some(message) => {
                assert_eq!(
                    status, 400,
                    "case {case}: an unsatisfied condition must be refused with 400, got \
                     {status} with {body}"
                );
                assert!(
                    error_code(&body).contains("ConditionCheckFailureException"),
                    "case {case}: expected ConditionCheckFailureException, got {body}"
                );
                assert_eq!(
                    error_message(&body),
                    *message,
                    "case {case}: expected the message {message:?}, got {body}"
                );
                // The refused put stored nothing: the resource carries what it did
                // before, which for an empty store is no policy at all.
                if *attach_first {
                    let reported = attached(&router, case, None).await;
                    assert_reports_document(&reported, &attached_document, case);
                } else {
                    assert_no_policy(&router, case, None).await;
                }
            }
            None => {
                assert_eq!(
                    status, 200,
                    "case {case}: a satisfied condition must be served, got {status} with {body}"
                );
                let reported = attached(&router, case, None).await;
                assert_reports_document(&reported, &new_document, case);
            }
        }
    }

    // Every published condition is accepted by the request layer (each appears in a row
    // above), and a value outside the published list is refused with a 400 that stores
    // nothing.
    for unpublished in ["MUST_NOT_EXIST", "must_exist", "EXISTS", ""] {
        assert!(
            !PUBLISHED_EXIST_CONDITIONS.contains(&unpublished),
            "case {unpublished:?}: this row must be outside the published list \
             {PUBLISHED_EXIST_CONDITIONS:?}"
        );
        let router = emulator();
        let mut request = put_request(&new_document, None);
        request["PolicyExistsCondition"] = json!(unpublished);
        let (status, body) = call(&router, "PutResourcePolicy", &request).await;
        assert_eq!(
            status, 400,
            "case {unpublished:?}: a condition outside the published list must be refused \
             with 400, got {status} with {body}"
        );
        assert_no_policy(&router, unpublished, None).await;
    }
}

/// `PolicyHashCondition` admits only the attached policy's own hash — on the put and on
/// the delete alike — and a refused call leaves the attachment exactly as it was.
#[tokio::test]
async fn the_hash_condition_admits_only_the_attached_policys_hash() {
    let first_document = policy_document(&["glue:GetDatabase"], CATALOG_ARN);
    let second_document = policy_document(&["glue:GetTables"], CATALOG_ARN);
    let third_document = policy_document(&["glue:GetPartitions"], CATALOG_ARN);

    // A hash that is the attached policy's lets the put through, and the hash moves on
    // with the document.
    let router = emulator();
    let first_hash = attach(&router, "a fresh attachment", &first_document, None).await;
    let mut request = put_request(&second_document, None);
    request["PolicyHashCondition"] = json!(first_hash);
    let answer = ok(&router, "PutResourcePolicy", &request).await;
    let second_hash = answer["PolicyHash"]
        .as_str()
        .unwrap_or_else(|| panic!("the served put reports a hash, got {answer}"))
        .to_string();
    assert_ne!(
        second_hash, first_hash,
        "a rewritten document gets a new hash, got {first_hash} twice"
    );

    // (case, the hash the request states) — every row is a hash that is NOT the attached
    // policy's, so each must fail the condition.
    let stale_cases: Vec<(&str, String)> = vec![
        ("the hash of the replaced document", first_hash.clone()),
        (
            "the hash of a document never attached",
            "0".repeat(64),
        ),
        ("a single character", "x".to_string()),
    ];

    for (case, hash_condition) in &stale_cases {
        let mut request = put_request(&third_document, None);
        request["PolicyHashCondition"] = json!(hash_condition);
        let (status, body) = call(&router, "PutResourcePolicy", &request).await;
        assert_eq!(
            status, 400,
            "case {case}: a stale PolicyHashCondition must be refused with 400, got {status} \
             with {body}"
        );
        assert!(
            error_code(&body).contains("ConditionCheckFailureException"),
            "case {case}: expected ConditionCheckFailureException, got {body}"
        );
        assert_eq!(
            error_message(&body),
            POLICY_CONDITION_FAILED,
            "case {case}: expected the message {POLICY_CONDITION_FAILED:?}, got {body}"
        );

        // The delete states the same condition and earns the same refusal.
        let mut delete = arn_request(None);
        delete["PolicyHashCondition"] = json!(hash_condition);
        let (status, body) = call(&router, "DeleteResourcePolicy", &delete).await;
        assert_eq!(
            status, 400,
            "case {case}: a stale PolicyHashCondition must stop the delete too, got {status} \
             with {body}"
        );
        assert_eq!(
            error_message(&body),
            POLICY_CONDITION_FAILED,
            "case {case}: expected the message {POLICY_CONDITION_FAILED:?}, got {body}"
        );

        // Neither refused call touched the attachment.
        let reported = attached(&router, case, None).await;
        assert_reports_document(&reported, &second_document, case);
        assert_eq!(
            reported["PolicyHash"],
            json!(second_hash),
            "case {case}: a refused call must leave the hash alone, got {reported}"
        );
    }

    // A hash stated against a resource that carries no policy fails the same way.
    let mut request = put_request(&first_document, Some(TABLE_ARN));
    request["PolicyHashCondition"] = json!(second_hash);
    let (status, body) = call(&router, "PutResourcePolicy", &request).await;
    assert_eq!(
        status, 400,
        "case a hash for an unattached resource: expected 400, got {status} with {body}"
    );
    assert_eq!(
        error_message(&body),
        POLICY_CONDITION_FAILED,
        "case a hash for an unattached resource: expected the message \
         {POLICY_CONDITION_FAILED:?}, got {body}"
    );
    assert_no_policy(&router, "a hash for an unattached resource", Some(TABLE_ARN)).await;

    // The attached policy's own hash lets the delete through, and the record is gone.
    let mut delete = arn_request(None);
    delete["PolicyHashCondition"] = json!(second_hash);
    let answer = ok(&router, "DeleteResourcePolicy", &delete).await;
    assert_eq!(
        answer,
        json!({}),
        "case the matching hash: DeleteResourcePolicy reports no members, got {answer}"
    );
    assert_no_policy(&router, "the matching hash", None).await;
}

/// `GetResourcePolicies` enumerates EVERY policy this account has attached, in ARN order —
/// the account-level Data Catalog policy among them, which stays readable through
/// `GetResourcePolicy` as well — as a walk a client follows from the opening page's token.
#[tokio::test]
async fn get_resource_policies_lists_every_attached_policy_including_the_catalogs() {
    let router = emulator();
    let catalog_document = policy_document(&["glue:GetDatabases"], CATALOG_ARN);
    let catalog_hash = attach(&router, "the catalog policy alone", &catalog_document, None).await;

    // With only the catalog's own policy attached, that policy is the whole enumeration:
    // one entry carrying the document that was put, the hash the put answered and both
    // instants of the attachment.
    let walked = walk_entries(&router, "the catalog policy alone", None, 3).await;
    assert_eq!(
        walked.len(),
        1,
        "case the catalog policy alone: the account-level catalog policy is one entry of the \
         enumeration, got {walked:?}"
    );
    let entry = &walked[0];
    assert_eq!(
        parsed(
            entry["PolicyInJson"]
                .as_str()
                .unwrap_or_else(|| panic!("case the catalog policy alone: {entry} carries a \
                                           PolicyInJson")),
            "the catalog policy alone"
        ),
        parsed(&catalog_document, "the catalog policy alone"),
        "case the catalog policy alone: the listed entry carries the document that was put, \
         got {entry}"
    );
    assert_eq!(
        entry["PolicyHash"],
        json!(catalog_hash),
        "case the catalog policy alone: the listed entry carries the hash the put answered, \
         got {entry}"
    );
    assert!(
        entry["CreateTime"].as_f64().is_some() && entry["UpdateTime"].as_f64().is_some(),
        "case the catalog policy alone: the listed entry carries both instants of the \
         attachment, got {entry}"
    );
    let catalog = attached(&router, "the catalog policy alone", None).await;
    assert_reports_document(&catalog, &catalog_document, "the catalog policy alone");

    // Attach one policy per individual resource too. The expected order is the ARNs' own,
    // with the catalog's among them rather than apart from them.
    let mut attachments: Vec<(&str, String, String)> = vec![
        (CATALOG_ARN, catalog_document.clone(), catalog_hash.clone()),
        (
            TABLE_ARN,
            policy_document(&["glue:GetTable"], TABLE_ARN),
            String::new(),
        ),
        (
            DATABASE_ARN,
            policy_document(&["glue:GetTables"], DATABASE_ARN),
            String::new(),
        ),
        (
            CONNECTION_ARN,
            policy_document(&["glue:GetConnection"], CONNECTION_ARN),
            String::new(),
        ),
        (
            CRAWLER_ARN,
            policy_document(&["glue:GetCrawler"], CRAWLER_ARN),
            String::new(),
        ),
    ];
    for (arn, document, hash) in &mut attachments {
        if *arn != CATALOG_ARN {
            *hash = attach(&router, arn, document, Some(arn)).await;
        }
    }
    attachments.sort_by(|left, right| left.0.cmp(right.0));
    let expected: Vec<Value> = attachments
        .iter()
        .map(|(arn, document, _)| parsed(document, arn))
        .collect();
    let expected_hashes: Vec<Value> = attachments
        .iter()
        .map(|(_, _, hash)| json!(hash))
        .collect();

    // One page per entry, plus the opening page and the one that reports the tail of the
    // set: a walk that asks for more than that is not terminating.
    let allowed = expected.len() + 2;
    let entries = walk_entries(&router, "every attached policy", None, allowed).await;
    let walked: Vec<Value> = entries
        .iter()
        .map(|entry| {
            parsed(
                entry["PolicyInJson"].as_str().unwrap_or_else(|| {
                    panic!("case every attached policy: {entry} carries a PolicyInJson")
                }),
                "every attached policy",
            )
        })
        .collect();
    assert_eq!(
        walked,
        expected,
        "case every attached policy: expected the {} documents in ARN order, got {walked:?}",
        expected.len()
    );
    assert!(
        walked.contains(&parsed(&catalog_document, "the catalog policy")),
        "case every attached policy: the catalog's own policy is enumerated too, got {walked:?}"
    );
    // Each entry is the record its own put wrote: the hash that put answered, and both
    // instants of the attachment.
    let walked_hashes: Vec<Value> = entries
        .iter()
        .map(|entry| entry["PolicyHash"].clone())
        .collect();
    assert_eq!(
        walked_hashes, expected_hashes,
        "case every attached policy: every entry must carry the hash its own put answered, \
         got {walked_hashes:?}"
    );
    for entry in &entries {
        assert!(
            entry["CreateTime"].as_f64().is_some() && entry["UpdateTime"].as_f64().is_some(),
            "case every attached policy: every entry carries both instants of its \
             attachment, got {entry}"
        );
    }
    let catalog = attached(&router, "every attached policy", None).await;
    assert_reports_document(&catalog, &catalog_document, "every attached policy");

    // Cut into pages of every size, one entry at a time included: the pages concatenate to
    // the whole set, in the same order, and the walk terminates — the page reached past the
    // last entry carries no token of its own.
    for page_size in [1_i64, 2, 3, MAX_PAGE_SIZE] {
        let case = format!("MaxResults {page_size}");
        let walked = walk_documents(&router, &case, Some(page_size), allowed).await;
        assert_eq!(
            walked, expected,
            "case {case}: the pages must concatenate to the whole set in ARN order, got \
             {walked:?}"
        );
    }

    // A token this store did not mint names no position and is refused rather than
    // resolved against an ARN that happens to sort somewhere.
    for forged in ["arn:aws:glue:us-east-1:000000000000:crawler/nightly", "zz"] {
        let (status, body) = call(
            &router,
            "GetResourcePolicies",
            &json!({ "NextToken": forged }),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {forged:?}: a continuation token this store did not mint must be refused \
             with 400, got {status} with {body}"
        );
    }

    // The published bounds are served; one step past either is refused.
    for page_size in [MIN_PAGE_SIZE, MAX_PAGE_SIZE] {
        let (status, body) = call(
            &router,
            "GetResourcePolicies",
            &json!({ "MaxResults": page_size }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case MaxResults {page_size}: a published bound must be served, got {status} with \
             {body}"
        );
    }
    for page_size in [MIN_PAGE_SIZE - 1, MAX_PAGE_SIZE + 1] {
        let (status, body) = call(
            &router,
            "GetResourcePolicies",
            &json!({ "MaxResults": page_size }),
        )
        .await;
        assert_eq!(
            status, 400,
            "case MaxResults {page_size}: a value outside the published range must be refused \
             with 400, got {status} with {body}"
        );
    }

    // A detached policy leaves the enumeration, the catalog's own included.
    for detached in [Some(TABLE_ARN), None] {
        ok(&router, "DeleteResourcePolicy", &arn_request(detached)).await;
    }
    let walked = walk_documents(&router, "after a detach", None, allowed).await;
    assert_eq!(
        walked.len(),
        expected.len() - 2,
        "case after a detach: a detached policy must leave the enumeration, got {walked:?}"
    );
    assert!(
        !walked.contains(&parsed(&catalog_document, "the catalog policy")),
        "case after a detach: the detached catalog policy must leave the enumeration, got \
         {walked:?}"
    );
}

/// A `ResourceArn` naming an estate this endpoint does not serve is refused by every
/// resource-policy operation, with 400 `InvalidInputException` naming the ARN — and the
/// refused write stores nothing.
#[tokio::test]
async fn a_resource_arn_of_another_estate_is_refused_by_every_operation() {
    // Glue ARNs that are not of this account and Region: another account, another
    // Region, and each of those spelled as a catalog.
    let foreign_arns = [
        "arn:aws:glue:us-east-1:210987654321:database/sales",
        "arn:aws:glue:eu-west-1:000000000000:database/sales",
        "arn:aws:glue:us-east-1:210987654321:catalog",
        "arn:aws:glue:ap-southeast-2:000000000000:catalog",
    ];

    for foreign in foreign_arns {
        let router = emulator();
        let own_document = policy_document(&["glue:GetDatabase"], CATALOG_ARN);
        attach(&router, foreign, &own_document, None).await;

        let requests: Vec<(&str, Value)> = vec![
            ("GetResourcePolicy", arn_request(Some(foreign))),
            (
                "PutResourcePolicy",
                put_request(&policy_document(&["glue:GetTable"], foreign), Some(foreign)),
            ),
            ("DeleteResourcePolicy", arn_request(Some(foreign))),
        ];
        for (operation, request) in &requests {
            let (status, body) = call(&router, operation, request).await;
            assert_eq!(
                status, 400,
                "case {foreign:?} / {operation}: expected 400, got {status} with {body}"
            );
            assert!(
                error_code(&body).contains("InvalidInputException"),
                "case {foreign:?} / {operation}: expected InvalidInputException, got {body}"
            );
            let message = error_message(&body);
            assert!(
                message.contains(foreign),
                "case {foreign:?} / {operation}: the refusal is built from the request, so it \
                 must name the ResourceArn it refused, got {message:?}"
            );
        }

        // The refused calls left this account's own attachment alone.
        let reported = attached(&router, foreign, None).await;
        assert_reports_document(&reported, &own_document, foreign);
    }

    // A value that is no Glue ARN at all is refused before the handler, by the member's
    // own modelled pattern (`^arn:aws(-(cn|us-gov|iso(-[bef])?))?:glue:`).
    for malformed in [
        "arn:aws:s3:::reports",
        "arn:aws:glue",
        "sales",
        "arn:aws:glue:us-east-1:000000000000",
    ] {
        let router = emulator();
        let (status, body) = call(
            &router,
            "GetResourcePolicy",
            &arn_request(Some(malformed)),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {malformed:?}: a value that is no Glue resource ARN must be refused with \
             400, got {status} with {body}"
        );
    }
}

/// `PolicyInJson` is "the policy document to set, in JSON format": a value that is no
/// JSON document is refused, and nothing is stored.
#[tokio::test]
async fn a_policy_document_that_is_not_a_json_object_is_refused() {
    // (case, the value sent as PolicyInJson) — each is at least the modelled minimum of
    // two characters, so the refusal is this operation's own rather than the member's
    // length constraint.
    let cases = [
        ("a JSON list", "[]"),
        ("a JSON string", "\"policy\""),
        ("a JSON number", "42"),
        ("a JSON null", "null"),
        ("a truncated object", "{\"Version\":"),
        ("prose", "allow everything"),
    ];

    for (case, document) in cases {
        let router = emulator();
        let (status, body) = call(
            &router,
            "PutResourcePolicy",
            &put_request(document, None),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case}: {document:?} is no policy document, so it must be refused with 400, \
             got {status} with {body}"
        );
        assert!(
            error_code(&body).contains("InvalidInputException"),
            "case {case}: expected InvalidInputException, got {body}"
        );
        assert_no_policy(&router, case, None).await;
    }

    // One step the other way: a JSON object IS a document, so it is stored.
    let router = emulator();
    let document = json!({ "Version": "2012-10-17", "Statement": [] }).to_string();
    attach(&router, "an object", &document, None).await;
    let reported = attached(&router, "an object", None).await;
    assert_reports_document(&reported, &document, "an object");
}

/// A `PolicyInJson` that is not JSON at all is refused with the service's parser's own
/// complaint: the word it stopped on, quoted, what a value may begin with, and the position
/// that word starts at in the submitted document.
#[tokio::test]
async fn a_policy_that_is_not_json_is_refused_with_the_parsers_complaint() {
    // (case, the value sent as PolicyInJson, the word named, the line and column reported).
    // The recorded case is the first: a document that begins with a bare word.
    let cases = [
        ("a bare word", "not a policy".to_string(), "not", 1, 1),
        ("a single word", "nope".to_string(), "nope", 1, 1),
        (
            "a word where a member's value belongs",
            "{\"Version\": allow}".to_string(),
            "allow",
            1,
            13,
        ),
        (
            "a word on the third line",
            "{\n  \"Version\": \"2012-10-17\",\n  \"Statement\": nothing\n}".to_string(),
            "nothing",
            3,
            16,
        ),
        (
            "a word after a long prefix of whitespace",
            format!("{}maybe", " ".repeat(40)),
            "maybe",
            1,
            41,
        ),
    ];

    for (case, document, token, line, column) in cases {
        let router = emulator();
        let (status, body) = call(&router, "PutResourcePolicy", &put_request(&document, None)).await;
        assert_eq!(
            status, 400,
            "case {case}: {document:?} is no policy document, so it must be refused with 400, \
             got {status} with {body}"
        );
        assert!(
            error_code(&body).contains("InvalidInputException"),
            "case {case}: expected InvalidInputException, got {body}"
        );
        let expected = format!(
            "Invalid input policy: Unexpected IOException: Unrecognized token '{token}': was \
             expecting (JSON String, Number, Array, Object or token 'null', 'true' or \
             'false')\n at [Source: REDACTED \
             (`StreamReadFeature.INCLUDE_SOURCE_IN_LOCATION` disabled); line: {line}, \
             column: {column}]"
        );
        assert_eq!(
            error_message(&body),
            expected,
            "case {case}: unexpected wording for a document that is not JSON"
        );
        assert_no_policy(&router, case, None).await;
    }
}

/// The document a read reports is the one the write carried, re-emitted in the service's
/// OWN layout: one member per line, two spaces of indent per nesting level, `" : "` between
/// a member's name and its value, and a list on one line between single spaces.
///
/// The rows differ along what the layout generalises over: nesting depth, a member's value
/// type, an empty object and an empty list, a list of scalars and a list of objects — and
/// the document a client already submits in that layout, which is reported unchanged.
#[tokio::test]
async fn a_policy_document_is_reported_in_the_services_own_layout() {
    // (case, the document submitted, the text every read of it reports)
    let cases: Vec<(&str, String, String)> = vec![
        (
            "an object with no member",
            "{}".to_string(),
            "{ }".to_string(),
        ),
        (
            "one member",
            r#"{"Version":"2012-10-17"}"#.to_string(),
            "{\n  \"Version\" : \"2012-10-17\"\n}".to_string(),
        ),
        (
            "an empty statement list",
            r#"{"Version":"2012-10-17","Statement":[]}"#.to_string(),
            "{\n  \"Version\" : \"2012-10-17\",\n  \"Statement\" : [ ]\n}".to_string(),
        ),
        (
            "a list of scalars",
            r#"{"Action":["glue:GetTable","glue:GetDatabase"]}"#.to_string(),
            "{\n  \"Action\" : [ \"glue:GetTable\", \"glue:GetDatabase\" ]\n}".to_string(),
        ),
        (
            // The recorded shape of a Glue resource policy: one statement, nested objects
            // and lists inside it.
            "a statement with a principal and a list of actions",
            policy_document(&["glue:GetDatabase"], CATALOG_ARN),
            [
                "{",
                "  \"Version\" : \"2012-10-17\",",
                "  \"Statement\" : [ {",
                "    \"Effect\" : \"Allow\",",
                "    \"Principal\" : {",
                &format!("      \"AWS\" : \"arn:aws:iam::{ACCOUNT_ID}:root\""),
                "    },",
                "    \"Action\" : [ \"glue:GetDatabase\" ],",
                &format!("    \"Resource\" : [ \"{CATALOG_ARN}\" ]"),
                "  } ]",
                "}",
            ]
            .join("\n"),
        ),
        (
            "values that are not strings",
            r#"{"Version":"2012-10-17","Count":2,"Enabled":true,"Unset":null}"#.to_string(),
            [
                "{",
                "  \"Version\" : \"2012-10-17\",",
                "  \"Count\" : 2,",
                "  \"Enabled\" : true,",
                "  \"Unset\" : null",
                "}",
            ]
            .join("\n"),
        ),
        (
            // A document already in that layout is reported as it stands, so a client that
            // reads a policy and writes it back sees no difference.
            "a document already in the service's layout",
            "{\n  \"Version\" : \"2012-10-17\",\n  \"Statement\" : [ ]\n}".to_string(),
            "{\n  \"Version\" : \"2012-10-17\",\n  \"Statement\" : [ ]\n}".to_string(),
        ),
    ];

    for (case, document, reported_text) in &cases {
        let router = emulator();
        attach(&router, case, document, None).await;

        let reported = attached(&router, case, None).await;
        assert_eq!(
            reported["PolicyInJson"],
            json!(reported_text),
            "case {case}: the read must report the service's own layout of {document}"
        );

        // The listing reports the same text for the same attachment, so no reader renders
        // a document of its own.
        attach(&router, case, document, Some(DATABASE_ARN)).await;
        let walked = walk_entries(&router, case, None, 4).await;
        let listed = walked
            .first()
            .map(|entry| entry["PolicyInJson"].clone())
            .unwrap_or_else(|| {
                panic!("case {case}: the enumeration must report the attached policies")
            });
        assert_eq!(
            listed,
            json!(reported_text),
            "case {case}: the listing must report the same text as the read, got {walked:?}"
        );
    }
}
