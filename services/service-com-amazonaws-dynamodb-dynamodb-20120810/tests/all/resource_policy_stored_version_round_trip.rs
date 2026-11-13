// SPDX-License-Identifier: CC0-1.0

//! The policy-language VERSION a stored resource-based policy document states, on every
//! resource a policy attaches to, and the stored document as a FIXED POINT of the attach.
//!
//! RULE pinned here: **the resource-based policy document a resource carries states the
//! policy-language version it is read under — "if you do not include a Version element, the
//! value defaults to 2008-10-17" (the IAM policy grammar reference), so a request that
//! states none leaves the stored document stating that default, and one that states a
//! version keeps the version it stated — for every resource such a policy attaches to
//! ("the resource, which can be a table or stream", the `PutResourcePolicy` doc). Every
//! other element the request stated is stored as it arrived, which makes the stored
//! document a fixed point of the attach path: re-attaching the document a read reported
//! stores exactly that document and reports the revision it already carried.**
//!
//! The rows differ along what the rule generalises over: WHICH resource carries the policy
//! (a table and that table's own stream, each with the actions that resource has), whether
//! the document states a version and which of the two the grammar defines it states, where
//! in the document it wrote it, and the shape of the document around it (one statement, two
//! statements under an `Id`, a statement carrying a `Sid` and a `Condition`, an `Action`
//! listing two actions). The version catalogue is the grammar reference's own.
//!
//! The round trip is the client workflow the rule matters for — read the stored document
//! back and put it again, as a drift check or a policy editor does — so each row asserts
//! the second put is the idempotent re-attach the operation documents rather than a new
//! revision, which holds only because the version is decided the same way every time.
//!
//! Driven over the wire (`POST /` with `X-Amz-Target: DynamoDB_20120810.<Operation>`)
//! through the same `MultiServiceRouter` the emulator's front doors use.

use std::time::{Duration, Instant};

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::emulator::{call, describe_when_active, emulator, ok};

/// The versions the IAM policy grammar defines: the one a document stating none is read
/// under, and the current one.
const DEFAULT_VERSION: &str = "2008-10-17";
const CURRENT_VERSION: &str = "2012-10-17";

/// How long a row waits for the policy it attached to become readable, and how often it
/// looks.
///
/// "`PutResourcePolicy` is an asynchronous operation", so a read made straight after the
/// write may still report the revision the resource carried before it. The wait is on the
/// REVISION the write reported rather than on a span, and the deadline is far longer than
/// the lag so a loaded host cannot turn the wait into a failure.
const POLICY_READ_DEADLINE: Duration = Duration::from_secs(60);
const POLICY_READ_INTERVAL: Duration = Duration::from_millis(50);

/// One statement granting `action` on `resource` to the account's own root principal — a
/// grant, so no document here takes this caller's policy-management access away and none
/// needs `ConfirmRemoveSelfResourceAccess`.
///
/// `account` is read off the resource's own ARN, so nothing of the emulator's account id is
/// written here.
fn allow(account: &str, action: Value, resource: &str) -> Value {
    json!({
        "Effect": "Allow",
        "Principal": {"AWS": format!("arn:aws:iam::{account}:root")},
        "Action": action,
        "Resource": resource,
    })
}

/// One row: the document a request states, and the version the stored document must state.
struct Row {
    case: String,
    stated: Value,
    stored_version: &'static str,
}

/// The rows for one resource: `arn` is the resource a policy is attached to and `actions`
/// are two actions THAT resource has, since a policy over a stream may only name the
/// stream's own actions.
fn rows(target: &str, arn: &str, actions: [&str; 2]) -> Vec<Row> {
    let account = arn.split(':').nth(4).unwrap_or_default().to_string();
    let one = allow(&account, json!(actions[0]), arn);
    let two = allow(&account, json!([actions[0], actions[1]]), arn);
    let mut conditioned = allow(&account, json!(actions[1]), arn);
    if let Some(statement) = conditioned.as_object_mut() {
        statement.insert("Sid".to_string(), json!("fromOneOfTwoNetworks"));
        statement.insert(
            "Condition".to_string(),
            json!({"StringEquals": {"aws:SourceVpc": ["vpc-1a2b3c4d", "vpc-5e6f7a8b"]}}),
        );
    }
    vec![
        Row {
            case: format!("{target}, a document that states no Version"),
            stated: json!({"Statement": [one.clone()]}),
            stored_version: DEFAULT_VERSION,
        },
        Row {
            case: format!("{target}, no Version, with an Id and two statements"),
            stated: json!({"Id": "two-statements", "Statement": [one.clone(), two.clone()]}),
            stored_version: DEFAULT_VERSION,
        },
        Row {
            case: format!("{target}, no Version, with a Sid and a Condition"),
            stated: json!({"Statement": [conditioned]}),
            stored_version: DEFAULT_VERSION,
        },
        Row {
            case: format!("{target}, a document that states the current version"),
            stated: json!({"Version": CURRENT_VERSION, "Statement": [two]}),
            stored_version: CURRENT_VERSION,
        },
        Row {
            case: format!("{target}, the default version stated after the Statement"),
            stated: json!({"Statement": [one], "Version": DEFAULT_VERSION}),
            stored_version: DEFAULT_VERSION,
        },
    ]
}

/// The document the rule says the resource carries for a request that stated `stated`: the
/// document as it arrived, with the version it is read under stated where it stated none.
fn expected_document(stated: &Value, stored_version: &str) -> Value {
    let mut expected = stated.clone();
    if expected.get("Version").is_none() {
        if let Some(elements) = expected.as_object_mut() {
            elements.insert("Version".to_string(), json!(stored_version));
        }
    }
    expected
}

