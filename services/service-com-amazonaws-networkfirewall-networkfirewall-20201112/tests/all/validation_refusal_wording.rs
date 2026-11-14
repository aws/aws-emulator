// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the WORDING of this service's boundary refusals — the HTTP 400
//! whose lowercase `message` member reads `<n> validation error(s) detected: …`, answered
//! before any handler runs for a request that breaks a constraint the model declares.
//!
//! Four rules, each a property of the CONSTRAINT rather than of the request, so each test
//! below is a TABLE whose rows span the class its rule covers — the constraint classes
//! (absent member, length, range, pattern, enumerated value set), the member kinds (a
//! scalar, a member nested inside a structure, a collection, a collection's elements) and
//! several operations per class:
//!
//! * a clause names the offending member path and the constraint it broke, and NOTHING of
//!   the value the caller sent — not the bytes submitted, and not a `null` for a member that
//!   was absent;
//! * a constraint the model declares on the ELEMENTS of a collection member is stated at
//!   that member, with no element position and no `member` step, carrying the element
//!   shape's WHOLE constraint set bracketed after `Member must satisfy constraint:` — while
//!   a constraint the collection ITSELF broke keeps its own requirement;
//! * a requirement quoting a `@pattern` quotes the expression AWS PUBLISHES for that
//!   member's shape — the model's expression, except for the pagination token, published
//!   without the anchor that opens it;
//! * a requirement enumerating an enumerated type's value set names every value the service
//!   accepts, which for the rule-group type is wider than the set the public model declares.
//!
//! The constraint sets and bounds the rows assert are the model's, read with
//! `smithy_query(query="shape",shape="VpcIds,AzSubnets,EnabledAnalysisType,\
//! PaginationMaxResults,RuleOrder,ResourceName,Description,TagList")`, and are written out
//! as literals here so these tests pin the WIRE text instead of restating the code.
//!
//! Every test drives the service the way a client does, through the shared [`Harness`] in
//! `crate::harness`: an awsJson1_0 `POST /` with
//! `X-Amz-Target: NetworkFirewall_20201112.<OperationName>`, so what is asserted is the wire
//! behaviour and not a handler's return value.

use serde_json::{json, Value};

use crate::harness::{error_code, Harness, ACCOUNT, REGION};

/// The error code every boundary refusal carries, with the 400 it is answered as.
const VALIDATION_EXCEPTION: &str = "ValidationException";

/// The requirement an absent required member states.
const MUST_NOT_BE_NULL: &str = "Member must not be null";

/// The constraint set of a `VpcIds` element (`VpcId`: `@length` 1..128, `@pattern`
/// `^vpc-[0-9a-f]+$`), as a clause states it at the collection member.
const VPC_ID_ELEMENT_SET: &str = "Member must satisfy constraint: [Member must have length less \
                                  than or equal to 128, Member must have length greater than or \
                                  equal to 1, Member must satisfy regular expression pattern: \
                                  ^vpc-[0-9a-f]+$]";

/// The same, for an `AzSubnets` element (`@length` 1..128, `@pattern` `^subnet-[0-9a-f]+$`).
const SUBNET_ID_ELEMENT_SET: &str = "Member must satisfy constraint: [Member must have length \
                                     less than or equal to 128, Member must have length greater \
                                     than or equal to 1, Member must satisfy regular expression \
                                     pattern: ^subnet-[0-9a-f]+$]";

/// The same, for an `EnabledAnalysisTypes` element — an enumerated element shape, so its set
/// is one value-set requirement.
const ANALYSIS_TYPE_ELEMENT_SET: &str =
    "Member must satisfy constraint: [Member must satisfy enum value set: [TLS_SNI, HTTP_HOST]]";

/// The value set a requirement naming the rule-group type states: every value the service's
/// own enumeration carries, of which the public model declares three.
const RULE_GROUP_TYPE_SET: &str =
    "Member must satisfy enum value set: [FIREWALL, STATELESS, IPS, STATEFUL, IPS_DOMAIN, \
     STATEFUL_DOMAIN]";

/// The value set the stateful rule order states — an enumerated type the service does not
/// widen, so the model's own two values.
const RULE_ORDER_SET: &str =
    "Member must satisfy enum value set: [DEFAULT_ACTION_ORDER, STRICT_ORDER]";

/// The lowercase `message` member of a refusal body, which is where the boundary states its
/// text (a modelled exception carries `Message`).
fn validation_message(body: &Value) -> String {
    body.get("message")
        .and_then(Value::as_str)
        .unwrap_or("<no message>")
        .to_string()
}

