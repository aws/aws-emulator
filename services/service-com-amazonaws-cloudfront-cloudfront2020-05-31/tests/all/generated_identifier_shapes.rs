// SPDX-License-Identifier: CC0-1.0

//! The identifiers and URIs CloudFront DERIVES at create time, end to end over HTTP.
//!
//! Every assertion goes through the router, so each case exercises routing, the restXml
//! (de)serialization, the handler, the persisted state and serialization — the same path a
//! client takes. The rules under test are:
//!
//! * `CreateFunction`'s `Location` — "The URL of the CloudFront function. Use the URL to
//!   manage the function with the CloudFront API" — addresses the function by the ARN the
//!   create MINTED ("The response contains an Amazon Resource Name (ARN) that uniquely
//!   identifies the function"), not by the name the request submitted;
//! * an OPAQUE namespace-prefixed identifier this service mints — a connection group's, a
//!   distribution tenant's, a connection function's — is its namespace label followed by 27
//!   characters of base62, the width and alphabet CloudFront writes one in
//!   (`dt_3JO1OQa51Jb3BZh7XQh6pbvF3Wa`), and the ARN built around it embeds exactly that id;
//! * an identifier this service mints as a UUID — a response headers policy's, a cache
//!   policy's, an origin request policy's, a key value store's — is a well-formed
//!   randomly-generated UUID: lowercase hex in 8-4-4-4-12 groups, with the VERSION nibble
//!   `4` and the VARIANT nibble in `8`, `9`, `a`, `b`, which is the form CloudFront's own
//!   minted policy ids take (`658327ea-f89d-4fab-a63d-7e88639e58f6`, the managed
//!   `CachingOptimized` cache policy).
//!
//! The rows span what each rule generalises over: for the function, the name's length and
//! character classes, the runtime and the size of the code — none of which the derivation
//! depends on; for the minted ids, every resource kind of the family, created twice so no
//! two creates of one kind share an id.
//!
//! The wire harness (the router and the request helper) is [`crate::distribution_lifecycle`]'s,
//! and each create goes through the sibling suite's own call path rather than a second copy
//! of it.

use std::collections::BTreeSet;

use smithy_server_runtime::MultiServiceRouter;

use crate::connection_function_lifecycle::create_connection_function;
use crate::connection_groups::create_connection_group;
use crate::distribution_lifecycle::{
    call, emulator, Answer, ACCOUNT_ID, API_VERSION, HOST,
};
use crate::distribution_tenant_lifecycle::{
    create_distribution_tenant, managed_certificate_request, multi_tenant_distribution,
};
use crate::key_value_store_lifecycle::create_key_value_store;

/// How many characters of base62 follow the namespace label of an opaque identifier
/// CloudFront mints, and the alphabet they are drawn from — spelled out here rather than
/// read off the crate's own constant so a wrong constant cannot make these assertions pass.
const OPAQUE_BODY_LEN: usize = 27;

/// The text of one member of a response document, or a panic naming the case.
fn member(answer: &Answer, path: &str, case: &str) -> String {
    answer
        .root()
        .find_text(path)
        .unwrap_or_else(|| panic!("case {case}: the answer carries no {path}: {}", answer.raw))
        .to_string()
}

// ---------------------------------------------------------------------------
// CreateFunction's announced URL
// ---------------------------------------------------------------------------

/// One `CreateFunction` request a row submits.
struct Function {
    case: &'static str,
    name: String,
    runtime: &'static str,
    code: Vec<u8>,
}

/// The creates the Location rule is asserted over: the shortest and the longest admissible
/// name, a name spanning the character classes `FunctionName`'s pattern admits, each with a
/// runtime and a code size of its own — the derivation depends on none of them.
fn function_table() -> Vec<Function> {
    vec![
        Function {
            case: "the shortest admissible name",
            name: "a".to_string(),
            runtime: "cloudfront-js-1.0",
            code: b"function handler(e) { return e.request; }".to_vec(),
        },
        Function {
            case: "a name spanning the admissible character classes",
            name: "Fn-9_derived-URL".to_string(),
            runtime: "cloudfront-js-2.0",
            code: vec![b'x'; 4096],
        },
        Function {
            case: "the longest admissible name",
            name: "f".repeat(64),
            runtime: "cloudfront-js-2.0",
            code: b"function handler(event) { return event.request; }".to_vec(),
        },
    ]
}

