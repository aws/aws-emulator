// SPDX-License-Identifier: CC0-1.0

//! Cross-service routing for the `ivs` SigV4 signing name.
//!
//! Amazon IVS (low latency) and Amazon IVS RealTime SHARE one signing name (`ivs`) and
//! one `arnNamespace` while serving two different endpoints (`ivs` and `ivsrealtime`).
//! The router resolves a request's routing key from the SigV4 credential scope first, so
//! both services see the key `ivs`: a service that claims on the signing name alone
//! claims the other's traffic too, and the router answers `500 AmbiguousService` for
//! BOTH, which is an error code no AWS model declares.
//!
//! The rule these suites pin: a request signed for a signing name several emulated
//! services share is claimed by exactly ONE of them — the one whose own model binds the
//! request's URI path, and, on a path both models bind (the `/tags/{resourceArn}`
//! tagging space), the one that owns the resource the ARN in that path names.
//!
//! Rows therefore span the dimensions that rule generalises over: which endpoint owns
//! the operation, every resource type either model declares an ARN shape for, both wire
//! forms of a path-label ARN (percent-encoded and raw), an ARN neither model knows, and
//! a `Host` that agrees with, disagrees with, or says nothing about the target — because
//! on a signed request the credential scope, not the host, resolves the key.

use std::sync::Arc;

use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::{MultiServiceRouter, Service};

/// A SigV4 `Authorization` header whose credential scope names the signing name both
/// services share. The signature is never checked: the routers here run with request
/// authentication off.
const AUTHORIZATION: &str = "AWS4-HMAC-SHA256 \
     Credential=AKIDTESTIVS/20260101/us-east-1/ivs/aws4_request, \
     SignedHeaders=host, Signature=0000000000000000000000000000000000000000000000000000000000000000";

/// How a row names the service that must claim it. `Both` and `Neither` are the two
/// routing failures: `Both` is the `500 AmbiguousService` this issue is about, `Neither`
/// the router's `404 UnknownService`.
const OWNER_IVS: &str = "ivs";
const OWNER_REALTIME: &str = "ivsrealtime";
const OWNER_BOTH: &str = "both (the router answers 500 AmbiguousService)";
const OWNER_NEITHER: &str = "neither (the router answers 404 UnknownService)";

type Ivs = service_com_amazonaws_ivs_amazoninteractivevideoservice::EmulatedService<
    engine::InMemoryEngine,
>;
type RealTime =
    service_com_amazonaws_ivsrealtime_amazoninteractivevideoservicerealtime::EmulatedService<
        engine::InMemoryEngine,
    >;

/// The two services that share the `ivs` signing name, over one shared engine, exactly
/// as `router/src/lib.rs` mounts them.
fn both_services() -> (Ivs, RealTime) {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    (Ivs::new(Arc::clone(&engine)), RealTime::new(engine))
}

/// A router with both of them mounted, so a request that two services claim really is
/// refused as `AmbiguousService`.
fn router_with_both_services() -> MultiServiceRouter {
    let (ivs, realtime) = both_services();
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(ivs);
    router.register_service(realtime);
    router
}

/// The request parts the router hands to `Service::can_handle`.
fn parts(method: &str, host: &str, uri: &str) -> http::request::Parts {
    let request = http::Request::builder()
        .method(method)
        .uri(uri)
        .header("host", host)
        .header("content-type", "application/json")
        .header("authorization", AUTHORIZATION)
        .body(Bytes::new())
        .expect("the test request is well formed");
    request.into_parts().0
}

/// An ARN as a client puts it in the non-greedy `{resourceArn}` path label: its `:` and
/// `/` percent-encoded.
fn label(arn: &str) -> String {
    arn.replace(':', "%3A").replace('/', "%2F")
}

/// The service that claims a request, as one of the four `OWNER_*` outcomes.
fn claimant(ivs: &Ivs, realtime: &RealTime, parts: &http::request::Parts) -> &'static str {
    match (ivs.can_handle(parts), realtime.can_handle(parts)) {
        (true, false) => OWNER_IVS,
        (false, true) => OWNER_REALTIME,
        (true, true) => OWNER_BOTH,
        (false, false) => OWNER_NEITHER,
    }
}

