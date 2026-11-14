// SPDX-License-Identifier: CC0-1.0

//! What the RESPONSE HEADERS POLICY a cache behaviour attaches does at the EDGE, end to end:
//! the headers the answer a viewer gets carries, and which viewers an edge address serves at
//! all.
//!
//! Three rules are under test.
//!
//! * The edge applies the matched cache behaviour's response headers policy to the answer it
//!   serves: the CORS group for a request whose `Origin` the policy's
//!   `AccessControlAllowOrigins` grants, the configured security and custom headers, and the
//!   removal of the headers its `RemoveHeadersConfig` names. WHICH of the CORS headers an
//!   answer carries is the headers' own definitions: the allowed methods, the allowed
//!   headers and the max age answer what a PREFLIGHT asked (an `OPTIONS` request carrying
//!   `Access-Control-Request-Method`), and the exposed headers answer what an ACTUAL
//!   request's answer may be read for.
//! * CORS is the BROWSER's to enforce, not the edge's: a request from an origin the policy
//!   does not grant is served exactly as a request carrying no `Origin` is — the object, not
//!   a refusal — and simply carries none of the CORS headers, so the page that made it
//!   cannot read the answer.
//! * A method the matched behaviour's `AllowedMethods` does not allow is refused by
//!   CloudFront itself, whatever the policy's CORS configuration says about methods: a
//!   response headers policy states what a RESPONSE carries and refuses nothing.
//!
//! The rows span what each rule generalises over: the origin a request presents (one the
//! policy names, the second one it names, one it does not, none at all, and a policy that
//! names the wildcard instead), whether the request is a preflight, the method (a read-only
//! one, a state-changing one), each group the configuration can carry (CORS, every security
//! header, custom headers, removals) and both settings of the `Override` that decides
//! against the origin's own headers.
//!
//! The last suite dials the address the distribution PUBLISHES, over a socket, which is what
//! a browser does: there the admission of the request is under test as well as the headers —
//! that address admits a browser request from another site only when the distribution's own
//! configuration NAMES its origin, which is this emulator guarding the machine it runs on and
//! is not something CloudFront does.
//!
//! The wire harness is [`crate::distribution_lifecycle`]'s, the policy document builder
//! [`crate::response_headers_policy_lifecycle`]'s and the viewer dial
//! [`crate::published_edge_address`]'s, reused rather than copied.

use std::collections::BTreeMap;

use bytes::Bytes;
use smithy_server_runtime::MultiServiceRouter;

use crate::distribution_lifecycle::{
    awaited_deployment, create_distribution, emulator, is_reachable_authority, API_VERSION, HOST,
};
use crate::published_edge_address::dial_with;
use crate::response_headers_policy_lifecycle::{create_policy, Cors, Hsts, Policy, Security, Xss};

/// An origin the policies below name.
const GRANTED: &str = "https://app.example.com";

/// The second origin the named-origins policy lists, so a row proves the answer carries the
/// origin THIS viewer presented rather than the list.
const SECOND_GRANTED: &str = "https://admin.example.com";

/// An origin no policy below names.
const FOREIGN: &str = "https://other.example";

/// The AWS-managed cache policy `Managed-CachingDisabled`, whose TTLs are all zero: every
/// request under it reaches the origin, so one row's answer is never another row's.
const CACHING_DISABLED: &str = "4135ea2d-6df8-44a3-9df3-4b5a84be39ad";

/// An origin this emulator SERVES: this service's own control-plane endpoint, whose
/// `GET /2020-05-31/distribution` answers a `DistributionList`.
const SERVED_ORIGIN: &str = HOST;

/// The viewer path every row fetches, which that origin answers.
const SERVED_PATH: &str = "/2020-05-31/distribution";

/// What CloudFront's own page says when the matched behaviour's `AllowedMethods` does not
/// allow the viewer's method.
const METHOD_REFUSED: &str = "This distribution is not configured to allow the HTTP request \
                              method that was used for this request. The distribution \
                              supports only cachable requests.";

/// What a distribution's own address answers a browser request it does not admit, which is
/// the core's own wording for such a request and not a page CloudFront ever serves.
const CROSS_SITE_REFUSED: &str = "cross-site origin";

