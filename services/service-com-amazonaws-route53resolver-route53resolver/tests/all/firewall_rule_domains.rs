// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the AutoAllowlist slice of DNS Firewall: the rule
//! `CreateFirewallRule` makes for the `AutoAllowlist` rule type, the managed
//! domain list DNS Firewall creates and maintains for it, and
//! `ListFirewallRuleDomains`, which reports that rule's domains.
//!
//! Every case goes over the wire the service actually speaks (awsJson1_1,
//! `POST /` with `X-Amz-Target: Route53Resolver.<OperationName>`), through the
//! shared [`super::harness`], so what is asserted is the HTTP behaviour and not
//! a handler called directly.
//!
//! What these cases pin is the rule the operation implements: an AutoAllowlist
//! rule owns ONE managed domain list, the review status of each domain on record
//! for the rule follows from that list holding it (`ALLOWED`), from a recorded
//! denial (`DENIED`) or from neither (`OBSERVED`), and
//! `ListFirewallRuleDomains` reports exactly those domains — the ones the rule
//! allows when the request names the list, the ones it has on record when the
//! request names the rule — filtered and sorted as `QueryOptions` say and cut
//! into pages by `MaxResults`.

use serde_json::{json, Value};
use service_com_amazonaws_route53resolver_route53resolver::state::{
    firewall_domain_canonical, AUTO_ALLOWLIST_LIST_NAME_PREFIX, DOMAIN_LIST_TYPE_AUTOALLOWLIST,
    FIREWALL_DOMAIN_LIST_STATUS_COMPLETE, FIREWALL_DOMAIN_STATUS_ALLOWED,
    FIREWALL_DOMAIN_STATUS_DENIED, FIREWALL_DOMAIN_STATUS_OBSERVED, FIREWALL_RULE_STATUS_COMPLETE,
};
use smithy_server_runtime::MultiServiceRouter;
use std::time::{Duration, Instant};

use super::harness::{call, emulator, error_type, expect_error, ok, string_member};

/// How long the polls below wait before they look again.
const POLL_INTERVAL: Duration = Duration::from_millis(10);

/// How long they keep looking before they give up: generous, so how fast the
/// host runs never decides the outcome.
const SETTLE_DEADLINE: Duration = Duration::from_secs(30);

/// The `InitialDataTimeframeHours` the rows below ask for, inside the 1..168 the
/// model's range allows
/// (`smithy_query(query="shape",shape="AutoAllowlistConfig")`).
const LOOK_BACK_HOURS: &[i64] = &[1, 24, 168];

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

/// Create one domain list holding `domains`, and answer its id.
async fn domain_list(router: &MultiServiceRouter, name: &str, domains: &[&str]) -> String {
    let created = ok(
        router,
        "CreateFirewallDomainList",
        json!({ "CreatorRequestId": format!("list-{name}"), "Name": name }),
    )
    .await;
    let id = string_member(
        created
            .get("FirewallDomainList")
            .expect("the create answers with the FirewallDomainList"),
        "Id",
    );
    if !domains.is_empty() {
        ok(
            router,
            "UpdateFirewallDomains",
            json!({ "FirewallDomainListId": id, "Operation": "ADD", "Domains": domains }),
        )
        .await;
    }
    id
}

/// What one AutoAllowlist rule a case created reports back.
struct AutoAllowlistRule {
    rule: Value,
    threat_protection_id: String,
    list_id: String,
}

/// Create one AutoAllowlist rule in `group`, with the look-back window `hours`
/// and the optional source list `import_from`.
async fn auto_allowlist_rule(
    router: &MultiServiceRouter,
    group: &str,
    name: &str,
    hours: i64,
    import_from: Option<&str>,
) -> AutoAllowlistRule {
    let mut config = json!({ "InitialDataTimeframeHours": hours });
    if let Some(source) = import_from {
        config["ImportFromFirewallDomainListId"] = json!(source);
    }
    let created = ok(
        router,
        "CreateFirewallRule",
        json!({
            "CreatorRequestId": format!("rule-{name}"),
            "FirewallRuleGroupId": group,
            "Name": name,
            "Priority": 100,
            "Action": "ALLOW",
            "FirewallRuleType": { "AutoAllowlist": config },
        }),
    )
    .await;
    let rule = created
        .get("FirewallRule")
        .expect("the create answers with the FirewallRule")
        .clone();
    let threat_protection_id = string_member(&rule, "FirewallThreatProtectionId");
    let list_id = string_member(auto_allowlist_config(&rule), "AutoAllowlistFirewallDomainListId");
    AutoAllowlistRule {
        rule,
        threat_protection_id,
        list_id,
    }
}

