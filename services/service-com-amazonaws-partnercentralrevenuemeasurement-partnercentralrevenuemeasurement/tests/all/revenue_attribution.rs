// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the revenue-attribution slice, driven over this
//! service's own `rpcv2Cbor` wire protocol through the same `MultiServiceRouter`
//! the production emulator dispatches with.
//!
//! Two rules are under test here, each asserted over a SET of inputs rather than
//! one recorded call:
//!
//! 1. ROUTING AND ENVELOPE. `rpcv2Cbor` names its target service in the request
//!    URI, so every request addressed `POST
//!    /service/PartnerCentralRevenueMeasurement/operation/{Operation}` with a
//!    SigV4-family credential is served by this service in its own CBOR envelope
//!    — including a credential the framework's routing-key resolver reads no
//!    service out of (a SigV4A header or presign scope, which carries no Region
//!    field), which is what a SigV4A-signing client sends. A URI naming ANOTHER
//!    service's shape, and a request presenting no credential at all, are still
//!    claimed by nobody; and a claimed request is still authenticated, so with
//!    request authentication on it is refused unless its signature verifies.
//! 2. THE REVENUE-ATTRIBUTION PLANE. A create is observable by the next get and
//!    list, an update appends a revision the next read reports (with the previous
//!    one still readable), and every read or update addressed to a well-formed
//!    identifier the catalog does not hold is refused `404`
//!    `ResourceNotFoundException`.

use std::sync::Arc;

use aws_smithy_schema::codec::{Codec, FinishSerializer};
use aws_smithy_schema::serde::{SerializableStruct, ShapeSerializer};
use aws_smithy_schema::Schema;
use aws_smithy_schema_server::{CborCodec, DeserializableStruct};
use service_com_amazonaws_partnercentralrevenuemeasurement_partnercentralrevenuemeasurement as prm;
use smithy_server_runtime::MultiServiceRouter;

use crate::cbor_request::{request_body, texts, Cbor};
use prm::ops::types::{
    CreateRevenueAttributionInput, CreateRevenueAttributionOutput,
    GetRevenueAttributionAllocationInput, GetRevenueAttributionInput, GetRevenueAttributionOutput,
    ListRevenueAttributionAllocationsInput, ListRevenueAttributionAllocationsOutput,
    ListRevenueAttributionsInput, ListRevenueAttributionsOutput, Tag,
    UpdateRevenueAttributionInput, UpdateRevenueAttributionOutput,
    CREATE_REVENUE_ATTRIBUTION_INPUT_SCHEMA, GET_REVENUE_ATTRIBUTION_ALLOCATION_INPUT_SCHEMA,
    GET_REVENUE_ATTRIBUTION_INPUT_SCHEMA, LIST_REVENUE_ATTRIBUTIONS_INPUT_SCHEMA,
    LIST_REVENUE_ATTRIBUTION_ALLOCATIONS_INPUT_SCHEMA, UPDATE_REVENUE_ATTRIBUTION_INPUT_SCHEMA,
};

/// The Region the request's SigV4 credential scope names.
const REGION: &str = "us-east-1";

/// This service's SigV4 signing name, which is one of the router's route keys.
const SIGNING_NAME: &str = "partnercentral";

/// The Smithy service shape name, which is the `rpcv2Cbor` URI's service
/// segment.
const SERVICE_SHAPE: &str = "PartnerCentralRevenueMeasurement";

/// The canonical emulator account id every ARN carries.
const ACCOUNT_ID: &str = "000000000000";

/// The catalog the suite works in.
const CATALOG: &str = "Sandbox";

/// Two well-formed `RevenueAttributionIdentifier`s no catalog holds, one in each
/// of the two forms the member's `@pattern` admits
/// (`^(arn:…:catalog/[a-zA-Z]+/revenue-attribution/ra-[a-z0-9]{13}|ra-[a-z0-9]{13})$`).
fn missing_identifiers() -> Vec<String> {
    vec![
        "ra-0f3ab9c7d5e21".to_string(),
        format!(
            "arn:aws:partnercentral:{REGION}:{ACCOUNT_ID}:catalog/{CATALOG}/revenue-attribution/\
             ra-7c1d4e9b2a60f"
        ),
    ]
}

