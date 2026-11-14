// SPDX-License-Identifier: CC0-1.0

//! How WAFv2 refuses a bad PARAMETER, end to end over the wire.
//!
//! Three properties, each asserted over a table that spans the class rather than
//! the one request that exposed it:
//!
//! 1. A refusal WAF words itself is the modelled `WAFInvalidParameterException`
//!    with its `Field`, `Parameter` and `Reason` members filled, and a `message`
//!    composed from exactly those three — never an unmodelled error shape with the
//!    structured members absent.
//! 2. WAF judges the resource ARNs a request CARRIES before it judges the syntax
//!    of the request's other members, so an ARN WAF will not accept at all is
//!    reported even when another member also violates its modelled constraint.
//! 3. A collection whose ELEMENTS fail the element shape's constraints is reported
//!    against the collection member's own path, with the value rendered as the
//!    whole list and every constraint of the element shape in one bracketed set.
//!
//! Every call goes through the same front door a real client uses (see the `Waf`
//! harness in `web_acl_lifecycle`), so what these tests observe is what an SDK
//! observes.

use serde_json::{json, Value};

use crate::web_acl_lifecycle::{create_ip_set, create_web_acl, rule, visibility, Waf};

/// The `ErrorReason` WAF carries for an ARN it will not accept, as recorded.
/// Spelled out here independently of the service's own constant, so this suite
/// pins the text and not merely its reuse.
const ARN_NOT_VALID: &str = "The ARN isn't valid. A valid ARN begins with arn: and includes \
     other information separated by colons or slashes.";

/// `TokenDomain`'s constraints as WAF lists them in one bracketed set
/// (`smithy_query(query="shape",shape="TokenDomain")`: `@length(1..253)` and
/// `@pattern(^[\w\.\-/]+$)`), in the order WAF lists them.
const TOKEN_DOMAIN_CONSTRAINTS: &str = "Member must have length less than or equal to 253, \
     Member must have length greater than or equal to 1, \
     Member must satisfy regular expression pattern: ^[\\w\\.\\-/]+$";

/// A `LockToken`-shaped token (`^[0-9a-f]{8}-(?:[0-9a-f]{4}-){3}[0-9a-f]{12}$`).
const WELL_FORMED_LOCK: &str = "11111111-2222-3333-4444-555555555555";

/// An `EntityId`-shaped id; the same pattern as `LockToken`.
const ENTITY_ID: &str = "00000000-0000-0000-0000-000000000000";

