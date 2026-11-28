// SPDX-License-Identifier: CC0-1.0

//! The Amazon IVS FAILOVER SLATE CONFIGURATION, end to end on the assembled router.
//!
//! Every case goes through the real front door — `MultiServiceRouter::dispatch` with a
//! restJson1 request, exactly as a signed `aws ivs` call arrives — on a router carrying
//! BOTH services that sign as `ivs` (the low-latency endpoint and IVS RealTime), because
//! this resource adds a sixth ARN resource type to the `/tags/{resourceArn}` path space
//! those two services share: a tagging call on a failover-slate-configuration ARN has to
//! be claimed by the endpoint that owns the resource, and that is a property of the
//! assembled router rather than of either service alone.
//!
//! The rules under test, each stated once and then exercised over a SET of inputs:
//!
//! 1. A create persists and mints an ARN of the modelled form; every member the request
//!    omits is reported as the default the model documents for it; a read answers from
//!    what was persisted, and a list reports the same members in its summary.
//! 2. A value outside a modelled range is refused with a 400 `ValidationException`, and
//!    both ends of every range are accepted.
//! 3. The resource is created in a non-terminal state and the CLOCK settles it: a create
//!    answers `CREATING` and an update `UPDATING`, and each becomes `CREATED` on its own,
//!    with no further call needed.
//! 4. An update replaces the members the caller sent and keeps the rest.
//! 5. A list answers every persisted configuration, in one stable order, cut into pages
//!    of at most `maxResults` with a `nextToken` that resumes exactly where the previous
//!    page stopped; a token this service did not issue is a bad request.
//! 6. A channel reports the failover slate configuration it names on every projection and
//!    is selected by the list filter for it; a delete of a configuration a channel still
//!    names is refused `ConflictException` (409) until the channel lets it go; a delete
//!    makes the resource observably gone.
//! 7. The configuration's tags are readable and writable through the shared tagging
//!    surface by its own ARN, on the router where both `ivs`-signing services are mounted.

use std::collections::BTreeSet;
use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

/// A SigV4 `Authorization` header is how a request names the service and the region it is
/// addressed to, so every call carries one. The signature itself is never checked here:
/// these routers run with request authentication off.
const AUTHORIZATION: &str = "AWS4-HMAC-SHA256 \
     Credential=AKIDTESTIVS/20260101/us-east-1/ivs/aws4_request, \
     SignedHeaders=host, Signature=0000000000000000000000000000000000000000000000000000000000000000";

/// The ARN prefix every failover slate configuration this emulator mints must carry, from
/// the modelled `FailoverSlateConfigurationArn` pattern
/// `^arn:aws:ivs:[a-z0-9-]+:[0-9]+:failover-slate-configuration/[a-zA-Z0-9-]+$` plus the
/// canonical emulator account id.
const ARN_PREFIX: &str = "arn:aws:ivs:us-east-1:000000000000:failover-slate-configuration/";

/// The states of the modelled `FailoverSlateConfigurationState` enumeration this
/// lifecycle passes through (`smithy_query(query="shape",
/// shape="FailoverSlateConfigurationState")` gives `CREATING | CREATE_FAILED | CREATED |
/// UPDATING`).
const CREATING: &str = "CREATING";
const UPDATING: &str = "UPDATING";
const CREATED: &str = "CREATED";

/// The documented default of each member a create may omit, from the model docs on
/// `FailoverSlateConfiguration` (`activationThresholdSeconds` "Default: 3",
/// `minSlateDisplaySeconds` "Default: 3", `maxSlateDisplaySeconds` "Default: 60").
const DEFAULT_ACTIVATION_THRESHOLD_SECONDS: i64 = 3;
const DEFAULT_MIN_SLATE_DISPLAY_SECONDS: i64 = 3;
const DEFAULT_MAX_SLATE_DISPLAY_SECONDS: i64 = 60;

