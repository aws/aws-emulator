// SPDX-License-Identifier: CC0-1.0

//! A Route 53 string-match health check against an endpoint this emulator SERVES — a bucket
//! serving a static website — end to end over the assembled router.
//!
//! The rule under test: **what a Route 53 health checker reports about an endpoint-type health
//! check is the result of a PROBE of the endpoint the configuration names, re-made every
//! probe interval, and for `HTTP_STR_MATCH` / `HTTPS_STR_MATCH` the criterion that probe
//! applies is the documented case-sensitive search of the answered body for `SearchString`
//! ("the string that you want Amazon Route 53 to search for in the response body from the
//! specified resource. If the string appears in the response body, Route 53 considers the
//! resource healthy", and "Route 53 considers case when searching for SearchString").** So a
//! body that carries the string is reported by every checker as a `Success: `-prefixed
//! description, and the same endpoint serving a body that does not carry it is reported as a
//! `Failure: `-prefixed description naming the string match — with the health check itself
//! untouched: the same `HealthCheckVersion`, the same `SearchString`, no call made to Route 53
//! between the two reads.
//!
//! Two crates have to agree for that to hold (S3 serves the monitored endpoint, Route 53
//! probes it), so the suite lives here: a crate-local suite can only ever see one side of it.
//! The Route 53 crate's `health_check_endpoint_probe` suite pins what a checker reports for an
//! endpoint nothing serves, and `endpoint_probe`'s own tables pin the predicate over bodies
//! and statuses; what only this suite reaches is the whole path — an endpoint of this
//! emulator, answering a body a client rewrote, deciding what the checkers report.
//!
//! The phases differ along what the rule generalises over: whether the body carries the
//! string at all, where in the body it sits, its case and the case of the body, the string as
//! a prefix of a longer word, and a body rewritten back to carrying it — each phase a
//! `PutObject` on the SAME key with nothing about the health check changed. A second table
//! spans how the monitored endpoint is ADDRESSED and published, and a third that the path the
//! probe requests is the configured `ResourcePath`.
//!
//! Every call here, the checker's own probe included, carries no caller identity the endpoint
//! reads: a probe is an anonymous HTTP request, which is what makes what it observes exactly
//! what the monitored endpoint serves to everyone.
//!
//! The awsQuery/XML element reader is the test crate's own ([`crate::query_wire`]), not
//! another copy.

use std::time::{Duration, Instant};

use bytes::Bytes;
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_route53_awsdnsv20130401::state::endpoint_probe::STATUS_STRING_MATCH_FAILURE;
use service_com_amazonaws_route53_awsdnsv20130401::state::{
    STATUS_FAILURE_PREFIX, STATUS_SUCCESS_PREFIX,
};

use crate::query_wire::elements;

/// The Region every call here is signed for, and so the Region of the bucket and of the
/// website host it is served on.
const REGION: &str = "us-west-2";

/// The XML namespace a client's S3 payload carries.
const S3_XMLNS: &str = "http://s3.amazonaws.com/doc/2006-03-01/";

/// The endpoint host a Route 53 request addresses, which is also the routing signal the
/// router resolves that service from.
const ROUTE53_HOST: &str = "route53.amazonaws.com";

/// The bucket serving the monitored site, the key the site's index document is at, and the
/// string the health check searches the answered body for.
const BUCKET: &str = "route53-probe-origin";
const INDEX_KEY: &str = "index.html";
const MARKER: &str = "MARKER";

/// The probe cadence the health checks below are created with: the fastest published
/// `RequestInterval` (modelled `range 10..30`) and the smallest `FailureThreshold`
/// (`range 1..10`), so one probe decides what the checkers report.
const REQUEST_INTERVAL: i64 = 10;
const FAILURE_THRESHOLD: i64 = 1;

