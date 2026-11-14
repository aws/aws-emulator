// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for the S3 WEBSITE ENDPOINT: the surface a bucket configured for static
//! website hosting is served on.
//!
//! RULE: a request addressed to a bucket's website host — `<bucket>.s3-website-<Region>.…`,
//! which is a different endpoint from the REST API's — is served from that bucket's website
//! configuration: a request addressing a directory gets that directory's `IndexDocument`
//! suffix, a key the public may read gets its bytes, a key it may read that is not there gets
//! the `ErrorDocument` body with HTTP 404, content the bucket does not make public gets 403, an
//! object carrying `x-amz-website-redirect-location` gets a 301 to it, a `RoutingRule` whose
//! `Condition` holds gets the redirect that rule names, and any method but a read gets 405.
//! The REST endpoint is untouched: a missing key there is still `NoSuchKey`, whatever routing
//! rules the bucket carries.
//!
//! Every request here is ANONYMOUS and goes over the wire through the router, exactly as a
//! browser addressing a static site makes it, so what is pinned is the HTTP behaviour (status,
//! `Location`, body, media type) rather than any handler's internals. The suites are TABLES
//! whose rows differ along what the rule generalises over: which path is addressed (the root, a
//! directory, a key, a missing key, a private key), which method is used, which condition a
//! routing rule carries and which replacement it names.

use smithy_server_runtime::MultiServiceRouter;

use super::bucket_lifecycle::{router, send_bytes, Answer};
use super::object_and_tagging_roundtrip::{bucket, put_object, text};

/// The Region the website host below names, which is the one this crate's suites address.
const REGION: &str = "us-east-1";

/// The media type the HTML objects below are stored under, and the one a served object is
/// answered with.
const HTML: &str = "text/html";

/// The website host of `bucket`: the older hyphenated form of the two Amazon S3 publishes
/// (`<bucket>.s3-website-<Region>.amazonaws.com`), so the suite also pins that both are read.
fn website_host(bucket: &str) -> String {
    format!("{bucket}.s3-website-{REGION}.amazonaws.com")
}

/// The dotted form of the same endpoint (`<bucket>.s3-website.<Region>.amazonaws.com`).
fn dotted_website_host(bucket: &str) -> String {
    format!("{bucket}.s3-website.{REGION}.amazonaws.com")
}

/// One ANONYMOUS request to a website host: no `Authorization` header at all, which is what a
/// browser sends.
async fn website(
    router: &MultiServiceRouter,
    method: &str,
    host: &str,
    path: &str,
) -> Answer {
    let request = http::Request::builder()
        .method(method)
        .uri(format!("http://{host}{path}"))
        .header("host", host)
        .body(bytes::Bytes::new())
        .expect("the test request must build");
    let response = router.dispatch(request).await;
    let (parts, body) = response.into_parts();
    Answer {
        status: parts.status.as_u16(),
        headers: parts.headers,
        body,
    }
}

/// The bucket policy that publishes the site: `s3:GetObject` on every object to every
/// principal — "a bucket policy statement that allows `s3:GetObject` on this object to every
/// principal", which is how a static site is published — with one key held back by an explicit
/// `Deny`, so the suite spans both directions of the decision.
fn site_policy(bucket: &str) -> String {
    format!(
        "{{\"Version\":\"2012-10-17\",\"Statement\":[\
         {{\"Sid\":\"PublicRead\",\"Effect\":\"Allow\",\"Principal\":\"*\",\
         \"Action\":\"s3:GetObject\",\"Resource\":\"arn:aws:s3:::{bucket}/*\"}},\
         {{\"Sid\":\"KeepOnePrivate\",\"Effect\":\"Deny\",\"Principal\":\"*\",\
         \"Action\":\"s3:GetObject\",\"Resource\":\"arn:aws:s3:::{bucket}/private.html\"}}]}}"
    )
}

