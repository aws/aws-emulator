// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `BatchCreateFirewallRule`: the batch create of DNS
//! Firewall rules, which decides EVERY entry on its own.
//!
//! The rule under test is the one the operation's two output members state: a
//! batch create applies each entry the way the single create would and reports
//! the entries it created in `CreatedFirewallRules` and the entries it could not
//! create in `CreateErrors` — so one call can succeed in part, and an entry that
//! is refused neither fails the call nor stops the entries around it from being
//! created.
//!
//! Every case goes over the wire the service actually speaks (awsJson1_1,
//! `POST /` with `X-Amz-Target: Route53Resolver.<OperationName>`), through the
//! same router the emulator mounts, so what is asserted is the HTTP behaviour
//! and not a handler called directly. The transport helpers are the shared ones
//! in [`super::harness`].

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_route53resolver_route53resolver::state::MAX_FIREWALL_RULE_ENTRIES_PER_BATCH;

use super::harness::{call, emulator, error_type, ok, string_member};

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

/// The list member `member` of a response, which the batch create reports for
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

/// The `RuleCount` the group reports, as `GetFirewallRuleGroup` reports it.
async fn rule_count(router: &MultiServiceRouter, group_id: &str) -> i64 {
    let group = ok(
        router,
        "GetFirewallRuleGroup",
        json!({ "FirewallRuleGroupId": group_id }),
    )
    .await;
    group
        .get("FirewallRuleGroup")
        .and_then(|group| group.get("RuleCount"))
        .and_then(Value::as_i64)
        .unwrap_or_else(|| panic!("GetFirewallRuleGroup must report a RuleCount, got {group}"))
}

/// One row of the entry table: the entry a batch carries, and what the call must
/// answer for it — either the rule it created or the error code it was refused
/// with.
struct EntryCase {
    case: &'static str,
    /// What the entry carries beside `CreatorRequestId` and `Name`.
    members: Value,
    /// The `Code` the refusal must report, or `None` for an entry the call must
    /// create.
    ///
    /// The codes are the modelled shape names of this service's exceptions
    /// (`smithy_query(query="closure",operation="CreateFirewallRule")`), which
    /// is what the single create answers with on the wire for the same entry.
    code: Option<&'static str>,
}

