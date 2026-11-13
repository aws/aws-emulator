// SPDX-License-Identifier: CC0-1.0

//! The preconditions a SAVED rule set has to satisfy, end to end over the wire.
//!
//! The rule asserted here: WAF resolves a web ACL's rule set as a WHOLE before it
//! writes it, and refuses the save — with the modelled `WAFInvalidParameterException`
//! (HTTP 400) naming the settings at fault, the value the request sent and WAF's
//! reason for it — when
//!
//! * two rules of one list claim the same `Name`, which is the identity every
//!   override, metric and sample resolves a rule by;
//! * two rules claim the same `Priority`, which IS the evaluation order ("the
//!   priorities don't need to be consecutive, but they must all be different",
//!   `Rule$Priority`);
//! * a `CustomResponse` references a body key the document's own
//!   `CustomResponseBodies` map does not define ("you first define the response body
//!   key and value in the `CustomResponseBodies` setting … then … you reference the
//!   response body using this key", `CustomResponse$CustomResponseBodyKey`);
//! * a `RuleActionOverride` on a CUSTOMER-OWNED rule-group reference names a rule
//!   that group does not declare — while the same unknown name on a MANAGED group's
//!   reference is accepted, the asymmetry `RuleActionOverride`'s own doc states;
//! * the rules price above the per-web-ACL web ACL capacity unit ceiling.
//!
//! A refused save writes nothing: no web ACL appears in `ListWebACLs` after a refused
//! create, and after a refused update the stored rules, bodies and `LockToken` are
//! exactly what they were.
//!
//! The rows span the class rather than one recorded request: for each precondition,
//! several shapes of violation (a repeated name at two positions, a repeated priority
//! at zero and above it, a reference from a rule action and from the web ACL's
//! `DefaultAction`, a name differing only in case) and an ACCEPTED control that
//! differs from the refused row only in the one thing the rule is about. The capacity
//! rows are driven at the published ceiling, one WCU past it and far past it, with the
//! price never written out: it is read from `CheckCapacity`, the report
//! `WebACL$Capacity` is derived with.
//!
//! Every call goes through the router's front door, the harness in
//! [`crate::web_acl_lifecycle`], so what these tests observe is what an SDK observes.

use serde_json::{json, Value};
use service_com_amazonaws_wafv2_awswaf_20190729::state::MAX_WEB_ACL_CAPACITY;

use crate::managed_rule_groups::managed_statement;
use crate::rule_group_lifecycle::{byte_match_statement, create_rule_group};
use crate::web_acl_lifecycle::{sqli_statement, visibility, xss_statement, Waf};

/// The scope the rows are saved in. A scope is only the namespace a save lands in,
/// and the preconditions are about the rule set itself, so one namespace carries the
/// table; `crate::web_acl_lifecycle` covers the scopes.
const SCOPE: &str = "REGIONAL";

/// The `ParameterExceptionField` values WAF names for the settings these refusals
/// are found in (`smithy_query(query="shape",shape="ParameterExceptionField")` lists
/// `RULE`, `CUSTOM_RESPONSE`, `RULE_GROUP_REFERENCE_STATEMENT` and `WEB_ACL`).
/// Written out here rather than imported, so the table does not restate the code
/// under test.
const FIELD_RULE: &str = "RULE";
const FIELD_CUSTOM_RESPONSE: &str = "CUSTOM_RESPONSE";
const FIELD_RULE_GROUP_REFERENCE_STATEMENT: &str = "RULE_GROUP_REFERENCE_STATEMENT";
const FIELD_WEB_ACL: &str = "WEB_ACL";

/// The `ErrorReason` sentences WAF carries for these refusals. See [`FIELD_RULE`].
const DUPLICATE_REASON: &str = "You have duplicated some of the information in the parameter.";
const PRIORITY_REASON: &str = "You have a duplicate priority. Priorities must be unique.";
const REFERENCE_REASON: &str = "AWS WAF couldn't find a resource that a parameter references.";
const CAPACITY_REASON: &str = "You exceeded the capacity limit for a rule group or web ACL.";