/// How long a poll keeps looking for a state the clock owes. Generous by design: it bounds
/// the transition having happened at all, not how fast this host runs, so a loaded machine
/// cannot fail a case a fast one passes.
const SETTLE_DEADLINE: Duration = Duration::from_secs(90);

/// How long a poll waits before looking again.
const POLL_INTERVAL: Duration = Duration::from_millis(50);

/// The two services that share the `ivs` signing name, over one shared engine, exactly as
/// `router/src/lib.rs` mounts them.
fn router_with_both_ivs_services() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_ivs_amazoninteractivevideoservice::EmulatedService::new(Arc::clone(
            &engine,
        )),
    );
    router.register_service(
        service_com_amazonaws_ivsrealtime_amazoninteractivevideoservicerealtime::EmulatedService::new(
            engine,
        ),
    );
    router
}

/// One answer from the router: the status, the restJson1 `__type` error code (the empty
/// string on a success) and the parsed body.
#[derive(Debug)]
struct Reply {
    status: u16,
    error_code: String,
    body: Value,
}

/// Send one request, with the body and the path a client sends.
async fn send(router: &MultiServiceRouter, method: &str, path: &str, body: Option<Value>) -> Reply {
    let request = http::Request::builder()
        .method(method)
        .uri(format!("https://ivs.us-east-1.amazonaws.com{path}"))
        .header("host", "ivs.us-east-1.amazonaws.com")
        .header("content-type", "application/json")
        .header("authorization", AUTHORIZATION)
        .body(
            body.map(|body| Bytes::from(body.to_string()))
                .unwrap_or_default(),
        )
        .expect("the test request is well formed");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let parsed: Value = if response.body().is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(response.body()).unwrap_or(Value::Null)
    };
    let error_code = parsed
        .get("__type")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    Reply {
        status,
        error_code,
        body: parsed,
    }
}

/// One modelled operation, bound to `POST /<Operation>`.
async fn post(router: &MultiServiceRouter, operation: &str, body: Value) -> Reply {
    send(router, "POST", &format!("/{operation}"), Some(body)).await
}

/// Call `operation` and require it to have succeeded, returning the body.
async fn ok(router: &MultiServiceRouter, operation: &str, body: Value) -> Value {
    let reply = post(router, operation, body.clone()).await;
    assert_eq!(
        (reply.status, reply.error_code.as_str()),
        (200, ""),
        "case {operation} {body}: expected HTTP 200, got {} {:?} ({})",
        reply.status,
        reply.error_code,
        reply.body
    );
    reply.body
}

/// Create a failover slate configuration and answer the configuration object.
async fn create(router: &MultiServiceRouter, request: Value) -> Value {
    ok(router, "CreateFailoverSlateConfiguration", request).await["failoverSlateConfiguration"]
        .clone()
}

/// Read one back and answer the configuration object.
async fn get(router: &MultiServiceRouter, arn: &str) -> Value {
    ok(
        router,
        "GetFailoverSlateConfiguration",
        json!({ "arn": arn }),
    )
    .await["failoverSlateConfiguration"]
        .clone()
}

/// One of the three operations bound to `/tags/{resourceArn}`, which carry the ARN in the
/// URI the way a client does: percent-encoded into one path segment.
async fn tagging(
    router: &MultiServiceRouter,
    method: &str,
    arn: &str,
    query: &str,
    body: Option<Value>,
) -> Reply {
    let path = format!("/tags/{}{query}", encode_path_label(arn));
    send(router, method, &path, body).await
}

/// Percent-encode one `@httpLabel` value, the way an SDK encodes a non-greedy label: an
/// ARN's `:` and `/` are escaped, so the whole ARN is ONE path segment.
fn encode_path_label(value: &str) -> String {
    value
        .bytes()
        .map(|byte| match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => {
                (byte as char).to_string()
            }
            other => format!("%{other:02X}"),
        })
        .collect()
}

