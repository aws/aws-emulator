// SPDX-License-Identifier: CC0-1.0

//! WHICH refusal a WAFv2 request gets when the IDENTIFIERS it hands over are ones WAF
//! will not act on, and how the request validator STATES a member-constraint failure —
//! end to end over the wire.
//!
//! Four rules, each asserted over a TABLE spanning its class rather than over the one
//! request that exposed it:
//!
//! 1. An ARN identifies a resource completely — the region whose endpoint serves it, its
//!    namespace, its kind, its name and its id — so a request addressing a web ACL BY
//!    ARN has all of that judged before anything is resolved and before the `Scope`
//!    member is judged: a value that is no wafv2 resource ARN, one naming another region,
//!    and one whose namespace is not the `Scope` the request names are the modelled
//!    `WAFInvalidParameterException` with `Field` RESOURCE_ARN and the ARN echoed in
//!    `Parameter`, while an ARN naming a resource of another KIND is refused with the
//!    access-denied code and a sentence naming the request shape and the ARN.
//! 2. A request that leaves out information WAF cannot act without is reported against
//!    the REQUEST: the access-denied code with "Critical information is missing in your
//!    request: " followed by the request shape and every member it did or did not carry,
//!    absent members rendered `null`.
//! 3. A `Scope` WAF reads as a scope but will not act on is its own bad-parameter
//!    refusal naming SCOPE_VALUE; a token its scope vocabulary does not carry at all is
//!    the request validator's value-set clause.
//! 4. A member that breaks both its `@pattern` and a `@length` bound has its pattern
//!    clause stated FIRST, and a `@pattern` is stated as the full-match expression
//!    (an unanchored pattern wrapped in `.*`) rather than as the model's bare text.
//!
//! Every call goes through the same front door a real client uses (the `Waf` harness in
//! [`crate::web_acl_lifecycle`]), so what these tests observe is what an SDK observes.

use serde_json::{json, Value};
use service_com_amazonaws_wafv2_awswaf_20190729::state::{SCOPE_CLOUDFRONT, SCOPE_REGIONAL};

use crate::rule_group_lifecycle::create_rule_group;
use crate::web_acl_lifecycle::{
    create_ip_set, create_web_acl, visibility, Waf, CLOUDFRONT_ENDPOINT_REGION, TEST_REGION,
};

/// The `ParameterExceptionField` value naming a resource ARN
/// (`smithy_query(query="shape",shape="ParameterExceptionField")` lists RESOURCE_ARN
/// among its values).
const FIELD_RESOURCE_ARN: &str = "RESOURCE_ARN";

/// The `ParameterExceptionField` value naming a `Scope`. See [`FIELD_RESOURCE_ARN`].
const FIELD_SCOPE_VALUE: &str = "SCOPE_VALUE";

/// The `Reason` WAF carries for an ARN it will not accept.
const ARN_NOT_VALID: &str = "The ARN isn't valid. A valid ARN begins with arn: and \
     includes other information separated by colons or slashes.";

/// The `Reason` WAF carries for a scope it will not accept.
const SCOPE_NOT_VALID: &str = "The scope is not valid.";

/// The error code WAF answers a request whose identifiers it will not act on.
const ACCESS_DENIED: &str = "AccessDeniedException";

/// The error code a member-constraint failure carries.
const VALIDATION_EXCEPTION: &str = "ValidationException";

/// The error body's message member, whichever casing the shape models.
fn error_message(body: &Value) -> &str {
    body.get("Message")
        .or_else(|| body.get("message"))
        .and_then(Value::as_str)
        .unwrap_or_default()
}

