// SPDX-License-Identifier: CC0-1.0

//! A parser for an AWS-vended log format is legal only as the FIRST processor of a pipeline.
//!
//! RULE: the model states on every vended-log parser that "if you use this processor, it must
//! be the first processor in your transformer" — each "always processes the entire log event
//! message", so there is nothing for it to do once another processor has restructured the
//! event. A pipeline that names one at any later position is refused with
//! `InvalidParameterException`, wherever a processor list is configured: the log group's own
//! transformer (`PutTransformer$transformerConfig`) and the account-level transformer policy
//! (`PutAccountPolicy` with `TRANSFORMER_POLICY`). The refusal names the processor that has to
//! lead, and the stored configuration is left as it was.
//!
//! The rows differ along what the rule generalises over: WHICH vended parser is misplaced
//! (every one the model admits only first), WHICH parser leads the pipeline, how far down the
//! misplaced one sits, and which of the two operations the pipeline is configured through.

use serde_json::{json, Value};

use crate::account_policies::{delete_body, put_body, transformer_document};
use crate::harness::Logs;
use crate::transformers::LEADING_ONLY_KINDS;

/// The refusal both operations answer a misplaced vended parser with.
const INVALID_PARAMETER: &str = "InvalidParameterException";

/// The log group every `PutTransformer` row is configured on.
const GROUP: &str = "/transformers/vended-parser-position";

/// The account-level policy every policy row is put under.
const POLICY: &str = "vended-parser-position";

/// The parsers a pipeline may be LED by in these rows: one configurable parser and one vended
/// parser, so what is refused is the misplaced processor's POSITION rather than one pairing.
const LEADING: [&str; 2] = ["parseJSON", "parseVPC"];

/// One processor as a client writes it in a pipeline: the member naming the kind, with the
/// configuration left to the service's defaults.
///
/// `pub(crate)` because the refusal-text suite drives the same rule from other operations and
/// at other depths, and a pipeline element is written the one way.
pub(crate) fn processor(member: &str) -> Value {
    json!({member: {}})
}

/// The sentence the service answers a misplaced vended parser with, for the kind `named`.
///
/// `pub(crate)` so the one statement of this error class's fixed text is shared with the
/// refusal-text suite rather than transcribed twice.
pub(crate) fn expected_message(named: &str) -> String {
    format!("Log processor {named} must be in the first position of the processor list")
}

/// A `PutTransformer` body for `pipeline` on the suite's log group.
fn transformer_body(pipeline: &[Value]) -> Value {
    json!({"logGroupIdentifier": GROUP, "transformerConfig": pipeline})
}

#[tokio::test]
async fn a_vended_parser_is_refused_anywhere_but_the_first_position() {
    let logs = Logs::new();
    logs.ok("CreateLogGroup", &json!({"logGroupName": GROUP})).await;

    // The pipeline the group starts with, and the one every refused put must leave in place.
    let stored = vec![processor("parseJSON")];
    logs.ok("PutTransformer", &transformer_body(&stored)).await;

    for misplaced in LEADING_ONLY_KINDS {
        for leading in LEADING {
            if leading == misplaced {
                continue;
            }
            // Two depths for the misplaced parser: straight behind the leading parser, and
            // behind a mutation step as well, so the rule is about its position rather than
            // about the processor next to it.
            for depth in [1usize, 2] {
                let mut pipeline = vec![processor(leading)];
                if depth == 2 {
                    pipeline.push(json!({"trimString": {"withKeys": ["message"]}}));
                }
                pipeline.push(processor(misplaced));
                let case = format!("{misplaced} at position {depth} behind {leading}");

                let (code, message) = logs
                    .refusal("PutTransformer", &transformer_body(&pipeline))
                    .await;
                assert_eq!(
                    code, INVALID_PARAMETER,
                    "case {case:?}: expected {INVALID_PARAMETER}, got {code}"
                );
                assert_eq!(
                    message,
                    expected_message(misplaced),
                    "case {case:?}: expected {:?}, got {message:?}",
                    expected_message(misplaced)
                );

                let found = logs
                    .ok("GetTransformer", &json!({"logGroupIdentifier": GROUP}))
                    .await;
                assert_eq!(
                    found["transformerConfig"],
                    json!(stored),
                    "case {case:?}: a refused put leaves the stored pipeline as it was, got \
                     {found}"
                );
            }

            // The control: the same parser LEADING its own pipeline is configured.
            let led = vec![processor(misplaced), json!({"trimString": {"withKeys": ["message"]}})];
            logs.ok("PutTransformer", &transformer_body(&led)).await;
            let found = logs
                .ok("GetTransformer", &json!({"logGroupIdentifier": GROUP}))
                .await;
            assert_eq!(
                found["transformerConfig"],
                json!(led),
                "case {misplaced} leading: a vended parser leads a pipeline, got {found}"
            );
            logs.ok("PutTransformer", &transformer_body(&stored)).await;
        }
    }
}

#[tokio::test]
async fn an_account_transformer_policy_holds_the_same_position_rule() {
    let logs = Logs::new();
    // The policy the account starts with, and the one every refused put must leave in place.
    logs.ok(
        "PutAccountPolicy",
        &put_body(POLICY, "TRANSFORMER_POLICY", &transformer_document(), None),
    )
    .await;

    for misplaced in LEADING_ONLY_KINDS {
        let document = json!([
            processor("parseJSON"),
            {"trimString": {"withKeys": ["message"]}},
            processor(misplaced),
        ])
        .to_string();
        let (code, message) = logs
            .refusal(
                "PutAccountPolicy",
                &put_body(POLICY, "TRANSFORMER_POLICY", &document, None),
            )
            .await;
        assert_eq!(
            code, INVALID_PARAMETER,
            "case {misplaced} in an account policy: expected {INVALID_PARAMETER}, got {code}"
        );
        assert_eq!(
            message,
            expected_message(misplaced),
            "case {misplaced} in an account policy: expected {:?}, got {message:?}",
            expected_message(misplaced)
        );

        let held = logs
            .ok(
                "DescribeAccountPolicies",
                &json!({"policyType": "TRANSFORMER_POLICY"}),
            )
            .await;
        let documents: Vec<String> = held["accountPolicies"]
            .as_array()
            .cloned()
            .unwrap_or_default()
            .iter()
            .map(|policy| policy["policyDocument"].as_str().unwrap_or_default().to_string())
            .collect();
        assert_eq!(
            documents,
            vec![transformer_document()],
            "case {misplaced} in an account policy: a refused put changes nothing, got {held}"
        );

        // The control: the same parser LEADING the policy's pipeline is accepted.
        let led = json!([processor(misplaced), {"trimString": {"withKeys": ["message"]}}])
            .to_string();
        logs.ok(
            "PutAccountPolicy",
            &put_body(POLICY, "TRANSFORMER_POLICY", &led, None),
        )
        .await;
        logs.ok(
            "PutAccountPolicy",
            &put_body(POLICY, "TRANSFORMER_POLICY", &transformer_document(), None),
        )
        .await;
    }

    logs.ok(
        "DeleteAccountPolicy",
        &delete_body(POLICY, "TRANSFORMER_POLICY"),
    )
    .await;
}
