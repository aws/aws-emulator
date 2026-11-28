// SPDX-License-Identifier: CC0-1.0

//! What WAF reports about the API SECURITY events in the traffic its web ACLs
//! inspected, end to end over the wire: `ListEvents`, `ListEventsByWebACL`, `GetEvent`,
//! `ListEventActors` and `GetEventActor`.
//!
//! The rules this suite pins:
//!
//! * An event is what WAF made of the REQUESTS a web ACL saw. `ApiSecurityEventDetail`
//!   carries the detection — the label WAF assigned, its threat category, its confidence,
//!   the API patterns it involves and the actors that produced it — and
//!   `EventActorSummary$AuthFingerprint` is what recognises one client across requests:
//!   "a hash of the authentication credentials that the client of the event actor
//!   provides in the request … you can use the fingerprint to match the same client
//!   across events". So the detection fires on one API being offered many DISTINCT
//!   credential sets, its confidence rises with the count, and a client that re-presents
//!   the same credentials is the same actor however many requests it sends and from
//!   however many addresses.
//! * Which traffic is analysed at all is the web ACL's `AnalyticsConfig`: "for regional
//!   web ACLs, security analytics is disabled by default when you omit this
//!   configuration. For web ACLs that protect CloudFront distributions (global web
//!   ACLs), security analytics is enabled by default", and `DISABLED` "stops
//!   transferring the web ACL's request logs".
//! * An event's `ApiPatterns` are the requests it involves, normalized the way
//!   `ApiPattern$PathPattern` publishes by example — "the normalized URI path … For
//!   example, `/api/v1/accounts/{id}`".
//! * The two listings are ONE report under two addresses, selected by `TimeWindow`,
//!   narrowed by `Filters` and `ApiPatternFilters`, ordered by `SortBy`/`SortOrder` and
//!   paged by `Limit`/`NextMarker`, with the timeline returned "only if you specify
//!   `TIMELINE` in the request `Include` setting".
//!
//! Every table below spans the class rather than one call. The requests are driven
//! through the data plane's own entry points
//! ([`service_com_amazonaws_wafv2_awswaf_20190729::service::edge`]) on the SAME shared
//! engine the service is mounted on — this service models no data-plane endpoint of its
//! own, because the requests a web ACL decides arrive at the protected resource's
//! endpoint — and every report is then read over the wire, so what these tests observe
//! is what an SDK observes.

use std::sync::Arc;

use serde_json::{json, Value};
use service_com_amazonaws_wafv2_awswaf_20190729::service::edge::{self, InspectedRequest};
use service_com_amazonaws_wafv2_awswaf_20190729::state::{
    self, ComAmazonawsWafv2Awswaf20190729State, EVENT_IDLE_SPAN_MILLIS, MAX_EVENT_WINDOW_MILLIS,
};

use crate::web_acl_lifecycle::{visibility, Waf, TEST_REGION};

/// The `Scope` of every web ACL here unless a row names another: an API Gateway stage is
/// a REGIONAL protected resource.
const REGIONAL: &str = "REGIONAL";

/// `AnalyticsMode$CROSS_REGION`, the value that "enables security analytics".
const CROSS_REGION: &str = "CROSS_REGION";

/// `AnalyticsMode$DISABLED`, the value that "disables security analytics".
const DISABLED: &str = "DISABLED";

/// The host every request below addresses, with the port it arrived on: the domain and
/// the port of the API pattern it belongs to (`ApiPattern$Domains`, `ApiPattern$Port`).
const API_HOST: &str = "api.example.com:8443";

/// The path every request addresses unless a row names another.
const API_PATH: &str = "/api/v1/accounts/12345";

/// The address every request comes from unless a row names another.
const CLIENT_A: &str = "198.51.100.5";

/// A second address, for the rows about a client that moved.
const CLIENT_B: &str = "203.0.113.9";

/// A stub protected resource: an API Gateway REST API stage, the one ARN form
/// `AssociateWebACLRequest$ResourceArn` documents for one. Each row binds its own, so no
/// two web ACLs decide one request.
fn stage_arn(suffix: &str) -> String {
    format!("arn:aws:apigateway:{TEST_REGION}::/restapis/a1b2c3d4e5/stages/{suffix}")
}

/// One request as the edge inspects it, presenting `credentials` in its `Authorization`
/// header when it presents any.
fn request(
    method: &str,
    path: &str,
    host: &str,
    client_ip: &str,
    credentials: Option<&str>,
) -> InspectedRequest {
    let mut headers = vec![
        ("user-agent".to_string(), "api-security".to_string()),
        ("host".to_string(), host.to_string()),
    ];
    if let Some(credentials) = credentials {
        headers.push(("authorization".to_string(), credentials.to_string()));
    }
    InspectedRequest {
        method: method.to_string(),
        uri_path: path.to_string(),
        query_string: String::new(),
        headers,
        body: Vec::new(),
        client_ip: client_ip.to_string(),
    }
}

/// Create a web ACL in `scope` carrying `analytics` (its `AnalyticsConfig$Mode`, or none
/// at all) and answer its ARN.
async fn create_web_acl(waf: &Waf, scope: &str, name: &str, analytics: Option<&str>) -> String {
    let mut body = json!({
        "Name": name,
        "Scope": scope,
        "DefaultAction": { "Allow": {} },
        "VisibilityConfig": visibility(name),
        "Rules": [],
    });
    if let Some(mode) = analytics {
        body["AnalyticsConfig"] = json!({ "Mode": mode });
    }
    waf.ok("CreateWebACL", body).await["Summary"]["ARN"]
        .as_str()
        .expect("Summary.ARN")
        .to_string()
}