/// How a request names — or fails to name — a target service in the headers the
/// framework's routing-key resolver reads.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Routing {
    /// An `AWS4-HMAC-SHA256` credential scope naming this service's signing
    /// name: the signal the resolver reads first.
    SignedScope,
    /// A SigV4A `AWS4-ECDSA-P256-SHA256` header, whose credential scope carries
    /// no Region field, so the resolver reads no service out of it. This
    /// service's model lists `aws.auth#sigv4a` FIRST.
    SigV4a,
    /// A presigned URL carrying its SigV4A scope in `X-Amz-Credential`, which
    /// the resolver reads no service out of for the same reason.
    PresignedSigV4a,
    /// No credential at all, and a loopback host: nothing names a service and
    /// the caller presented nothing, so this service claims nothing either.
    Unattributed,
}

/// One emulator holding exactly this service, dispatched over HTTP.
struct Emulator {
    router: MultiServiceRouter,
}
/// A dispatched response: the HTTP status, the envelope headers and the raw body.
struct Answer {
    status: u16,
    content_type: String,
    smithy_protocol: String,
    body: Vec<u8>,
}

impl Answer {
    /// The decoded output of a call that must have succeeded.
    fn ok<T: DeserializableStruct>(&self, case: &str) -> T {
        assert_eq!(
            self.status, 200,
            "case {case:?}: expected HTTP 200, got {} with body {}",
            self.status,
            self.render()
        );
        self.decodable_cbor(case);
        let codec = CborCodec::default();
        let mut deserializer = codec.create_deserializer(&self.body);
        T::deserialize(&mut deserializer)
            .unwrap_or_else(|error| panic!("case {case:?}: undecodable response body: {error}"))
    }

    /// Assert a modelled failure: the status the model gives the error, the error
    /// code the `rpcv2Cbor` envelope carries in `__type`, and the message.
    fn is_error(&self, case: &str, status: u16, code: &str, message: &str) {
        assert_eq!(
            self.status, status,
            "case {case:?}: expected HTTP {status} {code}, got {} with body {}",
            self.status,
            self.render()
        );
        assert!(
            self.mentions(code),
            "case {case:?}: expected the error code {code:?} in the body, got {}",
            self.render()
        );
        assert!(
            self.mentions(message),
            "case {case:?}: expected the message {message:?} in the body, got {}",
            self.render()
        );
        self.decodable_cbor(case);
    }

    /// Assert the `rpcv2Cbor` envelope: the two response headers the protocol
    /// requires, and a body that is a CBOR map rather than another encoding.
    ///
    /// The first byte of a CBOR map is major type 5 — `0xa0..=0xbb` for a
    /// definite length, `0xbf` for an indefinite one (RFC 8949 section 3). A
    /// JSON body starts `{` (`0x7b`), which a CBOR reader takes for a text
    /// string whose 8-byte length follows, and an XML body starts `<` (`0x3c`).
    fn decodable_cbor(&self, case: &str) {
        assert_eq!(
            (self.content_type.as_str(), self.smithy_protocol.as_str()),
            ("application/cbor", "rpc-v2-cbor"),
            "case {case:?}: expected the rpcv2Cbor envelope headers, got {:?} and {:?} with body {}",
            self.content_type,
            self.smithy_protocol,
            self.render()
        );
        let first = self.body.first().copied();
        assert!(
            first.is_some_and(|byte| (0xa0..=0xbb).contains(&byte) || byte == 0xbf),
            "case {case:?}: expected a CBOR map body, got first byte {first:?} and body {}",
            self.render()
        );
    }

    /// Whether the body carries `needle` as a text string. CBOR writes a
    /// definite-length text string as its UTF-8 bytes after a header byte, so a
    /// byte-window search finds any string member's value.
    fn mentions(&self, needle: &str) -> bool {
        self.body
            .windows(needle.len())
            .any(|window| window == needle.as_bytes())
    }

    /// The body as lossy text, for an assertion message.
    fn render(&self) -> String {
        String::from_utf8_lossy(&self.body).escape_debug().to_string()
    }
}

impl Emulator {
    fn new() -> Self {
        Self::with_authentication(false)
    }

