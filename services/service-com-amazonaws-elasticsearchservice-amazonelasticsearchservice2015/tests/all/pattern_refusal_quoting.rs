// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for how this API QUOTES a `@pattern` requirement when it refuses a
//! value at the boundary.
//!
//! The rule: a pattern clause states the expression the service PUBLISHES for the member's
//! shape — the `Pattern` the API reference documents for it. For most of this surface that
//! is the expression WITHOUT the whole-value pair the model wraps it in: a model writes
//! `^…$` to demand a total match, and the service publishes what is inside it. The check is
//! unchanged either way: the boundary still runs the model's pattern as written, so the same
//! values are refused; only the clause's quoting differs, and a caller that lifts the
//! published expression out of the message gets one it can apply itself.
//!
//! The pair is kept in two cases. One is textual: an expression that anchors its own
//! BRANCHES (an alternation of `^…$` branches) is a whole-value expression already — the two
//! ends of its text are the first and last branch's anchors — so it is quoted exactly as the
//! model declares it, since dropping those ends would state a requirement of no branch. The
//! version row below is that case. The other is the shape's own published text: a VPC
//! endpoint id is documented `^aos-[a-zA-Z0-9]*$`, anchors included, and the two rows for it
//! assert that — the published expression is reference data about the shape rather than
//! something recoverable from the model's text, since both renderings are spelled `^…$`
//! there. The collection-member half of the same rule is
//! [`super::vpc_endpoint_id_expression`].
//!
//! Each row's modelled expression is the model's, read with
//! `smithy_query(query="shape",shape="DomainName,PackageName,ElasticsearchVersionString,\
//! UserPoolId,IdentityPoolId,AWSAccount,VpcEndpointId")`, and the rows span what the rule
//! generalises over: the member's shape, where the value is bound (a URI label, a top-level
//! body member, a member nested in a structure), and the HTTP method of the route.

use serde_json::{json, Value};

use crate::harness::{call_in, router};

/// The Region every call is signed for.
const REGION: &str = "us-east-1";

/// The clause text the requirement is stated in, which this rule only changes the tail of.
const PATTERN_CLAUSE: &str = "failed to satisfy constraint: Member must satisfy regular \
                              expression pattern: ";

/// One row: a request carrying a value its member's `@pattern` does not admit.
struct Case {
    /// What the row is about, named in every assertion.
    case: &'static str,
    method: &'static str,
    path: String,
    body: Option<Value>,
    /// The `@pattern` the model declares for the member.
    modelled: &'static str,
    /// The expression the clause must quote: `modelled` without the whole-value pair.
    quoted: &'static str,
}

/// The `@pattern` of `DomainName` and of `PackageName` (both
/// `smithy_query(query="shape",shape="DomainName,PackageName")`).
const NAME_PATTERN: &str = r"^[a-z][a-z0-9\-]+$";

/// That expression without the pair the model wraps it in.
const NAME_QUOTED: &str = r"[a-z][a-z0-9\-]+";

/// The `@pattern` of `ElasticsearchVersionString`: an alternation each of whose branches
/// is anchored on its own, so nothing is dropped and the expression is published as the
/// model declares it.
const VERSION_PATTERN: &str =
    r"^[0-9]{1}\.[0-9]{1,2}$|^OpenSearch_[0-9]{1,2}\.[0-9]{1,2}$|^OS_[0-9]{1,2}\.[0-9]{1,2}$";