/// The `AutoAllowlist` configuration of a reported rule.
fn auto_allowlist_config(rule: &Value) -> &Value {
    rule.get("FirewallRuleType")
        .and_then(|rule_type| rule_type.get("AutoAllowlist"))
        .unwrap_or_else(|| panic!("the rule carries FirewallRuleType.AutoAllowlist, got {rule}"))
}

/// Apply one `UpdateFirewallDomains` call that must succeed.
async fn update_domains(router: &MultiServiceRouter, id: &str, operation: &str, domains: &[&str]) {
    ok(
        router,
        "UpdateFirewallDomains",
        json!({ "FirewallDomainListId": id, "Operation": operation, "Domains": domains }),
    )
    .await;
}

/// The `FirewallDomainList` of a `GetFirewallDomainList` response, beside the
/// two members a response reports for the managed list of an AutoAllowlist rule.
async fn read_list(router: &MultiServiceRouter, id: &str) -> Value {
    ok(
        router,
        "GetFirewallDomainList",
        json!({ "FirewallDomainListId": id }),
    )
    .await
}

/// One `ListFirewallRuleDomains` page.
async fn list_rule_domains(router: &MultiServiceRouter, body: Value) -> Value {
    ok(router, "ListFirewallRuleDomains", body).await
}

/// Every domain `ListFirewallRuleDomains` reports for one query, walked over
/// every page, in the order the pages reported them.
async fn walk_rule_domains(
    router: &MultiServiceRouter,
    body: Value,
    max_results: Option<i64>,
    case: &str,
) -> Vec<Value> {
    let mut walked: Vec<Value> = Vec::new();
    let mut next_token: Option<String> = None;
    let mut pages = 0;
    loop {
        let mut request = body.clone();
        if let Some(size) = max_results {
            request["MaxResults"] = json!(size);
        }
        if let Some(token) = &next_token {
            request["NextToken"] = json!(token);
        }
        let page = list_rule_domains(router, request).await;
        pages += 1;
        assert!(
            pages <= 1_000,
            "case {case:?}: paging did not terminate after {pages} pages"
        );
        let domains = page
            .get("Domains")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_else(|| panic!("case {case:?}: a page carries Domains, got {page}"));
        if let Some(size) = max_results {
            assert!(
                i64::try_from(domains.len()).unwrap_or(i64::MAX) <= size,
                "case {case:?}: a page must hold at most {size} domains, got {}",
                domains.len()
            );
        }
        walked.extend(domains);
        next_token = page
            .get("NextToken")
            .and_then(Value::as_str)
            .map(str::to_string);
        if next_token.is_none() {
            return walked;
        }
    }
}

/// The `DomainName`s of a reported page, in order.
fn domain_names(domains: &[Value]) -> Vec<String> {
    domains
        .iter()
        .map(|domain| string_member(domain, "DomainName"))
        .collect()
}

/// The `DomainStatus` one query reports for `domain`, and `None` when the query
/// reports no such domain.
fn status_of(domains: &[Value], domain: &str) -> Option<String> {
    let wanted = firewall_domain_canonical(domain);
    domains
        .iter()
        .find(|reported| string_member(reported, "DomainName") == wanted)
        .map(|reported| string_member(reported, "DomainStatus"))
}

/// Look at the managed domain list `id` again until the copy in flight has
/// settled, so a case that needs a settled list reads a settled one.
///
/// The CONDITION is what is waited for — the status the list REPORTS — and never
/// a span of the clock, so a loaded or a faster host changes nothing about the
/// outcome.
async fn settled_domain_list(router: &MultiServiceRouter, id: &str, case: &str) {
    let deadline = Instant::now() + SETTLE_DEADLINE;
    loop {
        let read = read_list(router, id).await;
        let list = read
            .get("FirewallDomainList")
            .unwrap_or_else(|| panic!("case {case:?}: the read answers a domain list, got {read}"));
        let status = string_member(list, "Status");
        if status == FIREWALL_DOMAIN_LIST_STATUS_COMPLETE {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "case {case:?}: the domain list must settle, it still reports {status:?}"
        );
        tokio::time::sleep(POLL_INTERVAL).await;
    }
}