/// `POST /2020-05-31/function` with the request document for `function`.
async fn create_function(router: &MultiServiceRouter, function: &Function) -> Answer {
    let body = format!(
        "<CreateFunctionRequest xmlns=\"http://cloudfront.amazonaws.com/doc/{API_VERSION}/\">\
         <Name>{}</Name>\
         <FunctionConfig><Comment>derived output</Comment><Runtime>{}</Runtime></FunctionConfig>\
         <FunctionCode>{}</FunctionCode>\
         </CreateFunctionRequest>",
        function.name,
        function.runtime,
        // The padded standard base64 a restXml blob MEMBER travels as.
        ::aws_smithy_types::base64::encode(&function.code)
    );
    call(
        router,
        "POST",
        &format!("/{API_VERSION}/function"),
        Some(&body),
        None,
    )
    .await
}

/// `CreateFunction` announces the new function at its own ARN: the `Location` is the
/// function resource under the endpoint the create was answered on, whose path segment is
/// the ARN the create minted — never the name the request carried.
#[tokio::test]
async fn a_function_create_announces_the_function_by_its_arn() {
    let router = emulator();
    let mut arns = BTreeSet::new();

    for function in function_table() {
        let case = function.case;
        let created = create_function(&router, &function).await;
        assert_eq!(
            created.status, 201,
            "case {case}: expected HTTP 201, got {} with {}",
            created.status, created.raw
        );

        // The ARN the service minted, read off the record the create answered with.
        let arn = member(&created, "FunctionARN", case);
        assert_eq!(
            arn,
            format!("arn:aws:cloudfront::{ACCOUNT_ID}:function/{}", function.name),
            "case {case}: expected the function's own ARN, got {arn:?}"
        );
        assert!(
            arns.insert(arn.clone()),
            "case {case}: the ARN {arn} repeated"
        );

        let location = created.header("location").to_string();
        assert_eq!(
            location,
            format!("https://{HOST}/{API_VERSION}/function/{arn}"),
            "case {case}: expected the function addressed by its ARN in Location, got \
             {location:?}"
        );
        assert_ne!(
            location,
            format!("https://{HOST}/{API_VERSION}/function/{}", function.name),
            "case {case}: Location must not address the function by the name the request \
             submitted"
        );
    }
}

// ---------------------------------------------------------------------------
// The opaque namespaced ids
// ---------------------------------------------------------------------------

/// Whether `body` is the body of an opaque identifier CloudFront mints: 27 base62
/// characters.
fn is_opaque_body(body: &str) -> bool {
    body.len() == OPAQUE_BODY_LEN && body.bytes().all(|b| b.is_ascii_alphanumeric())
}

/// One kind of opaque namespaced identifier, and what a create of that kind reports.
struct Minted {
    case: &'static str,
    /// The namespace label, separator included, every id of this kind carries.
    label: &'static str,
    /// The id the create reported.
    id: String,
    /// The ARN the create reported for the same resource, or `None` for a kind whose create
    /// reports none.
    arn: Option<String>,
    /// The resource part the ARN carries ahead of the id.
    arn_resource: &'static str,
}

