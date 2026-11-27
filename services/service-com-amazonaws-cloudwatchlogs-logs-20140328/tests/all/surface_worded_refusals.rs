// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the refusals CloudWatch Logs words PER SURFACE: the sentence an
//! operation family states for a condition the rest of the service states another way, and
//! the error CLASS a condition belongs to.
//!
//! Each test below pins one rule, over a table that spans what that rule generalises over
//! rather than one request:
//!
//! * the DATA PROTECTION POLICY operations — the read, the write and the delete — refuse a
//!   log group the account does not hold by stating the key such a policy is kept under: the
//!   group the request named and the account that would own it;
//! * the FIELD INDEX operations — the index-policy write, delete and read, and the
//!   field-index read — refuse one with that surface's fixed sentence, which names neither
//!   the group nor the account;
//! * the LOG-STREAM LISTING refuses a `nextToken` it cannot resume from with the sentence a
//!   LIST CALL's token is refused with, whichever way the token fails to place, while the
//!   log-event read beside it keeps the sentence the service's paginated reads state;
//! * a constraint clause about a COLLECTION member states the collection the request
//!   submitted, in the order it was sent;
//! * a `StartQuery` that selects its log groups NOWHERE does not compile, so it is refused
//!   with the compiler's own diagnostic under the malformed-query class rather than as a
//!   parameter outside its range.
//!
//! Every case drives the service the way a client does — `POST /` with
//! `X-Amz-Target: Logs_20140328.<Operation>` through the router that mounts it — so a
//! response a handler does not compute cannot pass.

use serde_json::{json, Value};
use service_com_amazonaws_cloudwatchlogs_logs_20140328::state::{
    INDEX_POLICY_LOG_GROUP_NOT_FOUND_MESSAGE, INVALID_NEXT_TOKEN_MESSAGE,
    PAGINATION_TOKEN_INVALID_MESSAGE,
};

use crate::harness::{base_ms, batch, now_ms, Logs, REGION};

/// The modelled codes these refusals are answered under.
const NOT_FOUND: &str = "ResourceNotFoundException";
const INVALID_PARAMETER: &str = "InvalidParameterException";
const MALFORMED_QUERY: &str = "MalformedQueryException";

/// The log group names no case below creates, which is what makes every one of them a
/// missing log group. They differ in the characters `LogGroupName` admits (`@pattern`
/// `[\.\-_/#A-Za-z0-9]+`) and in how deep the path is, so the sentence is shown to carry
/// whichever name the request named rather than one shape of name.
const ABSENT_GROUPS: [&str; 4] = [
    "/surface/never-created",
    "plain-name",
    "/aws/lambda/absent.function_1",
    "/a/deeply/nested/path/#tagged-group",
];

/// The log group ARN of `name`, in the form `logGroupIdentifier` admits without the
/// trailing `:*` — the other identifier form every operation here resolves.
///
/// The ARN form that CARRIES the trailing `:*` (a log group's own `arn` member) is not an
/// identifier form at all: `LogGroupIdentifier`'s `@pattern` admits no `*`, so a request
/// naming one is refused over that member instead. Every row below names a group by its
/// name or by this ARN, which are the two forms these operations resolve.
fn group_arn(name: &str) -> String {
    format!(
        "arn:aws:logs:{REGION}:{}:log-group:{name}",
        engine::emulator_account_id()
    )
}

/// The sentence the data protection surface states for a log group it cannot find: the
/// group that was looked for and the account that would own the policy.
///
/// Spelled here rather than read off the service, so the test pins the text instead of
/// restating whatever the service produced.
fn missing_group_for_owner(log_group_name: &str) -> String {
    format!(
        "Log group '{log_group_name}' for owner '{}' cannot be found",
        engine::emulator_account_id()
    )
}

/// A data protection policy document of the form `PutDataProtectionPolicy$policyDocument`
/// documents: an Audit block and a Deidentify block over one AWS-managed data identifier,
/// whose names come from the catalogue that member links to.
fn data_protection_document() -> String {
    let identifiers = json!(["arn:aws:dataprotection::aws:data-identifier/EmailAddress"]);
    json!({
        "Name": "surface-worded-refusals",
        "Version": "2021-06-01",
        "Statement": [
            {
                "Sid": "audit-sensitive-data",
                "DataIdentifier": identifiers,
                "Operation": {"Audit": {"FindingsDestination": {}}},
            },
            {
                "Sid": "mask-sensitive-data",
                "DataIdentifier": identifiers,
                "Operation": {"Deidentify": {"MaskConfig": {}}},
            },
        ],
    })
    .to_string()
}

