// SPDX-License-Identifier: CC0-1.0

//! What an SDK paginator sees when it reads `ListDomains` off the rpcv2Cbor wire.
//!
//! `ListDomains` is `@readonly` and `@paginated` (`inputToken`/`outputToken` `nextToken`,
//! `pageSize` `maxResults`, `items` `items`), and its output's `items` member is
//! `@required`, so the rule is: the listing always answers HTTP 200 carrying an `items`
//! member — PRESENT and empty for an account holding no domain — with `nextToken` only
//! when a further page remains, and a request it cannot serve is refused with one of the
//! four errors the operation models (403 / 400 / 429 / 500), never with a status or code
//! outside that set.
//!
//! `tests/all/domain_listing.rs` asserts that rule through the handler; this suite asserts
//! it on the BYTES, because a paginator reads the member out of the response body: a
//! `@required` empty list dropped from the body, or a status outside the modelled set, is
//! invisible to a handler-level call and breaks the SDK's paging loop.
//!
//! Rows span what the paginator can vary: the account's domain count (none, one), each
//! published bound of `maxResults`'s `@range(1..100)` plus its documented default
//! (absent), and the forms a `nextToken` this listing did not mint can take.

use aws_smithy_schema::codec::{Codec, FinishSerializer};
use aws_smithy_schema::serde::{SerializableStruct, ShapeSerializer};
use aws_smithy_schema::Schema;
use aws_smithy_schema_server::{CborCodec, DeserializableStruct};
use smithy_server_runtime::MultiServiceRouter;
use std::sync::Arc;

use service_com_amazonaws_cloudwatchomni_cloudwatchomnifrontend as omni;

use omni::ops::types::{
    CreateDomainInput, CreateDomainOutput, ListDomainsInput, ListDomainsOutput,
    CREATE_DOMAIN_INPUT_SCHEMA, LIST_DOMAINS_INPUT_SCHEMA,
};

/// The envelope reader the suites that read a refusal off the wire share.
use super::validation_message_wording::error_envelope;

/// The error code and status the model declares for a request this listing cannot serve.
const VALIDATION_EXCEPTION: &str = "ValidationException";
const VALIDATION_STATUS: u16 = 400;

/// The region every call below is signed for, which the handlers record on the resources
/// they create — so the listing is expected to report it back.
const REGION: &str = "us-west-2";

/// The bounds of `maxResults`'s `@range(min: 1, max: 100)`, its documented default
/// (absent) and one value inside the range.
///
/// Source: `smithy_query(query="operation", operation="ListDomains")` — input member
/// `maxResults`, `range {min: 1, max: 100}`.
const MAX_RESULTS_ROWS: &[Option<i32>] = &[None, Some(1), Some(100), Some(37)];

/// One emulator: one shared engine behind the router every call below goes through.
struct Wire {
    router: MultiServiceRouter,
}