/// A firewall-policy ARN in this emulator's form, so a request under test is refused for the
/// member it is about rather than for this one.
fn policy_arn(name: &str) -> String {
    format!("arn:aws:network-firewall:{REGION}:{ACCOUNT}:firewall-policy/{name}")
}

/// The envelope over one clause.
fn one_error(clause: &str) -> String {
    format!("1 validation error detected: {clause}")
}

/// The clause stating `requirement` at `member`, with no value.
fn clause(member: &str, requirement: &str) -> String {
    format!("Value at '{member}' failed to satisfy constraint: {requirement}")
}

/// Send one request that must be refused by the boundary, and return its `message`.
async fn refusal(harness: &Harness, case: &str, target: &str, body: &Value) -> String {
    let (status, parsed) = harness.call(target, body).await;
    assert_eq!(
        status, 400,
        "case {case}: expected HTTP 400, got {status} with body {parsed:?}"
    );
    let code = error_code(&parsed);
    assert_eq!(
        code, VALIDATION_EXCEPTION,
        "case {case}: expected {VALIDATION_EXCEPTION}, got {code:?} with body {parsed:?}"
    );
    validation_message(&parsed)
}

/// RULE: a member the model requires and the request omits is stated at its lowerCamel
/// member path with the requirement that it must not be null, and with NO value — not even
/// the `null` the member's absence consists of.
///
/// The rows differ along what the rule generalises over: the operation, the member, and the
/// kind of shape the absent member targets (a string, an enumerated type, an integer, a
/// structure, a list).
#[tokio::test]
async fn an_absent_required_member_is_stated_without_a_value() {
    let harness = Harness::new();
    let rule_group_document = json!({ "RulesSource": { "RulesString": "pass tcp any any -> any any (sid:1;)" } });
    // (case, operation, request body, the member path the clause names)
    let rows: Vec<(&str, &str, Value, &str)> = vec![
        (
            "a create whose policy ARN is absent",
            "CreateFirewall",
            json!({ "FirewallName": "fw-one", "SubnetMappings": [] }),
            "firewallPolicyArn",
        ),
        (
            "a create whose name is absent",
            "CreateFirewall",
            json!({ "FirewallPolicyArn": policy_arn("p-one") }),
            "firewallName",
        ),
        (
            "a rule group whose capacity is absent",
            "CreateRuleGroup",
            json!({
                "RuleGroupName": "rg-one",
                "Type": "STATEFUL",
                "RuleGroup": rule_group_document.clone(),
            }),
            "capacity",
        ),
        (
            "a rule group whose type is absent",
            "CreateRuleGroup",
            json!({
                "RuleGroupName": "rg-two",
                "Capacity": 10,
                "RuleGroup": rule_group_document.clone(),
            }),
            "type",
        ),
        (
            "a policy whose document is absent",
            "CreateFirewallPolicy",
            json!({ "FirewallPolicyName": "p-two" }),
            "firewallPolicy",
        ),
        (
            "an untag whose key list is absent",
            "UntagResource",
            json!({ "ResourceArn": policy_arn("p-three") }),
            "tagKeys",
        ),
        (
            "a disassociate whose subnet list is absent",
            "DisassociateSubnets",
            json!({ "FirewallName": "fw-two" }),
            "subnetIds",
        ),
    ];

    for (case, target, body, member) in rows {
        let expected = one_error(&clause(member, MUST_NOT_BE_NULL));
        let message = refusal(&harness, case, target, &body).await;
        assert_eq!(
            message, expected,
            "case {case}: expected {expected:?}, got {message:?}"
        );
    }
}