/// A field index policy document, whose one required field names the fields to index
/// (`PutIndexPolicy$policyDocument`).
fn index_policy_document() -> String {
    json!({"Fields": ["requestId"]}).to_string()
}

/// Every data protection policy operation refuses a log group the account does not hold
/// with the key such a policy is kept under — whichever of the three operations, and
/// whichever form the request names the group by.
#[tokio::test]
async fn the_data_protection_operations_state_the_key_a_missing_group_was_looked_for_under() {
    let logs = Logs::new();

    for name in ABSENT_GROUPS {
        for (form, identifier) in [("name", name.to_string()), ("arn", group_arn(name))] {
            // (the operation, its body beside `logGroupIdentifier`)
            let calls: [(&str, Value); 3] = [
                ("GetDataProtectionPolicy", json!({})),
                (
                    "PutDataProtectionPolicy",
                    json!({"policyDocument": data_protection_document()}),
                ),
                ("DeleteDataProtectionPolicy", json!({})),
            ];
            for (operation, extra) in calls {
                let mut body = json!({"logGroupIdentifier": identifier});
                if let (Some(target), Some(members)) = (body.as_object_mut(), extra.as_object()) {
                    for (member, value) in members {
                        target.insert(member.clone(), value.clone());
                    }
                }
                let case = format!("{operation} by {form} ({name})");
                let (code, message) = logs.refusal(operation, &body).await;
                assert_eq!(
                    code, NOT_FOUND,
                    "case {case:?}: a log group the account does not hold is a missing \
                     resource, got {code} {message:?}"
                );
                let expected = missing_group_for_owner(name);
                assert_eq!(
                    message, expected,
                    "case {case:?}: expected {expected:?}, got {message:?}"
                );
            }
        }
    }
}

/// The boundary of that sentence: a log group the account DOES hold is not refused by any of
/// the three, so the sentence above is about the missing group and not about the surface
/// refusing every request.
#[tokio::test]
async fn a_held_log_group_is_served_by_the_data_protection_operations() {
    let logs = Logs::new();
    const GROUP: &str = "/surface/data-protection";
    logs.ok("CreateLogGroup", &json!({"logGroupName": GROUP}))
        .await;

    let stored = logs
        .ok(
            "PutDataProtectionPolicy",
            &json!({
                "logGroupIdentifier": GROUP,
                "policyDocument": data_protection_document(),
            }),
        )
        .await;
    assert_eq!(
        stored.get("logGroupIdentifier").and_then(Value::as_str),
        Some(GROUP),
        "the put reports the identifier the request named, got {stored}"
    );
    let read = logs
        .ok(
            "GetDataProtectionPolicy",
            &json!({"logGroupIdentifier": GROUP}),
        )
        .await;
    assert_eq!(
        read.get("policyDocument").and_then(Value::as_str),
        Some(data_protection_document().as_str()),
        "the read reports the stored document, got {read}"
    );
    logs.ok(
        "DeleteDataProtectionPolicy",
        &json!({"logGroupIdentifier": GROUP}),
    )
    .await;
}

