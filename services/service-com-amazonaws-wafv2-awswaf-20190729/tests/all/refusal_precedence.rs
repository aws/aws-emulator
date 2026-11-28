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
//! 3. The TARGET a request acts on is resolved before anything its BODY carries is
//!    resolved against the store. Where that target is named by an ARN, a value WAF cannot
//!    even READ as the kind of ARN the member takes is judged from the request alone and
//!    outranks every store read. Where it is named by an identity — and the `Scope` is part
//!    of that identity — a request addressing one no resource of the namespace holds is the
//!    nonexistent-item refusal whatever reference its body names; only once that target has
//!    resolved are a reference WAF cannot use, and then the lock token, what the call is
//!    told about.
//!
//! Every call goes through the same front door a real client uses (the `Waf` harness
//! in [`crate::web_acl_lifecycle`]), so what these tests observe is what an SDK
//! observes.

use serde_json::{json, Value};
use service_com_amazonaws_wafv2_awswaf_20190729::state::{
    resource_arn, IP_SET_ARN_KIND, NONEXISTENT_ITEM_MESSAGE, RULE_GROUP_ARN_KIND, SCOPE_CLOUDFRONT,
    SCOPE_REGIONAL,
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

/// Rule 3: `PutManagedRuleSetVersions` reports the managed rule set it TARGETS — the resource
/// the call addresses — ahead of everything its body carries, so an identity no managed rule
/// set of this account holds is the nonexistent-item refusal whether the body publishes
/// nothing, a version defined by a rule group that exists, or one defined by a reference WAF
/// cannot use. Nothing is published either way.
///
/// The rows cross each way the target identity can fail to resolve (an unknown name,
/// an unknown id, both) with what the request asks to publish (nothing, a version
/// defined by a rule group that exists, one defined by a well-formed rule-group ARN
/// that resolves to nothing, two versions — one of each), so the answer is shown to
/// turn on the target whatever the body holds; the two blocks after the table are the
/// contrast, against the set the account DOES hold, where the reference is what is reported.
#[tokio::test]
async fn a_put_reports_the_missing_target_before_a_reference_it_cannot_use() {
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
        // What the request asks to publish, and which answer it earns: a body WAF can use
        // leaves the missing TARGET as what is reported, and one carrying a reference it
        // cannot use is told about that reference first.
        let publications = [
            ("nothing to publish", Value::Null, None),
            (
                "a version defined by a rule group that exists",
                json!({ "Version_1.0": { "AssociatedRuleGroupArn": &group_arn } }),
                None,
            ),
            (
                "a version defined by a rule group that does not",
                json!({ "Version_1.0": { "AssociatedRuleGroupArn": &absent_group } }),
                Some(absent_group.clone()),
            ),
            (
                "two versions, one of each",
                json!({
                    "Version_1.0": { "AssociatedRuleGroupArn": &group_arn },
                    "Version_2.0": { "AssociatedRuleGroupArn": &absent_group },
                }),
                Some(absent_group.clone()),
            ),
        ];

        for (identity, name, id) in &identities {
            for (publication, versions, unusable) in &publications {
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
                    "case {case:?}: expected a refusal, got status {status} and body {body}"
                );
                match unusable {
                    // The target is resolved before the body is judged, so the missing set is
                    // what every one of these bodies is told about.
                    Some(arn) => {
                        assert!(
                            error_code(&body).ends_with("WAFNonexistentItemException"),
                            "case {case:?}: expected WAFNonexistentItemException for the \
                             missing set rather than the reference {arn:?}, got {body}"
                        );
                    }
                    None => {
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
            }
        }

        // Against the set that DOES exist, that reference is what is reported: once the
        // target resolves, the body is judged.
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

        // A fault in the REQUEST ALONE is reported only once the target resolves: against a
        // set that does not exist, the missing set is still what the put is told about.
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
            "case {case:?}: expected the missing-set refusal, got status {status} and body {body}"
        );
        assert!(
            error_code(&body).ends_with("WAFNonexistentItemException"),
            "case {case:?}: expected WAFNonexistentItemException, got {body}"
        );

        // ...and against the set the account DOES hold, that same value is reported as the
        // bad parameter it is, which is what shows the rows above turned on the target.
        let case = format!("{scope}, the existing set and a value that is no ARN");
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

/// Rule 3, the whole order in one table: the TARGET the put acts on — the managed rule set
/// `(Scope, Name, Id)` names — outranks everything the body carries, so an identity nothing
/// holds is the nonexistent item whatever reference or token the request sends; against the set
/// the account HOLDS, a reference the put cannot USE — a value it cannot even READ as a
/// rule-group ARN, and one that is well formed and only fails to RESOLVE — outranks the lock
/// token's own modelled form, and once every reference resolves the token's form and then the
/// token the set carries are what is judged.
/// The rows cross HOW the reference fails to be one WAF can use (a value that is no
/// ARN, another service's ARN, a wafv2 ARN of another resource kind — none of which
/// needs the store — against a well-formed rule-group ARN nothing minted and a rule
/// group of the other namespace, which can only be told apart from a usable one by
/// reading the store) with the lock token (well formed, and three ways of failing
/// `LockToken`'s modelled form) and with the target (the set the account holds, an
/// identity nothing holds) — so a refusal that reported the token, or a reference, ahead of a
/// target the put cannot resolve shows up as a difference between the rows.
#[tokio::test]
async fn the_put_judges_its_references_its_token_and_its_target_in_wafs_order() {
    let (waf, engine) = Waf::sharing_engine();
    let (_group_id, group_arn, _group_lock) =
        create_rule_group(&waf, SCOPE_REGIONAL, "vendor-group", 100, json!([])).await;
    // A rule group of the OTHER namespace: a scope is a namespace, so its ARN is not
    // a reference this REGIONAL put can use.
    let (_global_group_id, global_group_arn, _global_group_lock) =
        create_rule_group(&waf, SCOPE_CLOUDFRONT, "vendor-group", 100, json!([])).await;
    let (_ip_set_id, ip_set_arn, _ip_set_lock) =
        create_ip_set(&waf, SCOPE_REGIONAL, "vendor-ips").await;
    let set_name = "vendor-set";
    let (set_id, _set_arn, set_lock) =
        provision_managed_rule_set(&engine, SCOPE_REGIONAL, set_name, None);

    /// A put publishing ONE version defined by `arn`, against `(name, id)`, under
    /// `token`.
    fn put(name: &str, id: &str, token: &str, arn: &str) -> Value {
        json!({
            "Name": name,
            "Scope": SCOPE_REGIONAL,
            "Id": id,
            "LockToken": token,
            "VersionsToPublish": {
                "Version_1.0": { "AssociatedRuleGroupArn": arn, "ForecastedLifetime": 90 },
            },
        })
    }

    // Values no store is needed to refuse: the put cannot read any of them as a
    // reference to one of the vendor's rule groups.
    let unreadable: [(&str, String); 3] = [
        // Past `ResourceArn`'s own modelled length floor of 20, so the value reaches
        // the service rather than the request boundary.
        (
            "a value that is no ARN",
            "not-an-arn-at-all-but-long-enough".to_string(),
        ),
        (
            "another service's ARN",
            "arn:aws:s3:::aws-waf-logs-vendor-bucket".to_string(),
        ),
        ("a wafv2 ARN of another resource kind", ip_set_arn.clone()),
    ];
    // The reference the STORE is what tells apart from a usable one: a well-formed
    // rule-group ARN of THIS namespace that names no rule group of the account.
    let unresolvable: [(&str, String); 1] = [(
        "a rule-group ARN nothing minted",
        format!("{group_arn}-never-minted"),
    )];
    // A reference of the OTHER namespace: an ARN names the region and the namespace the
    // resource it refers to lives in, and a resource lives in exactly that namespace, so
    // this one is a reference the request cannot use at all — a judgement between two
    // values the REQUEST carries, which is why it outranks the target lookup (the rule
    // `crate::managed_rule_set_reference_identity` asserts over its own table).
    let of_another_namespace: [(&str, String); 1] =
        [("a rule group of the other namespace", global_group_arn.clone())];
    // `LockToken` is `@length(1..36)` with `@pattern(^[0-9a-f]{8}-(?:[0-9a-f]{4}-){3}
    // [0-9a-f]{12}$)` (`smithy_query(query="shape",shape="LockToken")`): a value of
    // the wrong shape, a hex-looking value of the wrong length, and the empty string
    // at the length floor each fail it, and the set's own token satisfies it.
    let tokens = [set_lock.as_str(), "stale-token", "1111111", ""];
    let targets = [
        (
            "the set the account holds",
            set_name,
            set_id.as_str(),
            FIELD_RESOURCE_ARN,
        ),
        (
            "an identity nothing holds",
            "never-provisioned",
            WELL_FORMED_UUID,
            "",
        ),
    ];

    for (reference, arn) in &unreadable {
        for token in tokens {
            for (target, name, id, reports_the_reference) in targets {
                let case = format!("{reference}, LockToken {token:?}, against {target}");
                let (status, body) = waf
                    .call("PutManagedRuleSetVersions", put(name, id, token, arn))
                    .await;
                assert_eq!(
                    status, 400,
                    "case {case:?}: expected a refusal, got status {status} and \
                     body {body}"
                );
                if reports_the_reference.is_empty() {
                    // The target is resolved before the body is judged at all.
                    assert_eq!(
                        error_code(&body),
                        "WAFNonexistentItemException",
                        "case {case:?}: the target the put acts on is resolved first, got {body}"
                    );
                    continue;
                }
                assert_invalid_parameter(&body, FIELD_RESOURCE_ARN, arn, ARN_NOT_VALID, &case);
                assert!(
                    !body.to_string().contains("lockToken"),
                    "case {case:?}: the answer must name the reference, not the token, \
                     got {body}"
                );
            }
        }
    }

    // A reference the store is what judges is reported the same way as one no store is
    // needed to refuse: against the set the account holds, whatever the token — and against
    // an identity nothing holds the target is still what is reported.
    for (reference, arn) in &unresolvable {
        for token in tokens {
            for (target, name, id, reports_the_reference) in targets {
                let case = format!("{reference}, LockToken {token:?}, against {target}");
                let (status, body) = waf
                    .call("PutManagedRuleSetVersions", put(name, id, token, arn))
                    .await;
                assert_eq!(
                    status, 400,
                    "case {case:?}: expected a refusal, got status {status} and body \
                     {body}"
                );
                if reports_the_reference.is_empty() {
                    assert_eq!(
                        error_code(&body),
                        "WAFNonexistentItemException",
                        "case {case:?}: the target the put acts on is resolved first, got {body}"
                    );
                    continue;
                }
                assert_invalid_parameter(&body, FIELD_RESOURCE_ARN, arn, ARN_NOT_VALID, &case);
            }
        }
    }

    // A reference of the other namespace is reported whatever the target and whatever
    // the token: both halves of that judgement are in the request, so there is nothing
    // for the store to be asked first.
    for (reference, arn) in &of_another_namespace {
        for token in tokens {
            for (target, name, id, _reports_the_reference) in targets {
                let case = format!("{reference}, LockToken {token:?}, against {target}");
                let (status, body) = waf
                    .call("PutManagedRuleSetVersions", put(name, id, token, arn))
                    .await;
                assert_eq!(
                    status, 400,
                    "case {case:?}: expected a refusal, got status {status} and body \
                     {body}"
                );
                assert_invalid_parameter(&body, FIELD_RESOURCE_ARN, arn, ARN_NOT_VALID, &case);
            }
        }
    }

    // With a reference WAF CAN use, the token's own modelled form is what the set the
    // account holds reports — and an identity nothing holds still reports the missing
    // target, because the target is resolved before the token is judged. This is what
    // shows the rows above reported what they did rather than only ever reporting one
    // refusal.
    for token in ["stale-token", "1111111", ""] {
        let case = format!("a usable reference, LockToken {token:?}, against the set the account holds");
        let (status, body) = waf
            .call(
                "PutManagedRuleSetVersions",
                put(set_name, &set_id, token, &group_arn),
            )
            .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected the token refusal, got status {status} and body {body}"
        );
        assert_eq!(
            error_code(&body),
            "ValidationException",
            "case {case:?}: a member-constraint failure is the request validator's \
             own refusal, got {body}"
        );
        assert!(
            error_message(&body).contains("at 'lockToken' failed to satisfy constraint:"),
            "case {case:?}: the clause must name lockToken, got {body}"
        );

        let case = format!("a usable reference, LockToken {token:?}, against an identity nothing holds");
        let (status, body) = waf
            .call(
                "PutManagedRuleSetVersions",
                put("never-provisioned", WELL_FORMED_UUID, token, &group_arn),
            )
            .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected the missing-target refusal, got status {status} and \
             body {body}"
        );
        assert_eq!(
            error_code(&body),
            "WAFNonexistentItemException",
            "case {case:?}: the target the put acts on is resolved first, got {body}"
        );
    }

    // A usable reference and a well-formed token: the identity nothing holds is the
    // nonexistent item, and the set the account holds reports the token it does not
    // carry as the stale lock it is.
    let (status, body) = waf
        .call(
            "PutManagedRuleSetVersions",
            put(
                "never-provisioned",
                WELL_FORMED_UUID,
                WELL_FORMED_UUID,
                &group_arn,
            ),
        )
        .await;
    assert_eq!(
        (status, error_code(&body)),
        (400, "WAFNonexistentItemException"),
        "an identity nothing holds must be the nonexistent item, got {body}"
    );
    let (status, body) = waf
        .call(
            "PutManagedRuleSetVersions",
            put(set_name, &set_id, WELL_FORMED_UUID, &group_arn),
        )
        .await;
    assert_eq!(
        (status, error_code(&body)),
        (400, "WAFOptimisticLockException"),
        "a token the set does not carry must be the stale lock, got {body}"
    );

    // Nothing above published anything, and the set's own token still works: the
    // ordering refuses bad input, it does not refuse input.
    let (status, body) = waf
        .call(
            "GetManagedRuleSet",
            json!({ "Name": set_name, "Scope": SCOPE_REGIONAL, "Id": &set_id }),
        )
        .await;
    assert_eq!(
        (status, &body["ManagedRuleSet"]["PublishedVersions"]),
        (200, &json!({})),
        "every refused put must publish nothing, got {body}"
    );
    let (status, body) = waf
        .call(
            "PutManagedRuleSetVersions",
            put(set_name, &set_id, &set_lock, &group_arn),
        )
        .await;
    assert_eq!(
        status, 200,
        "a put with nothing wrong must be served, got status {status} and body {body}"
    );
    assert!(
        body["NextLockToken"]
            .as_str()
            .is_some_and(|token| token != set_lock),
        "a served put must rotate the set's token, got {body}"
    );
}

