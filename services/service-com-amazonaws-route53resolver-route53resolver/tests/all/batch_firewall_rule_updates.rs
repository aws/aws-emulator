// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `BatchUpdateFirewallRule`: the batch update of DNS
//! Firewall rules, which decides EVERY entry on its own.
//!
//! The rule under test is the one the operation's two output members state: an
//! entry names one rule of a rule group by the source that rule matches on,
//! replaces exactly the members it carries and leaves the rest of the rule as
//! stored, and has to arrive at settings the service admits — so one call can
//! succeed in part, an entry that is refused neither fails the call nor stops
//! the entries around it from being applied, and a batch carrying more entries
//! than one request may is refused WHOLE.
//!
//! Every case goes over the wire the service actually speaks (awsJson1_1,
//! `POST /` with `X-Amz-Target: Route53Resolver.<OperationName>`), through the
//! same router the emulator mounts, so what is asserted is the HTTP behaviour
//! and not a handler called directly. The transport helpers are the shared ones
//! in [`super::harness`].

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_route53resolver_route53resolver::state::{
    FIREWALL_RULE_UPDATE_BATCH_ENTRY_CAP, RSLVR_CODE_FIREWALL_RULE_BATCH_TOO_LARGE,
};

use super::harness::{call, emulator, error_type, ok, parse_time, string_member};

