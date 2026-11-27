// SPDX-License-Identifier: CC0-1.0

//! The sentence this service refuses a value an enum-constrained member does not admit
//! with, end to end over HTTP.
//!
//! The rule: a constraint-violation message lists the admitted values of the ENUM the
//! member targets in the order this API publishes that enum's values — a property of the
//! enum shape, not of the request that broke it — inside the fixed template "`N` validation
//! error[s] detected: Value '<value>' at '<member>' failed to satisfy constraint: Member
//! must satisfy enum value set: [<values>]", under HTTP 400 and `MalformedXML`. The set the
//! sentence states is the set the request boundary enforces: every value it lists is admitted,
//! and every value outside it is refused.
//!
//! The rows span what the rule generalises over:
//!
//! * the ENUM, which is what the order belongs to: `FunctionStage`, `CachePolicyType`,
//!   `OriginAccessControlSigningBehaviors` and `OriginAccessControlOriginTypes`, whose
//!   published orders are not the orders the schema declares, and `IpAddressType` and
//!   `OriginAccessControlSigningProtocols`, which this API publishes in the order it declares
//!   them;
//! * the OPERATION and the BINDING the value travelled in — a member of a request document,
//!   a member nested inside a payload, a query parameter on a collection route and one
//!   beside a path label — which say nothing about the value set, so the same enum is listed
//!   alike in each and all of them answer the one code;
//! * the value set's SIZE (two, three, four and five values);
//! * the form the inadmissible value takes (another word, an admitted value in the wrong
//!   case, an admitted value with trailing text, the empty value), which the sentence
//!   echoes but which never changes the set.

use smithy_server_runtime::MultiServiceRouter;

use crate::distribution_lifecycle::{call, emulator, Answer, API_VERSION};

/// The code this service answers a request whose published schema it cannot read with; a
/// value outside an enum's value set is one of those, in whatever binding it travelled.
const MALFORMED_XML: &str = "MalformedXML";

/// How the enums this service publishes in an order of their own are listed. `FunctionStage`
/// admits DEVELOPMENT and LIVE (`smithy_query(query="shape",shape="FunctionStage")`) and is
/// listed with the published stage first.
const FUNCTION_STAGE_SET: &str = "LIVE, DEVELOPMENT";

/// `CachePolicyType` admits managed and custom, in that order
/// (`smithy_query(query="shape",shape="CachePolicyType")`), and is listed with the
/// caller-owned kind first.
const CACHE_POLICY_TYPE_SET: &str = "custom, managed";

/// `IpAddressType` admits ipv4, ipv6 and dualstack, in that order
/// (`smithy_query(query="shape",shape="IpAddressType")`).
const IP_ADDRESS_TYPE_SET: &str = "ipv4, ipv6, dualstack";

/// `OriginAccessControlSigningProtocols` admits sigv4 and sigv4a, in that order
/// (`smithy_query(query="shape",shape="OriginAccessControlSigningProtocols")`).
const SIGNING_PROTOCOL_SET: &str = "sigv4, sigv4a";

/// `OriginAccessControlSigningBehaviors` admits never, always, no-override and
/// always-amz-auth, in that order
/// (`smithy_query(query="shape",shape="OriginAccessControlSigningBehaviors")`), and is
/// listed with the behaviour that signs and overrides an existing `Authorization` header
/// first.
const SIGNING_BEHAVIOR_SET: &str = "always-amz-auth, never, always, no-override";

/// `OriginAccessControlOriginTypes` admits s3, mediastore, mediapackagev2 and
/// lambda, in that order
/// (`smithy_query(query="shape",shape="OriginAccessControlOriginTypes")`) — each value is the
/// signing name of the service the origin belongs to — and is listed with the object store
/// first and then the compute and media origins.
const ORIGIN_TYPE_SET: &str = "s3, lambda, mediastore, mediapackagev2";