/// The rule the customer-owned rule group of these rows declares, and a name it does
/// not declare.
const DECLARED_RULE: &str = "deny-upload";

/// The error code of a refusal, as its `__type` carries it.
fn error_code(body: &Value) -> &str {
    body["__type"]
        .as_str()
        .unwrap_or_default()
        .rsplit('#')
        .next()
        .unwrap_or_default()
}

/// A rule at `priority` whose statement is `statement` and whose action is `action`.
fn acting_rule(name: &str, priority: i64, statement: Value, action: Value) -> Value {
    json!({
        "Name": name,
        "Priority": priority,
        "Statement": statement,
        "Action": action,
        "VisibilityConfig": visibility(name),
    })
}

/// A blocking rule at `priority`.
fn blocking_rule(name: &str, priority: i64, statement: Value) -> Value {
    acting_rule(name, priority, statement, json!({ "Block": {} }))
}

/// A rule whose top-level statement RUNS a rule group, which carries an
/// `OverrideAction` rather than an `Action` ("you can only use this for rule
/// statements that reference a rule group", `Rule$OverrideAction`).
fn group_running_rule(name: &str, priority: i64, statement: Value) -> Value {
    json!({
        "Name": name,
        "Priority": priority,
        "Statement": statement,
        "OverrideAction": { "None": {} },
        "VisibilityConfig": visibility(name),
    })
}

/// A `Block` action serving the custom response body `key`.
fn block_serving(key: &str) -> Value {
    json!({
        "Block": {
            "CustomResponse": { "ResponseCode": 403, "CustomResponseBodyKey": key }
        }
    })
}

/// A `CustomResponseBodies` map defining exactly `keys`.
fn bodies_defining(keys: &[&str]) -> Value {
    let mut map = serde_json::Map::new();
    for key in keys {
        map.insert(
            (*key).to_string(),
            json!({ "ContentType": "TEXT_HTML", "Content": "<html>blocked</html>" }),
        );
    }
    Value::Object(map)
}

/// A `RuleGroupReferenceStatement` on `arn` overriding the action of each of
/// `overridden`.
fn reference_overriding(arn: &str, overridden: &[&str]) -> Value {
    json!({
        "RuleGroupReferenceStatement": {
            "ARN": arn,
            "RuleActionOverrides": overridden
                .iter()
                .map(|name| json!({ "Name": name, "ActionToUse": { "Count": {} } }))
                .collect::<Vec<Value>>(),
        }
    })
}

/// A `ManagedRuleGroupStatement` overriding the action of each of `overridden`.
fn managed_overriding(group: &str, overridden: &[&str]) -> Value {
    let mut statement = managed_statement("AWS", group);
    statement["ManagedRuleGroupStatement"]["RuleActionOverrides"] = overridden
        .iter()
        .map(|name| json!({ "Name": name, "ActionToUse": { "Count": {} } }))
        .collect();
    statement
}

/// One rule set, with the document's other members, as a save carries it.
#[derive(Clone)]
struct RuleSet {
    case: String,
    default_action: Value,
    rules: Value,
    bodies: Option<Value>,
}

impl RuleSet {
    /// A rule set with the plain allowing default action and no custom response
    /// bodies.
    fn of(case: impl Into<String>, rules: Value) -> Self {
        Self {
            case: case.into(),
            default_action: json!({ "Allow": {} }),
            rules,
            bodies: None,
        }
    }

    fn with_bodies(mut self, bodies: Value) -> Self {
        self.bodies = Some(bodies);
        self
    }

    fn with_default_action(mut self, default_action: Value) -> Self {
        self.default_action = default_action;
        self
    }

    /// A `CreateWebACL` body for this rule set under `name`.
    fn create_body(&self, name: &str) -> Value {
        let mut body = json!({
            "Name": name,
            "Scope": SCOPE,
            "DefaultAction": self.default_action,
            "VisibilityConfig": visibility(name),
            "Rules": self.rules,
        });
        if let Some(bodies) = &self.bodies {
            body["CustomResponseBodies"] = bodies.clone();
        }
        body
    }

