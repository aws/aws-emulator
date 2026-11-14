// SPDX-License-Identifier: CC0-1.0

//! WHICH of several applicable refusals a WAFv2 request gets — the ORDER WAF takes its
//! judgements in — end to end over the wire.
//!
//! Three rules, each asserted over a table that spans its class rather than over the
//! one request that exposed it:
//!
//! 1. WAF judges whether the endpoint a request reached serves the namespace its
//!    `Scope` names BEFORE it reports any other member's modelled constraint, so a
//!    request naming the global namespace anywhere but its own endpoint is the
//!    modelled `WAFInvalidParameterException` with `Field` SCOPE_VALUE whatever else
//!    the request carries — and the same request at the endpoint that serves that
//!    namespace still has its members judged.
//! 2. Where a request names its target by an ARN, the ARN is judged against the
//!    `Scope` it was sent with before that precondition: an ARN that does not belong
//!    to the namespace asked about is the invalid-ARN refusal naming the ARN, because
//!    the pairing is what the request got wrong.
//! 3. A judgement that needs the STORE waits for the target the request acts on: a
//!    request against a resource that does not exist is the nonexistent-item refusal
//!    whatever its own references resolve to, while a fault in the REQUEST ALONE — an
//!    ARN that is not one, a member outside its modelled form — is still reported
//!    first.
//!
//! Every call goes through the same front door a real client uses (the `Waf` harness
//! in [`crate::web_acl_lifecycle`]), so what these tests observe is what an SDK
//! observes.

use serde_json::{json, Value};
use service_com_amazonaws_wafv2_awswaf_20190729::state::{
    NONEXISTENT_ITEM_MESSAGE, SCOPE_CLOUDFRONT, SCOPE_REGIONAL,
};

use crate::managed_rule_sets::provision_managed_rule_set;
use crate::rule_group_lifecycle::create_rule_group;
use crate::web_acl_lifecycle::{
    create_ip_set, create_web_acl, visibility, Waf, CLOUDFRONT_ENDPOINT_REGION, TEST_REGION,
};

/// The `ParameterExceptionField` value naming a `Scope`
/// (`smithy_query(query="shape",shape="ParameterExceptionField")` lists SCOPE_VALUE
/// and RESOURCE_ARN among its values).
const FIELD_SCOPE_VALUE: &str = "SCOPE_VALUE";

/// The `ParameterExceptionField` value naming a resource ARN. See
/// [`FIELD_SCOPE_VALUE`].
const FIELD_RESOURCE_ARN: &str = "RESOURCE_ARN";

/// The `Reason` WAF carries for a scope it will not accept.
const SCOPE_NOT_VALID: &str = "The scope is not valid.";

/// The `Reason` WAF carries for an ARN it will not accept.
const ARN_NOT_VALID: &str = "The ARN isn't valid. A valid ARN begins with arn: and \
     includes other information separated by colons or slashes.";

/// Endpoints other than the one the global namespace is served at.
const OTHER_ENDPOINTS: [&str; 3] = [TEST_REGION, "eu-central-1", "ap-southeast-2"];

/// A `LockToken`/`EntityId`-shaped value, which no resource below carries.
const WELL_FORMED_UUID: &str = "11111111-2222-3333-4444-555555555555";

/// The awsJson error code of a refusal, as its body carries it.
fn error_code(body: &Value) -> &str {
    body["__type"].as_str().unwrap_or_default()
}

/// The error body's message member, whichever casing the shape models.
fn error_message(body: &Value) -> &str {
    body.get("Message")
        .or_else(|| body.get("message"))
        .and_then(Value::as_str)
        .unwrap_or_default()
}

/// The three members the model gives `WAFInvalidParameterException`, and the message
/// WAF composes from exactly those three.
fn assert_invalid_parameter(body: &Value, field: &str, parameter: &str, reason: &str, case: &str) {
    assert!(
        error_code(body).ends_with("WAFInvalidParameterException"),
        "case {case:?}: expected WAFInvalidParameterException, got {body}"
    );
    assert_eq!(
        (
            body["Field"].as_str(),
            body["Parameter"].as_str(),
            body["Reason"].as_str()
        ),
        (Some(field), Some(parameter), Some(reason)),
        "case {case:?}: expected Field {field:?}, Parameter {parameter:?} and Reason \
         {reason:?}, got {body}"
    );
    assert_eq!(
        error_message(body),
        format!("Error reason: {reason}, field: {field}, parameter: {parameter}"),
        "case {case:?}: the message must read the way WAF composes it, got {body}"
    );
}