fn cases() -> Vec<Case> {
    vec![
        Case {
            // A URI label, on the reader the recorded probe called.
            case: "DescribeElasticsearchDomain's DomainName label",
            method: "GET",
            path: "/2015-01-01/es/domain/BADDOMAIN".to_string(),
            body: None,
            modelled: NAME_PATTERN,
            quoted: NAME_QUOTED,
        },
        Case {
            case: "DescribeElasticsearchDomainConfig's DomainName label",
            method: "GET",
            path: "/2015-01-01/es/domain/bad_domain/config".to_string(),
            body: None,
            modelled: NAME_PATTERN,
            quoted: NAME_QUOTED,
        },
        Case {
            case: "DeleteElasticsearchDomain's DomainName label",
            method: "DELETE",
            path: "/2015-01-01/es/domain/Bad-Domain".to_string(),
            body: None,
            modelled: NAME_PATTERN,
            quoted: NAME_QUOTED,
        },
        Case {
            // The same shape bound in the BODY rather than to a label.
            case: "CreateElasticsearchDomain's DomainName member",
            method: "POST",
            path: "/2015-01-01/es/domain".to_string(),
            body: Some(json!({"DomainName": "Bad_Name"})),
            modelled: NAME_PATTERN,
            quoted: NAME_QUOTED,
        },
        Case {
            // Another shape carrying the same expression, on another resource.
            case: "CreatePackage's PackageName member",
            method: "POST",
            path: "/2015-01-01/packages".to_string(),
            body: Some(json!({
                "PackageName": "Bad_Package",
                "PackageType": "TXT-DICTIONARY",
                "PackageSource": {"S3BucketName": "pkg-bucket", "S3Key": "v1.txt"}
            })),
            modelled: NAME_PATTERN,
            quoted: NAME_QUOTED,
        },
        Case {
            // The alternation: each branch anchored on its own, so the expression is
            // already a whole-value one and is quoted as declared.
            case: "CreateElasticsearchDomain's ElasticsearchVersion member",
            method: "POST",
            path: "/2015-01-01/es/domain".to_string(),
            body: Some(json!({
                "DomainName": "version-probe",
                "ElasticsearchVersion": "7.10.1"
            })),
            modelled: VERSION_PATTERN,
            quoted: VERSION_PATTERN,
        },
        Case {
            // A member nested inside a structure member.
            case: "CognitoOptions.UserPoolId, nested in the body",
            method: "POST",
            path: "/2015-01-01/es/domain".to_string(),
            body: Some(json!({
                "DomainName": "cognito-probe",
                "CognitoOptions": {"Enabled": true, "UserPoolId": "bad pool"}
            })),
            modelled: r"^[\w-]+_[0-9a-zA-Z]+$",
            quoted: r"[\w-]+_[0-9a-zA-Z]+",
        },
        Case {
            case: "CognitoOptions.IdentityPoolId, nested in the body",
            method: "POST",
            path: "/2015-01-01/es/domain".to_string(),
            body: Some(json!({
                "DomainName": "identity-probe",
                "CognitoOptions": {"Enabled": true, "IdentityPoolId": "bad pool"}
            })),
            modelled: r"^[\w-]+:[0-9a-f-]+$",
            quoted: r"[\w-]+:[0-9a-f-]+",
        },
        Case {
            // A digits-only expression, on an operation this crate does not implement:
            // the boundary refuses the request before any handler runs, so the clause is
            // the same one.
            case: "AuthorizeVpcEndpointAccess's Account member",
            method: "POST",
            path: "/2015-01-01/es/domain/vpc-probe-domain/authorizeVpcEndpointAccess"
                .to_string(),
            body: Some(json!({"Account": "0123456789ab"})),
            modelled: r"^[0-9]+$",
            quoted: r"[0-9]+",
        },
        Case {
            // A shape the API publishes WITH the pair, bound to a body member.
            case: "UpdateVpcEndpoint's VpcEndpointId member",
            method: "POST",
            path: "/2015-01-01/es/vpcEndpoints/update".to_string(),
            body: Some(json!({
                "VpcEndpointId": "bad-endpoint-id",
                "VpcOptions": {"SubnetIds": ["subnet-0000000000000000a"]}
            })),
            modelled: r"^aos-[a-zA-Z0-9]*$",
            quoted: r"^aos-[a-zA-Z0-9]*$",
        },
        Case {
            // The same shape on a URI label, so the published text is the shape's and not
            // the binding's.
            case: "DeleteVpcEndpoint's VpcEndpointId label",
            method: "DELETE",
            path: "/2015-01-01/es/vpcEndpoints/bad-endpoint-id".to_string(),
            body: None,
            modelled: r"^aos-[a-zA-Z0-9]*$",
            quoted: r"^aos-[a-zA-Z0-9]*$",
        },
    ]
}

/// RULE: a `@pattern` refusal quotes the expression the service publishes for the member's
/// shape — the model's text without the whole-value pair the model wrapped it in, and the
/// model's text unchanged for a shape whose published expression carries that pair — on
/// every member of this API's input surface.
#[tokio::test]
async fn a_pattern_refusal_quotes_the_expression_the_api_publishes() {
    let router = router();

    for Case {
        case,
        method,
        path,
        body,
        modelled,
        quoted,
    } in cases()
    {
        let answer = call_in(&router, REGION, method, &path, body.as_ref()).await;
        assert_eq!(
            answer.status, 400,
            "case {case:?}: a value outside the pattern must be refused with 400, got {} {}",
            answer.status, answer.body
        );
        assert!(
            answer.error_type.contains("ValidationException"),
            "case {case:?}: the error must be ValidationException, got {:?} / {}",
            answer.error_type,
            answer.body
        );
        let message = answer.text(case, "/message");
        assert!(
            message.starts_with("1 validation error detected: "),
            "case {case:?}: the envelope must count the errors it states, got {message:?}"
        );
        let stated = message
            .split_once(PATTERN_CLAUSE)
            .map(|(_, expression)| expression.to_string())
            .unwrap_or_else(|| {
                panic!(
                    "case {case:?}: the clause must state the pattern requirement, got \
                     {message:?}"
                )
            });
        // The expression runs to the end of the clause, and with one validation error the
        // clause ends the message — so this pins the WHOLE expression rather than
        // asserting that the quoted one is somewhere inside it.
        assert_eq!(
            stated, quoted,
            "case {case:?}: the clause must quote {quoted:?} (the model declares \
             {modelled:?}), got {stated:?}"
        );
    }
}

/// The control: the CHECK is the model's pattern as written, so a value the expression
/// admits is still accepted and one it does not is still refused.
#[tokio::test]
async fn the_check_still_runs_the_modelled_pattern() {
    let router = router();

    for (case, name, expected) in [
        ("a lowercase name", "good-domain", 200),
        ("digits after the first letter", "d0main-7", 200),
        ("an uppercase letter", "Good-Domain", 400),
        ("an underscore", "good_domain", 400),
        ("a leading digit", "0domain", 400),
    ] {
        let answer = call_in(
            &router,
            REGION,
            "POST",
            "/2015-01-01/es/domain",
            Some(&json!({"DomainName": name})),
        )
        .await;
        assert_eq!(
            answer.status, expected,
            "case {case:?} ({name:?}): expected HTTP {expected}, got {} {}",
            answer.status, answer.body
        );
    }
}
