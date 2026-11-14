// SPDX-License-Identifier: CC0-1.0

//! The payload of Parameter Store's `ParameterNotFound`, end to end over the front door.
//!
//! The rule under test: **the refusal a name Parameter Store holds no parameter for is
//! answered with is the error code alone — `ParameterNotFound` with NO `message` member —
//! because the shape's `message` is optional and the service words none for it.** A client
//! that compares the whole error payload (code and message) therefore reads an empty
//! message, not a sentence this emulator wrote.
//!
//! The table's rows differ along what the rule generalises over: the FORM of the name that
//! resolves to nothing (a plain name, a hierarchy path, a path addressed without its leading
//! slash, a name in the AWS-managed namespace no caller can write, a name whose parameter was
//! deleted, an ARN of another account), and the OPERATION that reports it — every read and
//! delete of the slice that answers this error for a name it holds no parameter for. A name
//! the store DOES hold is read back in the same pass, so no row passes against a handler that
//! refuses everything.
//!
//! A name in the hierarchy AWS reserves for its own published parameters is the one row a
//! DELETE does not answer this way: that hierarchy is not the account's to change, so the
//! delete is denied rather than reported absent (`all/restricted_parameter_hierarchy.rs`),
//! while a read of a name AWS publishes nothing under is still a miss.

use serde_json::{json, Value};

use crate::parameter_store::{call, error_code, router};

/// The operations that report `ParameterNotFound` for a name the store holds no parameter
/// for. Source: each operation's modelled errors
/// (`smithy_query(query="operation",operation="GetParameter,GetParameterHistory,DeleteParameter")`),
/// each of which declares `ParameterNotFound`.
const OPERATIONS: &[&str] = &["GetParameter", "GetParameterHistory", "DeleteParameter"];

/// One row: a name no parameter of this account and Region answers to.
struct Row {
    case: &'static str,
    name: &'static str,
    /// Whether a DELETE of this name is a miss too. It is, for every name that is simply
    /// absent; a name in the hierarchy AWS reserves for its own published parameters is
    /// denied instead, because the hierarchy is not the account's to change.
    deletable: bool,
}

const MISSING: &[Row] = &[
    Row {
        case: "a plain name nobody wrote",
        name: "no-such-parameter",
        deletable: true,
    },
    Row {
        case: "a hierarchy path nobody wrote",
        name: "/team/service/absent",
        deletable: true,
    },
    Row {
        case: "a hierarchy path addressed without its leading slash",
        name: "team/service/absent",
        deletable: true,
    },
    // "PutParameter refuses a name prefixed with aws or ssm", so no caller can have
    // created one of these — and AWS publishes nothing under this one, so every READ of it
    // is a miss (a name AWS DOES publish resolves against its catalog instead; see
    // `all/public_parameter_catalog.rs`).
    Row {
        case: "the AWS-managed namespace no caller can write",
        name: "/aws/service/ami-amazon-linux-latest/no-such-published-alias",
        deletable: false,
    },
    Row {
        case: "a name whose every character is part of the path",
        name: "/a/b/c/d/e/f",
        deletable: true,
    },
];

/// Every missing name, over every operation that reports the error, answers 400
/// `ParameterNotFound` with no message at all.
#[tokio::test]
async fn parameter_not_found_carries_no_message() {
    let router = router();

    // A parameter the store DOES hold, so the pass cannot be satisfied by a handler that
    // refuses every name.
    let held = "/present/config";
    let (status, written) = call(
        &router,
        "PutParameter",
        &json!({ "Name": held, "Type": "String", "Value": "here" }),
    )
    .await;
    assert_eq!(
        status, 200,
        "expected the bystander parameter to be written, got {status} {written}"
    );

    for row in MISSING {
        for operation in OPERATIONS {
            if *operation == "DeleteParameter" && !row.deletable {
                continue;
            }
            let (status, body) = call(&router, operation, &json!({ "Name": row.name })).await;
            assert_not_found(row.case, operation, status, &body);
        }
    }

    // The same refusal for a name the store HELD and no longer does: the parameter is gone,
    // so the read is a miss of exactly the same shape.
    let (status, deleted) = call(&router, "DeleteParameter", &json!({ "Name": held })).await;
    assert_eq!(
        status, 200,
        "expected the held parameter to be deleted, got {status} {deleted}"
    );
    for operation in OPERATIONS {
        let (status, body) = call(&router, operation, &json!({ "Name": held })).await;
        assert_not_found("a deleted parameter", operation, status, &body);
    }
}

/// A read of a parameter that EXISTS is unaffected: the rule is about the refusal's payload,
/// not about which names resolve.
#[tokio::test]
async fn a_parameter_that_exists_still_reads_back() {
    let router = router();
    let name = "/still/served";
    let (status, written) = call(
        &router,
        "PutParameter",
        &json!({ "Name": name, "Type": "String", "Value": "value" }),
    )
    .await;
    assert_eq!(
        status, 200,
        "expected PutParameter to succeed, got {status} {written}"
    );
    let (status, read) = call(&router, "GetParameter", &json!({ "Name": name })).await;
    assert_eq!(
        (status, read.pointer("/Parameter/Value").and_then(Value::as_str)),
        (200, Some("value")),
        "expected the parameter to read back, got {status} {read}"
    );
}

/// The payload every row must answer with: HTTP 400, the code `ParameterNotFound`, and no
/// `message` member — an absent member and an empty one are the same empty message on the
/// wire, so either is accepted and any sentence is not.
fn assert_not_found(case: &str, operation: &str, status: u16, body: &Value) {
    let message = body.get("message").or_else(|| body.get("Message"));
    let worded = message.and_then(Value::as_str).unwrap_or_default();
    assert_eq!(
        (status, error_code(body).as_str(), worded),
        (400, "ParameterNotFound", ""),
        "case {case:?}: expected {operation} to answer 400 ParameterNotFound with no \
         message, got {status} {body}"
    );
}