/// Look at the rules of `group` again until the rule named `name` reports
/// `COMPLETE`, which is the step an AutoAllowlist rule settles in.
async fn settled_rule(router: &MultiServiceRouter, group: &str, name: &str, case: &str) {
    let deadline = Instant::now() + SETTLE_DEADLINE;
    loop {
        let listed = ok(
            router,
            "ListFirewallRules",
            json!({ "FirewallRuleGroupId": group }),
        )
        .await;
        let rules = listed
            .get("FirewallRules")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        let status = rules
            .iter()
            .find(|rule| string_member(rule, "Name") == name)
            .map(|rule| string_member(rule, "Status"))
            .unwrap_or_default();
        if status == FIREWALL_RULE_STATUS_COMPLETE {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "case {case:?}: the rule must settle, it still reports {status:?}"
        );
        tokio::time::sleep(POLL_INTERVAL).await;
    }
}

/// An AutoAllowlist rule owns the managed domain list DNS Firewall created for
/// it: the list reports the rule that owns it, the rule reports the list's id,
/// the domains of the source list the rule named are copied into it, and
/// `ListFirewallRuleDomains` reports them as the domains the rule allows.
///
/// The table spans the look-back window the rule may carry — each end of the
/// model's 1..168 range and one value inside it — and the source lists vary in
/// the number of domains they hold, so what is asserted is the copy and not one
/// window or one list.
#[tokio::test]
async fn an_auto_allowlist_rule_reports_the_domains_its_managed_list_holds() {
    let router = emulator();
    let group = rule_group(&router, "auto-allowlist-import").await;

    for (index, hours) in LOOK_BACK_HOURS.iter().enumerate() {
        let case = format!("InitialDataTimeframeHours {hours}");
        // Each row's source list holds one domain more than the row before it,
        // so the copy is asserted over lists of different sizes.
        let seeded: Vec<String> = (0..=index)
            .map(|seed| format!("seed{seed}.{index}.example.test"))
            .collect();
        let sources: Vec<&str> = seeded.iter().map(String::as_str).collect();
        let source = domain_list(&router, &format!("source-{index}"), &sources).await;
        let rule_name = format!("auto-allowlist-{index}");
        // Each row's rule needs its own group: "a rule group can contain one
        // AutoAllowlist rule".
        let group = if index == 0 {
            group.clone()
        } else {
            rule_group(&router, &format!("auto-allowlist-import-{index}")).await
        };
        let created =
            auto_allowlist_rule(&router, &group, &rule_name, *hours, Some(&source)).await;

        assert!(
            created.threat_protection_id.starts_with("rslvr-ftp-"),
            "case {case:?}: the rule must report a FirewallThreatProtectionId, got {}",
            created.rule
        );
        assert!(
            !created.list_id.is_empty(),
            "case {case:?}: the rule must report AutoAllowlistFirewallDomainListId, got {}",
            created.rule
        );
        assert_eq!(
            string_member(&created.rule, "Status"),
            "CREATING",
            "case {case:?}: a new AutoAllowlist rule begins in CREATING, got {}",
            created.rule
        );
        assert_eq!(
            auto_allowlist_config(&created.rule)
                .get("InitialDataTimeframeHours")
                .and_then(Value::as_i64),
            Some(*hours),
            "case {case:?}: the rule reports the window it was created with, got {}",
            created.rule
        );

        // The copy is what the managed list's IMPORTING step reports, and it
        // settles in COMPLETE.
        settled_domain_list(&router, &created.list_id, &case).await;
        let read = read_list(&router, &created.list_id).await;
        let list = read
            .get("FirewallDomainList")
            .unwrap_or_else(|| panic!("case {case:?}: the read answers a domain list, got {read}"));
        assert_eq!(
            string_member(list, "ManagedListType"),
            DOMAIN_LIST_TYPE_AUTOALLOWLIST,
            "case {case:?}: the managed list reports its ManagedListType, got {read}"
        );
        assert_eq!(
            string_member(list, "Name"),
            format!("{AUTO_ALLOWLIST_LIST_NAME_PREFIX}{rule_name}"),
            "case {case:?}: the managed list is named after the rule, got {read}"
        );
        assert_eq!(
            string_member(&read, "FirewallRuleGroupId"),
            group,
            "case {case:?}: the read reports the rule group that owns the list, got {read}"
        );
        assert_eq!(
            string_member(&read, "FirewallThreatProtectionId"),
            created.threat_protection_id,
            "case {case:?}: the read reports the rule that owns the list, got {read}"
        );
        assert_eq!(
            list.get("DomainCount").and_then(Value::as_i64),
            Some(i64::try_from(seeded.len()).unwrap_or(i64::MAX)),
            "case {case:?}: the managed list holds the copied domains, got {read}"
        );

        // "Identify the rule's managed domain list by FirewallDomainListId to
        // get the domains that the rule currently allows."
        let allowed = walk_rule_domains(
            &router,
            json!({
                "FirewallRuleGroupId": group,
                "FirewallDomainListId": created.list_id,
            }),
            None,
            &case,
        )
        .await;
        let mut reported = domain_names(&allowed);
        reported.sort();
        let mut expected: Vec<String> = seeded.iter().map(|d| firewall_domain_canonical(d)).collect();
        expected.sort();
        assert_eq!(
            reported, expected,
            "case {case:?}: the rule allows the copied domains, got {allowed:?}"
        );
        for domain in &allowed {
            assert_eq!(
                string_member(domain, "DomainStatus"),
                FIREWALL_DOMAIN_STATUS_ALLOWED,
                "case {case:?}: a domain the managed list holds is allowed, got {domain}"
            );
            assert!(
                !string_member(domain, "AddedTime").is_empty(),
                "case {case:?}: a reported domain carries its AddedTime, got {domain}"
            );
        }

        // Nothing has been observed and left unreviewed, which is what the
        // operation reports when the request states no status.
        let observed = walk_rule_domains(
            &router,
            json!({
                "FirewallRuleGroupId": group,
                "FirewallThreatProtectionId": created.threat_protection_id,
            }),
            None,
            &case,
        )
        .await;
        assert!(
            observed.is_empty(),
            "case {case:?}: every copied domain is allowed, so none is observed, got {observed:?}"
        );
    }
}