/// Every FIELD INDEX operation refuses a log group the account does not hold with that
/// surface's own fixed sentence: the index-policy write, delete and read, and the
/// field-index read, which names its groups in a LIST.
#[tokio::test]
async fn the_field_index_operations_state_their_own_sentence_for_a_missing_group() {
    let logs = Logs::new();
    const HELD: &str = "/surface/field-indexes";
    logs.ok("CreateLogGroup", &json!({"logGroupName": HELD}))
        .await;

    for name in ABSENT_GROUPS {
        for (form, identifier) in [("name", name.to_string()), ("arn", group_arn(name))] {
            // (the case, the operation, the whole body)
            let calls: Vec<(&str, &str, Value)> = vec![
                (
                    "the index policy write",
                    "PutIndexPolicy",
                    json!({
                        "logGroupIdentifier": identifier,
                        "policyDocument": index_policy_document(),
                    }),
                ),
                (
                    "the index policy delete",
                    "DeleteIndexPolicy",
                    json!({"logGroupIdentifier": identifier}),
                ),
                (
                    "the index policy read",
                    "DescribeIndexPolicies",
                    json!({"logGroupIdentifiers": [identifier]}),
                ),
                (
                    "the field index read, the absent group alone",
                    "DescribeFieldIndexes",
                    json!({"logGroupIdentifiers": [identifier]}),
                ),
                (
                    "the field index read, the absent group after a held one",
                    "DescribeFieldIndexes",
                    json!({"logGroupIdentifiers": [HELD, identifier]}),
                ),
                (
                    "the field index read, the absent group before a held one",
                    "DescribeFieldIndexes",
                    json!({"logGroupIdentifiers": [identifier, HELD]}),
                ),
                (
                    // The categories a request asks for do not change which resource is
                    // missing: the group is resolved before anything is reported for it.
                    "the field index read, asking for no custom category",
                    "DescribeFieldIndexes",
                    json!({
                        "logGroupIdentifiers": [identifier],
                        "indexCategories": ["DEFAULT"],
                    }),
                ),
            ];
            for (what, operation, body) in calls {
                let case = format!("{what} by {form} ({name})");
                let (code, message) = logs.refusal(operation, &body).await;
                assert_eq!(
                    code, NOT_FOUND,
                    "case {case:?}: a log group the account does not hold is a missing \
                     resource, got {code} {message:?}"
                );
                assert_eq!(
                    message, INDEX_POLICY_LOG_GROUP_NOT_FOUND_MESSAGE,
                    "case {case:?}: expected {INDEX_POLICY_LOG_GROUP_NOT_FOUND_MESSAGE:?}, got \
                     {message:?}"
                );
            }
        }
    }

    // The boundary: the field index read of a group the account HOLDS is answered.
    let served = logs
        .ok(
            "DescribeFieldIndexes",
            &json!({"logGroupIdentifiers": [HELD]}),
        )
        .await;
    assert!(
        served.get("fieldIndexes").and_then(Value::as_array).is_some(),
        "a held log group is answered by a page of field indexes, got {served}"
    );
}