/// How long a phase waits for the checkers to report the probe of the body it wrote, and how
/// often it looks.
///
/// Generous on purpose: a phase is waiting for the next probe of a check whose interval is
/// [`REQUEST_INTERVAL`] seconds, so the deadline is only here so a failure is a failure rather
/// than a hang.
const DEADLINE: Duration = Duration::from_secs(120);
const POLL: Duration = Duration::from_millis(100);

/// The whole emulator over one engine, exactly as the binary assembles it — so which service
/// claims each call, the checker's own probe of the website endpoint included, is decided by
/// the production mount rather than by a hand-picked set.
fn router() -> MultiServiceRouter {
    let engine = aws_emulator_router::new_engine();
    let mut router = aws_emulator_router::build_router(engine);
    // Signature verification is the front door's concern, not this suite's: what is under
    // test is what a checker's probe OBSERVES of an endpoint this emulator serves, and a
    // checker — like the browser a static website is served to — signs nothing.
    router.set_request_authentication(false);
    router
}

/// A SigV4 `Authorization` header whose credential scope names `service`, which is what
/// routes the request. The signature is never checked: request authentication is off.
fn authorization(service: &str) -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{REGION}/{service}/aws4_request, \
         SignedHeaders=host, Signature=not-verified"
    )
}

/// The website host of the monitored bucket: one of the two spellings Amazon S3 publishes
/// (`<bucket>.s3-website-<Region>.amazonaws.com`), which is the endpoint a static site is
/// served on.
fn website_host() -> String {
    format!("{BUCKET}.s3-website-{REGION}.amazonaws.com")
}

/// One answer: its status and its body as text.
struct Answer {
    status: u16,
    body: String,
}

/// Dispatch one request against S3's REST surface, path-style.
async fn s3(router: &MultiServiceRouter, method: &str, path: &str, body: &[u8]) -> Answer {
    let host = format!("s3.{REGION}.amazonaws.com");
    let request = http::Request::builder()
        .method(method)
        .uri(format!("https://{host}{path}"))
        .header("host", host)
        .header("authorization", authorization("s3"))
        .header("content-type", "text/html")
        .body(Bytes::from(body.to_vec()))
        .expect("a well-formed request");
    let response = router.dispatch(request).await;
    Answer {
        status: response.status().as_u16(),
        body: String::from_utf8_lossy(response.body()).into_owned(),
    }
}

/// One ANONYMOUS request to the bucket's WEBSITE host — the endpoint the health check below
/// monitors, fetched here exactly as a browser addressing a static site fetches it, and
/// exactly as a health checker's probe does: no `Authorization` header at all.
async fn website(router: &MultiServiceRouter, path: &str) -> Answer {
    website_at(router, &website_host(), path).await
}

/// Stand up the monitored site: the bucket, the public-access settings and bucket policy that
/// publish it, its website configuration, and the index document carrying `body`.
async fn publish_site(router: &MultiServiceRouter, body: &str) {
    let created = s3(
        router,
        "PUT",
        &format!("/{BUCKET}"),
        format!(
            "<CreateBucketConfiguration xmlns=\"{S3_XMLNS}\">\
             <LocationConstraint>{REGION}</LocationConstraint></CreateBucketConfiguration>"
        )
        .as_bytes(),
    )
    .await;
    assert_eq!(
        created.status, 200,
        "CreateBucket must succeed, got {} with {}",
        created.status, created.body
    );
    let unblocked = s3(
        router,
        "PUT",
        &format!("/{BUCKET}?publicAccessBlock"),
        b"<PublicAccessBlockConfiguration>\
          <BlockPublicAcls>false</BlockPublicAcls>\
          <IgnorePublicAcls>false</IgnorePublicAcls>\
          <BlockPublicPolicy>false</BlockPublicPolicy>\
          <RestrictPublicBuckets>false</RestrictPublicBuckets>\
          </PublicAccessBlockConfiguration>",
    )
    .await;
    assert_eq!(
        unblocked.status, 200,
        "PutPublicAccessBlock must succeed, got {} with {}",
        unblocked.status, unblocked.body
    );
    let policy = s3(
        router,
        "PUT",
        &format!("/{BUCKET}?policy"),
        format!(
            "{{\"Version\":\"2012-10-17\",\"Statement\":[\
             {{\"Sid\":\"PublicRead\",\"Effect\":\"Allow\",\"Principal\":\"*\",\
             \"Action\":\"s3:GetObject\",\"Resource\":\"arn:aws:s3:::{BUCKET}/*\"}}]}}"
        )
        .as_bytes(),
    )
    .await;
    assert_eq!(
        policy.status, 204,
        "PutBucketPolicy must succeed, got {} with {}",
        policy.status, policy.body
    );
    let website_config = s3(
        router,
        "PUT",
        &format!("/{BUCKET}?website"),
        format!(
            "<WebsiteConfiguration><IndexDocument><Suffix>{INDEX_KEY}</Suffix></IndexDocument>\
             </WebsiteConfiguration>"
        )
        .as_bytes(),
    )
    .await;
    assert_eq!(
        website_config.status, 200,
        "PutBucketWebsite must succeed, got {} with {}",
        website_config.status, website_config.body
    );
    serve_body(router, "the site as published", body).await;
}

