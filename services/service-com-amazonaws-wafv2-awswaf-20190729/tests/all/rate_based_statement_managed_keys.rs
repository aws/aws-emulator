// SPDX-License-Identifier: CC0-1.0

//! `GetRateBasedStatementManagedKeys`, end to end over the wire.
//!
//! The operation "retrieves the IP addresses that are currently blocked by a
//! rate-based rule instance", and three rules are asserted here:
//!
//! 1. The report is about ONE rate-based rule instance, which WAF identifies by the
//!    "unique combination of web ACL, optional rule group, and rate-based rule": the
//!    `(Scope, WebACLName, WebACLId)` triple has to name a stored web ACL, and
//!    `RuleName` has to name a rate-based rule of that web ACL's rules — or, when
//!    `RuleGroupRuleName` is given, of the rules of the rule group the named
//!    rule-group reference statement points at. Anything else names no instance and
//!    is `WAFNonexistentItemException`.
//! 2. The call is available only for a rate-based rule that aggregates solely on an
//!    IP address: `AggregateKeyType` `IP` or `FORWARDED_IP`. The other settings the
//!    model declares are refused with `WAFUnsupportedAggregateKeyTypeException`.
//! 3. For an instance that resolves, both families are reported — each tagged with
//!    its `IPAddressVersion` — holding the addresses WAF is currently rate limiting.
//!    This emulator serves WAF's control plane only, so no address has a request rate
//!    and both lists are empty, which is also what WAF answers for a rate-based rule
//!    that is currently limiting nothing.
//!
//! Each is asserted over a TABLE spanning the class: the rows differ in scope, in
//! both aggregation settings the report is available for, in where the rate-based
//! rule lives (the web ACL itself or a referenced rule group), in whether the rule
//! carries a scope-down statement, forwarded-IP configuration or a non-default
//! evaluation window, and in its position among the web ACL's rules. The harness is
//! the one in [`crate::web_acl_lifecycle`]: every call goes through the router's
//! front door, so what these tests observe is what an SDK observes.

use serde_json::{json, Value};

use crate::rule_group_lifecycle::create_rule_group;
use crate::web_acl_lifecycle::{create_web_acl, ordered, rule, sqli_statement, xss_statement, Waf};

/// `WAFUnsupportedAggregateKeyTypeException`'s sentence, as the model documents it
/// (`smithy_query(query="shape",shape="WAFUnsupportedAggregateKeyTypeException")`),
/// spelled out here rather than imported so this suite pins the text itself.
const UNSUPPORTED_AGGREGATE_KEY_TYPE: &str = "The rule that you've named doesn't \
     aggregate solely on the IP address or solely on the forwarded IP address. This \
     call is only available for rate-based rules with an AggregateKeyType setting of \
     IP or FORWARDED_IP.";

/// `WAFNonexistentItemException`'s sentence, as recorded from WAF.
const NONEXISTENT_ITEM: &str =
    "AWS WAF couldn’t perform the operation because your resource doesn’t exist.";

/// An `EntityId`-shaped id no create minted, so it names nothing stored.
const MISSING_ID: &str = "00000000-1111-2222-3333-444444444444";

/// A `RateBasedStatement` aggregating on the request origin's IP address, limiting
/// `limit` requests per window. `Limit` is `@range(min 10, max 2000000000)`.
fn ip_rate_based(limit: i64) -> Value {
    json!({
        "RateBasedStatement": {
            "Limit": limit,
            "AggregateKeyType": "IP",
        }
    })
}

/// A `RateBasedStatement` aggregating on the IP address, with an explicit
/// `EvaluationWindowSec` and a scope-down statement narrowing what it counts.
///
/// "Valid settings are 60, 120, 300, and 600" (`EvaluationWindowSec`'s own doc).
fn ip_rate_based_with_scope_down(limit: i64, evaluation_window_sec: i64) -> Value {
    json!({
        "RateBasedStatement": {
            "Limit": limit,
            "EvaluationWindowSec": evaluation_window_sec,
            "AggregateKeyType": "IP",
            "ScopeDownStatement": sqli_statement(),
        }
    })
}

