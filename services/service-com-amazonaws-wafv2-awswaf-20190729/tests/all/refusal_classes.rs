// SPDX-License-Identifier: CC0-1.0

//! WHICH refusal a WAFv2 request gets, and what it says, end to end over the wire.
//!
//! Five rules are asserted here, each over a TABLE spanning its class rather than
//! over the one request that exposed it:
//!
//! 1. The CLOUDFRONT scope is served at ONE endpoint — "API and SDKs - For all
//!    calls, use the Region endpoint us-east-1" (`Scope`'s own doc) — so a request
//!    naming it anywhere else carries a scope this endpoint cannot act in: the
//!    modelled `WAFInvalidParameterException` with `Field` SCOPE_VALUE, `Parameter`
//!    the requested scope and `Reason` "The scope is not valid.", answered BEFORE
//!    anything the request names is resolved.
//! 2. A managed rule group is identified by its `(VendorName, Name)` pair alone, so
//!    a pair that resolves to nothing is refused with the nonexistent-item class
//!    whose sentence names that pair.
//! 3. An ARN member WAF cannot READ as the kind of ARN the member takes — malformed,
//!    or of the wrong kind — is the invalid-parameter refusal with `Field`
//!    RESOURCE_ARN, answered before the request's other members are judged; an ARN
//!    that is well formed and only fails to RESOLVE is judged against the store, so
//!    it comes after the target the request acts on.
//! 4. A regular expression a save would store has to be one WAF can inspect with:
//!    it must parse AND its match must be able to consume something, so a pattern
//!    that matches the empty string is refused as badly formatted.
//! 5. Monetization is a CloudFront-delivery feature, so a monetization query under
//!    the regional namespace is an invalid OPERATION, not an invalid parameter.
//!
//! Every call goes through the same front door a real client uses (the `Waf` harness
//! in [`crate::web_acl_lifecycle`]), so what these tests observe is what an SDK
//! observes.

use serde_json::{json, Value};
use service_com_amazonaws_wafv2_awswaf_20190729::state::{
    MANAGED_RULE_GROUP_CATALOGUE, SCOPE_CLOUDFRONT, SCOPE_REGIONAL,
};

use crate::rule_group_lifecycle::create_rule_group;
use crate::web_acl_lifecycle::{
    create_web_acl, rule, visibility, Waf, CLOUDFRONT_ENDPOINT_REGION, TEST_REGION,
};

/// The `ParameterExceptionField` value naming a `Scope`
/// (`smithy_query(query="shape",shape="ParameterExceptionField")` lists SCOPE_VALUE
/// among its values).
const FIELD_SCOPE_VALUE: &str = "SCOPE_VALUE";

/// The `ParameterExceptionField` value naming a resource ARN. See
/// [`FIELD_SCOPE_VALUE`].
const FIELD_RESOURCE_ARN: &str = "RESOURCE_ARN";

/// The `ParameterExceptionField` value naming a regex pattern set. See
/// [`FIELD_SCOPE_VALUE`].
const FIELD_REGEX_PATTERN_SET: &str = "REGEX_PATTERN_SET";

/// The `ParameterExceptionField` value naming a rule statement. See
/// [`FIELD_SCOPE_VALUE`].
const FIELD_STATEMENT: &str = "STATEMENT";

/// The `Reason` WAF carries for a scope it will not accept, as the recorded refusals
/// of that class word it.
const SCOPE_NOT_VALID: &str = "The scope is not valid.";

/// The `Reason` WAF carries for an ARN it will not accept.
const ARN_NOT_VALID: &str = "The ARN isn't valid. A valid ARN begins with arn: and \
     includes other information separated by colons or slashes.";

/// The `Reason` WAF carries for a parameter whose text is not in the form that
/// parameter takes.
const FORMAT_NOT_VALID: &str = "The parameter contains formatting that is not valid.";

/// The `Message` WAF carries when a monetization query names the regional namespace.
const MONETIZATION_NOT_AVAILABLE: &str = "Monetization is not available for regional \
     resources. Use CloudFront scope to configure monetization.";

