// SPDX-License-Identifier: CC0-1.0

//! How this service states a request it refuses for breaking an INPUT CONSTRAINT, end to
//! end over HTTP.
//!
//! Two rules, each asserted over a table whose rows differ along what the rule generalises
//! over rather than along the one request that exposed it:
//!
//! * A constraint clause names the member and the requirement it broke, and states a VALUE
//!   only where that value is the collection the caller submitted: a scalar member's own
//!   value is never echoed back, whatever its type, its length or the constraint class it
//!   broke, while a member that was ABSENT is still stated `Value null` — the one thing the
//!   value half reports about a value nobody sent — and a list is stated as the list it is.
//!   The envelope and the requirements themselves stay the ones every AWS service states.
//! * How many campaign ids one batch state read may name is the `@length` its model
//!   declares: a batch up to that maximum is processed, one answer per requested id, and
//!   only a batch past it is refused — with the list the caller submitted stated as the
//!   value of the member that broke it.
//! * A bound the real service does NOT apply is not applied here either: a dialer config's
//!   `bandwidthAllocation` is accepted whatever value it carries — above the modelled
//!   maximum included — and the value is stored and reported back as sent, while every
//!   other bound of the same request, `dialingCapacity`'s own range among them, is still
//!   refused.
//!
//! The rows are built from the constrained members of this service's own model
//! (`smithy_query(query="operation", operation="CreateCampaign,ListCampaigns,\
//! GetCampaignStateBatch")` and `smithy_query(query="shape", shape="CampaignName,\
//! MaxResults,NextToken,InstanceId,BandwidthAllocation,ContactFlowId")`), so they span the
//! constraint classes a request can break — a length minimum and maximum, a range minimum
//! and maximum, a pattern, an absent required member — over a scalar at the top level, a
//! scalar nested inside a structure and a union, and a list.

use serde_json::{json, Value};

use super::common::{call, router, Harness};

/// The region every case here signs for.
const REGION: &str = "us-east-1";

/// The Connect instance every campaign below is created for, registered with the account
/// so a create that IS well formed succeeds — the positive control proving each refusal
/// below is the constraint and not the instance gate in front of it.
const INSTANCE: &str = "constraint-instance";

/// The most campaign ids one batch state read may name.
///
/// `GetCampaignStateBatchRequest$campaignIds`' `@length` maximum, whose minimum is one
/// entry, as the overlay model that operation's input is declared from carries them
/// (`smithy-model/local-additions.json`, see `smithy-model/LOCAL-ADDITIONS.md`), written
/// out here rather than read back out of the implementation so this suite pins it: a batch
/// of up to this many ids is processed structurally, every id answered on its own, and only
/// one past it is refused. It is the maximum the SERVICE enforces rather than the one the
/// member's documentation prints — the recorded corpus shows a batch of five hundred ids
/// answered, every unknown id of it reported in `failedRequests`.
const MAX_BATCH: usize = 500;

/// `CampaignName`'s published length maximum (`@length(min 1, max 127)`).
const MAX_NAME: usize = 127;

/// `InstanceId`'s published length maximum (`@length(min 0, max 256)`).
const MAX_INSTANCE_ID: usize = 256;

/// `NextToken`'s published length maximum (`@length(max 1000)`).
const MAX_NEXT_TOKEN: usize = 1000;

/// `ContactFlowId`'s published length maximum (`@length(max 500)`).
const MAX_CONTACT_FLOW_ID: usize = 500;

/// `TagValue`'s published length maximum (`@length(min 0, max 256)`).
const MAX_TAG_VALUE: usize = 256;

/// The length of one id in the oversize row whose ids are LONG: well inside `CampaignId`'s
/// own published length maximum (`@length(min 0, max 256)`, which every entry of a batch is
/// held to), and chosen so the whole batch of them renders into thousands of characters and
/// still inside what one clause of this service's refusal envelope quotes (the shared echo
/// bound in `service_common::validation`, 16 KiB per clause), which is what makes "the
/// clause quotes the list in full" a claim about the rendering rather than about one short
/// list.
const LONG_CAMPAIGN_ID: usize = 20;

/// `MaxResults`' published range (`@range(min 1, max 50)`).
const MAX_RESULTS_RANGE: (i64, i64) = (1, 50);

/// `DialingCapacity`'s published range (`@range(min 0.01, max 1)`, from
/// `smithy_query(query="shape", shape="DialingCapacity")`) — a bound this service DOES
/// apply, which is why it is the nested double the rows below break.
const DIALING_CAPACITY_RANGE: (f64, f64) = (0.01, 1.0);

