// SPDX-License-Identifier: CC0-1.0

//! What a web ACL `AssociateDistributionWebACL` puts in front of a distribution does to the
//! viewer traffic of a PRIVATE Amazon S3 origin, end to end over HTTP across CloudFront, WAFv2
//! and S3 on one engine.
//!
//! RULE: a `CLOUDFRONT`-scope web ACL is in front of a distribution exactly while that
//! distribution's stored configuration names it, and it decides every viewer request BEFORE the
//! edge reads its cache or makes the origin request — a request a terminating action does not
//! let through is answered by the edge itself with HTTP 403 ("CloudFront responds to requests
//! either with the requested content or with an HTTP 403 status code (Forbidden)") carrying none
//! of the origin object's bytes, a request it allows is served the origin's own object, and
//! `DisassociateDistributionWebACL` restores the object on the very request that was refused.
//!
//! Why this suite is cross-service: only here do the three halves meet. The cloudfront crate's
//! own `edge_associated_web_acl` suite states the enforcement over an origin that needs no
//! credential, the wafv2 crate states what a rule tree decides, and `cloudfront_oac_s3_origin_edge`
//! states that a private bucket's object reaches a viewer through an origin access control —
//! none of them can state that the refusal lands IN FRONT of that signed origin fetch, so that
//! a private object's bytes never leave S3 for a request the web ACL refused, and come back on
//! the same request once the association is taken off.
//!
//! The rows span what the rule generalises over: the request component the blocking rule
//! inspects (the URI path, a single viewer header, the query string — three `FieldToMatch`
//! members, each the `smithy_query(query="shape",shape="FieldToMatch")` member of that name)
//! and a web ACL carrying no rule at all whose `DefaultAction` refuses everything; each row is
//! probed with a request the rule matches and one it does not, before the association, while it
//! stands and after it is removed. The recorded case — a `ByteMatchStatement` searching the URI
//! path for `blockme` under `CONTAINS` — is one row.
//!
//! The managed `CachingDisabled` cache policy is on every default behaviour, so each answer
//! below is the origin's own rather than a cached copy of an earlier one.

use std::sync::Arc;

use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::cloudfront_oac_s3_origin_edge::{
    awaited_deployment, call, create_bucket, create_distribution, create_origin_access_control,
    emulator, policy, put_bucket_policy, put_object, Answer, API_VERSION, CLOUDFRONT_HOST,
    CLOUDFRONT_PRINCIPAL, REGION,
};
// The `ByteMatchStatement` builder this crate already has, reused rather than copied: a
// second reading of how a blob member travels in an awsJson body would be a second thing to
// keep right.
use crate::elbv2_published_address_enforces_its_web_acl::byte_match;

/// The AWS-managed cache policy `Managed-CachingDisabled`, whose TTLs are all zero: every
/// viewer request under it reaches the origin, so no answer below comes out of the edge cache.
const CACHING_DISABLED: &str = "4135ea2d-6df8-44a3-9df3-4b5a84be39ad";

/// The scope a web ACL that fronts a distribution is created in: "for an Amazon CloudFront
/// distribution, use CLOUDFRONT" (`CreateWebACLRequest$Scope`).
const SCOPE_CLOUDFRONT: &str = "CLOUDFRONT";

/// The status the edge answers a viewer request the web ACL did not let through.
const REFUSED_STATUS: u16 = 403;

/// The header the edge reports an answer IT produced under, rather than one an origin gave it.
const CLOUDFRONT_OWN_ANSWER: &str = "Error from cloudfront";

/// The header an answer the edge fetched from the origin under this policy carries.
const ORIGIN_ANSWER: &str = "Miss from cloudfront";

/// One object the origin bucket holds: its key and its bytes, which are what an allowed request
/// is served and what a refused one must not carry.
struct Object {
    key: &'static str,
    body: &'static str,
}

/// The two objects every row's bucket holds.
const OBJECTS: &[Object] = &[
    Object {
        key: "index.html",
        body: "allowed-content",
    },
    Object {
        key: "blockme.html",
        body: "blocked-path-content",
    },
];

