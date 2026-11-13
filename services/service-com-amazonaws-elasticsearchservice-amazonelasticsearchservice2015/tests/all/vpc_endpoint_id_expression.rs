// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the EXPRESSION a constraint refusal of this API quotes, and for
//! where a constraint broken by a collection's ELEMENTS is stated.
//!
//! Every case drives the real wire path — a `restJson1` request through the multi-service
//! router into the mounted service — so what is asserted is what a client sees rather than
//! what a renderer returns. The harness is [`crate::harness`]; this suite adds no second
//! copy of it.
//!
//! The rule under test: a `@pattern` clause quotes the expression the API PUBLISHES for the
//! member's shape — the `Pattern` its API reference documents — which for the VPC endpoint
//! id is the model's text as written, anchors included, and for the domain and package
//! names is the model's text without the whole-value pair the model wrapped it in. The
//! expression belongs to the SHAPE, so every member targeting it is quoted alike wherever
//! the value was bound: an element of a list, a body member, a URI label. And a constraint
//! broken by an ELEMENT of a collection is ONE validation error stated at the collection
//! MEMBER (no element position, no `member` step) over the element shape's WHOLE declared
//! constraint set — the constraints the value satisfied as much as the one it broke — with
//! the offending elements quoted as the collection they came from — the collection the
//! REQUEST carried, read from the request rather than rebuilt from the violations.
//!
//! WHY the anchors are part of what is published: the message is the only place AWS states
//! the accepted syntax of an id, so a client that lifts the quoted expression out of it and
//! pre-validates with it has to accept exactly what the service accepts. The last test
//! below asserts that agreement over a table of ids, which is what an unanchored quoting
//! breaks — `vpce-aos-1` merely CONTAINS an `aos-` run.
//!
//! The expressions and bounds each row states are the model's, read with
//! `smithy_query(query="shape",shape="VpcEndpointId,VpcEndpointIdList,DomainName,\
//! DomainNameList")`.

use serde_json::{json, Value};

use crate::harness::{call_in, router};

/// The Region every call is signed for.
const REGION: &str = "us-east-1";

/// The expression a `VpcEndpointId` clause quotes: the `Pattern` AWS documents for a VPC
/// endpoint id, which is this model's `@pattern` text as written.
const VPC_ENDPOINT_ID_EXPRESSION: &str = r"^aos-[a-zA-Z0-9]*$";

/// The expression a `DomainName` clause quotes: the `Pattern` AWS documents for a domain
/// name, which is this model's `^[a-z][a-z0-9\-]+$` without the pair it wraps.
const DOMAIN_NAME_EXPRESSION: &str = r"[a-z][a-z0-9\-]+";

/// The whole constraint set `VpcEndpointId` declares, as one clause states it:
/// `@length(min: 5, max: 256)` and the published expression, in the order a clause states
/// them.
fn vpc_endpoint_id_set() -> String {
    format!(
        "Member must satisfy constraint: [Member must have length less than or equal to 256, \
         Member must have length greater than or equal to 5, Member must satisfy regular \
         expression pattern: {VPC_ENDPOINT_ID_EXPRESSION}]"
    )
}

/// The whole constraint set `DomainName` declares — a SECOND element shape, on a second
/// operation, so the rule asserted is the collection rule and this API's published
/// expression rather than one member's special case.
fn domain_name_set() -> String {
    format!(
        "Member must satisfy constraint: [Member must have length less than or equal to 28, \
         Member must have length greater than or equal to 3, Member must satisfy regular \
         expression pattern: {DOMAIN_NAME_EXPRESSION}]"
    )
}

/// `DescribeVpcEndpoints`: `POST /2015-01-01/es/vpcEndpoints/describe`.
const DESCRIBE_ENDPOINTS: &str = "/2015-01-01/es/vpcEndpoints/describe";

/// One row's request and the message it must be refused with.
struct Row {
    /// What the row is about, named in every assertion.
    case: &'static str,
    method: &'static str,
    path: String,
    body: Option<Value>,
    /// The whole `message` the refusal has to state.
    expected: String,
}