/// The expression `InstanceId` has to satisfy, as a clause quotes it: the model's
/// `@pattern` without the `^`/`$` a whole-value pattern is written with.
const INSTANCE_ID_EXPRESSION: &str = "[a-zA-Z0-9_\\-.]*";

/// The sentence a request refused for ONE broken constraint is answered with: the envelope
/// around a clause naming the value half the class states, the member it sits at, and the
/// requirement it broke. Written out here rather than read back out of the implementation,
/// so this suite pins the wording.
fn one_error(subject: &str, requirement: &str) -> String {
    format!("1 validation error detected: {subject} failed to satisfy constraint: {requirement}")
}

/// The same envelope over SEVERAL clauses, which is how one request breaking more than one
/// constraint is stated: the count, then the clauses in the order the walk found the
/// members, joined by `; `.
fn several_errors(clauses: &[(&str, &str)]) -> String {
    let stated: Vec<String> = clauses
        .iter()
        .map(|(subject, requirement)| {
            format!("{subject} failed to satisfy constraint: {requirement}")
        })
        .collect();
    format!(
        "{} validation errors detected: {}",
        clauses.len(),
        stated.join("; ")
    )
}

/// A minimal `CreateCampaign` body: every member the model marks required, and nothing
/// else. The rows below replace one member of it at a time.
fn create_body(name: &str, instance: &str) -> Value {
    json!({
        "name": name,
        "connectInstanceId": instance,
        "dialerConfig": {"progressiveDialerConfig": {"bandwidthAllocation": 0.5}},
        "outboundCallConfig": {"connectContactFlowId": "flow-1"},
    })
}

/// Register `INSTANCE` with the account and create one campaign for it, handing back the
/// minted campaign id — the positive control for every refusal below.
async fn a_campaign(router: &Harness, name: &str) -> String {
    router.register_connect_instance(REGION, INSTANCE);
    let created = call(router, REGION, "PUT", "/campaigns", &create_body(name, INSTANCE)).await;
    assert_eq!(
        created.status, 200,
        "case \"create campaign {name}\": expected HTTP 200, got {} {}",
        created.status, created.body
    );
    created.body["id"]
        .as_str()
        .unwrap_or_else(|| panic!("case \"create campaign {name}\": no id in {}", created.body))
        .to_string()
}

/// One refused request: what to send, and the clause it must be answered with.
struct Row {
    case: &'static str,
    method: &'static str,
    path: String,
    body: Value,
    /// The whole expected message, envelope included.
    expected: String,
    /// The value the request carried for the member that broke its constraint, which the
    /// message must NOT contain; empty where the submitted value is itself empty and so
    /// cannot be looked for.
    submitted: String,
}