/// One viewer request a row probes the distribution with.
struct Probe {
    label: &'static str,
    /// The request URI, query string included.
    path: &'static str,
    /// The viewer headers the request carries beside its host.
    headers: &'static [(&'static str, &'static str)],
    /// The object bytes this request is served when nothing refuses it.
    body: &'static str,
    /// Whether the row's web ACL refuses it while the association stands.
    refused: bool,
}

/// One row: the web ACL a distribution is put behind, and the requests it then decides.
struct Row {
    case: &'static str,
    /// WAFv2's own name for the web ACL, which is its identity there.
    web_acl: &'static str,
    /// `WebACL$DefaultAction`, the action for a request no rule matched.
    default_action: Value,
    /// `WebACL$Rules`, empty for a web ACL with no rules at all.
    rules: Value,
    probes: &'static [Probe],
}

/// A minimal `VisibilityConfig`, which every web ACL and every rule has to carry.
fn visibility(metric: &str) -> Value {
    json!({
        "SampledRequestsEnabled": false,
        "CloudWatchMetricsEnabled": false,
        "MetricName": metric,
    })
}

/// One `Block` rule at priority 0 over `statement`.
fn blocking_rule(name: &str, statement: Value) -> Value {
    json!([{
        "Name": name,
        "Priority": 0,
        "Statement": statement,
        "Action": { "Block": {} },
        "VisibilityConfig": visibility(name),
    }])
}

/// The header the single-header row's rule inspects, and the value it matches.
const PROBE_HEADER: &str = "x-waf-probe";
const PROBE_HEADER_REFUSED: &str = "refuse";

/// The table: one row per request component a blocking rule inspects, plus one web ACL that
/// refuses everything by its default action.
fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "a Block rule on the URI path",
            web_acl: "edge-origin-block-uri-path",
            default_action: json!({ "Allow": {} }),
            rules: blocking_rule(
                "block-the-uri-path",
                byte_match(json!({ "UriPath": {} }), "blockme", "CONTAINS"),
            ),
            probes: &[
                Probe {
                    label: "a path the rule does not match",
                    path: "/index.html",
                    headers: &[],
                    body: "allowed-content",
                    refused: false,
                },
                Probe {
                    label: "the path the rule matches",
                    path: "/blockme.html",
                    headers: &[],
                    body: "blocked-path-content",
                    refused: true,
                },
            ],
        },
        Row {
            case: "a Block rule on a single viewer header",
            web_acl: "edge-origin-block-header",
            default_action: json!({ "Allow": {} }),
            rules: blocking_rule(
                "block-the-header",
                byte_match(
                    json!({ "SingleHeader": { "Name": PROBE_HEADER } }),
                    PROBE_HEADER_REFUSED,
                    "EXACTLY",
                ),
            ),
            probes: &[
                Probe {
                    label: "the same path carrying no such header",
                    path: "/index.html",
                    headers: &[],
                    body: "allowed-content",
                    refused: false,
                },
                Probe {
                    label: "the path carrying the header the rule matches",
                    path: "/index.html",
                    headers: &[(PROBE_HEADER, PROBE_HEADER_REFUSED)],
                    body: "allowed-content",
                    refused: true,
                },
            ],
        },
        Row {
            case: "a Block rule on the query string",
            web_acl: "edge-origin-block-query-string",
            default_action: json!({ "Allow": {} }),
            rules: blocking_rule(
                "block-the-query-string",
                byte_match(json!({ "QueryString": {} }), "refuse", "CONTAINS"),
            ),
            probes: &[
                Probe {
                    label: "a query string the rule does not match",
                    path: "/blockme.html?keep=1",
                    headers: &[],
                    body: "blocked-path-content",
                    refused: false,
                },
                Probe {
                    label: "the query string the rule matches",
                    path: "/blockme.html?refuse=1",
                    headers: &[],
                    body: "blocked-path-content",
                    refused: true,
                },
            ],
        },
        Row {
            case: "no rules at all and a DefaultAction that blocks",
            web_acl: "edge-origin-block-everything",
            default_action: json!({ "Block": {} }),
            rules: json!([]),
            probes: &[
                Probe {
                    label: "one object of the origin",
                    path: "/index.html",
                    headers: &[],
                    body: "allowed-content",
                    refused: true,
                },
                Probe {
                    label: "the other object of the origin",
                    path: "/blockme.html",
                    headers: &[],
                    body: "blocked-path-content",
                    refused: true,
                },
            ],
        },
    ]
}