/// A batch create applies each entry on its own: it creates every entry it can,
/// refuses the rest with the code and the sentence the single create answers for
/// that entry, and answers HTTP 200 either way.
///
/// The table spans what the rule generalises over — the entry's outcome (created
/// or refused), the kind of fault (an unresolvable rule group, an unresolvable
/// domain list, a priority another rule in the group already holds, a missing
/// setting one `Action` requires, no match source at all, a `Name` outside the
/// modelled pattern, an action a DNS Firewall Advanced rule may not take), and
/// the position of the refused entries among the created ones (first, between
/// two creates, last).
#[tokio::test]
async fn batch_create_decides_every_entry_on_its_own() {
    let router = emulator();
    let group = rule_group(&router, "batch-mixed-group").await;
    let list = domain_list(&router, "batch-mixed-list").await;

    let cases = vec![
        EntryCase {
            case: "an unresolvable rule group",
            members: json!({
                "FirewallRuleGroupId": "rslvr-frg-ffffffffffffffff",
                "FirewallDomainListId": list,
                "Priority": 100,
                "Action": "ALERT",
            }),
            code: Some("ResourceNotFoundException"),
        },
        EntryCase {
            case: "a domain-list rule that alerts",
            members: json!({
                "FirewallRuleGroupId": group,
                "FirewallDomainListId": list,
                "Priority": 200,
                "Action": "ALERT",
            }),
            code: None,
        },
        EntryCase {
            case: "an unresolvable domain list",
            members: json!({
                "FirewallRuleGroupId": group,
                "FirewallDomainListId": "rslvr-fdl-ffffffffffffffff",
                "Priority": 300,
                "Action": "ALERT",
            }),
            code: Some("ResourceNotFoundException"),
        },
        EntryCase {
            case: "a domain-list rule that blocks with NXDOMAIN",
            members: json!({
                "FirewallRuleGroupId": group,
                "FirewallDomainListId": list,
                "Priority": 400,
                "Action": "BLOCK",
                "BlockResponse": "NXDOMAIN",
            }),
            code: None,
        },
        EntryCase {
            case: "a priority an entry before it already took",
            members: json!({
                "FirewallRuleGroupId": group,
                "FirewallDomainListId": list,
                "Priority": 200,
                "Action": "ALERT",
            }),
            code: Some("ValidationException"),
        },
        EntryCase {
            case: "BLOCK without the BlockResponse it requires",
            members: json!({
                "FirewallRuleGroupId": group,
                "FirewallDomainListId": list,
                "Priority": 500,
                "Action": "BLOCK",
            }),
            code: Some("ValidationException"),
        },
        EntryCase {
            case: "no match source at all",
            members: json!({
                "FirewallRuleGroupId": group,
                "Priority": 600,
                "Action": "ALERT",
            }),
            code: Some("ValidationException"),
        },
        EntryCase {
            case: "a Name of digits alone",
            members: json!({
                "FirewallRuleGroupId": group,
                "FirewallDomainListId": list,
                "Priority": 700,
                "Action": "ALERT",
            }),
            code: Some("ValidationException"),
        },
        EntryCase {
            case: "a threat-detector rule that allows",
            members: json!({
                "FirewallRuleGroupId": group,
                "DnsThreatProtection": "DGA",
                "ConfidenceThreshold": "HIGH",
                "Priority": 800,
                "Action": "ALLOW",
            }),
            code: Some("ValidationException"),
        },
        EntryCase {
            case: "a threat-detector rule that blocks",
            members: json!({
                "FirewallRuleGroupId": group,
                "DnsThreatProtection": "DNS_TUNNELING",
                "ConfidenceThreshold": "MEDIUM",
                "Priority": 900,
                "Action": "BLOCK",
                "BlockResponse": "NODATA",
            }),
            code: None,
        },
    ];

    // The one row whose fault is the `Name` itself names its rule with digits
    // alone; every other row is named after its case, which the pattern admits.
    let entry_name = |case: &EntryCase| -> String {
        if case.case == "a Name of digits alone" {
            "1234567890".to_string()
        } else {
            case.case.replace(' ', "-")
        }
    };

    let entries: Vec<Value> = cases
        .iter()
        .map(|case| {
            let mut entry = case.members.clone();
            let members = entry
                .as_object_mut()
                .expect("each row states its members as an object");
            members.insert(
                "CreatorRequestId".to_string(),
                json!(format!("batch-mixed-{}", entry_name(case))),
            );
            members.insert("Name".to_string(), json!(entry_name(case)));
            entry
        })
        .collect();

    let (status, body) = call(
        &router,
        "BatchCreateFirewallRule",
        json!({ "CreateFirewallRuleEntries": entries }),
    )
    .await;
    assert_eq!(
        status, 200,
        "a batch that creates some entries and refuses others must answer HTTP 200, \
         got {status}: {body}"
    );

    let created = list_member("the mixed batch", &body, "CreatedFirewallRules");
    let errors = list_member("the mixed batch", &body, "CreateErrors");

    let expected_created: Vec<String> = cases
        .iter()
        .filter(|case| case.code.is_none())
        .map(entry_name)
        .collect();
    let reported_created: Vec<String> = created
        .iter()
        .map(|rule| string_member(rule, "Name"))
        .collect();
    assert_eq!(
        reported_created, expected_created,
        "the batch must report exactly the entries it created, in the order the request \
         stated them: expected {expected_created:?}, got {reported_created:?} from {body}"
    );

    let expected_refused: Vec<(String, &str)> = cases
        .iter()
        .filter_map(|case| case.code.map(|code| (entry_name(case), code)))
        .collect();
    assert_eq!(
        errors.len(),
        expected_refused.len(),
        "the batch must report one error per entry it could not create: expected \
         {expected_refused:?}, got {errors:?}"
    );
    for ((name, code), error) in expected_refused.iter().zip(errors.iter()) {
        // "The firewall rule entry that caused the error": the entry the request
        // stated, so a caller can tell which of its entries this error is about.
        let entry = error
            .get("FirewallRule")
            .unwrap_or_else(|| panic!("case {name:?}: the error must echo its entry, got {error}"));
        assert_eq!(
            &string_member(entry, "Name"),
            name,
            "case {name:?}: the error must echo the entry that caused it, got {error}"
        );
        assert_eq!(
            &string_member(error, "Code"),
            code,
            "case {name:?}: expected Code {code}, got {error}"
        );
        assert!(
            !string_member(error, "Message").is_empty(),
            "case {name:?}: the error must state why the entry was refused, got {error}"
        );
    }

    // Exactly the created entries are persisted: a refused entry wrote nothing,
    // and the group reports the rules the batch created and no others.
    let listed: Vec<String> = listed_rules(&router, &group)
        .await
        .iter()
        .map(|rule| string_member(rule, "Name"))
        .collect();
    let mut expected_listed = expected_created.clone();
    expected_listed.sort();
    let mut listed_sorted = listed.clone();
    listed_sorted.sort();
    assert_eq!(
        listed_sorted, expected_listed,
        "the group must hold exactly the rules the batch created: expected \
         {expected_listed:?}, got {listed:?}"
    );
    assert_eq!(
        rule_count(&router, &group).await,
        expected_created.len() as i64,
        "the group's RuleCount must count exactly the rules the batch created, which are \
         {expected_created:?}"
    );
}