/// Endpoints other than the one the global namespace is served at.
const OTHER_ENDPOINTS: [&str; 3] = [TEST_REGION, "eu-central-1", "ap-southeast-2"];

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

/// One operation that answers 200 for a well-formed scoped request against an empty
/// account: its name and the request members beside `Scope`.
fn scoped_reads() -> Vec<(&'static str, Value)> {
    vec![
        ("ListIPSets", json!({})),
        ("ListWebACLs", json!({})),
        ("ListRuleGroups", json!({})),
        ("ListRegexPatternSets", json!({})),
        ("ListAPIKeys", json!({})),
        ("ListManagedRuleSets", json!({})),
        ("ListLoggingConfigurations", json!({})),
        ("ListAvailableManagedRuleGroups", json!({})),
        ("DescribeAllManagedProducts", json!({})),
        ("CheckCapacity", json!({ "Rules": [] })),
    ]
}

/// `body` with `Scope` set, the way every scoped request carries it.
fn scoped(scope: &str, body: &Value) -> Value {
    let mut request = body.clone();
    if let Some(members) = request.as_object_mut() {
        members.insert("Scope".to_string(), json!(scope));
    }
    request
}

/// Rule 1: the global namespace is served at its own endpoint and refused at every
/// other, while the regional namespace is served at all of them.
///
/// The rows cross ten scoped operations — reads of five resource kinds, the API-key
/// and managed-rule-set listings, the published catalogue, and the pricing call —
/// with four endpoints and both scopes, so the only thing that separates a served
/// call from a refused one is the (scope, endpoint) pair.
#[tokio::test]
async fn the_cloudfront_scope_is_served_only_at_its_own_endpoint() {
    let waf = Waf::new();
    for (operation, members) in scoped_reads() {
        // The regional namespace is every endpoint's own, including the one the
        // global namespace is served at.
        for region in OTHER_ENDPOINTS
            .into_iter()
            .chain([CLOUDFRONT_ENDPOINT_REGION])
        {
            let case = format!("{operation} REGIONAL from {region}");
            let (status, body) = waf
                .call_from(region, operation, scoped(SCOPE_REGIONAL, &members))
                .await;
            assert_eq!(
                status, 200,
                "case {case:?}: the regional namespace is served at every endpoint, \
                 got status {status} and body {body}"
            );
        }

        let case = format!("{operation} CLOUDFRONT from {CLOUDFRONT_ENDPOINT_REGION}");
        let (status, body) = waf
            .call_from(
                CLOUDFRONT_ENDPOINT_REGION,
                operation,
                scoped(SCOPE_CLOUDFRONT, &members),
            )
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: the global namespace is served at its own endpoint, got \
             status {status} and body {body}"
        );

        for region in OTHER_ENDPOINTS {
            let case = format!("{operation} CLOUDFRONT from {region}");
            let (status, body) = waf
                .call_from(region, operation, scoped(SCOPE_CLOUDFRONT, &members))
                .await;
            assert_eq!(
                status, 400,
                "case {case:?}: the global namespace is not served here, got status \
                 {status} and body {body}"
            );
            assert_invalid_parameter(
                &body,
                FIELD_SCOPE_VALUE,
                SCOPE_CLOUDFRONT,
                SCOPE_NOT_VALID,
                &case,
            );
        }
    }
}