/// Create one rule group and answer the id Route 53 Resolver minted for it.
async fn rule_group(router: &MultiServiceRouter, name: &str) -> String {
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
async fn domain_list(router: &MultiServiceRouter, name: &str) -> String {
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

/// Create one rule from the members `members` states and answer it as the
/// create reported it.
async fn create_rule(router: &MultiServiceRouter, name: &str, members: Value) -> Value {
    let mut request = members;
    let object = request
        .as_object_mut()
        .expect("a rule's members are stated as an object");
    object.insert("Name".to_string(), json!(name));
    object.insert("CreatorRequestId".to_string(), json!(format!("rule-{name}")));
    let created = ok(router, "CreateFirewallRule", request).await;
    created
        .get("FirewallRule")
        .cloned()
        .unwrap_or_else(|| panic!("the create of {name:?} answers with the FirewallRule"))
}

/// The list member `member` of a response, which the batch update reports for
/// every call — empty or not.
fn list_member(case: &str, body: &Value, member: &str) -> Vec<Value> {
    body.get(member)
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_else(|| {
            panic!("case {case:?}: the response must carry {member} as a list, got {body}")
        })
}

/// The rules the named group holds, as `ListFirewallRules` reports them.
async fn listed_rules(router: &MultiServiceRouter, group_id: &str) -> Vec<Value> {
    let listed = ok(
        router,
        "ListFirewallRules",
        json!({ "FirewallRuleGroupId": group_id }),
    )
    .await;
    listed
        .get("FirewallRules")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
}

/// The one rule of `group_id` named `name`, as `ListFirewallRules` reports it.
async fn listed_rule(router: &MultiServiceRouter, group_id: &str, name: &str) -> Value {
    let rules = listed_rules(router, group_id).await;
    rules
        .iter()
        .find(|rule| string_member(rule, "Name") == name)
        .cloned()
        .unwrap_or_else(|| panic!("the group must still hold the rule {name:?}, got {rules:?}"))
}

/// One row of the entry table: the entry a batch carries, and what the call must
/// answer for it — either the rule it updated or the error code it was refused
/// with.
struct EntryCase {
    case: &'static str,
    /// What the entry carries beside `FirewallRuleGroupId`.
    members: Value,
    /// The `Code` the refusal must report, or `None` for an entry the call must
    /// apply.
    ///
    /// The codes are the modelled shape names of this service's exceptions
    /// (`smithy_query(query="closure",operation="UpdateFirewallRule")`), which
    /// is what the single update answers with on the wire for the same entry.
    code: Option<&'static str>,
    /// The `Name` of the rule the entry changes, for a row that is applied.
    updates: Option<&'static str>,
}

/// A batch update applies each entry on its own: it applies every entry it can,
/// refuses the rest with the code and the sentence the single update answers for
/// that entry, and answers HTTP 200 either way.
///
/// The table spans what the rule generalises over — the entry's outcome (applied
/// or refused), the kind of fault (an unresolvable rule group, a match source no
/// rule of the group matches, no match source at all, two match sources at once,
/// a match source more than one rule of the group matches, a setting the
/// resulting rule would lack, a priority another rule holds, an action a DNS
/// Firewall Advanced rule may not take, a change to what the rule matches, a
/// `Name` outside the modelled pattern), the member it changes, and the position
/// of the refused entries among the applied ones (first, between two applied,
/// last).
#[tokio::test]
async fn batch_update_decides_every_entry_on_its_own() {
    let router = emulator();
    let group = rule_group(&router, "batch-update-group").await;
    let list_one = domain_list(&router, "batch-update-list-one").await;
    let list_two = domain_list(&router, "batch-update-list-two").await;
    let shared_list = domain_list(&router, "batch-update-list-shared").await;

    create_rule(
        &router,
        "rule-one",
        json!({
            "FirewallRuleGroupId": group,
            "FirewallDomainListId": list_one,
            "Priority": 100,
            "Action": "ALERT",
        }),
    )
    .await;
    create_rule(
        &router,
        "rule-two",
        json!({
            "FirewallRuleGroupId": group,
            "FirewallDomainListId": list_two,
            "Priority": 200,
            "Action": "BLOCK",
            "BlockResponse": "NXDOMAIN",
        }),
    )
    .await;
    let detector = create_rule(
        &router,
        "rule-three",
        json!({
            "FirewallRuleGroupId": group,
            "DnsThreatProtection": "DGA",
            "ConfidenceThreshold": "HIGH",
            "Priority": 300,
            "Action": "BLOCK",
            "BlockResponse": "NODATA",
        }),
    )
    .await;
    let detector_id = string_member(&detector, "FirewallThreatProtectionId");
    assert!(
        !detector_id.is_empty(),
        "a DNS Firewall Advanced rule reports the id it is named by, got {detector}"
    );
    // Two rules of one group may match the same domain list for different query
    // types, which is what makes `Qtype` part of how an entry names one of them.
    create_rule(
        &router,
        "rule-four",
        json!({
            "FirewallRuleGroupId": group,
            "FirewallDomainListId": shared_list,
            "Priority": 400,
            "Action": "ALERT",
            "Qtype": "A",
        }),
    )
    .await;
    create_rule(
        &router,
        "rule-five",
        json!({
            "FirewallRuleGroupId": group,
            "FirewallDomainListId": shared_list,
            "Priority": 500,
            "Action": "ALERT",
            "Qtype": "AAAA",
        }),
    )
    .await;

    let cases = vec![
        EntryCase {
            case: "an unresolvable rule group",
            members: json!({
                "FirewallRuleGroupId": "rslvr-frg-ffffffffffffffff",
                "FirewallDomainListId": list_one,
                "Priority": 150,
            }),
            code: Some("ResourceNotFoundException"),
            updates: None,
        },
        EntryCase {
            case: "a priority the entry moves",
            members: json!({
                "FirewallRuleGroupId": group,
                "FirewallDomainListId": list_one,
                "Priority": 150,
            }),
            code: None,
            updates: Some("rule-one"),
        },
        EntryCase {
            case: "a match source no rule of the group matches",
            members: json!({
                "FirewallRuleGroupId": group,
                "FirewallDomainListId": "rslvr-fdl-ffffffffffffffff",
                "Priority": 160,
            }),
            code: Some("ResourceNotFoundException"),
            updates: None,
        },
        EntryCase {
            case: "an action the rule takes instead",
            members: json!({
                "FirewallRuleGroupId": group,
                "FirewallDomainListId": list_two,
                "Action": "ALERT",
            }),
            code: None,
            updates: Some("rule-two"),
        },
        EntryCase {
            case: "no match source at all",
            members: json!({
                "FirewallRuleGroupId": group,
                "Priority": 170,
            }),
            code: Some("ValidationException"),
            updates: None,
        },
        EntryCase {
            case: "two match sources at once",
            members: json!({
                "FirewallRuleGroupId": group,
                "FirewallDomainListId": list_one,
                "FirewallThreatProtectionId": detector_id,
                "Priority": 180,
            }),
            code: Some("ValidationException"),
            updates: None,
        },
        EntryCase {
            case: "BLOCK without the BlockResponse it requires",
            members: json!({
                "FirewallRuleGroupId": group,
                "FirewallDomainListId": list_one,
                "Action": "BLOCK",
            }),
            code: Some("ValidationException"),
            updates: None,
        },
        EntryCase {
            case: "a priority another rule of the group holds",
            members: json!({
                "FirewallRuleGroupId": group,
                "FirewallDomainListId": list_two,
                "Priority": 300,
            }),
            code: Some("ValidationException"),
            updates: None,
        },
        EntryCase {
            case: "ALLOW on a DNS Firewall Advanced rule",
            members: json!({
                "FirewallRuleGroupId": group,
                "FirewallThreatProtectionId": detector_id,
                "Action": "ALLOW",
            }),
            code: Some("ValidationException"),
            updates: None,
        },
        EntryCase {
            case: "another match source than the rule's",
            members: json!({
                "FirewallRuleGroupId": group,
                "FirewallThreatProtectionId": detector_id,
                "DnsThreatProtection": "DNS_TUNNELING",
            }),
            code: Some("ValidationException"),
            updates: None,
        },
        EntryCase {
            case: "a Name of digits alone",
            members: json!({
                "FirewallRuleGroupId": group,
                "FirewallDomainListId": list_one,
                "Name": "1234567890",
            }),
            code: Some("ValidationException"),
            updates: None,
        },
        EntryCase {
            case: "the query type that tells two rules apart",
            members: json!({
                "FirewallRuleGroupId": group,
                "FirewallDomainListId": shared_list,
                "Qtype": "AAAA",
                "Priority": 550,
            }),
            code: None,
            updates: Some("rule-five"),
        },
        EntryCase {
            case: "a match source more than one rule of the group matches",
            members: json!({
                "FirewallRuleGroupId": group,
                "FirewallDomainListId": shared_list,
                "Priority": 560,
            }),
            code: Some("ValidationException"),
            updates: None,
        },
    ];

    // The cap on what one call may carry is a cap on the CALL, so a table wider
    // than it is stated over as many calls as it takes; each call applies its
    // own entries in order, against the store the call before it left.
    let mut updated: Vec<Value> = Vec::new();
    let mut errors: Vec<Value> = Vec::new();
    for chunk in cases.chunks(FIREWALL_RULE_UPDATE_BATCH_ENTRY_CAP - 1) {
        let entries: Vec<Value> = chunk.iter().map(|case| case.members.clone()).collect();
        let (status, body) = call(
            &router,
            "BatchUpdateFirewallRule",
            json!({ "UpdateFirewallRuleEntries": entries }),
        )
        .await;
        assert_eq!(
            status, 200,
            "a batch that applies some entries and refuses others must answer HTTP 200, \
             got {status}: {body}"
        );
        updated.extend(list_member("the mixed batch", &body, "UpdatedFirewallRules"));
        errors.extend(list_member("the mixed batch", &body, "UpdateErrors"));
    }

    let expected_updated: Vec<&str> = cases.iter().filter_map(|case| case.updates).collect();
    let reported_updated: Vec<String> = updated
        .iter()
        .map(|rule| string_member(rule, "Name"))
        .collect();
    assert_eq!(
        reported_updated, expected_updated,
        "the batch must report exactly the rules it updated, in the order the request \
         stated them: expected {expected_updated:?}, got {reported_updated:?} from {updated:?}"
    );

    let expected_refused: Vec<(&str, &str)> = cases
        .iter()
        .filter_map(|case| case.code.map(|code| (case.case, code)))
        .collect();
    assert_eq!(
        errors.len(),
        expected_refused.len(),
        "the batch must report one error per entry it could not apply: expected \
         {expected_refused:?}, got {errors:?}"
    );
    for ((case, code), error) in expected_refused.iter().zip(errors.iter()) {
        // "The firewall rule entry that caused the error": the entry the request
        // stated, so a caller can tell which of its entries this error is about.
        let entry = error
            .get("FirewallRule")
            .unwrap_or_else(|| panic!("case {case:?}: the error must echo its entry, got {error}"));
        assert_eq!(
            entry.get("FirewallRuleGroupId"),
            cases
                .iter()
                .find(|row| row.case == *case)
                .and_then(|row| row.members.get("FirewallRuleGroupId")),
            "case {case:?}: the error must echo the entry that caused it, got {error}"
        );
        assert_eq!(
            &string_member(error, "Code"),
            code,
            "case {case:?}: expected Code {code}, got {error}"
        );
        assert!(
            !string_member(error, "Message").is_empty(),
            "case {case:?}: the error must state why the entry was refused, got {error}"
        );
    }

    // Exactly the applied entries are persisted: a refused entry wrote nothing,
    // and every member no applied entry named is as it was created.
    for (case, name, member, value) in [
        ("the moved priority", "rule-one", "Priority", json!(150)),
        ("the moved priority", "rule-one", "Action", json!("ALERT")),
        ("the changed action", "rule-two", "Action", json!("ALERT")),
        ("the changed action", "rule-two", "Priority", json!(200)),
        (
            "the rule the refusals named",
            "rule-three",
            "Priority",
            json!(300),
        ),
        (
            "the rule the refusals named",
            "rule-three",
            "Action",
            json!("BLOCK"),
        ),
        (
            "the rule the Qtype did not name",
            "rule-four",
            "Priority",
            json!(400),
        ),
        (
            "the rule the Qtype named",
            "rule-five",
            "Priority",
            json!(550),
        ),
        (
            "the rule the Qtype named",
            "rule-five",
            "Qtype",
            json!("AAAA"),
        ),
    ] {
        let rule = listed_rule(&router, &group, name).await;
        assert_eq!(
            rule.get(member),
            Some(&value),
            "case {case:?}: {name}'s {member} must be {value} after the batch, got {rule}"
        );
    }
}

/// An entry replaces exactly the members it carries: every other member of the
/// rule is the one it was stored with, and the rule the call answers is the rule
/// a later read reports.
///
/// The table spans the members an entry may change — a number, a name, an enum
/// value, a query type, a block response and the override settings that go with
/// it — one at a time, each row starting from the rule the row before it left.
#[tokio::test]
async fn an_entry_changes_only_the_members_it_carries() {
    let router = emulator();
    let group = rule_group(&router, "batch-update-members-group").await;
    let list = domain_list(&router, "batch-update-members-list").await;

    create_rule(
        &router,
        "member-rule",
        json!({
            "FirewallRuleGroupId": group,
            "FirewallDomainListId": list,
            "Priority": 100,
            "Action": "BLOCK",
            "BlockResponse": "OVERRIDE",
            "BlockOverrideDomain": "example.test",
            "BlockOverrideDnsType": "CNAME",
            "BlockOverrideTtl": 60,
            "Qtype": "A",
            "FirewallDomainRedirectionAction": "INSPECT_REDIRECTION_DOMAIN",
        }),
    )
    .await;

    // Each row is (the member, the value the entry carries for it). The members
    // and their admissible values are the ones `UpdateFirewallRuleEntry`
    // declares (`smithy_query(query="shape",shape="UpdateFirewallRuleEntry")`).
    let rows = [
        ("Priority", json!(250)),
        ("Name", json!("member-rule-renamed")),
        ("BlockOverrideTtl", json!(120)),
        // The override target is a DNS name the rule reports canonical, so the
        // row carries the canonical spelling: the write is idempotent over it.
        ("BlockOverrideDomain", json!("other.example.test.")),
        ("Qtype", json!("AAAA")),
        (
            "FirewallDomainRedirectionAction",
            json!("TRUST_REDIRECTION_DOMAIN"),
        ),
        ("BlockResponse", json!("NXDOMAIN")),
        ("Action", json!("ALERT")),
    ];

    let mut stored = listed_rules(&router, &group)
        .await
        .first()
        .cloned()
        .expect("the group holds the rule the suite created");

    for (member, value) in &rows {
        let case = format!("an entry carrying {member}");
        let body = ok(
            &router,
            "BatchUpdateFirewallRule",
            json!({ "UpdateFirewallRuleEntries": [{
                "FirewallRuleGroupId": group,
                "FirewallDomainListId": list,
                member.to_string(): value,
            }] }),
        )
        .await;
        assert!(
            list_member(&case, &body, "UpdateErrors").is_empty(),
            "case {case:?}: an entry the service admits must be applied, got {body}"
        );
        let updated = list_member(&case, &body, "UpdatedFirewallRules");
        assert_eq!(
            updated.len(),
            1,
            "case {case:?}: the call must answer the one rule it updated, got {body}"
        );
        let updated = &updated[0];
        assert_eq!(
            updated.get(*member),
            Some(value),
            "case {case:?}: expected {member} to become {value}, got {updated}"
        );

        let before = stored
            .as_object()
            .expect("a rule is reported as an object")
            .clone();
        for (other, was) in &before {
            if other == member {
                continue;
            }
            if other == "ModificationTime" {
                let moved = parse_time(&case, "ModificationTime", &string_member(updated, other));
                let had = parse_time(&case, "ModificationTime", was.as_str().unwrap_or_default());
                assert!(
                    moved >= had,
                    "case {case:?}: ModificationTime must not go backwards, was {was} \
                     and is now {moved:?}"
                );
                continue;
            }
            assert_eq!(
                updated.get(other),
                Some(was),
                "case {case:?}: {other} must stay {was}, got {updated}"
            );
        }

        // The rule the call answered is the rule the store now holds.
        let read_back = listed_rules(&router, &group)
            .await
            .first()
            .cloned()
            .unwrap_or_else(|| panic!("case {case:?}: the group must still hold its one rule"));
        assert_eq!(
            &read_back, updated,
            "case {case:?}: a later read must report the rule the update answered, \
             got {read_back}"
        );
        stored = read_back;
    }
}

/// A batch past the entries one call may carry is refused WHOLE, with the
/// service's sentence for that limit: not even its one admissible entry is
/// applied.
#[tokio::test]
async fn a_batch_past_the_entry_bound_updates_nothing() {
    let router = emulator();
    let group = rule_group(&router, "batch-update-bound-group").await;
    let list = domain_list(&router, "batch-update-bound-list").await;
    create_rule(
        &router,
        "bound-rule",
        json!({
            "FirewallRuleGroupId": group,
            "FirewallDomainListId": list,
            "Priority": 100,
            "Action": "ALERT",
        }),
    )
    .await;

    // One under the cap, two under it, and the cap itself: a request has to
    // carry FEWER entries than the cap, so the call it refuses is the one that
    // reaches it.
    for (case, count, expected_status) in [
        (
            "two under the cap",
            FIREWALL_RULE_UPDATE_BATCH_ENTRY_CAP - 2,
            200,
        ),
        (
            "one under the cap",
            FIREWALL_RULE_UPDATE_BATCH_ENTRY_CAP - 1,
            200,
        ),
        ("the cap itself", FIREWALL_RULE_UPDATE_BATCH_ENTRY_CAP, 400),
        (
            "far past the cap",
            FIREWALL_RULE_UPDATE_BATCH_ENTRY_CAP * 10,
            400,
        ),
    ] {
        let before = listed_rule(&router, &group, "bound-rule").await;
        let entries: Vec<Value> = (0..count)
            .map(|index| {
                json!({
                    "FirewallRuleGroupId": group,
                    "FirewallDomainListId": list,
                    "Priority": 1_000 + index as i64,
                })
            })
            .collect();

        let (status, body) = call(
            &router,
            "BatchUpdateFirewallRule",
            json!({ "UpdateFirewallRuleEntries": entries }),
        )
        .await;
        assert_eq!(
            status, expected_status,
            "case {case:?} ({count} entries): expected HTTP {expected_status}, got {status} \
             with {body}"
        );

        if expected_status == 200 {
            assert_eq!(
                list_member(case, &body, "UpdatedFirewallRules").len(),
                count,
                "case {case:?}: a batch within the cap must apply every entry, got {body}"
            );
            assert_eq!(
                listed_rule(&router, &group, "bound-rule")
                    .await
                    .get("Priority"),
                Some(&json!(1_000 + count as i64 - 1)),
                "case {case:?}: the last entry of the batch is the one that stands"
            );
        } else {
            assert!(
                error_type(&body).contains("LimitExceededException"),
                "case {case:?}: expected LimitExceededException, got {body}"
            );
            assert_eq!(
                string_member(&body, "Message"),
                format!(
                    "[{RSLVR_CODE_FIREWALL_RULE_BATCH_TOO_LARGE}] The number of firewall rules \
                     in the update batch exceeds the allowed maximum. Provide fewer than \
                     \"{FIREWALL_RULE_UPDATE_BATCH_ENTRY_CAP}\" rules."
                ),
                "case {case:?}: the refusal must state the limit it is, got {body}"
            );
            assert_eq!(
                listed_rule(&router, &group, "bound-rule").await,
                before,
                "case {case:?}: a refused batch must apply nothing at all"
            );
        }
    }
}

/// What a rule matches on cannot be changed, and the settings that go with the
/// match source can: an entry restating a threat detector with another
/// confidence threshold is an update of that setting, and one restating it with
/// another detector is refused.
#[tokio::test]
async fn a_rules_match_source_is_fixed_and_its_settings_are_not() {
    let router = emulator();
    let group = rule_group(&router, "batch-update-source-group").await;

    // `FirewallRuleType` is a tagged union whose member is what the rule matches
    // against; a rule that matches a built-in detector carries the threshold it
    // reads at inside that member.
    let detector_rule = create_rule(
        &router,
        "union-rule",
        json!({
            "FirewallRuleGroupId": group,
            "FirewallRuleType": {
                "DnsThreatProtection": { "Value": "DGA", "ConfidenceThreshold": "HIGH" },
            },
            "Priority": 100,
            "Action": "ALERT",
        }),
    )
    .await;
    let detector_id = string_member(&detector_rule, "FirewallThreatProtectionId");

    let body = ok(
        &router,
        "BatchUpdateFirewallRule",
        json!({ "UpdateFirewallRuleEntries": [{
            "FirewallRuleGroupId": group,
            "FirewallThreatProtectionId": detector_id,
            "FirewallRuleType": {
                "DnsThreatProtection": { "Value": "DGA", "ConfidenceThreshold": "LOW" },
            },
        }] }),
    )
    .await;
    let updated = list_member("the restated threshold", &body, "UpdatedFirewallRules");
    assert_eq!(
        updated.len(),
        1,
        "restating the rule's own match source with another threshold is an update, got {body}"
    );
    assert_eq!(
        updated[0]
            .get("FirewallRuleType")
            .and_then(|rule_type| rule_type.get("DnsThreatProtection"))
            .and_then(|detector| detector.get("ConfidenceThreshold")),
        Some(&json!("LOW")),
        "case \"the restated threshold\": the threshold must be the one the entry stated, \
         got {body}"
    );

    for (case, entry_type) in [
        (
            "another detector",
            json!({ "DnsThreatProtection": { "Value": "DNS_TUNNELING", "ConfidenceThreshold": "LOW" } }),
        ),
        (
            "another kind of source",
            json!({ "FirewallAdvancedThreatCategory": { "Category": "PHISHING" } }),
        ),
    ] {
        let body = ok(
            &router,
            "BatchUpdateFirewallRule",
            json!({ "UpdateFirewallRuleEntries": [{
                "FirewallRuleGroupId": group,
                "FirewallThreatProtectionId": detector_id,
                "FirewallRuleType": entry_type,
            }] }),
        )
        .await;
        let errors = list_member(case, &body, "UpdateErrors");
        assert_eq!(
            errors.len(),
            1,
            "case {case:?}: changing what the rule matches must be refused, got {body}"
        );
        assert_eq!(
            string_member(&errors[0], "Code"),
            "ValidationException",
            "case {case:?}: expected Code ValidationException, got {body}"
        );
        assert!(
            list_member(case, &body, "UpdatedFirewallRules").is_empty(),
            "case {case:?}: a refused entry updates nothing, got {body}"
        );
    }

    // The threshold the accepted entry wrote is the one the store still holds,
    // and nothing the refused entries asked for reached it.
    let read_back = listed_rule(&router, &group, "union-rule").await;
    assert_eq!(
        read_back
            .get("FirewallRuleType")
            .and_then(|rule_type| rule_type.get("DnsThreatProtection")),
        Some(&json!({ "Value": "DGA", "ConfidenceThreshold": "LOW" })),
        "the stored rule must match the detector it was created for, at the threshold the \
         accepted entry set, got {read_back}"
    );
}

/// A rule whose provisioning has not settled cannot be updated: the entry is
/// refused with the state-transition conflict and the rule is left as it was.
#[tokio::test]
async fn a_rule_still_being_provisioned_is_not_updated() {
    let router = emulator();
    let group = rule_group(&router, "batch-update-provisioning-group").await;

    // A rule matching a partner threat feed is the one match source whose
    // provisioning is asynchronous, so such a rule begins in CREATING.
    let partner_rule = create_rule(
        &router,
        "partner-rule",
        json!({
            "FirewallRuleGroupId": group,
            "FirewallRuleType": {
                "PartnerThreatProtection": { "Partner": "example-threat-feed" },
            },
            "Priority": 100,
            "Action": "ALERT",
        }),
    )
    .await;
    let partner_id = string_member(&partner_rule, "FirewallThreatProtectionId");
    assert_eq!(
        partner_rule.get("Status"),
        Some(&json!("CREATING")),
        "a rule whose provisioning is asynchronous begins in CREATING, got {partner_rule}"
    );

    let body = ok(
        &router,
        "BatchUpdateFirewallRule",
        json!({ "UpdateFirewallRuleEntries": [{
            "FirewallRuleGroupId": group,
            "FirewallThreatProtectionId": partner_id,
            "Priority": 200,
        }] }),
    )
    .await;
    let errors = list_member("a rule still provisioning", &body, "UpdateErrors");
    assert_eq!(
        errors.len(),
        1,
        "a rule that is still being provisioned must not be updated, got {body}"
    );
    assert_eq!(
        string_member(&errors[0], "Code"),
        "ConflictException",
        "case \"a rule still provisioning\": expected Code ConflictException, got {body}"
    );
    assert_eq!(
        listed_rule(&router, &group, "partner-rule")
            .await
            .get("Priority"),
        Some(&json!(100)),
        "case \"a rule still provisioning\": the refused entry must write nothing"
    );
}