/// Create one resource of every kind this service mints an opaque namespaced id for, under
/// identifiers derived from `run` so two rounds of this never collide on a name.
async fn mint_namespaced(router: &MultiServiceRouter, run: usize) -> Vec<Minted> {
    let group = create_connection_group(
        router,
        &format!(
            "<CreateConnectionGroupRequest \
             xmlns=\"http://cloudfront.amazonaws.com/doc/{API_VERSION}/\">\
             <Name>minted-group-{run}</Name></CreateConnectionGroupRequest>"
        ),
    )
    .await;
    assert_eq!(
        group.status, 201,
        "case CreateConnectionGroup: expected HTTP 201, got {} with {}",
        group.status, group.raw
    );

    let distribution = multi_tenant_distribution(router, &format!("minted-ids-{run}")).await;
    let tenant = create_distribution_tenant(
        router,
        &format!(
            "<CreateDistributionTenantRequest \
             xmlns=\"http://cloudfront.amazonaws.com/doc/{API_VERSION}/\">\
             <DistributionId>{distribution}</DistributionId>\
             <Name>minted-tenant-{run}</Name>\
             <Domains><member><Domain>minted-{run}.minted-ids.example.com</Domain></member>\
             </Domains>\
             <Parameters><member><Name>tenantName</Name><Value>minted-{run}</Value></member>\
             </Parameters>{}</CreateDistributionTenantRequest>",
            managed_certificate_request()
        ),
    )
    .await;
    assert_eq!(
        tenant.status, 201,
        "case CreateDistributionTenant: expected HTTP 201, got {} with {}",
        tenant.status, tenant.raw
    );

    // The connection function's own id is drawn from the same mint, so it is one row of
    // this table too.
    let function = create_connection_function(
        router,
        &format!(
            "<CreateConnectionFunctionRequest \
             xmlns=\"http://cloudfront.amazonaws.com/doc/{API_VERSION}/\">\
             <Name>minted_function_{run}</Name>\
             <ConnectionFunctionConfig><Comment>minted</Comment>\
             <Runtime>cloudfront-js-2.0</Runtime></ConnectionFunctionConfig>\
             <ConnectionFunctionCode>{}</ConnectionFunctionCode>\
             </CreateConnectionFunctionRequest>",
            ::aws_smithy_types::base64::encode("function handler(e) { return e.request; }")
        ),
    )
    .await;
    assert_eq!(
        function.status, 201,
        "case CreateConnectionFunction: expected HTTP 201, got {} with {}",
        function.status, function.raw
    );

    vec![
        Minted {
            case: "CreateConnectionGroup",
            label: "cg_",
            id: member(&group, "Id", "CreateConnectionGroup"),
            arn: Some(member(&group, "Arn", "CreateConnectionGroup")),
            arn_resource: "connection-group",
        },
        Minted {
            case: "CreateDistributionTenant",
            label: "dt_",
            id: member(&tenant, "Id", "CreateDistributionTenant"),
            arn: Some(member(&tenant, "Arn", "CreateDistributionTenant")),
            arn_resource: "distribution-tenant",
        },
        Minted {
            case: "CreateConnectionFunction",
            label: "cf_",
            id: member(&function, "Id", "CreateConnectionFunction"),
            arn: Some(member(
                &function,
                "ConnectionFunctionArn",
                "CreateConnectionFunction",
            )),
            arn_resource: "connection-function",
        },
    ]
}

/// Every opaque namespaced id this service mints is its namespace label followed by 27
/// base62 characters, the ARN built around it embeds that very id, and no two creates of one
/// kind are minted the same id.
#[tokio::test]
async fn a_minted_namespaced_id_is_its_label_and_a_base62_token() {
    let router = emulator();
    let mut ids = BTreeSet::new();

    // Two rounds, so the uniqueness of each kind's ids is asserted over creates rather than
    // over one.
    for run in 0..2 {
        for minted in mint_namespaced(&router, run).await {
            let Minted {
                case,
                label,
                id,
                arn,
                arn_resource,
            } = minted;
            let body = id.strip_prefix(label).unwrap_or_else(|| {
                panic!("case {case}: expected an id under the {label} label, got {id:?}")
            });
            assert!(
                is_opaque_body(body),
                "case {case}: expected {OPAQUE_BODY_LEN} base62 characters after {label}, got \
                 {id:?}"
            );
            if let Some(arn) = arn {
                assert_eq!(
                    arn,
                    format!("arn:aws:cloudfront::{ACCOUNT_ID}:{arn_resource}/{id}"),
                    "case {case}: expected the ARN built from the minted id {id}, got {arn:?}"
                );
            }
            assert!(
                ids.insert(id.clone()),
                "case {case}: the id {id} repeated"
            );
        }
    }
}

// ---------------------------------------------------------------------------
// The minted UUIDs
// ---------------------------------------------------------------------------

/// Whether `value` is a well-formed randomly-generated UUID: lowercase hex in 8-4-4-4-12
/// groups, with the version nibble `4` and the variant nibble in `8`, `9`, `a`, `b` — the
/// form CloudFront's own minted policy ids take.
fn is_version_4_uuid(value: &str) -> bool {
    let groups: Vec<&str> = value.split('-').collect();
    let widths = [8usize, 4, 4, 4, 12];
    if groups.len() != widths.len() {
        return false;
    }
    let shaped = groups.iter().zip(widths).all(|(group, width)| {
        group.len() == width
            && group
                .bytes()
                .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
    });
    shaped
        && groups[2].starts_with('4')
        && groups[3].starts_with(['8', '9', 'a', 'b'])
}