/// The log-stream listing refuses a `nextToken` it cannot resume from with the sentence a
/// LIST CALL's token is refused with — whichever way the token fails to place: never minted
/// here, undecodable, or minted for another enumeration (another log group, another
/// ordering, another direction, another prefix).
#[tokio::test]
async fn the_log_stream_listing_refuses_a_token_it_cannot_resume_with_the_list_call_sentence() {
    let logs = Logs::new();
    const GROUP: &str = "/surface/streams";
    const OTHER_GROUP: &str = "/surface/streams-two";
    for group in [GROUP, OTHER_GROUP] {
        logs.ok("CreateLogGroup", &json!({"logGroupName": group}))
            .await;
        for stream in ["app-one", "app-two", "sys-one"] {
            logs.ok(
                "CreateLogStream",
                &json!({"logGroupName": group, "logStreamName": stream}),
            )
            .await;
        }
    }

    // A token this listing DID mint, for the unfiltered name-ordered enumeration of GROUP.
    let page = logs
        .ok(
            "DescribeLogStreams",
            &json!({"logGroupName": GROUP, "limit": 1}),
        )
        .await;
    let minted = page["nextToken"]
        .as_str()
        .unwrap_or_else(|| panic!("a page with more to come hands back a token, got {page}"))
        .to_string();

    // (case, the request carrying a token this listing cannot resume from)
    let cases: Vec<(&str, Value)> = vec![
        (
            "a token this service never minted",
            json!({"logGroupName": GROUP, "nextToken": "not-a-token"}),
        ),
        (
            "hex that decodes to no cursor of this service",
            json!({"logGroupName": GROUP, "nextToken": "deadbeef"}),
        ),
        (
            "a single character",
            json!({"logGroupName": GROUP, "nextToken": "x"}),
        ),
        (
            "a long run of text",
            json!({"logGroupName": GROUP, "nextToken": "z".repeat(300)}),
        ),
        (
            "a cursor minted under another kind",
            json!({
                "logGroupName": GROUP,
                "nextToken": "7b226b696e64223a224f74686572227d",
            }),
        ),
        (
            "this group's token replayed against another log group",
            json!({"logGroupName": OTHER_GROUP, "limit": 1, "nextToken": minted.clone()}),
        ),
        (
            "this group's token replayed by the group's ARN",
            json!({
                "logGroupIdentifier": group_arn(OTHER_GROUP),
                "limit": 1,
                "nextToken": minted.clone(),
            }),
        ),
        (
            "a name-ordered token replayed against the event-time ordering",
            json!({
                "logGroupName": GROUP,
                "limit": 1,
                "orderBy": "LastEventTime",
                "nextToken": minted.clone(),
            }),
        ),
        (
            "an ascending token replayed against the descending enumeration",
            json!({
                "logGroupName": GROUP,
                "limit": 1,
                "descending": true,
                "nextToken": minted.clone(),
            }),
        ),
        (
            "an unfiltered token replayed against a prefixed enumeration",
            json!({
                "logGroupName": GROUP,
                "limit": 1,
                "logStreamNamePrefix": "app-",
                "nextToken": minted.clone(),
            }),
        ),
    ];

    for (case, body) in cases {
        let (code, message) = logs.refusal("DescribeLogStreams", &body).await;
        assert_eq!(
            code, INVALID_PARAMETER,
            "case {case:?}: an unusable token is an incorrectly specified parameter, got {code} \
             {message:?}"
        );
        assert_eq!(
            message, PAGINATION_TOKEN_INVALID_MESSAGE,
            "case {case:?}: expected {PAGINATION_TOKEN_INVALID_MESSAGE:?}, got {message:?}"
        );
    }

    // The boundary: the token DOES resume the enumeration it was minted for.
    let continued = logs
        .ok(
            "DescribeLogStreams",
            &json!({"logGroupName": GROUP, "limit": 1, "nextToken": minted}),
        )
        .await;
    assert_eq!(
        continued["logStreams"]
            .as_array()
            .map(|streams| streams.len()),
        Some(1),
        "the minted token resumes its own enumeration, got {continued}"
    );
}

/// The other side of that rule: the LOG EVENT read's token is not a list call's token, so
/// one it cannot resume from keeps the sentence this service's paginated reads state.
#[tokio::test]
async fn the_log_event_read_keeps_the_services_own_token_sentence() {
    let logs = Logs::new();
    const GROUP: &str = "/surface/events";
    logs.ok("CreateLogGroup", &json!({"logGroupName": GROUP}))
        .await;
    let base = base_ms();
    for stream in ["events-one", "events-two"] {
        logs.ok(
            "CreateLogStream",
            &json!({"logGroupName": GROUP, "logStreamName": stream}),
        )
        .await;
        logs.ok(
            "PutLogEvents",
            &json!({
                "logGroupName": GROUP,
                "logStreamName": stream,
                "logEvents": batch(&[(base, "one"), (base + 1_000, "two")]),
            }),
        )
        .await;
    }
    let page = logs
        .ok(
            "GetLogEvents",
            &json!({"logGroupName": GROUP, "logStreamName": "events-one", "limit": 1}),
        )
        .await;
    let forward = page["nextForwardToken"]
        .as_str()
        .unwrap_or_else(|| panic!("a page of events hands back a forward token, got {page}"))
        .to_string();

    for (case, body) in [
        (
            "a token this service never minted",
            json!({
                "logGroupName": GROUP,
                "logStreamName": "events-one",
                "nextToken": "not-a-token",
            }),
        ),
        (
            "one stream's token replayed against another stream",
            json!({
                "logGroupName": GROUP,
                "logStreamName": "events-two",
                "nextToken": forward,
            }),
        ),
    ] {
        let (code, message) = logs.refusal("GetLogEvents", &body).await;
        assert_eq!(
            code, INVALID_PARAMETER,
            "case {case:?}: an unusable token is an incorrectly specified parameter, got {code} \
             {message:?}"
        );
        assert_eq!(
            message, INVALID_NEXT_TOKEN_MESSAGE,
            "case {case:?}: expected {INVALID_NEXT_TOKEN_MESSAGE:?}, got {message:?}"
        );
    }
}