/// The three members the model gives `WAFInvalidParameterException`, and the message WAF
/// composes from exactly those three.
fn assert_invalid_parameter(body: &Value, field: &str, parameter: &str, reason: &str, case: &str) {
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

/// Rule 1: every way an ARN can be one this request may not use, and the one refusal
/// each way gets.
///
/// The rows span the forms: a value that is no ARN at all, another service's ARN, a
/// namespace segment wafv2 does not have, too few and empty resource segments, a web ACL
/// ARN of another region, the global namespace's ARN at a regional endpoint, and an ARN
/// of each other wafv2 resource kind. The web ACL's own ARN, read at its own endpoint, is
/// served in the same table, so what separates a served call from a refused one is the
/// ARN alone.
#[tokio::test]
async fn an_arn_a_get_cannot_use_is_refused_before_the_web_acl_is_resolved() {
    let waf = Waf::new();
    let (_acl_id, acl_arn, _acl_lock) =
        create_web_acl(&waf, SCOPE_REGIONAL, "ident-acl", json!([])).await;
    let (_global_id, global_arn, _global_lock) =
        create_web_acl(&waf, SCOPE_CLOUDFRONT, "ident-global-acl", json!([])).await;
    let (_set_id, ip_set_arn, _set_lock) = create_ip_set(&waf, SCOPE_REGIONAL, "ident-ips").await;
    let (_group_id, group_arn, _group_lock) =
        create_rule_group(&waf, SCOPE_REGIONAL, "ident-group", 100, json!([])).await;
    let pattern_set_arn = ip_set_arn.replace("/ipset/", "/regexpatternset/");
    let other_region_arn = acl_arn.replace(TEST_REGION, "eu-west-3");

    // The web ACL's own ARN, at the endpoint of the region that ARN names, resolves.
    let found = waf.ok("GetWebACL", json!({ "ARN": &acl_arn })).await;
    assert_eq!(
        found["WebACL"]["Name"],
        json!("ident-acl"),
        "a web ACL's own ARN must resolve at its own endpoint, got {found}"
    );
    let global = waf
        .call_from(
            CLOUDFRONT_ENDPOINT_REGION,
            "GetWebACL",
            json!({ "ARN": &global_arn }),
        )
        .await;
    assert_eq!(
        global.1["WebACL"]["Name"],
        json!("ident-global-acl"),
        "the global namespace's ARN must resolve at the endpoint that serves it, got {:?}",
        global.1
    );

    // Every form WAF reports as an ARN it cannot use, each echoed in `Parameter`.
    let unusable: Vec<(&str, String)> = vec![
        // Past `ResourceArn`'s modelled length floor of 20, so the value reaches the
        // service rather than the boundary.
        ("a value that is no ARN", "not-an-arn-but-long-enough".to_string()),
        (
            "another service's ARN",
            "arn:aws:s3:::a-bucket-named-like-this".to_string(),
        ),
        (
            "a namespace segment wafv2 does not have",
            "arn:aws:wafv2:us-west-2:000000000000:worldwide/webacl/absent/1111".to_string(),
        ),
        (
            "too few resource segments",
            "arn:aws:wafv2:us-west-2:000000000000:regional/webacl/absent".to_string(),
        ),
        (
            "an empty resource segment",
            "arn:aws:wafv2:us-west-2:000000000000:regional/webacl//1111".to_string(),
        ),
        ("a web ACL ARN of another region", other_region_arn),
        (
            "the global namespace's ARN at a regional endpoint",
            global_arn.clone(),
        ),
    ];
    for (case, arn) in &unusable {
        let body = waf
            .refused(
                "GetWebACL",
                json!({ "ARN": arn }),
                "WAFInvalidParameterException",
                case,
            )
            .await;
        assert_invalid_parameter(&body, FIELD_RESOURCE_ARN, arn, ARN_NOT_VALID, case);
    }

    // An ARN of the right form naming another KIND of resource: the access-denied code
    // and a sentence naming the request shape and the ARN.
    for (case, arn) in [
        ("an IP set's ARN", ip_set_arn.as_str()),
        ("a rule group's ARN", group_arn.as_str()),
        ("a regex pattern set's ARN", pattern_set_arn.as_str()),
    ] {
        let body = waf
            .refused("GetWebACL", json!({ "ARN": arn }), ACCESS_DENIED, case)
            .await;
        assert_eq!(
            error_message(&body),
            format!("Invalid ARN provided for GetWebACLRequest {arn}"),
            "case {case:?}: the refusal must name the request shape and the ARN, got {body}"
        );
    }

    // The ARN is judged before the `Scope` member: a request whose ARN and scope name
    // different namespaces is told about its ARN. The row is driven at the endpoint that
    // SERVES the named scope, because a scope this endpoint cannot act in is WAF's own
    // precondition and is answered ahead of everything else about the request
    // (`crate::service::scope_gate`) — so what this row varies is the disagreement alone.
    let case = "a regional ARN with the global scope, at the endpoint serving that scope";
    let (status, body) = waf
        .call_from(
            CLOUDFRONT_ENDPOINT_REGION,
            "GetWebACL",
            json!({ "ARN": &acl_arn, "Scope": SCOPE_CLOUDFRONT }),
        )
        .await;
    assert_eq!(
        status, 400,
        "case {case:?}: expected the ARN refusal, got status {status} and body {body}"
    );
    assert_invalid_parameter(&body, FIELD_RESOURCE_ARN, &acl_arn, ARN_NOT_VALID, case);

    // A well-formed web ACL ARN of this endpoint's own region naming nothing stored is
    // the nonexistent item, so the rows above refuse on the ARN rather than on absence.
    waf.refused(
        "GetWebACL",
        json!({ "ARN": format!("{}-never-minted", acl_arn.replace("ident-acl", "absent")) }),
        "WAFNonexistentItemException",
        "a web ACL ARN of this region naming nothing",
    )
    .await;
}

/// Rule 2: a request that leaves out what WAF cannot act without is reported against the
/// request shape, member by member.
///
/// The rows span the incomplete identifier combinations of `GetWebACL` — none of the four
/// members, each one alone, and each pair — and the two shapes of a `ListWebACLs` that
/// carries no `Scope`, with and without the paging members, so the rendering is exercised
/// with absent and supplied values in every position.
#[tokio::test]
async fn a_request_missing_what_it_cannot_act_without_names_the_request_shape() {
    let waf = Waf::new();
    create_web_acl(&waf, SCOPE_REGIONAL, "omission-acl", json!([])).await;
    let id = "11111111-2222-3333-4444-555555555555";

    // GetWebACL addresses a web ACL by its ARN or by (Name, Scope, Id); anything less is
    // an omission. Each row is the request members and the rendering they produce.
    let rows: Vec<(&str, Value, String)> = vec![
        (
            "no member at all",
            json!({}),
            "GetWebACLRequest(name=null, scope=null, id=null, aRN=null)".to_string(),
        ),
        (
            "only a name",
            json!({ "Name": "omission-acl" }),
            "GetWebACLRequest(name=omission-acl, scope=null, id=null, aRN=null)".to_string(),
        ),
        (
            "only a scope",
            json!({ "Scope": SCOPE_REGIONAL }),
            format!("GetWebACLRequest(name=null, scope={SCOPE_REGIONAL}, id=null, aRN=null)"),
        ),
        (
            "only an id",
            json!({ "Id": id }),
            format!("GetWebACLRequest(name=null, scope=null, id={id}, aRN=null)"),
        ),
        (
            "a name and a scope but no id",
            json!({ "Name": "omission-acl", "Scope": SCOPE_REGIONAL }),
            format!(
                "GetWebACLRequest(name=omission-acl, scope={SCOPE_REGIONAL}, id=null, aRN=null)"
            ),
        ),
        (
            "a name and an id but no scope",
            json!({ "Name": "omission-acl", "Id": id }),
            format!("GetWebACLRequest(name=omission-acl, scope=null, id={id}, aRN=null)"),
        ),
        (
            "a scope and an id but no name",
            json!({ "Scope": SCOPE_REGIONAL, "Id": id }),
            format!("GetWebACLRequest(name=null, scope={SCOPE_REGIONAL}, id={id}, aRN=null)"),
        ),
    ];
    for (case, request, rendering) in rows {
        let body = waf
            .refused("GetWebACL", request, ACCESS_DENIED, case)
            .await;
        assert_eq!(
            error_message(&body),
            format!("Critical information is missing in your request: {rendering}."),
            "case {case:?}: the refusal must render the request shape, got {body}"
        );
    }

    // ListWebACLs cannot list without a `Scope` (`@required`), and the same sentence
    // renders its three members, whichever of the other two the request carried.
    let listings: Vec<(&str, Value, &str)> = vec![
        (
            "no member at all",
            json!({}),
            "ListWebACLsRequest(scope=null, nextMarker=null, limit=null)",
        ),
        (
            "a marker but no scope",
            json!({ "NextMarker": "omission-acl" }),
            "ListWebACLsRequest(scope=null, nextMarker=omission-acl, limit=null)",
        ),
        (
            "a limit but no scope",
            json!({ "Limit": 7 }),
            "ListWebACLsRequest(scope=null, nextMarker=null, limit=7)",
        ),
        (
            "a marker and a limit but no scope",
            json!({ "NextMarker": "omission-acl", "Limit": 100 }),
            "ListWebACLsRequest(scope=null, nextMarker=omission-acl, limit=100)",
        ),
    ];
    for (case, request, rendering) in listings {
        let body = waf
            .refused("ListWebACLs", request, ACCESS_DENIED, case)
            .await;
        assert_eq!(
            error_message(&body),
            format!("Critical information is missing in your request: {rendering}."),
            "case {case:?}: the refusal must render the request shape, got {body}"
        );
    }

    // The complete forms of both operations are served, so what the rows above changed
    // is the omission and nothing else.
    let listed = waf
        .ok("ListWebACLs", json!({ "Scope": SCOPE_REGIONAL }))
        .await;
    assert_eq!(
        listed["WebACLs"].as_array().map(Vec::len),
        Some(1),
        "the complete listing must still be served, got {listed}"
    );
}

/// Rule 3: the two refusals an undeclared `Scope` gets, and which value gets which.
///
/// `GLOBAL` is the global namespace's own name — the segment every CLOUDFRONT-scoped
/// resource's ARN carries — and WAF reads it as a scope, so it is refused as a bad WAF
/// PARAMETER with the offending value echoed. A token WAF's scope vocabulary does not
/// carry at all is the request validator's value-set clause instead. The rows span both
/// classes across the three web ACL operations that take a `Scope` and judge it
/// themselves.
#[tokio::test]
async fn an_undeclared_scope_is_refused_as_a_parameter_or_as_a_value_set_violation() {
    let waf = Waf::new();
    let (acl_id, _acl_arn, acl_lock) =
        create_web_acl(&waf, SCOPE_REGIONAL, "scope-acl", json!([])).await;
    let update_body = |scope: &str| {
        json!({
            "Name": "scope-acl",
            "Scope": scope,
            "Id": &acl_id,
            "DefaultAction": { "Allow": {} },
            "VisibilityConfig": visibility("scope-acl"),
            "LockToken": &acl_lock,
        })
    };
    let requests = |scope: &str| -> Vec<(&'static str, Value)> {
        vec![
            ("ListWebACLs", json!({ "Scope": scope })),
            (
                "GetWebACL",
                json!({ "Name": "scope-acl", "Scope": scope, "Id": &acl_id }),
            ),
            ("UpdateWebACL", update_body(scope)),
        ]
    };

    // The namespace's own name: WAF's bad-parameter refusal, naming the value.
    for (operation, request) in requests("GLOBAL") {
        let case = format!("{operation} with the GLOBAL namespace token");
        let (status, body) = waf.call_from(TEST_REGION, operation, request).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected the scope refusal, got status {status} and body {body}"
        );
        assert_eq!(
            body["__type"].as_str().unwrap_or_default(),
            "WAFInvalidParameterException",
            "case {case:?}: expected WAF's own parameter refusal, got {body}"
        );
        assert_invalid_parameter(&body, FIELD_SCOPE_VALUE, "GLOBAL", SCOPE_NOT_VALID, &case);
    }

    // Tokens WAF's scope vocabulary does not carry: the value-set clause, naming the
    // member and the two values the enum declares.
    for token in ["regional", "cloudfront", "", "CLOUDFRONT REGIONAL", "Global"] {
        for (operation, request) in requests(token) {
            let case = format!("{operation} with the scope token {token:?}");
            let (status, body) = waf.call_from(TEST_REGION, operation, request).await;
            assert_eq!(
                status, 400,
                "case {case:?}: expected the validator's refusal, got status {status} \
                 and body {body}"
            );
            assert_eq!(
                body["__type"].as_str().unwrap_or_default(),
                VALIDATION_EXCEPTION,
                "case {case:?}: a token outside WAF's scope vocabulary is the request \
                 validator's own refusal, got {body}"
            );
            assert_eq!(
                error_message(&body),
                format!(
                    "1 validation error detected: Value '{token}' at 'scope' failed to \
                     satisfy constraint: Member must satisfy enum value set: \
                     [{SCOPE_CLOUDFRONT}, {SCOPE_REGIONAL}]"
                ),
                "case {case:?}: the clause must name the member and the value set, got {body}"
            );
        }
    }
}