    /// One emulator with request authentication on or off. The SINGLE shared
    /// engine, exactly as `router/src/lib.rs` builds it.
    fn with_authentication(enabled: bool) -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        router.set_request_authentication(enabled);
        router.register_service(prm::EmulatedService::new(engine));
        Self { router }
    }

    async fn call<I: SerializableStruct>(
        &self,
        operation: &str,
        schema: &Schema,
        input: &I,
    ) -> Answer {
        let codec = CborCodec::default();
        let mut serializer = codec.create_serializer();
        serializer
            .write_struct(schema, input)
            .expect("a serializable request body");
        self.dispatch(SERVICE_SHAPE, operation, serializer.finish(), Routing::SignedScope)
            .await
    }

    async fn call_raw(&self, operation: &str, entries: Vec<(&str, Cbor)>) -> Answer {
        self.dispatch(
            SERVICE_SHAPE,
            operation,
            request_body(entries),
            Routing::SignedScope,
        )
        .await
    }

    /// One request, addressed to `service`'s `rpcv2Cbor` URI and naming its
    /// target service the way `routing` says.
    async fn dispatch(
        &self,
        service: &str,
        operation: &str,
        payload: Vec<u8>,
        routing: Routing,
    ) -> Answer {
        let mut request = http::Request::builder()
            .method("POST")
            .uri(match routing {
                // A presigned request names its scope in the query string, which
                // is where the framework reads a presigned key from.
                Routing::PresignedSigV4a => format!(
                    "/service/{service}/operation/{operation}\
                     ?X-Amz-Algorithm=AWS4-ECDSA-P256-SHA256\
                     &X-Amz-Credential=AKIDEMULATOR%2F20260101%2F{SIGNING_NAME}%2Faws4_request"
                ),
                _ => format!("/service/{service}/operation/{operation}"),
            })
            .header("content-type", "application/cbor")
            .header("smithy-protocol", "rpc-v2-cbor");
        request = match routing {
            Routing::SignedScope => request.header(
                "authorization",
                format!(
                    "AWS4-HMAC-SHA256 Credential=AKIDEMULATOR/20260101/{REGION}/{SIGNING_NAME}/\
                     aws4_request, SignedHeaders=host;x-amz-date, Signature=00000000"
                ),
            ),
            // A SigV4A credential scope is `<akid>/<date>/<service>/aws4_request`
            // — no Region field — under an algorithm the resolver does not read.
            Routing::SigV4a => request
                .header("host", "127.0.0.1:9297")
                .header(
                    "authorization",
                    format!(
                        "AWS4-ECDSA-P256-SHA256 Credential=AKIDEMULATOR/20260101/{SIGNING_NAME}/\
                         aws4_request, SignedHeaders=host;x-amz-date, Signature=00000000"
                    ),
                ),
            // A presigned request carries that same scope in the query string.
            Routing::PresignedSigV4a => request.header("host", "127.0.0.1:9297"),
            // A loopback host names no service, which is where a client pointed
            // at this emulator reaches it, and nothing else names one either.
            Routing::Unattributed => request.header("host", "127.0.0.1:9297"),
        };
        let request = request
            .body(bytes::Bytes::from(payload))
            .expect("a well-formed request");
        let response = self.router.dispatch(request).await;
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

    async fn create(&self, input: &CreateRevenueAttributionInput) -> Answer {
        self.call(
            "CreateRevenueAttribution",
            CREATE_REVENUE_ATTRIBUTION_INPUT_SCHEMA,
            input,
        )
        .await
    }

    async fn get(&self, input: &GetRevenueAttributionInput) -> Answer {
        self.call(
            "GetRevenueAttribution",
            GET_REVENUE_ATTRIBUTION_INPUT_SCHEMA,
            input,
        )
        .await
    }

    async fn update(&self, input: &UpdateRevenueAttributionInput) -> Answer {
        self.call(
            "UpdateRevenueAttribution",
            UPDATE_REVENUE_ATTRIBUTION_INPUT_SCHEMA,
            input,
        )
        .await
    }

    /// `ListRevenueAttributions`, whose `Identifiers` filter is a list of
    /// SCALARS and so is built directly — see `crate::cbor_request`.
    async fn list(&self, entries: Vec<(&str, Cbor)>) -> Answer {
        self.call_raw("ListRevenueAttributions", entries).await
    }

    /// `ListRevenueAttributions` through the generated serializer, for a request
    /// carrying a member the hand-built body cannot express (`MaxResults` is an
    /// integer).
    async fn list_typed(&self, input: &ListRevenueAttributionsInput) -> Answer {
        self.call(
            "ListRevenueAttributions",
            LIST_REVENUE_ATTRIBUTIONS_INPUT_SCHEMA,
            input,
        )
        .await
    }

    async fn list_allocations(&self, input: &ListRevenueAttributionAllocationsInput) -> Answer {
        self.call(
            "ListRevenueAttributionAllocations",
            LIST_REVENUE_ATTRIBUTION_ALLOCATIONS_INPUT_SCHEMA,
            input,
        )
        .await
    }

    async fn get_allocation(&self, input: &GetRevenueAttributionAllocationInput) -> Answer {
        self.call(
            "GetRevenueAttributionAllocation",
            GET_REVENUE_ATTRIBUTION_ALLOCATION_INPUT_SCHEMA,
            input,
        )
        .await
    }

    /// One stored attribution named `name`, and the create's own answer.
    async fn stored(&self, name: &str) -> CreateRevenueAttributionOutput {
        let answer = self
            .create(&CreateRevenueAttributionInput {
                catalog: CATALOG.to_string(),
                name: name.to_string(),
                tenancy_model: "SINGLE_TENANT".to_string(),
                ..Default::default()
            })
            .await;
        answer.ok(name)
    }
}

