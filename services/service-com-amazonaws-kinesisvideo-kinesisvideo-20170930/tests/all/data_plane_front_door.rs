// SPDX-License-Identifier: CC0-1.0

//! Which plane an announced data address BELONGS to, as the reading that scopes a
//! plane's routes.
//!
//! `GetDataEndpoint` publishes one address per plane of a stream's media, and a route of
//! that plane is served at the address announced for it and nowhere else ("Caller used
//! wrong endpoint to write data to a stream"). `kvs::state::published_plane` is the one
//! reading of that: it answers which plane of this store an authority is the published
//! address of, and nothing for an address that is not a plane's.
//!
//! The rule under test: an authority the control plane announced for a plane resolves to
//! THAT plane, and every other authority — the control-plane endpoint the call itself
//! arrived on, a port nothing was announced at, an authority of another shape — resolves
//! to no plane at all.
//!
//! The rows differ in the API the address was announced for (so in the plane), and in the
//! shape of the authority asked about.

use std::sync::Arc;

use service_com_amazonaws_kinesisvideo_kinesisvideo_20170930 as kvs;
use smithy_server_runtime::MultiServiceRouter;

use kvs::state::DataPlane;

/// The control-plane authority every control call in this suite is addressed to.
const CONTROL: &str = "127.0.0.1:9297";

/// One router over one engine, with the engine kept so the state readings this suite is
/// about can be made against the same store the router served.
struct Harness {
    router: MultiServiceRouter,
    engine: Arc<engine::InMemoryEngine>,
}

impl Harness {
    fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        router.set_request_authentication(false);
        router.register_service(kvs::EmulatedService::new(Arc::clone(&engine)));
        Self { router, engine }
    }

    /// POST one restJson1 call at the control-plane endpoint and answer its body.
    async fn call(&self, path: &str, body: serde_json::Value) -> serde_json::Value {
        let request = http::Request::builder()
            .method("POST")
            .uri(format!("https://{CONTROL}{path}"))
            .header("host", CONTROL)
            .header("content-type", "application/json")
            .header(
                "authorization",
                "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20261107/us-east-1/kinesisvideo/\
                 aws4_request, SignedHeaders=host, Signature=0",
            )
            .body(bytes::Bytes::from(
                serde_json::to_vec(&body).expect("the request body serializes"),
            ))
            .expect("a well-formed request");
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let answered = response.into_body();
        assert_eq!(
            status,
            200,
            "case {path:?}: expected HTTP 200, got {status} with {:?}",
            String::from_utf8_lossy(&answered)
        );
        serde_json::from_slice(&answered).unwrap_or(serde_json::Value::Null)
    }

    /// The authority the endpoint announced for `api_name` on `stream` is published at.
    async fn announced_authority(&self, stream: &str, api_name: &str) -> String {
        let answered = self
            .call(
                "/getDataEndpoint",
                serde_json::json!({ "StreamName": stream, "APIName": api_name }),
            )
            .await;
        let endpoint = answered["DataEndpoint"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        let scheme = format!("{}://", kvs::state::DATA_PLANE_SCHEME);
        endpoint
            .strip_prefix(&scheme)
            .unwrap_or_else(|| {
                panic!("case {api_name:?}: expected a {scheme} origin, got {endpoint:?}")
            })
            .to_string()
    }
}

/// RULE: an announced authority resolves to the plane it was announced for, and an
/// authority that is not a plane's resolves to none.
#[tokio::test]
async fn an_announced_authority_resolves_to_the_plane_it_was_published_for() {
    let harness = Harness::new();
    harness
        .call(
            "/createStream",
            serde_json::json!({ "StreamName": "scoped", "DataRetentionInHours": 1 }),
        )
        .await;

    // (the API the address was announced for, the media endpoint that serves it)
    let rows: &[(&str, DataPlane)] = &[
        ("PUT_MEDIA", DataPlane::Media),
        ("GET_MEDIA", DataPlane::Media),
        ("LIST_FRAGMENTS", DataPlane::Archive),
        ("GET_MEDIA_FOR_FRAGMENT_LIST", DataPlane::Archive),
        ("GET_CLIP", DataPlane::Archive),
        ("GET_IMAGES", DataPlane::Archive),
    ];
    for (api_name, plane) in rows {
        let authority = harness.announced_authority("scoped", api_name).await;
        assert_eq!(
            kvs::state::published_plane(&*harness.engine, &authority),
            Some(*plane),
            "case {api_name:?}: the authority {authority:?} announced for it belongs to \
             {plane:?}"
        );
        // Reading it again does not move it: the address a client was handed stays the
        // address of that plane.
        assert_eq!(
            harness.announced_authority("scoped", api_name).await,
            authority,
            "case {api_name:?}: the announced authority must not change between reads"
        );
    }

    // Every authority that is NOT a plane's published address: the control-plane endpoint
    // the calls above arrived on, a loopback port nothing was announced at, an authority
    // with no port, a public AWS name, and one of this family's own regional hosts.
    for case in [
        CONTROL,
        "127.0.0.1:1",
        "127.0.0.1",
        "kinesisvideo.us-east-1.amazonaws.com",
        "localhost:9297",
        "",
    ] {
        assert_eq!(
            kvs::state::published_plane(&*harness.engine, case),
            None,
            "case {case:?}: an authority no plane of this store was announced at belongs \
             to no plane"
        );
    }
}