/// RULE: a constraint broken by a collection's elements is one validation error at the
/// collection member, over the element shape's whole set, quoting the published expression
/// and the collection the request carried.
#[tokio::test]
async fn a_collection_element_is_refused_at_the_collection_member_with_the_published_expression()
{
    let router = router();

    // Each row differs along what the rule generalises over: which of the element shape's
    // constraints the value breaks, how many elements offend, whether a valid element sits
    // beside them, and which operation's collection member — over which element shape —
    // carries them.
    let rows: Vec<Row> = vec![
        Row {
            case: "an id outside the element shape's pattern",
            method: "POST",
            path: DESCRIBE_ENDPOINTS.to_string(),
            body: Some(json!({"VpcEndpointIds": ["vpce-0000000000absent"]})),
            expected: format!(
                "1 validation error detected: Value '[vpce-0000000000absent]' at \
                 'vpcEndpointIds' failed to satisfy constraint: {}",
                vpc_endpoint_id_set()
            ),
        },
        Row {
            case: "an id below the element shape's length floor, and outside its pattern",
            method: "POST",
            path: DESCRIBE_ENDPOINTS.to_string(),
            body: Some(json!({"VpcEndpointIds": ["aos"]})),
            expected: format!(
                "1 validation error detected: Value '[aos]' at 'vpcEndpointIds' failed to \
                 satisfy constraint: {}",
                vpc_endpoint_id_set()
            ),
        },
        Row {
            case: "two offending ids in one list",
            method: "POST",
            path: DESCRIBE_ENDPOINTS.to_string(),
            body: Some(json!({"VpcEndpointIds": ["vpce-one", "nope-two"]})),
            expected: format!(
                "1 validation error detected: Value '[vpce-one, nope-two]' at \
                 'vpcEndpointIds' failed to satisfy constraint: {}",
                vpc_endpoint_id_set()
            ),
        },
        Row {
            // The value a clause quotes is the one the MEMBER HELD, so a list only partly
            // invalid is echoed whole — the well-formed element included.
            case: "an offending id beside a well-formed one",
            method: "POST",
            path: DESCRIBE_ENDPOINTS.to_string(),
            body: Some(json!({"VpcEndpointIds": ["aos-0123456789", "vpce-one"]})),
            expected: format!(
                "1 validation error detected: Value '[aos-0123456789, vpce-one]' at \
                 'vpcEndpointIds' failed to satisfy constraint: {}",
                vpc_endpoint_id_set()
            ),
        },
        Row {
            case: "another operation's collection, over another element shape",
            method: "POST",
            path: "/2015-01-01/es/domain-info".to_string(),
            body: Some(json!({"DomainNames": ["good-domain", "Bad_Domain"]})),
            expected: format!(
                "1 validation error detected: Value '[good-domain, Bad_Domain]' at \
                 'domainNames' failed to satisfy constraint: {}",
                domain_name_set()
            ),
        },
    ];

    for Row {
        case,
        method,
        path,
        body,
        expected,
    } in rows
    {
        let answer = call_in(&router, REGION, method, &path, body.as_ref()).await;
        assert_eq!(
            answer.status, 400,
            "case {case:?}: expected HTTP 400, got {} {}",
            answer.status, answer.body
        );
        assert!(
            answer.error_type.contains("ValidationException"),
            "case {case:?}: expected ValidationException, got {:?} / {}",
            answer.error_type,
            answer.body
        );
        let stated = answer.text(case, "/message");
        assert_eq!(
            stated, expected,
            "case {case:?}: expected {expected:?}, got {stated:?}"
        );
    }
}