/// The message AWS words a miss with on an operation that ADDRESSES one
/// attribution by its own identifier.
fn not_found_message(identifier: &str) -> String {
    format!("We couldn't find revenue attribution '{identifier}'. Verify the ID and try again.")
}

/// The message AWS words a miss with on `ListRevenueAttributions`, which
/// REFERENCES the attribution in its `Identifiers` filter.
fn missing_message(identifier: &str) -> String {
    format!("Revenue attribution {identifier} doesn't exist. Verify the ID and try again.")
}

/// RULE 1, the positive half: a request whose `rpcv2Cbor` URI names this service
/// is served by it in its own envelope, whichever SigV4-family credential the
/// caller presented — including one the framework's routing-key resolver reads no
/// service out of.
///
/// One row per routing form and per operation: the answer is this service's own
/// modelled `ResourceNotFoundException`, in a decodable CBOR body, never the
/// router's awsJson `UnknownService` refusal.
#[tokio::test]
async fn every_rpcv2_cbor_request_naming_this_service_is_served_in_its_own_envelope() {
    let emulator = Emulator::new();
    let identifier = &missing_identifiers()[0];
    let get_body = |identifier: &str| {
        vec![
            ("Catalog", Cbor::text(CATALOG)),
            ("Identifier", Cbor::text(identifier)),
        ]
    };
    for routing in [
        Routing::SignedScope,
        Routing::SigV4a,
        Routing::PresignedSigV4a,
    ] {
        for operation in [
            "GetRevenueAttribution",
            "UpdateRevenueAttribution",
            "ListRevenueAttributions",
        ] {
            let case = format!("{routing:?}/{operation}");
            let entries = match operation {
                "ListRevenueAttributions" => vec![
                    ("Catalog", Cbor::text(CATALOG)),
                    ("Identifiers", texts(&[identifier.as_str()])),
                ],
                "UpdateRevenueAttribution" => {
                    let mut entries = get_body(identifier);
                    entries.push(("Revision", Cbor::text("1")));
                    entries
                }
                _ => get_body(identifier),
            };
            let answer = emulator
                .dispatch(SERVICE_SHAPE, operation, request_body(entries), routing)
                .await;
            let message = if operation == "ListRevenueAttributions" {
                missing_message(identifier)
            } else {
                not_found_message(identifier)
            };
            answer.is_error(&case, 404, "ResourceNotFoundException", &message);
        }
    }
}