/// One answer from the router: the HTTP status and the restJson1 `__type` error code
/// (the empty string on a success, which carries no `__type`).
async fn call(
    router: &MultiServiceRouter,
    method: &str,
    uri: &str,
    body: Value,
) -> (u16, String, Value) {
    let payload = if body.is_null() {
        String::new()
    } else {
        body.to_string()
    };
    let request = http::Request::builder()
        .method(method)
        .uri(uri)
        .header("host", "localhost:9297")
        .header("content-type", "application/json")
        .header("authorization", AUTHORIZATION)
        .body(Bytes::from(payload))
        .expect("the test request is well formed");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let parsed: Value = if response.body().is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(response.body()).unwrap_or(Value::Null)
    };
    let code = parsed
        .get("__type")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    (status, code, parsed)
}

#[test]
fn exactly_one_service_sharing_the_ivs_signing_name_claims_each_request() {
    let (ivs, realtime) = both_services();

    // A 36-row table: (case, method, host, uri, the service that must claim it)
    //
    // The operation paths come from each model's `smithy.api#http` bindings
    // (`smithy_query(query="digest", …)` on the two models); the ARNs are built from the
    // resource segment each model's ARN shapes pin — `channel`, `stream-key`,
    // `playback-key`, `playback-restriction-policy`, `recording-configuration`,
    // `ad-configuration` and `failover-slate-configuration` on the `ivs` model, `stage`,
    // `encoder-configuration`,
    // `storage-configuration`, `composition`, `public-key` and `ingest-configuration` on
    // the `ivsrealtime` one.
    let rows: Vec<(String, &str, &str, String, &str)> = vec![
        // The low-latency endpoint's own operations, on hosts that name it, name its
        // sibling, and name nothing at all.
        (
            "ivs CreateChannel".to_string(),
            "POST",
            "ivs.us-east-1.amazonaws.com",
            "/CreateChannel".to_string(),
            OWNER_IVS,
        ),
        (
            "ivs CreateChannel on the sibling's host".to_string(),
            "POST",
            "ivsrealtime.us-east-1.amazonaws.com",
            "/CreateChannel".to_string(),
            OWNER_IVS,
        ),
        (
            "ivs GetChannel on a bare emulator endpoint".to_string(),
            "POST",
            "localhost:9297",
            "/GetChannel".to_string(),
            OWNER_IVS,
        ),
        (
            "ivs ListRecordingConfigurations".to_string(),
            "POST",
            "localhost:9297",
            "/ListRecordingConfigurations".to_string(),
            OWNER_IVS,
        ),
        (
            "ivs ImportPlaybackKeyPair".to_string(),
            "POST",
            "localhost:9297",
            "/ImportPlaybackKeyPair".to_string(),
            OWNER_IVS,
        ),
        (
            "ivs ListPlaybackKeyPairs".to_string(),
            "POST",
            "localhost:9297",
            "/ListPlaybackKeyPairs".to_string(),
            OWNER_IVS,
        ),
        (
            "ivs CreateFailoverSlateConfiguration".to_string(),
            "POST",
            "localhost:9297",
            "/CreateFailoverSlateConfiguration".to_string(),
            OWNER_IVS,
        ),
        (
            "ivs CreatePlaybackRestrictionPolicy".to_string(),
            "POST",
            "localhost:9297",
            "/CreatePlaybackRestrictionPolicy".to_string(),
            OWNER_IVS,
        ),
        // The RealTime endpoint's own operations, likewise.
        (
            "realtime CreateStage".to_string(),
            "POST",
            "ivsrealtime.us-west-2.amazonaws.com",
            "/CreateStage".to_string(),
            OWNER_REALTIME,
        ),
        (
            "realtime CreateStage on the sibling's host".to_string(),
            "POST",
            "ivs.us-west-2.amazonaws.com",
            "/CreateStage".to_string(),
            OWNER_REALTIME,
        ),
        (
            "realtime GetStage on a bare emulator endpoint".to_string(),
            "POST",
            "localhost:9297",
            "/GetStage".to_string(),
            OWNER_REALTIME,
        ),
        (
            "realtime ListPublicKeys".to_string(),
            "POST",
            "localhost:9297",
            "/ListPublicKeys".to_string(),
            OWNER_REALTIME,
        ),
        (
            "realtime ListParticipants".to_string(),
            "POST",
            "localhost:9297",
            "/ListParticipants".to_string(),
            OWNER_REALTIME,
        ),
        (
            "realtime StartComposition".to_string(),
            "POST",
            "localhost:9297",
            "/StartComposition".to_string(),
            OWNER_REALTIME,
        ),
        // The shared `/tags/{resourceArn}` path space: one row per resource type either
        // model declares, across all three tagging methods and several regions and
        // accounts.
        (
            "tags: list on a channel".to_string(),
            "GET",
            "localhost:9297",
            format!(
                "/tags/{}",
                label("arn:aws:ivs:us-east-1:000000000000:channel/abcdef123456")
            ),
            OWNER_IVS,
        ),
        (
            "tags: tag a stream key".to_string(),
            "POST",
            "localhost:9297",
            format!(
                "/tags/{}",
                label("arn:aws:ivs:eu-west-1:123456789012:stream-key/0a1b2c3d4e5f")
            ),
            OWNER_IVS,
        ),
        (
            "tags: untag a playback key pair".to_string(),
            "DELETE",
            "localhost:9297",
            format!(
                "/tags/{}",
                label("arn:aws:ivs:ap-northeast-1:000000000000:playback-key/key-1")
            ),
            OWNER_IVS,
        ),
        (
            "tags: list on a playback restriction policy".to_string(),
            "GET",
            "localhost:9297",
            format!(
                "/tags/{}",
                label(
                    "arn:aws:ivs:us-east-1:000000000000:playback-restriction-policy/policy-1"
                )
            ),
            OWNER_IVS,
        ),
        (
            "tags: list on a recording configuration".to_string(),
            "GET",
            "localhost:9297",
            format!(
                "/tags/{}",
                label("arn:aws:ivs:us-east-1:000000000000:recording-configuration/rec-1")
            ),
            OWNER_IVS,
        ),
        (
            "tags: list on an ad configuration".to_string(),
            "GET",
            "localhost:9297",
            format!(
                "/tags/{}",
                label("arn:aws:ivs:us-east-1:000000000000:ad-configuration/ad-1")
            ),
            OWNER_IVS,
        ),
        (
            "tags: list on a failover slate configuration".to_string(),
            "GET",
            "localhost:9297",
            format!(
                "/tags/{}",
                label(
                    "arn:aws:ivs:us-east-1:000000000000:failover-slate-configuration/slate-1"
                )
            ),
            OWNER_IVS,
        ),
        (
            "tags: list on a stage".to_string(),
            "GET",
            "localhost:9297",
            format!(
                "/tags/{}",
                label("arn:aws:ivs:us-east-1:000000000000:stage/stage-1")
            ),
            OWNER_REALTIME,
        ),
        (
            "tags: tag an encoder configuration".to_string(),
            "POST",
            "localhost:9297",
            format!(
                "/tags/{}",
                label("arn:aws:ivs:us-east-2:210987654321:encoder-configuration/enc-1")
            ),
            OWNER_REALTIME,
        ),
        (
            "tags: untag a storage configuration".to_string(),
            "DELETE",
            "localhost:9297",
            format!(
                "/tags/{}",
                label("arn:aws:ivs:us-east-1:000000000000:storage-configuration/store-1")
            ),
            OWNER_REALTIME,
        ),
        (
            "tags: list on a composition".to_string(),
            "GET",
            "localhost:9297",
            format!(
                "/tags/{}",
                label("arn:aws:ivs:us-east-1:000000000000:composition/comp-1")
            ),
            OWNER_REALTIME,
        ),
        (
            "tags: list on a public key".to_string(),
            "GET",
            "localhost:9297",
            format!(
                "/tags/{}",
                label("arn:aws:ivs:us-east-1:000000000000:public-key/pub-1")
            ),
            OWNER_REALTIME,
        ),
        (
            "tags: list on an ingest configuration".to_string(),
            "GET",
            "localhost:9297",
            format!(
                "/tags/{}",
                label("arn:aws:ivs:us-east-1:000000000000:ingest-configuration/ing-1")
            ),
            OWNER_REALTIME,
        ),
        // The same two resource types with the ARN's separators sent RAW rather than
        // percent-encoded: the resource type decides either way.
        (
            "tags: raw (unencoded) channel ARN".to_string(),
            "GET",
            "localhost:9297",
            "/tags/arn:aws:ivs:us-east-1:000000000000:channel/abcdef123456".to_string(),
            OWNER_IVS,
        ),
        (
            "tags: raw (unencoded) stage ARN".to_string(),
            "GET",
            "localhost:9297",
            "/tags/arn:aws:ivs:us-east-1:000000000000:stage/stage-1".to_string(),
            OWNER_REALTIME,
        ),
        // An ARN neither model recognises, and no ARN at all: the endpoint whose
        // `endpointPrefix` IS the shared signing name answers, so the caller gets a
        // modelled 4xx from a real service rather than the router's 404.
        (
            "tags: an ARN of an unknown resource type".to_string(),
            "GET",
            "localhost:9297",
            format!(
                "/tags/{}",
                label("arn:aws:ivs:us-east-1:000000000000:unknown-thing/x1")
            ),
            OWNER_IVS,
        ),
        // A resource type that is not canonically cased still reaches the endpoint that
        // owns it; a string that merely has a sixth `:`-field — another service's ARN —
        // names no IVS resource and falls to the endpoint whose prefix is the signing
        // name; and a percent-encoded `tags` segment is a path NEITHER model binds (the
        // framework matches a path segment by exact equality), so it is `UnknownService`
        // rather than an ambiguity.
        (
            "tags: a stage ARN that is not canonically cased".to_string(),
            "GET",
            "localhost:9297",
            format!(
                "/tags/{}",
                label("arn:aws:ivs:us-east-1:000000000000:Stage/stage-1")
            ),
            OWNER_REALTIME,
        ),
        (
            "tags: another service's ARN whose sixth field looks like a resource type"
                .to_string(),
            "GET",
            "localhost:9297",
            format!(
                "/tags/{}",
                label("arn:aws:ivschat:us-east-1:000000000000:stage/stage-1")
            ),
            OWNER_IVS,
        ),
        (
            "tags: the `tags` path segment itself percent-encoded".to_string(),
            "GET",
            "localhost:9297",
            format!(
                "/%74ags/{}",
                label("arn:aws:ivs:us-east-1:000000000000:stage/stage-1")
            ),
            OWNER_NEITHER,
        ),
        (
            "tags: not an ARN at all".to_string(),
            "GET",
            "localhost:9297",
            "/tags/not-an-arn".to_string(),
            OWNER_IVS,
        ),
        (
            "tags: no resource at all".to_string(),
            "GET",
            "localhost:9297",
            "/tags".to_string(),
            OWNER_IVS,
        ),
        // A path neither model binds belongs to neither service, which is the router's
        // `UnknownService` and NOT an ambiguity.
        (
            "a path neither model binds".to_string(),
            "POST",
            "localhost:9297",
            "/NoSuchOperation".to_string(),
            OWNER_NEITHER,
        ),
    ];

    for (case, method, host, uri, expected) in rows {
        let actual = claimant(&ivs, &realtime, &parts(method, host, &uri));
        assert_eq!(
            actual, expected,
            "case {case:?} ({method} {uri} to host {host}): expected it to be claimed by \
             {expected}, got {actual}"
        );
    }
}