/// RULE, the scalar half: a member that is not a collection element keeps its own clause at
/// its own path, quoting the same published expression as the element set does.
#[tokio::test]
async fn a_scalar_member_states_the_published_expression_at_its_own_path() {
    let router = router();

    // The rows span the bindings this model carries a `VpcEndpointId` in — a body member
    // and a URI label — plus a shape whose published expression DROPS the pair, so the two
    // renderings are asserted against each other rather than one alone.
    let rows = vec![
        (
            "UpdateVpcEndpoint's VpcEndpointId body member",
            "POST",
            "/2015-01-01/es/vpcEndpoints/update".to_string(),
            Some(json!({
                "VpcEndpointId": "vpce-0000000000absent",
                "VpcOptions": {"SubnetIds": ["subnet-0000000000000000a"]}
            })),
            format!(
                "1 validation error detected: Value 'vpce-0000000000absent' at \
                 'vpcEndpointId' failed to satisfy constraint: Member must satisfy regular \
                 expression pattern: {VPC_ENDPOINT_ID_EXPRESSION}"
            ),
        ),
        (
            "DeleteVpcEndpoint's VpcEndpointId URI label",
            "DELETE",
            "/2015-01-01/es/vpcEndpoints/vpce-0000000000absent".to_string(),
            None,
            format!(
                "1 validation error detected: Value 'vpce-0000000000absent' at \
                 'vpcEndpointId' failed to satisfy constraint: Member must satisfy regular \
                 expression pattern: {VPC_ENDPOINT_ID_EXPRESSION}"
            ),
        ),
        (
            "DescribeElasticsearchDomain's DomainName URI label",
            "GET",
            "/2015-01-01/es/domain/Bad_Domain".to_string(),
            None,
            format!(
                "1 validation error detected: Value 'Bad_Domain' at 'domainName' failed to \
                 satisfy constraint: Member must satisfy regular expression pattern: \
                 {DOMAIN_NAME_EXPRESSION}"
            ),
        ),
    ];

    for (case, method, path, body, expected) in rows {
        let answer = call_in(&router, REGION, method, &path, body.as_ref()).await;
        assert_eq!(
            answer.status, 400,
            "case {case:?}: expected HTTP 400, got {} {}",
            answer.status, answer.body
        );
        let stated = answer.text(case, "/message");
        assert_eq!(
            stated, expected,
            "case {case:?}: expected {expected:?}, got {stated:?}"
        );
    }
}

/// The clause a `DeleteVpcEndpoint` request is refused with, or `None` when the id reached
/// the handler — which answers this operation's own failure rather than a constraint
/// report, since the CHECK is the model's pattern as written.
async fn pattern_refusal(router: &smithy_server_runtime::MultiServiceRouter, id: &str) -> Option<String> {
    let path = format!("/2015-01-01/es/vpcEndpoints/{id}");
    let answer = call_in(router, REGION, "DELETE", &path, None).await;
    if answer.status != 400 || !answer.error_type.contains("ValidationException") {
        return None;
    }
    answer.body["message"].as_str().map(str::to_owned)
}

/// RULE, stated as the invariant the quoted expression has to satisfy: the expression the
/// refusal publishes, compiled by a client and applied to a candidate id, accepts exactly
/// the ids the service itself accepts.
///
/// This is why the pair is part of what the VPC endpoint id publishes: an unanchored
/// spelling of the same character grammar admits every value that merely CONTAINS an `aos-`
/// run, which the service refuses.
#[tokio::test]
async fn the_published_expression_accepts_exactly_what_the_service_accepts() {
    let router = router();

    // Every candidate is within the shape's `@length(5, 256)`, so the `@pattern` is the
    // only constraint that can decide it, and the rows span what the expression separates:
    // the shortest admitted form, a long one, a mixed-case body, a value whose `aos-` run
    // is not at the start, one that is not all alphanumeric after the prefix, and one whose
    // prefix is cased differently.
    let candidates = [
        ("the shortest admitted id", "aos-0"),
        ("a long alphanumeric id", "aos-0123456789abcdefABCDEF"),
        ("a mixed-case body", "aos-AbC123"),
        ("an aos- run that is not at the start", "vpce-aos-1"),
        ("an aos- run with a suffix after it", "aos-1-vpce"),
        ("a non-alphanumeric body", "aos-ab_cd"),
        ("an upper-case prefix", "AOS-012345"),
        ("no prefix at all", "vpce-0000000000absent"),
    ];

    // The expression the service publishes, taken from a refusal rather than restated, so
    // the compiled matcher below is the one a client would lift out of the message.
    let refused = pattern_refusal(&router, "vpce-0000000000absent")
        .await
        .expect("an id outside the published expression is refused");
    let published = refused
        .rsplit("regular expression pattern: ")
        .next()
        .unwrap_or_default()
        .to_owned();
    assert_eq!(
        published, VPC_ENDPOINT_ID_EXPRESSION,
        "the refusal must publish the documented expression, got {published:?}"
    );
    let matcher = ::regex::Regex::new(&published)
        .unwrap_or_else(|error| panic!("the published expression must compile: {error}"));

    for (case, candidate) in candidates {
        let accepted_by_the_client = matcher.is_match(candidate);
        let refusal = pattern_refusal(&router, candidate).await;
        assert_eq!(
            refusal.is_none(),
            accepted_by_the_client,
            "case {case:?} ({candidate:?}): the service and the published expression must \
             agree; the expression {}, the service answered {refusal:?}",
            if accepted_by_the_client {
                "accepts it"
            } else {
                "refuses it"
            }
        );
    }
}