/// RULE 1, the negative half: this service claims neither a URI naming ANOTHER
/// service's shape nor a request that presents no SigV4-family credential at all,
/// so each reaches nobody and the router answers its own `UnknownService`
/// refusal — the claim narrows to the requests the protocol addresses to THIS
/// service on a credential the resolver could not read.
#[tokio::test]
async fn a_uri_naming_another_service_or_a_request_with_no_credential_is_not_claimed() {
    let emulator = Emulator::new();
    let identifier = &missing_identifiers()[0];
    let rows = [
        ("PartnerCentralChannel", Routing::SigV4a),
        ("AWSPartnerDemandGenerator", Routing::SigV4a),
        // This service's own URI, presented with no credential of any kind.
        (SERVICE_SHAPE, Routing::Unattributed),
    ];
    for (service, routing) in rows {
        let case = format!("{service}/{routing:?}");
        let answer = emulator
            .dispatch(
                service,
                "GetRevenueAttribution",
                request_body(vec![
                    ("Catalog", Cbor::text(CATALOG)),
                    ("Identifier", Cbor::text(identifier)),
                ]),
                routing,
            )
            .await;
        assert_eq!(
            answer.status, 404,
            "case {case:?}: expected the router's 404 for a request nothing serves, got {} \
             with body {}",
            answer.status,
            answer.render()
        );
        assert!(
            answer.mentions("UnknownService"),
            "case {case:?}: expected the router's UnknownService refusal, got {}",
            answer.render()
        );
    }
}

/// RULE 1, the control it does not touch: claiming is not authentication. With
/// request authentication ON, a claimed request whose signature the emulator
/// cannot verify is REFUSED — in this service's own CBOR envelope rather than
/// the router's awsJson one, which is the whole of what the claim changes.
#[tokio::test]
async fn a_claimed_request_is_still_authenticated_when_authentication_is_on() {
    let emulator = Emulator::with_authentication(true);
    let identifier = &missing_identifiers()[0];
    for routing in [Routing::SignedScope, Routing::SigV4a] {
        let case = format!("{routing:?} under authentication");
        let answer = emulator
            .dispatch(
                SERVICE_SHAPE,
                "GetRevenueAttribution",
                request_body(vec![
                    ("Catalog", Cbor::text(CATALOG)),
                    ("Identifier", Cbor::text(identifier)),
                ]),
                routing,
            )
            .await;
        assert_ne!(
            answer.status, 200,
            "case {case:?}: a request the emulator cannot verify must not be served, got {} \
             with body {}",
            answer.status,
            answer.render()
        );
        assert!(
            answer.mentions("AccessDenied"),
            "case {case:?}: expected the framework's credential refusal, got {} with body {}",
            answer.status,
            answer.render()
        );
        answer.decodable_cbor(&case);
    }
}

/// RULE 2: a create is observable by the next get, in either form of the
/// identifier, and every member the get answers is read off the stored record.
#[tokio::test]
async fn a_created_attribution_reads_back_through_get() {
    let emulator = Emulator::new();
    let created: CreateRevenueAttributionOutput = emulator
        .create(&CreateRevenueAttributionInput {
            catalog: CATALOG.to_string(),
            name: "measured revenue".to_string(),
            description: Some("the first revision".to_string()),
            tenancy_model: "MULTI_TENANT".to_string(),
            product_identifier: Some("prod-abc123def4567".to_string()),
            tags: Some(vec![Tag {
                key: "team".to_string(),
                value: "measurement".to_string(),
            }]),
            ..Default::default()
        })
        .await
        .ok("create");
    assert!(
        created.id.starts_with("ra-") && created.id.len() == 16,
        "case \"create\": expected a minted ra- identifier, got {:?}",
        created.id
    );
    assert_eq!(
        created.revision.as_deref(),
        Some("1"),
        "case \"create\": expected the create to write revision 1, got {:?}",
        created.revision
    );

    // Both forms of the identifier address the same record: the short id the
    // create minted, and the ARN the create answered with.
    for identifier in [created.id.clone(), created.arn.clone()] {
        let got: GetRevenueAttributionOutput = emulator
            .get(&GetRevenueAttributionInput {
                catalog: CATALOG.to_string(),
                identifier: identifier.clone(),
                ..Default::default()
            })
            .await
            .ok(&identifier);
        assert_eq!(
            (
                got.id.as_str(),
                got.arn.as_str(),
                got.catalog.as_str(),
                got.name.as_deref(),
                got.description.as_deref(),
                got.tenancy_model.as_str(),
                got.revision.as_deref(),
                got.latest_revision.as_deref(),
            ),
            (
                created.id.as_str(),
                created.arn.as_str(),
                CATALOG,
                Some("measured revenue"),
                Some("the first revision"),
                "MULTI_TENANT",
                Some("1"),
                Some("1"),
            ),
            "case {identifier:?}: the get must answer the stored record, got {got:?}"
        );
        assert_eq!(
            got.marketplace_product
                .as_ref()
                .and_then(|product| product.product_id.as_deref()),
            Some("prod-abc123def4567"),
            "case {identifier:?}: expected the stored product id, got {:?}",
            got.marketplace_product
        );
        // No allocation exists until the allocations task writes one, so the
        // derived counts are zero and the effective window is absent.
        assert_eq!(
            (
                got.total_revenue_attribution_allocation_count,
                got.total_active_revenue_attribution_allocation_count,
                got.effective_from.as_deref(),
                got.effective_until.as_deref(),
            ),
            (Some(0), Some(0), None, None),
            "case {identifier:?}: expected the derived allocation members of an attribution with \
             no allocation, got {got:?}"
        );
    }
}