/// What a BROWSER states on a request it sends from the page's own origin, and what it
/// states on one it sends from another site.
///
/// The emulator's front door refuses a request whose browser-set headers say a browser sent
/// it from another site before any service sees it — a property of this emulator rather than
/// of CloudFront — so the rows that go through the ROUTER state the same-origin
/// classification beside their `Origin`, and the suite that dials a distribution's own
/// address states the cross-site one, that address being where a browser's classification is
/// read.
const SAME_ORIGIN: (&str, &str) = ("sec-fetch-site", "same-origin");
const CROSS_SITE: (&str, &str) = ("sec-fetch-site", "cross-site");

/// The header a browser asks a preflight with.
const ASKS_GET: (&str, &str) = ("access-control-request-method", "GET");

// ---------------------------------------------------------------------------
// The configurations under test
// ---------------------------------------------------------------------------

/// A policy naming two origins, with every CORS member a configuration can carry.
fn named_origins_policy(name: &str, origin_override: bool) -> Policy {
    Policy {
        case: "a policy naming its origins",
        name: name.to_string(),
        cors: Some(Cors {
            origins: vec![GRANTED.to_string(), SECOND_GRANTED.to_string()],
            headers: vec!["content-type".to_string()],
            methods: vec!["GET".to_string(), "OPTIONS".to_string()],
            credentials: true,
            expose: Some(vec!["x-amz-cf-id".to_string()]),
            max_age: Some(600),
            origin_override,
        }),
        ..Policy::default()
    }
}

/// A policy whose lists carry the wildcard entry instead of naming values.
fn wildcard_policy(name: &str) -> Policy {
    Policy {
        case: "a policy carrying the wildcard",
        name: name.to_string(),
        cors: Some(Cors {
            origins: vec!["*".to_string()],
            headers: vec!["*".to_string()],
            // The entry that stands for every method the member's value set names.
            methods: vec!["ALL".to_string()],
            credentials: false,
            expose: None,
            max_age: None,
            origin_override: false,
        }),
        ..Policy::default()
    }
}

/// A policy with no CORS group at all: every security header, a custom header, and the
/// removal of a header the origin states.
fn header_groups_policy(name: &str) -> Policy {
    Policy {
        case: "a policy carrying the other groups",
        name: name.to_string(),
        security: Some(Security {
            xss: Some(Xss {
                r#override: true,
                protection: true,
                mode_block: Some(true),
                report_uri: None,
            }),
            frame: Some((true, "DENY".to_string())),
            referrer: Some((true, "same-origin".to_string())),
            csp: Some((true, "default-src 'self'".to_string())),
            content_type: Some(true),
            hsts: Some(Hsts {
                r#override: true,
                include_subdomains: Some(true),
                preload: Some(true),
                max_age: 31_536_000,
            }),
        }),
        custom_headers: Some(vec![(
            "x-made-by".to_string(),
            "the-policy".to_string(),
            true,
        )]),
        remove_headers: Some(vec!["content-type".to_string()]),
        ..Policy::default()
    }
}

/// A policy whose CUSTOM header names one the origin states, so the row decides whether the
/// policy's value wins.
fn overriding_custom_header_policy(name: &str, overrides: bool) -> Policy {
    Policy {
        case: "a policy restating a header the origin sent",
        name: name.to_string(),
        custom_headers: Some(vec![(
            "content-type".to_string(),
            "text/plain".to_string(),
            overrides,
        )]),
        ..Policy::default()
    }
}

/// One `DistributionConfig`: one origin this emulator serves, and a default behaviour that
/// allows the read-only methods and attaches `policy`.
fn config(caller_reference: &str, policy: &str) -> String {
    format!(
        "<DistributionConfig xmlns=\"http://cloudfront.amazonaws.com/doc/{API_VERSION}/\">\
         <CallerReference>{caller_reference}</CallerReference>\
         <Origins><Quantity>1</Quantity><Items><Origin>\
         <Id>primary</Id><DomainName>{SERVED_ORIGIN}</DomainName>\
         <CustomOriginConfig><HTTPPort>80</HTTPPort><HTTPSPort>443</HTTPSPort>\
         <OriginProtocolPolicy>https-only</OriginProtocolPolicy></CustomOriginConfig>\
         </Origin></Items></Origins>\
         <DefaultCacheBehavior><TargetOriginId>primary</TargetOriginId>\
         <ViewerProtocolPolicy>allow-all</ViewerProtocolPolicy>\
         <AllowedMethods><Quantity>3</Quantity>\
         <Items><Method>GET</Method><Method>HEAD</Method><Method>OPTIONS</Method></Items>\
         <CachedMethods><Quantity>2</Quantity>\
         <Items><Method>GET</Method><Method>HEAD</Method></Items></CachedMethods>\
         </AllowedMethods>\
         <CachePolicyId>{CACHING_DISABLED}</CachePolicyId>\
         <ResponseHeadersPolicyId>{policy}</ResponseHeadersPolicyId>\
         </DefaultCacheBehavior>\
         <Comment>{caller_reference}</Comment>\
         <Enabled>true</Enabled>\
         </DistributionConfig>"
    )
}