/// The modelled maximum of `StartLiveTailLogGroupIdentifiers`
/// (`smithy_query(query="operation",operation="StartLiveTail")` -> `logGroupIdentifiers`,
/// `@length` 1..10).
const MAX_LIVE_TAIL_GROUPS: usize = 10;

/// A constraint clause about a COLLECTION member states the collection the request
/// submitted: the elements in the order they were sent, comma-separated, in brackets.
///
/// The rows differ along what the rule generalises over: how many elements are past the
/// bound, the FORM each element takes (a name, either ARN form), and the order they were
/// sent in — two rows carrying the SAME identifiers in two orders, so the quoted order is
/// shown to come from the request.
#[tokio::test]
async fn a_clause_about_a_collection_states_the_collection_that_was_submitted() {
    let logs = Logs::new();

    let names: Vec<String> = (1..=MAX_LIVE_TAIL_GROUPS + 1)
        .map(|index| format!("/surface/live-tail-{index}"))
        .collect();
    let one_past: Vec<String> = names.clone();
    let reversed: Vec<String> = names.iter().rev().cloned().collect();
    let twice_the_bound: Vec<String> = (1..=MAX_LIVE_TAIL_GROUPS * 2)
        .map(|index| format!("/surface/live-tail-{index}"))
        .collect();
    let mixed_forms: Vec<String> = names
        .iter()
        .enumerate()
        .map(|(index, name)| {
            if index % 2 == 0 {
                name.clone()
            } else {
                group_arn(name)
            }
        })
        .collect();

    for (case, identifiers) in [
        ("one identifier past the maximum", &one_past),
        ("the same identifiers in the other order", &reversed),
        ("twice the maximum", &twice_the_bound),
        ("the two identifier forms mixed", &mixed_forms),
    ] {
        let (code, message) = logs
            .refusal("StartLiveTail", &json!({"logGroupIdentifiers": identifiers}))
            .await;
        assert_eq!(
            code, INVALID_PARAMETER,
            "case {case:?}: a value outside a modelled constraint is an incorrectly specified \
             parameter, got {code} {message:?}"
        );
        let expected = format!(
            "1 validation error detected: Value '[{}]' at 'logGroupIdentifiers' failed to \
             satisfy constraint: Member must have length less than or equal to \
             {MAX_LIVE_TAIL_GROUPS}",
            identifiers.join(", ")
        );
        assert_eq!(
            message, expected,
            "case {case:?}: expected {expected:?}, got {message:?}"
        );
    }

    // The two orders are quoted differently, so the order is the REQUEST's and not one over
    // the elements.
    assert_ne!(
        one_past.join(", "),
        reversed.join(", "),
        "the two submitted orders must be quoted differently"
    );

    // The bound ITSELF is inside the constraint: a request at the maximum is past this
    // clause altogether and refused for the log groups it names instead.
    let at_the_bound = &names[..MAX_LIVE_TAIL_GROUPS];
    let (code, message) = logs
        .refusal("StartLiveTail", &json!({"logGroupIdentifiers": at_the_bound}))
        .await;
    assert_eq!(
        code, NOT_FOUND,
        "case at the maximum: the collection satisfies its bound, got {code} {message:?}"
    );
    assert!(
        !message.contains("validation error detected"),
        "case at the maximum: no constraint clause is stated, got {message:?}"
    );

    // A collection carrying NO element has no element to state, so its clause keeps the
    // position-only subject this service states for a scalar member.
    let (code, message) = logs
        .refusal("StartLiveTail", &json!({"logGroupIdentifiers": []}))
        .await;
    assert_eq!(
        (code.as_str(), message.as_str()),
        (
            INVALID_PARAMETER,
            "1 validation error detected: Value at 'logGroupIdentifiers' failed to satisfy \
             constraint: Member must have length greater than or equal to 1"
        ),
        "case an empty collection: expected the position-only clause, got {code} {message:?}"
    );
}