/// One row of the review table: what the caller asks of the managed domain list,
/// and the review status the rule's domain stands in afterwards.
struct ReviewCase {
    case: &'static str,
    operation: &'static str,
    domain: &'static str,
    status: &'static str,
    /// Whether the managed domain list holds the domain after the operation,
    /// which is what makes a domain one the rule allows.
    on_list: bool,
}

/// The review status `ListFirewallRuleDomains` reports for a domain follows the
/// managed domain list and the denials recorded on it: `ADD` "also marks the
/// observed domains as allowed", `REMOVE` leaves the rule no longer allowing
/// them, and `DENY` marks them denied and takes them off the list.
///
/// The table walks one domain through every operation the managed list takes, and
/// each row is asserted both through the status the rule reports for the domain
/// and through the filter that selects that status.
#[tokio::test]
async fn the_review_status_of_a_domain_follows_the_managed_list() {
    let router = emulator();
    let group = rule_group(&router, "auto-allowlist-review").await;
    // No source list, so the managed list is settled the moment it exists and
    // every row's change is the caller's own.
    let created = auto_allowlist_rule(&router, &group, "review", 24, None).await;

    let cases = [
        ReviewCase {
            case: "ADD allows a domain",
            operation: "ADD",
            domain: "allowed.example.test",
            status: FIREWALL_DOMAIN_STATUS_ALLOWED,
            on_list: true,
        },
        ReviewCase {
            case: "REMOVE leaves it observed",
            operation: "REMOVE",
            domain: "allowed.example.test",
            status: FIREWALL_DOMAIN_STATUS_OBSERVED,
            on_list: false,
        },
        ReviewCase {
            case: "DENY marks it denied",
            operation: "DENY",
            domain: "allowed.example.test",
            status: FIREWALL_DOMAIN_STATUS_DENIED,
            on_list: false,
        },
        ReviewCase {
            case: "ADD allows a denied domain again",
            operation: "ADD",
            domain: "allowed.example.test",
            status: FIREWALL_DOMAIN_STATUS_ALLOWED,
            on_list: true,
        },
    ];

    for case in cases {
        update_domains(&router, &created.list_id, case.operation, &[case.domain]).await;

        let on_record = walk_rule_domains(
            &router,
            json!({
                "FirewallRuleGroupId": group,
                "FirewallThreatProtectionId": created.threat_protection_id,
                "QueryOptions": { "DomainStatus": case.status },
            }),
            None,
            case.case,
        )
        .await;
        assert_eq!(
            status_of(&on_record, case.domain).as_deref(),
            Some(case.status),
            "case {:?}: the rule reports {} for {}, got {on_record:?}",
            case.case,
            case.status,
            case.domain
        );

        let allowed = walk_rule_domains(
            &router,
            json!({
                "FirewallRuleGroupId": group,
                "FirewallDomainListId": created.list_id,
            }),
            None,
            case.case,
        )
        .await;
        assert_eq!(
            status_of(&allowed, case.domain).is_some(),
            case.on_list,
            "case {:?}: the managed list holding {} must be {}, got {allowed:?}",
            case.case,
            case.domain,
            case.on_list
        );

        // A domain stands in exactly ONE review status, so the two statuses it
        // is not in report it nowhere.
        for other in [
            FIREWALL_DOMAIN_STATUS_OBSERVED,
            FIREWALL_DOMAIN_STATUS_ALLOWED,
            FIREWALL_DOMAIN_STATUS_DENIED,
        ]
        .into_iter()
        .filter(|status| *status != case.status)
        {
            let reported = walk_rule_domains(
                &router,
                json!({
                    "FirewallRuleGroupId": group,
                    "FirewallThreatProtectionId": created.threat_protection_id,
                    "QueryOptions": { "DomainStatus": other },
                }),
                None,
                case.case,
            )
            .await;
            assert_eq!(
                status_of(&reported, case.domain),
                None,
                "case {:?}: {} is in {}, so {other} must not report it, got {reported:?}",
                case.case,
                case.domain,
                case.status
            );
        }
    }

    // "DENY ... mark the specified OBSERVED domains as denied": a domain the rule
    // has nothing on record for is not one of them, so the denial records no
    // domain and no status reports it.
    let unseen = "never-seen.example.test";
    update_domains(&router, &created.list_id, "DENY", &[unseen]).await;
    for status in [
        FIREWALL_DOMAIN_STATUS_OBSERVED,
        FIREWALL_DOMAIN_STATUS_ALLOWED,
        FIREWALL_DOMAIN_STATUS_DENIED,
    ] {
        let reported = walk_rule_domains(
            &router,
            json!({
                "FirewallRuleGroupId": group,
                "FirewallThreatProtectionId": created.threat_protection_id,
                "QueryOptions": { "DomainStatus": status },
            }),
            None,
            "a denial of a domain the rule never had on record",
        )
        .await;
        assert_eq!(
            status_of(&reported, unseen),
            None,
            "case {:?}: {status} must not report {unseen}, got {reported:?}",
            "a denial of a domain the rule never had on record"
        );
    }
}

