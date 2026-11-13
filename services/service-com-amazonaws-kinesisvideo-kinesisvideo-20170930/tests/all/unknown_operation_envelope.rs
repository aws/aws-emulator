// SPDX-License-Identifier: CC0-1.0

//! What an endpoint of this family answers a request it can name NO operation for.
//!
//! The rule under test: a request whose method and URI name no operation the endpoint it
//! reached hosts is rejected by the front door, ahead of any handler and without reading
//! the request, with HTTP 400 and this family's unknown-operation envelope — the fault in
//! `Output` (its `__type` and the one sentence that class carries) beside the envelope's
//! `Version`. Nothing in that body comes from the request or from the store: no operation
//! was identified, so there is no name, path, member or value to report, and no
//! `x-amzn-errortype` names a shape of this service's.
//!
//! The rows differ in WHY no operation could be named: a method this service's model binds
//! no route under, on each of several of its own routes. The modelled `POST` on the same
//! route at the control-plane endpoint is served, so the refusals are about what is bound
//! rather than about the request. A route of this service asked for at an address it
//! announced for a DATA plane is the other refusal class — a path that endpoint routes
//! nothing for — and is asserted in `tests/all/data_plane_addresses.rs`.

use std::sync::Arc;

use bytes::Bytes;
use service_com_amazonaws_kinesisvideo_kinesisvideo_20170930 as kvs;
use smithy_server_runtime::MultiServiceRouter;

/// The authority the control calls in this suite arrive on: a client on the machine this
/// emulator runs on reaches it at a loopback address.
const CONTROL: &str = "127.0.0.1:9297";

/// The region of the credential scope every request carries.
const REGION: &str = "us-east-1";

/// The envelope the Kinesis Video Streams front door answers a request it can name no
/// operation for, written out here as the wire carries it rather than read back off the
/// code under test.
pub(crate) fn unknown_operation_envelope() -> serde_json::Value {
    serde_json::json!({
        "Output": {
            "__type": "com.amazon.kinesis.video.v20170930#UnknownOperationException",
            "Message": "Unable to determine operation name",
        },
        "Version": "1.0",
    })
}

/// A router carrying only this service.
struct Harness {
    router: MultiServiceRouter,
}

impl Harness {
    fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        router.set_request_authentication(false);
        router.register_service(kvs::EmulatedService::new(engine));
        Self { router }
    }

    /// Send one restJson1 call with `method` at `authority`.
    async fn send(
        &self,
        method: &str,
        authority: &str,
        path: &str,
        body: &serde_json::Value,
    ) -> http::Response<Bytes> {
        let request = http::Request::builder()
            .method(method)
            .uri(format!("https://{authority}{path}"))
            .header("host", authority)
            .header("content-type", "application/json")
            .header(
                "authorization",
                format!(
                    "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20261107/{REGION}/kinesisvideo/\
                     aws4_request, SignedHeaders=host, Signature=0000"
                ),
            )
            .body(Bytes::from(
                serde_json::to_vec(body).expect("the request body serializes"),
            ))
            .expect("a well-formed request");
        self.router.dispatch(request).await
    }

    /// Create `name` at the control-plane endpoint.
    async fn create_stream(&self, name: &str) {
        let response = self
            .send(
                "POST",
                CONTROL,
                "/createStream",
                &serde_json::json!({ "StreamName": name, "DataRetentionInHours": 1 }),
            )
            .await;
        assert_eq!(
            response.status().as_u16(),
            200,
            "case {name:?}: expected HTTP 200 from CreateStream, got {} {}",
            response.status(),
            text(&response)
        );
    }
}

fn text(response: &http::Response<Bytes>) -> String {
    String::from_utf8_lossy(response.body()).to_string()
}

fn json(response: &http::Response<Bytes>) -> serde_json::Value {
    serde_json::from_slice(response.body()).unwrap_or(serde_json::Value::Null)
}

/// Assert `response` is the front door's answer to a request it could name no operation
/// for.
fn assert_unroutable(case: &str, response: &http::Response<Bytes>) {
    assert_eq!(
        response.status().as_u16(),
        400,
        "case {case:?}: expected the front door's unroutable-request answer, got {} {}",
        response.status(),
        text(response)
    );
    assert_eq!(
        json(response),
        unknown_operation_envelope(),
        "case {case:?}: expected the unknown-operation envelope as the whole body, got {:?}",
        text(response)
    );
    assert!(
        response.headers().get("x-amzn-errortype").is_none(),
        "case {case:?}: no operation was identified, so none is named in the error-type \
         header, got {:?}",
        response.headers().get("x-amzn-errortype")
    );
}

/// RULE: a request on one of this service's own routes under a method its model binds no
/// operation under names no operation, and the front door answers it in the envelope
/// above.
#[tokio::test]
async fn a_method_the_model_binds_no_route_under_is_answered_by_the_front_door() {
    let harness = Harness::new();
    harness.create_stream("fronted").await;
    let body = serde_json::json!({ "StreamName": "fronted" });

    // (the method, one of this service's own modelled routes asked for under it)
    let rows: &[(&str, &str)] = &[
        ("GET", "/describeStream"),
        ("PUT", "/createStream"),
        ("DELETE", "/listStreams"),
        ("PATCH", "/listTagsForStream"),
        ("GET", "/TagResource"),
    ];
    for (method, path) in rows {
        let case = format!("{method} {path}");
        let response = harness.send(method, CONTROL, path, &body).await;
        assert_unroutable(&case, &response);
    }

    // And the modelled method on one of those routes at the same endpoint is served, so
    // the rows above are about what is bound and not about the request.
    let served = harness.send("POST", CONTROL, "/describeStream", &body).await;
    assert_eq!(
        served.status().as_u16(),
        200,
        "case \"POST /describeStream\": the modelled method on the same route is served, \
         got {} {}",
        served.status(),
        text(&served)
    );
}