/// A REGIONAL web ACL with security analytics enabled, in front of its own stage:
/// answers `(web ACL ARN, stage ARN)`.
async fn analysed_web_acl(waf: &Waf, name: &str) -> (String, String) {
    let web_acl_arn = create_web_acl(waf, REGIONAL, name, Some(CROSS_REGION)).await;
    let resource = stage_arn(name);
    waf.ok(
        "AssociateWebACL",
        json!({ "WebACLArn": web_acl_arn, "ResourceArn": resource }),
    )
    .await;
    (web_acl_arn, resource)
}

/// Drive one request at `resource` through the data plane, which is what records what
/// WAF's analysis makes of it.
fn offer(engine: &Arc<::engine::InMemoryEngine>, resource: &str, request: &InspectedRequest, case: &str) {
    edge::decide(&**engine, resource, request)
        .unwrap_or_else(|| panic!("case {case:?}: the associated web ACL must decide the request"));
}

/// `count` distinct credential sets, each a bearer token — one of the forms
/// `EventActorSummary$AuthFingerprint` names ("a JSON Web Token (JWT), a JSON Web
/// Encryption (JWE) token, an Amazon Web Services Signature Version 4 (SigV4)
/// signature, or basic authentication credentials").
fn bearer_tokens(count: usize) -> Vec<String> {
    (0..count)
        .map(|index| format!("Bearer eyJhbGciOiJIUzI1NiJ9.account-{index}.signature"))
        .collect()
}

/// A SigV4 `Authorization` header signed with the key `key_id` and carrying `signature`.
fn sigv4(key_id: &str, signature: &str) -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential={key_id}/20260101/{TEST_REGION}/execute-api/aws4_request, \
         SignedHeaders=host, Signature={signature}"
    )
}

/// A `TimeWindow` spanning the hour around the call, which every row that is not about
/// the window itself reports over: generous at both ends, so no row depends on how fast
/// the host runs.
fn surrounding_window() -> Value {
    let now = state::now_millis() as f64 / 1_000.0;
    json!({ "StartTime": now - 3_600.0, "EndTime": now + 3_600.0 })
}

/// The events `ListEventsByWebACL` reports for `web_acl_arn`.
async fn events_of(waf: &Waf, web_acl_arn: &str) -> Vec<Value> {
    waf.ok(
        "ListEventsByWebACL",
        json!({
            "Scope": REGIONAL,
            "WebAclArn": web_acl_arn,
            "TimeWindow": surrounding_window(),
        }),
    )
    .await["Events"]
        .as_array()
        .cloned()
        .unwrap_or_default()
}

/// The detection `EventDetailSummary` carries: `(label, category, confidence)`.
fn detection(event: &Value) -> (Option<&str>, Option<&str>, Option<&str>) {
    let detail = &event["ApiSecurityEventDetail"];
    (
        detail["Label"].as_str(),
        detail["Category"].as_str(),
        detail["Confidence"].as_str(),
    )
}

/// The detection fires once one API has been offered the documented floor of distinct
/// credential sets, and its confidence rises with that count.
///
/// The table spans the class: below the floor nothing is reported at all, at the floor
/// the weakest confidence, and each further step of the scale — the three values
/// `Confidence` declares — on a count that reaches it, including one count past each
/// step so the scale is read as a floor rather than as an exact figure.
#[tokio::test]
async fn the_detection_fires_at_the_floor_and_its_confidence_rises_with_the_count() {
    let (waf, engine) = Waf::sharing_engine();

    // `(case, distinct credential sets offered, the Confidence reported)`.
    let cases: &[(&str, usize, Option<&str>)] = &[
        ("one credential set, which is an ordinary sign-in", 1, None),
        ("two credential sets", 2, None),
        ("three credential sets, the detection floor", 3, Some("LOW")),
        ("four credential sets", 4, Some("LOW")),
        ("five credential sets", 5, Some("MEDIUM")),
        ("nine credential sets", 9, Some("MEDIUM")),
        ("ten credential sets", 10, Some("HIGH")),
        ("twelve credential sets", 12, Some("HIGH")),
    ];

    for (row, (case, offered, confidence)) in cases.iter().enumerate() {
        let name = format!("floor-{row}");
        let (web_acl_arn, resource) = analysed_web_acl(&waf, &name).await;
        for credentials in bearer_tokens(*offered) {
            offer(
                &engine,
                &resource,
                &request("POST", API_PATH, API_HOST, CLIENT_A, Some(&credentials)),
                case,
            );
        }

        let events = events_of(&waf, &web_acl_arn).await;
        let Some(expected) = confidence else {
            assert!(
                events.is_empty(),
                "case {case:?}: expected no event below the detection floor, got {events:?}"
            );
            continue;
        };
        assert_eq!(
            events.len(),
            1,
            "case {case:?}: expected one event for the resource the credentials were \
             offered to, got {events:?}"
        );
        let event = &events[0];
        let (label, category, reported) = detection(event);
        assert_eq!(
            reported,
            Some(*expected),
            "case {case:?}: expected Confidence {expected}, got {event}"
        );
        assert_eq!(
            label,
            Some(
                format!(
                    "awswaf:managed:aws:api-security:credential-stuffing-{}",
                    expected.to_ascii_lowercase()
                )
                .as_str()
            ),
            "case {case:?}: expected the detection's fully qualified label, got {event}"
        );
        assert_eq!(
            category,
            Some("API2:2023"),
            "case {case:?}: expected the broken-authentication threat category, got {event}"
        );
        assert_eq!(
            event["EventCategory"].as_str(),
            Some("API_SECURITY"),
            "case {case:?}: expected the API_SECURITY event category, got {event}"
        );
        assert_eq!(
            event["WebAclArn"].as_str(),
            Some(web_acl_arn.as_str()),
            "case {case:?}: expected the event to name its web ACL, got {event}"
        );
        assert_eq!(
            event["ResourceArn"].as_str(),
            Some(resource.as_str()),
            "case {case:?}: expected the event to name the protected resource, got {event}"
        );
        assert!(
            event["EventId"]
                .as_str()
                .is_some_and(crate::web_acl_lifecycle::is_model_uuid),
            "case {case:?}: expected an EntityId-shaped event id, got {event}"
        );

        // The event reads back whole through `GetEvent`, which resolves it by the id the
        // listing reported: the round trip of the report.
        let event_id = event["EventId"].as_str().expect("EventId");
        let retrieved = waf
            .ok(
                "GetEvent",
                json!({ "Scope": REGIONAL, "EventId": event_id, "WebAclArn": web_acl_arn }),
            )
            .await;
        assert_eq!(
            retrieved["Event"]["EventId"].as_str(),
            Some(event_id),
            "case {case:?}: expected GetEvent to resolve the listed event, got {retrieved}"
        );
        assert_eq!(
            detection(&retrieved["Event"]).2,
            Some(*expected),
            "case {case:?}: expected GetEvent to report the same confidence, got {retrieved}"
        );
        // "Including the event actors that are associated with the event and the event
        // timeline" (`GetEvent`): the timeline is part of its answer, unasked.
        assert!(
            retrieved["Event"]["ApiSecurityEventDetail"]["Timeline"]
                .as_array()
                .is_some_and(|timeline| !timeline.is_empty()),
            "case {case:?}: expected GetEvent to carry the event timeline, got {retrieved}"
        );
    }
}