/// A `RateBasedStatement` aggregating on the first IP address in `header`: "with
/// this option, you must specify the header to use in the `ForwardedIPConfig`
/// property" (`RateBasedStatement$AggregateKeyType`).
fn forwarded_ip_rate_based(limit: i64, header: &str, fallback: &str) -> Value {
    json!({
        "RateBasedStatement": {
            "Limit": limit,
            "AggregateKeyType": "FORWARDED_IP",
            "ForwardedIPConfig": {
                "HeaderName": header,
                "FallbackBehavior": fallback,
            },
        }
    })
}

/// A `RateBasedStatement` aggregating on custom keys: "with this option, you must
/// specify the aggregate keys in the `CustomKeys` property".
fn custom_keys_rate_based(limit: i64) -> Value {
    json!({
        "RateBasedStatement": {
            "Limit": limit,
            "AggregateKeyType": "CUSTOM_KEYS",
            "CustomKeys": [{ "HTTPMethod": {} }],
        }
    })
}

/// A `RateBasedStatement` that counts every request matching its scope-down
/// statement without aggregating further: "with this option, you must configure the
/// `ScopeDownStatement` property".
fn constant_rate_based(limit: i64) -> Value {
    json!({
        "RateBasedStatement": {
            "Limit": limit,
            "AggregateKeyType": "CONSTANT",
            "ScopeDownStatement": xss_statement(),
        }
    })
}

/// A rule-group reference statement naming `arn`.
fn rule_group_reference(arn: &str) -> Value {
    json!({ "RuleGroupReferenceStatement": { "ARN": arn } })
}

/// A `GetRateBasedStatementManagedKeys` request body. `RuleGroupRuleName` is present
/// only when the case supplies one, so an absent member is really absent on the wire.
fn managed_keys_request(
    scope: &str,
    web_acl_name: &str,
    web_acl_id: &str,
    rule_name: &str,
    rule_group_rule_name: Option<&str>,
) -> Value {
    let mut body = json!({
        "Scope": scope,
        "WebACLName": web_acl_name,
        "WebACLId": web_acl_id,
        "RuleName": rule_name,
    });
    if let Some(rule_group_rule_name) = rule_group_rule_name {
        body["RuleGroupRuleName"] = json!(rule_group_rule_name);
    }
    body
}

/// Both halves of the report, each tagged with the family it covers and holding the
/// addresses WAF is currently rate limiting — none, in a control-plane emulator.
fn assert_empty_managed_keys(body: &Value, case: &str) {
    for (member, version) in [("ManagedKeysIPV4", "IPV4"), ("ManagedKeysIPV6", "IPV6")] {
        assert_eq!(
            body[member]["IPAddressVersion"].as_str(),
            Some(version),
            "case {case:?}: {member} must report IPAddressVersion {version}, got {body}"
        );
        assert_eq!(
            body[member]["Addresses"],
            json!([]),
            "case {case:?}: {member} must report no blocked address, got {body}"
        );
    }
}

