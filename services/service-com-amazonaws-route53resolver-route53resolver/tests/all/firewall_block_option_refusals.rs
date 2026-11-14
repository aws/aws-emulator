// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the refusal a DNS Firewall rule write gets when it
//! leaves out a block option the rule it describes requires.
//!
//! The rule under test: a rule that blocks needs the options that say HOW it
//! blocks — `BlockResponse$doc`: "this setting is required if the rule action
//! setting is BLOCK", and each `BlockOverride*` member "is required if the
//! BlockResponse setting is OVERRIDE" — and a write that leaves one out is
//! refused `ValidationException` with ONE service-coded sentence for that fault,
//! naming the OPTION in the service's own spelling (its words, upper case,
//! separated by spaces) and nothing else from the request. The same sentence is
//! stated for each of the four options, by every operation that writes a rule.
//!
//! The conditional requirement is recorded behavior: `BlockResponse` and the
//! three `BlockOverride*` members are plain optional members of
//! `CreateFirewallRuleRequest`
//! (`smithy_query(query="operation",operation="CreateFirewallRule")`), so
//! nothing in the model expresses "required when".
//!
//! The table spans what the rule generalises over: which of the four options the
//! write leaves out, and the operation that writes the rule — the single create,
//! one entry of a batch create (where the same code and sentence are reported in
//! the response body), and the update that moves a rule onto `BLOCK`.
//!
//! Every case goes over the wire the service actually speaks (awsJson1_1,
//! `POST /` with `X-Amz-Target: Route53Resolver.<OperationName>`), through the
//! same router the emulator mounts, so what is asserted is the HTTP behaviour
//! and not a handler called directly. The transport helpers are the shared ones
//! in [`super::harness`].

use serde_json::{json, Value};

use super::harness::{call, emulator, error_type, ok, string_member, Emulator};

/// One row: the block options the write states, the option it therefore leaves
/// out, and the sentence the service refuses it with.
struct Row {
    case: &'static str,
    /// The block options the request states beside `Action: BLOCK`.
    stated: fn() -> Value,
    sentence: &'static str,
}

/// The sentence a write that leaves out the block response is refused with,
/// written out here rather than built from the emulator's own constants so the
/// wording itself is what this suite pins.
const NO_BLOCK_RESPONSE: &str = "[RSLVR-02016] Your block option BLOCK RESPONSE is not valid.";

/// The same sentence for each override option a `BlockResponse` of `OVERRIDE`
/// requires, naming that option as the service spells it.
const NO_OVERRIDE_DOMAIN: &str =
    "[RSLVR-02016] Your block option BLOCK OVERRIDE DOMAIN is not valid.";
const NO_OVERRIDE_DNS_TYPE: &str =
    "[RSLVR-02016] Your block option BLOCK OVERRIDE DNS TYPE is not valid.";
const NO_OVERRIDE_TTL: &str = "[RSLVR-02016] Your block option BLOCK OVERRIDE TTL is not valid.";

/// Every block option a blocking rule may leave out, each with the sentence the
/// service states for it.
const ROWS: &[Row] = &[
    Row {
        case: "BLOCK with no block response at all",
        stated: || json!({}),
        sentence: NO_BLOCK_RESPONSE,
    },
    Row {
        case: "OVERRIDE with no override target",
        stated: || {
            json!({
                "BlockResponse": "OVERRIDE",
                "BlockOverrideDnsType": "CNAME",
                "BlockOverrideTtl": 60,
            })
        },
        sentence: NO_OVERRIDE_DOMAIN,
    },
    Row {
        case: "OVERRIDE with no record type",
        stated: || {
            json!({
                "BlockResponse": "OVERRIDE",
                "BlockOverrideDomain": "override.example.com",
                "BlockOverrideTtl": 60,
            })
        },
        sentence: NO_OVERRIDE_DNS_TYPE,
    },
    Row {
        case: "OVERRIDE with no cache lifetime",
        stated: || {
            json!({
                "BlockResponse": "OVERRIDE",
                "BlockOverrideDomain": "override.example.com",
                "BlockOverrideDnsType": "CNAME",
            })
        },
        sentence: NO_OVERRIDE_TTL,
    },
];