/// The setup plane: CloudFront, S3 and WAFv2 on one engine with request authentication off, so
/// each setup call carries a credential scope without a computed signature. The VIEWER plane is
/// the authenticating one, which is what makes the signature the edge puts on its origin fetch
/// the only thing that can make the private bucket serve it.
fn setup_plane(engine: &Arc<engine::InMemoryEngine>) -> MultiServiceRouter {
    let mut router = emulator(engine, false);
    router.register_service(
        ::service_com_amazonaws_wafv2_awswaf_20190729::EmulatedService::new(Arc::clone(engine)),
    );
    router
}

/// `wafv2:CreateWebACL` in the `CLOUDFRONT` scope, answering the ARN a CloudFront resource
/// names its web ACL by.
async fn create_web_acl(router: &MultiServiceRouter, row: &Row) -> String {
    let request = json!({
        "Name": row.web_acl,
        "Scope": SCOPE_CLOUDFRONT,
        "DefaultAction": row.default_action,
        "Rules": row.rules,
        "VisibilityConfig": visibility(row.web_acl),
    });
    let created = call(
        router,
        "wafv2",
        &format!("wafv2.{REGION}.amazonaws.com"),
        "POST",
        "/",
        &[
            ("content-type", "application/x-amz-json-1.1"),
            ("x-amz-target", "AWSWAF_20190729.CreateWebACL"),
        ],
        Bytes::from(serde_json::to_vec(&request).expect("the request serializes")),
    )
    .await;
    assert_eq!(
        created.status,
        200,
        "case {:?}: CreateWebACL must succeed, got {} with {:?}",
        row.case,
        created.status,
        created.text()
    );
    let parsed: Value = serde_json::from_slice(&created.body).unwrap_or(Value::Null);
    parsed["Summary"]["ARN"]
        .as_str()
        .unwrap_or_else(|| {
            panic!(
                "case {:?}: CreateWebACL must answer Summary.ARN, got {parsed}",
                row.case
            )
        })
        .to_string()
}

/// `cloudfront:GetDistribution`, as `(the version token, the `WebACLId` it reports)`.
async fn read_distribution(router: &MultiServiceRouter, case: &str, id: &str) -> (String, String) {
    let read = call(
        router,
        "cloudfront",
        CLOUDFRONT_HOST,
        "GET",
        &format!("/{API_VERSION}/distribution/{id}"),
        &[],
        Bytes::new(),
    )
    .await;
    assert_eq!(
        read.status,
        200,
        "case {case:?}: GetDistribution must succeed, got {} with {:?}",
        read.status,
        read.text()
    );
    let etag = read.header("etag");
    assert!(
        !etag.is_empty(),
        "case {case:?}: GetDistribution must report the current version: {:?}",
        read.text()
    );
    (etag, read.element("WebACLId"))
}