/// One client is one actor however many requests it sends, whatever form its credentials
/// take and whatever address it sends them from — and a move is recorded as a change to
/// its network attributes.
#[tokio::test]
async fn one_client_is_one_actor_whatever_form_its_credentials_take() {
    let (waf, engine) = Waf::sharing_engine();

    // `(case, the requests as (address, credentials), how many actors they are)`.
    struct Case {
        case: &'static str,
        offered: Vec<(&'static str, String)>,
        actors: usize,
        /// Whether one of those actors must carry a recorded address change.
        moved: bool,
    }
    // Each row's credentials are its own: an actor belongs to the ACCOUNT ("the event
    // actors in your account", `ListEventActors`), so a credential set shared between
    // two rows would be one client appearing in both their events.
    let cases = [
        Case {
            case: "a bearer token re-presented from a second address",
            offered: vec![
                (CLIENT_A, "Bearer eyJhbGciOiJIUzI1NiJ9.r0.sig".to_string()),
                (CLIENT_B, "Bearer eyJhbGciOiJIUzI1NiJ9.r0.sig".to_string()),
                (CLIENT_A, "Basic cjA6YQ==".to_string()),
                (CLIENT_A, "Basic cjA6Yg==".to_string()),
            ],
            actors: 3,
            moved: true,
        },
        Case {
            case: "a SigV4 request re-signed per call with one key",
            offered: vec![
                (CLIENT_A, sigv4("AKIDROWONEEXAMPLE", "a1")),
                (CLIENT_A, sigv4("AKIDROWONEEXAMPLE", "b2")),
                (CLIENT_A, "Basic cjE6YQ==".to_string()),
                (CLIENT_A, "Basic cjE6Yg==".to_string()),
            ],
            actors: 3,
            moved: false,
        },
        Case {
            case: "SigV4 requests signed with two different keys",
            offered: vec![
                (CLIENT_A, sigv4("AKIDROWTWOFIRSTKEY", "a1")),
                (CLIENT_A, sigv4("AKIDROWTWOSECONDKEY", "a1")),
                (CLIENT_A, "Basic cjI6YQ==".to_string()),
            ],
            actors: 3,
            moved: false,
        },
        Case {
            case: "basic credentials differing only in their password",
            offered: vec![
                (CLIENT_A, "Basic cjM6YQ==".to_string()),
                (CLIENT_A, "Basic cjM6Yg==".to_string()),
                (CLIENT_A, "Basic cjM6Yw==".to_string()),
            ],
            actors: 3,
            moved: false,
        },
        Case {
            case: "requests presenting no credentials at all, beside three that do",
            offered: vec![
                (CLIENT_A, "Bearer eyJhbGciOiJIUzI1NiJ9.r4.sig".to_string()),
                (CLIENT_A, "Basic cjQ6YQ==".to_string()),
                (CLIENT_A, "Basic cjQ6Yg==".to_string()),
            ],
            actors: 3,
            moved: false,
        },
    ];

    for (row, case) in cases.iter().enumerate() {
        let name = format!("actors-{row}");
        let (web_acl_arn, resource) = analysed_web_acl(&waf, &name).await;
        // The row's own requests, plus — for the row about it — one carrying no
        // credentials, which is no part of such an attempt and so contributes no actor.
        if case.case.starts_with("requests presenting no credentials") {
            offer(
                &engine,
                &resource,
                &request("POST", API_PATH, API_HOST, CLIENT_B, None),
                case.case,
            );
        }
        for (client_ip, credentials) in &case.offered {
            offer(
                &engine,
                &resource,
                &request("POST", API_PATH, API_HOST, client_ip, Some(credentials)),
                case.case,
            );
        }

        let events = events_of(&waf, &web_acl_arn).await;
        assert_eq!(
            events.len(),
            1,
            "case {:?}: expected one event, got {events:?}",
            case.case
        );
        let actors = events[0]["ApiSecurityEventDetail"]["EventActors"]
            .as_array()
            .cloned()
            .unwrap_or_default();
        assert_eq!(
            actors.len(),
            case.actors,
            "case {:?}: expected {} event actors, got {actors:?}",
            case.case,
            case.actors
        );

        // Every actor the event names resolves through `GetEventActor`, reports the
        // fingerprint the event referenced it by, and is counted against this one event
        // by the listing of actors.
        let listed = waf
            .ok(
                "ListEventActors",
                json!({ "Scope": REGIONAL, "TimeWindow": surrounding_window() }),
            )
            .await["EventActors"]
            .as_array()
            .cloned()
            .unwrap_or_default();
        let mut moved = 0_usize;
        for actor in &actors {
            let actor_id = actor["EventActorId"].as_str().expect("EventActorId");
            let detail = waf
                .ok(
                    "GetEventActor",
                    json!({ "Scope": REGIONAL, "EventActorId": actor_id }),
                )
                .await["EventActor"]
                .clone();
            assert_eq!(
                detail["Summary"]["AuthFingerprint"].as_str(),
                actor["EventActorValue"].as_str(),
                "case {:?}: expected the actor to report the value the event named it \
                 by, got {detail}",
                case.case
            );
            assert!(
                detail["Events"]
                    .as_array()
                    .is_some_and(|related| related
                        .iter()
                        .any(|event| event["EventId"] == events[0]["EventId"])),
                "case {:?}: expected the actor to be associated with the event, got \
                 {detail}",
                case.case
            );
            let changes = detail["Changes"].as_array().cloned().unwrap_or_default();
            if !changes.is_empty() {
                moved += 1;
                assert_eq!(
                    changes.last().and_then(|change| change["ClientIp"].as_str()),
                    detail["Summary"]["ClientIp"].as_str(),
                    "case {:?}: expected the newest change to carry the address the \
                     actor is now at, got {detail}",
                    case.case
                );
            }
            let summary = listed
                .iter()
                .find(|listed| listed["EventActorId"].as_str() == Some(actor_id));
            assert_eq!(
                summary.and_then(|summary| summary["EventCount"].as_i64()),
                Some(1),
                "case {:?}: expected the actor to be listed with one event in the \
                 window, got {summary:?}",
                case.case
            );
        }
        assert_eq!(
            moved > 0,
            case.moved,
            "case {:?}: expected moved={}, got {moved} actors carrying a change",
            case.case,
            case.moved
        );
    }
}

/// An event's `ApiPatterns` are the requests it involves, normalized the way
/// `ApiPattern$PathPattern` publishes by example.
///
/// The table spans the class: the published example itself, a path whose identifier is a
/// UUID rather than a number, a path with two identifiers, a path with none at all (an
/// entry the example does not cite), the root path, and a request that names no port.
#[tokio::test]
async fn the_api_pattern_is_the_normalized_request_it_addressed() {
    let (waf, engine) = Waf::sharing_engine();

    // One row per request shape the normalization has to answer for.
    struct PatternRow {
        case: &'static str,
        method: &'static str,
        path: &'static str,
        host: &'static str,
        /// The `PathPattern` the event must report.
        pattern: &'static str,
        /// The `Port` and `Domains` it must report.
        port: i64,
        domains: &'static [&'static str],
    }
    let cases: &[PatternRow] = &[
        PatternRow {
            case: "the published example: a numeric resource id",
            method: "POST",
            path: "/api/v1/accounts/12345",
            host: API_HOST,
            pattern: "/api/v1/accounts/{id}",
            port: 8443,
            domains: &["api.example.com"],
        },
        PatternRow {
            case: "a resource id spelled as a UUID",
            method: "GET",
            path: "/api/v1/accounts/3f2504e0-4f89-41d3-9a0c-0305e82c3301",
            host: API_HOST,
            pattern: "/api/v1/accounts/{id}",
            port: 8443,
            domains: &["api.example.com"],
        },
        PatternRow {
            case: "two identifiers in one path",
            method: "PUT",
            path: "/orders/42/items/7",
            host: API_HOST,
            pattern: "/orders/{id}/items/{id}",
            port: 8443,
            domains: &["api.example.com"],
        },
        PatternRow {
            case: "a path that names no single resource",
            method: "POST",
            path: "/api/v1/accounts",
            host: API_HOST,
            pattern: "/api/v1/accounts",
            port: 8443,
            domains: &["api.example.com"],
        },
        PatternRow {
            case: "the root path",
            method: "POST",
            path: "/",
            host: API_HOST,
            pattern: "/",
            port: 8443,
            domains: &["api.example.com"],
        },
        PatternRow {
            case: "a host that names no port",
            method: "POST",
            path: "/login",
            host: "api.example.com",
            pattern: "/login",
            port: 0,
            domains: &["api.example.com"],
        },
    ];

    for (row, case) in cases.iter().enumerate() {
        let PatternRow {
            case,
            method,
            path,
            host,
            pattern,
            port,
            domains,
        } = case;
        let name = format!("pattern-{row}");
        let (web_acl_arn, resource) = analysed_web_acl(&waf, &name).await;
        for credentials in bearer_tokens(3) {
            offer(
                &engine,
                &resource,
                &request(method, path, host, CLIENT_A, Some(&credentials)),
                case,
            );
        }

        let events = events_of(&waf, &web_acl_arn).await;
        assert_eq!(
            events.len(),
            1,
            "case {case:?}: expected one event, got {events:?}"
        );
        let patterns = events[0]["ApiSecurityEventDetail"]["ApiPatterns"]
            .as_array()
            .cloned()
            .unwrap_or_default();
        assert_eq!(
            patterns.len(),
            1,
            "case {case:?}: expected the one API pattern the traffic addressed, got \
             {patterns:?}"
        );
        let observed = &patterns[0];
        assert_eq!(
            observed["PathPattern"].as_str(),
            Some(*pattern),
            "case {case:?}: expected the normalized path {pattern}, got {observed}"
        );
        assert_eq!(
            observed["Method"].as_str(),
            Some(*method),
            "case {case:?}: expected the method {method}, got {observed}"
        );
        assert_eq!(
            observed["Port"].as_i64(),
            Some(*port),
            "case {case:?}: expected the port {port}, got {observed}"
        );
        assert_eq!(
            observed["Domains"],
            json!(domains),
            "case {case:?}: expected the domains {domains:?}, got {observed}"
        );
    }
}

