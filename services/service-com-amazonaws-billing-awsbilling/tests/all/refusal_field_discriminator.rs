// SPDX-License-Identifier: CC0-1.0

//! What a `fieldList` entry of this service's refusals carries on the wire.
//!
//! One rule: every entry of a `ValidationException$fieldList` this surface
//! answers carries the `__type` discriminator naming the field structure in the
//! namespace of the backend that serves this service — beside the two members
//! the model declares, `name` and `message`. On the awsJson protocols that
//! discriminator is how a client decides how to decode a JSON error document,
//! and the backend stamps one on each nested error structure as well as at the
//! top level; the same recorded detail makes this surface announce
//! `application/x-amz-json-1.1` while the model declares `awsJson1_0`.
//!
//! The rows span what the rule generalises over: the two paths a refusal of this
//! service comes from — the constraint check in FRONT of the operation
//! (`crate::service`'s `ConstraintRefusals`) and an operation's OWN rule, raised
//! from its handler — across several operations and several members, including
//! one refusal whose entry names a member no modelled constraint covers. The
//! shape of the entry cannot depend on which path produced it: a client reads
//! one error document either way.
//!
//! The harness (`call`, `error_code`, …) is the one `billing_views.rs` owns.

use serde_json::{json, Value};

use crate::billing_views::{call, emulator, error_code, ABSENT_ARN, PRIMARY_ARN};

/// The shape id the wire stamps on a `ValidationExceptionField` entry's
/// `__type`: the field structure, named in the namespace of the backend that
/// serves this surface rather than in this model's own.
const FIELD_WIRE_TYPE: &str = "com.amazonaws.awsinsightsindexservice.v0#ValidationExceptionField";

/// One row: the call it makes, the member the refusal must name, and which path
/// the refusal comes from (recorded in the case text, so a failure says which).
struct Row {
    case: &'static str,
    operation: &'static str,
    request: Value,
    member: &'static str,
}

fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "the constraint check in front of the operation: an arn outside its pattern",
            operation: "GetBillingView",
            request: json!({ "arn": "not-an-arn" }),
            member: "arn",
        },
        Row {
            case: "the constraint check in front of the operation: an absent required member",
            operation: "DeleteBillingView",
            request: json!({}),
            member: "arn",
        },
        Row {
            case: "the constraint check in front of the operation: maxResults below its range",
            operation: "ListBillingViews",
            request: json!({ "maxResults": 0 }),
            member: "maxResults",
        },
        Row {
            case: "the constraint check in front of the operation: a billingMonth pattern",
            operation: "GetEnterpriseSupportChargeSummary",
            request: json!({ "billingMonth": "2024-13" }),
            member: "billingMonth",
        },
        Row {
            case: "the operation's own rule: an edit of a view that is not CUSTOM",
            operation: "UpdateBillingView",
            request: json!({ "arn": PRIMARY_ARN, "name": "renamed" }),
            member: "arn",
        },
        Row {
            case: "the operation's own rule: a delete of an arn no view is stored under",
            operation: "DeleteBillingView",
            request: json!({ "arn": ABSENT_ARN }),
            member: "arn",
        },
        Row {
            case: "the operation's own rule: a data filter period that selects no instant",
            operation: "CreateBillingView",
            request: json!({
                "name": "inverted-period",
                "sourceViews": [PRIMARY_ARN],
                "dataFilterExpression": {
                    "timeRange": { "beginDateInclusive": 2, "endDateInclusive": 1 },
                },
            }),
            member: "dataFilterExpression",
        },
        // The operations the model added most recently are refused the same way,
        // whatever their handlers answer a well-formed request: the refusal comes
        // from in front of the operation, so the shape of its fieldList entry is
        // a property of the service and not of what is written behind it.
        Row {
            case: "the constraint check in front of a contract operation: an arn pattern",
            operation: "AssociateContract",
            request: json!({ "contractArn": "not-an-arn", "contractDocumentId": "1" }),
            member: "contractArn",
        },
        Row {
            case: "the constraint check in front of a contract operation: absent members",
            operation: "GetContractInformation",
            request: json!({}),
            member: "contractArn",
        },
    ]
}

#[tokio::test]
async fn every_field_list_entry_carries_the_backend_type_discriminator() {
    let router = emulator();

    for row in rows() {
        let case = row.case;
        let (status, body) = call(&router, row.operation, &row.request).await;
        assert_eq!(
            (status, error_code(&body).as_str()),
            (400, "ValidationException"),
            "case {case:?}: expected 400 ValidationException from {}, got {status} {body:?}",
            row.operation
        );
        let entries = body["fieldList"]
            .as_array()
            .cloned()
            .unwrap_or_else(|| panic!("case {case:?}: expected a fieldList, got {body:?}"));
        assert!(
            !entries.is_empty(),
            "case {case:?}: a member refusal names the member it is about, got {body:?}"
        );
        for entry in &entries {
            assert_eq!(
                entry["__type"].as_str(),
                Some(FIELD_WIRE_TYPE),
                "case {case:?}: expected the entry to carry the backend discriminator, \
                 got {entry:?}"
            );
            assert!(
                entry["name"].as_str().is_some_and(|name| !name.is_empty()),
                "case {case:?}: expected the entry to name a member, got {entry:?}"
            );
            assert!(
                entry["message"]
                    .as_str()
                    .is_some_and(|message| !message.is_empty()),
                "case {case:?}: expected the entry to say why, got {entry:?}"
            );
        }
        assert_eq!(
            entries[0]["name"].as_str(),
            Some(row.member),
            "case {case:?}: expected the first entry to name {:?}, got {entries:?}",
            row.member
        );
    }
}