/// One row of the "the instance resolves" table: a web ACL to create, the rules it
/// carries, and the rate-based rule the report is asked for.
struct ServedCase {
    case: &'static str,
    scope: &'static str,
    /// The rules of the web ACL this row creates.
    rules: Value,
    /// `RuleName`: the rate-based rule the keys are asked for.
    rule_name: &'static str,
    /// `RuleGroupRuleName`, for a rate-based rule nested inside a rule group.
    rule_group_rule_name: Option<&'static str>,
    /// The rules of a rule group to create first and reference from `rules`, with the
    /// name of the web ACL rule that references it.
    rule_group: Option<(&'static str, Value)>,
}

/// A rate-based rule that aggregates solely on an IP address reports both families,
/// each empty: no web request reaches a web ACL in this emulator, so no address is
/// being rate limited.
///
/// One row per way an instance can be addressed and configured — both aggregation
/// settings the call is available for, both scopes, the rule alone in the web ACL and
/// third among several, with and without a scope-down statement, with a non-default
/// evaluation window, with either `FallbackBehavior`, and nested inside a referenced
/// rule group (the form `RuleGroupRuleName` exists for) — and the report must not
/// depend on any of it.
#[tokio::test]
async fn a_rate_based_rule_aggregating_solely_on_an_ip_reports_both_families_empty() {
    let cases = [
        ServedCase {
            case: "the only rule of a REGIONAL web ACL, aggregating on IP",
            scope: "REGIONAL",
            rules: json!([rule("rate-ip", ip_rate_based(1000))]),
            rule_name: "rate-ip",
            rule_group_rule_name: None,
            rule_group: None,
        },
        ServedCase {
            case: "the minimum limit, in a CLOUDFRONT web ACL",
            scope: "CLOUDFRONT",
            rules: json!([rule("rate-ip", ip_rate_based(10))]),
            rule_name: "rate-ip",
            rule_group_rule_name: None,
            rule_group: None,
        },
        ServedCase {
            case: "third among several rules, with a scope-down statement",
            scope: "REGIONAL",
            rules: ordered(json!([
                rule("plain-sqli", sqli_statement()),
                rule("plain-xss", xss_statement()),
                rule("rate-scoped", ip_rate_based_with_scope_down(2_000, 60)),
            ])),
            rule_name: "rate-scoped",
            rule_group_rule_name: None,
            rule_group: None,
        },
        ServedCase {
            case: "the longest evaluation window",
            scope: "REGIONAL",
            rules: json!([rule(
                "rate-window",
                ip_rate_based_with_scope_down(2_000_000_000, 600)
            )]),
            rule_name: "rate-window",
            rule_group_rule_name: None,
            rule_group: None,
        },
        ServedCase {
            case: "aggregating on the forwarded IP address, falling back to MATCH",
            scope: "REGIONAL",
            rules: json!([rule(
                "rate-forwarded",
                forwarded_ip_rate_based(500, "X-Forwarded-For", "MATCH")
            )]),
            rule_name: "rate-forwarded",
            rule_group_rule_name: None,
            rule_group: None,
        },
        ServedCase {
            case: "aggregating on a custom forwarded header, falling back to NO_MATCH",
            scope: "CLOUDFRONT",
            rules: json!([rule(
                "rate-forwarded-custom",
                forwarded_ip_rate_based(100, "True-Client-IP", "NO_MATCH")
            )]),
            rule_name: "rate-forwarded-custom",
            rule_group_rule_name: None,
            rule_group: None,
        },
        ServedCase {
            case: "nested inside a referenced rule group",
            scope: "REGIONAL",
            rules: Value::Null,
            rule_name: "grouped-rate-ip",
            rule_group_rule_name: Some("group-reference"),
            rule_group: Some((
                "group-reference",
                json!([rule("grouped-rate-ip", ip_rate_based(750))]),
            )),
        },
        ServedCase {
            case: "nested inside a referenced rule group, beside other rules",
            scope: "CLOUDFRONT",
            rules: Value::Null,
            rule_name: "grouped-rate-forwarded",
            rule_group_rule_name: Some("group-reference"),
            rule_group: Some((
                "group-reference",
                ordered(json!([
                    rule("grouped-sqli", sqli_statement()),
                    rule(
                        "grouped-rate-forwarded",
                        forwarded_ip_rate_based(10_000, "X-Forwarded-For", "MATCH")
                    ),
                ])),
            )),
        },
    ];

    let waf = Waf::new();
    for (index, case) in cases.iter().enumerate() {
        let web_acl_name = format!("rate-keys-served-{index}");
        // A rate-based rule may live in a rule group too, and then the report is
        // addressed through the web ACL rule that references the group.
        let rules = match &case.rule_group {
            Some((reference_rule_name, group_rules)) => {
                let group_name = format!("rate-keys-group-{index}");
                let (_group_id, group_arn, _group_lock) = create_rule_group(
                    &waf,
                    case.scope,
                    &group_name,
                    100,
                    group_rules.clone(),
                )
                .await;
                json!([rule(reference_rule_name, rule_group_reference(&group_arn))])
            }
            None => case.rules.clone(),
        };
        let (web_acl_id, _arn, lock_token) =
            create_web_acl(&waf, case.scope, &web_acl_name, rules).await;

        let body = waf
            .ok(
                "GetRateBasedStatementManagedKeys",
                managed_keys_request(
                    case.scope,
                    &web_acl_name,
                    &web_acl_id,
                    case.rule_name,
                    case.rule_group_rule_name,
                ),
            )
            .await;
        assert_empty_managed_keys(&body, case.case);

        // The report is a read: the web ACL it was asked about is unchanged, so a
        // second call answers the same thing and the lock token still matches.
        let again = waf
            .ok(
                "GetRateBasedStatementManagedKeys",
                managed_keys_request(
                    case.scope,
                    &web_acl_name,
                    &web_acl_id,
                    case.rule_name,
                    case.rule_group_rule_name,
                ),
            )
            .await;
        assert_eq!(
            again, body,
            "case {:?}: a second report must answer the same keys, got {again}",
            case.case
        );
        let read_back = waf
            .ok(
                "GetWebACL",
                json!({ "Name": &web_acl_name, "Scope": case.scope, "Id": &web_acl_id }),
            )
            .await;
        assert_eq!(
            read_back["LockToken"].as_str(),
            Some(lock_token.as_str()),
            "case {:?}: a report must not change the web ACL's lock token, got {read_back}",
            case.case
        );
    }
}

/// The call is available only for a rate-based rule that aggregates solely on an IP
/// address: every other `AggregateKeyType` the model declares is refused.
///
/// One row per remaining setting of `RateBasedStatementAggregateKeyType`
/// (`smithy_query(query="shape",shape="RateBasedStatementAggregateKeyType")` → `IP`,
/// `FORWARDED_IP`, `CUSTOM_KEYS`, `CONSTANT`), in both scopes and both addressing
/// forms, with the sentence the refusal carries pinned.
#[tokio::test]
async fn a_rate_based_rule_that_does_not_aggregate_solely_on_an_ip_is_refused() {
    let waf = Waf::new();

    // A rule group holding one of each refused aggregation, so the nested addressing
    // form is refused the same way the direct one is.
    let (_group_id, group_arn, _group_lock) = create_rule_group(
        &waf,
        "REGIONAL",
        "rate-keys-unsupported-group",
        200,
        ordered(json!([
            rule("grouped-custom-keys", custom_keys_rate_based(1_000)),
            rule("grouped-constant", constant_rate_based(1_000)),
        ])),
    )
    .await;
    let (regional_id, _regional_arn, _regional_lock) = create_web_acl(
        &waf,
        "REGIONAL",
        "rate-keys-unsupported",
        ordered(json!([
            rule("rate-custom-keys", custom_keys_rate_based(5_000)),
            rule("rate-constant", constant_rate_based(20)),
            rule("group-reference", rule_group_reference(&group_arn)),
        ])),
    )
    .await;
    let (cloudfront_id, _cloudfront_arn, _cloudfront_lock) = create_web_acl(
        &waf,
        "CLOUDFRONT",
        "rate-keys-unsupported-cf",
        json!([rule("rate-custom-keys", custom_keys_rate_based(1_000_000))]),
    )
    .await;

    let rows: &[(&str, &str, &str, &str, Option<&str>)] = &[
        (
            "CUSTOM_KEYS in a REGIONAL web ACL",
            "REGIONAL",
            "rate-keys-unsupported",
            "rate-custom-keys",
            None,
        ),
        (
            "CONSTANT in a REGIONAL web ACL",
            "REGIONAL",
            "rate-keys-unsupported",
            "rate-constant",
            None,
        ),
        (
            "CUSTOM_KEYS in a CLOUDFRONT web ACL",
            "CLOUDFRONT",
            "rate-keys-unsupported-cf",
            "rate-custom-keys",
            None,
        ),
        (
            "CUSTOM_KEYS inside a referenced rule group",
            "REGIONAL",
            "rate-keys-unsupported",
            "grouped-custom-keys",
            Some("group-reference"),
        ),
        (
            "CONSTANT inside a referenced rule group",
            "REGIONAL",
            "rate-keys-unsupported",
            "grouped-constant",
            Some("group-reference"),
        ),
    ];

    for (case, scope, web_acl_name, rule_name, rule_group_rule_name) in rows {
        let web_acl_id = if *scope == "CLOUDFRONT" {
            &cloudfront_id
        } else {
            &regional_id
        };
        let body = waf
            .refused(
                "GetRateBasedStatementManagedKeys",
                managed_keys_request(
                    scope,
                    web_acl_name,
                    web_acl_id,
                    rule_name,
                    *rule_group_rule_name,
                ),
                "WAFUnsupportedAggregateKeyTypeException",
                case,
            )
            .await;
        assert_eq!(
            body["Message"].as_str(),
            Some(UNSUPPORTED_AGGREGATE_KEY_TYPE),
            "case {case:?}: expected the sentence this exception type carries, got {body}"
        );
    }
}

/// One row of the "the instance does not resolve" table: the addressing a report is
/// asked with, and nothing it can resolve to.
struct UnresolvedCase<'a> {
    case: &'static str,
    scope: &'static str,
    web_acl_name: &'static str,
    /// `WebACLId`, borrowed from the id a create minted or from a literal that names
    /// nothing stored.
    web_acl_id: &'a str,
    rule_name: &'static str,
    rule_group_rule_name: Option<&'static str>,
}