#[tokio::test]
async fn a_constraint_clause_states_the_member_and_the_requirement_and_echoes_no_scalar() {
    let router = router();
    let control = a_campaign(&router, "control").await;

    let long_name = "x".repeat(MAX_NAME + 1);
    let long_instance = "a".repeat(MAX_INSTANCE_ID + 1);
    let long_token = "A".repeat(MAX_NEXT_TOKEN + 1);
    let long_flow = "f".repeat(MAX_CONTACT_FLOW_ID + 1);
    let long_tag_value = "v".repeat(MAX_TAG_VALUE + 1);

    let rows: Vec<Row> = vec![
        Row {
            case: "a name past its length maximum",
            method: "PUT",
            path: "/campaigns".to_string(),
            body: create_body(&long_name, INSTANCE),
            expected: one_error(
                "Value at 'name'",
                &format!("Member must have length less than or equal to {MAX_NAME}"),
            ),
            submitted: long_name.clone(),
        },
        Row {
            case: "a name below its length minimum",
            method: "PUT",
            path: "/campaigns".to_string(),
            body: create_body("", INSTANCE),
            expected: one_error(
                "Value at 'name'",
                "Member must have length greater than or equal to 1",
            ),
            // The submitted value is the empty string, which every message contains.
            submitted: String::new(),
        },
        Row {
            case: "a required member the request omits",
            method: "PUT",
            path: "/campaigns".to_string(),
            body: json!({
                "connectInstanceId": INSTANCE,
                "dialerConfig": {"progressiveDialerConfig": {"bandwidthAllocation": 0.5}},
                "outboundCallConfig": {"connectContactFlowId": "flow-1"},
            }),
            expected: one_error("Value null at 'name'", "Member must not be null"),
            submitted: String::new(),
        },
        Row {
            case: "an instance id outside its pattern",
            method: "PUT",
            path: "/campaigns".to_string(),
            body: create_body("fine", "not a legal instance id"),
            expected: one_error(
                "Value at 'connectInstanceId'",
                &format!(
                    "Member must satisfy regular expression pattern: {INSTANCE_ID_EXPRESSION}"
                ),
            ),
            submitted: "not a legal instance id".to_string(),
        },
        Row {
            case: "an instance id past its length maximum",
            method: "PUT",
            path: "/campaigns".to_string(),
            body: create_body("fine", &long_instance),
            expected: one_error(
                "Value at 'connectInstanceId'",
                &format!("Member must have length less than or equal to {MAX_INSTANCE_ID}"),
            ),
            submitted: long_instance.clone(),
        },
        Row {
            case: "a page size below its range minimum",
            method: "POST",
            path: "/campaigns-summary".to_string(),
            body: json!({"maxResults": 0}),
            expected: one_error(
                "Value at 'maxResults'",
                &format!(
                    "Member must have value greater than or equal to {}",
                    MAX_RESULTS_RANGE.0
                ),
            ),
            submitted: "0".to_string(),
        },
        Row {
            case: "a page size past its range maximum",
            method: "POST",
            path: "/campaigns-summary".to_string(),
            body: json!({"maxResults": MAX_RESULTS_RANGE.1 + 1}),
            expected: one_error(
                "Value at 'maxResults'",
                &format!(
                    "Member must have value less than or equal to {}",
                    MAX_RESULTS_RANGE.1
                ),
            ),
            submitted: (MAX_RESULTS_RANGE.1 + 1).to_string(),
        },
        Row {
            case: "a page token past its length maximum",
            method: "POST",
            path: "/campaigns-summary".to_string(),
            body: json!({"nextToken": long_token}),
            expected: one_error(
                "Value at 'nextToken'",
                &format!("Member must have length less than or equal to {MAX_NEXT_TOKEN}"),
            ),
            submitted: long_token.clone(),
        },
        Row {
            case: "a double nested inside a union, past its range maximum",
            method: "PUT",
            path: "/campaigns".to_string(),
            body: json!({
                "name": "fine",
                "connectInstanceId": INSTANCE,
                "dialerConfig": {"progressiveDialerConfig": {
                    "bandwidthAllocation": 0.5,
                    "dialingCapacity": 2.5,
                }},
                "outboundCallConfig": {"connectContactFlowId": "flow-1"},
            }),
            expected: one_error(
                "Value at 'dialerConfig.progressiveDialerConfig.dialingCapacity'",
                &format!(
                    "Member must have value less than or equal to {}",
                    DIALING_CAPACITY_RANGE.1
                ),
            ),
            submitted: "2.5".to_string(),
        },
        Row {
            case: "a string nested inside a structure, past its length maximum",
            method: "PUT",
            path: "/campaigns".to_string(),
            body: json!({
                "name": "fine",
                "connectInstanceId": INSTANCE,
                "dialerConfig": {"progressiveDialerConfig": {"bandwidthAllocation": 0.5}},
                "outboundCallConfig": {"connectContactFlowId": long_flow},
            }),
            expected: one_error(
                "Value at 'outboundCallConfig.connectContactFlowId'",
                &format!("Member must have length less than or equal to {MAX_CONTACT_FLOW_ID}"),
            ),
            submitted: long_flow.clone(),
        },
        Row {
            case: "a tag value past its length maximum",
            method: "PUT",
            path: "/campaigns".to_string(),
            body: json!({
                "name": "fine",
                "connectInstanceId": INSTANCE,
                "dialerConfig": {"progressiveDialerConfig": {"bandwidthAllocation": 0.5}},
                "outboundCallConfig": {"connectContactFlowId": "flow-1"},
                "tags": {"team": long_tag_value},
            }),
            // A map entry names itself by the KEY the caller chose, which is a value the
            // request carried and is echoed as sent — the member path, not the value half.
            expected: one_error(
                "Value at 'tags.team'",
                &format!("Member must have length less than or equal to {MAX_TAG_VALUE}"),
            ),
            submitted: long_tag_value.clone(),
        },
        Row {
            case: "an emptied list member",
            method: "POST",
            path: "/campaigns-state".to_string(),
            body: json!({"campaignIds": []}),
            // A list IS stated as the value it is, because a list has a rendering of its
            // own: this is the half a scalar's clause drops.
            expected: one_error(
                "Value '[]' at 'campaignIds'",
                "Member must have length greater than or equal to 1",
            ),
            submitted: String::new(),
        },
        Row {
            case: "two members outside their constraints at once",
            method: "PUT",
            path: "/campaigns".to_string(),
            body: create_body(&long_name, "not a legal instance id"),
            expected: several_errors(&[
                (
                    "Value at 'name'",
                    &format!("Member must have length less than or equal to {MAX_NAME}"),
                ),
                (
                    "Value at 'connectInstanceId'",
                    &format!(
                        "Member must satisfy regular expression pattern: {INSTANCE_ID_EXPRESSION}"
                    ),
                ),
            ]),
            submitted: long_name.clone(),
        },
    ];

    for row in rows {
        let case = row.case;
        let refused = call(&router, REGION, row.method, &row.path, &row.body).await;
        assert_eq!(
            refused.status, 400,
            "case {case:?}: expected HTTP 400 for a value outside its constraint, got {} {}",
            refused.status, refused.body
        );
        assert!(
            refused.error_code.ends_with("ValidationException"),
            "case {case:?}: expected ValidationException, got {:?} on HTTP {}",
            refused.error_code,
            refused.status
        );
        assert_eq!(
            refused.message(),
            row.expected,
            "case {case:?}: expected {:?}, got {}",
            row.expected,
            refused.body
        );
        if !row.submitted.is_empty() {
            assert!(
                !refused.message().contains(&row.submitted),
                "case {case:?}: the clause must not echo the value the caller submitted, got \
                 {}",
                refused.body
            );
        }
    }

    // None of those refusals stored or changed anything: the control campaign still reads
    // back, and it is still the only one.
    let described = call(
        &router,
        REGION,
        "GET",
        &format!("/campaigns/{control}"),
        &Value::Null,
    )
    .await;
    assert_eq!(
        described.status, 200,
        "case \"the control campaign after every refusal\": expected HTTP 200, got {} {}",
        described.status, described.body
    );
    let listed = call(
        &router,
        REGION,
        "POST",
        "/campaigns-summary",
        &json!({"maxResults": MAX_RESULTS_RANGE.1}),
    )
    .await;
    assert_eq!(
        listed.body["campaignSummaryList"]
            .as_array()
            .map_or(0, Vec::len),
        1,
        "case \"the store after every refusal\": expected only the control campaign, got {}",
        listed.body
    );
}