/// `PUT /<bucket>?publicAccessBlock` — `PutPublicAccessBlock` with every setting OFF, which is
/// the step a static site needs before its policy can be public: "public policies are prevented
/// by the BlockPublicPolicy setting in S3 Block Public Access" until it is turned off.
async fn allow_public_policies(router: &MultiServiceRouter, bucket: &str) {
    let body = "<PublicAccessBlockConfiguration>\
         <BlockPublicAcls>false</BlockPublicAcls>\
         <IgnorePublicAcls>false</IgnorePublicAcls>\
         <BlockPublicPolicy>false</BlockPublicPolicy>\
         <RestrictPublicBuckets>false</RestrictPublicBuckets>\
         </PublicAccessBlockConfiguration>";
    let answered = send_bytes(
        router,
        "PUT",
        &format!("/{bucket}?publicAccessBlock"),
        &[("content-type", "application/xml")],
        body.as_bytes(),
    )
    .await;
    assert_eq!(
        answered.status, 200,
        "the public access block must be accepted, got {} with {:?}",
        answered.status,
        text(&answered)
    );
}

/// `PUT /<bucket>?policy` — `PutBucketPolicy`.
async fn put_policy(router: &MultiServiceRouter, bucket: &str, document: &str) {
    let answered = send_bytes(
        router,
        "PUT",
        &format!("/{bucket}?policy"),
        &[("content-type", "application/json")],
        document.as_bytes(),
    )
    .await;
    assert_eq!(
        answered.status, 204,
        "the bucket policy must be accepted, got {} with {:?}",
        answered.status,
        text(&answered)
    );
}

/// `PUT /<bucket>?website` — `PutBucketWebsite` with the index and error documents, plus
/// whatever `extra` the row adds (a `RoutingRules` container).
async fn put_website(router: &MultiServiceRouter, bucket: &str, extra: &str) {
    let body = format!(
        "<WebsiteConfiguration><IndexDocument><Suffix>index.html</Suffix></IndexDocument>\
         <ErrorDocument><Key>error.html</Key></ErrorDocument>{extra}</WebsiteConfiguration>"
    );
    let answered = send_bytes(
        router,
        "PUT",
        &format!("/{bucket}?website"),
        &[("content-type", "application/xml")],
        body.as_bytes(),
    )
    .await;
    assert_eq!(
        answered.status, 200,
        "the website configuration must be accepted, got {} with {:?}",
        answered.status,
        text(&answered)
    );
}

/// The keys every served bucket below holds, each with the body it serves.
const INDEX_BODY: &str = "<html>index</html>";
/// See [`INDEX_BODY`].
const SUB_INDEX_BODY: &str = "<html>sub index</html>";
/// See [`INDEX_BODY`].
const ERROR_BODY: &str = "<html>error</html>";
/// See [`INDEX_BODY`].
const PAGE_BODY: &str = "<html>page</html>";
/// See [`INDEX_BODY`].
const PRIVATE_BODY: &str = "<html>private</html>";

/// Stand up a bucket serving a static site: the objects, the policy that makes all but
/// `private.html` public, and the website configuration (plus `extra`).
async fn site(router: &MultiServiceRouter, name: &str, extra: &str) {
    bucket(router, name).await;
    for (key, body) in [
        ("index.html", INDEX_BODY),
        ("sub/index.html", SUB_INDEX_BODY),
        ("error.html", ERROR_BODY),
        ("page.html", PAGE_BODY),
        ("documents/guide.html", PAGE_BODY),
        ("private.html", PRIVATE_BODY),
    ] {
        let written = put_object(
            router,
            name,
            key,
            body.as_bytes(),
            &[("content-type", HTML)],
        )
        .await;
        assert_eq!(
            written.status, 200,
            "{key:?} must be written first, got {} with {:?}",
            written.status,
            text(&written)
        );
    }
    allow_public_policies(router, name).await;
    put_policy(router, name, &site_policy(name)).await;
    put_website(router, name, extra).await;
}

/// One row of the served-request table: the method and path addressed, and what the endpoint
/// must answer.
struct Row {
    case: &'static str,
    method: &'static str,
    path: &'static str,
    status: u16,
    /// The body the answer must carry, or `None` where the row does not pin one.
    body: Option<&'static str>,
}

fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "the root, which addresses the index document",
            method: "GET",
            path: "/",
            status: 200,
            body: Some(INDEX_BODY),
        },
        Row {
            case: "a directory, which addresses the index document inside it",
            method: "GET",
            path: "/sub/",
            status: 200,
            body: Some(SUB_INDEX_BODY),
        },
        Row {
            case: "a public key",
            method: "GET",
            path: "/page.html",
            status: 200,
            body: Some(PAGE_BODY),
        },
        Row {
            case: "a HEAD of a public key, which carries no body",
            method: "HEAD",
            path: "/page.html",
            status: 200,
            body: Some(""),
        },
        Row {
            case: "a key the public may read that is not there",
            method: "GET",
            path: "/missing.html",
            status: 404,
            body: Some(ERROR_BODY),
        },
        Row {
            case: "a directory with no index document in it",
            method: "GET",
            path: "/empty/",
            status: 404,
            body: Some(ERROR_BODY),
        },
        Row {
            case: "content the bucket does not make public",
            method: "GET",
            path: "/private.html",
            status: 403,
            body: None,
        },
        Row {
            case: "a write, which the website endpoint does not serve",
            method: "PUT",
            path: "/page.html",
            status: 405,
            body: None,
        },
        Row {
            case: "a delete, which it does not serve either",
            method: "DELETE",
            path: "/page.html",
            status: 405,
            body: None,
        },
        Row {
            case: "a POST",
            method: "POST",
            path: "/",
            status: 405,
            body: None,
        },
    ]
}

#[tokio::test]
async fn the_website_endpoint_serves_the_bucket_its_configuration_describes() {
    let router = router();
    let name = "website-endpoint-site";
    site(&router, name, "").await;

    // Both published spellings of the endpoint reach the same surface.
    for host in [website_host(name), dotted_website_host(name)] {
        for row in rows() {
            let answered = website(&router, row.method, &host, row.path).await;
            assert_eq!(
                answered.status, row.status,
                "case {:?} on {host:?}: expected HTTP {}, got {} with body {:?}",
                row.case,
                row.status,
                answered.status,
                text(&answered)
            );
            if let Some(expected) = row.body {
                assert_eq!(
                    text(&answered),
                    expected,
                    "case {:?} on {host:?}: expected the body {expected:?}, got {:?}",
                    row.case,
                    text(&answered)
                );
            }
            // A HEAD sends no body and still reports the length its GET would carry, which is
            // what a client sizing the object reads.
            if row.method == "HEAD" {
                assert_eq!(
                    answered.header("content-length").as_deref(),
                    Some(PAGE_BODY.len().to_string().as_str()),
                    "case {:?}: expected the entity length on a HEAD, got {:?}",
                    row.case,
                    answered.header("content-length")
                );
            }
            // Nothing this surface serves carries a `Location`, which is what a redirect is.
            if row.status == 200 {
                assert_eq!(
                    answered.header("location"),
                    None,
                    "case {:?}: a served object carries no Location, got {:?}",
                    row.case,
                    answered.header("location")
                );
                assert_eq!(
                    answered.header("content-type").as_deref(),
                    Some(HTML),
                    "case {:?}: expected the media type the object is stored under, got {:?}",
                    row.case,
                    answered.header("content-type")
                );
            }
        }
    }
}