/// Whether a web ACL's traffic is analysed at all is its `AnalyticsConfig`, whose
/// defaults the member documents per namespace — and the configuration itself round
/// trips through a read, saved either way.
#[tokio::test]
async fn security_analytics_decides_whether_the_traffic_is_analysed() {
    let (waf, engine) = Waf::sharing_engine();

    // `(case, scope, the configured Mode, the Mode a read reports, events reported)`.
    let cases: &[(&str, &str, Option<&str>, &str, bool)] = &[
        (
            "a regional web ACL that configures nothing",
            REGIONAL,
            None,
            DISABLED,
            false,
        ),
        (
            "a regional web ACL that enables analytics",
            REGIONAL,
            Some(CROSS_REGION),
            CROSS_REGION,
            true,
        ),
        (
            "a regional web ACL that disables analytics",
            REGIONAL,
            Some(DISABLED),
            DISABLED,
            false,
        ),
        (
            "a global web ACL that configures nothing",
            "CLOUDFRONT",
            None,
            CROSS_REGION,
            true,
        ),
        (
            "a global web ACL that disables analytics",
            "CLOUDFRONT",
            Some(DISABLED),
            DISABLED,
            false,
        ),
    ];

    for (row, (case, scope, configured, reported, analysed)) in cases.iter().enumerate() {
        let name = format!("analytics-{row}");
        let web_acl_arn = create_web_acl(&waf, scope, &name, *configured).await;

        // The member round trips: a read reports the mode the save named, or the
        // documented default of the namespace when the save named none.
        let stored = waf
            .ok("GetWebACL", json!({ "Scope": scope, "ARN": web_acl_arn }))
            .await;
        assert_eq!(
            stored["WebACL"]["AnalyticsConfig"]["Mode"].as_str(),
            Some(*reported),
            "case {case:?}: expected AnalyticsConfig.Mode {reported}, got {stored}"
        );

        // The traffic: a global web ACL is named by the distribution in front of it
        // rather than associated here, so its requests are decided against the web ACL
        // itself; a regional one is associated with its stage.
        let credentials = bearer_tokens(3);
        if *scope == REGIONAL {
            let resource = stage_arn(&name);
            waf.ok(
                "AssociateWebACL",
                json!({ "WebACLArn": web_acl_arn, "ResourceArn": resource }),
            )
            .await;
            for credential in &credentials {
                offer(
                    &engine,
                    &resource,
                    &request("POST", API_PATH, API_HOST, CLIENT_A, Some(credential)),
                    case,
                );
            }
        } else {
            for credential in &credentials {
                edge::decide_against(
                    &*engine,
                    &web_acl_arn,
                    &request("POST", API_PATH, API_HOST, CLIENT_A, Some(credential)),
                )
                .unwrap_or_else(|| panic!("case {case:?}: the web ACL must decide the request"));
            }
        }

        let events = waf
            .ok(
                "ListEventsByWebACL",
                json!({
                    "Scope": scope,
                    "WebAclArn": web_acl_arn,
                    "TimeWindow": surrounding_window(),
                }),
            )
            .await["Events"]
            .as_array()
            .cloned()
            .unwrap_or_default();
        assert_eq!(
            !events.is_empty(),
            *analysed,
            "case {case:?}: expected analysed={analysed}, got {events:?}"
        );
    }
}