/// Create one resource of every kind this service identifies by a minted UUID, under names
/// derived from `run`, and answer `(case, id)` for each.
async fn mint_uuids(router: &MultiServiceRouter, run: usize) -> Vec<(&'static str, String)> {
    let response_headers_policy = call(
        router,
        "POST",
        &format!("/{API_VERSION}/response-headers-policy"),
        Some(&format!(
            "<ResponseHeadersPolicyConfig \
             xmlns=\"http://cloudfront.amazonaws.com/doc/{API_VERSION}/\">\
             <Name>minted-rhp-{run}</Name></ResponseHeadersPolicyConfig>"
        )),
        None,
    )
    .await;
    let cache_policy = call(
        router,
        "POST",
        &format!("/{API_VERSION}/cache-policy"),
        Some(&format!(
            "<CachePolicyConfig xmlns=\"http://cloudfront.amazonaws.com/doc/{API_VERSION}/\">\
             <Name>minted-cache-{run}</Name><MinTTL>1</MinTTL>\
             <ParametersInCacheKeyAndForwardedToOrigin>\
             <EnableAcceptEncodingGzip>false</EnableAcceptEncodingGzip>\
             <HeadersConfig><HeaderBehavior>none</HeaderBehavior></HeadersConfig>\
             <CookiesConfig><CookieBehavior>none</CookieBehavior></CookiesConfig>\
             <QueryStringsConfig><QueryStringBehavior>none</QueryStringBehavior>\
             </QueryStringsConfig>\
             </ParametersInCacheKeyAndForwardedToOrigin></CachePolicyConfig>"
        )),
        None,
    )
    .await;
    let origin_request_policy = call(
        router,
        "POST",
        &format!("/{API_VERSION}/origin-request-policy"),
        Some(&format!(
            "<OriginRequestPolicyConfig \
             xmlns=\"http://cloudfront.amazonaws.com/doc/{API_VERSION}/\">\
             <Name>minted-orp-{run}</Name>\
             <HeadersConfig><HeaderBehavior>none</HeaderBehavior></HeadersConfig>\
             <CookiesConfig><CookieBehavior>none</CookieBehavior></CookiesConfig>\
             <QueryStringsConfig><QueryStringBehavior>none</QueryStringBehavior>\
             </QueryStringsConfig></OriginRequestPolicyConfig>"
        )),
        None,
    )
    .await;
    let key_value_store = create_key_value_store(
        router,
        &format!(
            "<CreateKeyValueStoreRequest \
             xmlns=\"http://cloudfront.amazonaws.com/doc/{API_VERSION}/\">\
             <Name>minted-kvs-{run}</Name></CreateKeyValueStoreRequest>"
        ),
    )
    .await;

    let mut minted = Vec::new();
    for (case, answer) in [
        ("CreateResponseHeadersPolicy", &response_headers_policy),
        ("CreateCachePolicy", &cache_policy),
        ("CreateOriginRequestPolicy", &origin_request_policy),
        ("CreateKeyValueStore", &key_value_store),
    ] {
        assert_eq!(
            answer.status, 201,
            "case {case}: expected HTTP 201, got {} with {}",
            answer.status, answer.raw
        );
        minted.push((case, member(answer, "Id", case)));
    }

    // The policy whose create ANNOUNCES its id in a `Location` is held to it there too: the
    // URI's last segment is the id the create minted.
    let location = response_headers_policy.header("location").to_string();
    let announced = minted
        .first()
        .map(|(_, id)| id.clone())
        .unwrap_or_default();
    assert_eq!(
        location,
        format!("https://{HOST}/{API_VERSION}/response-headers-policy/{announced}"),
        "case CreateResponseHeadersPolicy: expected the policy addressed by its minted id in \
         Location, got {location:?}"
    );

    minted
}

/// Every id this service mints as a UUID is a well-formed randomly-generated UUID, and no
/// two creates are minted the same one.
#[tokio::test]
async fn a_minted_uuid_id_is_a_well_formed_random_uuid() {
    let router = emulator();
    let mut ids = BTreeSet::new();

    // Two rounds, so the uniqueness is asserted over creates rather than over one.
    for run in 0..2 {
        for (case, id) in mint_uuids(&router, run).await {
            assert!(
                is_version_4_uuid(&id),
                "case {case}: expected a well-formed randomly-generated UUID, got {id:?}"
            );
            assert!(ids.insert(id.clone()), "case {case}: the id {id} repeated");
        }
    }
}