/// One case: an operation, the binding its enum-constrained member travels in, and what the
/// refusal must say about it.
struct Case {
    /// What this row is, for the assertion messages.
    case: &'static str,
    /// One value the enum under test DOES admit, so the inadmissible values are built from
    /// the value set itself rather than from a literal.
    admitted: &'static str,
    /// The member as the refusal names it on the wire — the path the value travelled, so a
    /// member nested inside a payload is named under the payload it is nested in.
    member: &'static str,
    /// The enum's admitted values, in the order this API publishes them.
    values: &'static str,
    /// How the case's value reaches the service.
    binding: Binding,
}

/// Where the value under test travels: each is a different place the request carries it, and
/// none of them is a property of the enum.
enum Binding {
    /// `ListConnectionFunctions`: a member of the request document.
    ListConnectionFunctionsStage,
    /// `ListFunctions`: a query parameter of a collection route.
    ListFunctionsStage,
    /// `DescribeConnectionFunction`: a query parameter beside a path label.
    DescribeConnectionFunctionStage,
    /// `ListCachePolicies`: a query parameter naming a kind to filter on.
    ListCachePoliciesType,
    /// `CreateAnycastIpList`: a member of a create document, beside members that are fine.
    CreateAnycastIpListAddressType,
    /// `CreateOriginAccessControl`: a member NESTED inside the request's payload.
    CreateOriginAccessControlSigningProtocol,
    /// `CreateOriginAccessControl`: a second member nested inside that same payload, whose
    /// enum this API publishes in an order of its own.
    CreateOriginAccessControlSigningBehavior,
    /// `CreateOriginAccessControl`: a third member of that payload, whose five-value set this
    /// API publishes in an order of its own.
    CreateOriginAccessControlOriginType,
}

/// Every case the rule is asserted over.
fn cases() -> Vec<Case> {
    vec![
        Case {
            case: "a stage in a request document",
            admitted: "DEVELOPMENT",
            member: "stage",
            values: FUNCTION_STAGE_SET,
            binding: Binding::ListConnectionFunctionsStage,
        },
        Case {
            case: "a stage in a query parameter",
            admitted: "LIVE",
            member: "stage",
            values: FUNCTION_STAGE_SET,
            binding: Binding::ListFunctionsStage,
        },
        Case {
            case: "a stage in a query parameter beside a path label",
            admitted: "DEVELOPMENT",
            member: "stage",
            values: FUNCTION_STAGE_SET,
            binding: Binding::DescribeConnectionFunctionStage,
        },
        Case {
            case: "a policy kind in a query parameter",
            admitted: "managed",
            member: "type",
            values: CACHE_POLICY_TYPE_SET,
            binding: Binding::ListCachePoliciesType,
        },
        Case {
            case: "an address type in a create document",
            admitted: "ipv4",
            member: "ipAddressType",
            values: IP_ADDRESS_TYPE_SET,
            binding: Binding::CreateAnycastIpListAddressType,
        },
        Case {
            case: "a signing protocol nested in a payload",
            admitted: "sigv4",
            member: "originAccessControlConfig.signingProtocol",
            values: SIGNING_PROTOCOL_SET,
            binding: Binding::CreateOriginAccessControlSigningProtocol,
        },
        Case {
            case: "a signing behaviour nested in a payload",
            admitted: "no-override",
            member: "originAccessControlConfig.signingBehavior",
            values: SIGNING_BEHAVIOR_SET,
            binding: Binding::CreateOriginAccessControlSigningBehavior,
        },
        Case {
            case: "an origin type nested in a payload",
            admitted: "s3",
            member: "originAccessControlConfig.originAccessControlOriginType",
            values: ORIGIN_TYPE_SET,
            binding: Binding::CreateOriginAccessControlOriginType,
        },
    ]
}

/// The forms a value outside the enum takes, each built from a value the enum DOES admit so
/// the rows come from the value set rather than from a literal.
fn inadmissible(admitted: &str) -> Vec<(&'static str, String)> {
    // An admitted value this API spells in capitals is inadmissible in lower case, and one it
    // spells in lower case is inadmissible in capitals.
    let wrong_case = if admitted.chars().any(char::is_uppercase) {
        admitted.to_lowercase()
    } else {
        admitted.to_uppercase()
    };
    vec![
        ("another word", format!("{admitted}-bogus")),
        ("an admitted value in the wrong case", wrong_case),
        ("an admitted value with trailing text", format!("{admitted}X")),
        ("the empty value", String::new()),
    ]
}