/// Create `policy`, attach it to a new distribution, and report the address that
/// distribution publishes once the clock has deployed it.
async fn given_distribution(router: &MultiServiceRouter, reference: &str, policy: Policy) -> String {
    let created = create_policy(router, &policy.xml()).await;
    assert_eq!(
        created.status, 201,
        "creating the policy {:?} answered {}: {}",
        policy.name, created.status, created.raw
    );
    let policy_id = created
        .root()
        .find_text("Id")
        .expect("the created policy reports an id")
        .to_string();
    let created = create_distribution(router, &config(reference, &policy_id)).await;
    assert_eq!(
        created.status, 201,
        "creating the distribution {reference:?} answered {}: {}",
        created.status, created.raw
    );
    let id = created
        .root()
        .find_text("Id")
        .expect("the created distribution reports an id")
        .to_string();
    let domain = created
        .root()
        .find_text("DomainName")
        .expect("the created distribution reports a domain name")
        .to_string();
    assert!(
        is_reachable_authority(&domain),
        "case {reference}: expected an address a client can dial, got {domain:?}"
    );
    // A configuration that has not reached the edge locations serves no viewer.
    awaited_deployment(router, reference, &id).await;
    domain
}

// ---------------------------------------------------------------------------
// Sending one viewer request
// ---------------------------------------------------------------------------

/// What the edge answered one viewer request with: every header, so a rule about the headers
/// a policy writes and removes is read off the answer itself.
struct Served {
    status: u16,
    headers: BTreeMap<String, String>,
    body: String,
}

impl Served {
    /// One header of the answer, or `None` when it carries none under that name.
    fn header(&self, name: &str) -> Option<&str> {
        self.headers.get(name).map(String::as_str)
    }
}

/// Send one VIEWER request through the router: no AWS credential, addressed to `host` —
/// which is what resolves the distribution — carrying `headers`.
async fn view(
    router: &MultiServiceRouter,
    method: &str,
    host: &str,
    headers: &[(&str, &str)],
) -> Served {
    let mut builder = ::http::Request::builder()
        .method(method)
        .uri(SERVED_PATH)
        .header("host", host);
    for (name, value) in headers {
        builder = builder.header(*name, *value);
    }
    let request = builder
        .body(Bytes::new())
        .expect("the test request is well formed");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let headers = response
        .headers()
        .iter()
        .map(|(name, value)| {
            (
                name.as_str().to_ascii_lowercase(),
                value.to_str().unwrap_or_default().to_string(),
            )
        })
        .collect();
    let body = String::from_utf8_lossy(&response.into_body()).to_string();
    Served {
        status,
        headers,
        body,
    }
}

/// One row: a viewer request, and what the answer must carry.
struct Row<'r> {
    case: &'r str,
    /// The address of the distribution it addresses.
    domain: &'r str,
    method: &'r str,
    headers: &'r [(&'r str, &'r str)],
    /// The status the answer must have, or `None` where the row is only about the headers —
    /// the status being the ORIGIN's for a request the edge passed to it.
    status: Option<u16>,
    /// The headers the answer must carry, with these values.
    stated: &'r [(&'r str, &'r str)],
    /// The headers the answer must not carry at all.
    absent: &'r [&'r str],
    /// What the answer's body must name.
    names: Option<&'r str>,
}