/// The policy the resource carries once the revision `revision_id` names is the readable
/// one: the whole `GetResourcePolicy` answer.
///
/// The wait is on that CONDITION and gives up only past [`POLICY_READ_DEADLINE`]. A read
/// made inside the write's own span reports the revision the resource carried before it, or
/// refuses where it carried none at all ("or return the `PolicyNotFoundException`" — the
/// `PutResourcePolicy` doc), so neither answer ends the wait.
async fn policy_once_readable(
    case: &str,
    router: &MultiServiceRouter,
    arn: &str,
    revision_id: &str,
) -> Value {
    let deadline = Instant::now() + POLICY_READ_DEADLINE;
    loop {
        let (status, read) = call(router, "GetResourcePolicy", json!({"ResourceArn": arn})).await;
        if status == 200 && read["RevisionId"] == json!(revision_id) {
            return read;
        }
        assert!(
            Instant::now() < deadline,
            "case {case:?}: expected the policy at revision {revision_id:?} to become readable, \
             last read {status} with {read}"
        );
        tokio::time::sleep(POLICY_READ_INTERVAL).await;
    }
}

/// The `Policy` document a read reported, as its text and as the JSON it parses to.
fn reported_document(case: &str, read: &Value) -> (String, Value) {
    let text = read["Policy"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: expected a Policy document, got {read}"))
        .to_string();
    let parsed = serde_json::from_str(&text)
        .unwrap_or_else(|_| panic!("case {case:?}: expected a JSON document, got {text:?}"));
    (text, parsed)
}

/// Attach `policy` to `arn` and report the `RevisionId` the put answered.
async fn put_policy(case: &str, router: &MultiServiceRouter, arn: &str, policy: &str) -> String {
    let written = ok(
        router,
        "PutResourcePolicy",
        json!({"ResourceArn": arn, "Policy": policy}),
    )
    .await;
    written["RevisionId"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: expected a RevisionId, got {written}"))
        .to_string()
}

/// Hold every row of one resource to the rule.
async fn every_row_of(router: &MultiServiceRouter, target: &str, arn: &str, actions: [&str; 2]) {
    for Row {
        case,
        stated,
        stored_version,
    } in rows(target, arn, actions)
    {
        let revision_id = put_policy(&case, router, arn, &stated.to_string()).await;
        let read = policy_once_readable(&case, router, arn, &revision_id).await;
        let (text, stored) = reported_document(&case, &read);

        assert_eq!(
            stored["Version"],
            json!(stored_version),
            "case {case:?}: expected the stored document to state Version {stored_version:?}, \
             got {stored}"
        );
        assert_eq!(
            stored,
            expected_document(&stated, stored_version),
            "case {case:?}: expected the document as it arrived with the version it is read \
             under, got {stored}"
        );

        // The round trip: putting back the document the read reported is the documented
        // idempotent re-attach ("running it multiple times on the same resource using the
        // same policy document will return the same revision ID"), and the resource still
        // carries that same document afterwards — so a client that reads a policy and puts
        // it again sees no drift and no new revision.
        let again = put_policy(&case, router, arn, &text).await;
        assert_eq!(
            again, revision_id,
            "case {case:?}: expected re-attaching the stored document to report the revision \
             it already carried ({revision_id}), got {again}"
        );
        let reread = policy_once_readable(&case, router, arn, &revision_id).await;
        let (rereported, _) = reported_document(&case, &reread);
        assert_eq!(
            rereported, text,
            "case {case:?}: expected the re-attached document to be stored unchanged, got \
             {rereported:?}"
        );
    }
}

#[tokio::test]
async fn every_resource_stores_a_policy_under_the_version_it_is_read_under() {
    let router = emulator();
    let table = "PolicyVersionPerResource";
    // A stream-enabled table, so BOTH resources a resource-based policy attaches to exist.
    ok(
        &router,
        "CreateTable",
        json!({
            "TableName": table,
            "BillingMode": "PAY_PER_REQUEST",
            "KeySchema": [{"AttributeName": "pk", "KeyType": "HASH"}],
            "AttributeDefinitions": [{"AttributeName": "pk", "AttributeType": "S"}],
            "StreamSpecification": {"StreamEnabled": true, "StreamViewType": "NEW_IMAGE"},
        }),
    )
    .await;
    let described = describe_when_active(&router, "the policy-bearing table", table).await;
    let table_arn = described["TableArn"]
        .as_str()
        .unwrap_or_else(|| panic!("expected the table's own ARN, got {described}"))
        .to_string();
    let stream_arn = described["LatestStreamArn"]
        .as_str()
        .unwrap_or_else(|| panic!("expected the table's stream ARN, got {described}"))
        .to_string();

    // Each resource's policy names that resource's own actions: the table's item actions,
    // and the stream actions "supported for stream resource-based policies" (the Amazon
    // DynamoDB Developer Guide's resource-based policy entry for streams). The two slots are
    // independent, so the rows of one resource are driven alongside the rows of the other
    // and neither waits on the other's propagation.
    tokio::join!(
        every_row_of(
            &router,
            "the table",
            &table_arn,
            ["dynamodb:GetItem", "dynamodb:Query"],
        ),
        every_row_of(
            &router,
            "its stream",
            &stream_arn,
            ["dynamodb:GetRecords", "dynamodb:DescribeStream"],
        ),
    );
}