/// RULE: a constraint the model declares on the ELEMENTS of a collection member is stated at
/// that member — no element position, no `member` step and no value — with the element
/// shape's whole constraint set bracketed; a constraint the COLLECTION itself broke keeps its
/// own requirement and is not bracketed.
///
/// The rows differ along what the rule generalises over: which of the element shape's
/// constraints was broken, how long the offending value is, how many elements offended and at
/// which positions, the element shape's own kind (a constrained string, an enumerated type),
/// the operation, and whether the violation is about the elements or about the collection.
#[tokio::test]
async fn an_element_constraint_is_stated_at_the_collection_member() {
    let harness = Harness::new();
    let over_the_maximum = format!("vpc-{}", "a".repeat(125));
    // (case, operation, request body, the clause expected)
    let rows: Vec<(&str, &str, Value, String)> = vec![
        (
            "one VPC id outside the element pattern",
            "ListFirewalls",
            json!({ "VpcIds": ["DMZVPC"] }),
            clause("vpcIds", VPC_ID_ELEMENT_SET),
        ),
        (
            "an empty VPC id, under the element minimum",
            "ListFirewalls",
            json!({ "VpcIds": [""] }),
            clause("vpcIds", VPC_ID_ELEMENT_SET),
        ),
        (
            "a VPC id one character over the element maximum",
            "ListFirewalls",
            json!({ "VpcIds": [over_the_maximum] }),
            clause("vpcIds", VPC_ID_ELEMENT_SET),
        ),
        (
            "two offending VPC ids after a valid one",
            "ListFirewalls",
            json!({ "VpcIds": ["vpc-0abc", "NOPE", "also bad"] }),
            clause("vpcIds", VPC_ID_ELEMENT_SET),
        ),
        (
            "a subnet id outside its element pattern, on another operation",
            "DisassociateSubnets",
            json!({ "FirewallName": "fw-three", "SubnetIds": ["subnet-NOPE"] }),
            clause("subnetIds", SUBNET_ID_ELEMENT_SET),
        ),
        (
            "an analysis type outside its element value set",
            "UpdateFirewallAnalysisSettings",
            json!({ "FirewallName": "fw-four", "EnabledAnalysisTypes": ["tls_sni"] }),
            clause("enabledAnalysisTypes", ANALYSIS_TYPE_ELEMENT_SET),
        ),
        (
            // The collection's OWN `@length` is not an element constraint: it keeps its own
            // requirement, and still states no value, because a list has no scalar
            // rendering to quote.
            "an empty tag list, under the collection's own minimum",
            "CreateFirewall",
            json!({
                "FirewallName": "fw-five",
                "FirewallPolicyArn": policy_arn("p-four"),
                "Tags": [],
            }),
            clause(
                "tags",
                "Member must have length greater than or equal to 1",
            ),
        ),
    ];

    for (case, target, body, expected_clause) in rows {
        let expected = one_error(&expected_clause);
        let message = refusal(&harness, case, target, &body).await;
        assert_eq!(
            message, expected,
            "case {case}: expected {expected:?}, got {message:?}"
        );
    }
}

/// RULE: a constraint a scalar member broke is stated at that member's path — the dotted path
/// of a member nested inside a structure included — with the requirement it broke and no
/// value; and a requirement naming the rule-group type's value set names every value the
/// service accepts.
///
/// The rows differ along what the rule generalises over: the constraint class (both `@range`
/// bounds, both `@length` bounds, a `@pattern`, an enumerated value set), the depth of the
/// member path, the operation, and which enumerated type the member targets.
#[tokio::test]
async fn a_scalar_constraint_is_stated_without_the_submitted_value() {
    let harness = Harness::new();
    let over_the_maximum = "d".repeat(513);
    let long_name = "a".repeat(129);
    // (case, operation, request body, member path, the requirement expected)
    let rows: Vec<(&str, &str, Value, &str, String)> = vec![
        (
            "a page size below the published minimum",
            "ListFirewallPolicies",
            json!({ "MaxResults": 0 }),
            "maxResults",
            "Member must have value greater than or equal to 1".to_string(),
        ),
        (
            "a page size above the published maximum",
            "ListFirewallPolicies",
            json!({ "MaxResults": 101 }),
            "maxResults",
            "Member must have value less than or equal to 100".to_string(),
        ),
        (
            "a page size far above it, on another operation",
            "ListRuleGroups",
            json!({ "MaxResults": 100_000 }),
            "maxResults",
            "Member must have value less than or equal to 100".to_string(),
        ),
        (
            "a lower-case rule group type",
            "ListRuleGroups",
            json!({ "Type": "stateful" }),
            "type",
            RULE_GROUP_TYPE_SET.to_string(),
        ),
        (
            "a misspelled rule group type on the create path",
            "CreateRuleGroup",
            json!({
                "RuleGroupName": "rg-three",
                "Type": "Stateful",
                "Capacity": 10,
                "RuleGroup": { "RulesSource": { "RulesString": "pass tcp any any -> any any (sid:1;)" } },
            }),
            "type",
            RULE_GROUP_TYPE_SET.to_string(),
        ),
        (
            "a lower-case rule order, nested two members deep",
            "CreateFirewallPolicy",
            json!({
                "FirewallPolicyName": "p-five",
                "FirewallPolicy": {
                    "StatelessDefaultActions": ["aws:forward_to_sfe"],
                    "StatelessFragmentDefaultActions": ["aws:forward_to_sfe"],
                    "StatefulEngineOptions": { "RuleOrder": "strict_order" },
                },
            }),
            "firewallPolicy.statefulEngineOptions.ruleOrder",
            RULE_ORDER_SET.to_string(),
        ),
        (
            "a firewall name outside its pattern",
            "CreateFirewall",
            json!({ "FirewallName": "bad name!", "FirewallPolicyArn": policy_arn("p-six") }),
            "firewallName",
            "Member must satisfy regular expression pattern: ^[a-zA-Z0-9-]+$".to_string(),
        ),
        (
            "a firewall name one character over its maximum",
            "CreateFirewall",
            json!({ "FirewallName": long_name, "FirewallPolicyArn": policy_arn("p-seven") }),
            "firewallName",
            "Member must have length less than or equal to 128".to_string(),
        ),
        (
            "a description one character over its maximum",
            "CreateFirewall",
            json!({
                "FirewallName": "fw-six",
                "FirewallPolicyArn": policy_arn("p-eight"),
                "Description": over_the_maximum,
            }),
            "description",
            "Member must have length less than or equal to 512".to_string(),
        ),
        (
            "a policy ARN of no partition at all",
            "CreateFirewall",
            json!({ "FirewallName": "fw-seven", "FirewallPolicyArn": "not-an-arn" }),
            "firewallPolicyArn",
            "Member must satisfy regular expression pattern: ^arn:aws".to_string(),
        ),
    ];

    for (case, target, body, member, requirement) in rows {
        let expected = one_error(&clause(member, &requirement));
        let message = refusal(&harness, case, target, &body).await;
        assert_eq!(
            message, expected,
            "case {case}: expected {expected:?}, got {message:?}"
        );
    }
}