/// Call the operation of `binding`, carrying `value` where that binding carries it.
async fn refuse(router: &MultiServiceRouter, binding: &Binding, value: &str) -> Answer {
    match binding {
        Binding::ListConnectionFunctionsStage => {
            let body = format!(
                "<ListConnectionFunctionsRequest \
                 xmlns=\"http://cloudfront.amazonaws.com/doc/{API_VERSION}/\">\
                 <Stage>{value}</Stage></ListConnectionFunctionsRequest>"
            );
            call(
                router,
                "POST",
                &format!("/{API_VERSION}/connection-functions"),
                Some(&body),
                None,
            )
            .await
        }
        Binding::ListFunctionsStage => {
            call(
                router,
                "GET",
                &format!("/{API_VERSION}/function?Stage={value}"),
                None,
                None,
            )
            .await
        }
        Binding::DescribeConnectionFunctionStage => {
            call(
                router,
                "GET",
                &format!(
                    "/{API_VERSION}/connection-function/cf_AAAAAAAAAAAAA/describe?Stage={value}"
                ),
                None,
                None,
            )
            .await
        }
        Binding::ListCachePoliciesType => {
            call(
                router,
                "GET",
                &format!("/{API_VERSION}/cache-policy?Type={value}"),
                None,
                None,
            )
            .await
        }
        Binding::CreateAnycastIpListAddressType => {
            let body = format!(
                "<CreateAnycastIpListRequest \
                 xmlns=\"http://cloudfront.amazonaws.com/doc/{API_VERSION}/\">\
                 <Name>enum-value-set-message</Name><IpCount>3</IpCount>\
                 <IpAddressType>{value}</IpAddressType></CreateAnycastIpListRequest>"
            );
            call(
                router,
                "POST",
                &format!("/{API_VERSION}/anycast-ip-list"),
                Some(&body),
                None,
            )
            .await
        }
        Binding::CreateOriginAccessControlSigningProtocol => {
            let body = format!(
                "<OriginAccessControlConfig \
                 xmlns=\"http://cloudfront.amazonaws.com/doc/{API_VERSION}/\">\
                 <Name>enum-value-set-message</Name><SigningProtocol>{value}</SigningProtocol>\
                 <SigningBehavior>always</SigningBehavior><OriginAccessControlOriginType>s3\
                 </OriginAccessControlOriginType></OriginAccessControlConfig>"
            );
            call(
                router,
                "POST",
                &format!("/{API_VERSION}/origin-access-control"),
                Some(&body),
                None,
            )
            .await
        }
        Binding::CreateOriginAccessControlSigningBehavior => {
            let body = format!(
                "<OriginAccessControlConfig \
                 xmlns=\"http://cloudfront.amazonaws.com/doc/{API_VERSION}/\">\
                 <Name>enum-value-set-behavior</Name><SigningProtocol>sigv4</SigningProtocol>\
                 <SigningBehavior>{value}</SigningBehavior><OriginAccessControlOriginType>s3\
                 </OriginAccessControlOriginType></OriginAccessControlConfig>"
            );
            call(
                router,
                "POST",
                &format!("/{API_VERSION}/origin-access-control"),
                Some(&body),
                None,
            )
            .await
        }
        Binding::CreateOriginAccessControlOriginType => {
            let body = format!(
                "<OriginAccessControlConfig \
                 xmlns=\"http://cloudfront.amazonaws.com/doc/{API_VERSION}/\">\
                 <Name>enum-value-set-origin-type</Name>\
                 <SigningProtocol>sigv4</SigningProtocol>\
                 <SigningBehavior>always</SigningBehavior>\
                 <OriginAccessControlOriginType>{value}</OriginAccessControlOriginType>\
                 </OriginAccessControlConfig>"
            );
            call(
                router,
                "POST",
                &format!("/{API_VERSION}/origin-access-control"),
                Some(&body),
                None,
            )
            .await
        }
    }
}