    /// An `UpdateWebACL` body for this rule set on `(name, id, lock_token)`.
    fn update_body(&self, name: &str, id: &str, lock_token: &str) -> Value {
        let mut body = self.create_body(name);
        body["Id"] = json!(id);
        body["LockToken"] = json!(lock_token);
        body
    }
}

/// One rule set a save must refuse, and the refusal it must answer.
struct Refusal {
    set: RuleSet,
    /// `WAFInvalidParameterException$Field`.
    field: &'static str,
    /// `WAFInvalidParameterException$Parameter` — the value the request sent.
    parameter: String,
    /// `WAFInvalidParameterException$Reason`.
    reason: &'static str,
}

/// Assert that `body` is the invalid-parameter refusal `expected` describes.
fn assert_refusal(body: &Value, expected: &Refusal) {
    let case = &expected.set.case;
    assert_eq!(
        error_code(body),
        "WAFInvalidParameterException",
        "case {case:?}: a rule set WAF will not resolve is an invalid parameter, got {body}"
    );
    assert_eq!(
        (
            body["Field"].as_str(),
            body["Parameter"].as_str(),
            body["Reason"].as_str()
        ),
        (
            Some(expected.field),
            Some(expected.parameter.as_str()),
            Some(expected.reason)
        ),
        "case {case:?}: expected the refusal to name field {:?}, parameter {:?} and \
         reason {:?}, got {body}",
        expected.field,
        expected.parameter,
        expected.reason
    );
    assert_eq!(
        body["message"].as_str().unwrap_or_default(),
        format!(
            "Error reason: {}, field: {}, parameter: {}",
            expected.reason, expected.field, expected.parameter
        ),
        "case {case:?}: the message must be composed from the three members, got {body}"
    );
}