/// Place the event of `resource_arn` at `start_millis`, last seen `last_millis`.
///
/// The instants an event is ordered and selected by are the instants its traffic
/// arrived at, so a suite about ordering, selection and the end of an event places them
/// itself — the state the data plane writes, written here against the clock, rather than
/// waiting for a wall clock to produce them.
fn place_event(
    state: &::engine::ServiceState<ComAmazonawsWafv2Awswaf20190729State>,
    resource_arn: &str,
    start_millis: i64,
    last_millis: i64,
) {
    let mut store = state.lock();
    let record = store
        .api_security_events
        .values_mut()
        .find(|record| record.resource_arn.as_deref() == Some(resource_arn))
        .expect("the data plane recorded an event for the resource");
    record.first_observed_millis = start_millis;
    record.last_observed_millis = last_millis;
}

/// Three events of one web ACL, placed `oldest_secs_back`, ten and twenty seconds
/// later so their order is the traffic's rather than the scheduler's: answers
/// `(web ACL ARN, [resource ARN], [event id])` oldest first.
async fn three_events(
    waf: &Waf,
    engine: &Arc<::engine::InMemoryEngine>,
    name: &str,
    oldest_secs_back: i64,
) -> (String, Vec<String>, Vec<String>) {
    let web_acl_arn = create_web_acl(waf, REGIONAL, name, Some(CROSS_REGION)).await;
    let state: ::engine::ServiceState<ComAmazonawsWafv2Awswaf20190729State> =
        state::service_state(&**engine).expect("the wafv2 store");
    let now = state::now_millis();
    let mut resources = Vec::new();
    for index in 0..3 {
        let resource = stage_arn(&format!("{name}-{index}"));
        waf.ok(
            "AssociateWebACL",
            json!({ "WebACLArn": web_acl_arn, "ResourceArn": resource }),
        )
        .await;
        // A different path per event, so an `ApiPatternFilters` row can tell them apart.
        let path = format!("/api/v1/accounts/{index}/login");
        for credentials in bearer_tokens(3) {
            offer(
                engine,
                &resource,
                &request("POST", &path, API_HOST, CLIENT_A, Some(&credentials)),
                name,
            );
        }
        let placed = now - (oldest_secs_back - i64::from(index) * 10) * 1_000;
        place_event(&state, &resource, placed, placed + 1_000);
        resources.push(resource);
    }
    let mut ordered: Vec<(i64, String)> = {
        let store = state.lock();
        store
            .api_security_events
            .values()
            .filter(|record| record.web_acl_arn == web_acl_arn)
            .filter_map(|record| {
                record
                    .event_id
                    .clone()
                    .map(|event_id| (record.first_observed_millis, event_id))
            })
            .collect()
    };
    ordered.sort();
    (
        web_acl_arn,
        resources,
        ordered.into_iter().map(|(_, event_id)| event_id).collect(),
    )
}