/// The sentence this service refuses one inadmissible value with: the template's fixed text,
/// the value and member the request carried, and the enum's value set.
fn expected_message(value: &str, member: &str, values: &str) -> String {
    format!(
        "1 validation error detected: Value '{value}' at '{member}' failed to satisfy \
         constraint: Member must satisfy enum value set: [{values}]"
    )
}

/// A value an enum-constrained member does not admit is refused 400 `MalformedXML` under the
/// parameter-validation sentence, which lists the enum's value set in the order this API
/// publishes it — in every operation and in whatever binding carried the value.
#[tokio::test]
async fn an_enum_refusal_lists_the_value_set_in_the_published_order() {
    let router = emulator();

    for case in cases() {
        for (form, value) in inadmissible(case.admitted) {
            let answer = refuse(&router, &case.binding, &value).await;
            let label = format!("{} / {form}", case.case);
            assert_eq!(
                answer.status, 400,
                "case {label:?} ({value:?}): expected HTTP 400, got {} with {}",
                answer.status, answer.raw
            );
            assert_eq!(
                answer.error_code(),
                MALFORMED_XML,
                "case {label:?} ({value:?}): expected {MALFORMED_XML}, got {:?} in {}",
                answer.error_code(),
                answer.raw
            );
            let actual = answer
                .root()
                .find_text("Message")
                .unwrap_or_default()
                .to_string();
            let expected = expected_message(&value, case.member, case.values);
            assert_eq!(
                actual, expected,
                "case {label:?} ({value:?}): expected the message {expected:?}, got {actual:?}"
            );
        }
    }
}

/// The control for the rule: a value each enum DOES admit is read rather than refused, so
/// every refusal above is about the value and not about the member or the route.
#[tokio::test]
async fn a_value_each_enum_admits_is_not_refused_over_its_value_set() {
    let router = emulator();

    for case in cases() {
        let answer = refuse(&router, &case.binding, case.admitted).await;
        let message = answer
            .root()
            .find_text("Message")
            .unwrap_or_default()
            .to_string();
        assert!(
            !message.contains("Member must satisfy enum value set"),
            "case {:?}: an admitted value ({:?}) must not be refused over the value set, got \
             {} with {message:?}",
            case.case,
            case.admitted,
            answer.status
        );
    }
}

/// The set a refusal STATES is the set the request boundary ENFORCES: every value the sentence
/// lists is admitted for the member it was stated about, so a caller that reads the refusal and
/// retries with one of the values it names is served rather than refused again.
///
/// Driven over the origin type — this suite's five-value set, and the one whose values name
/// whole services — with the rows read off the stated set itself rather than written out a
/// second time, and each create carrying a name of its own so nothing but the value under test
/// decides the answer.
#[tokio::test]
async fn every_value_the_stated_set_lists_is_admitted_for_its_member() {
    let router = emulator();

    for (index, origin_type) in ORIGIN_TYPE_SET.split(", ").enumerate() {
        let body = format!(
            "<OriginAccessControlConfig \
             xmlns=\"http://cloudfront.amazonaws.com/doc/{API_VERSION}/\">\
             <Name>stated-set-{index}</Name><SigningProtocol>sigv4</SigningProtocol>\
             <SigningBehavior>always</SigningBehavior>\
             <OriginAccessControlOriginType>{origin_type}</OriginAccessControlOriginType>\
             </OriginAccessControlConfig>"
        );
        let answer = call(
            &router,
            "POST",
            &format!("/{API_VERSION}/origin-access-control"),
            Some(&body),
            None,
        )
        .await;
        assert_eq!(
            answer.status, 201,
            "case {origin_type:?}: a value the stated set lists must be admitted, got {} with {}",
            answer.status, answer.raw
        );
        let reported = answer
            .root()
            .find_text("OriginAccessControlOriginType")
            .unwrap_or_default();
        assert_eq!(
            reported, origin_type,
            "case {origin_type:?}: expected the stored origin type to be reported back, got \
             {reported:?} in {}",
            answer.raw
        );
    }
}
