// SPDX-License-Identifier: CC0-1.0

//! Wire-envelope probes for the PartnerCentral Revenue Measurement emulator.
//!
//! These assert what every request to this service gets back regardless of
//! which operation it names: the `rpcv2Cbor` envelope on a success, an honest
//! HTTP 501 `NotImplemented` for an operation nobody has written yet (never an
//! empty 200), and a 404 for an operation this service does not model.
//!
//! Every body here goes through the generated schema serializer (`CborCodec` +
//! the operation's `*_INPUT_SCHEMA`), the same path a client built from this
//! project's types takes. The probe that names an operation this service does
//! not model has no input schema of its own, so it carries a body serialized
//! from a modelled input: the request never reaches a handler, and what is
//! asserted is the routing refusal.

use std::sync::Arc;

use aws_smithy_schema::codec::{Codec, FinishSerializer};
use aws_smithy_schema::serde::{SerializableStruct, ShapeSerializer};
use aws_smithy_schema::Schema;
use aws_smithy_schema_server::CborCodec;
use service_com_amazonaws_partnercentralrevenuemeasurement_partnercentralrevenuemeasurement as prm;
use smithy_server_runtime::MultiServiceRouter;

use prm::ops::types::{
    CreateMarketplaceRevenueShareInput, GetRevenueAttributionAllocationsTaskInput,
    GetRevenueAttributionInput, ListRevenueAttributionAllocationsInput,
    ListRevenueAttributionsInput, ListTagsForResourceInput, MarketplaceRevenueShareProductEntry,
    CREATE_MARKETPLACE_REVENUE_SHARE_INPUT_SCHEMA,
    GET_REVENUE_ATTRIBUTION_ALLOCATIONS_TASK_INPUT_SCHEMA, GET_REVENUE_ATTRIBUTION_INPUT_SCHEMA,
    LIST_REVENUE_ATTRIBUTIONS_INPUT_SCHEMA, LIST_REVENUE_ATTRIBUTION_ALLOCATIONS_INPUT_SCHEMA,
    LIST_TAGS_FOR_RESOURCE_INPUT_SCHEMA,
};

/// The Region the request's SigV4 credential scope names.
const REGION: &str = "us-east-1";

/// This service's SigV4 signing name, which is also the router's route key.
const SIGNING_NAME: &str = "partnercentral";

/// The Smithy service shape name, which is the `rpcv2Cbor` URI's service
/// segment.
const SERVICE_SHAPE: &str = "PartnerCentralRevenueMeasurement";

/// The canonical emulator account id every ARN carries.
const ACCOUNT_ID: &str = "000000000000";

/// A dispatched response.
struct Answer {
    status: u16,
    content_type: String,
    smithy_protocol: String,
    body: Vec<u8>,
}

impl Answer {
    /// Whether the CBOR body carries `needle` as a text string.
    fn mentions(&self, needle: &str) -> bool {
        self.body
            .windows(needle.len())
            .any(|window| window == needle.as_bytes())
    }

    fn render(&self) -> String {
        String::from_utf8_lossy(&self.body).escape_debug().to_string()
    }
}

/// A request body as the generated serializer writes it.
fn body<I: SerializableStruct>(schema: &Schema, input: &I) -> Vec<u8> {
    let codec = CborCodec::default();
    let mut serializer = codec.create_serializer();
    serializer
        .write_struct(schema, input)
        .expect("a serializable request body");
    serializer.finish()
}

async fn call(operation: &str, payload: Vec<u8>) -> Answer {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(prm::EmulatedService::new(engine));
    let authorization = format!(
        "AWS4-HMAC-SHA256 Credential=AKIDEMULATOR/20260101/{REGION}/{SIGNING_NAME}/aws4_request, \
         SignedHeaders=host;x-amz-date, Signature=0000000000000000000000000000000000000000000000000000000000000000"
    );
    let request = http::Request::builder()
        .method("POST")
        .uri(format!("/service/{SERVICE_SHAPE}/operation/{operation}"))
        .header("authorization", authorization)
        .header("content-type", "application/cbor")
        .header("smithy-protocol", "rpc-v2-cbor")
        .body(bytes::Bytes::from(payload))
        .expect("a well-formed request");
    let response = router.dispatch(request).await;
    let header = |name: &str| {
        response
            .headers()
            .get(name)
            .and_then(|value| value.to_str().ok())
            .unwrap_or_default()
            .to_string()
    };
    Answer {
        status: response.status().as_u16(),
        content_type: header("content-type"),
        smithy_protocol: header("smithy-protocol"),
        body: response.body().to_vec(),
    }
}