/// The `message` member of an awsJson error body, whichever casing the shape
/// models it with.
///
/// Shared with the sibling suites rather than copied into each one.
pub(crate) fn error_message(body: &Value) -> String {
    body.get("Message")
        .or_else(|| body.get("message"))
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

/// The awsJson error code of a refusal, from the body's `__type`.
///
/// Shared with the sibling suites rather than copied into each one. Compared
/// exactly as the body carries it, so a code stays pinned to the shape name this
/// service answers with.
pub(crate) fn error_code(body: &Value) -> String {
    body["__type"].as_str().unwrap_or_default().to_string()
}

/// A `PutManagedRuleSetVersions` request publishing ONE version whose
/// `AssociatedRuleGroupArn` is `arn`, under `lock_token`.
fn put_versions(arn: Option<&str>, lock_token: &str) -> Value {
    let mut request = json!({
        "Name": "vendor-managed-set",
        "Scope": "REGIONAL",
        "Id": ENTITY_ID,
        "LockToken": lock_token,
        "RecommendedVersion": "Version_1.0",
    });
    if let Some(arn) = arn {
        request["VersionsToPublish"] = json!({
            "Version_1.0": { "AssociatedRuleGroupArn": arn, "ForecastedLifetime": 90 },
        });
    }
    request
}

/// One refusal WAF words itself: the call, and the three structured members the
/// answer must carry.
struct Case {
    case: &'static str,
    operation: &'static str,
    request: Value,
    field: &'static str,
    parameter: String,
}

/// Every refusal WAF words itself answers `WAFInvalidParameterException` with
/// `Field`, `Parameter` and `Reason` filled, and a `message` composed from exactly
/// those three.
///
/// One row per (operation, `ParameterExceptionField`) pair the implemented slice
/// can reach, the rows differing in which operation is called, which member is at
/// fault, which `ParameterExceptionField` names it and what kind of value was
/// rejected (an ARN, a CIDR block, a rule tree, an addressing form) — so a body
/// built for one row would show up as a difference from the others. The composed
/// message is checked against the row's OWN `Field`/`Parameter`/`Reason`, so it is
/// the composition rule that is asserted, not a sentence.
#[tokio::test]
async fn every_invalid_parameter_refusal_carries_field_parameter_and_reason() {
    let waf = Waf::new();
    let (_ip_set_id, ip_set_arn, _ip_set_lock) =
        create_ip_set(&waf, "REGIONAL", "probe-reference-ips").await;
    // A CLOUDFRONT-scoped web ACL, for the WEB_ACL row: such a web ACL is put in front
    // of a distribution through the distribution's own configuration, so associating it
    // through this API is the refusal that names WEB_ACL.
    let (_global_id, global_acl_arn, _global_lock) =
        create_web_acl(&waf, "CLOUDFRONT", "probe-global-acl", json!([])).await;

    // A rule tree one level past the published nesting maximum, for the STATEMENT
    // row: ten `NotStatement`s wrapping a leaf is eleven levels.
    let mut statement = json!({
        "SqliMatchStatement": {
            "FieldToMatch": { "QueryString": {} },
            "TextTransformations": [{ "Priority": 0, "Type": "NONE" }],
        }
    });
    for _ in 0..10 {
        statement = json!({ "NotStatement": { "Statement": statement } });
    }

    let malformed_arn = "arn:aws:s3:::a-bucket-named-like-this";
    // A wafv2 ARN whose first resource segment is neither `global` nor `regional`,
    // so it is not a wafv2 resource ARN at all.
    let unknown_scope_arn = "arn:aws:wafv2:us-east-1:000000000000:worldwide/rulegroup/\
                             absent/00000000-0000-0000-0000-000000000000";

    let cases = vec![
        Case {
            case: "PutManagedRuleSetVersions, a reference with an unknown scope segment",
            operation: "PutManagedRuleSetVersions",
            request: put_versions(Some(unknown_scope_arn), WELL_FORMED_LOCK),
            field: "RESOURCE_ARN",
            parameter: unknown_scope_arn.to_string(),
        },
        Case {
            case: "PutManagedRuleSetVersions, another service's ARN as the reference",
            operation: "PutManagedRuleSetVersions",
            request: put_versions(Some(malformed_arn), WELL_FORMED_LOCK),
            field: "RESOURCE_ARN",
            parameter: malformed_arn.to_string(),
        },
        Case {
            case: "PutManagedRuleSetVersions, a live IP set's ARN as the reference",
            operation: "PutManagedRuleSetVersions",
            request: put_versions(Some(ip_set_arn.as_str()), WELL_FORMED_LOCK),
            field: "RESOURCE_ARN",
            parameter: ip_set_arn.clone(),
        },
        Case {
            case: "GetWebACL, an ARN that is not a wafv2 resource ARN",
            operation: "GetWebACL",
            request: json!({ "ARN": malformed_arn }),
            field: "RESOURCE_ARN",
            parameter: malformed_arn.to_string(),
        },
        // A request that names no identifier at all is NOT an invalid parameter: WAF
        // reports the omission against the request itself, under its own code and
        // sentence, which `crate::web_acl_identifier_refusals` asserts.
        Case {
            case: "AssociateWebACL, a CLOUDFRONT-scoped web ACL",
            operation: "AssociateWebACL",
            request: json!({
                "WebACLArn": &global_acl_arn,
                "ResourceArn": "arn:aws:elasticloadbalancing:us-west-2:000000000000:\
                                loadbalancer/app/probe-lb/1234567890abcdef",
            }),
            field: "WEB_ACL",
            parameter: global_acl_arn.clone(),
        },
        Case {
            case: "CreateIPSet, an address that is not in CIDR notation",
            operation: "CreateIPSet",
            request: json!({
                "Name": "probe-ips-not-cidr",
                "Scope": "REGIONAL",
                "IPAddressVersion": "IPV4",
                "Addresses": ["10.0.0.0"],
            }),
            field: "IP_ADDRESS",
            parameter: "10.0.0.0".to_string(),
        },
        Case {
            case: "CreateIPSet, an address of the other family",
            operation: "CreateIPSet",
            request: json!({
                "Name": "probe-ips-wrong-family",
                "Scope": "REGIONAL",
                "IPAddressVersion": "IPV4",
                "Addresses": ["2001:db8::/32"],
            }),
            field: "IP_ADDRESS",
            parameter: "2001:db8::/32".to_string(),
        },
        Case {
            case: "CreateWebACL, a rule tree past the nesting maximum",
            operation: "CreateWebACL",
            request: json!({
                "Name": "probe-acl-too-deep",
                "Scope": "REGIONAL",
                "DefaultAction": { "Allow": {} },
                "VisibilityConfig": visibility("probe-acl-too-deep"),
                "Rules": [rule("deep", statement)],
            }),
            field: "STATEMENT",
            parameter: "Rules".to_string(),
        },
    ];

    let mut fields_covered: Vec<&str> = Vec::new();
    for case in &cases {
        let (status, body) = waf.call(case.operation, case.request.clone()).await;
        assert_eq!(
            status, 400,
            "case {:?}: expected 400, got status {status} and body {body}",
            case.case
        );
        assert_eq!(
            error_code(&body),
            "WAFInvalidParameterException",
            "case {:?}: expected the modelled WAFInvalidParameterException, got {body}",
            case.case
        );
        let field = body["Field"].as_str().unwrap_or_default().to_string();
        let parameter = body["Parameter"].as_str().unwrap_or_default().to_string();
        let reason = body["Reason"].as_str().unwrap_or_default().to_string();
        assert_eq!(
            field, case.field,
            "case {:?}: expected Field {:?}, got {field:?} in body {body}",
            case.case, case.field
        );
        assert_eq!(
            parameter, case.parameter,
            "case {:?}: expected Parameter {:?}, got {parameter:?} in body {body}",
            case.case, case.parameter
        );
        assert!(
            !reason.is_empty(),
            "case {:?}: Reason must say why the parameter was refused, got {body}",
            case.case
        );
        // The composition rule: the message is built from the body's own three
        // members and names nothing else.
        assert_eq!(
            error_message(&body),
            format!("Error reason: {reason}, field: {field}, parameter: {parameter}"),
            "case {:?}: the message must be composed from Reason, Field and Parameter, \
             got {body}",
            case.case
        );
        fields_covered.push(case.field);
    }

    fields_covered.sort_unstable();
    fields_covered.dedup();
    assert_eq!(
        fields_covered,
        vec!["IP_ADDRESS", "RESOURCE_ARN", "STATEMENT", "WEB_ACL"],
        "the table must span several ParameterExceptionField values, covered \
         {fields_covered:?}"
    );
}

/// An ARN WAF will not accept for a resource reference is refused with the one
/// `Reason` that class carries, whichever way the ARN fails to be one.
///
/// The rows differ in HOW the ARN fails — not an ARN at all, another service's
/// ARN, a wafv2 ARN with an unknown scope segment, too few resource segments, and
/// a well-formed ARN of a resource of the WRONG kind that really exists — so none
/// of them can be passing by the same test as the recorded one. Each is at least
/// the modelled 20 characters and carries no whitespace, so `ResourceArn`'s own
/// boundary constraints are not what refuses it.
///
/// A well-formed `rulegroup` ARN naming a rule group that does not exist is NOT in
/// this table: that is target resolution rather than a bad parameter, and WAF
/// answers it with its nonexistent-item failure (see
/// `crate::managed_rule_sets`).
#[tokio::test]
async fn an_unacceptable_resource_reference_carries_the_arn_reason() {
    let waf = Waf::new();
    let (_ip_set_id, ip_set_arn, _ip_set_lock) =
        create_ip_set(&waf, "REGIONAL", "probe-wrong-kind-ips").await;

    let arns: Vec<(&str, String)> = vec![
        ("not an ARN at all", "rulegroup/absent/1111111111".to_string()),
        (
            "another service's ARN",
            "arn:aws:s3:::a-bucket-named-like-this".to_string(),
        ),
        (
            "an unknown scope segment",
            "arn:aws:wafv2:us-east-1:000000000000:worldwide/rulegroup/absent/1111".to_string(),
        ),
        (
            "too few resource segments",
            "arn:aws:wafv2:us-east-1:000000000000:regional/rulegroup/absent".to_string(),
        ),
        ("a live resource of the wrong kind", ip_set_arn.clone()),
    ];

    for (case, arn) in &arns {
        let (status, body) = waf
            .call(
                "PutManagedRuleSetVersions",
                put_versions(Some(arn.as_str()), WELL_FORMED_LOCK),
            )
            .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected 400, got status {status} and body {body}"
        );
        assert_eq!(
            error_code(&body),
            "WAFInvalidParameterException",
            "case {case:?}: expected WAFInvalidParameterException, got {body}"
        );
        assert_eq!(
            body["Reason"].as_str().unwrap_or_default(),
            ARN_NOT_VALID,
            "case {case:?}: expected the ARN reason, got {body}"
        );
        assert_eq!(
            body["Parameter"].as_str().unwrap_or_default(),
            arn.as_str(),
            "case {case:?}: Parameter must echo the rejected ARN, got {body}"
        );
    }
}