/// Create one rule group and answer the id Route 53 Resolver minted for it.
async fn rule_group(router: &Emulator, name: &str) -> String {
    let created = ok(
        router,
        "CreateFirewallRuleGroup",
        json!({ "CreatorRequestId": format!("group-{name}"), "Name": name }),
    )
    .await;
    string_member(
        created
            .get("FirewallRuleGroup")
            .expect("the create answers with the FirewallRuleGroup"),
        "Id",
    )
}

/// Create one domain list and answer the id Route 53 Resolver minted for it.
async fn domain_list(router: &Emulator, name: &str) -> String {
    let created = ok(
        router,
        "CreateFirewallDomainList",
        json!({ "CreatorRequestId": format!("list-{name}"), "Name": name }),
    )
    .await;
    string_member(
        created
            .get("FirewallDomainList")
            .expect("the create answers with the FirewallDomainList"),
        "Id",
    )
}

/// The rule entry a row states: the group, the match source, the name, the
/// priority, `Action: BLOCK`, and the block options the row carries.
fn entry(row: &Row, group: &str, list: &str, request_id: &str, priority: i64) -> Value {
    let mut body = json!({
        "CreatorRequestId": request_id,
        "FirewallRuleGroupId": group,
        "FirewallDomainListId": list,
        "Name": request_id,
        "Priority": priority,
        "Action": "BLOCK",
    });
    let object = body.as_object_mut().expect("the entry is an object");
    for (member, value) in (row.stated)()
        .as_object()
        .expect("the row states its options as an object")
        .clone()
    {
        object.insert(member, value);
    }
    body
}

/// Every rule the group holds.
async fn listed_rules(router: &Emulator, group: &str) -> Vec<Value> {
    ok(
        router,
        "ListFirewallRules",
        json!({ "FirewallRuleGroupId": group }),
    )
    .await
    .get("FirewallRules")
    .and_then(Value::as_array)
    .cloned()
    .unwrap_or_default()
}

/// `CreateFirewallRule` refuses a rule that leaves out a block option it
/// requires with the coded sentence naming that option, and writes nothing.
#[tokio::test]
async fn a_create_that_leaves_out_a_required_block_option_is_refused_with_its_coded_sentence() {
    let router = emulator();
    let group = rule_group(&router, "block-option").await;
    let list = domain_list(&router, "block-option").await;

    for (index, row) in ROWS.iter().enumerate()
    {
        let priority = 100 + i64::try_from(index).expect("a small index") * 10;
        let (status, parsed) = call(
            &router,
            "CreateFirewallRule",
            entry(
                row,
                &group,
                &list,
                &format!("block-option-{index}"),
                priority,
            ),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {:?}: the create must be refused with HTTP 400, got {status}: {parsed}",
            row.case
        );
        assert!(
            error_type(&parsed).contains("ValidationException"),
            "case {:?}: the refusal must name ValidationException, got {parsed}",
            row.case
        );
        let message = string_member(&parsed, "Message");
        assert_eq!(
            message, row.sentence,
            "case {:?}: expected Message {:?}, got {message:?}",
            row.case, row.sentence
        );
    }

    assert!(
        listed_rules(&router, &group).await.is_empty(),
        "a refused create must add no rule to the group"
    );
}

