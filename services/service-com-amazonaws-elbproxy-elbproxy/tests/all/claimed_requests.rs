// SPDX-License-Identifier: CC0-1.0

//! The claim under the shared signing name, over the WHOLE modelled surface rather than a
//! few of its operations.
//!
//! The rule under test is the one `tests/all/signing_name_claim.rs` states for named rows:
//! this service signs `elasticloadbalancing`, the SigV4 name both Elastic Load Balancing
//! models carry, so it claims a request only when that key names it AND the request
//! addresses this service's own `rpcv2Cbor` surface — `/service/ELBProxy/operation/<Op>`.
//! **Every** operation the model declares is such a surface, and no `POST /` awsQuery
//! request is, whichever host it names: that is what this suite asserts, so an operation
//! added to the model later is covered without a row being written for it.
//!
//! The operation names come from the MODEL, read at test time, rather than from a
//! hand-written list: a list would say nothing about an operation nobody remembered to add
//! to it, which is exactly the case a claim rule is got wrong in.

use std::sync::Arc;

use service_com_amazonaws_elbproxy_elbproxy::EmulatedService;
use smithy_server_runtime::Service;

/// This service's Smithy model, as it ships beside this crate. Read at test time rather
/// than embedded: it is the model that generated the crate, and no non-test build needs it.
const MODEL_PATH: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/smithy-model/com-amazonaws-elbproxy-elbproxy.json"
);

/// The local name of this model's service shape, which an `rpcv2Cbor` request names in its
/// path (`smithy_query(query="service_info")` reports the service as `ELBProxy`).
const SERVICE_SHAPE: &str = "ELBProxy";

/// The signing name this service shares with both Elastic Load Balancing models.
const SHARED_SIGNING_NAME: &str = "elasticloadbalancing";

/// Whether this service claims a `POST uri` signed for [`SHARED_SIGNING_NAME`] and addressed
/// to `host`.
fn claims(service: &EmulatedService<engine::InMemoryEngine>, host: &str, uri: &str) -> bool {
    let (parts, ()) = http::Request::builder()
        .method("POST")
        .uri(uri)
        .header("host", host)
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 \
                 Credential=AKIDEXAMPLE/20261120/us-east-1/{SHARED_SIGNING_NAME}/aws4_request, \
                 SignedHeaders=host, Signature=0000"
            ),
        )
        .body(())
        .expect("build the request")
        .into_parts();
    service.can_handle(&parts)
}

/// Every operation shape the model declares, by its local name.
fn modelled_operations() -> Vec<String> {
    let model = ::std::fs::read_to_string(MODEL_PATH)
        .expect("the model that generated this crate ships beside it");
    let model: ::serde_json::Value =
        ::serde_json::from_str(&model).expect("the model that generated this crate is JSON");
    let shapes = model["shapes"]
        .as_object()
        .expect("a Smithy model declares shapes")
        .iter()
        .filter(|(_, shape)| shape["type"] == "operation")
        .filter_map(|(id, _)| id.rsplit('#').next().map(str::to_string))
        .collect::<Vec<String>>();
    assert!(
        !shapes.is_empty(),
        "case \"the modelled surface\": a service model declares operations"
    );
    shapes
}

#[test]
fn every_modelled_operation_is_claimed_on_this_services_own_protocol_path() {
    let service = EmulatedService::new(Arc::new(engine::InMemoryEngine::new()));
    for operation in modelled_operations() {
        let uri = format!("/service/{SERVICE_SHAPE}/operation/{operation}");
        assert!(
            claims(&service, "localhost:9297", &uri),
            "case {operation:?}: expected this service to claim POST {uri}, which names it \
             and one of its own operations"
        );
    }
}

#[test]
fn the_shared_keys_query_traffic_is_claimed_on_no_host() {
    // The hosts the shared key's awsQuery traffic arrives on: the emulator's own endpoint,
    // the Elastic Load Balancing endpoint both other models publish, and this service's own
    // endpoint prefix. The claim rule reads the request's own wire surface, so none of them
    // makes `POST /` this service's.
    let service = EmulatedService::new(Arc::new(engine::InMemoryEngine::new()));
    for host in [
        "localhost:9297",
        "elasticloadbalancing.us-east-1.amazonaws.com",
        "elb-proxy.us-east-1.amazonaws.com",
    ] {
        assert!(
            !claims(&service, host, "/"),
            "case {host:?}: expected this service to claim no awsQuery request on POST /, \
             which is the Elastic Load Balancing front door's surface"
        );
    }
}