#[tokio::test]
async fn an_object_carrying_a_redirect_location_is_answered_as_a_redirect() {
    let router = router();
    let name = "website-endpoint-redirect";
    site(&router, name, "").await;
    // The object the website endpoint redirects FROM: its bytes are what the REST endpoint
    // still serves.
    let written = put_object(
        &router,
        name,
        "redirect.html",
        b"the bytes behind the redirect",
        &[
            ("content-type", HTML),
            ("x-amz-website-redirect-location", "/page.html"),
        ],
    )
    .await;
    assert_eq!(
        written.status, 200,
        "the redirecting object must be written, got {} with {:?}",
        written.status,
        text(&written)
    );

    let host = website_host(name);
    let answered = website(&router, "GET", &host, "/redirect.html").await;
    assert_eq!(
        answered.status, 301,
        "expected the website endpoint to answer a redirect, got {} with {:?}",
        answered.status,
        text(&answered)
    );
    assert_eq!(
        answered.header("location").as_deref(),
        Some("/page.html"),
        "expected the Location the object carries, got {:?}",
        answered.header("location")
    );

    // The REST endpoint serves the BODY of the same object: the redirect is the website
    // endpoint's reading of that member, not a property of the object plane.
    let read = send_bytes(&router, "GET", &format!("/{name}/redirect.html"), &[], b"").await;
    assert_eq!(
        read.status, 200,
        "expected the REST endpoint to serve the object, got {} with {:?}",
        read.status,
        text(&read)
    );
    assert_eq!(
        text(&read),
        "the bytes behind the redirect",
        "expected the REST endpoint to serve the stored bytes, got {:?}",
        text(&read)
    );
}

/// The routing rules the suite below configures: one conditioned on the ERROR a request earns,
/// one on the KEY PREFIX it addresses.
const ROUTING_RULES: &str = "<RoutingRules>\
     <RoutingRule>\
     <Condition><HttpErrorCodeReturnedEquals>404</HttpErrorCodeReturnedEquals></Condition>\
     <Redirect><HttpRedirectCode>302</HttpRedirectCode>\
     <ReplaceKeyWith>fallback.html</ReplaceKeyWith></Redirect>\
     </RoutingRule>\
     <RoutingRule>\
     <Condition><KeyPrefixEquals>docs/</KeyPrefixEquals></Condition>\
     <Redirect><ReplaceKeyPrefixWith>documents/</ReplaceKeyPrefixWith></Redirect>\
     </RoutingRule>\
     </RoutingRules>";

/// One row of the routing table: the path addressed, and the status and `Location` the rules
/// call for (`None` for a request no rule redirects).
struct RoutingRow {
    case: &'static str,
    path: &'static str,
    status: u16,
    location: Option<&'static str>,
}

fn routing_rows() -> Vec<RoutingRow> {
    vec![
        RoutingRow {
            case: "a missing key, which the error-conditioned rule redirects",
            path: "/missing.html",
            status: 302,
            location: Some("/fallback.html"),
        },
        RoutingRow {
            case: "a key under the prefix, which the prefix rule redirects",
            path: "/docs/guide.html",
            status: 301,
            location: Some("/documents/guide.html"),
        },
        RoutingRow {
            case: "a deeper key under the prefix",
            path: "/docs/sub/guide.html",
            status: 301,
            location: Some("/documents/sub/guide.html"),
        },
        RoutingRow {
            case: "an existing key no rule names, which is served",
            path: "/page.html",
            status: 200,
            location: None,
        },
        RoutingRow {
            case: "the root, which is still the index document",
            path: "/",
            status: 200,
            location: None,
        },
    ]
}

#[tokio::test]
async fn the_routing_rules_redirect_the_requests_their_conditions_name() {
    let router = router();
    let name = "website-endpoint-routing";
    site(&router, name, ROUTING_RULES).await;
    let host = website_host(name);

    for row in routing_rows() {
        let answered = website(&router, "GET", &host, row.path).await;
        assert_eq!(
            answered.status, row.status,
            "case {:?}: expected HTTP {}, got {} with body {:?}",
            row.case,
            row.status,
            answered.status,
            text(&answered)
        );
        let expected = row
            .location
            .map(|target| format!("http://{host}{target}"));
        assert_eq!(
            answered.header("location"),
            expected,
            "case {:?}: expected the Location {expected:?}, got {:?}",
            row.case,
            answered.header("location")
        );
    }

    // The REST endpoint ignores routing rules entirely: the same missing key is the object
    // plane's own refusal there.
    let read = send_bytes(&router, "GET", &format!("/{name}/missing.html"), &[], b"").await;
    assert_eq!(
        read.status, 404,
        "expected the REST endpoint to answer 404, got {} with {:?}",
        read.status,
        text(&read)
    );
    assert_eq!(
        read.error_code(),
        "NoSuchKey",
        "expected the REST endpoint's own refusal, got {:?} in {:?}",
        read.error_code(),
        text(&read)
    );
    assert_eq!(
        read.header("location"),
        None,
        "expected the REST endpoint to redirect nothing, got {:?}",
        read.header("location")
    );
}