impl Wire {
    fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        router.set_request_authentication(false);
        router.register_service(omni::EmulatedService::new(engine));
        Self { router }
    }

    /// Dispatch one rpcv2Cbor request and return its status and body bytes.
    async fn call<I: SerializableStruct>(
        &self,
        operation: &str,
        input_schema: &Schema,
        input: &I,
    ) -> (u16, bytes::Bytes) {
        let codec = CborCodec::default();
        let mut serializer = codec.create_serializer();
        serializer
            .write_struct(input_schema, input)
            .expect("the input shape serializes");
        let request = http::Request::builder()
            .method("POST")
            .uri(format!(
                "/service/CloudWatchOmniFrontend/operation/{operation}"
            ))
            .header("content-type", "application/cbor")
            .header("smithy-protocol", "rpc-v2-cbor")
            .header(
                "authorization",
                format!(
                    "AWS4-HMAC-SHA256 Credential=AKIDEMULATOR/20260101/{REGION}/cloudwatch/\
                     aws4_request, SignedHeaders=host, Signature=emulator"
                ),
            )
            .body(bytes::Bytes::from(serializer.finish()))
            .expect("a well-formed rpcv2Cbor request");
        let response = self.router.dispatch(request).await;
        (response.status().as_u16(), response.body().clone())
    }

    /// The account's domain, created over the same wire, and the `domainId` the backend
    /// minted for it.
    async fn create_domain(&self, case: &str, name: &str, providers: &[&str]) -> String {
        let (status, body) = self
            .call(
                "CreateDomain",
                CREATE_DOMAIN_INPUT_SCHEMA,
                &CreateDomainInput {
                    name: name.to_string(),
                    identity_providers: providers.iter().map(|p| (*p).to_string()).collect(),
                    client_token: Some(format!("token-{name}")),
                    ..Default::default()
                },
            )
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: CreateDomain over the wire answered {status}: {:?}",
            String::from_utf8_lossy(&body)
        );
        let codec = CborCodec::default();
        let mut deserializer = codec.create_deserializer(&body);
        CreateDomainOutput::deserialize(&mut deserializer)
            .unwrap_or_else(|error| panic!("case {case:?}: CreateDomain's output: {error}"))
            .domain
            .domain_id
    }

    /// One `ListDomains` call: the status it answered with and the body bytes it carried,
    /// which [`page`] decodes when the call succeeded.
    async fn list_domains(
        &self,
        max_results: Option<i32>,
        next_token: Option<String>,
    ) -> (u16, bytes::Bytes) {
        self.call(
            "ListDomains",
            LIST_DOMAINS_INPUT_SCHEMA,
            &ListDomainsInput {
                max_results,
                next_token,
            },
        )
        .await
    }
}

/// The members a response body carries at its top level, read as the CBOR map keys they
/// are rather than scanned as text — so a member name occurring inside a VALUE (a domain
/// name or an ARN) is not mistaken for the member itself.
///
/// A map arrives written either with its member count or indefinitely, terminated by a
/// break; both are valid CBOR and an encoder picks.
///
/// `pub(crate)`: every suite of this crate that reads which members a wire body carries
/// reads them here, so the tree holds ONE reading of a response body's top level.
pub(crate) fn top_level_members(case: &str, body: &[u8]) -> Vec<String> {
    let mut decoder = aws_smithy_cbor::Decoder::new(body);
    let declared = decoder.map().unwrap_or_else(|error| {
        panic!("case {case:?}: the response body is not a CBOR map: {error}")
    });
    let mut members: Vec<String> = Vec::new();
    loop {
        match declared {
            Some(count) if members.len() as u64 >= count => break,
            Some(_) => {}
            None => {
                let datatype = decoder.datatype().unwrap_or_else(|error| {
                    panic!("case {case:?}: the body ends mid-member: {error}")
                });
                if matches!(datatype, ::aws_smithy_cbor::data::Type::Break) {
                    break;
                }
            }
        }
        let key = decoder
            .str()
            .unwrap_or_else(|error| panic!("case {case:?}: a member key is not text: {error}"))
            .into_owned();
        decoder
            .skip()
            .unwrap_or_else(|error| panic!("case {case:?}: the value of {key:?}: {error}"));
        members.push(key);
    }
    members
}

/// Require a 200 whose body carries the `@required` `items` member, and report the page it
/// decodes to.
fn page(case: &str, status: u16, body: &bytes::Bytes) -> ListDomainsOutput {
    assert_eq!(
        status, 200,
        "case {case:?}: ListDomains must answer 200, got {status}: {:?}",
        String::from_utf8_lossy(body)
    );
    let members = top_level_members(case, body);
    assert!(
        members.iter().any(|member| member == "items"),
        "case {case:?}: the required items member is missing from the body, which carries \
         {members:?}"
    );
    let codec = CborCodec::default();
    let mut deserializer = codec.create_deserializer(body);
    ListDomainsOutput::deserialize(&mut deserializer)
        .unwrap_or_else(|error| panic!("case {case:?}: ListDomains' output: {error}"))
}

