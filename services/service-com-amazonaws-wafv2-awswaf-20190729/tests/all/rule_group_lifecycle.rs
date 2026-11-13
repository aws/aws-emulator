// SPDX-License-Identifier: CC0-1.0

//! The WAFv2 RuleGroup surface, end to end over the wire.
//!
//! `CreateRuleGroup` "creates a RuleGroup per the specifications provided", and the
//! rule it is asserted against here is: a successful save PERSISTS the rule group
//! the request described and answers its `RuleGroupSummary`, so every later read
//! and every reference resolves against stored state — `GetRuleGroup` answers the
//! stored shape (with the members WAF derives server-side) and its current lock
//! token, a `RuleGroupReferenceStatement` in a web ACL rule resolves to the stored
//! group and costs its declared `Capacity`, and nothing else can be saved under the
//! same `(Scope, Name)`.
//!
//! Each suite asserts that over a TABLE spanning the class — scope, declared
//! capacity, description, rule count and statement shape, labels, custom response
//! bodies — rather than over one request. The harness is the one in
//! [`crate::web_acl_lifecycle`]: every call goes through the router's front door, so
//! what these tests observe is what an SDK observes.

use std::collections::BTreeSet;

use serde_json::{json, Value};

use crate::web_acl_lifecycle::{
    create_ip_set, create_web_acl, is_model_uuid, ordered, rule, sqli_statement, visibility, Waf,
    TEST_REGION,
};

/// The region a CLOUDFRONT-scoped resource's ARN carries whatever the caller signed
/// for: "API and SDKs - For all calls, use the Region endpoint us-east-1"
/// (`Scope`'s own doc).
const CLOUDFRONT_ARN_REGION: &str = "us-east-1";

/// The `(ARN region, ARN resource prefix)` a scope's ARNs carry.
fn arn_parts(scope: &str) -> (&'static str, &'static str) {
    if scope == "CLOUDFRONT" {
        (CLOUDFRONT_ARN_REGION, "global")
    } else {
        (TEST_REGION, "regional")
    }
}

/// The ARN this service mints for a rule group, as
/// `arn:aws:wafv2:<region>:<account>:<global|regional>/rulegroup/<name>/<id>`.
pub(crate) fn rule_group_arn(scope: &str, name: &str, id: &str) -> String {
    let (region, prefix) = arn_parts(scope);
    let account = engine::EMULATOR_ACCOUNT_ID;
    format!("arn:aws:wafv2:{region}:{account}:{prefix}/rulegroup/{name}/{id}")
}

/// `awswaf:<account ID>:rulegroup:<rule group name>:` — the prefix
/// `RuleGroup$LabelNamespace` documents for a rule group's labels.
pub(crate) fn label_namespace(name: &str) -> String {
    format!("awswaf:{}:rulegroup:{name}:", engine::EMULATOR_ACCOUNT_ID)
}

/// `[{ "Name": … }, …]` — the `LabelSummaries` projection of a label list.
pub(crate) fn label_summaries(names: &[&str]) -> Value {
    Value::Array(names.iter().map(|name| json!({ "Name": name })).collect())
}

/// A `ByteMatchStatement` over the URI path.
pub(crate) fn byte_match_statement() -> Value {
    json!({
        "ByteMatchStatement": {
            "SearchString": "YQ==",
            "FieldToMatch": { "UriPath": {} },
            "TextTransformations": [{ "Priority": 0, "Type": "NONE" }],
            "PositionalConstraint": "CONTAINS",
        }
    })
}

/// A rule carrying `labels` in its `RuleLabels`.
pub(crate) fn labelled_rule(name: &str, statement: Value, labels: &[&str]) -> Value {
    let mut built = rule(name, statement);
    built["RuleLabels"] = label_summaries(labels);
    built
}

/// A rule whose statement matches on the label `key`.
///
/// `LabelMatchStatement$Scope` is `LABEL` or `NAMESPACE`
/// (`smithy_query(query="shape",shape="LabelMatchScope")`); `LABEL` matches one
/// label.
pub(crate) fn label_match_rule(name: &str, key: &str) -> Value {
    rule(name, json!({ "LabelMatchStatement": { "Scope": "LABEL", "Key": key } }))
}

/// A rule that RUNS a rule group.
///
/// A rule-group reference carries an `OverrideAction` rather than an `Action`, the
/// way `Rule$OverrideAction` documents ("you can only use this for rule statements
/// that reference a rule group").
fn rule_group_reference_rule(name: &str, priority: i64, arn: &str) -> Value {
    json!({
        "Name": name,
        "Priority": priority,
        "Statement": { "RuleGroupReferenceStatement": { "ARN": arn } },
        "OverrideAction": { "None": {} },
        "VisibilityConfig": visibility(name),
    })
}