#[tokio::test]
async fn a_bucket_with_no_website_configuration_serves_no_website() {
    let router = router();
    let name = "website-endpoint-unconfigured";
    bucket(&router, name).await;
    let answered = website(&router, "GET", &website_host(name), "/").await;
    assert_eq!(
        answered.status, 404,
        "expected 404 for a bucket that was never configured, got {} with {:?}",
        answered.status,
        text(&answered)
    );
    assert!(
        text(&answered).contains("NoSuchWebsiteConfiguration"),
        "expected the code for a bucket with no website configuration, got {:?}",
        text(&answered)
    );

    // And a website host naming no bucket at all answers the code for a bucket that does not
    // exist rather than anything of another surface.
    let absent = website(&router, "GET", &website_host("website-endpoint-absent"), "/").await;
    assert_eq!(
        absent.status, 404,
        "expected 404 for a bucket this emulator does not hold, got {} with {:?}",
        absent.status,
        text(&absent)
    );
    assert!(
        text(&absent).contains("NoSuchBucket"),
        "expected the code for a bucket that does not exist, got {:?}",
        text(&absent)
    );

    // A host whose second label merely BEGINS with the website label is not the website
    // endpoint, so nothing of that surface answers it: whatever the request then resolves to,
    // it is never this bucket's website.
    let near_miss = website(
        &router,
        "GET",
        &format!("{name}.s3-websitefoo.example.com"),
        "/",
    )
    .await;
    assert!(
        !text(&near_miss).contains("NoSuchWebsiteConfiguration"),
        "a host that is not a website endpoint must not answer the website refusal, got {:?}",
        text(&near_miss)
    );
}

#[tokio::test]
async fn a_bucket_whose_name_carries_dots_is_served_at_its_own_website_host() {
    // "A bucket name can consist of … lowercase letters, numbers, dots (.), and hyphens"
    // (Amazon S3 User Guide, *Bucket naming rules*), and such a bucket is served at
    // `<bucket>.s3-website-<Region>.amazonaws.com` like any other — so the bucket is
    // everything in front of the website label, not the host's first label alone. The rows
    // span the forms a name takes around that reading: several dot-separated parts, one dot,
    // a part that is only digits, and a plain name beside them.
    let rows: &[(&str, &str)] = &[
        ("several dotted parts", "website.dotted.example"),
        ("one dot", "website.dotted"),
        ("a numeric part", "website.2026.example"),
        ("no dot at all", "website-undotted-example"),
        // A name that itself carries a label of the website endpoint's form: the endpoint's
        // own label is the LAST one, because the Region and the DNS suffix follow it.
        ("a part shaped like the website label", "website.s3-website-us-east-1.site"),
    ];
    for (case, name) in rows {
        let router = router();
        site(&router, name, "").await;
        for (spelling, host) in [
            ("the hyphenated form", website_host(name)),
            ("the dotted form", dotted_website_host(name)),
            ("the root label's dot", format!("{}.", website_host(name))),
        ] {
            let served = website(&router, "GET", &host, "/").await;
            assert_eq!(
                served.status,
                200,
                "case {case:?} ({spelling}): the site of {name:?} must be served at {host:?}, \
                 got {} with {:?}",
                served.status,
                text(&served)
            );
            assert_eq!(
                text(&served),
                INDEX_BODY,
                "case {case:?} ({spelling}): expected the index document, got {:?}",
                text(&served)
            );
        }
    }
}