/// Every rule set the save path must refuse, with the refusal each earns.
///
/// `group_arn` is a customer-owned rule group declaring exactly [`DECLARED_RULE`].
fn refusals(group_arn: &str) -> Vec<Refusal> {
    let dangling = "undefined-body-key";
    let defined = "blocked-html";
    vec![
        // Two rules claiming one name, at the two positions a repeat can sit in.
        Refusal {
            set: RuleSet::of(
                "two rules sharing a name, at distinct priorities",
                json!([
                    blocking_rule("dup-name", 0, sqli_statement()),
                    blocking_rule("dup-name", 1, xss_statement()),
                ]),
            ),
            field: FIELD_RULE,
            parameter: "dup-name".to_string(),
            reason: DUPLICATE_REASON,
        },
        Refusal {
            set: RuleSet::of(
                "the first and last of three rules share a name",
                json!([
                    blocking_rule("repeated", 0, sqli_statement()),
                    blocking_rule("between", 1, xss_statement()),
                    blocking_rule("repeated", 2, byte_match_statement()),
                ]),
            ),
            field: FIELD_RULE,
            parameter: "repeated".to_string(),
            reason: DUPLICATE_REASON,
        },
        // Two rules claiming one priority: at zero, above zero, and among three.
        Refusal {
            set: RuleSet::of(
                "two rules at priority 0",
                json!([
                    blocking_rule("first", 0, sqli_statement()),
                    blocking_rule("second", 0, xss_statement()),
                ]),
            ),
            field: FIELD_RULE,
            parameter: "0".to_string(),
            reason: PRIORITY_REASON,
        },
        Refusal {
            set: RuleSet::of(
                "two rules at priority 7",
                json!([
                    blocking_rule("first", 7, sqli_statement()),
                    blocking_rule("second", 7, xss_statement()),
                ]),
            ),
            field: FIELD_RULE,
            parameter: "7".to_string(),
            reason: PRIORITY_REASON,
        },
        Refusal {
            set: RuleSet::of(
                "the last two of three rules share priority 5",
                json!([
                    blocking_rule("first", 0, sqli_statement()),
                    blocking_rule("second", 5, xss_statement()),
                    blocking_rule("third", 5, byte_match_statement()),
                ]),
            ),
            field: FIELD_RULE,
            parameter: "5".to_string(),
            reason: PRIORITY_REASON,
        },
        // A custom-response reference that resolves to nothing, from each position
        // one can be written in and in each way it can fail to resolve.
        Refusal {
            set: RuleSet::of(
                "a rule action serving a body key the map does not define",
                json!([acting_rule(
                    "blocks",
                    0,
                    sqli_statement(),
                    block_serving(dangling)
                )]),
            )
            .with_bodies(bodies_defining(&[defined])),
            field: FIELD_CUSTOM_RESPONSE,
            parameter: dangling.to_string(),
            reason: REFERENCE_REASON,
        },
        Refusal {
            set: RuleSet::of(
                "the bodies map is left out from under a live reference",
                json!([acting_rule(
                    "blocks",
                    0,
                    sqli_statement(),
                    block_serving(defined)
                )]),
            ),
            field: FIELD_CUSTOM_RESPONSE,
            parameter: defined.to_string(),
            reason: REFERENCE_REASON,
        },
        Refusal {
            set: RuleSet::of(
                "the web ACL's own DefaultAction serves an undefined key",
                json!([blocking_rule("inspects", 0, sqli_statement())]),
            )
            .with_bodies(bodies_defining(&[defined]))
            .with_default_action(block_serving(dangling)),
            field: FIELD_CUSTOM_RESPONSE,
            parameter: dangling.to_string(),
            reason: REFERENCE_REASON,
        },
        Refusal {
            set: RuleSet::of(
                "a body key differing from the defined one only in case",
                json!([acting_rule(
                    "blocks",
                    0,
                    sqli_statement(),
                    block_serving("Blocked-HTML")
                )]),
            )
            .with_bodies(bodies_defining(&[defined])),
            field: FIELD_CUSTOM_RESPONSE,
            parameter: "Blocked-HTML".to_string(),
            reason: REFERENCE_REASON,
        },
        // An override naming no rule of the customer-owned group it overrides.
        Refusal {
            set: RuleSet::of(
                "an override naming a rule the referenced group does not declare",
                json!([group_running_rule(
                    "runs-group",
                    0,
                    reference_overriding(group_arn, &["no-such-rule"])
                )]),
            ),
            field: FIELD_RULE_GROUP_REFERENCE_STATEMENT,
            parameter: "no-such-rule".to_string(),
            reason: REFERENCE_REASON,
        },
        Refusal {
            set: RuleSet::of(
                "an override differing from a declared rule only in case",
                json!([group_running_rule(
                    "runs-group",
                    0,
                    reference_overriding(group_arn, &["Deny-Upload"])
                )]),
            ),
            field: FIELD_RULE_GROUP_REFERENCE_STATEMENT,
            parameter: "Deny-Upload".to_string(),
            reason: REFERENCE_REASON,
        },
        Refusal {
            set: RuleSet::of(
                "the second of two overrides is unknown",
                json!([group_running_rule(
                    "runs-group",
                    0,
                    reference_overriding(group_arn, &[DECLARED_RULE, "also-missing"])
                )]),
            ),
            field: FIELD_RULE_GROUP_REFERENCE_STATEMENT,
            parameter: "also-missing".to_string(),
            reason: REFERENCE_REASON,
        },
    ]
}