/// Rewrite the index document so the site serves `body`, and prove through the website
/// endpoint that it is what the endpoint now answers.
async fn serve_body(router: &MultiServiceRouter, case: &str, body: &str) {
    let written = s3(
        router,
        "PUT",
        &format!("/{BUCKET}/{INDEX_KEY}"),
        body.as_bytes(),
    )
    .await;
    assert_eq!(
        written.status, 200,
        "case {case:?}: PutObject must succeed, got {} with {}",
        written.status, written.body
    );
    let served = website(router, "/").await;
    assert_eq!(
        (served.status, served.body.as_str()),
        (200, body),
        "case {case:?}: the website endpoint must serve the body that was just written, got \
         {} with {:?}",
        served.status,
        served.body
    );
}

/// Call one Route 53 operation over its restXml wire, requiring the status `expected`.
async fn route53(
    router: &MultiServiceRouter,
    case: &str,
    method: &str,
    uri: &str,
    body: &str,
    expected: u16,
) -> String {
    let request = http::Request::builder()
        .method(method)
        .uri(format!("https://{ROUTE53_HOST}{uri}"))
        .header("host", ROUTE53_HOST)
        .header("authorization", authorization("route53"))
        .header("content-type", "application/xml")
        .body(Bytes::from(body.to_owned()))
        .expect("a well-formed request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let read = String::from_utf8_lossy(response.body()).to_string();
    assert_eq!(
        status, expected,
        "case {case:?}: expected HTTP {expected} from Route 53 {method} {uri}, got {status} \
         with body {read}"
    );
    read
}

/// `CreateHealthCheck` with `config` as the body of its `HealthCheckConfig`, answering the id
/// it minted.
async fn create_health_check(
    router: &MultiServiceRouter,
    case: &str,
    caller_reference: &str,
    config: &str,
) -> String {
    let created = route53(
        router,
        case,
        "POST",
        "/2013-04-01/healthcheck",
        &format!(
            "<CreateHealthCheckRequest xmlns=\"https://route53.amazonaws.com/doc/2013-04-01/\">\
             <CallerReference>{caller_reference}</CallerReference>\
             <HealthCheckConfig>{config}</HealthCheckConfig></CreateHealthCheckRequest>"
        ),
        201,
    )
    .await;
    elements(&created, "Id")
        .into_iter()
        .next()
        .unwrap_or_else(|| panic!("case {case:?}: the create must report the new id, got {created}"))
}

/// The `StatusReport$Status` every checker of `id` is reporting, in wire order.
async fn reported_statuses(router: &MultiServiceRouter, case: &str, id: &str) -> Vec<String> {
    let read = route53(
        router,
        case,
        "GET",
        &format!("/2013-04-01/healthcheck/{id}/status"),
        "",
        200,
    )
    .await;
    elements(&read, "Status")
}

/// Wait until every checker of `id` reports a status opening with `prefix`, and answer the
/// statuses they report.
///
/// What is waited for is the CONDITION — the checkers reporting the probe of the body the
/// phase wrote — rather than a span of time: a phase is as long as the check's next probe
/// takes to come round.
async fn statuses_opening_with(
    router: &MultiServiceRouter,
    case: &str,
    id: &str,
    prefix: &str,
) -> Vec<String> {
    let deadline = Instant::now() + DEADLINE;
    loop {
        let reported = reported_statuses(router, case, id).await;
        let settled = !reported.is_empty()
            && reported
                .iter()
                .all(|status| status.starts_with(prefix));
        if settled || Instant::now() >= deadline {
            assert!(
                settled,
                "case {case:?}: expected every checker to report a status opening with \
                 {prefix:?}, got {reported:?}"
            );
            return reported;
        }
        ::tokio::time::sleep(POLL).await;
    }
}

/// The `HealthCheckVersion` and the `SearchString` the health check `id` carries, which no
/// rewrite of the monitored body may change.
async fn version_and_search_string(
    router: &MultiServiceRouter,
    case: &str,
    id: &str,
) -> (String, String) {
    let read = route53(
        router,
        case,
        "GET",
        &format!("/2013-04-01/healthcheck/{id}"),
        "",
        200,
    )
    .await;
    let version = elements(&read, "HealthCheckVersion")
        .into_iter()
        .next()
        .unwrap_or_default();
    let search = elements(&read, "SearchString")
        .into_iter()
        .next()
        .unwrap_or_default();
    (version, search)
}

/// One phase: the body the monitored endpoint is rewritten to serve, and whether the string
/// the health check searches for appears in it.
struct Phase {
    case: &'static str,
    body: &'static str,
    carries_the_string: bool,
}

#[tokio::test]
async fn the_search_string_the_monitored_body_carries_governs_what_every_checker_reports() {
    let router = router();
    publish_site(&router, &format!("<html>{MARKER}</html>")).await;

    let id = create_health_check(
        &router,
        "a string-match check of the site",
        "string-match-site",
        &format!(
            "<Type>HTTP_STR_MATCH</Type>\
             <FullyQualifiedDomainName>{}</FullyQualifiedDomainName><Port>80</Port>\
             <ResourcePath>/</ResourcePath><SearchString>{MARKER}</SearchString>\
             <RequestInterval>{REQUEST_INTERVAL}</RequestInterval>\
             <FailureThreshold>{FAILURE_THRESHOLD}</FailureThreshold>\
             <MeasureLatency>false</MeasureLatency><Inverted>false</Inverted>",
            website_host()
        ),
    )
    .await;
    let configured = version_and_search_string(&router, "as created", &id).await;
    assert_eq!(
        configured.1, MARKER,
        "the create must have stored the SearchString, got {configured:?}"
    );

    // Each phase rewrites the SAME key and nothing else: no call is made to Route 53 between
    // the write and the read, so what changes the reported status is the body alone. The
    // phases ALTERNATE between a body that carries the string and one that does not, so the
    // status a phase waits for can only come from a probe of the body that phase wrote —
    // never from the round before it.
    let phases: &[Phase] = &[
        Phase {
            case: "the body the site was published with, which carries the string",
            body: "<html>MARKER</html>",
            carries_the_string: true,
        },
        Phase {
            case: "a body that no longer carries the string",
            body: "<html>maintenance in progress</html>",
            carries_the_string: false,
        },
        Phase {
            case: "a body carrying the string deep inside a larger document",
            body: "<html><head><title>site</title></head><body><p>serving</p>\
                   <p>MARKER</p></body></html>",
            carries_the_string: true,
        },
        Phase {
            case: "a body carrying the string in another case",
            body: "<html>marker</html>",
            carries_the_string: false,
        },
        Phase {
            case: "a body carrying the string as the prefix of a longer word",
            body: "<html>MARKERS everywhere</html>",
            carries_the_string: true,
        },
        Phase {
            case: "an empty body",
            body: "",
            carries_the_string: false,
        },
        Phase {
            case: "the string restored",
            body: "<html>MARKER</html>",
            carries_the_string: true,
        },
    ];

    for phase in phases {
        serve_body(&router, phase.case, phase.body).await;
        let prefix = if phase.carries_the_string {
            STATUS_SUCCESS_PREFIX
        } else {
            STATUS_FAILURE_PREFIX
        };
        let reported = statuses_opening_with(&router, phase.case, &id, prefix).await;
        assert!(
            reported.len() >= 3,
            "case {:?}: the checkers reporting on an endpoint are the health-checking Regions, \
             got {reported:?}",
            phase.case
        );
        if !phase.carries_the_string {
            // The reason names the STRING MATCH: a body that does not carry the string is a
            // different fact about the endpoint from a connection that could not be made or
            // an answer the check's own request rejected.
            for status in &reported {
                assert_eq!(
                    status, STATUS_STRING_MATCH_FAILURE,
                    "case {:?}: expected the string-match failure, got {status:?}",
                    phase.case
                );
            }
        }
        // Nothing about the health check itself moved across the flip: the version and the
        // configuration are the ones the create stored.
        let after = version_and_search_string(&router, phase.case, &id).await;
        assert_eq!(
            after, configured,
            "case {:?}: rewriting the monitored body must leave the health check itself \
             untouched, got {after:?} against {configured:?}",
            phase.case
        );
    }
}

/// One ANONYMOUS request to the website host of a bucket other than [`BUCKET`], for the
/// addressing table below.
async fn website_at(router: &MultiServiceRouter, host: &str, path: &str) -> Answer {
    let request = http::Request::builder()
        .method("GET")
        .uri(format!("http://{host}{path}"))
        .header("host", host)
        .body(Bytes::new())
        .expect("a well-formed request");
    let response = router.dispatch(request).await;
    Answer {
        status: response.status().as_u16(),
        body: String::from_utf8_lossy(response.body()).into_owned(),
    }
}

/// One row of the addressing table: how the monitored site is named and published.
struct SiteRow {
    case: &'static str,
    bucket: &'static str,
    principal: &'static str,
    action: &'static str,
    resource: &'static str,
}

#[tokio::test]
async fn every_form_of_the_monitored_endpoints_address_is_probed_as_the_configuration_names_it() {
    // The rule holds over the ADDRESS of the monitored endpoint as much as over the body it
    // serves, so the rows differ along how the site is named and published: a bucket name
    // carrying dots ("a bucket name can consist of … lowercase letters, numbers, dots (.),
    // and hyphens", Amazon S3 User Guide, *Bucket naming rules*) as well as a plain one, the
    // `Principal`, `Action` and `Resource` of the publishing policy written both as a single
    // value and as a list, and — for each site — `FullyQualifiedDomainName` written both
    // without and with the root label's dot, which names the same host.
    let router = router();
    let rows: &[SiteRow] = &[
        SiteRow {
            case: "a plain bucket name, published with single-valued statements",
            bucket: "probe-plain-site",
            principal: "\"*\"",
            action: "\"s3:GetObject\"",
            resource: "\"arn:aws:s3:::probe-plain-site/*\"",
        },
        SiteRow {
            case: "a policy whose principal, action and resource are lists",
            bucket: "probe-lists-site",
            principal: "{\"AWS\":[\"*\"]}",
            action: "[\"s3:GetObject\",\"s3:GetObjectVersion\"]",
            resource: "[\"arn:aws:s3:::probe-lists-site/*\"]",
        },
        SiteRow {
            case: "a bucket name carrying dots",
            bucket: "probe.dotted.site",
            principal: "\"*\"",
            action: "\"s3:GetObject\"",
            resource: "\"arn:aws:s3:::probe.dotted.site/*\"",
        },
    ];

    for row in rows {
        let (case, bucket) = (row.case, row.bucket);
        let (principal, action, resource) = (row.principal, row.action, row.resource);
        let created = s3(
            &router,
            "PUT",
            &format!("/{bucket}"),
            format!(
                "<CreateBucketConfiguration xmlns=\"{S3_XMLNS}\">\
                 <LocationConstraint>{REGION}</LocationConstraint></CreateBucketConfiguration>"
            )
            .as_bytes(),
        )
        .await;
        assert_eq!(
            created.status, 200,
            "case {case:?}: CreateBucket({bucket:?}) must succeed, got {} with {}",
            created.status, created.body
        );
        let unblocked = s3(
            &router,
            "PUT",
            &format!("/{bucket}?publicAccessBlock"),
            b"<PublicAccessBlockConfiguration>\
              <BlockPublicAcls>false</BlockPublicAcls>\
              <IgnorePublicAcls>false</IgnorePublicAcls>\
              <BlockPublicPolicy>false</BlockPublicPolicy>\
              <RestrictPublicBuckets>false</RestrictPublicBuckets>\
              </PublicAccessBlockConfiguration>",
        )
        .await;
        assert_eq!(
            unblocked.status, 200,
            "case {case:?}: PutPublicAccessBlock must succeed, got {} with {}",
            unblocked.status, unblocked.body
        );
        let policy = s3(
            &router,
            "PUT",
            &format!("/{bucket}?policy"),
            format!(
                "{{\"Version\":\"2012-10-17\",\"Statement\":[\
                 {{\"Sid\":\"PublicRead\",\"Effect\":\"Allow\",\"Principal\":{principal},\
                 \"Action\":{action},\"Resource\":{resource}}}]}}"
            )
            .as_bytes(),
        )
        .await;
        assert_eq!(
            policy.status, 204,
            "case {case:?}: PutBucketPolicy must succeed, got {} with {}",
            policy.status, policy.body
        );
        let site = s3(
            &router,
            "PUT",
            &format!("/{bucket}?website"),
            format!(
                "<WebsiteConfiguration><IndexDocument><Suffix>{INDEX_KEY}</Suffix>\
                 </IndexDocument></WebsiteConfiguration>"
            )
            .as_bytes(),
        )
        .await;
        assert_eq!(
            site.status, 200,
            "case {case:?}: PutBucketWebsite must succeed, got {} with {}",
            site.status, site.body
        );
        let body = format!("<html>{MARKER}</html>");
        let object = s3(
            &router,
            "PUT",
            &format!("/{bucket}/{INDEX_KEY}"),
            body.as_bytes(),
        )
        .await;
        assert_eq!(
            object.status, 200,
            "case {case:?}: PutObject must succeed, got {} with {}",
            object.status, object.body
        );
        // The site is real before anything is asserted about the health check: the endpoint
        // the check monitors serves that body at the host S3 publishes it under.
        let host = format!("{bucket}.s3-website-{REGION}.amazonaws.com");
        let served = website_at(&router, &host, "/").await;
        assert_eq!(
            (served.status, served.body.as_str()),
            (200, body.as_str()),
            "case {case:?}: the website endpoint of {bucket:?} must serve the published body, \
             got {} with {:?}",
            served.status,
            served.body
        );

        for (spelling, fqdn) in [
            ("as the host is written", host.clone()),
            ("with the root label's dot", format!("{host}.")),
        ] {
            let case = format!("{case} ({spelling})");
            let id = create_health_check(
                &router,
                &case,
                &format!("probe-address-{bucket}-{spelling}"),
                &format!(
                    "<Type>HTTP_STR_MATCH</Type>\
                     <FullyQualifiedDomainName>{fqdn}</FullyQualifiedDomainName><Port>80</Port>\
                     <ResourcePath>/</ResourcePath><SearchString>{MARKER}</SearchString>\
                     <RequestInterval>{REQUEST_INTERVAL}</RequestInterval>\
                     <FailureThreshold>{FAILURE_THRESHOLD}</FailureThreshold>"
                ),
            )
            .await;
            let reported = statuses_opening_with(&router, &case, &id, STATUS_SUCCESS_PREFIX).await;
            assert!(
                reported.len() >= 3,
                "case {case:?}: the checkers reporting on an endpoint are the health-checking \
                 Regions, got {reported:?}"
            );
        }
    }
}

#[tokio::test]
async fn the_probe_requests_the_resource_path_the_configuration_names() {
    // The path the probe requests is `ResourcePath`, so a check pointed at one document of the
    // site is decided by THAT document's body and not by another one's: the two keys below
    // carry opposite bodies, and each check reports its own.
    let router = router();
    publish_site(&router, "<html>no marker here</html>").await;
    let written = s3(
        &router,
        "PUT",
        &format!("/{BUCKET}/status.html"),
        format!("<html>{MARKER}</html>").as_bytes(),
    )
    .await;
    assert_eq!(
        written.status, 200,
        "PutObject of the monitored document must succeed, got {} with {}",
        written.status, written.body
    );

    let watching_the_path = create_health_check(
        &router,
        "a check of the document that carries the string",
        "string-match-path",
        &format!(
            "<Type>HTTP_STR_MATCH</Type>\
             <FullyQualifiedDomainName>{}</FullyQualifiedDomainName><Port>80</Port>\
             <ResourcePath>/status.html</ResourcePath><SearchString>{MARKER}</SearchString>\
             <RequestInterval>{REQUEST_INTERVAL}</RequestInterval>\
             <FailureThreshold>{FAILURE_THRESHOLD}</FailureThreshold>",
            website_host()
        ),
    )
    .await;
    let watching_the_root = create_health_check(
        &router,
        "a check of the index document, which does not",
        "string-match-root",
        &format!(
            "<Type>HTTP_STR_MATCH</Type>\
             <FullyQualifiedDomainName>{}</FullyQualifiedDomainName><Port>80</Port>\
             <ResourcePath>/</ResourcePath><SearchString>{MARKER}</SearchString>\
             <RequestInterval>{REQUEST_INTERVAL}</RequestInterval>\
             <FailureThreshold>{FAILURE_THRESHOLD}</FailureThreshold>",
            website_host()
        ),
    )
    .await;

    statuses_opening_with(
        &router,
        "the document that carries the string",
        &watching_the_path,
        STATUS_SUCCESS_PREFIX,
    )
    .await;
    let failing = statuses_opening_with(
        &router,
        "the document that does not",
        &watching_the_root,
        STATUS_FAILURE_PREFIX,
    )
    .await;
    for status in &failing {
        assert_eq!(
            status, STATUS_STRING_MATCH_FAILURE,
            "the check of a document without the string reports the string-match failure, got \
             {status:?}"
        );
    }

    // A key the site does not hold at all is the endpoint's own 404, which is a failure of the
    // check's REQUEST rather than of the string match — a different fact, reported as one.
    let watching_nothing = create_health_check(
        &router,
        "a check of a document the site does not hold",
        "string-match-missing",
        &format!(
            "<Type>HTTP_STR_MATCH</Type>\
             <FullyQualifiedDomainName>{}</FullyQualifiedDomainName><Port>80</Port>\
             <ResourcePath>/absent.html</ResourcePath><SearchString>{MARKER}</SearchString>\
             <RequestInterval>{REQUEST_INTERVAL}</RequestInterval>\
             <FailureThreshold>{FAILURE_THRESHOLD}</FailureThreshold>",
            website_host()
        ),
    )
    .await;
    let missing = statuses_opening_with(
        &router,
        "a document the site does not hold",
        &watching_nothing,
        STATUS_FAILURE_PREFIX,
    )
    .await;
    for status in &missing {
        assert!(
            status.contains("404") && *status != STATUS_STRING_MATCH_FAILURE,
            "a request the endpoint answered with 404 is reported as that answer, got \
             {status:?}"
        );
    }
}