/// `cloudfront:AssociateDistributionWebACL` at the version `if_match`, exactly as a client makes
/// it: the web ACL travels in the request document and the version in the `If-Match` header.
async fn associate_web_acl(
    router: &MultiServiceRouter,
    case: &str,
    id: &str,
    web_acl_arn: &str,
    if_match: &str,
) {
    let body = format!(
        "<AssociateDistributionWebACLRequest \
         xmlns=\"http://cloudfront.amazonaws.com/doc/{API_VERSION}/\">\
         <WebACLArn>{web_acl_arn}</WebACLArn></AssociateDistributionWebACLRequest>"
    );
    let answer = call(
        router,
        "cloudfront",
        CLOUDFRONT_HOST,
        "PUT",
        &format!("/{API_VERSION}/distribution/{id}/associate-web-acl"),
        &[
            ("content-type", "application/xml"),
            ("if-match", if_match),
        ],
        Bytes::from(body),
    )
    .await;
    assert_eq!(
        answer.status,
        200,
        "case {case:?}: AssociateDistributionWebACL must succeed, got {} with {:?}",
        answer.status,
        answer.text()
    );
    assert_eq!(
        answer.element("WebACLArn"),
        web_acl_arn,
        "case {case:?}: the association must report the web ACL it bound, got {:?}",
        answer.text()
    );
}

/// `cloudfront:DisassociateDistributionWebACL` at the version `if_match`. The operation carries
/// no body.
async fn disassociate_web_acl(
    router: &MultiServiceRouter,
    case: &str,
    id: &str,
    if_match: &str,
) {
    let answer = call(
        router,
        "cloudfront",
        CLOUDFRONT_HOST,
        "PUT",
        &format!("/{API_VERSION}/distribution/{id}/disassociate-web-acl"),
        &[("if-match", if_match)],
        Bytes::new(),
    )
    .await;
    assert_eq!(
        answer.status,
        200,
        "case {case:?}: DisassociateDistributionWebACL must succeed, got {} with {:?}",
        answer.status,
        answer.text()
    );
}

/// One VIEWER request: addressed to the distribution's own domain name, carrying the probe's
/// headers and no credential of any kind, exactly as a browser reaches a distribution.
async fn probe_edge(router: &MultiServiceRouter, domain: &str, probe: &Probe) -> Answer {
    let mut builder = ::http::Request::builder()
        .method("GET")
        .uri(probe.path)
        .header("host", domain);
    for (name, value) in probe.headers {
        builder = builder.header(*name, *value);
    }
    let request = builder
        .body(Bytes::new())
        .expect("a well-formed viewer request");
    let response = router.dispatch(request).await;
    let (parts, body) = response.into_parts();
    Answer {
        status: parts.status.as_u16(),
        headers: parts.headers,
        body,
    }
}

/// Assert the viewer was served the ORIGIN's own object for `probe`.
async fn assert_served(
    router: &MultiServiceRouter,
    case: &str,
    stage: &str,
    domain: &str,
    probe: &Probe,
) {
    let answer = probe_edge(router, domain, probe).await;
    assert_eq!(
        answer.status,
        200,
        "case {case:?} ({stage}, {}): expected the origin's object, got {} with {:?}",
        probe.label,
        answer.status,
        answer.text()
    );
    assert_eq!(
        answer.text(),
        probe.body,
        "case {case:?} ({stage}, {}): expected the object's own bytes {:?}, got {:?}",
        probe.label,
        probe.body,
        answer.text()
    );
    assert_eq!(
        answer.header("x-cache"),
        ORIGIN_ANSWER,
        "case {case:?} ({stage}, {}): expected an answer fetched from the origin, got X-Cache \
         {:?}",
        probe.label,
        answer.header("x-cache")
    );
}

/// Assert the EDGE refused `probe` itself: the refusal status, none of the object's bytes, and
/// the answer reported as CloudFront's own rather than an origin's — so the private origin was
/// never reached for it.
async fn assert_refused(
    router: &MultiServiceRouter,
    case: &str,
    stage: &str,
    domain: &str,
    probe: &Probe,
) {
    let answer = probe_edge(router, domain, probe).await;
    assert_eq!(
        answer.status, REFUSED_STATUS,
        "case {case:?} ({stage}, {}): expected the edge to refuse the request {REFUSED_STATUS}, \
         got {} with {:?}",
        probe.label,
        answer.status,
        answer.text()
    );
    assert!(
        !answer.text().contains(probe.body),
        "case {case:?} ({stage}, {}): a refusal the edge produced carries none of the object's \
         bytes, got {:?}",
        probe.label,
        answer.text()
    );
    assert_eq!(
        answer.header("x-cache"),
        CLOUDFRONT_OWN_ANSWER,
        "case {case:?} ({stage}, {}): expected the answer to be CloudFront's own, got X-Cache \
         {:?}",
        probe.label,
        answer.header("x-cache")
    );
}