/// One request that breaks the modelled constraints of exactly ONE member besides its
/// `Scope`: the operation, the member the request validator must name when the scope
/// IS served, and the members to send.
struct BadMember {
    case: &'static str,
    operation: &'static str,
    /// The member path the validator's clause names, as it renders it.
    member: &'static str,
    members: Value,
}

/// Every operation that resolves something in a scope, with one member outside its
/// modelled form.
///
/// The rows differ along what the rule generalises over: which operation is called
/// (a read, a list, two updates, two deletes, a publish), which member is at fault
/// (`LockToken`, `Id`, `Name`, `NextMarker`, `Limit`), and which KIND of constraint
/// it breaks (a `@pattern`, the floor and the ceiling of a `@length`, a `@range`) —
/// so a scope precondition keyed to one operation or one member would show up as a
/// difference from the others. Every member and constraint named here is the model's
/// (`smithy_query(query="operation",operation="UpdateWebACL,DeleteWebACL,UpdateIPSet,DeleteRuleGroup,GetIPSet,ListWebACLs,PutManagedRuleSetVersions")`).
fn requests_with_one_bad_member() -> Vec<BadMember> {
    vec![
        BadMember {
            case: "UpdateWebACL with a LockToken outside its pattern",
            operation: "UpdateWebACL",
            member: "lockToken",
            members: json!({
                "Name": "precedence-acl",
                "Id": WELL_FORMED_UUID,
                "LockToken": "not-a-real-lock-token",
                "DefaultAction": { "Allow": {} },
                "VisibilityConfig": visibility("precedence-acl"),
            }),
        },
        BadMember {
            case: "DeleteWebACL with a LockToken outside its pattern",
            operation: "DeleteWebACL",
            member: "lockToken",
            members: json!({
                "Name": "precedence-acl",
                "Id": WELL_FORMED_UUID,
                "LockToken": "not-a-real-lock-token",
            }),
        },
        BadMember {
            case: "DeleteRuleGroup with an empty LockToken, at the length floor",
            operation: "DeleteRuleGroup",
            member: "lockToken",
            members: json!({
                "Name": "precedence-group",
                "Id": WELL_FORMED_UUID,
                "LockToken": "",
            }),
        },
        BadMember {
            case: "UpdateIPSet with an Id outside its pattern",
            operation: "UpdateIPSet",
            member: "id",
            members: json!({
                "Name": "precedence-ips",
                "Id": "not-a-uuid",
                "LockToken": WELL_FORMED_UUID,
                "Addresses": ["10.0.0.0/8"],
            }),
        },
        BadMember {
            case: "GetIPSet with a Name outside its pattern",
            operation: "GetIPSet",
            member: "name",
            members: json!({ "Name": "not a valid name!", "Id": WELL_FORMED_UUID }),
        },
        BadMember {
            case: "PutManagedRuleSetVersions with an Id past the length ceiling",
            operation: "PutManagedRuleSetVersions",
            member: "id",
            members: json!({
                "Name": "precedence-set",
                "Id": "0".repeat(37),
                "LockToken": WELL_FORMED_UUID,
            }),
        },
        BadMember {
            case: "ListWebACLs with a Limit below its range",
            operation: "ListWebACLs",
            member: "limit",
            members: json!({ "Limit": 0 }),
        },
        BadMember {
            case: "ListWebACLs with a NextMarker that is only whitespace",
            operation: "ListWebACLs",
            member: "nextMarker",
            members: json!({ "NextMarker": " " }),
        },
    ]
}

/// `members` with `Scope` set, the way every scoped request carries it.
fn scoped(scope: &str, members: &Value) -> Value {
    let mut request = members.clone();
    if let Some(fields) = request.as_object_mut() {
        fields.insert("Scope".to_string(), json!(scope));
    }
    request
}