/// `count` campaign ids no campaign carries, each well formed under `CampaignId`
/// (len 0..256, pattern `^[\S]*$`).
fn unknown_ids(count: usize) -> Vec<String> {
    (0..count).map(|index| format!("absent-{index}")).collect()
}

#[tokio::test]
async fn a_batch_up_to_the_modelled_maximum_is_processed_and_only_one_past_it_is_refused() {
    let router = router();
    let known = a_campaign(&router, "known").await;

    // Rows differ along HOW MANY ids the batch names, spanning the sizes a caller can
    // send inside the maximum: one, a handful, one short of the maximum, and the maximum
    // itself.
    let sizes = [1, 2, 26, MAX_BATCH - 1, MAX_BATCH];
    for size in sizes {
        let case = format!("a batch of {size} ids");
        // The stored campaign is asked for first, so every row carries one id that
        // resolves beside ids that do not.
        let mut ids = vec![known.clone()];
        ids.extend(unknown_ids(size - 1));
        let answer = call(
            &router,
            REGION,
            "POST",
            "/campaigns-state",
            &json!({"campaignIds": ids}),
        )
        .await;
        assert_eq!(
            answer.status, 200,
            "case {case:?}: a batch inside the modelled maximum is processed, got {} {}",
            answer.status, answer.body
        );
        let successful = answer.body["successfulRequests"]
            .as_array()
            .cloned()
            .unwrap_or_default();
        let failed = answer.body["failedRequests"]
            .as_array()
            .cloned()
            .unwrap_or_default();
        assert_eq!(
            (successful.len(), failed.len()),
            (1, size - 1),
            "case {case:?}: expected one answer per requested id, got {} successful and {} \
             failed for {size} ids",
            successful.len(),
            failed.len()
        );
        assert_eq!(
            successful[0]["campaignId"].as_str(),
            Some(known.as_str()),
            "case {case:?}: expected the stored campaign to be served, got {}",
            answer.body
        );
        assert_eq!(
            successful[0]["state"].as_str(),
            Some("Initialized"),
            "case {case:?}: expected the stored run state, got {}",
            answer.body
        );
    }

    // One id past the maximum is refused, and the clause states the list the caller
    // submitted as the value of the member that broke it. The rows differ along how long
    // that list renders: ids of a handful of characters, and ids long enough that the whole
    // batch renders into thousands of characters, which the clause still quotes in full.
    let refusal_rows: Vec<(String, Vec<String>)> = vec![
        (
            format!("a batch of {} short ids", MAX_BATCH + 1),
            unknown_ids(MAX_BATCH + 1),
        ),
        (
            format!("a batch of {} longer ids", MAX_BATCH + 1),
            (0..=MAX_BATCH)
                .map(|index| format!("{index:0>width$}", width = LONG_CAMPAIGN_ID))
                .collect(),
        ),
    ];

    for (case, over) in refusal_rows {
        let refused = call(
            &router,
            REGION,
            "POST",
            "/campaigns-state",
            &json!({"campaignIds": over}),
        )
        .await;
        assert_eq!(
            refused.status, 400,
            "case {case:?}: expected HTTP 400 one id past the modelled maximum, got {} {}",
            refused.status, refused.body
        );
        assert!(
            refused.error_code.ends_with("ValidationException"),
            "case {case:?}: expected ValidationException, got {:?}",
            refused.error_code
        );
        assert_eq!(
            refused.message(),
            one_error(
                &format!("Value '[{}]' at 'campaignIds'", over.join(", ")),
                &format!("Member must have length less than or equal to {MAX_BATCH}"),
            ),
            "case {case:?}: expected the submitted ids stated as the list they are, got a \
             message of {} characters",
            refused.message().chars().count()
        );
        assert!(
            refused.body.get("successfulRequests").is_none()
                && refused.body.get("failedRequests").is_none(),
            "case {case:?}: a refused batch answers no state at all, got {}",
            refused.message()
        );
    }

    // And the refusal changed nothing: the stored campaign still reads back on its own.
    let after = call(
        &router,
        REGION,
        "POST",
        "/campaigns-state",
        &json!({"campaignIds": [known]}),
    )
    .await;
    assert_eq!(
        (
            after.status,
            after.body["successfulRequests"]
                .as_array()
                .map_or(0, Vec::len)
        ),
        (200, 1),
        "case \"the stored campaign after the refusal\": expected it to read back, got {} {}",
        after.status,
        after.body
    );
}