/// The ids an events listing answers with, in the order it answered them.
fn listed_ids(events: &[Value]) -> Vec<String> {
    events
        .iter()
        .filter_map(|event| event["EventId"].as_str().map(str::to_string))
        .collect()
}

/// The two listings are one report, selected by its window, narrowed by its filters,
/// ordered by its sort settings and paged by its limit and marker.
#[tokio::test]
async fn the_listings_select_order_and_page_the_events_they_report() {
    let (waf, engine) = Waf::sharing_engine();
    // The two web ACLs' events are placed over disjoint spans — this one's are the
    // newest of the account — so one order is the whole account's order as well.
    let (web_acl_arn, resources, oldest_first) = three_events(&waf, &engine, "listing", 30).await;
    let newest_first: Vec<String> = oldest_first.iter().rev().cloned().collect();
    // A second web ACL's events belong to it alone, which is what the by-web-ACL
    // listing narrows to.
    let (other_arn, _, other_events) = three_events(&waf, &engine, "other", 60).await;
    let other_newest_first: Vec<String> = other_events.iter().rev().cloned().collect();

    // `(case, the request body over ListEvents, the ids it must answer, in order)`.
    let cases: Vec<(&str, Value, Vec<String>)> = vec![
        (
            "no window at all, which reports over the past year",
            json!({ "Scope": REGIONAL }),
            [newest_first.clone(), other_newest_first.clone()].concat(),
        ),
        (
            "the default order, which is descending by start time",
            json!({ "Scope": REGIONAL, "WebAclArn": &web_acl_arn }),
            newest_first.clone(),
        ),
        (
            "ascending by start time",
            json!({
                "Scope": REGIONAL,
                "WebAclArn": &web_acl_arn,
                "SortBy": "START_TIME",
                "SortOrder": "ASC",
            }),
            oldest_first.clone(),
        ),
        (
            "descending by start time, stated rather than defaulted",
            json!({
                "Scope": REGIONAL,
                "WebAclArn": &web_acl_arn,
                "SortBy": "START_TIME",
                "SortOrder": "DESC",
            }),
            newest_first.clone(),
        ),
        (
            "a window that starts after the oldest event",
            json!({
                "Scope": REGIONAL,
                "WebAclArn": &web_acl_arn,
                "TimeWindow": {
                    "StartTime": (state::now_millis() - 25_000) as f64 / 1_000.0,
                    "EndTime": (state::now_millis() + 3_600_000) as f64 / 1_000.0,
                },
                "SortOrder": "ASC",
            }),
            oldest_first[1..].to_vec(),
        ),
        (
            "a filter naming one protected resource",
            json!({
                "Scope": REGIONAL,
                "Filters": [{ "Name": "RESOURCE_ARN", "Values": [&resources[1]] }],
            }),
            vec![oldest_first[1].clone()],
        ),
        (
            "a filter naming two protected resources",
            json!({
                "Scope": REGIONAL,
                "Filters": [{
                    "Name": "RESOURCE_ARN",
                    "Values": [&resources[0], &resources[2]],
                }],
                "SortOrder": "ASC",
            }),
            vec![oldest_first[0].clone(), oldest_first[2].clone()],
        ),
        (
            "a filter naming the one event category there is",
            json!({
                "Scope": REGIONAL,
                "WebAclArn": &web_acl_arn,
                "Filters": [{ "Name": "EVENT_CATEGORY", "Values": ["API_SECURITY"] }],
                "SortOrder": "ASC",
            }),
            oldest_first.clone(),
        ),
        (
            "two filters, which narrow by both fields at once",
            json!({
                "Scope": REGIONAL,
                "Filters": [
                    { "Name": "EVENT_CATEGORY", "Values": ["API_SECURITY"] },
                    { "Name": "RESOURCE_ARN", "Values": [&resources[2]] },
                ],
            }),
            vec![oldest_first[2].clone()],
        ),
        (
            "an API pattern filter naming one path",
            json!({
                "Scope": REGIONAL,
                "ApiPatternFilters": [{ "PathPattern": "/api/v1/accounts/{id}/login" }],
                "SortOrder": "ASC",
            }),
            [other_events.clone(), oldest_first.clone()].concat(),
        ),
        (
            "an API pattern filter naming a method nothing sent",
            json!({
                "Scope": REGIONAL,
                "ApiPatternFilters": [{ "Method": "DELETE" }],
            }),
            Vec::new(),
        ),
        (
            "an API pattern filter naming the port the traffic arrived on",
            json!({
                "Scope": REGIONAL,
                "WebAclArn": &web_acl_arn,
                "ApiPatternFilters": [{ "Port": 8443, "Domains": ["api.example.com"] }],
                "SortOrder": "ASC",
            }),
            oldest_first.clone(),
        ),
        (
            "two API pattern filters, of which one matches",
            json!({
                "Scope": REGIONAL,
                "WebAclArn": &web_acl_arn,
                "ApiPatternFilters": [
                    { "Method": "DELETE" },
                    { "Port": 8443 },
                ],
                "SortOrder": "ASC",
            }),
            oldest_first.clone(),
        ),
    ];

    for (case, body, expected) in cases {
        // A row naming a web ACL is asked of the listing that takes one; every other
        // row of the whole account's listing. `ListEvents` models no `WebAclArn`, so a
        // row cannot be asked of both with one body.
        let operation = if body.get("WebAclArn").is_some() {
            "ListEventsByWebACL"
        } else {
            "ListEvents"
        };
        let answered = waf.ok(operation, body.clone()).await;
        let events = answered["Events"].as_array().cloned().unwrap_or_default();
        assert_eq!(
            listed_ids(&events),
            expected,
            "case {case:?}: expected {operation} to answer {expected:?}, got {answered}"
        );
        assert!(
            events
                .iter()
                .all(|event| event["ApiSecurityEventDetail"]["Timeline"].is_null()),
            "case {case:?}: expected {operation} to leave the timeline out unasked, got \
             {answered}"
        );
    }

    // The two listings are ONE report: the whole account's listing narrowed to this web
    // ACL's own resources answers exactly what the by-web-ACL listing answers.
    let by_web_acl = waf
        .ok(
            "ListEventsByWebACL",
            json!({ "Scope": REGIONAL, "WebAclArn": &web_acl_arn, "SortOrder": "ASC" }),
        )
        .await;
    let by_filter = waf
        .ok(
            "ListEvents",
            json!({
                "Scope": REGIONAL,
                "Filters": [{ "Name": "RESOURCE_ARN", "Values": &resources }],
                "SortOrder": "ASC",
            }),
        )
        .await;
    assert_eq!(
        by_filter["Events"], by_web_acl["Events"],
        "expected the account's listing narrowed to this web ACL's resources to answer \
         what the by-web-ACL listing answers ({by_web_acl}), got {by_filter}"
    );

    // `Include` TIMELINE: "specify TIMELINE to return the timeline of each event".
    let with_timeline = waf
        .ok(
            "ListEventsByWebACL",
            json!({
                "Scope": REGIONAL,
                "WebAclArn": &web_acl_arn,
                "Include": ["TIMELINE"],
            }),
        )
        .await;
    assert!(
        with_timeline["Events"]
            .as_array()
            .is_some_and(|events| !events.is_empty()
                && events.iter().all(|event| event["ApiSecurityEventDetail"]
                    ["Timeline"]
                    .as_array()
                    .is_some_and(|timeline| !timeline.is_empty()))),
        "expected every event to carry its timeline when TIMELINE is included, got \
         {with_timeline}"
    );

    // The pages: a `Limit` of one walks the whole ordered sequence through its
    // `NextMarker`, and the page after the last one is empty and carries none.
    let mut walked: Vec<String> = Vec::new();
    let mut marker: Option<String> = None;
    for _ in 0..oldest_first.len() + 1 {
        let mut body = json!({
            "Scope": REGIONAL,
            "WebAclArn": &web_acl_arn,
            "SortOrder": "ASC",
            "Limit": 1,
        });
        if let Some(marker) = marker.as_deref() {
            body["NextMarker"] = json!(marker);
        }
        let page = waf.ok("ListEventsByWebACL", body).await;
        let ids = listed_ids(&page["Events"].as_array().cloned().unwrap_or_default());
        if ids.is_empty() {
            assert!(
                page["NextMarker"].is_null(),
                "expected the page after the last to carry no marker, got {page}"
            );
            break;
        }
        assert_eq!(
            ids.len(),
            1,
            "expected a page of one event for Limit 1, got {page}"
        );
        marker = page["NextMarker"].as_str().map(str::to_string);
        assert_eq!(
            marker.as_deref(),
            Some(ids[0].as_str()),
            "expected the marker to name the last event of the page, got {page}"
        );
        walked.extend(ids);
    }
    assert_eq!(
        walked, oldest_first,
        "expected the pages to walk the whole ordered sequence, got {walked:?}"
    );

    // And the by-web-ACL listing never reports another web ACL's events.
    let narrowed = waf
        .ok(
            "ListEventsByWebACL",
            json!({ "Scope": REGIONAL, "WebAclArn": &other_arn, "SortOrder": "ASC" }),
        )
        .await;
    assert_eq!(
        listed_ids(&narrowed["Events"].as_array().cloned().unwrap_or_default()),
        other_events,
        "expected the by-web-ACL listing to report that web ACL's events alone, got \
         {narrowed}"
    );
}