/// Rule 1, continued: the scope is judged BEFORE the request's target is resolved,
/// and before a mutation writes anything.
///
/// A caller at the wrong endpoint is told its scope is wrong — not that its resource
/// does not exist, which is what sends it off to recreate a resource it already has
/// — and the same request made at the right endpoint reaches the lookup, where the
/// missing-resource failure is the honest answer.
#[tokio::test]
async fn the_scope_is_judged_before_the_request_is_resolved() {
    let waf = Waf::new();
    let id = "11111111-2222-3333-4444-555555555555";
    let triple = json!({ "Name": "absent-set", "Id": id });
    let created = json!({
        "Name": "wrong-endpoint-set",
        "IPAddressVersion": "IPV4",
        "Addresses": ["10.0.0.0/24"],
    });

    for (operation, members) in [("GetIPSet", triple), ("CreateIPSet", created)] {
        for region in OTHER_ENDPOINTS {
            let case = format!("{operation} CLOUDFRONT from {region}");
            let (status, body) = waf
                .call_from(region, operation, scoped(SCOPE_CLOUDFRONT, &members))
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
    }

    // At the endpoint that serves the global namespace the same read reaches the
    // lookup, and the triple that names nothing is the missing-resource failure.
    let case = "GetIPSet CLOUDFRONT at its own endpoint";
    let (status, body) = waf
        .call_from(
            CLOUDFRONT_ENDPOINT_REGION,
            "GetIPSet",
            json!({ "Name": "absent-set", "Scope": SCOPE_CLOUDFRONT, "Id": id }),
        )
        .await;
    assert_eq!(
        status, 400,
        "case {case:?}: a triple naming nothing is refused, got status {status} and \
         body {body}"
    );
    assert!(
        error_code(&body).ends_with("WAFNonexistentItemException"),
        "case {case:?}: expected the missing-resource failure, got {body}"
    );

    // The refused create left nothing behind at the endpoint that would have served
    // it: the mutation never ran.
    let listed = waf
        .call_from(
            CLOUDFRONT_ENDPOINT_REGION,
            "ListIPSets",
            json!({ "Scope": SCOPE_CLOUDFRONT }),
        )
        .await;
    assert_eq!(
        listed.1["IPSets"],
        json!([]),
        "a create refused at the wrong endpoint must store nothing, got {:?}",
        listed.1
    );
}

/// Rule 2: the nonexistent-item refusal for a managed rule group names the vendor and
/// the group that resolved to nothing.
///
/// The rows differ along what the pair can get wrong — an unpublished group name, an
/// unknown vendor, either name in the wrong case — and span both operations that
/// resolve such a pair and both scopes. The catalogued pair of the same table is
/// served, so the pair is the only difference between the two answers.
#[tokio::test]
async fn a_managed_rule_group_that_resolves_to_nothing_is_named_in_the_refusal() {
    let waf = Waf::new();
    let catalogued = MANAGED_RULE_GROUP_CATALOGUE[0];
    let rows: Vec<(&str, String, String)> = vec![
        (
            "a group name no vendor publishes",
            catalogued.vendor_name.to_string(),
            "AWSManagedRulesNoSuchRuleSet".to_string(),
        ),
        (
            "an unknown vendor",
            "NotARealVendor".to_string(),
            "NotARealRuleGroup".to_string(),
        ),
        (
            "a catalogued group under an unknown vendor",
            "SomeOtherVendor".to_string(),
            catalogued.name.to_string(),
        ),
        (
            "a catalogued group name in the wrong case",
            catalogued.vendor_name.to_string(),
            catalogued.name.to_lowercase(),
        ),
    ];

    for (case, vendor, name) in rows {
        for operation in [
            "DescribeManagedRuleGroup",
            "ListAvailableManagedRuleGroupVersions",
        ] {
            for scope in [SCOPE_REGIONAL, SCOPE_CLOUDFRONT] {
                let case = format!("{operation} {scope}: {case}");
                let body = waf
                    .refused(
                        operation,
                        json!({ "Scope": scope, "VendorName": &vendor, "Name": &name }),
                        "WAFNonexistentItemException",
                        &case,
                    )
                    .await;
                assert_eq!(
                    error_message(&body),
                    format!(
                        "ManagedRules with name {name} by vendor {vendor} does not \
                         exist. Please check your inputs."
                    ),
                    "case {case:?}: the refusal must name the group and its vendor, \
                     got {body}"
                );
            }
        }
    }

    // The catalogued pair is served by both operations, so what the rows above
    // changed is the pair and nothing else.
    for operation in [
        "DescribeManagedRuleGroup",
        "ListAvailableManagedRuleGroupVersions",
    ] {
        let served = waf
            .ok(
                operation,
                json!({
                    "Scope": SCOPE_REGIONAL,
                    "VendorName": catalogued.vendor_name,
                    "Name": catalogued.name,
                }),
            )
            .await;
        assert!(
            !served.is_null(),
            "{operation} must serve the catalogued pair, got {served}"
        );
    }
}

/// Rule 3: an `AssociatedRuleGroupArn` WAF cannot READ as a rule-group ARN is the
/// invalid-parameter refusal naming the ARN, and it is answered before the lock token
/// and before the managed rule set the put targets are judged.
///
/// The rows differ in HOW the ARN fails to be one — not an ARN at all, another
/// service's ARN, a wafv2 ARN of another resource kind — and each is sent with a lock
/// token that is not even `LockToken`-shaped and against a set no account holds, so
/// what the refusal reports is the ARN rather than either of those. Each of those
/// faults is in the REQUEST, which is why it is reported ahead of both: an ARN that is
/// well formed and only fails to RESOLVE is a different judgement, made against the
/// store and so after the target the put acts on
/// (`crate::refusal_precedence`).
#[tokio::test]
async fn an_arn_the_put_cannot_use_is_refused_before_anything_else() {
    let waf = Waf::new();
    let (_group_id, group_arn, _group_lock) =
        create_rule_group(&waf, SCOPE_REGIONAL, "vendor-group", 100, json!([])).await;
    let ip_set_kind = group_arn.replace("/rulegroup/", "/ipset/");

    for (case, arn) in [
        (
            // Past `ResourceArn`'s own modelled length floor of 20, so the value
            // reaches the service rather than the boundary.
            "a value that is no ARN",
            "not-an-arn-at-all-but-long-enough".to_string(),
        ),
        (
            "another service's ARN",
            "arn:aws:s3:::aws-waf-logs-bucket".to_string(),
        ),
        ("a wafv2 ARN of another kind", ip_set_kind),
    ] {
        let body = waf
            .call_from(
                TEST_REGION,
                "PutManagedRuleSetVersions",
                json!({
                    "Name": "no-such-set",
                    "Scope": SCOPE_REGIONAL,
                    "Id": "00000000-0000-0000-0000-000000000000",
                    "LockToken": "stale-token",
                    "VersionsToPublish": {
                        "Version_1.0": { "AssociatedRuleGroupArn": &arn },
                    },
                }),
            )
            .await;
        assert_eq!(
            body.0, 400,
            "case {case:?}: expected a refusal, got status {} and body {}",
            body.0, body.1
        );
        assert_invalid_parameter(&body.1, FIELD_RESOURCE_ARN, &arn, ARN_NOT_VALID, case);
    }

    // With an ARN WAF CAN use, the next thing judged is the lock token's own
    // modelled form — which is how the rows above are shown to have reported the ARN
    // first rather than only ever reporting ARNs.
    let (status, body) = waf
        .call_from(
            TEST_REGION,
            "PutManagedRuleSetVersions",
            json!({
                "Name": "no-such-set",
                "Scope": SCOPE_REGIONAL,
                "Id": "00000000-0000-0000-0000-000000000000",
                "LockToken": "stale-token",
                "VersionsToPublish": {
                    "Version_1.0": { "AssociatedRuleGroupArn": &group_arn },
                },
            }),
        )
        .await;
    assert_eq!(
        status, 400,
        "a lock token that is not LockToken-shaped is refused, got {body}"
    );
    assert!(
        error_message(&body).contains("lockToken"),
        "once the ARN resolves, the lock token is what is reported, got {body}"
    );
}

/// One regular expression, and whether WAF will inspect requests with it.
struct Pattern {
    /// What the case is called in an assertion.
    case: &'static str,
    /// The expression the request submits.
    pattern: &'static str,
    /// Whether the service must accept it.
    usable: bool,
}

/// Rule 4's table: expressions whose match can be EMPTY, and expressions that have
/// to consume at least one byte.
///
/// The rows differ along what makes the match empty — a star over any character, a
/// star over one, an optional group, an optional repetition range, an anchored empty
/// alternative, an alternation one branch of which is empty — and, on the accepted
/// side, the same shapes made to consume something, plus a plain literal and a
/// counted repetition. Every row parses, so the only thing that separates the two
/// halves is whether the expression can match nothing.
fn patterns() -> Vec<Pattern> {
    vec![
        Pattern {
            case: "any character, any number of times",
            pattern: ".*",
            usable: false,
        },
        Pattern {
            case: "one character, any number of times",
            pattern: "a*",
            usable: false,
        },
        Pattern {
            case: "an optional group",
            pattern: "(admin)?",
            usable: false,
        },
        Pattern {
            case: "a repetition range whose floor is zero",
            pattern: "[a-z]{0,5}",
            usable: false,
        },
        Pattern {
            case: "the anchored empty string",
            pattern: "^$",
            usable: false,
        },
        Pattern {
            case: "an alternation with an empty branch",
            pattern: "admin|",
            usable: false,
        },
        Pattern {
            case: "whitespace, any number of times",
            pattern: r"\s*",
            usable: false,
        },
        Pattern {
            case: "any character, at least once",
            pattern: ".+",
            usable: true,
        },
        Pattern {
            case: "one character, at least once",
            pattern: "a+",
            usable: true,
        },
        Pattern {
            case: "a required group",
            pattern: "(admin)",
            usable: true,
        },
        Pattern {
            case: "a repetition range whose floor is one",
            pattern: "[a-z]{1,5}",
            usable: true,
        },
        Pattern {
            case: "an anchored literal",
            pattern: "^/admin",
            usable: true,
        },
        Pattern {
            case: "an alternation of two literals",
            pattern: "admin|root",
            usable: true,
        },
    ]
}

/// `[{ "RegexString": … }]` — a `RegularExpressionList` over one pattern.
fn regular_expression_list(pattern: &str) -> Value {
    json!([{ "RegexString": pattern }])
}

/// Rule 4: a pattern WAF cannot inspect requests with is refused wherever a save
/// carries one, and one that consumes at least a byte is stored.
///
/// Driven through the pattern-set save path and through a rule's own
/// `RegexMatchStatement`, so the judgement is shown to be about the expression and
/// not about the operation that carried it.
#[tokio::test]
async fn an_expression_whose_match_can_be_empty_is_refused() {
    let waf = Waf::new();
    for (index, row) in patterns().iter().enumerate() {
        let Pattern {
            case,
            pattern,
            usable,
        } = row;
        let set_name = format!("empty-match-set-{index}");
        let acl_name = format!("empty-match-acl-{index}");
        let statement = json!({
            "RegexMatchStatement": {
                "RegexString": pattern,
                "FieldToMatch": { "UriPath": {} },
                "TextTransformations": [{ "Priority": 0, "Type": "NONE" }],
            }
        });
        let create_set = json!({
            "Name": &set_name,
            "Scope": SCOPE_REGIONAL,
            "RegularExpressionList": regular_expression_list(pattern),
        });
        let create_acl = json!({
            "Name": &acl_name,
            "Scope": SCOPE_REGIONAL,
            "DefaultAction": { "Allow": {} },
            "VisibilityConfig": visibility(&acl_name),
            "Rules": [rule("inspect", statement)],
        });

        if *usable {
            let created = waf.ok("CreateRegexPatternSet", create_set).await;
            let id = created["Summary"]["Id"]
                .as_str()
                .unwrap_or_else(|| panic!("case {case:?}: the create answers Summary.Id"))
                .to_string();
            let got = waf
                .ok(
                    "GetRegexPatternSet",
                    json!({ "Name": &set_name, "Scope": SCOPE_REGIONAL, "Id": &id }),
                )
                .await;
            assert_eq!(
                got["RegexPatternSet"]["RegularExpressionList"],
                regular_expression_list(pattern),
                "case {case:?}: an accepted expression must read back byte for byte, \
                 got {got}"
            );
            waf.ok("CreateWebACL", create_acl).await;
            continue;
        }

        let body = waf
            .refused(
                "CreateRegexPatternSet",
                create_set,
                "WAFInvalidParameterException",
                case,
            )
            .await;
        assert_invalid_parameter(
            &body,
            FIELD_REGEX_PATTERN_SET,
            pattern,
            FORMAT_NOT_VALID,
            case,
        );
        let listed = waf
            .ok("ListRegexPatternSets", json!({ "Scope": SCOPE_REGIONAL }))
            .await;
        assert!(
            !listed.to_string().contains(set_name.as_str()),
            "case {case:?}: a refused create must leave no set named {set_name:?}, \
             got {listed}"
        );

        let body = waf
            .refused(
                "CreateWebACL",
                create_acl,
                "WAFInvalidParameterException",
                case,
            )
            .await;
        assert_invalid_parameter(&body, FIELD_STATEMENT, pattern, FORMAT_NOT_VALID, case);
    }
}

/// Rule 5: a monetization query under the regional namespace is the invalid
/// OPERATION, with the sentence that says what is unavailable and what to use
/// instead.
///
/// Both monetization reads are rows of the same rule, and the same query under the
/// global namespace is served, so the scope is the only difference.
#[tokio::test]
async fn monetization_under_the_regional_namespace_is_an_invalid_operation() {
    let waf = Waf::new();
    let window = json!({ "StartTime": 1_767_225_600_i64, "EndTime": 1_767_229_200_i64 });

    for operation in ["ListSettlementRecords", "GetRevenueStatisticsSummary"] {
        let body = waf
            .refused(
                operation,
                json!({
                    "Scope": SCOPE_REGIONAL,
                    "Currency": "USDC",
                    "TimeWindow": window.clone(),
                }),
                "WAFInvalidOperationException",
                operation,
            )
            .await;
        assert_eq!(
            error_message(&body),
            MONETIZATION_NOT_AVAILABLE,
            "case {operation:?}: the refusal must carry WAF's monetization sentence, \
             got {body}"
        );

        let served = waf
            .ok(
                operation,
                json!({
                    "Scope": SCOPE_CLOUDFRONT,
                    "Currency": "USDC",
                    "TimeWindow": window.clone(),
                }),
            )
            .await;
        assert!(
            !served.is_null(),
            "case {operation:?}: the global namespace is served, got {served}"
        );
    }
}

/// A traffic query over a web ACL nothing was recorded for answers the EMPTY result,
/// not a refusal: an empty aggregation is a success.
///
/// The rows differ in why nothing matches — a web ACL with no traffic at all, a
/// window before any traffic could have been recorded, and a path prefix no request
/// could be under — so "no data yet" is shown to be one answer and not three.
#[tokio::test]
async fn a_traffic_query_that_matches_nothing_answers_the_empty_result() {
    let waf = Waf::new();
    let (_id, arn, _lock) = create_web_acl(&waf, SCOPE_REGIONAL, "quiet-acl", json!([])).await;

    for (case, extra) in [
        ("a web ACL with no traffic", json!({})),
        (
            "a window in the past",
            json!({ "TimeWindow": { "StartTime": 1_000_000, "EndTime": 1_000_060 } }),
        ),
        ("a path prefix nothing is under", json!({ "UriPathPrefix": "/nothing" })),
    ] {
        let mut request = json!({
            "WebAclArn": &arn,
            "Scope": SCOPE_REGIONAL,
            "TimeWindow": { "StartTime": 1_767_225_600_i64, "EndTime": 1_767_229_200_i64 },
            "Limit": 50,
            "NumberOfTopTrafficBotsPerPath": 10,
        });
        if let (Some(target), Some(source)) = (request.as_object_mut(), extra.as_object()) {
            for (key, value) in source {
                target.insert(key.clone(), value.clone());
            }
        }
        let answer = waf.ok("GetTopPathStatisticsByTraffic", request).await;
        assert_eq!(
            (
                answer["PathStatistics"].as_array().map(Vec::len),
                answer["TotalRequestCount"].as_i64()
            ),
            (Some(0), Some(0)),
            "case {case:?}: expected an empty PathStatistics list and \
             TotalRequestCount 0, got {answer}"
        );
    }
}