/// WAF judges the resource ARNs a request CARRIES before it judges the syntax of
/// the request's other members: with both wrong, the ARN is what is reported.
///
/// The table crosses the reference (an ARN WAF will not accept / absent) with the
/// lock token (malformed in three different ways / well formed), so every
/// combination is pinned — including that a request whose reference is acceptable
/// still has its token checked, and that a request with neither fault reaches the
/// operation itself.
#[tokio::test]
async fn a_resource_reference_is_resolved_before_the_lock_token() {
    let waf = Waf::new();
    // Not a wafv2 resource ARN at all, so it is a bad PARAMETER whatever the store
    // holds.
    let unacceptable_arn = "arn:aws:s3:::a-bucket-named-like-this";

    // Three ways a `LockToken` can fail its modelled constraints: the wrong shape,
    // a hex-looking value of the wrong length, and the empty string (the length
    // floor). Each is the value the clause must echo.
    let bad_tokens = ["stale-token", "1111111", ""];

    for token in bad_tokens {
        // Reference unacceptable AND token bad: the reference wins.
        let (status, body) = waf
            .call(
                "PutManagedRuleSetVersions",
                put_versions(Some(unacceptable_arn), token),
            )
            .await;
        let case = format!("unacceptable reference with LockToken {token:?}");
        assert_eq!(
            status, 400,
            "case {case:?}: expected 400, got status {status} and body {body}"
        );
        assert_eq!(
            error_code(&body),
            "WAFInvalidParameterException",
            "case {case:?}: the unacceptable reference must be what is reported, got {body}"
        );
        assert_eq!(
            body["Field"].as_str().unwrap_or_default(),
            "RESOURCE_ARN",
            "case {case:?}: expected Field RESOURCE_ARN, got {body}"
        );
        assert!(
            !body.to_string().contains(token) || token.is_empty(),
            "case {case:?}: the answer must name the ARN, not the token, got {body}"
        );

        // No reference to resolve: the token's own modelled constraint is reported,
        // in the framework's envelope and naming the token.
        let (status, body) = waf
            .call("PutManagedRuleSetVersions", put_versions(None, token))
            .await;
        let case = format!("no reference with LockToken {token:?}");
        assert_eq!(
            status, 400,
            "case {case:?}: expected 400, got status {status} and body {body}"
        );
        assert_eq!(
            error_code(&body),
            "ValidationException",
            "case {case:?}: a member-constraint failure is the request validator's \
             own refusal, got {body}"
        );
        let message = error_message(&body);
        assert!(
            message.contains("validation error")
                && message.contains("at 'lockToken' failed to satisfy constraint:"),
            "case {case:?}: the clause must name lockToken, got {message:?}"
        );
    }

    // Neither fault: the request reaches the operation, which resolves its target —
    // the managed rule set `(Scope, Name, Id)` names — and refuses a request naming
    // one this account does not hold. A validation layer that answered anything else
    // here would be refusing valid input.
    let (status, body) = waf
        .call(
            "PutManagedRuleSetVersions",
            put_versions(None, WELL_FORMED_LOCK),
        )
        .await;
    assert_eq!(
        status, 400,
        "a request with no bad parameter must reach the operation, got status \
         {status} and body {body}"
    );
    assert_eq!(
        error_code(&body),
        "WAFNonexistentItemException",
        "the operation must refuse an identity no managed rule set carries, got {body}"
    );
}