/// An event's status is derived from its own traffic: it has started until more arrives,
/// is in progress while it does, and has ended — with the end time populated — once the
/// traffic stopped.
#[tokio::test]
async fn an_events_status_follows_the_traffic_it_is_about() {
    let (waf, engine) = Waf::sharing_engine();
    let (web_acl_arn, resource) = analysed_web_acl(&waf, "status").await;
    let state: ::engine::ServiceState<ComAmazonawsWafv2Awswaf20190729State> =
        state::service_state(&*engine).expect("the wafv2 store");

    let offered = bearer_tokens(5);
    for credentials in offered.iter().take(3) {
        offer(
            &engine,
            &resource,
            &request("POST", API_PATH, API_HOST, CLIENT_A, Some(credentials)),
            "the detection has just fired",
        );
    }
    let started = events_of(&waf, &web_acl_arn).await;
    assert_eq!(
        started.first().map(|event| &event["EventStatus"]),
        Some(&json!("EVENT_STARTED")),
        "expected a freshly reported event to have started, got {started:?}"
    );
    assert!(
        started
            .first()
            .is_some_and(|event| event["EndTime"].is_null()),
        "expected no end time while the event is ongoing, got {started:?}"
    );

    for credentials in offered.iter().skip(3) {
        offer(
            &engine,
            &resource,
            &request("POST", API_PATH, API_HOST, CLIENT_A, Some(credentials)),
            "more of the same traffic",
        );
    }
    let in_progress = events_of(&waf, &web_acl_arn).await;
    assert_eq!(
        in_progress.first().map(|event| &event["EventStatus"]),
        Some(&json!("EVENT_IN_PROGRESS")),
        "expected an event whose traffic is still arriving to be in progress, got \
         {in_progress:?}"
    );

    // The traffic stopped: placed far enough back that the clock alone has made the
    // transition due.
    let stopped = state::now_millis() - EVENT_IDLE_SPAN_MILLIS - 1_000;
    place_event(&state, &resource, stopped - 1_000, stopped);
    let ended = events_of(&waf, &web_acl_arn).await;
    assert_eq!(
        ended.first().map(|event| &event["EventStatus"]),
        Some(&json!("EVENT_ENDED")),
        "expected an event whose traffic stopped to have ended, got {ended:?}"
    );
    assert_eq!(
        ended
            .first()
            .and_then(|event| event["EndTime"].as_f64())
            .map(|end| (end * 1_000.0) as i64),
        Some(stopped),
        "expected the end time to be the last request the event saw, got {ended:?}"
    );

    // And the report changed nothing a later caller sees: the same read answers the
    // same event, status included.
    let again = events_of(&waf, &web_acl_arn).await;
    assert_eq!(
        again, ended,
        "expected a report to leave the events it read unchanged, got {again:?}"
    );
}