/// `QueryOptions` filter and sort the domains the operation reports, and
/// `MaxResults`/`NextToken` cut them into pages: the name filter keeps the
/// domains whose name contains the substring however it is spelled, the sort
/// order reverses the order the domains were recorded in, and a cursor minted for
/// one query is not one another query accepts.
#[tokio::test]
async fn query_options_filter_sort_and_page_the_reported_domains() {
    let router = emulator();
    let group = rule_group(&router, "auto-allowlist-query").await;
    let created = auto_allowlist_rule(&router, &group, "query", 168, None).await;

    // Recorded in this order, so the order the operation reports them in is the
    // one the sort decides and not the one they happen to sit in.
    let recorded = [
        "first.alpha.example.test",
        "second.beta.example.test",
        "third.alpha.example.test",
    ];
    for domain in recorded {
        update_domains(&router, &created.list_id, "ADD", &[domain]).await;
    }
    let canonical: Vec<String> = recorded
        .iter()
        .map(|domain| firewall_domain_canonical(domain))
        .collect();

    let by_list = json!({
        "FirewallRuleGroupId": group,
        "FirewallDomainListId": created.list_id,
    });

    // "If you don't specify a value, the operation sorts by TIME_ADDED" "in
    // descending order": the domain recorded last is reported first.
    let defaulted = walk_rule_domains(&router, by_list.clone(), None, "default order").await;
    let mut newest_first = canonical.clone();
    newest_first.reverse();
    assert_eq!(
        domain_names(&defaulted),
        newest_first,
        "case {:?}: the default order is newest first, got {defaulted:?}",
        "default order"
    );

    // Each `SortBy` the model declares is accepted, and `ASC` reverses the order
    // `DESC` reports.
    for sort_by in ["TIME_ADDED", "DAILY_TOTAL", "TOTAL_REQUESTS"] {
        for (order, expected) in [("ASC", &canonical), ("DESC", &newest_first)] {
            let case = format!("{sort_by} {order}");
            let mut body = by_list.clone();
            body["QueryOptions"] = json!({ "SortBy": sort_by, "SortOrder": order });
            let sorted = walk_rule_domains(&router, body, None, &case).await;
            assert_eq!(
                domain_names(&sorted),
                *expected,
                "case {case:?}: the reported order follows the sort, got {sorted:?}"
            );
        }
    }

    // "Returns only the domains whose name contains this substring", read the way
    // the stored domains are spelled.
    for (case, filter, expected) in [
        ("substring", "alpha", 2),
        ("case folded", "ALPHA", 2),
        ("whole name", "second.beta.example.test", 1),
        ("rooted name", "second.beta.example.test.", 1),
        ("no match", "gamma", 0),
    ] {
        let mut body = by_list.clone();
        body["QueryOptions"] = json!({ "DomainNameFilter": filter });
        let filtered = walk_rule_domains(&router, body, None, case).await;
        assert_eq!(
            filtered.len(),
            expected,
            "case {case:?}: the filter {filter:?} keeps {expected} domains, got {filtered:?}"
        );
    }

    // A page is never longer than `MaxResults`, and paging reports every domain
    // exactly once.
    for size in [1, 2, 100] {
        let case = format!("MaxResults {size}");
        let paged = walk_rule_domains(&router, by_list.clone(), Some(size), &case).await;
        assert_eq!(
            domain_names(&paged),
            newest_first,
            "case {case:?}: paging reports the whole collection in order, got {paged:?}"
        );
    }

    // The cursor belongs to the query it was minted over: the SAME offset in
    // another query's collection is not something it addresses.
    let first_page = list_rule_domains(
        &router,
        json!({
            "FirewallRuleGroupId": group,
            "FirewallDomainListId": created.list_id,
            "MaxResults": 1,
        }),
    )
    .await;
    let token = string_member(&first_page, "NextToken");
    assert!(
        !token.is_empty(),
        "a collection with more domains than one page carries a NextToken, got {first_page}"
    );
    let (status, parsed) = call(
        &router,
        "ListFirewallRuleDomains",
        json!({
            "FirewallRuleGroupId": group,
            "FirewallThreatProtectionId": created.threat_protection_id,
            "MaxResults": 1,
            "NextToken": token,
        }),
    )
    .await;
    assert_eq!(
        status, 400,
        "a cursor of another query must be refused, got {status}: {parsed}"
    );
    assert!(
        error_type(&parsed).contains("ValidationException"),
        "a cursor of another query must be refused with a ValidationException, got {parsed}"
    );
}