/// A create body for a product this service accepts.
fn create_share_body() -> Vec<u8> {
    body(
        CREATE_MARKETPLACE_REVENUE_SHARE_INPUT_SCHEMA,
        &CreateMarketplaceRevenueShareInput {
            catalog: "AWS".to_string(),
            products: vec![MarketplaceRevenueShareProductEntry {
                product_id: "prod-abc1234567890".to_string(),
                accounts: None,
            }],
            ..Default::default()
        },
    )
}

#[tokio::test]
async fn a_success_carries_the_rpcv2_cbor_envelope() {
    let answer = call("CreateMarketplaceRevenueShare", create_share_body()).await;
    assert_eq!(
        answer.status, 200,
        "expected the create to succeed, got {} with body {}",
        answer.status,
        answer.render()
    );
    assert_eq!(
        (answer.content_type.as_str(), answer.smithy_protocol.as_str()),
        ("application/cbor", "rpc-v2-cbor"),
        "expected the rpcv2Cbor envelope headers, got {:?} and {:?}",
        answer.content_type,
        answer.smithy_protocol
    );
}

/// One row per operation this crate still carries the scaffold header for, with
/// a body that satisfies every modelled constraint — so the answer comes from
/// the handler and not from the boundary validator.
///
/// The rule: an operation nobody has written yet answers HTTP 501
/// `NotImplemented`, never an empty 200 that a caller cannot tell from a real
/// implementation.
#[tokio::test]
async fn an_unimplemented_operation_answers_501_rather_than_an_empty_success() {
    let share_arn = format!(
        "arn:aws:partnercentral:{REGION}:{ACCOUNT_ID}:catalog/AWS/marketplace-revenue-share/mrs-abc1234567890"
    );
    let attribution = "ra-abc1234567890".to_string();
    let rows: Vec<(&str, Vec<u8>)> = vec![
        (
            "ListTagsForResource",
            body(
                LIST_TAGS_FOR_RESOURCE_INPUT_SCHEMA,
                &ListTagsForResourceInput {
                    resource_arn: share_arn.clone(),
                },
            ),
        ),
        (
            "ListRevenueAttributions",
            body(
                LIST_REVENUE_ATTRIBUTIONS_INPUT_SCHEMA,
                &ListRevenueAttributionsInput {
                    catalog: "AWS".to_string(),
                    ..Default::default()
                },
            ),
        ),
        (
            "GetRevenueAttribution",
            body(
                GET_REVENUE_ATTRIBUTION_INPUT_SCHEMA,
                &GetRevenueAttributionInput {
                    catalog: "AWS".to_string(),
                    identifier: attribution.clone(),
                    ..Default::default()
                },
            ),
        ),
        (
            "GetRevenueAttributionAllocationsTask",
            body(
                GET_REVENUE_ATTRIBUTION_ALLOCATIONS_TASK_INPUT_SCHEMA,
                &GetRevenueAttributionAllocationsTaskInput {
                    catalog: "AWS".to_string(),
                    revenue_attribution_identifier: attribution.clone(),
                },
            ),
        ),
        (
            "ListRevenueAttributionAllocations",
            body(
                LIST_REVENUE_ATTRIBUTION_ALLOCATIONS_INPUT_SCHEMA,
                &ListRevenueAttributionAllocationsInput {
                    catalog: "AWS".to_string(),
                    revenue_attribution_identifier: attribution.clone(),
                    ..Default::default()
                },
            ),
        ),
    ];
    for (operation, payload) in rows {
        let answer = call(operation, payload).await;
        assert_eq!(
            answer.status, 501,
            "case {operation:?}: expected HTTP 501 NotImplemented, got {} with body {}",
            answer.status,
            answer.render()
        );
        assert!(
            answer.mentions("NotImplemented"),
            "case {operation:?}: expected the NotImplemented code in the body, got {}",
            answer.render()
        );
    }
}

#[tokio::test]
async fn an_operation_the_model_does_not_declare_is_not_found() {
    // The body is a legal `rpcv2Cbor` map; what is asserted is that routing
    // refuses the operation name before any handler sees it.
    let answer = call("DeleteMarketplaceRevenueShare", create_share_body()).await;
    assert_eq!(
        answer.status, 404,
        "expected HTTP 404 for an operation the model does not declare, got {} with body {}",
        answer.status,
        answer.render()
    );
}