#[tokio::test]
async fn every_ivs_signed_request_is_dispatched_instead_of_refused_as_ambiguous() {
    let router = router_with_both_services();

    // (case, method, uri, body, expected status, expected error code)
    //
    // Each row names the answer of the ONE service that owns it, which is what excludes
    // both routing failures: `AmbiguousService` (500, two claimants) and `UnknownService`
    // (404, none). Each endpoint's implemented operations answer their own modelled
    // success or error; an operation that is still scaffolded answers the scaffold's
    // `501 NotImplemented`, which is also an answer from the service that owns the path.
    //
    // The RealTime endpoint's own answers (its implemented stage operations, and the 501
    // of the ones still scaffolded) are asserted end-to-end against the same router in
    // that service's own suite,
    // `services/service-com-amazonaws-ivsrealtime-amazoninteractivevideoservicerealtime/tests/all/stages.rs`;
    // which of the two services CLAIMS each `ivs`-signed path, including every RealTime
    // one, is the table in `exactly_one_service_sharing_the_ivs_signing_name_claims_each_request`
    // above.
    let rows: [(&str, &str, &str, Value, u16, &str); 7] = [
        (
            "ivs CreateChannel",
            "POST",
            "/CreateChannel",
            json!({"name": "shared-signing-name"}),
            200,
            "",
        ),
        (
            "ivs ListChannels",
            "POST",
            "/ListChannels",
            json!({}),
            200,
            "",
        ),
        (
            "ivs GetChannel on a channel that does not exist",
            "POST",
            "/GetChannel",
            json!({"arn": "arn:aws:ivs:us-east-1:000000000000:channel/doesnotexist"}),
            404,
            "ResourceNotFoundException",
        ),
        (
            "ivs CreateStreamKey for a channel that does not exist",
            "POST",
            "/CreateStreamKey",
            json!({"channelArn": "arn:aws:ivs:us-east-1:000000000000:channel/doesnotexist"}),
            404,
            "ResourceNotFoundException",
        ),
        (
            "ivs CreateChannel with a name outside the modelled ChannelName pattern",
            "POST",
            "/CreateChannel",
            json!({"name": "not a valid name!"}),
            400,
            "ValidationException",
        ),
        (
            "ivs CreateChannel with multitrack input enabled on a TS container",
            "POST",
            "/CreateChannel",
            json!({
                "name": "multitrack",
                "type": "STANDARD",
                "containerFormat": "TS",
                "multitrackInputConfiguration": {
                    "enabled": true,
                    "policy": "REQUIRE",
                    "maximumResolution": "FULL_HD"
                }
            }),
            400,
            "ValidationException",
        ),
        (
            "ivs ImportPlaybackKeyPair with material that is not a public key",
            "POST",
            "/ImportPlaybackKeyPair",
            json!({"publicKeyMaterial": "-----BEGIN PUBLIC KEY-----\n-----END PUBLIC KEY-----"}),
            400,
            "ValidationException",
        ),
    ];

    for (case, method, uri, body, expected_status, expected_code) in rows {
        let (status, code, parsed) = call(&router, method, uri, body).await;
        assert_eq!(
            (status, code.as_str()),
            (expected_status, expected_code),
            "case {case:?}: expected HTTP {expected_status} {expected_code:?} from the one \
             service that owns {uri}, got {status} {code:?} ({parsed})"
        );
    }
}