/// The ids a page reports, in the order it reports them.
fn ids(page: &ListDomainsOutput) -> Vec<&str> {
    page.items.iter().map(|item| item.domain_id.as_str()).collect()
}

#[tokio::test]
async fn an_account_holding_no_domain_is_served_an_empty_items_member() {
    let wire = Wire::new();

    // Every bound of `maxResults` is the same empty page: `items` present and empty, and
    // no `nextToken`, because nothing remains beyond it.
    for max_results in MAX_RESULTS_ROWS {
        let case = format!("no domain, maxResults={max_results:?}");
        let (status, body) = wire.list_domains(*max_results, None).await;
        let listed = page(&case, status, &body);
        assert_eq!(
            (ids(&listed), listed.next_token.clone()),
            (Vec::new(), None),
            "case {case:?}: an account with no domain must be served an empty listing, got \
             {listed:?}"
        );
    }
}

#[tokio::test]
async fn the_page_reports_the_account_s_domain_at_every_published_bound() {
    let wire = Wire::new();
    let domain_id = wire
        .create_domain("one domain", "omni-wire-listed", &["IAM", "IDC"])
        .await;

    for max_results in MAX_RESULTS_ROWS {
        let case = format!("one domain, maxResults={max_results:?}");
        let (status, body) = wire.list_domains(*max_results, None).await;
        let listed = page(&case, status, &body);
        assert_eq!(
            (ids(&listed), listed.next_token.clone()),
            (vec![domain_id.as_str()], None),
            "case {case:?}: expected the account's one domain in one page, got {listed:?}"
        );
        let summary = &listed.items[0];
        assert!(
            summary.domain_arn.as_deref().is_some_and(|arn| arn.contains(&domain_id))
                && summary.name.as_deref() == Some("omni-wire-listed")
                && summary.region.as_deref() == Some(REGION)
                && !summary.status.is_empty(),
            "case {case:?}: the summary must carry the stored domain's members, got \
             {summary:?}"
        );
    }
}

#[tokio::test]
async fn a_token_this_listing_did_not_mint_is_refused_with_a_modelled_error() {
    let wire = Wire::new();
    wire.create_domain("tokens", "omni-wire-tokens", &["IAM"])
        .await;

    // Rows span the forms a token that is not this listing's resume point takes on the
    // wire: not hex at all, hex that decodes to other text, and the longest token the
    // modelled `@length(1..2048)` admits.
    for (case, token) in [
        ("not hex", "not-a-token".to_string()),
        ("hex that decodes to other text", "6e6f70652d31".to_string()),
        ("the longest token the model admits", "a".repeat(2048)),
    ] {
        let (status, body) = wire.list_domains(None, Some(token)).await;
        assert_eq!(
            status, VALIDATION_STATUS,
            "case {case:?}: expected {VALIDATION_EXCEPTION} ({VALIDATION_STATUS}), got \
             {status}: {:?}",
            String::from_utf8_lossy(&body)
        );
        let (code, message) = error_envelope(case, &body);
        assert_eq!(
            code.as_str(),
            VALIDATION_EXCEPTION,
            "case {case:?}: expected the modelled {VALIDATION_EXCEPTION}, got {code:?}: \
             {message:?}"
        );
        assert!(
            !message.is_empty() && !message.contains("scaffolded"),
            "case {case:?}: the refusal must state a modelled message, got {message:?}"
        );
    }

    // The refused calls reported nothing and changed nothing: the listing still serves the
    // account's domain.
    let (status, body) = wire.list_domains(None, None).await;
    let listed = page("after the refusals", status, &body);
    assert_eq!(
        ids(&listed).len(),
        1,
        "the refused tokens changed the listing: {listed:?}"
    );
}