/// Rule 1: the scope precondition is answered BEFORE the request validator reports a
/// member outside its modelled form.
///
/// Each row is sent three ways: naming the global namespace at an endpoint that does
/// not serve it (the scope refusal, whatever the bad member), naming it at the
/// endpoint that does (the member, because the precondition holds), and naming the
/// regional namespace anywhere (the member again). So the only thing that moves the
/// answer from one class to the other is the (scope, endpoint) pair, and the gate is
/// shown to PRECEDE the validator rather than to replace it.
#[tokio::test]
async fn the_scope_precondition_precedes_a_member_constraint_refusal() {
    let waf = Waf::new();
    for row in requests_with_one_bad_member() {
        for region in OTHER_ENDPOINTS {
            let case = format!("{} from {region}", row.case);
            let (status, body) = waf
                .call_from(region, row.operation, scoped(SCOPE_CLOUDFRONT, &row.members))
                .await;
            assert_eq!(
                status, 400,
                "case {case:?}: expected the scope refusal, got status {status} and \
                 body {body}"
            );
            assert_invalid_parameter(
                &body,
                FIELD_SCOPE_VALUE,
                SCOPE_CLOUDFRONT,
                SCOPE_NOT_VALID,
                &case,
            );
        }

        // At the endpoint that serves the global namespace, and under the regional
        // namespace at any endpoint, the bad member is what the request validator
        // reports — in its own envelope, naming the member.
        for (scope, region) in [
            (SCOPE_CLOUDFRONT, CLOUDFRONT_ENDPOINT_REGION),
            (SCOPE_REGIONAL, TEST_REGION),
            (SCOPE_REGIONAL, CLOUDFRONT_ENDPOINT_REGION),
        ] {
            let case = format!("{} as {scope} from {region}", row.case);
            let (status, body) = waf
                .call_from(region, row.operation, scoped(scope, &row.members))
                .await;
            assert_eq!(
                status, 400,
                "case {case:?}: expected the member-constraint refusal, got status \
                 {status} and body {body}"
            );
            assert_eq!(
                error_code(&body),
                "ValidationException",
                "case {case:?}: a member-constraint failure is the request \
                 validator's own refusal, got {body}"
            );
            let message = error_message(&body);
            assert!(
                message.contains(&format!("at '{}' failed to satisfy constraint:", row.member)),
                "case {case:?}: the clause must name {:?}, got {message:?}",
                row.member
            );
        }
    }
}

/// Rule 2: a traffic query whose `WebAclArn` does not belong to the namespace its
/// `Scope` names is refused on the ARN, and that judgement precedes the scope
/// precondition.
///
/// The rows differ along which side of the pairing is wrong and where the call was
/// made: a regional ARN asked about globally (at the global endpoint and at a
/// regional one, which is the row that pins the ARN ahead of the precondition), a
/// global ARN asked about regionally, and an ARN of another resource kind. The
/// matching pairs are served from the same table, so the pairing is the only
/// difference between an answer and a refusal.
#[tokio::test]
async fn an_arn_outside_the_scope_it_was_sent_with_is_refused_on_the_arn() {
    let waf = Waf::new();
    let (_regional_id, regional_arn, _regional_lock) =
        create_web_acl(&waf, SCOPE_REGIONAL, "paired-acl", json!([])).await;
    let (_global_id, global_arn, _global_lock) =
        create_web_acl(&waf, SCOPE_CLOUDFRONT, "paired-acl", json!([])).await;
    let (_ip_set_id, ip_set_arn, _ip_set_lock) =
        create_ip_set(&waf, SCOPE_REGIONAL, "paired-ips").await;

    /// The query the rows send, with the four members beside `WebAclArn` the model
    /// marks `@required`.
    fn query(arn: &str, scope: &str) -> Value {
        json!({
            "WebAclArn": arn,
            "Scope": scope,
            "TimeWindow": { "StartTime": 1_767_225_600_i64, "EndTime": 1_767_229_200_i64 },
            "Limit": 10,
            "NumberOfTopTrafficBotsPerPath": 5,
        })
    }

    for (case, arn, scope, region) in [
        (
            "a regional web ACL asked about globally, at the global endpoint",
            regional_arn.clone(),
            SCOPE_CLOUDFRONT,
            CLOUDFRONT_ENDPOINT_REGION,
        ),
        (
            "a regional web ACL asked about globally, from a regional endpoint",
            regional_arn.clone(),
            SCOPE_CLOUDFRONT,
            TEST_REGION,
        ),
        (
            "a global web ACL asked about regionally",
            global_arn.clone(),
            SCOPE_REGIONAL,
            TEST_REGION,
        ),
        (
            "an ARN of another resource kind",
            ip_set_arn,
            SCOPE_REGIONAL,
            TEST_REGION,
        ),
    ] {
        let (status, body) = waf
            .call_from(region, "GetTopPathStatisticsByTraffic", query(&arn, scope))
            .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected the ARN refusal, got status {status} and body {body}"
        );
        assert_invalid_parameter(&body, FIELD_RESOURCE_ARN, &arn, ARN_NOT_VALID, case);
    }

    // The matching pairs are served, each from the endpoint that serves its
    // namespace, and each reports the empty aggregation of a web ACL with no
    // recorded traffic.
    for (case, arn, scope, region) in [
        (
            "the regional web ACL asked about regionally",
            regional_arn,
            SCOPE_REGIONAL,
            TEST_REGION,
        ),
        (
            "the global web ACL asked about globally",
            global_arn,
            SCOPE_CLOUDFRONT,
            CLOUDFRONT_ENDPOINT_REGION,
        ),
    ] {
        let (status, body) = waf
            .call_from(region, "GetTopPathStatisticsByTraffic", query(&arn, scope))
            .await;
        assert_eq!(
            (status, body["TotalRequestCount"].as_i64()),
            (200, Some(0)),
            "case {case:?}: an ARN of the namespace asked about is served, got status \
             {status} and body {body}"
        );
    }
}