/// RULE 2: every read and update addressed to a well-formed identifier the
/// catalog does not hold is refused `404 ResourceNotFoundException`, naming the
/// identifier the request sent.
///
/// One row per operation and per form of the identifier, and a row for an
/// identifier stored in the OTHER catalog — the two catalogs are separate
/// namespaces.
#[tokio::test]
async fn an_identifier_the_catalog_does_not_hold_is_refused_not_found() {
    let emulator = Emulator::new();
    let stored = emulator.stored("in the sandbox catalog").await;
    let mut identifiers = missing_identifiers();
    identifiers.push(stored.id.clone());
    for identifier in identifiers {
        // The stored attribution lives in `Sandbox`; every row below asks the
        // `AWS` catalog, which holds nothing.
        let catalog = if identifier == stored.id {
            "AWS"
        } else {
            CATALOG
        };
        let message = not_found_message(&identifier);

        let case = format!("GetRevenueAttribution/{identifier}");
        emulator
            .get(&GetRevenueAttributionInput {
                catalog: catalog.to_string(),
                identifier: identifier.clone(),
                ..Default::default()
            })
            .await
            .is_error(&case, 404, "ResourceNotFoundException", &message);

        let case = format!("UpdateRevenueAttribution/{identifier}");
        emulator
            .update(&UpdateRevenueAttributionInput {
                catalog: catalog.to_string(),
                identifier: identifier.clone(),
                revision: "1".to_string(),
                description: Some("never written".to_string()),
                ..Default::default()
            })
            .await
            .is_error(&case, 404, "ResourceNotFoundException", &message);

        let case = format!("ListRevenueAttributionAllocations/{identifier}");
        emulator
            .list_allocations(&ListRevenueAttributionAllocationsInput {
                catalog: catalog.to_string(),
                revenue_attribution_identifier: identifier.clone(),
                ..Default::default()
            })
            .await
            .is_error(&case, 404, "ResourceNotFoundException", &message);

        let case = format!("GetRevenueAttributionAllocation/{identifier}");
        emulator
            .get_allocation(&GetRevenueAttributionAllocationInput {
                catalog: catalog.to_string(),
                revenue_attribution_identifier: identifier.clone(),
                revenue_attribution_allocation_id: "alloc-0a1b2c3d4e5f6".to_string(),
                ..Default::default()
            })
            .await
            .is_error(&case, 404, "ResourceNotFoundException", &message);

        let case = format!("ListRevenueAttributions/{identifier}");
        emulator
            .list(vec![
                ("Catalog", Cbor::text(catalog)),
                ("Identifiers", texts(&[identifier.as_str()])),
            ])
            .await
            .is_error(
                &case,
                404,
                "ResourceNotFoundException",
                &missing_message(&identifier),
            );
    }
}