/// RULE: the envelope counts the CLAUSES — one per refused member, however many of a
/// collection's elements were reported — and a request inside every declared bound is not
/// refused at all, including at each published bound of a range.
#[tokio::test]
async fn the_envelope_counts_clauses_and_valid_input_is_served() {
    let harness = Harness::new();

    let two = refusal(
        &harness,
        "a collection's elements and another member, both refused",
        "ListFirewalls",
        &json!({ "VpcIds": ["NOPE", "ALSO-NOPE"], "MaxResults": 0 }),
    )
    .await;
    let expected = format!(
        "2 validation errors detected: {}; {}",
        clause("vpcIds", VPC_ID_ELEMENT_SET),
        clause("maxResults", "Member must have value greater than or equal to 1")
    );
    assert_eq!(
        two, expected,
        "case two refused members: expected {expected:?}, got {two:?}"
    );

    // One member breaking two of its own constraints is two clauses, each naming it: the
    // count is of requirements broken, not of members.
    let same_member = refusal(
        &harness,
        "one member breaking two of its constraints",
        "CreateFirewall",
        &json!({ "FirewallName": "", "FirewallPolicyArn": policy_arn("p-ten") }),
    )
    .await;
    let expected_same_member = format!(
        "2 validation errors detected: {}; {}",
        clause(
            "firewallName",
            "Member must have length greater than or equal to 1"
        ),
        clause(
            "firewallName",
            "Member must satisfy regular expression pattern: ^[a-zA-Z0-9-]+$"
        )
    );
    assert_eq!(
        same_member, expected_same_member,
        "case one member, two constraints: expected {expected_same_member:?}, got {same_member:?}"
    );

    // Each published bound of the page-size range, and a well-formed VPC id, are inside the
    // constraints and so are served rather than refused.
    harness.create_policy("p-nine").await;
    for (case, body) in [
        ("the range minimum", json!({ "MaxResults": 1 })),
        ("the range maximum", json!({ "MaxResults": 100 })),
        (
            "a well-formed VPC id",
            json!({ "VpcIds": ["vpc-0123456789abcdef"] }),
        ),
    ] {
        let (status, parsed) = harness.call("ListFirewalls", &body).await;
        assert_eq!(
            status, 200,
            "case {case}: expected HTTP 200, got {status} with body {parsed:?}"
        );
    }

    // The policy created above is still listable, which a boundary refusal never reaches.
    let listed = harness
        .ok("ListFirewallPolicies", &json!({ "MaxResults": 100 }))
        .await;
    let policies = listed
        .get("FirewallPolicies")
        .and_then(Value::as_array)
        .map_or(0, Vec::len);
    assert_eq!(
        policies, 1,
        "case listing after the refusals: expected 1 policy, got {policies} in {listed:?}"
    );
}