/// A batch create is the single create, applied per entry: each created rule
/// reports the members its entry carried and is readable back through
/// `ListFirewallRules`, and a repeat of the same batch — the same
/// `CreatorRequestId`s — creates nothing more and answers the same rules.
#[tokio::test]
async fn batch_created_rules_round_trip_and_repeat_idempotently() {
    let router = emulator();
    let group = rule_group(&router, "batch-roundtrip-group").await;
    let list = domain_list(&router, "batch-roundtrip-list").await;

    let entries = json!([
        {
            "CreatorRequestId": "batch-roundtrip-override",
            "FirewallRuleGroupId": group,
            "FirewallDomainListId": list,
            "Name": "override-rule",
            "Priority": 100,
            "Action": "BLOCK",
            "BlockResponse": "OVERRIDE",
            "BlockOverrideDomain": "example.test",
            "BlockOverrideDnsType": "CNAME",
            "BlockOverrideTtl": 60,
            "FirewallDomainRedirectionAction": "TRUST_REDIRECTION_DOMAIN",
            "Qtype": "AAAA",
        },
        {
            "CreatorRequestId": "batch-roundtrip-alert",
            "FirewallRuleGroupId": group,
            "FirewallDomainListId": list,
            "Name": "alert-rule",
            "Priority": 200,
            "Action": "ALERT",
        },
    ]);

    let body = ok(
        &router,
        "BatchCreateFirewallRule",
        json!({ "CreateFirewallRuleEntries": entries }),
    )
    .await;
    let created = list_member("the all-good batch", &body, "CreatedFirewallRules");
    assert_eq!(
        created.len(),
        2,
        "a batch whose entries are all good must create them all, got {body}"
    );
    assert!(
        list_member("the all-good batch", &body, "CreateErrors").is_empty(),
        "a batch whose entries are all good must report no error, got {body}"
    );

    // The members the first entry declared, as the created rule must report them
    // back and as a later read must report them too.
    let expected = [
        ("Name", json!("override-rule")),
        ("Priority", json!(100)),
        ("Action", json!("BLOCK")),
        ("BlockResponse", json!("OVERRIDE")),
        // The override target is a DNS name, so the rule reports it canonical —
        // rooted — whatever spelling the entry wrote it in.
        ("BlockOverrideDomain", json!("example.test.")),
        ("BlockOverrideDnsType", json!("CNAME")),
        ("BlockOverrideTtl", json!(60)),
        (
            "FirewallDomainRedirectionAction",
            json!("TRUST_REDIRECTION_DOMAIN"),
        ),
        ("Qtype", json!("AAAA")),
        ("FirewallRuleGroupId", json!(group)),
        ("FirewallDomainListId", json!(list)),
        ("CreatorRequestId", json!("batch-roundtrip-override")),
    ];
    for (member, value) in &expected {
        assert_eq!(
            created[0].get(*member),
            Some(value),
            "case \"created override rule\": {member} must be {value}, got {}",
            created[0]
        );
    }

    let listed = listed_rules(&router, &group).await;
    assert_eq!(
        listed.len(),
        2,
        "the group must hold the two rules the batch created, got {listed:?}"
    );
    let read_back = listed
        .iter()
        .find(|rule| string_member(rule, "Name") == "override-rule")
        .expect("the created rule is listed under the name its entry carried");
    for (member, value) in &expected {
        assert_eq!(
            read_back.get(*member),
            Some(value),
            "case \"listed override rule\": {member} must be {value}, got {read_back}"
        );
    }

    // "A unique string that identifies the request and that allows you to retry
    // failed requests without the risk of running the operation twice": the
    // repeat resolves to the rules the first call made.
    let repeat = ok(
        &router,
        "BatchCreateFirewallRule",
        json!({ "CreateFirewallRuleEntries": entries }),
    )
    .await;
    let repeated = list_member("the repeated batch", &repeat, "CreatedFirewallRules");
    assert_eq!(
        repeated, created,
        "a repeated batch must answer the rules it already created, got {repeat}"
    );
    assert!(
        list_member("the repeated batch", &repeat, "CreateErrors").is_empty(),
        "a repeated batch must report no error, got {repeat}"
    );
    assert_eq!(
        rule_count(&router, &group).await,
        2,
        "a repeated batch must create nothing more, so the group still holds two rules"
    );
}