/// Every rule set the save path must ACCEPT: for each refused row above, one that
/// differs from it only in the thing that rule is about.
fn accepted(group_arn: &str) -> Vec<RuleSet> {
    let defined = "blocked-html";
    vec![
        RuleSet::of(
            "distinct names at distinct priorities",
            json!([
                blocking_rule("first", 0, sqli_statement()),
                blocking_rule("second", 1, xss_statement()),
            ]),
        ),
        RuleSet::of(
            "names differing only in case are different names",
            json!([
                blocking_rule("Dup-Name", 0, sqli_statement()),
                blocking_rule("dup-name", 1, xss_statement()),
            ]),
        ),
        RuleSet::of(
            "distinct, non-consecutive priorities",
            json!([
                blocking_rule("first", 7, sqli_statement()),
                blocking_rule("second", 8, xss_statement()),
            ]),
        ),
        RuleSet::of(
            "a rule action and the default action both serving a defined key",
            json!([acting_rule(
                "blocks",
                0,
                sqli_statement(),
                block_serving(defined)
            )]),
        )
        .with_bodies(bodies_defining(&[defined, "too-many"]))
        .with_default_action(block_serving(defined)),
        RuleSet::of(
            "an override naming the rule the referenced group declares",
            json!([group_running_rule(
                "runs-group",
                0,
                reference_overriding(group_arn, &[DECLARED_RULE])
            )]),
        ),
        RuleSet::of(
            "an unknown override name on a MANAGED group, which WAF ignores",
            json!([group_running_rule(
                "runs-managed",
                0,
                managed_overriding("AWSManagedRulesAdminProtectionRuleSet", &["no-such-rule"])
            )]),
        ),
    ]
}

/// A customer-owned rule group declaring exactly [`DECLARED_RULE`].
async fn declaring_group(waf: &Waf, name: &str) -> String {
    let (_id, arn, _lock) = create_rule_group(
        waf,
        SCOPE,
        name,
        100,
        json!([blocking_rule(DECLARED_RULE, 0, byte_match_statement())]),
    )
    .await;
    arn
}

/// The names `ListWebACLs` reports for the scope.
async fn listed_names(waf: &Waf) -> Vec<String> {
    let listed = waf.ok("ListWebACLs", json!({ "Scope": SCOPE })).await;
    listed["WebACLs"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|summary| summary["Name"].as_str().unwrap_or_default().to_string())
        .collect()
}