/// What the service refuses: a request that names no single rule, one naming a
/// rule or a group that does not exist, a value outside a `QueryOptions` enum, a
/// second AutoAllowlist rule in one group, the settings such a rule decides for
/// itself, and the domain-list operations the managed list of such a rule is not
/// written by.
#[tokio::test]
async fn the_auto_allowlist_slice_refuses_what_it_cannot_serve() {
    let router = emulator();
    let group = rule_group(&router, "auto-allowlist-refusals").await;
    let other_group = rule_group(&router, "auto-allowlist-refusals-other").await;
    let plain_list = domain_list(&router, "plain", &["plain.example.test"]).await;
    let created = auto_allowlist_rule(&router, &group, "refusals", 24, None).await;
    let elsewhere = auto_allowlist_rule(&router, &other_group, "elsewhere", 24, None).await;

    // A rule is addressed by exactly one of the two members it matches on.
    for (case, body) in [
        (
            "neither id",
            json!({ "FirewallRuleGroupId": group }),
        ),
        (
            "both ids",
            json!({
                "FirewallRuleGroupId": group,
                "FirewallDomainListId": created.list_id,
                "FirewallThreatProtectionId": created.threat_protection_id,
            }),
        ),
        (
            "a value outside QueryOptions.DomainStatus",
            json!({
                "FirewallRuleGroupId": group,
                "FirewallThreatProtectionId": created.threat_protection_id,
                "QueryOptions": { "DomainStatus": "APPROVED" },
            }),
        ),
        (
            "a value outside QueryOptions.SortBy",
            json!({
                "FirewallRuleGroupId": group,
                "FirewallThreatProtectionId": created.threat_protection_id,
                "QueryOptions": { "SortBy": "NAME" },
            }),
        ),
        (
            "a value outside QueryOptions.SortOrder",
            json!({
                "FirewallRuleGroupId": group,
                "FirewallThreatProtectionId": created.threat_protection_id,
                "QueryOptions": { "SortOrder": "ASCENDING" },
            }),
        ),
    ] {
        expect_error(
            &router,
            "ListFirewallRuleDomains",
            body,
            case,
            "ValidationException",
        )
        .await;
    }

    // An id that answers to no AutoAllowlist rule of the group names no rule.
    for (case, body) in [
        (
            "a rule group that does not exist",
            json!({
                "FirewallRuleGroupId": "rslvr-frg-0000000000000000",
                "FirewallThreatProtectionId": created.threat_protection_id,
            }),
        ),
        (
            "a threat protection id of another group's rule",
            json!({
                "FirewallRuleGroupId": group,
                "FirewallThreatProtectionId": elsewhere.threat_protection_id,
            }),
        ),
        (
            "a domain list that is not an AutoAllowlist list",
            json!({
                "FirewallRuleGroupId": group,
                "FirewallDomainListId": plain_list,
            }),
        ),
        (
            "a domain list that does not exist",
            json!({
                "FirewallRuleGroupId": group,
                "FirewallDomainListId": "rslvr-fdl-0000000000000000",
            }),
        ),
    ] {
        expect_error(
            &router,
            "ListFirewallRuleDomains",
            body,
            case,
            "ResourceNotFoundException",
        )
        .await;
    }

    // "A rule group can contain one AutoAllowlist rule", "an AutoAllowlist rule
    // uses the ALLOW action", and "you can't set Qtype,
    // FirewallDomainRedirectionAction, BlockResponse" or the id of the managed
    // list on it.
    for (case, members) in [
        (
            "a second AutoAllowlist rule in one group",
            json!({ "Action": "ALLOW", "FirewallRuleType": { "AutoAllowlist": { "InitialDataTimeframeHours": 24 } } }),
        ),
        (
            "an action other than ALLOW",
            json!({
                "Action": "BLOCK",
                "BlockResponse": "NODATA",
                "FirewallRuleType": { "AutoAllowlist": { "InitialDataTimeframeHours": 24 } },
            }),
        ),
        (
            "a Qtype",
            json!({
                "Action": "ALLOW",
                "Qtype": "AAAA",
                "FirewallRuleType": { "AutoAllowlist": { "InitialDataTimeframeHours": 24 } },
            }),
        ),
        (
            "a FirewallDomainRedirectionAction",
            json!({
                "Action": "ALLOW",
                "FirewallDomainRedirectionAction": "TRUST_REDIRECTION_DOMAIN",
                "FirewallRuleType": { "AutoAllowlist": { "InitialDataTimeframeHours": 24 } },
            }),
        ),
        (
            "the id of the managed list",
            json!({
                "Action": "ALLOW",
                "FirewallRuleType": {
                    "AutoAllowlist": {
                        "InitialDataTimeframeHours": 24,
                        "AutoAllowlistFirewallDomainListId": "rslvr-fdl-0000000000000000",
                    },
                },
            }),
        ),
    ] {
        let mut body = json!({
            "CreatorRequestId": format!("refused-{case}"),
            "FirewallRuleGroupId": group,
            "Name": "refused",
            "Priority": 500,
        });
        for (member, value) in members.as_object().expect("the row states members") {
            body[member] = value.clone();
        }
        expect_error(
            &router,
            "CreateFirewallRule",
            body,
            case,
            "ValidationException",
        )
        .await;
    }

    // "DENY — for an AutoAllowlist rule's managed domain list only", "REPLACE …
    // not supported for an AutoAllowlist rule's managed domain list", and the
    // import's only operation is REPLACE.
    for (case, body) in [
        (
            "DENY on a list of the account",
            json!({
                "FirewallDomainListId": plain_list,
                "Operation": "DENY",
                "Domains": ["plain.example.test"],
            }),
        ),
        (
            "REPLACE on the managed list of an AutoAllowlist rule",
            json!({
                "FirewallDomainListId": created.list_id,
                "Operation": "REPLACE",
                "Domains": ["replaced.example.test"],
            }),
        ),
    ] {
        expect_error(
            &router,
            "UpdateFirewallDomains",
            body,
            case,
            "ValidationException",
        )
        .await;
    }
    expect_error(
        &router,
        "ImportFirewallDomains",
        json!({
            "FirewallDomainListId": created.list_id,
            "Operation": "REPLACE",
            "DomainFileUrl": "https://example.test/domains.txt",
        }),
        "an import into the managed list of an AutoAllowlist rule",
        "ValidationException",
    )
    .await;

    // The managed list exists FOR the rule, so it is not a list a call of its
    // own deletes.
    expect_error(
        &router,
        "DeleteFirewallDomainList",
        json!({ "FirewallDomainListId": created.list_id }),
        "deleting the managed list of a live AutoAllowlist rule",
        "ConflictException",
    )
    .await;
}