/// A string member, read as the empty string when absent — which is what a client sees for
/// a member the wire omits.
fn text(value: &Value, key: &str) -> String {
    value
        .get(key)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

/// An integer member, or `None` when the wire omits it.
fn number(value: &Value, key: &str) -> Option<i64> {
    value.get(key).and_then(Value::as_i64)
}

/// Look again until the configuration `arn` names reports `waiting_for`, or
/// [`SETTLE_DEADLINE`] passes: the condition is what the test waits for, never a fixed
/// time.
async fn poll_until_state(
    router: &MultiServiceRouter,
    case: &str,
    arn: &str,
    waiting_for: &str,
) -> Value {
    let deadline = std::time::Instant::now() + SETTLE_DEADLINE;
    loop {
        let seen = get(router, arn).await;
        if text(&seen, "state") == waiting_for {
            return seen;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "case {case:?}: expected the state {waiting_for:?} within the deadline, got {seen}"
        );
        tokio::time::sleep(POLL_INTERVAL).await;
    }
}

/// One row of rule 1: the create request, and what every projection of the created
/// resource must report for the four configured members.
struct Case {
    case: &'static str,
    request: Value,
    name: &'static str,
    activation: i64,
    minimum: i64,
    maximum: i64,
}

/// The table of rule 1: the rows differ along which members the request names and, for
/// each second count, along both ends of the range the model declares for it
/// (`activationThresholdSeconds` 3..5, `minSlateDisplaySeconds` 3..10,
/// `maxSlateDisplaySeconds` 60..180 — `smithy_query(query="shape",
/// shape="FailoverSlateActivationThresholdSeconds,FailoverSlateMinDisplaySeconds,\
/// FailoverSlateMaxDisplaySeconds")`).
fn table() -> Vec<Case> {
    vec![
        Case {
            case: "an empty request takes every documented default",
            request: json!({}),
            name: "",
            activation: DEFAULT_ACTIVATION_THRESHOLD_SECONDS,
            minimum: DEFAULT_MIN_SLATE_DISPLAY_SECONDS,
            maximum: DEFAULT_MAX_SLATE_DISPLAY_SECONDS,
        },
        Case {
            case: "a named configuration with no second count",
            request: json!({ "name": "named-slate" }),
            name: "named-slate",
            activation: DEFAULT_ACTIVATION_THRESHOLD_SECONDS,
            minimum: DEFAULT_MIN_SLATE_DISPLAY_SECONDS,
            maximum: DEFAULT_MAX_SLATE_DISPLAY_SECONDS,
        },
        Case {
            case: "every second count at the LOW end of its modelled range",
            request: json!({ "name": "low-ends", "activationThresholdSeconds": 3,
                             "minSlateDisplaySeconds": 3, "maxSlateDisplaySeconds": 60 }),
            name: "low-ends",
            activation: 3,
            minimum: 3,
            maximum: 60,
        },
        Case {
            case: "every second count at the HIGH end of its modelled range",
            request: json!({ "name": "high-ends", "activationThresholdSeconds": 5,
                             "minSlateDisplaySeconds": 10, "maxSlateDisplaySeconds": 180 }),
            name: "high-ends",
            activation: 5,
            minimum: 10,
            maximum: 180,
        },
        Case {
            case: "one member named and the other two defaulted",
            request: json!({ "name": "part-named", "minSlateDisplaySeconds": 7 }),
            name: "part-named",
            activation: DEFAULT_ACTIVATION_THRESHOLD_SECONDS,
            minimum: 7,
            maximum: DEFAULT_MAX_SLATE_DISPLAY_SECONDS,
        },
        Case {
            case: "a tagged configuration with a mid-range threshold",
            request: json!({ "name": "tagged-slate", "activationThresholdSeconds": 4,
                             "tags": { "team": "video" } }),
            name: "tagged-slate",
            activation: 4,
            minimum: DEFAULT_MIN_SLATE_DISPLAY_SECONDS,
            maximum: DEFAULT_MAX_SLATE_DISPLAY_SECONDS,
        },
    ]
}

/// Assert one configuration object reports the members `row` expects.
fn assert_members(case: &str, projection: &str, configuration: &Value, row: &Case) {
    assert_eq!(
        text(configuration, "name"),
        row.name,
        "case {case:?}: {projection} must report name {:?}, got {configuration}",
        row.name
    );
    for (member, expected) in [
        ("activationThresholdSeconds", row.activation),
        ("minSlateDisplaySeconds", row.minimum),
        ("maxSlateDisplaySeconds", row.maximum),
    ] {
        let actual = number(configuration, member);
        assert_eq!(
            actual,
            Some(expected),
            "case {case:?}: {projection} must report {member} as {expected}, got {actual:?} \
             in {configuration}"
        );
    }
}

/// Rule 1: a create persists, mints a modelled ARN, defaults every member the request
/// omitted, and the read and the list answer from what was persisted.
#[tokio::test]
async fn a_created_failover_slate_configuration_reads_back_with_the_documented_defaults() {
    let router = router_with_both_ivs_services();
    for row in table() {
        let created = create(&router, row.request.clone()).await;
        let arn = text(&created, "arn");
        assert!(
            arn.starts_with(ARN_PREFIX) && arn.len() <= 128 && arn.len() > ARN_PREFIX.len(),
            "case {:?}: expected a modelled FailoverSlateConfigurationArn, got {arn:?}",
            row.case
        );
        assert_eq!(
            text(&created, "state"),
            CREATING,
            "case {:?}: a create answers the non-terminal state, got {created}",
            row.case
        );
        assert_members(row.case, "CreateFailoverSlateConfiguration", &created, &row);

        let fetched = get(&router, &arn).await;
        assert_members(row.case, "GetFailoverSlateConfiguration", &fetched, &row);
        assert_eq!(
            text(&fetched, "arn"),
            arn,
            "case {:?}: the read answers the resource that was created, got {fetched}",
            row.case
        );

        let listed = ok(&router, "ListFailoverSlateConfigurations", json!({})).await;
        let summary = listed["failoverSlateConfigurations"]
            .as_array()
            .and_then(|page| {
                page.iter()
                    .find(|summary| text(summary, "arn") == arn)
                    .cloned()
            })
            .unwrap_or_else(|| panic!("case {:?}: the list reports it, got {listed}", row.case));
        assert_members(row.case, "ListFailoverSlateConfigurations", &summary, &row);

        // The tags the create persisted are reported by the read, whether the request
        // named any or not.
        let expected_tags = row
            .request
            .get("tags")
            .cloned()
            .unwrap_or_else(|| json!({}));
        assert_eq!(
            fetched.get("tags").cloned().unwrap_or(Value::Null),
            expected_tags,
            "case {:?}: the read answers the tags the create persisted, got {fetched}",
            row.case
        );
    }
}

/// Rule 2: a value outside a modelled range is refused, and one just past each published
/// bound is refused too.
#[tokio::test]
async fn a_second_count_outside_its_modelled_range_is_refused() {
    let router = router_with_both_ivs_services();

    // One row per bound of each range, taken one step past it.
    let rows: [(&str, Value); 6] = [
        (
            "activationThresholdSeconds one below its minimum",
            json!({ "activationThresholdSeconds": 2 }),
        ),
        (
            "activationThresholdSeconds one above its maximum",
            json!({ "activationThresholdSeconds": 6 }),
        ),
        (
            "minSlateDisplaySeconds one below its minimum",
            json!({ "minSlateDisplaySeconds": 2 }),
        ),
        (
            "minSlateDisplaySeconds one above its maximum",
            json!({ "minSlateDisplaySeconds": 11 }),
        ),
        (
            "maxSlateDisplaySeconds one below its minimum",
            json!({ "maxSlateDisplaySeconds": 59 }),
        ),
        (
            "maxSlateDisplaySeconds one above its maximum",
            json!({ "maxSlateDisplaySeconds": 181 }),
        ),
    ];
    for (case, request) in rows {
        let reply = post(&router, "CreateFailoverSlateConfiguration", request.clone()).await;
        assert_eq!(
            (reply.status, reply.error_code.as_str()),
            (400, "ValidationException"),
            "case {case:?}: expected HTTP 400 ValidationException for {request}, got {} {:?} \
             ({})",
            reply.status,
            reply.error_code,
            reply.body
        );
    }

    // A name outside the modelled `FailoverSlateConfigurationName` pattern
    // `^[a-zA-Z0-9-_]*$` is refused the same way, and the longest name the length admits
    // is accepted.
    let refused = post(
        &router,
        "CreateFailoverSlateConfiguration",
        json!({ "name": "not a valid name!" }),
    )
    .await;
    assert_eq!(
        (refused.status, refused.error_code.as_str()),
        (400, "ValidationException"),
        "case \"a name outside the modelled pattern\": expected HTTP 400 ValidationException, \
         got {} {:?} ({})",
        refused.status,
        refused.error_code,
        refused.body
    );
    let longest = "n".repeat(128);
    let created = create(&router, json!({ "name": longest.clone() })).await;
    assert_eq!(
        text(&created, "name"),
        longest,
        "case \"the longest name the model admits\": it is accepted and reported, got {created}"
    );
}

/// Rule 3 and rule 4: the create's `CREATING` settles to `CREATED` on its own, an update
/// answers `UPDATING` and settles the same way, and an update replaces only what it named.
#[tokio::test]
async fn the_state_settles_on_its_own_and_an_update_replaces_only_what_it_names() {
    let router = router_with_both_ivs_services();
    let created = create(
        &router,
        json!({ "name": "settling", "activationThresholdSeconds": 4,
                "minSlateDisplaySeconds": 6, "maxSlateDisplaySeconds": 120 }),
    )
    .await;
    let arn = text(&created, "arn");

    let settled = poll_until_state(&router, "a created configuration", &arn, CREATED).await;
    assert_eq!(
        number(&settled, "activationThresholdSeconds"),
        Some(4),
        "case \"a created configuration\": settling changes the state and nothing else, \
         got {settled}"
    );

    // Each row updates ONE member (and the last two together), and every member it does
    // not name must keep the value the previous call left.
    let rows: [(&str, Value, &str, i64, i64, i64); 4] = [
        ("the name alone", json!({ "name": "renamed" }), "renamed", 4, 6, 120),
        (
            "the activation threshold alone",
            json!({ "activationThresholdSeconds": 5 }),
            "renamed",
            5,
            6,
            120,
        ),
        (
            "both display bounds together",
            json!({ "minSlateDisplaySeconds": 10, "maxSlateDisplaySeconds": 180 }),
            "renamed",
            5,
            10,
            180,
        ),
        (
            "nothing but the ARN",
            json!({}),
            "renamed",
            5,
            10,
            180,
        ),
    ];
    for (case, mut change, name, activation, minimum, maximum) in rows {
        change["arn"] = json!(arn.clone());
        let updated = ok(&router, "UpdateFailoverSlateConfiguration", change.clone()).await
            ["failoverSlateConfiguration"]
            .clone();
        assert_eq!(
            text(&updated, "state"),
            UPDATING,
            "case {case:?}: an applied update answers the non-terminal state, got {updated}"
        );
        let expected = Case {
            case,
            request: change,
            name,
            activation,
            minimum,
            maximum,
        };
        assert_members(case, "UpdateFailoverSlateConfiguration", &updated, &expected);

        let settled = poll_until_state(&router, case, &arn, CREATED).await;
        assert_members(case, "the settled read", &settled, &expected);
    }

    // An update of something this store never held names no resource to change.
    let absent = post(
        &router,
        "UpdateFailoverSlateConfiguration",
        json!({ "arn": format!("{ARN_PREFIX}absentslate"), "name": "ghost" }),
    )
    .await;
    assert_eq!(
        (absent.status, absent.error_code.as_str()),
        (404, "ResourceNotFoundException"),
        "case \"an update of a configuration that does not exist\": expected HTTP 404 \
         ResourceNotFoundException, got {} {:?} ({})",
        absent.status,
        absent.error_code,
        absent.body
    );
}

/// Rule 5: the list pages over every persisted configuration, in one stable order, and a
/// token it did not issue is a bad request.
#[tokio::test]
async fn the_list_pages_over_every_configuration_and_refuses_a_token_it_did_not_issue() {
    let router = router_with_both_ivs_services();

    // The configurations differ in shape, not just in name: each row names a different
    // set of members.
    let shapes = [
        json!({ "name": "page-a" }),
        json!({ "name": "page-b", "activationThresholdSeconds": 5 }),
        json!({ "name": "page-c", "minSlateDisplaySeconds": 10, "tags": { "team": "video" } }),
        json!({ "name": "page-d", "maxSlateDisplaySeconds": 180 }),
        json!({ "name": "page-e", "activationThresholdSeconds": 4,
                "minSlateDisplaySeconds": 5, "maxSlateDisplaySeconds": 90 }),
    ];
    let mut minted = BTreeSet::new();
    for shape in shapes {
        minted.insert(text(&create(&router, shape).await, "arn"));
    }

    // Every page size from one to more than the set, so a page that fits everything and
    // one that cuts the set are both covered.
    for page_size in [1_usize, 2, 5, 100] {
        let mut seen: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        loop {
            let mut request = json!({ "maxResults": page_size });
            if let Some(token) = &token {
                request["nextToken"] = json!(token);
            }
            let page = ok(&router, "ListFailoverSlateConfigurations", request).await;
            let configurations = page["failoverSlateConfigurations"]
                .as_array()
                .cloned()
                .unwrap_or_default();
            assert!(
                !configurations.is_empty() && configurations.len() <= page_size,
                "case \"maxResults {page_size}\": a page holds at least one and at most \
                 {page_size} entries, got {page}"
            );
            seen.extend(configurations.iter().map(|entry| text(entry, "arn")));
            token = page
                .get("nextToken")
                .and_then(Value::as_str)
                .map(str::to_string);
            if token.is_none() {
                break;
            }
        }
        let mut ordered = seen.clone();
        ordered.sort();
        ordered.dedup();
        assert_eq!(
            seen.len(),
            ordered.len(),
            "case \"maxResults {page_size}\": no configuration is reported twice across the \
             pages, got {seen:?}"
        );
        assert_eq!(
            ordered.into_iter().collect::<BTreeSet<_>>(),
            minted,
            "case \"maxResults {page_size}\": the pages together hold every persisted \
             configuration, got {seen:?}"
        );
    }

    // A token this service did not issue for THIS list: a bad request, not an empty page.
    // The token codec is hex, so a value that decodes cleanly and still is not a boundary
    // this list issued is built by hand.
    let forged: String = format!("ivs:1:channel:{ARN_PREFIX}forged")
        .bytes()
        .map(|byte| format!("{byte:02x}"))
        .collect();
    for (case, token) in [
        ("a token stamped for another list", forged),
        ("a token that is not hex at all", "not-a-token".to_string()),
    ] {
        let reply = post(
            &router,
            "ListFailoverSlateConfigurations",
            json!({ "nextToken": token }),
        )
        .await;
        assert_eq!(
            (reply.status, reply.error_code.as_str()),
            (400, "ValidationException"),
            "case {case:?}: expected HTTP 400 ValidationException, got {} {:?} ({})",
            reply.status,
            reply.error_code,
            reply.body
        );
    }
}

/// Rule 6: a channel reports the configuration it names, the list filter selects on it,
/// and the delete is refused while the channel still holds the reference.
#[tokio::test]
async fn a_channel_holds_the_configuration_against_its_delete_until_it_lets_go() {
    let router = router_with_both_ivs_services();
    let configuration = create(&router, json!({ "name": "attached" })).await;
    let arn = text(&configuration, "arn");

    // Rows: the channel names the configuration at creation, and the channel names it
    // through an update instead — both are references that must hold it.
    let attached_at_creation = ok(
        &router,
        "CreateChannel",
        json!({ "name": "with-slate", "failoverSlateConfigurationArn": arn.clone() }),
    )
    .await["channel"]
        .clone();
    let bare = ok(&router, "CreateChannel", json!({ "name": "no-slate" })).await["channel"].clone();
    let bare_arn = text(&bare, "arn");
    let attached_by_update = ok(
        &router,
        "UpdateChannel",
        json!({ "arn": bare_arn.clone(), "failoverSlateConfigurationArn": arn.clone() }),
    )
    .await["channel"]
        .clone();

    for (case, channel) in [
        ("a channel created with the configuration", &attached_at_creation),
        ("a channel updated onto the configuration", &attached_by_update),
    ] {
        assert_eq!(
            text(channel, "failoverSlateConfigurationArn"),
            arn,
            "case {case:?}: the channel reports the configuration it names, got {channel}"
        );
        let channel_arn = text(channel, "arn");
        let fetched = ok(&router, "GetChannel", json!({ "arn": channel_arn.clone() })).await
            ["channel"]
            .clone();
        assert_eq!(
            text(&fetched, "failoverSlateConfigurationArn"),
            arn,
            "case {case:?}: GetChannel reports it too, got {fetched}"
        );
        let filtered = ok(
            &router,
            "ListChannels",
            json!({ "filterByFailoverSlateConfigurationArn": arn.clone() }),
        )
        .await;
        let selected: BTreeSet<String> = filtered["channels"]
            .as_array()
            .cloned()
            .unwrap_or_default()
            .iter()
            .map(|summary| text(summary, "arn"))
            .collect();
        assert!(
            selected.contains(&channel_arn),
            "case {case:?}: the list filter selects the channel that names it, got {filtered}"
        );

        // The delete is refused while that channel holds the reference.
        let refused = post(
            &router,
            "DeleteFailoverSlateConfiguration",
            json!({ "arn": arn.clone() }),
        )
        .await;
        assert_eq!(
            (refused.status, refused.error_code.as_str()),
            (409, "ConflictException"),
            "case {case:?}: a configuration a channel still names cannot be deleted, got {} \
             {:?} ({})",
            refused.status,
            refused.error_code,
            refused.body
        );
        let message = text(&refused.body, "message");
        assert!(
            message.contains("DeleteFailoverSlateConfiguration") && message.contains(&arn),
            "case {case:?}: the refusal names the action it refused and the resource the \
             request named, got {message:?}"
        );

        // The channel lets go by naming the empty string, which the model documents as
        // "failover slate configuration is disabled".
        let detached = ok(
            &router,
            "UpdateChannel",
            json!({ "arn": channel_arn, "failoverSlateConfigurationArn": "" }),
        )
        .await["channel"]
            .clone();
        assert_eq!(
            text(&detached, "failoverSlateConfigurationArn"),
            "",
            "case {case:?}: a channel that disabled it reports the unconfigured value, \
             got {detached}"
        );
    }

    // Nothing references it now, so the delete goes through and the resource is gone.
    let deleted = post(
        &router,
        "DeleteFailoverSlateConfiguration",
        json!({ "arn": arn.clone() }),
    )
    .await;
    assert_eq!(
        deleted.status, 204,
        "case \"a configuration nothing references\": expected HTTP 204, got {} ({})",
        deleted.status, deleted.body
    );
    for (case, operation, request) in [
        (
            "a read of the deleted configuration",
            "GetFailoverSlateConfiguration",
            json!({ "arn": arn.clone() }),
        ),
        (
            "a second delete of the same configuration",
            "DeleteFailoverSlateConfiguration",
            json!({ "arn": arn.clone() }),
        ),
        (
            "a read of an ARN this store never held",
            "GetFailoverSlateConfiguration",
            json!({ "arn": format!("{ARN_PREFIX}neverheld") }),
        ),
    ] {
        let reply = post(&router, operation, request).await;
        assert_eq!(
            (reply.status, reply.error_code.as_str()),
            (404, "ResourceNotFoundException"),
            "case {case:?}: expected HTTP 404 ResourceNotFoundException, got {} {:?} ({})",
            reply.status,
            reply.error_code,
            reply.body
        );
    }
    let listed = ok(&router, "ListFailoverSlateConfigurations", json!({})).await;
    let remaining: Vec<String> = listed["failoverSlateConfigurations"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|summary| text(summary, "arn"))
        .collect();
    assert!(
        !remaining.contains(&arn),
        "case \"the list after the delete\": the deleted configuration is gone, got {remaining:?}"
    );
}

/// Rule 7: the configuration's tags are read and written through the shared
/// `/tags/{resourceArn}` surface by its own ARN — on the router where both services that
/// sign as `ivs` are mounted, so the request is claimed by the endpoint that owns this
/// resource type.
#[tokio::test]
async fn the_configurations_tags_round_trip_through_the_shared_tagging_surface() {
    let router = router_with_both_ivs_services();

    // Rows: a configuration created WITH tags and one created without, because the
    // surface must answer the set the create persisted either way.
    let rows = [
        (
            "a configuration tagged at creation",
            json!({ "name": "tagged", "tags": { "probe": "failover-slate" } }),
            json!({ "probe": "failover-slate" }),
        ),
        (
            "a configuration created without tags",
            json!({ "name": "untagged" }),
            json!({}),
        ),
    ];
    for (case, request, at_creation) in rows {
        let arn = text(&create(&router, request).await, "arn");

        let listed = tagging(&router, "GET", &arn, "", None).await;
        assert_eq!(
            (listed.status, listed.error_code.as_str()),
            (200, ""),
            "case {case:?}: ListTagsForResource on {arn} must be served by the endpoint that \
             owns it, got {} {:?} ({})",
            listed.status,
            listed.error_code,
            listed.body
        );
        assert_eq!(
            listed.body.get("tags").cloned().unwrap_or(Value::Null),
            at_creation,
            "case {case:?}: the surface answers the tags the create persisted, got {}",
            listed.body
        );

        let written = tagging(
            &router,
            "POST",
            &arn,
            "",
            Some(json!({ "tags": { "team": "video" } })),
        )
        .await;
        assert_eq!(
            written.status, 204,
            "case {case:?}: TagResource answers no content, got {} ({})",
            written.status, written.body
        );
        let after_write = tagging(&router, "GET", &arn, "", None).await;
        assert_eq!(
            after_write.body["tags"]["team"],
            json!("video"),
            "case {case:?}: a written tag is readable back, got {}",
            after_write.body
        );
        // The write is on the resource, so the read of the resource itself reports it too.
        let read_back = get(&router, &arn).await;
        assert_eq!(
            read_back["tags"]["team"],
            json!("video"),
            "case {case:?}: the resource read reports the written tag, got {read_back}"
        );

        let removed = tagging(&router, "DELETE", &arn, "?tagKeys=team", None).await;
        assert_eq!(
            removed.status, 204,
            "case {case:?}: UntagResource answers no content, got {} ({})",
            removed.status, removed.body
        );
        let after_removal = tagging(&router, "GET", &arn, "", None).await;
        assert_eq!(
            after_removal.body.get("tags").cloned().unwrap_or(Value::Null),
            at_creation,
            "case {case:?}: the named key is gone and the rest is kept, got {}",
            after_removal.body
        );
    }
}