/// RULE: the `CLOUDFRONT`-scope web ACL an association puts in front of a distribution decides
/// every viewer request before the edge reaches the private S3 origin — a request it does not
/// let through is answered 403 by the edge with none of the object's bytes, a request it allows
/// is served the object itself — and the disassociation restores the object on the refused
/// request.
#[tokio::test]
async fn an_associated_web_acl_refuses_the_private_origins_bytes_until_it_is_taken_off() {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let setup = setup_plane(&engine);
    let viewers = emulator(&engine, true);

    for (index, row) in rows().into_iter().enumerate() {
        let case = row.case;
        let web_acl_arn = create_web_acl(&setup, &row).await;

        // A bucket of this row's own, so one row's grant cannot admit another row's read.
        let bucket = format!("waf-edge-origin-{index}");
        create_bucket(&setup, case, &bucket).await;
        for object in OBJECTS {
            put_object(&setup, case, &bucket, object.key, object.body, &[]).await;
        }

        let control =
            create_origin_access_control(&setup, case, &format!("waf-edge-oac-{index}"), "always")
                .await;
        let (id, domain, arn) = create_distribution(
            &setup,
            case,
            &format!("waf-edge-reference-{index}"),
            &bucket,
            &control,
            CACHING_DISABLED,
        )
        .await;
        // The grant is scoped to the distribution's own ARN, so it can only be written once
        // that distribution exists — the order a client has to follow.
        put_bucket_policy(
            &setup,
            case,
            &bucket,
            &policy(
                CLOUDFRONT_PRINCIPAL,
                "s3:GetObject",
                &format!("arn:aws:s3:::{bucket}/*"),
                Some(&format!(
                    "{{\"StringEquals\":{{\"AWS:SourceArn\":\"{arn}\"}}}}"
                )),
            ),
        )
        .await;
        awaited_deployment(&setup, case, &id).await;

        // THE POSITIVE CONTROL: with nothing in front of it, the distribution serves every
        // probe the origin's own object — so a refusal below is the web ACL's doing and not an
        // origin that never answered.
        for probe in row.probes {
            assert_served(&viewers, case, "before the association", &domain, probe).await;
        }

        let (created_version, before) = read_distribution(&setup, case, &id).await;
        assert!(
            before.is_empty(),
            "case {case:?}: the distribution must name no web ACL before the association, got \
             {before:?}"
        );
        associate_web_acl(&setup, case, &id, &web_acl_arn, &created_version).await;
        awaited_deployment(&setup, case, &id).await;
        let (associated_version, named) = read_distribution(&setup, case, &id).await;
        assert_eq!(
            named, web_acl_arn,
            "case {case:?}: the distribution must name the associated web ACL, got {named:?}"
        );

        for probe in row.probes {
            if probe.refused {
                assert_refused(&viewers, case, "with the web ACL associated", &domain, probe)
                    .await;
            } else {
                assert_served(&viewers, case, "with the web ACL associated", &domain, probe).await;
            }
        }

        // Taking the association off leaves nothing in front of the distribution, so every
        // probe is served the origin's object again — including the ones just refused.
        disassociate_web_acl(&setup, case, &id, &associated_version).await;
        awaited_deployment(&setup, case, &id).await;
        let (_, cleared) = read_distribution(&setup, case, &id).await;
        assert!(
            cleared.is_empty(),
            "case {case:?}: the disassociation must leave the distribution naming no web ACL, \
             got {cleared:?}"
        );

        for probe in row.probes {
            assert_served(&viewers, case, "after the disassociation", &domain, probe).await;
        }
    }
}