/// The managed domain list follows the rule that owns it: it "is renamed when you
/// rename the rule", and deleting the rule takes the list with it.
#[tokio::test]
async fn the_managed_list_follows_the_rule_that_owns_it() {
    let router = emulator();
    let group = rule_group(&router, "auto-allowlist-lifecycle").await;
    let created = auto_allowlist_rule(&router, &group, "before", 24, None).await;

    // "Until the status is COMPLETE … UpdateFirewallRule rejects the rule": the
    // rename waits for the step the rule settles in.
    expect_error(
        &router,
        "UpdateFirewallRule",
        json!({
            "FirewallRuleGroupId": group,
            "FirewallThreatProtectionId": created.threat_protection_id,
            "Name": "after",
        }),
        "renaming a rule DNS Firewall is still provisioning",
        "ConflictException",
    )
    .await;
    settled_rule(&router, &group, "before", "rename").await;

    let updated = ok(
        &router,
        "UpdateFirewallRule",
        json!({
            "FirewallRuleGroupId": group,
            "FirewallThreatProtectionId": created.threat_protection_id,
            "Name": "after",
        }),
    )
    .await;
    let rule = updated
        .get("FirewallRule")
        .expect("the update answers with the FirewallRule");
    assert_eq!(
        string_member(auto_allowlist_config(rule), "AutoAllowlistFirewallDomainListId"),
        created.list_id,
        "the rule keeps the managed list DNS Firewall created for it, got {updated}"
    );
    let read = read_list(&router, &created.list_id).await;
    let list = read
        .get("FirewallDomainList")
        .expect("the read answers a domain list");
    assert_eq!(
        string_member(list, "Name"),
        format!("{AUTO_ALLOWLIST_LIST_NAME_PREFIX}after"),
        "the managed list is renamed with the rule, got {read}"
    );

    // The list `ListFirewallDomainLists` reports for the account carries the rule
    // that owns it and the step the list stands in.
    let listed = ok(&router, "ListFirewallDomainLists", json!({})).await;
    let metadata = listed
        .get("FirewallDomainLists")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
        .into_iter()
        .find(|entry| string_member(entry, "Id") == created.list_id)
        .unwrap_or_else(|| panic!("the account's lists include the managed one, got {listed}"));
    assert_eq!(
        (
            string_member(&metadata, "ManagedListType"),
            string_member(&metadata, "FirewallRuleGroupId"),
            string_member(&metadata, "FirewallThreatProtectionId"),
            string_member(&metadata, "Status"),
        ),
        (
            DOMAIN_LIST_TYPE_AUTOALLOWLIST.to_string(),
            group.clone(),
            created.threat_protection_id.clone(),
            FIREWALL_DOMAIN_LIST_STATUS_COMPLETE.to_string(),
        ),
        "the metadata of the managed list names the rule that owns it, got {metadata}"
    );

    // Deleting the rule deletes the list that existed for it, so no later read
    // resolves it.
    ok(
        &router,
        "DeleteFirewallRule",
        json!({
            "FirewallRuleGroupId": group,
            "FirewallThreatProtectionId": created.threat_protection_id,
        }),
    )
    .await;
    expect_error(
        &router,
        "GetFirewallDomainList",
        json!({ "FirewallDomainListId": created.list_id }),
        "the managed list of a deleted AutoAllowlist rule",
        "ResourceNotFoundException",
    )
    .await;
}