/// Drive every row and report what it got.
async fn assert_rows(router: &MultiServiceRouter, rows: &[Row<'_>]) {
    for row in rows {
        let answer = view(router, row.method, row.domain, row.headers).await;
        if let Some(status) = row.status {
            assert_eq!(
                answer.status, status,
                "case {:?}: expected status {status}, got {} (headers {:?}, body {:?})",
                row.case, answer.status, answer.headers, answer.body
            );
        }
        for (name, value) in row.stated {
            assert_eq!(
                answer.header(name),
                Some(*value),
                "case {:?}: expected {name:?} to be {value:?}, got {:?} (every header {:?})",
                row.case,
                answer.header(name),
                answer.headers
            );
        }
        for name in row.absent {
            assert_eq!(
                answer.header(name),
                None,
                "case {:?}: expected no {name:?} at all, got {:?} (every header {:?})",
                row.case,
                answer.header(name),
                answer.headers
            );
        }
        if let Some(named) = row.names {
            assert!(
                answer.body.contains(named),
                "case {:?}: expected the answer to name {named:?}, got {:?}",
                row.case,
                answer.body
            );
        }
    }
}

// ---------------------------------------------------------------------------
// The CORS group
// ---------------------------------------------------------------------------

/// The CORS group of the policy the behaviour attaches decides what each class of viewer
/// request is answered with — and refuses nothing: every row below is SERVED.
#[tokio::test]
async fn the_cors_group_decorates_the_answer_for_an_origin_the_policy_grants() {
    let router = emulator();
    let named = given_distribution(
        &router,
        "rhp-edge-named",
        named_origins_policy("rhp-edge-named", true),
    )
    .await;
    let wildcard = given_distribution(
        &router,
        "rhp-edge-wildcard",
        wildcard_policy("rhp-edge-wildcard"),
    )
    .await;

    let allow_origin = "access-control-allow-origin";
    let allow_methods = "access-control-allow-methods";
    let allow_headers = "access-control-allow-headers";
    let allow_credentials = "access-control-allow-credentials";
    let expose_headers = "access-control-expose-headers";
    let max_age = "access-control-max-age";

    let rows: &[Row<'_>] = &[
        Row {
            case: "an actual GET from an origin the policy names",
            domain: &named,
            method: "GET",
            headers: &[("origin", GRANTED), SAME_ORIGIN],
            status: Some(200),
            stated: &[
                (allow_origin, GRANTED),
                (expose_headers, "x-amz-cf-id"),
                (allow_credentials, "true"),
            ],
            // What a preflight asked about is not on an actual request's answer.
            absent: &[allow_methods, allow_headers, max_age],
            names: Some("DistributionList"),
        },
        Row {
            case: "an actual GET from the second origin the policy names",
            domain: &named,
            method: "GET",
            headers: &[("origin", SECOND_GRANTED), SAME_ORIGIN],
            status: Some(200),
            stated: &[(allow_origin, SECOND_GRANTED)],
            absent: &[],
            names: Some("DistributionList"),
        },
        Row {
            case: "an actual HEAD from an origin the policy names",
            domain: &named,
            method: "HEAD",
            headers: &[("origin", GRANTED), SAME_ORIGIN],
            // The status is the ORIGIN's, which answers a HEAD of this path as it answers
            // it: what the row is about is the group on the answer.
            status: None,
            stated: &[(allow_origin, GRANTED)],
            absent: &[allow_methods],
            names: None,
        },
        Row {
            case: "an actual GET from an origin the policy does not name is served without \
                   the group",
            domain: &named,
            method: "GET",
            headers: &[("origin", FOREIGN), SAME_ORIGIN],
            status: Some(200),
            stated: &[],
            absent: &[
                allow_origin,
                allow_methods,
                allow_headers,
                allow_credentials,
                expose_headers,
                max_age,
            ],
            names: Some("DistributionList"),
        },
        Row {
            case: "a request carrying no origin at all is no CORS request",
            domain: &named,
            method: "GET",
            headers: &[],
            status: Some(200),
            stated: &[],
            absent: &[allow_origin, allow_credentials, expose_headers],
            names: Some("DistributionList"),
        },
        Row {
            case: "a preflight from an origin the policy names",
            domain: &named,
            method: "OPTIONS",
            headers: &[("origin", GRANTED), ASKS_GET, SAME_ORIGIN],
            // The status is the one the ORIGIN answered the preflight with: the policy
            // decorates the answer, it does not produce it.
            status: None,
            stated: &[
                (allow_origin, GRANTED),
                (allow_methods, "GET, OPTIONS"),
                (allow_headers, "content-type"),
                (max_age, "600"),
                (allow_credentials, "true"),
            ],
            // What an actual request's answer may be read for is not a preflight's answer.
            absent: &[expose_headers],
            names: None,
        },
        Row {
            case: "an OPTIONS request that asks about no method is not a preflight",
            domain: &named,
            method: "OPTIONS",
            headers: &[("origin", GRANTED), SAME_ORIGIN],
            status: None,
            stated: &[(allow_origin, GRANTED), (expose_headers, "x-amz-cf-id")],
            absent: &[allow_methods, allow_headers, max_age],
            names: None,
        },
        Row {
            case: "the wildcard entry grants every origin and is the value itself",
            domain: &wildcard,
            method: "GET",
            headers: &[("origin", FOREIGN), SAME_ORIGIN],
            status: Some(200),
            stated: &[(allow_origin, "*")],
            // The configuration asks for no credentials and lists no exposed headers.
            absent: &[allow_credentials, expose_headers],
            names: Some("DistributionList"),
        },
        Row {
            case: "a preflight under the wildcard lists every method the member names",
            domain: &wildcard,
            method: "OPTIONS",
            headers: &[("origin", FOREIGN), ASKS_GET, SAME_ORIGIN],
            status: None,
            stated: &[
                (allow_origin, "*"),
                (allow_methods, "DELETE, GET, HEAD, OPTIONS, PATCH, POST, PUT"),
                (allow_headers, "*"),
            ],
            // The configuration states no max age.
            absent: &[max_age],
            names: None,
        },
        Row {
            case: "a method the behaviour does not allow is refused by CloudFront itself",
            domain: &named,
            method: "POST",
            headers: &[("origin", GRANTED), SAME_ORIGIN],
            status: Some(403),
            stated: &[("x-cache", "Error from cloudfront")],
            absent: &[],
            names: Some(METHOD_REFUSED),
        },
    ];
    assert_rows(&router, rows).await;
}

// ---------------------------------------------------------------------------
// The other groups
// ---------------------------------------------------------------------------

/// The security headers, the custom headers and the removals the policy configures are on
/// the answer, and the `Override` of a header decides against the one the origin sent.
#[tokio::test]
async fn the_configured_headers_are_written_and_the_removed_ones_are_gone() {
    let router = emulator();
    let groups = given_distribution(
        &router,
        "rhp-edge-groups",
        header_groups_policy("rhp-edge-groups"),
    )
    .await;
    let overriding = given_distribution(
        &router,
        "rhp-edge-override",
        overriding_custom_header_policy("rhp-edge-override", true),
    )
    .await;
    let yielding = given_distribution(
        &router,
        "rhp-edge-yield",
        overriding_custom_header_policy("rhp-edge-yield", false),
    )
    .await;

    let rows: &[Row<'_>] = &[
        Row {
            case: "every security header the configuration sets, and its custom header",
            domain: &groups,
            method: "GET",
            headers: &[],
            status: Some(200),
            stated: &[
                ("x-xss-protection", "1; mode=block"),
                ("x-frame-options", "DENY"),
                ("referrer-policy", "same-origin"),
                ("content-security-policy", "default-src 'self'"),
                ("x-content-type-options", "nosniff"),
                (
                    "strict-transport-security",
                    "max-age=31536000; includeSubDomains; preload",
                ),
                ("x-made-by", "the-policy"),
            ],
            // The header the removal names is gone, and a policy with no CORS group states
            // none of its headers.
            absent: &["content-type", "access-control-allow-origin"],
            names: Some("DistributionList"),
        },
        Row {
            case: "a custom header that overrides the one the origin sent",
            domain: &overriding,
            method: "GET",
            headers: &[],
            status: Some(200),
            stated: &[("content-type", "text/plain")],
            absent: &[],
            names: Some("DistributionList"),
        },
    ];
    assert_rows(&router, rows).await;

    // And the same header, configured not to override: the origin's own value stands. Read
    // as its own case because what it must NOT be is the policy's value, whatever the origin
    // happens to state.
    let answer = view(&router, "GET", &yielding, &[]).await;
    let content_type = answer.header("content-type").unwrap_or_default().to_string();
    assert!(
        !content_type.is_empty() && content_type != "text/plain",
        "case a custom header that does not override: expected the origin's own content type, \
         got {content_type:?} (every header {:?})",
        answer.headers
    );
}

// ---------------------------------------------------------------------------
// The address a viewer dials
// ---------------------------------------------------------------------------

/// What the address a distribution PUBLISHES does with a request a browser sends from another
/// site: it serves the ones whose `Origin` the distribution's own configuration NAMES — which
/// is what makes the policy's CORS verdict observable where a viewer reaches it — and refuses
/// every other browser request from another site before the edge, which is this emulator
/// guarding the machine it runs on rather than anything CloudFront does. A client that is no
/// browser reaches the edge as it always did.
#[tokio::test]
async fn the_published_address_serves_the_origins_the_distribution_names() {
    let router = emulator();
    let domain = given_distribution(
        &router,
        "rhp-edge-dialled",
        named_origins_policy("rhp-edge-dialled", true),
    )
    .await;

    // The rows differ along the origin (named, the second named one, not named, none at all)
    // and along the method, a preflight included.
    let granted_header = format!("access-control-allow-origin: {GRANTED}");
    let second_header = format!("access-control-allow-origin: {SECOND_GRANTED}");
    let cases: [Dialled<'_>; 7] = [
        Dialled {
            case: "a cross-site GET from an origin the policy names",
            method: "GET",
            headers: &[("origin", GRANTED), CROSS_SITE],
            contains: &["HTTP/1.1 200", "DistributionList", &granted_header],
            absent: &[CROSS_SITE_REFUSED],
        },
        Dialled {
            case: "a cross-site GET from the second origin the policy names",
            method: "GET",
            headers: &[("origin", SECOND_GRANTED), CROSS_SITE],
            contains: &["HTTP/1.1 200", "DistributionList", &second_header],
            absent: &[CROSS_SITE_REFUSED],
        },
        Dialled {
            case: "a cross-site preflight from an origin the policy names",
            method: "OPTIONS",
            headers: &[("origin", GRANTED), ASKS_GET, CROSS_SITE],
            contains: &[
                "access-control-allow-methods: GET, OPTIONS",
                "access-control-max-age: 600",
            ],
            absent: &[CROSS_SITE_REFUSED],
        },
        Dialled {
            case: "a cross-site GET from an origin the policy does not name is refused by the \
                   address",
            method: "GET",
            headers: &[("origin", FOREIGN), CROSS_SITE],
            contains: &["HTTP/1.1 403", CROSS_SITE_REFUSED],
            absent: &["DistributionList", "access-control-allow-origin"],
        },
        Dialled {
            case: "a cross-site POST from an origin the policy names reaches the edge",
            method: "POST",
            headers: &[("origin", GRANTED), CROSS_SITE],
            contains: &["HTTP/1.1 403", METHOD_REFUSED],
            absent: &[CROSS_SITE_REFUSED],
        },
        Dialled {
            case: "a cross-site POST from an origin the policy does not name is refused by \
                   the address",
            method: "POST",
            headers: &[("origin", FOREIGN), CROSS_SITE],
            contains: &["HTTP/1.1 403", CROSS_SITE_REFUSED],
            absent: &[METHOD_REFUSED],
        },
        Dialled {
            case: "a GET from a client that is no browser is served as it always was",
            method: "GET",
            headers: &[],
            contains: &["HTTP/1.1 200", "DistributionList"],
            absent: &[CROSS_SITE_REFUSED, "access-control-allow-origin"],
        },
    ];

    for row in cases {
        let answer = dial_with(&domain, row.method, SERVED_PATH, &domain, row.headers).await;
        let lowered = answer.to_ascii_lowercase();
        let case = row.case;
        for expected in row.contains {
            assert!(
                lowered.contains(&expected.to_ascii_lowercase()),
                "case {case:?}: expected the answer to carry {expected:?}, got {answer:?}"
            );
        }
        for unexpected in row.absent {
            assert!(
                !lowered.contains(&unexpected.to_ascii_lowercase()),
                "case {case:?}: expected no {unexpected:?} in the answer, got {answer:?}"
            );
        }
    }
}

/// One row of the suite above: a request DIALLED at the address the distribution published,
/// and what the answer that address wrote back must and must not carry.
struct Dialled<'r> {
    case: &'r str,
    method: &'r str,
    /// The headers the browser set on it.
    headers: &'r [(&'r str, &'r str)],
    /// What the whole answer — status line, headers and body — must contain, compared
    /// without case, as a header name is.
    contains: &'r [&'r str],
    /// What it must not contain at all.
    absent: &'r [&'r str],
}