/// A `CreateRuleGroup` body. `Description` and `CustomResponseBodies` are present
/// only when the case supplies them, so an absent member is really absent on the
/// wire.
pub(crate) fn create_body(
    name: &str,
    scope: &str,
    capacity: i64,
    description: Option<&str>,
    rules: &Value,
    custom_response_bodies: Option<&Value>,
) -> Value {
    let mut body = json!({
        "Name": name,
        "Scope": scope,
        "Capacity": capacity,
        "VisibilityConfig": visibility(name),
        "Rules": rules,
    });
    if let Some(description) = description {
        body["Description"] = json!(description);
    }
    if let Some(bodies) = custom_response_bodies {
        body["CustomResponseBodies"] = bodies.clone();
    }
    body
}

/// Create a rule group and answer `(id, arn, lock token)`.
pub(crate) async fn create_rule_group(
    waf: &Waf,
    scope: &str,
    name: &str,
    capacity: i64,
    rules: Value,
) -> (String, String, String) {
    let created = waf
        .ok(
            "CreateRuleGroup",
            create_body(name, scope, capacity, None, &rules, None),
        )
        .await;
    let summary = &created["Summary"];
    (
        summary["Id"].as_str().expect("Summary.Id").to_string(),
        summary["ARN"].as_str().expect("Summary.ARN").to_string(),
        summary["LockToken"]
            .as_str()
            .expect("Summary.LockToken")
            .to_string(),
    )
}

/// One row of the create/read table.
struct RuleGroupCase {
    case: &'static str,
    scope: &'static str,
    capacity: i64,
    description: Option<&'static str>,
    rules: Value,
    custom_response_bodies: Option<Value>,
    /// What `AvailableLabels` must report: the labels the rules ADD, deduplicated.
    available_labels: Vec<&'static str>,
    /// What `ConsumedLabels` must report: the labels the rules MATCH AGAINST.
    consumed_labels: Vec<&'static str>,
}