/// Rule 4: how the request validator states a member-constraint failure — the pattern
/// clause first, and the pattern as its full-match expression.
///
/// The rows span the constraints `ListWebACLs$NextMarker` carries
/// (`smithy_query(query="operation",operation="ListWebACLs")`: `@length(1..256)` and
/// `@pattern \S`): a value that breaks both, one that breaks the pattern alone, one that
/// breaks the length ceiling alone, and the bounds themselves, which are served.
#[tokio::test]
async fn a_constraint_failure_states_the_pattern_clause_first_and_in_full_match_form() {
    let waf = Waf::new();
    create_web_acl(&waf, SCOPE_REGIONAL, "clause-acl", json!([])).await;

    // `NextMarker`'s own clauses, as the validator words them for this member.
    let pattern_clause = "Value '{v}' at 'nextMarker' failed to satisfy constraint: \
                          Member must satisfy regular expression pattern: .*\\S.*";
    let min_length_clause = "Value '{v}' at 'nextMarker' failed to satisfy constraint: \
                             Member must have length greater than or equal to 1";
    let max_length_clause = "Value '{v}' at 'nextMarker' failed to satisfy constraint: \
                             Member must have length less than or equal to 256";
    let clause = |template: &str, value: &str| template.replace("{v}", value);

    let over_limit = "x".repeat(257);
    // A clause echoes at most 256 bytes of the rejected value and says how long it was,
    // so the over-ceiling row's value token is that elision rather than the whole value.
    let echoed_over_limit = format!("{}... [{} bytes]", "x".repeat(256), over_limit.len());
    let rows: Vec<(&str, String, String)> = vec![
        (
            "an empty marker, which breaks the pattern and the length floor",
            String::new(),
            format!(
                "2 validation errors detected: {}; {}",
                clause(pattern_clause, ""),
                clause(min_length_clause, "")
            ),
        ),
        (
            "a whitespace-only marker, which breaks the pattern alone",
            " ".to_string(),
            format!("1 validation error detected: {}", clause(pattern_clause, " ")),
        ),
        (
            "a marker one past the length ceiling",
            over_limit.clone(),
            format!(
                "1 validation error detected: {}",
                clause(max_length_clause, &echoed_over_limit)
            ),
        ),
    ];
    for (case, marker, expected) in rows {
        let body = waf
            .refused(
                "ListWebACLs",
                json!({ "Scope": SCOPE_REGIONAL, "NextMarker": marker }),
                VALIDATION_EXCEPTION,
                case,
            )
            .await;
        assert_eq!(
            error_message(&body),
            expected,
            "case {case:?}: expected the validator's envelope in WAF's clause order, got {body}"
        );
    }

    // A marker on each published length bound is served, so the rows above are refused
    // for the constraint each names and not for carrying a marker at all.
    for (case, marker) in [
        ("a marker on the length floor", "x".to_string()),
        ("a marker on the length ceiling", "x".repeat(256)),
    ] {
        let page = waf
            .ok(
                "ListWebACLs",
                json!({ "Scope": SCOPE_REGIONAL, "NextMarker": marker }),
            )
            .await;
        assert!(
            page["WebACLs"].is_array(),
            "case {case:?}: a marker within the bounds must be served, got {page}"
        );
    }
}