/// One write that addresses a target by identity and carries a reference to another
/// resource: the request against an identity the account holds nothing for, and the
/// same body against the target that exists.
struct AddressedWrite {
    case: &'static str,
    operation: &'static str,
    /// The body sent against an identity no resource of the account carries.
    missing: Value,
    /// The class that body earns against the missing target: the nonexistent-item refusal,
    /// because every write of this service resolves the target it addresses before the
    /// references its body carries.
    missing_class: &'static str,
    /// The same body against the target that DOES exist, with the class it earns
    /// there — `None` for a body whose references all resolve, which that target
    /// would simply accept.
    live: Option<(Value, &'static str)>,
}

/// Rule 3 wherever it holds: a write resolves the target it addresses before the references
/// its body carries, so an identity the account holds nothing for is the nonexistent-item
/// refusal and WAF's own sentence however those references resolve — the managed-rule-set
/// PUBLISH included. The same body against the target that exists is judged on those
/// references.
///
/// The rows are one per (operation, reference) pair the implemented slice can reach —
/// a version publish referencing a rule group, a web-ACL save referencing a rule
/// group and an IP set, a rule-group save referencing an IP set — each with a
/// reference that resolves and one that is well formed and resolves to nothing, so
/// the rule is asserted across the operations it holds for rather than only on the
/// one the recorded case used. Nothing any row sends is stored.
#[tokio::test]
async fn each_write_judges_its_target_and_its_references_in_wafs_order() {
    let (waf, engine) = Waf::sharing_engine();
    let (_group_id, group_arn, _group_lock) =
        create_rule_group(&waf, SCOPE_REGIONAL, "vendor-group", 100, json!([])).await;
    let (_ip_set_id, ip_set_arn, _ip_set_lock) =
        create_ip_set(&waf, SCOPE_REGIONAL, "vendor-ips").await;
    let (acl_id, _acl_arn, acl_lock) =
        create_web_acl(&waf, SCOPE_REGIONAL, "live-acl", json!([])).await;
    let (live_group_id, _live_group_arn, live_group_lock) =
        create_rule_group(&waf, SCOPE_REGIONAL, "live-group", 500, json!([])).await;
    let set_name = "vendor-set";
    let (set_id, _set_arn, set_lock) =
        provision_managed_rule_set(&engine, SCOPE_REGIONAL, set_name, None);

    // Well-formed ARNs of this namespace that nothing minted: each can only be told
    // apart from the live one beside it by reading the store.
    let absent_group = format!("{group_arn}-never-minted");
    let absent_ip_set = format!("{ip_set_arn}-never-minted");
    // The identity no resource of this account carries, and a token no resource
    // carries either.
    let absent_name = "never-provisioned";

    let put = |name: &str, id: &str, token: &str, arn: &str| {
        json!({
            "Name": name,
            "Scope": SCOPE_REGIONAL,
            "Id": id,
            "LockToken": token,
            "VersionsToPublish": {
                "Version_1.0": { "AssociatedRuleGroupArn": arn },
            },
        })
    };
    let save_web_acl = |name: &str, id: &str, token: &str, statement: Value| {
        json!({
            "Name": name,
            "Scope": SCOPE_REGIONAL,
            "Id": id,
            "LockToken": token,
            "DefaultAction": { "Allow": {} },
            "VisibilityConfig": visibility(name),
            "Rules": [{
                "Name": "reference",
                "Priority": 0,
                "Statement": statement,
                "OverrideAction": { "None": {} },
                "VisibilityConfig": visibility("reference"),
            }],
        })
    };
    let save_rule_group = |name: &str, id: &str, token: &str, statement: Value| {
        json!({
            "Name": name,
            "Scope": SCOPE_REGIONAL,
            "Id": id,
            "LockToken": token,
            "VisibilityConfig": visibility(name),
            "Rules": [{
                "Name": "reference",
                "Priority": 0,
                "Statement": statement,
                "Action": { "Block": {} },
                "VisibilityConfig": visibility("reference"),
            }],
        })
    };
    let rule_group_reference =
        |arn: &str| json!({ "RuleGroupReferenceStatement": { "ARN": arn } });
    let ip_set_reference = |arn: &str| json!({ "IPSetReferenceStatement": { "ARN": arn } });

    let writes = vec![
        AddressedWrite {
            case: "a publish referencing a rule group that exists",
            operation: "PutManagedRuleSetVersions",
            missing: put(absent_name, WELL_FORMED_UUID, WELL_FORMED_UUID, &group_arn),
            missing_class: "WAFNonexistentItemException",
            live: None,
        },
        AddressedWrite {
            case: "a publish referencing a rule group that does not",
            operation: "PutManagedRuleSetVersions",
            missing: put(absent_name, WELL_FORMED_UUID, WELL_FORMED_UUID, &absent_group),
            missing_class: "WAFNonexistentItemException",
            live: Some((
                put(set_name, &set_id, &set_lock, &absent_group),
                "WAFInvalidParameterException",
            )),
        },
        AddressedWrite {
            case: "a web-ACL save referencing a rule group that exists",
            operation: "UpdateWebACL",
            missing: save_web_acl(
                absent_name,
                WELL_FORMED_UUID,
                WELL_FORMED_UUID,
                rule_group_reference(&group_arn),
            ),
            missing_class: "WAFNonexistentItemException",
            live: None,
        },
        AddressedWrite {
            case: "a web-ACL save referencing a rule group that does not",
            operation: "UpdateWebACL",
            missing: save_web_acl(
                absent_name,
                WELL_FORMED_UUID,
                WELL_FORMED_UUID,
                rule_group_reference(&absent_group),
            ),
            missing_class: "WAFNonexistentItemException",
            live: Some((
                save_web_acl(
                    "live-acl",
                    &acl_id,
                    &acl_lock,
                    rule_group_reference(&absent_group),
                ),
                "WAFNonexistentItemException",
            )),
        },
        AddressedWrite {
            case: "a rule-group save referencing an IP set that exists",
            operation: "UpdateRuleGroup",
            missing: save_rule_group(
                absent_name,
                WELL_FORMED_UUID,
                WELL_FORMED_UUID,
                ip_set_reference(&ip_set_arn),
            ),
            missing_class: "WAFNonexistentItemException",
            live: None,
        },
        AddressedWrite {
            case: "a rule-group save referencing an IP set that does not",
            operation: "UpdateRuleGroup",
            missing: save_rule_group(
                absent_name,
                WELL_FORMED_UUID,
                WELL_FORMED_UUID,
                ip_set_reference(&absent_ip_set),
            ),
            missing_class: "WAFNonexistentItemException",
            live: Some((
                save_rule_group(
                    "live-group",
                    &live_group_id,
                    &live_group_lock,
                    ip_set_reference(&absent_ip_set),
                ),
                "WAFNonexistentItemException",
            )),
        },
    ];

    for write in &writes {
        let (status, body) = waf.call(write.operation, write.missing.clone()).await;
        assert_eq!(
            status, 400,
            "case {:?}: expected a refusal, got status {status} and body {body}",
            write.case
        );
        assert!(
            error_code(&body).ends_with(write.missing_class),
            "case {:?}: expected {}, got {body}",
            write.case,
            write.missing_class
        );
        if write.missing_class == "WAFNonexistentItemException" {
            assert_eq!(
                error_message(&body),
                NONEXISTENT_ITEM_MESSAGE,
                "case {:?}: expected WAF's own sentence, got {body}",
                write.case
            );
        }

        if let Some((request, code)) = &write.live {
            let (status, body) = waf.call(write.operation, request.clone()).await;
            assert_eq!(
                status, 400,
                "case {:?}: expected {code} against the target that exists, got status \
                 {status} and body {body}",
                write.case
            );
            assert!(
                error_code(&body).ends_with(code),
                "case {:?}: expected {code} once the target resolves, got {body}",
                write.case
            );
        }
    }

    // Nothing above was stored: every addressed resource still holds what it held and
    // still carries the token it carried.
    let read_acl = waf
        .ok(
            "GetWebACL",
            json!({ "Name": "live-acl", "Scope": SCOPE_REGIONAL, "Id": &acl_id }),
        )
        .await;
    assert_eq!(
        (
            &read_acl["WebACL"]["Rules"],
            read_acl["LockToken"].as_str()
        ),
        (&json!([]), Some(acl_lock.as_str())),
        "a refused save must leave the web ACL and its token alone, got {read_acl}"
    );
    let read_group = waf
        .ok(
            "GetRuleGroup",
            json!({ "Name": "live-group", "Scope": SCOPE_REGIONAL, "Id": &live_group_id }),
        )
        .await;
    assert_eq!(
        (
            &read_group["RuleGroup"]["Rules"],
            read_group["LockToken"].as_str()
        ),
        (&json!([]), Some(live_group_lock.as_str())),
        "a refused save must leave the rule group and its token alone, got {read_group}"
    );
    let read_set = waf
        .ok(
            "GetManagedRuleSet",
            json!({ "Name": set_name, "Scope": SCOPE_REGIONAL, "Id": &set_id }),
        )
        .await;
    assert_eq!(
        (
            &read_set["ManagedRuleSet"]["PublishedVersions"],
            read_set["LockToken"].as_str()
        ),
        (&json!({}), Some(set_lock.as_str())),
        "a refused publish must leave the set and its token alone, got {read_set}"
    );
}

/// A token outside the form the model pins for a `LockToken`.
///
/// `LockToken` is `@length(1..36)` with `@pattern(^[0-9a-f]{8}-(?:[0-9a-f]{4}-){3}[0-9a-f]{12}$)`
/// (`smithy_query(query="shape",shape="LockToken")`), and this value satisfies the pattern
/// of neither — a reference the put cannot use is still what the put is told about.
const MALFORMED_LOCK_TOKEN: &str = "stale-token";

/// An account id that is not this emulator's own, for an ARN of another account.
const FOREIGN_ACCOUNT: &str = "123456789012";

/// Every way an `AssociatedRuleGroupArn` can be one WAF cannot use for a published
/// version, given the namespace and the identity the put addresses: the case name and
/// the value to send.
///
/// The rows differ along what the rule generalises over — the ARN's partition, its
/// account, its region, its resource kind, its service, and whether it is an ARN at
/// all — and the first row is the form WAF's own ARN builder mints for the
/// `(Scope, Name, Id)` the put addresses (`state::resource_arn`), which on an account
/// that has minted no rule group resolves to nothing. The first three rows are
/// well-formed rule-group ARNs that only the STORE can tell from a usable reference;
/// the last three need no store at all.
///
/// What is deliberately NOT here is a reference of another NAMESPACE (another region, or
/// the other `Scope`'s namespace segment): an ARN names the namespace its resource lives
/// in, so such a reference is one the request cannot use at all, judged between two
/// values the request itself carries and therefore ahead of this target lookup — the rule
/// `crate::managed_rule_set_reference_identity` asserts over its own table, including on
/// an account that holds no set.
fn references_a_fresh_account_cannot_use(
    scope: &str,
    region: &str,
    name: &str,
    id: &str,
) -> Vec<(String, String)> {
    // The first resource segment of a wafv2 ARN, as `state::resource_arn` lays it out:
    // `global` for the CLOUDFRONT namespace, `regional` for the other.
    let namespace = if scope == SCOPE_CLOUDFRONT {
        "global"
    } else {
        "regional"
    };
    let arn_region = if scope == SCOPE_CLOUDFRONT {
        CLOUDFRONT_ENDPOINT_REGION
    } else {
        region
    };
    let account = ::engine::emulator_account_id();
    let arn_of = |partition: &str, account: &str, arn_region: &str, kind: &str| {
        format!("arn:{partition}:wafv2:{arn_region}:{account}:{namespace}/{kind}/{name}/{id}")
    };
    vec![
        (
            "WAF's own rule-group ARN form for the identity the put addresses".to_string(),
            resource_arn(scope, region, RULE_GROUP_ARN_KIND, name, id),
        ),
        (
            "a rule-group ARN of another account".to_string(),
            arn_of("aws", FOREIGN_ACCOUNT, arn_region, RULE_GROUP_ARN_KIND),
        ),
        (
            "a rule-group ARN of another partition".to_string(),
            arn_of("aws-cn", account, arn_region, RULE_GROUP_ARN_KIND),
        ),
        (
            "a wafv2 ARN of another resource kind".to_string(),
            arn_of("aws", account, arn_region, IP_SET_ARN_KIND),
        ),
        (
            "another service's ARN".to_string(),
            "arn:aws:s3:::aws-waf-logs-vendor-bucket".to_string(),
        ),
        (
            // Past `ResourceArn`'s own modelled length floor of 20, so the value
            // reaches the service rather than the request boundary.
            "a value that is no ARN".to_string(),
            "not-an-arn-at-all-but-long-enough".to_string(),
        ),
    ]
}

/// Rule 3 on an account that owns NOTHING — the state a vendor's first publish call meets:
/// the managed rule set the put ADDRESSES is resolved before anything its body carries, so
/// every such publish is the nonexistent-item refusal carrying WAF's own sentence — whatever
/// reference the body names, whatever version key it publishes under and whatever lock token
/// it sends. Nothing is published.
///
/// The rows cross both namespaces with every way a reference fails to be one WAF can use
/// (see [`references_a_fresh_account_cannot_use`]) and with the lock token (a value outside
/// `LockToken`'s modelled form, and one inside it that no set carries), and the version key
/// the put publishes under varies across the rows — so an answer keyed to one ARN form, one
/// token or one key form shows up as a difference between the rows. The contrast at the end of
/// each namespace is the same body against a managed rule set the account DOES hold, where the
/// reference is what is reported: that is what shows the rows above turned on the target.
#[tokio::test]
async fn a_publish_on_an_account_that_owns_nothing_reports_the_missing_target() {
    // Vendor-plausible version keys, both inside `VersionKeyString`'s
    // `@pattern(^[\w#:\.\-/]+$)` (`smithy_query(query="shape",shape="VersionKeyString")`).
    let version_keys = ["Version_1.0", "1.0"];

    for scope in [SCOPE_REGIONAL, SCOPE_CLOUDFRONT] {
        let (waf, engine) = Waf::sharing_engine();
        let region = if scope == SCOPE_CLOUDFRONT {
            CLOUDFRONT_ENDPOINT_REGION
        } else {
            TEST_REGION
        };
        // The identity the put addresses: no managed rule set of this account carries
        // it, because this account carries none at all.
        let set_name = "never-provisioned";
        let set_id = WELL_FORMED_UUID;

        for (index, (reference, arn)) in
            references_a_fresh_account_cannot_use(scope, region, set_name, set_id)
                .into_iter()
                .enumerate()
        {
            let version = version_keys[index % version_keys.len()];
            for (token_case, token) in [
                ("a token outside LockToken's form", MALFORMED_LOCK_TOKEN),
                ("a token no set carries", WELL_FORMED_UUID),
            ] {
                let case = format!("{scope}, {reference}, {token_case}, under {version}");
                let (status, body) = waf
                    .call_from(
                        region,
                        "PutManagedRuleSetVersions",
                        json!({
                            "Name": set_name,
                            "Scope": scope,
                            "Id": set_id,
                            "LockToken": token,
                            "RecommendedVersion": version,
                            "VersionsToPublish": {
                                version: {
                                    "AssociatedRuleGroupArn": &arn,
                                    "ForecastedLifetime": 90,
                                },
                            },
                        }),
                    )
                    .await;
                assert_eq!(
                    status, 400,
                    "case {case:?}: expected the missing-target refusal, got status {status} and \
                     body {body}"
                );
                assert_eq!(
                    (status, error_code(&body)),
                    (400, "WAFNonexistentItemException"),
                    "case {case:?}: the set the put addresses is resolved first, got {body}"
                );
                assert_eq!(
                    error_message(&body),
                    NONEXISTENT_ITEM_MESSAGE,
                    "case {case:?}: expected WAF's own sentence, got {body}"
                );
                assert!(
                    !body.to_string().contains(&arn),
                    "case {case:?}: the answer must name no reference of the body, got {body}"
                );
            }
        }

        // A body carrying no reference at all reads the same answer on the same empty
        // account, whatever the token's form.
        for token in [MALFORMED_LOCK_TOKEN, WELL_FORMED_UUID] {
            let case = format!("{scope}, nothing to publish, LockToken {token:?}");
            let (status, body) = waf
                .call_from(
                    region,
                    "PutManagedRuleSetVersions",
                    json!({
                        "Name": set_name,
                        "Scope": scope,
                        "Id": set_id,
                        "LockToken": token,
                    }),
                )
                .await;
            assert_eq!(
                (status, error_code(&body)),
                (400, "WAFNonexistentItemException"),
                "case {case:?}: a put on an empty account must report the missing target, \
                 got {body}"
            );
            assert_eq!(
                error_message(&body),
                NONEXISTENT_ITEM_MESSAGE,
                "case {case:?}: expected WAF's own sentence, got {body}"
            );
        }

        // Nothing any refusal above published: the account still owns no managed rule set.
        let (status, listed) = waf
            .call_from(region, "ListManagedRuleSets", json!({ "Scope": scope }))
            .await;
        let summaries = listed["ManagedRuleSets"]
            .as_array()
            .cloned()
            .unwrap_or_default();
        assert_eq!(
            (status, summaries.len()),
            (200, 0),
            "case {scope:?}: a refused publish must leave the account owning nothing, \
             got {listed}"
        );

        // The contrast: once the account DOES hold the set the put addresses, the same
        // unusable reference is what is reported — so the rows above turned on the target.
        let held = "vendor-set";
        let (held_id, _held_arn, held_lock) =
            provision_managed_rule_set(&engine, scope, held, None);
        let unusable = "not-an-arn-at-all-but-long-enough";
        let case = format!("{scope}, the set the account holds with an unusable reference");
        let (status, body) = waf
            .call_from(
                region,
                "PutManagedRuleSetVersions",
                json!({
                    "Name": held,
                    "Scope": scope,
                    "Id": &held_id,
                    "LockToken": &held_lock,
                    "VersionsToPublish": {
                        "Version_1.0": { "AssociatedRuleGroupArn": unusable },
                    },
                }),
            )
            .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected the ARN refusal, got status {status} and body {body}"
        );
        assert_invalid_parameter(&body, FIELD_RESOURCE_ARN, unusable, ARN_NOT_VALID, &case);
    }
}