/// One entry of a batch create is decided on the same grounds and reports the
/// same code and sentence in the response body, while the entries beside it are
/// created.
#[tokio::test]
async fn a_batch_entry_that_leaves_out_a_required_block_option_reports_the_same_sentence() {
    let router = emulator();
    let group = rule_group(&router, "batch-block-option").await;
    let good_list = domain_list(&router, "batch-block-option-good").await;

    let mut entries: Vec<Value> = Vec::new();
    let mut expected: Vec<&str> = Vec::new();
    for (index, row) in ROWS.iter().enumerate() {
        let list = domain_list(&router, &format!("batch-block-option-{index}")).await;
        entries.push(entry(
            row,
            &group,
            &list,
            &format!("batch-block-option-{index}"),
            100 + i64::try_from(index).expect("a small index") * 10,
        ));
        expected.push(row.sentence);
    }
    // One entry the service admits, so the refusals are about the entries that
    // left an option out and not about the batch.
    entries.push(json!({
        "CreatorRequestId": "batch-block-option-good",
        "FirewallRuleGroupId": group,
        "FirewallDomainListId": good_list,
        "Name": "batch-block-option-good",
        "Priority": 900,
        "Action": "BLOCK",
        "BlockResponse": "NXDOMAIN",
    }));

    let body = ok(
        &router,
        "BatchCreateFirewallRule",
        json!({ "CreateFirewallRuleEntries": entries }),
    )
    .await;
    let errors = body
        .get("CreateErrors")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    assert_eq!(
        errors.len(),
        expected.len(),
        "the batch must report one error per entry that left an option out, got {body}"
    );
    for (sentence, error) in expected.iter().zip(errors.iter()) {
        assert_eq!(
            string_member(error, "Code"),
            "ValidationException",
            "the error must carry the code the single create answers with, got {error}"
        );
        assert_eq!(
            &string_member(error, "Message"),
            sentence,
            "the error must state the sentence the single create answers with, got {error}"
        );
    }
    let created = listed_rules(&router, &group).await;
    assert_eq!(
        created.len(),
        1,
        "only the entry the service admits may be created, got {created:?}"
    );
}

/// The update path answers the same sentence: a rule moved onto `BLOCK` without
/// the block response it then requires is refused, and the rule keeps the action
/// it had.
#[tokio::test]
async fn an_update_onto_block_without_the_block_response_is_refused_with_the_same_sentence() {
    let router = emulator();
    let group = rule_group(&router, "update-block-option").await;
    let list = domain_list(&router, "update-block-option").await;

    ok(
        &router,
        "CreateFirewallRule",
        json!({
            "CreatorRequestId": "update-block-option",
            "FirewallRuleGroupId": group,
            "FirewallDomainListId": list,
            "Name": "update-block-option",
            "Priority": 100,
            "Action": "ALERT",
        }),
    )
    .await;

    let (status, parsed) = call(
        &router,
        "UpdateFirewallRule",
        json!({
            "FirewallRuleGroupId": group,
            "FirewallDomainListId": list,
            "Action": "BLOCK",
        }),
    )
    .await;
    assert_eq!(
        status, 400,
        "the update must be refused with HTTP 400, got {status}: {parsed}"
    );
    assert!(
        error_type(&parsed).contains("ValidationException"),
        "the refusal must name ValidationException, got {parsed}"
    );
    assert_eq!(
        string_member(&parsed, "Message"),
        NO_BLOCK_RESPONSE,
        "the update must state the sentence the create answers with, got {parsed}"
    );

    let rules = listed_rules(&router, &group).await;
    let rule = rules
        .first()
        .unwrap_or_else(|| panic!("the group must still hold its one rule, got {rules:?}"));
    assert_eq!(
        string_member(rule, "Action"),
        "ALERT",
        "the refused update must leave the rule's Action alone, got {rule}"
    );

    // The same update WITH the response it requires is served, so what the
    // refusal was about is the missing option.
    let updated = ok(
        &router,
        "UpdateFirewallRule",
        json!({
            "FirewallRuleGroupId": group,
            "FirewallDomainListId": list,
            "Action": "BLOCK",
            "BlockResponse": "NODATA",
        }),
    )
    .await;
    let rule = updated
        .get("FirewallRule")
        .unwrap_or_else(|| panic!("the update answers the rule, got {updated}"));
    assert_eq!(
        string_member(rule, "Action"),
        "BLOCK",
        "the update that states the option must be served, got {updated}"
    );
}