/// A `bandwidthAllocation` is taken whatever value it carries, and the value is stored and
/// reported back exactly as sent: the bound the model declares for it is not one the
/// service applies to a request.
///
/// The rows differ along where the value sits relative to that modelled range — inside it,
/// on each published bound, and past the maximum, at a value a caller might plausibly send
/// and at a far larger one — and along which dialer config carries it, because the member
/// is the same `BandwidthAllocation` shape in both.
#[tokio::test]
async fn a_bandwidth_allocation_outside_the_modelled_range_is_taken_and_echoed() {
    let router = router();
    router.register_connect_instance(REGION, INSTANCE);

    // `BandwidthAllocation`'s published range is (min 0, max 1), from
    // `smithy_query(query="shape", shape="BandwidthAllocation")`: the rows cover both
    // bounds and the far side of the maximum.
    let allocations: &[(&str, f64)] = &[
        ("the range minimum", 0.0),
        ("inside the range", 0.5),
        ("the range maximum", 1.0),
        ("just past the range maximum", 1.5),
        ("far past the range maximum", 42.0),
    ];

    for (variant, variant_case) in [
        ("progressiveDialerConfig", "a progressive dialer"),
        ("predictiveDialerConfig", "a predictive dialer"),
    ] {
        for (allocation_case, allocation) in allocations {
            let case = format!("{variant_case}, {allocation_case}");
            let sent = json!({variant: {"bandwidthAllocation": allocation}});
            let created = call(
                &router,
                REGION,
                "PUT",
                "/campaigns",
                &json!({
                    "name": format!("allocation-{variant}-{allocation}"),
                    "connectInstanceId": INSTANCE,
                    "dialerConfig": sent,
                    "outboundCallConfig": {"connectContactFlowId": "flow-1"},
                }),
            )
            .await;
            assert_eq!(
                created.status, 200,
                "case {case:?}: expected the create to be served, got {} {}",
                created.status, created.body
            );
            let id = created.body["id"]
                .as_str()
                .unwrap_or_else(|| panic!("case {case:?}: no minted id in {}", created.body))
                .to_string();

            // Stored verbatim: the describe reports the value the request carried, neither
            // clamped to the modelled maximum nor rounded.
            let described = call(
                &router,
                REGION,
                "GET",
                &format!("/campaigns/{id}"),
                &Value::Null,
            )
            .await;
            assert_eq!(
                (
                    described.status,
                    described
                        .body
                        .pointer(&format!(
                            "/campaign/dialerConfig/{variant}/bandwidthAllocation"
                        ))
                        .and_then(Value::as_f64)
                ),
                (200, Some(*allocation)),
                "case {case:?}: expected the stored allocation to be reported as sent, \
                 got {} {}",
                described.status,
                described.body
            );
        }
    }
}