/// The parameters these reports refuse, and the bounds they accept.
#[tokio::test]
async fn the_reports_refuse_the_parameters_they_document() {
    let (waf, engine) = Waf::sharing_engine();
    let (web_acl_arn, resource) = analysed_web_acl(&waf, "refusals").await;
    for credentials in bearer_tokens(3) {
        offer(
            &engine,
            &resource,
            &request("POST", API_PATH, API_HOST, CLIENT_A, Some(&credentials)),
            "the event the refusals are asked about",
        );
    }
    let events = events_of(&waf, &web_acl_arn).await;
    let event_id = events[0]["EventId"].as_str().expect("EventId").to_string();
    // A whole second an hour ahead, so every window below is a round number of seconds
    // and no row depends on where the call fell inside a second.
    let window_end = (state::now_millis().div_euclid(1_000) + 3_600) * 1_000;
    let seconds = |millis: i64| millis as f64 / 1_000.0;

    // A window exactly as long as the longest one allowed is accepted; one second
    // longer is refused: "the time window can't be longer than one year".
    let at_the_bound = json!({
        "Scope": REGIONAL,
        "TimeWindow": {
            "StartTime": seconds(window_end - MAX_EVENT_WINDOW_MILLIS),
            "EndTime": seconds(window_end),
        },
    });
    let accepted = waf.ok("ListEvents", at_the_bound).await;
    assert_eq!(
        listed_ids(&accepted["Events"].as_array().cloned().unwrap_or_default()),
        vec![event_id.clone()],
        "expected a window of exactly one year to be reported over, got {accepted}"
    );
    let over_the_bound = json!({
        "StartTime": seconds(window_end - MAX_EVENT_WINDOW_MILLIS - 1_000),
        "EndTime": seconds(window_end),
    });

    // `(case, operation, body, the error code)`.
    let cases: Vec<(&str, &str, Value, &str)> = vec![
        (
            "a window longer than the longest one allowed",
            "ListEvents",
            json!({ "Scope": REGIONAL, "TimeWindow": over_the_bound }),
            "WAFInvalidParameterException",
        ),
        (
            "a window longer than the longest one allowed, by web ACL",
            "ListEventsByWebACL",
            json!({
                "Scope": REGIONAL,
                "WebAclArn": &web_acl_arn,
                "TimeWindow": over_the_bound,
            }),
            "WAFInvalidParameterException",
        ),
        (
            "a window longer than the longest one allowed, for actors",
            "ListEventActors",
            json!({ "Scope": REGIONAL, "TimeWindow": over_the_bound }),
            "WAFInvalidParameterException",
        ),
        (
            "a marker naming no position in the sequence",
            "ListEvents",
            json!({ "Scope": REGIONAL, "NextMarker": "not-a-position" }),
            "WAFInvalidParameterException",
        ),
        (
            "a marker naming no position in the actor sequence",
            "ListEventActors",
            json!({
                "Scope": REGIONAL,
                "TimeWindow": surrounding_window(),
                "NextMarker": "not-a-position",
            }),
            "WAFInvalidParameterException",
        ),
        (
            "a limit past the largest page an events listing answers with",
            "ListEvents",
            json!({ "Scope": REGIONAL, "Limit": 21 }),
            "ValidationException",
        ),
        (
            "a web ACL ARN that is no ARN at all",
            "ListEventsByWebACL",
            json!({ "Scope": REGIONAL, "WebAclArn": "not-an-arn-but-long-enough" }),
            "WAFInvalidParameterException",
        ),
        (
            "a well-formed web ACL ARN naming nothing stored",
            "ListEventsByWebACL",
            json!({
                "Scope": REGIONAL,
                "WebAclArn": format!(
                    "arn:aws:wafv2:{TEST_REGION}:111122223333:regional/webacl/absent/\
                     3f2504e0-4f89-41d3-9a0c-0305e82c3301"
                ),
            }),
            "WAFNonexistentItemException",
        ),
        (
            "an event id nothing minted",
            "GetEvent",
            json!({
                "Scope": REGIONAL,
                "EventId": "3f2504e0-4f89-41d3-9a0c-0305e82c3301",
                "WebAclArn": &web_acl_arn,
            }),
            "WAFNonexistentItemException",
        ),
        (
            "an event id that is no ARN's event",
            "GetEvent",
            json!({
                "Scope": REGIONAL,
                "EventId": &event_id,
                "WebAclArn": format!(
                    "arn:aws:wafv2:{TEST_REGION}:111122223333:regional/webacl/absent/\
                     3f2504e0-4f89-41d3-9a0c-0305e82c3301"
                ),
            }),
            "WAFNonexistentItemException",
        ),
        (
            "an event actor id nothing minted",
            "GetEventActor",
            json!({
                "Scope": REGIONAL,
                "EventActorId": "3f2504e0-4f89-41d3-9a0c-0305e82c3301",
            }),
            "WAFNonexistentItemException",
        ),
    ];

    for (case, operation, body, code) in cases {
        waf.refused(operation, body, code, case).await;
    }

    // The smallest and the largest page an events listing answers with are both
    // accepted: "valid values are 1 to 20".
    for limit in [1, 20] {
        let page = waf
            .ok("ListEvents", json!({ "Scope": REGIONAL, "Limit": limit }))
            .await;
        assert_eq!(
            listed_ids(&page["Events"].as_array().cloned().unwrap_or_default()),
            vec![event_id.clone()],
            "expected a Limit of {limit} to be accepted, got {page}"
        );
    }
}