/// A batch past the entries one call may carry is refused WHOLE: nothing is
/// created for any of its entries.
#[tokio::test]
async fn a_batch_past_the_entry_bound_creates_nothing() {
    let router = emulator();
    let group = rule_group(&router, "batch-bound-group").await;
    let list = domain_list(&router, "batch-bound-list").await;

    // The published bound itself, one past it, and one under it: the bound is a
    // bound, so the call it refuses is the one PAST it.
    for (case, count, expected_status) in [
        ("one under the bound", MAX_FIREWALL_RULE_ENTRIES_PER_BATCH - 1, 200),
        ("the bound itself", MAX_FIREWALL_RULE_ENTRIES_PER_BATCH, 200),
        ("one past the bound", MAX_FIREWALL_RULE_ENTRIES_PER_BATCH + 1, 400),
    ] {
        let group = rule_group(&router, &format!("batch-bound-{count}")).await;
        let entries: Vec<Value> = (0..count)
            .map(|index| {
                json!({
                    "CreatorRequestId": format!("batch-bound-{count}-{index}"),
                    "FirewallRuleGroupId": group,
                    "FirewallDomainListId": list,
                    "Name": format!("bound-rule-{index}"),
                    "Priority": 100 + index as i64,
                    "Action": "ALERT",
                })
            })
            .collect();

        let (status, body) = call(
            &router,
            "BatchCreateFirewallRule",
            json!({ "CreateFirewallRuleEntries": entries }),
        )
        .await;
        assert_eq!(
            status, expected_status,
            "case {case:?} ({count} entries): expected HTTP {expected_status}, got {status} \
             with {body}"
        );
        if expected_status == 200 {
            assert_eq!(
                list_member(case, &body, "CreatedFirewallRules").len(),
                count,
                "case {case:?}: a batch within the bound must create every entry, got {body}"
            );
        } else {
            assert!(
                error_type(&body).contains("LimitExceededException"),
                "case {case:?}: expected LimitExceededException, got {body}"
            );
            assert_eq!(
                rule_count(&router, &group).await,
                0,
                "case {case:?}: a refused batch must create nothing at all"
            );
        }
    }

    assert_eq!(
        rule_count(&router, &group).await,
        0,
        "no case wrote into the group it never named"
    );
}