/// One token-domain list to refuse, and the elements the clause must render.
struct Domains {
    case: &'static str,
    domains: Vec<String>,
}

/// A collection whose ELEMENTS fail the element shape's constraints is reported
/// against the COLLECTION member's path, with the value rendered as the whole list
/// and every constraint of the element shape in one bracketed set.
///
/// The rows differ along what the rule generalises over: which constraint the
/// element breaks (the pattern, the length floor, the length ceiling), how many
/// elements break one (one, two), and whether the list also holds legal elements —
/// which is the row that pins "the value is the LIST", not the offending element.
#[tokio::test]
async fn token_domain_failures_are_reported_against_the_list_member() {
    let waf = Waf::new();

    let cases = vec![
        Domains {
            case: "one domain containing spaces",
            domains: vec!["not a valid domain".to_string()],
        },
        Domains {
            case: "one domain containing a character the pattern excludes",
            domains: vec!["abc#com".to_string()],
        },
        Domains {
            case: "an empty domain, at the length floor",
            domains: vec![String::new()],
        },
        Domains {
            case: "a domain one character past the length ceiling",
            domains: vec!["d".repeat(254)],
        },
        Domains {
            case: "two domains, each breaking a different constraint",
            domains: vec!["not a valid domain".to_string(), String::new()],
        },
        Domains {
            case: "a legal domain beside an illegal one",
            domains: vec!["store.abc.com".to_string(), "not a valid domain".to_string()],
        },
        Domains {
            case: "a legal domain either side of an illegal one",
            domains: vec![
                "abc.com".to_string(),
                "bad domain".to_string(),
                "store.abc.com".to_string(),
            ],
        },
    ];

    for case in &cases {
        let (status, body) = waf
            .call(
                "CreateAPIKey",
                json!({ "Scope": "REGIONAL", "TokenDomains": case.domains }),
            )
            .await;
        assert_eq!(
            status, 400,
            "case {:?}: expected 400, got status {status} and body {body}",
            case.case
        );
        assert_eq!(
            error_code(&body),
            "ValidationException",
            "case {:?}: expected the request validator's own refusal, got {body}",
            case.case
        );
        let expected = format!(
            "1 validation error detected: Value '[{}]' at 'tokenDomains' failed to \
             satisfy constraint: Member must satisfy constraint: [{TOKEN_DOMAIN_CONSTRAINTS}]",
            case.domains.join(", ")
        );
        assert_eq!(
            error_message(&body),
            expected,
            "case {:?}: expected {expected:?}, got {:?}",
            case.case,
            error_message(&body)
        );
    }

    // The collection's OWN element-count bound is a different constraint about a
    // different value, so it keeps its own clause — and still renders the list.
    let (status, body) = waf
        .call(
            "CreateAPIKey",
            json!({ "Scope": "REGIONAL", "TokenDomains": Vec::<String>::new() }),
        )
        .await;
    assert_eq!(
        status, 400,
        "an empty TokenDomains must be refused, got status {status} and body {body}"
    );
    assert_eq!(
        error_message(&body),
        "1 validation error detected: Value '[]' at 'tokenDomains' failed to satisfy \
         constraint: Member must have length greater than or equal to 1",
        "the element-count bound keeps its own clause, got {body}"
    );

    // The clause echoes the whole list, so a large list must not turn a small
    // request into a large response: the echo is capped the way the framework caps
    // every echoed value.
    let many: Vec<String> = (0..200).map(|index| format!("bad domain {index}")).collect();
    let (status, body) = waf
        .call(
            "CreateAPIKey",
            json!({ "Scope": "REGIONAL", "TokenDomains": many }),
        )
        .await;
    assert_eq!(
        status, 400,
        "a list of illegal domains must be refused, got status {status} and body {body}"
    );
    let message = error_message(&body);
    assert!(
        message.len() < 1_000,
        "a long list must not yield a long clause, got {} bytes: {message:?}",
        message.len()
    );
    assert!(
        message.contains("... ["),
        "the elision must report the real size, got {message:?}"
    );

    // A list every element of which is legal reaches the operation, which mints a
    // key for it: the layer refuses bad input, it does not refuse input.
    for domains in [
        vec!["abc.com".to_string()],
        vec!["store.abc.com".to_string(), "a".to_string()],
        vec!["d".repeat(253)],
    ] {
        let (status, body) = waf
            .call(
                "CreateAPIKey",
                json!({ "Scope": "REGIONAL", "TokenDomains": domains }),
            )
            .await;
        assert_eq!(
            status, 200,
            "case {domains:?}: a legal TokenDomains must reach the operation, got \
             status {status} and body {body}"
        );
        assert!(
            body["APIKey"].as_str().is_some_and(|key| !key.is_empty()),
            "case {domains:?}: the operation must answer the key it minted, got {body}"
        );
    }
}