/// A `StartQuery` that selects its log groups NOWHERE — not in one of the three members and
/// not in the query string — is refused as the MALFORMED QUERY it is, with the COMPILER'S
/// diagnostic: the query string had to be a snippet opening on the selecting command, and
/// the symbol it opens on instead is named, with where in the submitted string it sits.
///
/// The rows differ along what the rule generalises over: the query language the request
/// names, the command the string opens on, where that symbol sits in the string, and one
/// string whose own text mentions the selecting command without being it.
#[tokio::test]
async fn a_query_that_selects_no_log_group_is_a_malformed_query() {
    let logs = Logs::new();
    const GROUP: &str = "/surface/queried";
    logs.ok("CreateLogGroup", &json!({"logGroupName": GROUP}))
        .await;
    // The window `StartQuery` documents in SECONDS, whose end is generously after the
    // instant the group above was created, so no accepted case below is refused for a range
    // that closes before its log group existed.
    let end_time = now_ms() / 1_000 + 60;
    let start_time = end_time - 3_600;

    // (case, the members beside startTime/endTime, the symbol the compiler stops on and
    // where it sits)
    let cases: Vec<(&str, Value, &str, usize, usize)> = vec![
        (
            "a one-stage pipeline",
            json!({"queryString": "fields @message"}),
            "fields",
            1,
            0,
        ),
        (
            "a pipeline of several stages",
            json!({"queryString": "stats count(*) by bin(5m) | sort @timestamp desc | limit 5"}),
            "stats",
            1,
            0,
        ),
        (
            "a filtering pipeline whose own text mentions the selecting command",
            json!({"queryString": "filter @message like \"SOURCE\" | fields @message"}),
            "filter",
            1,
            0,
        ),
        (
            // The place is the symbol's own: a string that opens with whitespace and a
            // newline is stopped at on the line the symbol is on, at its position there.
            "a pipeline written after a leading newline",
            json!({"queryString": "  \n   display @message"}),
            "display",
            2,
            3,
        ),
        (
            "a query in the PPL language with no selecting command",
            json!({
                "queryString": "fields `@message` | head 5",
                "queryLanguage": "PPL",
            }),
            "fields",
            1,
            0,
        ),
    ];

    for (case, members, symbol, line, position) in cases {
        let mut body = json!({"startTime": start_time, "endTime": end_time});
        if let (Some(target), Some(members)) = (body.as_object_mut(), members.as_object()) {
            for (member, value) in members {
                target.insert(member.clone(), value.clone());
            }
        }
        let (code, said) = logs.refusal("StartQuery", &body).await;
        assert_eq!(
            code, MALFORMED_QUERY,
            "case {case:?}: a query selecting no log group is malformed, got {code} {said:?}"
        );
        // The diagnostic is the SENTENCE of the message: the syntax that was being read, then
        // the symbol the compiler stopped on and where in the submitted string it sits — and
        // then the envelope clause every refusal of this service closes with.
        let message =
            crate::harness::without_sdk_envelope(case, &said, 400, MALFORMED_QUERY);
        let expected = format!(
            "Invalid syntax while using query definition snippets: unexpected symbol found \
             {symbol} at line {line} and position {position}"
        );
        assert_eq!(
            message, expected,
            "case {case:?}: expected {expected:?}, got {message:?}"
        );
    }

    // The boundary: a request that DOES select its log groups, in each of the four places
    // the sentence names, is admitted and minted a query id.
    let (_, arn) = logs.group_arns(GROUP).await;
    let selecting: Vec<(&str, Value)> = vec![
        (
            "logGroupName",
            json!({"logGroupName": GROUP, "queryString": "fields @message"}),
        ),
        (
            "logGroupNames",
            json!({"logGroupNames": [GROUP], "queryString": "fields @message"}),
        ),
        (
            "logGroupIdentifiers",
            json!({"logGroupIdentifiers": [arn], "queryString": "fields @message"}),
        ),
        (
            "a SOURCE command in the query string",
            json!({
                "queryString": "SOURCE logGroups(namePrefix: [\"/surface/\"]) | fields @message",
            }),
        ),
    ];
    for (case, members) in selecting {
        let mut body = json!({"startTime": start_time, "endTime": end_time});
        if let (Some(target), Some(members)) = (body.as_object_mut(), members.as_object()) {
            for (member, value) in members {
                target.insert(member.clone(), value.clone());
            }
        }
        let answer = logs.ok("StartQuery", &body).await;
        assert!(
            answer
                .get("queryId")
                .and_then(Value::as_str)
                .is_some_and(|id| !id.is_empty()),
            "case {case:?}: a query that selects its log groups is admitted, got {answer}"
        );
    }
}