#[tokio::test]
async fn tagging_requests_on_the_shared_path_reach_a_service() {
    let router = router_with_both_services();

    // One ARN per endpoint, plus one ARN neither model knows. Every tagging operation of
    // both services is still scaffolded, so the ANSWER cannot say which one served the
    // request — what this row asserts is that a service did: neither of the router's own
    // refusals may appear.
    let arns = [
        (
            "a channel, owned by the ivs endpoint",
            "arn:aws:ivs:us-east-1:000000000000:channel/abcdef123456",
        ),
        (
            "a stage, owned by the ivsrealtime endpoint",
            "arn:aws:ivs:us-east-1:000000000000:stage/stage-1",
        ),
        (
            "an ARN neither model knows",
            "arn:aws:ivs:us-east-1:000000000000:unknown-thing/x1",
        ),
    ];

    for (case, arn) in arns {
        let uri = format!("/tags/{}", label(arn));
        let (status, code, parsed) = call(&router, "GET", &uri, Value::Null).await;
        assert!(
            code != "AmbiguousService" && code != "UnknownService",
            "case {case:?}: a tagging request must be served by exactly one of the two \
             services sharing the `ivs` signing name, got HTTP {status} {code:?} ({parsed})"
        );
    }
}

#[tokio::test]
async fn a_channel_created_through_the_shared_endpoint_reads_back() {
    let router = router_with_both_services();

    let (status, code, created) = call(
        &router,
        "POST",
        "/CreateChannel",
        json!({"name": "round-trip", "type": "ADVANCED_HD"}),
    )
    .await;
    assert_eq!(
        (status, code.as_str()),
        (200, ""),
        "case \"CreateChannel\": expected HTTP 200, got {status} {code:?} ({created})"
    );
    let arn = created["channel"]["arn"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    assert!(
        arn.starts_with("arn:aws:ivs:us-east-1:000000000000:channel/"),
        "case \"CreateChannel\": expected a modelled ChannelArn, got {arn:?}"
    );

    let (status, code, fetched) =
        call(&router, "POST", "/GetChannel", json!({"arn": arn.clone()})).await;
    assert_eq!(
        (status, code.as_str()),
        (200, ""),
        "case \"GetChannel after CreateChannel\": expected HTTP 200, got {status} {code:?} \
         ({fetched})"
    );
    assert_eq!(
        fetched["channel"], created["channel"],
        "case \"GetChannel after CreateChannel\": the channel read back must be the channel \
         created, got {} for {}",
        fetched["channel"], created["channel"]
    );
}