/// Rule 3: `PutManagedRuleSetVersions` against a managed rule set that does not exist
/// is the nonexistent-item refusal, whatever its `VersionsToPublish` entries
/// reference — and nothing is published.
///
/// The rows cross each way the target identity can fail to resolve (an unknown name,
/// an unknown id, both) with what the request asks to publish (nothing, a version
/// defined by a rule group that exists, one defined by a well-formed rule-group ARN
/// that resolves to nothing, two versions at once), so an answer keyed to the
/// published entries rather than to the target would show up as a difference between
/// the rows. The same unresolvable reference against the EXISTING set is refused on
/// the ARN, which is how the rows above are shown to report the target rather than to
/// answer the nonexistent-item class for everything.
#[tokio::test]
async fn a_put_against_a_set_that_does_not_exist_names_the_missing_resource() {
    for scope in [SCOPE_REGIONAL, SCOPE_CLOUDFRONT] {
        let (waf, engine) = Waf::sharing_engine();
        let endpoint = if scope == SCOPE_CLOUDFRONT {
            CLOUDFRONT_ENDPOINT_REGION
        } else {
            TEST_REGION
        };
        let (_group_id, group_arn, _group_lock) =
            create_rule_group(&waf, scope, "vendor-group", 100, json!([])).await;
        let set_name = "vendor-set";
        let (set_id, _set_arn, set_lock) =
            provision_managed_rule_set(&engine, scope, set_name, Some("the vendor's set"));
        // A well-formed rule-group ARN of this namespace that no rule group carries:
        // the reference can only be told apart from a usable one by reading the store.
        let absent_group = format!("{group_arn}-never-minted");

        let identities = [
            ("a name no set carries", format!("{set_name}-absent"), set_id.clone()),
            ("an id no set carries", set_name.to_string(), WELL_FORMED_UUID.to_string()),
            (
                "neither the name nor the id",
                format!("{set_name}-absent"),
                WELL_FORMED_UUID.to_string(),
            ),
        ];
        let publications = [
            ("nothing to publish", Value::Null),
            (
                "a version defined by a rule group that exists",
                json!({ "Version_1.0": { "AssociatedRuleGroupArn": &group_arn } }),
            ),
            (
                "a version defined by a rule group that does not",
                json!({ "Version_1.0": { "AssociatedRuleGroupArn": &absent_group } }),
            ),
            (
                "two versions, one of each",
                json!({
                    "Version_1.0": { "AssociatedRuleGroupArn": &group_arn },
                    "Version_2.0": { "AssociatedRuleGroupArn": &absent_group },
                }),
            ),
        ];

        for (identity, name, id) in &identities {
            for (publication, versions) in &publications {
                let case = format!("{scope}, {identity}, {publication}");
                let mut request = json!({
                    "Name": name,
                    "Scope": scope,
                    "Id": id,
                    "LockToken": WELL_FORMED_UUID,
                });
                if !versions.is_null() {
                    request["VersionsToPublish"] = versions.clone();
                }
                let (status, body) = waf
                    .call_from(endpoint, "PutManagedRuleSetVersions", request)
                    .await;
                assert_eq!(
                    status, 400,
                    "case {case:?}: expected the nonexistent-item refusal, got status \
                     {status} and body {body}"
                );
                assert!(
                    error_code(&body).ends_with("WAFNonexistentItemException"),
                    "case {case:?}: expected WAFNonexistentItemException, got {body}"
                );
                assert_eq!(
                    error_message(&body),
                    NONEXISTENT_ITEM_MESSAGE,
                    "case {case:?}: expected WAF's own sentence, got {body}"
                );
            }
        }

        // Against the set that DOES exist, the reference that resolves to nothing is
        // refused on the ARN: resolving it reads the same store the target lives in,
        // so it is judged once the target has been.
        let case = format!("{scope}, the existing set with a reference that resolves to nothing");
        let (status, body) = waf
            .call_from(
                endpoint,
                "PutManagedRuleSetVersions",
                json!({
                    "Name": set_name,
                    "Scope": scope,
                    "Id": &set_id,
                    "LockToken": &set_lock,
                    "VersionsToPublish": {
                        "Version_1.0": { "AssociatedRuleGroupArn": &absent_group },
                    },
                }),
            )
            .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected the ARN refusal, got status {status} and body {body}"
        );
        assert_invalid_parameter(&body, FIELD_RESOURCE_ARN, &absent_group, ARN_NOT_VALID, &case);

        // A fault in the REQUEST ALONE is still reported first, against a target that
        // does not exist: the value is no ARN at all, which needs no store to judge.
        let not_an_arn = "not-an-arn-at-all-but-long-enough";
        let case = format!("{scope}, a set that does not exist and a value that is no ARN");
        let (status, body) = waf
            .call_from(
                endpoint,
                "PutManagedRuleSetVersions",
                json!({
                    "Name": format!("{set_name}-absent"),
                    "Scope": scope,
                    "Id": WELL_FORMED_UUID,
                    "LockToken": WELL_FORMED_UUID,
                    "VersionsToPublish": {
                        "Version_1.0": { "AssociatedRuleGroupArn": not_an_arn },
                    },
                }),
            )
            .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected the ARN refusal, got status {status} and body {body}"
        );
        assert_invalid_parameter(&body, FIELD_RESOURCE_ARN, not_an_arn, ARN_NOT_VALID, &case);

        // Nothing above published anything: the account still holds the one
        // provisioned set, and it still offers no version at all.
        let listed = waf
            .call_from(endpoint, "ListManagedRuleSets", json!({ "Scope": scope }))
            .await;
        let summaries = listed.1["ManagedRuleSets"]
            .as_array()
            .cloned()
            .unwrap_or_default();
        let names: Vec<&str> = summaries
            .iter()
            .filter_map(|summary| summary["Name"].as_str())
            .collect();
        assert_eq!(
            names,
            vec![set_name],
            "case {scope:?}: a refused put must leave the account's sets alone, got {:?}",
            listed.1
        );
        let read = waf
            .call_from(
                endpoint,
                "GetManagedRuleSet",
                json!({ "Name": set_name, "Scope": scope, "Id": &set_id }),
            )
            .await;
        assert_eq!(
            read.1["ManagedRuleSet"]["PublishedVersions"],
            json!({}),
            "case {scope:?}: a refused put must publish nothing, got {:?}",
            read.1
        );
        assert_eq!(
            read.1["LockToken"].as_str(),
            Some(set_lock.as_str()),
            "case {scope:?}: a refused put must not rotate the token, got {:?}",
            read.1
        );
    }
}