/// `CreateWebACL` refuses every rule set the preconditions reject, names the value
/// at fault, and leaves no web ACL behind — while the controls beside them are saved
/// and read back verbatim.
#[tokio::test]
async fn create_web_acl_refuses_an_unresolvable_rule_set_and_stores_nothing() {
    let waf = Waf::new();
    let group_arn = declaring_group(&waf, "precondition-group").await;

    for (index, expected) in refusals(&group_arn).iter().enumerate() {
        let case = &expected.set.case;
        let name = format!("refused-acl-{index}");
        let (status, body) = waf
            .call("CreateWebACL", expected.set.create_body(&name))
            .await;
        assert_eq!(
            status, 400,
            "case {case:?}: the save must be refused, got status {status} and body {body}"
        );
        assert_refusal(&body, expected);
        assert!(
            body["Summary"].is_null(),
            "case {case:?}: a refused create must answer no summary, got {body}"
        );
        let listed = listed_names(&waf).await;
        assert!(
            !listed.contains(&name),
            "case {case:?}: a refused create must leave no web ACL behind, got {listed:?}"
        );
    }

    for (index, set) in accepted(&group_arn).iter().enumerate() {
        let case = &set.case;
        let name = format!("accepted-acl-{index}");
        let created = waf.ok("CreateWebACL", set.create_body(&name)).await;
        let id = created["Summary"]["Id"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: expected a Summary.Id, got {created}"))
            .to_string();
        let got = waf
            .ok(
                "GetWebACL",
                json!({ "Name": &name, "Scope": SCOPE, "Id": &id }),
            )
            .await;
        assert_eq!(
            got["WebACL"]["Rules"], set.rules,
            "case {case:?}: an accepted rule set must read back verbatim, got {got}"
        );
        if let Some(bodies) = &set.bodies {
            assert_eq!(
                got["WebACL"]["CustomResponseBodies"], *bodies,
                "case {case:?}: the bodies the reference resolves against must read \
                 back verbatim, got {got}"
            );
        }
    }
}

/// `UpdateWebACL` refuses the same rule sets, and a refused update changes nothing
/// at all: the stored rules, the stored bodies and the `LockToken` are what they
/// were, and the token the caller holds still works.
#[tokio::test]
async fn update_web_acl_refuses_the_same_rule_sets_and_changes_nothing() {
    let waf = Waf::new();
    let group_arn = declaring_group(&waf, "precondition-group").await;
    let name = "updated-acl";
    let defined = "blocked-html";
    // The stored starting point: a rule set that satisfies every precondition, with
    // a live custom-response reference and a resolvable override.
    let before = RuleSet::of(
        "the stored rule set",
        json!([
            acting_rule("blocks", 0, sqli_statement(), block_serving(defined)),
            group_running_rule(
                "runs-group",
                1,
                reference_overriding(&group_arn, &[DECLARED_RULE])
            ),
        ]),
    )
    .with_bodies(bodies_defining(&[defined]));
    let created = waf.ok("CreateWebACL", before.create_body(name)).await;
    let id = created["Summary"]["Id"]
        .as_str()
        .expect("Summary.Id")
        .to_string();
    let lock_token = created["Summary"]["LockToken"]
        .as_str()
        .expect("Summary.LockToken")
        .to_string();

    for expected in refusals(&group_arn).iter() {
        let case = &expected.set.case;
        let (status, body) = waf
            .call(
                "UpdateWebACL",
                expected.set.update_body(name, &id, &lock_token),
            )
            .await;
        assert_eq!(
            status, 400,
            "case {case:?}: the update must be refused, got status {status} and body {body}"
        );
        assert_refusal(&body, expected);
        assert!(
            body["NextLockToken"].is_null(),
            "case {case:?}: a refused update must answer no lock token, got {body}"
        );

        let got = waf
            .ok(
                "GetWebACL",
                json!({ "Name": name, "Scope": SCOPE, "Id": &id }),
            )
            .await;
        assert_eq!(
            (
                got["WebACL"]["Rules"].clone(),
                got["WebACL"]["CustomResponseBodies"].clone(),
                got["LockToken"].as_str()
            ),
            (
                before.rules.clone(),
                before.bodies.clone().unwrap_or(Value::Null),
                Some(lock_token.as_str())
            ),
            "case {case:?}: a refused update must change nothing at all, got {got}"
        );
    }

    // The token the caller still holds works, so nothing was consumed either.
    let after = RuleSet::of(
        "a resolvable replacement",
        json!([blocking_rule("replacement", 0, xss_statement())]),
    );
    let updated = waf
        .ok("UpdateWebACL", after.update_body(name, &id, &lock_token))
        .await;
    assert!(
        updated["NextLockToken"].as_str().is_some_and(|next| next != lock_token),
        "case \"a resolvable replacement\": an accepted update must rotate the lock \
         token, got {updated}"
    );
}

/// An override naming no rule of a CUSTOMER-OWNED group is refused, while the same
/// name on a MANAGED group's reference is accepted and ignored: "with managed rule
/// groups, WAF silently ignores any override that uses an invalid rule name. With
/// customer-owned rule groups, invalid rule names in your overrides will cause web
/// ACL updates to fail" (`RuleActionOverride`'s own doc).
///
/// The accepted managed row is read back, so the override WAF ignores is still
/// STORED rather than dropped; the refused customer-owned row names the override.
#[tokio::test]
async fn an_unknown_override_name_is_refused_for_an_owned_group_and_ignored_for_a_managed_one() {
    let waf = Waf::new();
    let group_arn = declaring_group(&waf, "asymmetry-group").await;
    let unknown = "no-such-rule";

    // Two managed groups, so the acceptance is not a property of one of them.
    for (index, group) in [
        "AWSManagedRulesAdminProtectionRuleSet",
        "AWSManagedRulesAmazonIpReputationList",
    ]
    .iter()
    .enumerate()
    {
        let case = format!("an unknown override on the managed group {group:?}");
        let name = format!("managed-override-{index}");
        let statement = managed_overriding(group, &[unknown]);
        let rules = json!([group_running_rule("runs-managed", 0, statement)]);
        let created = waf
            .ok(
                "CreateWebACL",
                json!({
                    "Name": &name,
                    "Scope": SCOPE,
                    "DefaultAction": { "Allow": {} },
                    "VisibilityConfig": visibility(&name),
                    "Rules": &rules,
                }),
            )
            .await;
        let id = created["Summary"]["Id"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: expected a Summary.Id, got {created}"))
            .to_string();
        let got = waf
            .ok(
                "GetWebACL",
                json!({ "Name": &name, "Scope": SCOPE, "Id": &id }),
            )
            .await;
        assert_eq!(
            got["WebACL"]["Rules"], rules,
            "case {case:?}: an override WAF ignores must still be stored as sent, got {got}"
        );
    }

    // The same unknown name against the customer-owned group is refused, whichever
    // save path writes it.
    let set = RuleSet::of(
        "an unknown override on a customer-owned group",
        json!([group_running_rule(
            "runs-group",
            0,
            reference_overriding(&group_arn, &[unknown])
        )]),
    );
    let expected = Refusal {
        set: set.clone(),
        field: FIELD_RULE_GROUP_REFERENCE_STATEMENT,
        parameter: unknown.to_string(),
        reason: REFERENCE_REASON,
    };
    let (status, body) = waf
        .call("CreateWebACL", set.create_body("owned-override"))
        .await;
    assert_eq!(
        status, 400,
        "case {:?}: CreateWebACL must refuse it, got status {status} and body {body}",
        set.case
    );
    assert_refusal(&body, &expected);
}

/// The rules of a web ACL may price up to the published web ACL capacity unit
/// ceiling and no further: a save whose rules cost more is refused as the invalid
/// parameter, naming the price those rules cost — the figure `CheckCapacity`
/// publishes for them — and nothing is stored.
///
/// One row at the ceiling, one WCU past it, and far past it, each reached through the
/// capacity a referenced rule group declares (`RuleGroup$Capacity` is stored as
/// declared), plus a row that reaches the same total through two references and one
/// that adds a statement of its own on top of a reference at the ceiling.
#[tokio::test]
async fn the_rules_of_a_web_acl_may_price_up_to_the_published_ceiling_and_no_further() {
    let waf = Waf::new();
    let reference = |arn: &str| json!({ "RuleGroupReferenceStatement": { "ARN": arn } });
    let mut declaring: Vec<String> = Vec::new();
    for capacity in [
        MAX_WEB_ACL_CAPACITY,
        MAX_WEB_ACL_CAPACITY + 1,
        MAX_WEB_ACL_CAPACITY / 2,
        MAX_WEB_ACL_CAPACITY * 3,
    ] {
        let (_id, arn, _lock) = create_rule_group(
            &waf,
            SCOPE,
            &format!("ceiling-group-{capacity}"),
            capacity,
            json!([]),
        )
        .await;
        declaring.push(arn);
    }
    let (at_ceiling, one_past, half, far_past) = (
        declaring[0].clone(),
        declaring[1].clone(),
        declaring[2].clone(),
        declaring[3].clone(),
    );

    // `(case, rules, saved)`.
    let rows: Vec<(&str, Value, bool)> = vec![
        (
            "one reference priced exactly at the ceiling",
            json!([group_running_rule("rg", 0, reference(&at_ceiling))]),
            true,
        ),
        (
            "two references summing to exactly the ceiling",
            json!([
                group_running_rule("rg-a", 0, reference(&half)),
                group_running_rule("rg-b", 1, reference(&half)),
            ]),
            true,
        ),
        (
            "one reference priced one WCU past the ceiling",
            json!([group_running_rule("rg", 0, reference(&one_past))]),
            false,
        ),
        (
            "a reference at the ceiling plus a statement of its own",
            json!([
                group_running_rule("rg", 0, reference(&at_ceiling)),
                blocking_rule("sqli", 1, sqli_statement()),
            ]),
            false,
        ),
        (
            "one reference priced far past the ceiling",
            json!([group_running_rule("rg", 0, reference(&far_past))]),
            false,
        ),
    ];

    for (index, (case, rules, saved)) in rows.iter().enumerate() {
        // The price is the one the service PUBLISHES for these rules, never a
        // figure written out here.
        let priced = waf
            .ok(
                "CheckCapacity",
                json!({ "Scope": SCOPE, "Rules": rules.clone() }),
            )
            .await;
        let price = priced["Capacity"]
            .as_i64()
            .unwrap_or_else(|| panic!("case {case:?}: CheckCapacity must price it, got {priced}"));
        assert_eq!(
            price > MAX_WEB_ACL_CAPACITY,
            !*saved,
            "case {case:?}: the row must sit on the side of the ceiling it claims, \
             priced {price} against {MAX_WEB_ACL_CAPACITY}"
        );

        let name = format!("ceiling-acl-{index}");
        let set = RuleSet::of(*case, rules.clone());
        if *saved {
            let created = waf.ok("CreateWebACL", set.create_body(&name)).await;
            let id = created["Summary"]["Id"]
                .as_str()
                .unwrap_or_else(|| panic!("case {case:?}: expected a Summary.Id, got {created}"))
                .to_string();
            let got = waf
                .ok(
                    "GetWebACL",
                    json!({ "Name": &name, "Scope": SCOPE, "Id": &id }),
                )
                .await;
            assert_eq!(
                got["WebACL"]["Capacity"].as_i64(),
                Some(price),
                "case {case:?}: a web ACL at the ceiling must store the price it was \
                 checked at, got {got}"
            );
            continue;
        }

        let (status, body) = waf.call("CreateWebACL", set.create_body(&name)).await;
        assert_eq!(
            status, 400,
            "case {case:?}: rules above the ceiling must be refused, got status \
             {status} and body {body}"
        );
        assert_refusal(
            &body,
            &Refusal {
                set,
                field: FIELD_WEB_ACL,
                parameter: price.to_string(),
                reason: CAPACITY_REASON,
            },
        );
        let listed = listed_names(&waf).await;
        assert!(
            !listed.contains(&name),
            "case {case:?}: a refused create must leave no web ACL behind, got {listed:?}"
        );
    }

    // And an UPDATE is judged the same way, leaving the stored web ACL untouched.
    let name = "ceiling-updated-acl";
    let before = RuleSet::of(
        "a web ACL below the ceiling",
        json!([blocking_rule("sqli", 0, sqli_statement())]),
    );
    let created = waf.ok("CreateWebACL", before.create_body(name)).await;
    let id = created["Summary"]["Id"]
        .as_str()
        .expect("Summary.Id")
        .to_string();
    let lock_token = created["Summary"]["LockToken"]
        .as_str()
        .expect("Summary.LockToken")
        .to_string();
    let over = RuleSet::of(
        "an update past the ceiling",
        json!([group_running_rule("rg", 0, reference(&one_past))]),
    );
    let (status, body) = waf
        .call("UpdateWebACL", over.update_body(name, &id, &lock_token))
        .await;
    assert_eq!(
        status, 400,
        "case {:?}: an update past the ceiling must be refused, got status {status} \
         and body {body}",
        over.case
    );
    assert_refusal(
        &body,
        &Refusal {
            set: over,
            field: FIELD_WEB_ACL,
            parameter: (MAX_WEB_ACL_CAPACITY + 1).to_string(),
            reason: CAPACITY_REASON,
        },
    );
    let got = waf
        .ok(
            "GetWebACL",
            json!({ "Name": name, "Scope": SCOPE, "Id": &id }),
        )
        .await;
    assert_eq!(
        (got["WebACL"]["Rules"].clone(), got["LockToken"].as_str()),
        (before.rules.clone(), Some(lock_token.as_str())),
        "case \"an update past the ceiling\": the stored web ACL must be untouched, got {got}"
    );
}