/// RULE 2: an update appends a revision — the next read reports it, the previous
/// revision is still readable, and a `Revision` that is not the current one is
/// refused `409 ConflictException`.
#[tokio::test]
async fn an_update_appends_a_revision_the_next_read_reports() {
    let emulator = Emulator::new();
    let created = emulator.stored("revised").await;

    // A stale revision is the optimistic concurrency check: `1` is current, so
    // every other value conflicts.
    for stale in ["2", "7"] {
        let answer = emulator
            .update(&UpdateRevenueAttributionInput {
                catalog: CATALOG.to_string(),
                identifier: created.id.clone(),
                revision: stale.to_string(),
                description: Some("not written".to_string()),
                ..Default::default()
            })
            .await;
        answer.is_error(
            &format!("stale revision {stale}"),
            409,
            "ConflictException",
            "REVISION_MISMATCH",
        );
    }

    let updated: UpdateRevenueAttributionOutput = emulator
        .update(&UpdateRevenueAttributionInput {
            catalog: CATALOG.to_string(),
            identifier: created.id.clone(),
            revision: "1".to_string(),
            description: Some("the second revision".to_string()),
            ..Default::default()
        })
        .await
        .ok("update");
    assert_eq!(
        (
            updated.id.as_str(),
            updated.description.as_deref(),
            updated.latest_revision.as_deref(),
        ),
        (
            created.id.as_str(),
            Some("the second revision"),
            Some("2"),
        ),
        "case \"update\": expected the update to answer the revision it wrote, got {updated:?}"
    );

    // Each revision reads back as it was written, and a revision the attribution
    // never reached names nothing. The create carried no description, so revision
    // 1 holds none.
    let rows = [
        (None, Some("the second revision"), "2"),
        (Some("2"), Some("the second revision"), "2"),
        (Some("1"), None, "1"),
    ];
    for (revision, description, reported) in rows {
        let case = format!("get at revision {revision:?}");
        let got: GetRevenueAttributionOutput = emulator
            .get(&GetRevenueAttributionInput {
                catalog: CATALOG.to_string(),
                identifier: created.id.clone(),
                revision: revision.map(str::to_string),
            })
            .await
            .ok(&case);
        assert_eq!(
            (got.revision.as_deref(), got.latest_revision.as_deref()),
            (Some(reported), Some("2")),
            "case {case:?}: expected the revision read and the latest one, got {got:?}"
        );
        assert_eq!(
            got.description.as_deref(),
            description,
            "case {case:?}: expected the description of revision {reported}, got {:?}",
            got.description
        );
    }
    emulator
        .get(&GetRevenueAttributionInput {
            catalog: CATALOG.to_string(),
            identifier: created.id.clone(),
            revision: Some("3".to_string()),
        })
        .await
        .is_error(
            "get at an unreached revision",
            404,
            "ResourceNotFoundException",
            "We couldn't find revision 3 of revenue attribution",
        );
}

/// RULE 2: the list reports the attributions the store holds in this catalog,
/// filtered by `Identifiers` and paged by `MaxResults`.
#[tokio::test]
async fn the_list_reports_the_stored_attributions() {
    let emulator = Emulator::new();
    let mut ids: Vec<String> = Vec::new();
    for name in ["first", "second", "third"] {
        ids.push(emulator.stored(name).await.id);
    }

    let all: ListRevenueAttributionsOutput = emulator
        .list(vec![("Catalog", Cbor::text(CATALOG))])
        .await
        .ok("every attribution");
    assert_eq!(
        all.revenue_attribution_summaries
            .as_ref()
            .map(|summaries| summaries
                .iter()
                .filter_map(|summary| summary.id.clone())
                .collect::<Vec<String>>()),
        Some(ids.clone()),
        "case \"every attribution\": expected every stored id in creation order, got {:?}",
        all.revenue_attribution_summaries
    );

    // One row per page size: the page holds that many ids and `NextToken` names
    // the next one.
    for size in [1usize, 2, 3] {
        let case = format!("MaxResults {size}");
        let page: ListRevenueAttributionsOutput = emulator
            .list_typed(&ListRevenueAttributionsInput {
                catalog: CATALOG.to_string(),
                max_results: Some(i32::try_from(size).expect("a page size")),
                ..Default::default()
            })
            .await
            .ok(&case);
        let returned: Vec<String> = page
            .revenue_attribution_summaries
            .iter()
            .flatten()
            .filter_map(|summary| summary.id.clone())
            .collect();
        assert_eq!(
            returned,
            ids[..size],
            "case {case:?}: expected the first {size} ids, got {returned:?}"
        );
        assert_eq!(
            page.next_token.as_deref(),
            ids.get(size).map(String::as_str),
            "case {case:?}: expected the next page's first id as NextToken, got {:?}",
            page.next_token
        );
    }

    // The `Identifiers` filter keeps exactly the attributions it names.
    let filtered: ListRevenueAttributionsOutput = emulator
        .list(vec![
            ("Catalog", Cbor::text(CATALOG)),
            ("Identifiers", texts(&[ids[1].as_str()])),
        ])
        .await
        .ok("one identifier");
    assert_eq!(
        filtered
            .revenue_attribution_summaries
            .iter()
            .flatten()
            .filter_map(|summary| summary.id.clone())
            .collect::<Vec<String>>(),
        vec![ids[1].clone()],
        "case \"one identifier\": expected only the named attribution, got {:?}",
        filtered.revenue_attribution_summaries
    );

    // A catalog that holds nothing answers with the empty page, present rather
    // than absent.
    let empty: ListRevenueAttributionsOutput = emulator
        .list(vec![("Catalog", Cbor::text("AWS"))])
        .await
        .ok("the other catalog");
    assert!(
        empty
            .revenue_attribution_summaries
            .as_ref()
            .is_some_and(|summaries| summaries.is_empty()),
        "case \"the other catalog\": expected the empty page, got {:?}",
        empty.revenue_attribution_summaries
    );
}