/// A report is about one rate-based rule INSTANCE: the web ACL, the optional rule
/// group and the rate-based rule all have to resolve, or the call is refused with
/// `WAFNonexistentItemException`.
///
/// One row per way an address can name no instance: a web ACL triple that describes
/// nothing stored (wrong name, wrong id, the other scope, a deleted web ACL), a rule
/// name that names no rule or names a rule whose statement is of another kind, a
/// rate-based rule of a referenced rule group addressed without naming the reference,
/// and a `RuleGroupRuleName` that names no rule-group reference or whose group holds
/// no such rate-based rule.
#[tokio::test]
async fn a_report_for_an_instance_that_does_not_resolve_is_refused() {
    let waf = Waf::new();

    let (_group_id, group_arn, _group_lock) = create_rule_group(
        &waf,
        "REGIONAL",
        "rate-keys-addressed-group",
        100,
        json!([rule("grouped-rate-ip", ip_rate_based(1_000))]),
    )
    .await;
    let (web_acl_id, _arn, _lock) = create_web_acl(
        &waf,
        "REGIONAL",
        "rate-keys-addressed",
        ordered(json!([
            rule("rate-ip", ip_rate_based(1_000)),
            rule("plain-sqli", sqli_statement()),
            rule("group-reference", rule_group_reference(&group_arn)),
        ])),
    )
    .await;

    // A web ACL that existed and was deleted names nothing afterwards: the report
    // follows stored state, not the shape of the request.
    let (deleted_id, _deleted_arn, deleted_lock) = create_web_acl(
        &waf,
        "REGIONAL",
        "rate-keys-deleted",
        json!([rule("rate-ip", ip_rate_based(1_000))]),
    )
    .await;
    waf.ok(
        "DeleteWebACL",
        json!({
            "Name": "rate-keys-deleted",
            "Scope": "REGIONAL",
            "Id": &deleted_id,
            "LockToken": deleted_lock,
        }),
    )
    .await;

    let rows = [
        UnresolvedCase {
            case: "a web ACL name nothing was created under",
            scope: "REGIONAL",
            web_acl_name: "rate-keys-absent",
            web_acl_id: &web_acl_id,
            rule_name: "rate-ip",
            rule_group_rule_name: None,
        },
        UnresolvedCase {
            case: "the stored name with an id nothing was created under",
            scope: "REGIONAL",
            web_acl_name: "rate-keys-addressed",
            web_acl_id: MISSING_ID,
            rule_name: "rate-ip",
            rule_group_rule_name: None,
        },
        UnresolvedCase {
            case: "the stored web ACL read in the other scope",
            scope: "CLOUDFRONT",
            web_acl_name: "rate-keys-addressed",
            web_acl_id: &web_acl_id,
            rule_name: "rate-ip",
            rule_group_rule_name: None,
        },
        UnresolvedCase {
            case: "a web ACL that was deleted",
            scope: "REGIONAL",
            web_acl_name: "rate-keys-deleted",
            web_acl_id: &deleted_id,
            rule_name: "rate-ip",
            rule_group_rule_name: None,
        },
        UnresolvedCase {
            case: "a rule name no rule of the web ACL carries",
            scope: "REGIONAL",
            web_acl_name: "rate-keys-addressed",
            web_acl_id: &web_acl_id,
            rule_name: "rate-absent",
            rule_group_rule_name: None,
        },
        UnresolvedCase {
            case: "a rule whose statement is not rate-based",
            scope: "REGIONAL",
            web_acl_name: "rate-keys-addressed",
            web_acl_id: &web_acl_id,
            rule_name: "plain-sqli",
            rule_group_rule_name: None,
        },
        UnresolvedCase {
            case: "a rule of the referenced rule group, without naming the reference",
            scope: "REGIONAL",
            web_acl_name: "rate-keys-addressed",
            web_acl_id: &web_acl_id,
            rule_name: "grouped-rate-ip",
            rule_group_rule_name: None,
        },
        UnresolvedCase {
            case: "a rule group reference name no rule of the web ACL carries",
            scope: "REGIONAL",
            web_acl_name: "rate-keys-addressed",
            web_acl_id: &web_acl_id,
            rule_name: "grouped-rate-ip",
            rule_group_rule_name: Some("group-absent"),
        },
        UnresolvedCase {
            case: "a rule group reference name whose rule references no rule group",
            scope: "REGIONAL",
            web_acl_name: "rate-keys-addressed",
            web_acl_id: &web_acl_id,
            rule_name: "grouped-rate-ip",
            rule_group_rule_name: Some("plain-sqli"),
        },
        UnresolvedCase {
            case: "a rate-based rule of the web ACL, addressed inside the rule group",
            scope: "REGIONAL",
            web_acl_name: "rate-keys-addressed",
            web_acl_id: &web_acl_id,
            rule_name: "rate-ip",
            rule_group_rule_name: Some("group-reference"),
        },
        UnresolvedCase {
            case: "a rule name no rule of the referenced rule group carries",
            scope: "REGIONAL",
            web_acl_name: "rate-keys-addressed",
            web_acl_id: &web_acl_id,
            rule_name: "grouped-absent",
            rule_group_rule_name: Some("group-reference"),
        },
    ];

    for row in &rows {
        let case = row.case;
        let body = waf
            .refused(
                "GetRateBasedStatementManagedKeys",
                managed_keys_request(
                    row.scope,
                    row.web_acl_name,
                    row.web_acl_id,
                    row.rule_name,
                    row.rule_group_rule_name,
                ),
                "WAFNonexistentItemException",
                case,
            )
            .await;
        assert_eq!(
            body["Message"].as_str(),
            Some(NONEXISTENT_ITEM),
            "case {case:?}: expected the sentence this exception type carries, got {body}"
        );
        assert!(
            body["ManagedKeysIPV4"].is_null() && body["ManagedKeysIPV6"].is_null(),
            "case {case:?}: a refusal must carry no keys, got {body}"
        );
    }

    // Every refusal above left the stored instance reportable.
    let body = waf
        .ok(
            "GetRateBasedStatementManagedKeys",
            managed_keys_request("REGIONAL", "rate-keys-addressed", &web_acl_id, "rate-ip", None),
        )
        .await;
    assert_empty_managed_keys(&body, "the stored instance, after every refusal");
}

/// `Scope` is a modelled enum, so a value the model does not declare is refused at
/// the request boundary and no report is produced. One row per shape of a wrong
/// value: an unknown word, a declared value in the wrong case, one with a trailing
/// space, and the empty string.
#[tokio::test]
async fn a_scope_the_model_does_not_declare_is_refused() {
    let waf = Waf::new();
    let (web_acl_id, _arn, _lock) = create_web_acl(
        &waf,
        "REGIONAL",
        "rate-keys-scope",
        json!([rule("rate-ip", ip_rate_based(1_000))]),
    )
    .await;

    for scope in ["GLOBAL", "regional", "CLOUDFRONT ", ""] {
        let body = waf
            .refused(
                "GetRateBasedStatementManagedKeys",
                managed_keys_request(scope, "rate-keys-scope", &web_acl_id, "rate-ip", None),
                "ValidationException",
                scope,
            )
            .await;
        assert!(
            body["ManagedKeysIPV4"].is_null() && body["ManagedKeysIPV6"].is_null(),
            "case {scope:?}: a refusal must carry no keys, got {body}"
        );
    }
}