/// A successful `CreateRuleGroup` persists the rule group it was given, and
/// `GetRuleGroup` answers it back — by the `(Scope, Name, Id)` triple and by the
/// ARN alike — together with the current lock token.
///
/// The rows differ along everything the rule generalises over: the scope (both
/// values `Scope` declares), the declared `Capacity` (the published minimum 1 and
/// larger values), whether a `Description` and `CustomResponseBodies` are supplied,
/// how many rules there are (none, one, three) and what their statements are
/// (match, nested logical, label match), and which labels the rules add and
/// consume. The members WAF derives server-side are asserted too: `Description` is
/// `""` when the save named none, `Rules` is a list even when empty,
/// `LabelNamespace` is `RuleGroup$LabelNamespace`'s documented prefix, and
/// `AvailableLabels` / `ConsumedLabels` come from the stored rules.
#[tokio::test]
async fn create_rule_group_persists_the_group_and_get_reads_it_back() {
    let cases = [
        RuleGroupCase {
            case: "no rules, no description, the minimum capacity",
            scope: "REGIONAL",
            capacity: 1,
            description: None,
            rules: json!([]),
            custom_response_bodies: None,
            available_labels: vec![],
            consumed_labels: vec![],
        },
        RuleGroupCase {
            case: "one SQLi rule, with a description",
            scope: "REGIONAL",
            capacity: 20,
            description: Some("blocks SQL injection"),
            rules: json!([rule("sqli", sqli_statement())]),
            custom_response_bodies: None,
            available_labels: vec![],
            consumed_labels: vec![],
        },
        RuleGroupCase {
            case: "three rules, two of which add labels",
            scope: "REGIONAL",
            capacity: 500,
            description: Some("a three rule group"),
            rules: ordered(json!([
                labelled_rule("first", byte_match_statement(), &["suspect", "audited"]),
                labelled_rule("second", sqli_statement(), &["suspect"]),
                rule("third", byte_match_statement()),
            ])),
            custom_response_bodies: None,
            // Deduplicated and in a stable order.
            available_labels: vec!["audited", "suspect"],
            consumed_labels: vec![],
        },
        RuleGroupCase {
            case: "a rule that matches on a label, and one that adds another",
            scope: "REGIONAL",
            capacity: 50,
            description: None,
            rules: ordered(json!([
                label_match_rule("consumer", "upstream-label"),
                labelled_rule("producer", byte_match_statement(), &["downstream-label"]),
            ])),
            custom_response_bodies: None,
            available_labels: vec!["downstream-label"],
            consumed_labels: vec!["upstream-label"],
        },
        RuleGroupCase {
            case: "a nested logical statement, in CLOUDFRONT scope",
            scope: "CLOUDFRONT",
            capacity: 1500,
            description: Some("a nested group"),
            rules: json!([rule(
                "nested",
                json!({
                    "AndStatement": {
                        "Statements": [
                            byte_match_statement(),
                            {
                                "NotStatement": {
                                    "Statement": {
                                        "LabelMatchStatement": {
                                            "Scope": "NAMESPACE",
                                            "Key": "awswaf:managed:",
                                        }
                                    }
                                }
                            },
                        ]
                    }
                })
            )]),
            custom_response_bodies: None,
            available_labels: vec![],
            // A label match nested two levels deep is still a consumed label.
            consumed_labels: vec!["awswaf:managed:"],
        },
        RuleGroupCase {
            case: "custom response bodies, used by a blocking rule",
            scope: "REGIONAL",
            capacity: 10,
            description: None,
            rules: json!([{
                "Name": "custom-block",
                "Priority": 0,
                "Statement": byte_match_statement(),
                "Action": {
                    "Block": {
                        "CustomResponse": {
                            "ResponseCode": 429,
                            "CustomResponseBodyKey": "too-many",
                        }
                    }
                },
                "VisibilityConfig": visibility("custom-block"),
            }]),
            custom_response_bodies: Some(json!({
                "too-many": { "ContentType": "TEXT_PLAIN", "Content": "slow down" }
            })),
            available_labels: vec![],
            consumed_labels: vec![],
        },
    ];

    let waf = Waf::new();
    let mut ids: BTreeSet<String> = BTreeSet::new();
    let mut tokens: BTreeSet<String> = BTreeSet::new();
    for (index, row) in cases.iter().enumerate() {
        let case = row.case;
        let name = format!("rule-group-{index}");
        let created = waf
            .ok(
                "CreateRuleGroup",
                create_body(
                    &name,
                    row.scope,
                    row.capacity,
                    row.description,
                    &row.rules,
                    row.custom_response_bodies.as_ref(),
                ),
            )
            .await;

        let summary = created["Summary"].clone();
        let id = summary["Id"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        let lock_token = summary["LockToken"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        assert!(
            is_model_uuid(&id),
            "case {case:?}: Summary.Id must be an EntityId, got {summary}"
        );
        assert!(
            is_model_uuid(&lock_token),
            "case {case:?}: Summary.LockToken must be a LockToken, got {summary}"
        );
        assert!(
            ids.insert(id.clone()),
            "case {case:?}: every rule group needs its own id, saw {id:?} twice"
        );
        assert!(
            tokens.insert(lock_token.clone()),
            "case {case:?}: every save needs its own lock token, saw {lock_token:?} twice"
        );
        let expected_description = json!(row.description.unwrap_or_default());
        let expected_arn = rule_group_arn(row.scope, &name, &id);
        assert_eq!(
            summary,
            json!({
                "Name": &name,
                "Id": &id,
                "Description": expected_description,
                "LockToken": &lock_token,
                "ARN": &expected_arn,
            }),
            "case {case:?}: CreateRuleGroup must answer the RuleGroupSummary of what it \
             stored, got {summary}"
        );

        // The round trip: the same rule group, read back through the implemented
        // reader, by its triple and by its ARN.
        let by_triple = waf
            .ok(
                "GetRuleGroup",
                json!({ "Name": &name, "Scope": row.scope, "Id": &id }),
            )
            .await;
        let by_arn = waf
            .ok("GetRuleGroup", json!({ "ARN": &expected_arn }))
            .await;
        assert_eq!(
            by_triple, by_arn,
            "case {case:?}: both addressing forms must answer the same rule group, got \
             {by_triple} and {by_arn}"
        );
        assert_eq!(
            by_triple["LockToken"],
            json!(&lock_token),
            "case {case:?}: GetRuleGroup must answer the current lock token, got {by_triple}"
        );
        let mut expected_rules = row.rules.clone();
        if expected_rules.as_array().is_some_and(|rules| rules.is_empty()) {
            expected_rules = json!([]);
        }
        let mut expected_group = json!({
            "Name": &name,
            "Id": &id,
            "ARN": &expected_arn,
            // Declared by the request and immutable: "you define this, and you
            // cannot change it after creation" (`Capacity`'s doc).
            "Capacity": row.capacity,
            "Description": expected_description,
            "Rules": expected_rules,
            "VisibilityConfig": visibility(&name),
            "LabelNamespace": label_namespace(&name),
            "AvailableLabels": label_summaries(&row.available_labels),
            "ConsumedLabels": label_summaries(&row.consumed_labels),
        });
        if let Some(bodies) = row.custom_response_bodies.as_ref() {
            expected_group["CustomResponseBodies"] = bodies.clone();
        }
        assert_eq!(
            by_triple["RuleGroup"], expected_group,
            "case {case:?}: GetRuleGroup must answer the stored rule group, got {by_triple}"
        );
    }
}

/// A rule group's `(Scope, Name)` is unique, and a scope is a namespace.
///
/// One row per scope, so the same name is free in the other one; a second save of a
/// name already taken in the SAME scope is refused with the modelled
/// `WAFDuplicateItemException` and leaves the stored group untouched.
#[tokio::test]
async fn rule_group_names_are_unique_within_a_scope_and_free_across_scopes() {
    let waf = Waf::new();
    let mut ids: BTreeSet<String> = BTreeSet::new();
    for scope in ["REGIONAL", "CLOUDFRONT"] {
        let (id, arn, lock_token) =
            create_rule_group(&waf, scope, "shared-name", 10, json!([])).await;
        assert_eq!(
            arn,
            rule_group_arn(scope, "shared-name", &id),
            "case {scope}: the rule group ARN must carry the scope's region and prefix"
        );
        assert!(
            ids.insert(id.clone()),
            "case {scope}: every scope's rule group needs its own id, saw {id:?} twice"
        );

        // A second create of the same name in the SAME scope is a duplicate, whatever
        // else the request carries.
        waf.refused(
            "CreateRuleGroup",
            create_body(
                "shared-name",
                scope,
                999,
                Some("a different description"),
                &json!([rule("sqli", sqli_statement())]),
                None,
            ),
            "WAFDuplicateItemException",
            scope,
        )
        .await;

        // The refused duplicate changed nothing: the stored group still reports the
        // capacity, rules and lock token of the save that created it.
        let got = waf
            .ok(
                "GetRuleGroup",
                json!({ "Name": "shared-name", "Scope": scope, "Id": &id }),
            )
            .await;
        assert_eq!(
            got["RuleGroup"]["Capacity"],
            json!(10),
            "case {scope}: a refused duplicate must not change the stored group, got {got}"
        );
        assert_eq!(
            got["RuleGroup"]["Rules"],
            json!([]),
            "case {scope}: a refused duplicate must not change the stored rules, got {got}"
        );
        assert_eq!(
            got["LockToken"],
            json!(lock_token),
            "case {scope}: a refused duplicate must not rotate the lock token, got {got}"
        );
    }
}

/// `GetRuleGroup` resolves ONLY a rule group this service holds, addressed either
/// way, and refuses everything else with the error the model names for it.
///
/// One row per way an address can fail: a triple naming nothing, a triple whose
/// three members do not describe one stored group (wrong name, wrong id, wrong
/// scope), an incomplete triple, an ARN of another resource kind, a well-formed ARN
/// naming nothing, and an ARN that is malformed. A malformed ARN is
/// `WAFInvalidParameterException` ("Your request references an ARN that is
/// malformed", that error's own doc); everything else names no resource, which is
/// `WAFNonexistentItemException`.
#[tokio::test]
async fn get_rule_group_refuses_an_address_that_names_no_rule_group() {
    let waf = Waf::new();
    let (id, arn, _lock) = create_rule_group(&waf, "REGIONAL", "addressed", 10, json!([])).await;
    let (_ip_set_id, ip_set_arn, _ip_lock) =
        create_ip_set(&waf, "REGIONAL", "addressed-ips").await;
    let (_acl_id, acl_arn, _acl_lock) =
        create_web_acl(&waf, "REGIONAL", "addressed-acl", json!([])).await;
    let absent_id = "11111111-2222-4333-8444-555555555555";

    let nonexistent: Vec<(&str, Value)> = vec![
        (
            "a triple naming no rule group",
            json!({ "Name": "absent", "Scope": "REGIONAL", "Id": absent_id }),
        ),
        (
            "the stored id under another name",
            json!({ "Name": "not-addressed", "Scope": "REGIONAL", "Id": &id }),
        ),
        (
            "the stored name with another id",
            json!({ "Name": "addressed", "Scope": "REGIONAL", "Id": absent_id }),
        ),
        (
            "the stored triple in the other scope",
            json!({ "Name": "addressed", "Scope": "CLOUDFRONT", "Id": &id }),
        ),
        ("the ARN of an IP set", json!({ "ARN": &ip_set_arn })),
        ("the ARN of a web ACL", json!({ "ARN": &acl_arn })),
        (
            "a rule group ARN that was never created",
            json!({ "ARN": rule_group_arn("REGIONAL", "absent", absent_id) }),
        ),
    ];
    for (case, request) in nonexistent {
        waf.refused(
            "GetRuleGroup",
            request,
            "WAFNonexistentItemException",
            case,
        )
        .await;
    }

    let invalid: Vec<(&str, Value)> = vec![
        (
            "not an ARN at all",
            json!({ "ARN": "rulegroup/absent/1111111111111111" }),
        ),
        (
            "another service's ARN",
            json!({ "ARN": "arn:aws:s3:::a-bucket-named-like-this" }),
        ),
        (
            "an unknown scope segment",
            json!({ "ARN": "arn:aws:wafv2:us-west-2:000000000000:worldwide/rulegroup/absent/1111" }),
        ),
        (
            "too few resource segments",
            json!({ "ARN": "arn:aws:wafv2:us-west-2:000000000000:regional/rulegroup/absent" }),
        ),
        (
            "neither an ARN nor a complete triple",
            json!({ "Name": "addressed", "Scope": "REGIONAL" }),
        ),
        ("an empty request", json!({})),
    ];
    for (case, request) in invalid {
        waf.refused(
            "GetRuleGroup",
            request,
            "WAFInvalidParameterException",
            case,
        )
        .await;
    }

    // The stored group is still reachable after every refusal.
    let got = waf.ok("GetRuleGroup", json!({ "ARN": &arn })).await;
    assert_eq!(
        got["RuleGroup"]["Id"],
        json!(id),
        "a refused read must leave the stored rule group reachable, got {got}"
    );
}

/// A web ACL rule can RUN a stored rule group, and the web ACL's derived
/// `Capacity` then costs that group's declared `Capacity`.
///
/// `RuleGroupReferenceStatement` is "a rule statement used to run the rules that
/// are defined in a RuleGroup … provide the ARN of the rule group in this
/// statement", and `RuleGroup$Capacity` is "the web ACL capacity units (WCUs)
/// required for this rule group" — so a reference costs exactly what the referenced
/// group declared. One row per number of references and per combination with a
/// statement whose own cost is known, plus an `UpdateWebACL` that adds a reference
/// to a web ACL that had none.
#[tokio::test]
async fn a_web_acl_rule_can_reference_a_stored_rule_group_and_is_charged_its_capacity() {
    let waf = Waf::new();
    // Three groups with different declared capacities, so a row's expected total
    // cannot come out right by accident.
    let (_small_id, small_arn, _small_lock) =
        create_rule_group(&waf, "REGIONAL", "capacity-small", 1, json!([])).await;
    let (_medium_id, medium_arn, _medium_lock) =
        create_rule_group(&waf, "REGIONAL", "capacity-medium", 73, json!([])).await;
    let (_large_id, large_arn, _large_lock) =
        create_rule_group(&waf, "REGIONAL", "capacity-large", 500, json!([])).await;

    // A `CONTAINS` byte match costs 10 (`state::BYTE_MATCH_SCANNING_WCU`) and a SQLi
    // match costs 20, as `web_acl_capacity_is_derived_from_the_stored_rules` pins
    // them.
    let cases: Vec<(&str, Value, i64)> = vec![
        (
            "one reference to the smallest group",
            json!([rule_group_reference_rule("rg", 0, &small_arn)]),
            1,
        ),
        (
            "one reference to a 73 WCU group",
            json!([rule_group_reference_rule("rg", 0, &medium_arn)]),
            73,
        ),
        (
            "three references, one per group",
            json!([
                rule_group_reference_rule("small", 0, &small_arn),
                rule_group_reference_rule("medium", 1, &medium_arn),
                rule_group_reference_rule("large", 2, &large_arn),
            ]),
            574,
        ),
        (
            "the same group referenced twice",
            json!([
                rule_group_reference_rule("first", 0, &medium_arn),
                rule_group_reference_rule("second", 1, &medium_arn),
            ]),
            146,
        ),
        (
            "a reference beside a byte match and a SQLi match",
            ordered(json!([
                rule_group_reference_rule("rg", 0, &small_arn),
                rule("bytes", byte_match_statement()),
                rule("sqli", sqli_statement()),
            ])),
            31,
        ),
    ];

    for (index, (case, rules, expected_capacity)) in cases.into_iter().enumerate() {
        let name = format!("referencing-acl-{index}");
        let (acl_id, _acl_arn, _acl_lock) =
            create_web_acl(&waf, "REGIONAL", &name, rules.clone()).await;
        let got = waf
            .ok(
                "GetWebACL",
                json!({ "Name": &name, "Scope": "REGIONAL", "Id": &acl_id }),
            )
            .await;
        assert_eq!(
            got["WebACL"]["Capacity"],
            json!(expected_capacity),
            "case {case:?}: WebACL.Capacity must cost every referenced rule group's \
             declared capacity, got {got}"
        );
        assert_eq!(
            got["WebACL"]["Rules"], rules,
            "case {case:?}: the referencing rules must read back verbatim, got {got}"
        );
    }

    // An update that ADDS a reference is priced the same way, and one that drops it
    // goes back to the rules' own cost.
    let (acl_id, _arn, lock_token) = create_web_acl(
        &waf,
        "REGIONAL",
        "reprice-acl",
        json!([rule("bytes", byte_match_statement())]),
    )
    .await;
    let updated = waf
        .ok(
            "UpdateWebACL",
            json!({
                "Name": "reprice-acl",
                "Scope": "REGIONAL",
                "Id": &acl_id,
                "DefaultAction": { "Allow": {} },
                "VisibilityConfig": visibility("reprice-acl"),
                "LockToken": &lock_token,
                "Rules": [
                    rule("bytes", byte_match_statement()),
                    rule_group_reference_rule("rg", 1, &large_arn),
                ],
            }),
        )
        .await;
    let next_lock_token = updated["NextLockToken"]
        .as_str()
        .expect("NextLockToken")
        .to_string();
    let got = waf
        .ok(
            "GetWebACL",
            json!({ "Name": "reprice-acl", "Scope": "REGIONAL", "Id": &acl_id }),
        )
        .await;
    assert_eq!(
        got["WebACL"]["Capacity"],
        json!(510),
        "an update that adds a rule group reference must re-price the web ACL, got {got}"
    );
    waf.ok(
        "UpdateWebACL",
        json!({
            "Name": "reprice-acl",
            "Scope": "REGIONAL",
            "Id": &acl_id,
            "DefaultAction": { "Allow": {} },
            "VisibilityConfig": visibility("reprice-acl"),
            "LockToken": &next_lock_token,
            "Rules": [rule("bytes", byte_match_statement())],
        }),
    )
    .await;
    let got = waf
        .ok(
            "GetWebACL",
            json!({ "Name": "reprice-acl", "Scope": "REGIONAL", "Id": &acl_id }),
        )
        .await;
    assert_eq!(
        got["WebACL"]["Capacity"],
        json!(10),
        "an update that drops the reference must re-price the web ACL, got {got}"
    );
}

/// A `RuleGroupReferenceStatement` is well placed ONLY as the top-level statement
/// of a rule in a web ACL, and must name a rule group of the SAME scope.
///
/// The model states the placement rule on the statement itself: "You cannot nest a
/// RuleGroupReferenceStatement, for example for use inside a NotStatement or
/// OrStatement. You cannot use a rule group reference statement inside another rule
/// group. You can only reference a rule group as a top-level statement within a
/// rule that you define in a web ACL."
///
/// One row per way a reference can be misplaced — inside each logical nesting, and
/// in a rule group's own rules, top level or nested — which is
/// `WAFInvalidParameterException`; and one row per way a well-placed reference can
/// fail to resolve — an ARN naming nothing, and a group that exists in the other
/// scope — which is `WAFNonexistentItemException`.
#[tokio::test]
async fn a_rule_group_reference_must_be_a_web_acl_rule_s_own_statement() {
    let waf = Waf::new();
    let (_regional_id, regional_arn, _regional_lock) =
        create_rule_group(&waf, "REGIONAL", "placed-regional", 12, json!([])).await;
    let (_cloudfront_id, cloudfront_arn, _cloudfront_lock) =
        create_rule_group(&waf, "CLOUDFRONT", "placed-cloudfront", 12, json!([])).await;
    let reference = json!({ "RuleGroupReferenceStatement": { "ARN": &regional_arn } });

    // Nested inside each nesting the model has: refused as a parameter WAF does not
    // accept, even though the ARN resolves.
    let nested: Vec<(&str, Value)> = vec![
        (
            "inside an AndStatement",
            json!({ "AndStatement": { "Statements": [byte_match_statement(), reference.clone()] } }),
        ),
        (
            "inside an OrStatement",
            json!({ "OrStatement": { "Statements": [byte_match_statement(), reference.clone()] } }),
        ),
        (
            "inside a NotStatement",
            json!({ "NotStatement": { "Statement": reference.clone() } }),
        ),
        (
            "inside a rate-based scope-down statement",
            json!({
                "RateBasedStatement": {
                    "Limit": 2000,
                    "AggregateKeyType": "IP",
                    "ScopeDownStatement": reference.clone(),
                }
            }),
        ),
        (
            "two levels down, inside an AndStatement's NotStatement",
            json!({
                "AndStatement": {
                    "Statements": [
                        byte_match_statement(),
                        { "NotStatement": { "Statement": reference.clone() } },
                    ]
                }
            }),
        ),
    ];
    for (index, (case, statement)) in nested.into_iter().enumerate() {
        let name = format!("nested-reference-acl-{index}");
        waf.refused(
            "CreateWebACL",
            json!({
                "Name": &name,
                "Scope": "REGIONAL",
                "DefaultAction": { "Allow": {} },
                "VisibilityConfig": visibility(&name),
                "Rules": [rule("nested", statement)],
            }),
            "WAFInvalidParameterException",
            case,
        )
        .await;
        // A refused create stores nothing.
        waf.refused(
            "GetWebACL",
            json!({ "Name": &name, "Scope": "REGIONAL", "Id": "11111111-2222-4333-8444-555555555555" }),
            "WAFNonexistentItemException",
            case,
        )
        .await;
    }

    // A rule group may not reference another rule group at all, top level or nested.
    let inside_a_group: Vec<(&str, Value)> = vec![
        ("a rule's own statement", reference.clone()),
        (
            "nested inside an AndStatement",
            json!({ "AndStatement": { "Statements": [byte_match_statement(), reference.clone()] } }),
        ),
    ];
    for (index, (case, statement)) in inside_a_group.into_iter().enumerate() {
        let name = format!("nesting-group-{index}");
        waf.refused(
            "CreateRuleGroup",
            create_body(
                &name,
                "REGIONAL",
                100,
                None,
                &json!([rule("reference", statement)]),
                None,
            ),
            "WAFInvalidParameterException",
            case,
        )
        .await;
    }

    // Well placed, but naming no rule group of this scope.
    let unresolvable: Vec<(&str, String)> = vec![
        (
            "an ARN naming no rule group",
            rule_group_arn("REGIONAL", "absent", "11111111-2222-4333-8444-555555555555"),
        ),
        ("a rule group of the other scope", cloudfront_arn.clone()),
    ];
    for (index, (case, arn)) in unresolvable.into_iter().enumerate() {
        let name = format!("unresolvable-reference-acl-{index}");
        waf.refused(
            "CreateWebACL",
            json!({
                "Name": &name,
                "Scope": "REGIONAL",
                "DefaultAction": { "Allow": {} },
                "VisibilityConfig": visibility(&name),
                "Rules": [rule_group_reference_rule("rg", 0, &arn)],
            }),
            "WAFNonexistentItemException",
            case,
        )
        .await;
    }

    // And the well-placed, resolvable reference is accepted in each scope.
    for (scope, arn) in [
        ("REGIONAL", regional_arn.as_str()),
        ("CLOUDFRONT", cloudfront_arn.as_str()),
    ] {
        let name = format!("placed-acl-{scope}");
        let (acl_id, _acl_arn, _acl_lock) = create_web_acl(
            &waf,
            scope,
            &name,
            json!([rule_group_reference_rule("rg", 0, arn)]),
        )
        .await;
        let got = waf
            .ok(
                "GetWebACL",
                json!({ "Name": &name, "Scope": scope, "Id": &acl_id }),
            )
            .await;
        assert_eq!(
            got["WebACL"]["Rules"][0]["Statement"]["RuleGroupReferenceStatement"]["ARN"],
            json!(arn),
            "case {scope}: a well-placed reference must be stored verbatim, got {got}"
        );
        assert_eq!(
            got["WebACL"]["Capacity"],
            json!(12),
            "case {scope}: the web ACL must cost the referenced group's capacity, got {got}"
        );
    }
}

/// A rule group's own rules resolve their references the way a web ACL's do: a
/// reference to an entity this service holds in the same scope is stored, and one
/// naming nothing is refused with `WAFNonexistentItemException` and stores nothing.
///
/// One row per outcome, over the reference kind a rule group CAN carry (an IP set):
/// stored in this scope, stored in the other scope, and naming nothing at all.
#[tokio::test]
async fn rule_group_rules_must_reference_an_entity_of_the_same_scope() {
    let waf = Waf::new();
    let (_regional_id, regional_ips, _regional_lock) =
        create_ip_set(&waf, "REGIONAL", "group-ips").await;
    let (_other_id, cloudfront_ips, _other_lock) =
        create_ip_set(&waf, "CLOUDFRONT", "group-cloudfront-ips").await;
    let absent_ips = format!(
        "arn:aws:wafv2:{TEST_REGION}:{}:regional/ipset/absent-ips/\
         11111111-2222-4333-8444-555555555555",
        engine::EMULATOR_ACCOUNT_ID
    );

    let cases: Vec<(&str, String, bool)> = vec![
        ("an IP set stored in this scope", regional_ips.clone(), true),
        (
            "an IP set stored in the other scope",
            cloudfront_ips.clone(),
            false,
        ),
        ("an IP set ARN naming nothing", absent_ips.clone(), false),
    ];

    for (index, (case, arn, accepted)) in cases.into_iter().enumerate() {
        let name = format!("referencing-group-{index}");
        let rules = json!([rule(
            "ips",
            json!({ "IPSetReferenceStatement": { "ARN": &arn } })
        )]);
        let request = create_body(&name, "REGIONAL", 100, None, &rules, None);
        if accepted {
            let created = waf.ok("CreateRuleGroup", request).await;
            let id = created["Summary"]["Id"]
                .as_str()
                .expect("Summary.Id")
                .to_string();
            let got = waf
                .ok(
                    "GetRuleGroup",
                    json!({ "Name": &name, "Scope": "REGIONAL", "Id": &id }),
                )
                .await;
            assert_eq!(
                got["RuleGroup"]["Rules"], rules,
                "case {case:?}: the stored rule must read back verbatim, got {got}"
            );
        } else {
            waf.refused(
                "CreateRuleGroup",
                request,
                "WAFNonexistentItemException",
                case,
            )
            .await;
            // A refused create stores nothing, so the name is free: creating it with
            // no rules succeeds rather than being a duplicate.
            let (_id, _arn, _lock) =
                create_rule_group(&waf, "REGIONAL", &name, 100, json!([])).await;
        }
    }
}

/// `UpdateRuleGroup` replaces a stored rule group with what the request describes,
/// under the optimistic-lock token, and keeps what a rule group cannot change.
///
/// The rows differ in WHICH way a save can be refused — an unknown triple, a stale
/// token, a rule list that does not fit the capacity the group was created with (a
/// rule group's `Capacity` is declared at create and "you cannot change it after
/// creation", and "when you add or modify the rules in a rule group, WAF enforces
/// this limit") — and each refusal is followed by a read proving nothing changed.
#[tokio::test]
async fn update_rule_group_replaces_the_stored_rule_group_under_its_lock_token() {
    let waf = Waf::new();
    // One SQLi match statement costs 20 WCUs, which `CheckCapacity` publishes, so a
    // group declared at 20 fits exactly one of them and no more.
    let declared_capacity = 20;
    let name = "updated-group";
    let (id, arn, create_lock) = create_rule_group(
        &waf,
        "REGIONAL",
        name,
        declared_capacity,
        json!([rule("block-sqli", sqli_statement())]),
    )
    .await;
    let before = waf
        .ok(
            "GetRuleGroup",
            json!({ "Name": name, "Scope": "REGIONAL", "Id": &id }),
        )
        .await;

    let update_body = |lock: &str, rules: Value| {
        json!({
            "Name": name,
            "Scope": "REGIONAL",
            "Id": &id,
            "LockToken": lock,
            "VisibilityConfig": visibility(name),
            "Description": "second generation",
            "Rules": rules,
        })
    };
    let fitting_rules = json!([labelled_rule(
        "block-sqli-again",
        sqli_statement(),
        &["generation:two"]
    )]);

    // An unknown triple names no rule group, whatever the token says.
    waf.refused(
        "UpdateRuleGroup",
        json!({
            "Name": name,
            "Scope": "REGIONAL",
            "Id": "00000000-1111-2222-3333-444444444444",
            "LockToken": &create_lock,
            "VisibilityConfig": visibility(name),
            "Rules": fitting_rules.clone(),
        }),
        "WAFNonexistentItemException",
        "UpdateRuleGroup with an Id no rule group carries",
    )
    .await;
    // A stale token is refused for the lock.
    waf.refused(
        "UpdateRuleGroup",
        update_body("00000000-0000-4000-8000-000000000000", fitting_rules.clone()),
        "WAFOptimisticLockException",
        "UpdateRuleGroup with a stale LockToken",
    )
    .await;
    // Rules that cost more than the declared capacity are refused rather than
    // stored under a capacity they do not fit.
    let mut second_sqli = rule("block-sqli-b", sqli_statement());
    second_sqli["Priority"] = json!(1);
    waf.refused(
        "UpdateRuleGroup",
        update_body(
            &create_lock,
            json!([rule("block-sqli-a", sqli_statement()), second_sqli]),
        ),
        "WAFInvalidParameterException",
        "UpdateRuleGroup with rules above the declared capacity",
    )
    .await;
    let unchanged = waf
        .ok(
            "GetRuleGroup",
            json!({ "Name": name, "Scope": "REGIONAL", "Id": &id }),
        )
        .await;
    assert_eq!(
        unchanged, before,
        "a refused UpdateRuleGroup must leave the stored rule group and its lock \
         token untouched, got {unchanged}"
    );

    // The current token applies the update and rotates to a fresh one.
    let updated = waf
        .ok("UpdateRuleGroup", update_body(&create_lock, fitting_rules.clone()))
        .await;
    let next_lock = updated["NextLockToken"]
        .as_str()
        .expect("NextLockToken")
        .to_string();
    assert!(
        is_model_uuid(&next_lock),
        "UpdateRuleGroup must answer a UUID NextLockToken, got {next_lock:?}"
    );
    assert_ne!(
        next_lock, create_lock,
        "UpdateRuleGroup must rotate the lock token"
    );
    let after = waf
        .ok(
            "GetRuleGroup",
            json!({ "Name": name, "Scope": "REGIONAL", "Id": &id }),
        )
        .await;
    assert_eq!(
        after["RuleGroup"]["Rules"], fitting_rules,
        "the update must replace the stored rules, got {after}"
    );
    assert_eq!(
        after["RuleGroup"]["Description"],
        json!("second generation"),
        "the update must replace the stored description, got {after}"
    );
    assert_eq!(
        after["RuleGroup"]["AvailableLabels"],
        label_summaries(&["generation:two"]),
        "the update must re-derive the labels from the stored rules, got {after}"
    );
    assert_eq!(
        after["LockToken"],
        json!(next_lock),
        "the read must answer the rotated lock token, got {after}"
    );
    // What a rule group cannot change: its name, its id, its ARN and its capacity.
    assert_eq!(
        after["RuleGroup"]["Capacity"],
        json!(declared_capacity),
        "the update must keep the declared capacity, got {after}"
    );
    assert_eq!(
        after["RuleGroup"]["ARN"],
        json!(arn),
        "the update must keep the ARN, got {after}"
    );
    assert_eq!(
        after["RuleGroup"]["Id"],
        json!(id),
        "the update must keep the Id, got {after}"
    );

    // The token the update rotated away stops working, and the fresh one works.
    waf.refused(
        "UpdateRuleGroup",
        update_body(&create_lock, json!([])),
        "WAFOptimisticLockException",
        "UpdateRuleGroup with the token the last update rotated away",
    )
    .await;
    waf.ok("UpdateRuleGroup", update_body(&next_lock, json!([])))
        .await;
    let emptied = waf
        .ok(
            "GetRuleGroup",
            json!({ "Name": name, "Scope": "REGIONAL", "Id": &id }),
        )
        .await;
    assert_eq!(
        emptied["RuleGroup"]["Rules"],
        json!([]),
        "an update with no rules must leave the rule group with none, got {emptied}"
    );
}