/// RULE 2: the allocation reads answer out of the attribution's own allocation
/// store — which no operation fills yet (`StartRevenueAttributionAllocationsTask`
/// is still a scaffold stub), so the list is empty and every allocation id is
/// not found.
#[tokio::test]
async fn the_allocation_reads_report_the_attribution_s_own_allocations() {
    let emulator = Emulator::new();
    let created = emulator.stored("no allocations yet").await;

    let page: ListRevenueAttributionAllocationsOutput = emulator
        .list_allocations(&ListRevenueAttributionAllocationsInput {
            catalog: CATALOG.to_string(),
            revenue_attribution_identifier: created.id.clone(),
            ..Default::default()
        })
        .await
        .ok("list allocations");
    assert!(
        page.revenue_attribution_allocation_summaries.is_empty()
            && page.next_token.is_none(),
        "case \"list allocations\": expected the empty page an attribution with no allocation \
         holds, got {page:?}"
    );

    for allocation_id in ["alloc-0a1b2c3d4e5f6", "alloc-ffffffffffff0"] {
        emulator
            .get_allocation(&GetRevenueAttributionAllocationInput {
                catalog: CATALOG.to_string(),
                revenue_attribution_identifier: created.id.clone(),
                revenue_attribution_allocation_id: allocation_id.to_string(),
                ..Default::default()
            })
            .await
            .is_error(
                allocation_id,
                404,
                "ResourceNotFoundException",
                &format!(
                    "We couldn't find revenue attribution allocation '{allocation_id}'. Verify \
                     the ID and try again."
                ),
            );
    }
}

/// RULE 2: the create is `@idempotent` — the same `ClientToken` answers with the
/// attribution it already minted, and the same token with different parameters
/// conflicts.
#[tokio::test]
async fn a_replayed_client_token_answers_with_the_attribution_it_minted() {
    let emulator = Emulator::new();
    let request = CreateRevenueAttributionInput {
        catalog: CATALOG.to_string(),
        client_token: Some("token-0001".to_string()),
        name: "idempotent".to_string(),
        tenancy_model: "SINGLE_TENANT".to_string(),
        ..Default::default()
    };
    let first: CreateRevenueAttributionOutput = emulator.create(&request).await.ok("first");
    let replay: CreateRevenueAttributionOutput = emulator.create(&request).await.ok("replay");
    assert_eq!(
        (replay.id.as_str(), replay.revision.as_deref()),
        (first.id.as_str(), first.revision.as_deref()),
        "case \"replay\": expected the attribution the first call minted, got {replay:?}"
    );

    let changed = CreateRevenueAttributionInput {
        name: "something else".to_string(),
        ..request.clone()
    };
    emulator.create(&changed).await.is_error(
        "token reused with different parameters",
        409,
        "ConflictException",
        "CONFLICT_CLIENT_TOKEN",
    );

    // A second create with no token at all is a second attribution, not a
    // conflict: the identifier is minted by the service.
    let second: CreateRevenueAttributionOutput = emulator
        .create(&CreateRevenueAttributionInput {
            client_token: None,
            ..request
        })
        .await
        .ok("no token");
    assert_ne!(
        second.id, first.id,
        "case \"no token\": expected a second attribution, got the first one's id {:?}",
        second.id
    );
}
