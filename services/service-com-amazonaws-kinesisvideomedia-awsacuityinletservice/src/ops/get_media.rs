// SPDX-License-Identifier: CC0-1.0

//! @op GetMedia
//!
//! Reads a contiguous run of a stream's persisted chunks and answers them as the
//! response payload, framed the way Kinesis Video Streams frames them: each stored
//! chunk is followed by the Matroska `Tags` element carrying that chunk's
//! `AWS_KINESISVIDEO_*` metadata, including the continuation token that resumes the
//! stream immediately after it.
//!
//! Nothing here is canned: every byte of the payload, every fragment number and
//! timestamp, and every continuation token come from [`crate::state`], which the
//! producer path writes through
//! [`ComAmazonawsKinesisvideomediaAwsacuityinletserviceState::put_fragment`](crate::state::ComAmazonawsKinesisvideomediaAwsacuityinletserviceState::put_fragment).
//! The stream a request addresses is the Kinesis Video Streams control plane's
//! resource, resolved in its registry through
//! [`ComAmazonawsKinesisvideomediaAwsacuityinletserviceState::resolve_stream`](crate::state::ComAmazonawsKinesisvideomediaAwsacuityinletserviceState::resolve_stream),
//! so a stream `CreateStream` made is readable here — with an empty payload until a
//! producer writes to it — and `ResourceNotFoundException` is only ever for an
//! identifier that registry does not hold.
//!
//! Before any of that, the call is AUTHORIZED: AWS decides at the IAM layer, ahead of the
//! operation, so this handler asks [`crate::service::authorization`] whether the caller's
//! identity-based policies allow `kinesisvideo:GetMedia` on the stream the request
//! addresses, and a caller they do not allow is refused `AccessDeniedException` (HTTP 403)
//! with nothing resolved and no media served.
//!
//! Where this emulator stops short of the real service, it stops honestly: a real
//! `GetMedia` holds the connection open and keeps streaming new fragments, while this
//! one ends the session at the newest fragment persisted when the call arrived and
//! hands back the token that resumes there. The throttling
//! (`ClientLimitExceededException`, `ConnectionLimitExceededException`) and wrong-endpoint
//! (`InvalidEndpointException`) behaviours, and the stream-level/expired-token refusal the
//! modelled `NotAuthorizedException` (401) carries, are not modelled here and are filed as
//! their own gap issues.
//!
//! ## Shape of an operation
//!
//! Each operation is a struct that owns the shared engine and implements
//! [`smithy_server_runtime::Operation`]. The framework constructs one per service via
//! `GetMedia::new(engine)` and drives it through the centralized request walk:
//! deserialize → validate → `handle` → serialize. You write ONLY the `handle` body.
//!
//! ## Reaching this service's state
//!
//! The engine is held on `self` (constructor-injected), NOT passed per request — so
//! engine access is guaranteed by the time any request runs. Reach the store through
//! [`crate::state::service_state`] and read and write it through the guard it hands back.
//!
//! `ctx` ([`smithy_server_runtime::OperationContext`]) carries request metadata only —
//! headers, URI, method, and anything middleware placed in `ctx.parts().extensions`.
//! The engine is NOT reached through `ctx`.
//!
//! Shared types live in `super::types`; the error channel in `crate::scaffolded`.
//!
//! ---
//!
//! # Modeled contract for `GetMedia`
//!
//! What AWS documents for this operation, from this service's Smithy model. HTML is
//! stripped and long entries are clipped at ` …`. For the FULL text of any one item,
//! ask the model rather than reading the raw JSON:
//!
//! ```text
//! smithy_query(query="doc",shape="<Shape>",member="<Member>",model_path="services/service-com-amazonaws-kinesisvideomedia-awsacuityinletservice/smithy-model/com-amazonaws-kinesisvideomedia-awsacuityinletservice.json")
//! ```
//!
//! Use this API to retrieve media content from a Kinesis video stream. In the request, you
//! identify the stream name or stream Amazon Resource Name (ARN), and the starting chunk.
//! Kinesis Video Streams then returns a stream of chunks in order by fragment number. You
//! must first call the GetDataEndpoint API to get an endpoint. Then send the GetMedia
//! requests to this endpoint using the --endpoint-url parameter. When you put media data
//! (fragments) on a stream, Kinesis Video Streams stores each incoming fragment and related
//! metadata in what is called a "chunk." For more information, see PutMedia. The GetMedia
//! API returns a stream of these chunks starting from the chunk that you specify in the
//! request. The following limits apply when using the GetMedia API: A client can call
//! GetMedia up to five times per second per stream. Kinesis Video Streams sends media data
//! at a rate of up to 25 megabytes per second (or 200 megabits per second) during a
//! GetMedia session. If an error is thrown after invoking a Kinesis Video Streams media
//! API, in addition to the HTTP status code and the response body, it includes the
//! following pieces of information: x-amz-ErrorType HTTP header – contains a more specific
//! error type in addition to what the HTTP status code provides. x-amz-RequestId HTTP
//! header – if you want to report an issue to AWS, the support team can better diagnose the
//! problem if given the Request Id. Both the HTTP status code and the ErrorType header can
//! be utilized to make programmatic decisions about whether errors are retry-able and under
//! what conditions, as well as provide information on what actions the client programmer
//! might need to take in order to successfully try again. For more information, see the
//! Errors section at the bottom of this topic, as well as Common Errors.
//!
//! ## Input
//!
//! * `StartSelector` (StartSelector) **required** — Identifies the starting chunk to get from
//!   the specified stream.
//! * `StreamARN` (ResourceARN, len 1..1024, pattern
//!   ^arn:aws:kinesisvideo:[a-z0-9-]+:[0-9]+:[a-z]+/[a-zA-Z0-9_.-]+/[0-9]+$) — The ARN of the
//!   stream from where you want to get the media content. If you don't specify the streamARN,
//!   you must specify the streamName.
//! * `StreamName` (StreamName, len 1..256, pattern ^[a-zA-Z0-9_.-]+$) — The Kinesis video
//!   stream name from where you want to get the media content. If you don't specify the
//!   streamName, you must specify the streamARN.
//!
//! ## Output
//!
//! * `ContentType` (ContentType, len 1..128, pattern ^[a-zA-Z0-9_\.\-]+$) — The content type
//!   of the requested media.
//! * `Payload` (Payload, default "") **required** — The payload Kinesis Video Streams returns
//!   is a sequence of chunks from the specified stream. For information about the chunks, see
//!   . The chunks that Kinesis Video Streams returns in the GetMedia call also include the
//!   following additional Matroska (MKV) tags: AWS_KINESISVIDEO_CONTINUATION_TOKEN (UTF-8
//!   string) - In the event your GetMedia call terminates, you can use this continuation
//!   token in your next request to get the next chunk where the last request terminated.
//!   AWS_KINESISVIDEO_MILLIS_BEHIND_NOW (UTF-8 string) - Client applications can use this tag
//!   value to determine how far behind the chunk returned in the response is from the latest
//!   chunk on the stream. …
//!
//! ## Errors
//!
//! * `ClientLimitExceededException` (HTTP 400) — Kinesis Video Streams has throttled the
//!   request because you have exceeded the limit of allowed client calls. Try making the call
//!   later.
//! * `ConnectionLimitExceededException` (HTTP 400) — Kinesis Video Streams has throttled the
//!   request because you have exceeded the limit of allowed client connections.
//! * `InvalidArgumentException` (HTTP 400) — The value for this input parameter is invalid.
//! * `InvalidEndpointException` (HTTP 400) — Status Code: 400, Caller used wrong endpoint to
//!   write data to a stream. On receiving such an exception, the user must call
//!   GetDataEndpoint with AccessMode set to "READ" and use the endpoint Kinesis Video returns
//!   in the next GetMedia call.
//! * `NotAuthorizedException` (HTTP 401) — Status Code: 403, The caller is not authorized to
//!   perform an operation on the given stream, or the token has expired.
//! * `ResourceNotFoundException` (HTTP 404) — Status Code: 404, The stream with the given
//!   name does not exist.

#[allow(unused_imports)]
use super::types::*;

// `#[operation_error]` below expands to an invocation of the LOCAL declarative
// `smithy!` macro, which this line is what emits into the module. Without it the
// expansion fails with `cannot find macro 'smithy' in this scope` and the
// `GetMediaError` enum is never produced.
smithy_macros::smithy_model!("smithy-model/com-amazonaws-kinesisvideomedia-awsacuityinletservice.json");

// The operation's MODELED errors. `#[operation_error]` accepts only variants the
// model declares for this operation, so the scaffold's `NotImplemented` cannot live
// here — hence the `GetMediaError` alias below. An operation that models no errors
// generates an UNINHABITED enum here, which is correct and expected: there is nothing
// to construct until the model says otherwise.
#[smithy_macros::operation_error("com.amazonaws.kinesisvideomedia#GetMedia")]
#[derive(Debug)]
pub enum GetMediaErrorModeled {
    ClientLimitExceededException(ClientLimitExceededException),
    ConnectionLimitExceededException(ConnectionLimitExceededException),
    InvalidArgumentException(InvalidArgumentException),
    InvalidEndpointException(InvalidEndpointException),
    NotAuthorizedException(NotAuthorizedException),
    ResourceNotFoundException(ResourceNotFoundException),
}

/// This operation's error channel, as the generated `impl Operation` names it
/// (`crate::ops::get_media::GetMediaError`). Wrapping the modeled enum is what
/// lets a scaffolded operation answer 501 for one whose model declares no errors at all.
pub type GetMediaError = crate::scaffolded::Scaffolded<GetMediaErrorModeled>;

/// Lets `?` and `.into()` lift a modeled failure into the error channel, so an
/// implemented handler writes `Err(SomeModeledError.into())` rather than naming the
/// wrapper variant. See `crate::scaffolded` for how to drop the wrapper entirely once
/// this operation is implemented.
impl From<GetMediaErrorModeled> for GetMediaError {
    fn from(modeled: GetMediaErrorModeled) -> Self {
        crate::scaffolded::Scaffolded::Modeled(modeled)
    }
}

/// `InvalidArgumentException` (HTTP 400) — "The value for this input parameter is
/// invalid": the request named no usable starting chunk or no usable stream identifier.
fn invalid_argument(message: impl Into<String>) -> GetMediaError {
    GetMediaErrorModeled::InvalidArgumentException(InvalidArgumentException {
        message: Some(message.into()),
    })
    .into()
}

/// The message a `StartSelector$ContinuationToken` this service never issued is refused
/// with: the one fixed sentence Kinesis Video Streams states when it cannot DECODE the
/// token a request presented.
///
/// A continuation token is an opaque value the service itself mints and encodes, so a token
/// it did not mint is a token it cannot decode, whatever text it carries. The whole message
/// is fixed: no part of it comes from the request or from this service's state, which is
/// what keeps a token and a stream name out of an error channel that states neither.
pub const UNDECODABLE_CONTINUATION_TOKEN_MESSAGE: &str = "Illegal embedded sign character";

/// `ResourceNotFoundException` (HTTP 404). The modelled shape documents exactly one
/// condition — "The stream with the given name does not exist" — so it is answered for
/// an unresolvable stream identifier and for nothing else.
fn resource_not_found(message: impl Into<String>) -> GetMediaError {
    GetMediaErrorModeled::ResourceNotFoundException(ResourceNotFoundException {
        message: Some(message.into()),
    })
    .into()
}

// ---------------------------------------------------------------------------
// Chunk framing: the Matroska `Tags` element Kinesis Video Streams adds to each
// chunk it returns.
// ---------------------------------------------------------------------------
//
// `GetMediaOutput$Payload` is documented as "a sequence of chunks from the specified
// stream" whose chunks "also include the following additional Matroska (MKV) tags:
// AWS_KINESISVIDEO_CONTINUATION_TOKEN, AWS_KINESISVIDEO_MILLIS_BEHIND_NOW,
// AWS_KINESISVIDEO_FRAGMENT_NUMBER, AWS_KINESISVIDEO_SERVER_TIMESTAMP,
// AWS_KINESISVIDEO_PRODUCER_TIMESTAMP". That is the only place the continuation token
// reaches the client, so the framing is part of this operation's contract.
//
// The producer's media bytes are passed through untouched — this operation neither
// parses nor re-muxes them — and the only thing written here is one `Tags` master
// element per emitted chunk, built from five fixed Matroska elements whose ids the
// Matroska specification defines and whose sizes are EBML variable-size integers
// (RFC 8794 section 4). A Matroska reader/muxer crate would be the right tool for
// parsing or rewriting media; emitting this fixed five-element metadata block is the
// whole of the format work here, so it is written against the spec directly. This
// module therefore carries a WRITER only: no code path in this crate parses EBML.

/// Matroska `Tags` (level-1 master element). Element ids are written verbatim: an id
/// is itself already an EBML variable-size integer.
pub const ID_TAGS: &[u8] = &[0x12, 0x54, 0xC3, 0x67];
/// Matroska `Tag` (a group of `SimpleTag`s inside `Tags`).
pub const ID_TAG: &[u8] = &[0x73, 0x73];
/// Matroska `SimpleTag` (one name/value pair).
pub const ID_SIMPLE_TAG: &[u8] = &[0x67, 0xC8];
/// Matroska `TagName` (the pair's name, UTF-8).
pub const ID_TAG_NAME: &[u8] = &[0x45, 0xA3];
/// Matroska `TagString` (the pair's value, UTF-8).
pub const ID_TAG_STRING: &[u8] = &[0x44, 0x87];

/// How much media one `GetMedia` session carries before it ends and hands back the
/// continuation token that resumes there.
///
/// A real session streams continuously, at a rate this operation's documentation
/// states as "up to 25 megabytes per second"; this emulator answers one buffered
/// response, so it bounds a session at one second of that documented (decimal) rate
/// rather than at the size of the whole stream. A session always carries at least one
/// chunk, so a stream of larger fragments still makes progress.
const MAX_SESSION_BYTES: usize = 25_000_000;

/// Room allowed per chunk for its `Tags` element: five `SimpleTag`s of a tag name and
/// a short decimal value. Counted against [`MAX_SESSION_BYTES`] alongside the media
/// bytes, so a run of tiny chunks cannot carry an unbounded amount of framing past the
/// session bound, and reused as the response buffer's allocation hint. The element
/// itself is always written in full, whatever its size.
const TAGS_ELEMENT_BUDGET: usize = 256;

/// Append the EBML variable-size integer (RFC 8794 section 4.4) encoding `size`,
/// in the shortest width that can hold it.
fn push_ebml_size(out: &mut Vec<u8>, size: u64) {
    for width in 1..=8usize {
        let bits = 7 * width;
        // VINT_DATA of all ones is reserved for "unknown size", so the largest
        // representable length at this width is two less than the power of two.
        let max = (1u64 << bits) - 2;
        if size <= max {
            // The marker bit sits immediately above the VINT_DATA bits, which puts
            // the whole value in exactly `width` big-endian bytes.
            let encoded = (size | (1u64 << bits)).to_be_bytes();
            out.extend_from_slice(&encoded[8 - width..]);
            return;
        }
    }
    // Unreachable: `size` is a slice length, and no slice here approaches 2^56 - 2
    // bytes. Fail loudly in a test build rather than emit a differently-shaped
    // element; 0xFF, the "unknown size" VINT, is the honest encoding of a length this
    // writer cannot state.
    debug_assert!(false, "element body of {size} bytes exceeds the largest EBML size");
    out.push(0xFF);
}

/// Append one EBML element: its id, its size, then `body`.
///
/// This writer is the whole of this crate's EBML surface: the chunk framing a
/// `GetMedia` payload carries. There is no reader here — this operation never parses
/// media or metadata, it only emits it (`tests/all/get_media.rs` carries the inverse
/// reader that checks what this writes).
pub fn push_ebml_element(out: &mut Vec<u8>, id: &[u8], body: &[u8]) {
    out.extend_from_slice(id);
    push_ebml_size(out, body.len() as u64);
    out.extend_from_slice(body);
}

/// The `Tags` element Kinesis Video Streams appends to one returned chunk, carrying
/// the five `AWS_KINESISVIDEO_*` values `GetMediaOutput$Payload` documents. Both
/// timestamps are epoch milliseconds, the unit the service reports them in.
fn kvs_tags_element(
    fragment_number: &str,
    server_timestamp_millis: i64,
    producer_timestamp_millis: i64,
    continuation_token: &str,
    millis_behind_now: i64,
) -> Vec<u8> {
    let mut tag_body = Vec::new();
    for (name, value) in [
        (
            "AWS_KINESISVIDEO_FRAGMENT_NUMBER",
            fragment_number.to_string(),
        ),
        (
            "AWS_KINESISVIDEO_SERVER_TIMESTAMP",
            server_timestamp_millis.to_string(),
        ),
        (
            "AWS_KINESISVIDEO_PRODUCER_TIMESTAMP",
            producer_timestamp_millis.to_string(),
        ),
        (
            "AWS_KINESISVIDEO_CONTINUATION_TOKEN",
            continuation_token.to_string(),
        ),
        (
            "AWS_KINESISVIDEO_MILLIS_BEHIND_NOW",
            millis_behind_now.to_string(),
        ),
    ] {
        let mut simple_tag = Vec::new();
        push_ebml_element(&mut simple_tag, ID_TAG_NAME, name.as_bytes());
        push_ebml_element(&mut simple_tag, ID_TAG_STRING, value.as_bytes());
        push_ebml_element(&mut tag_body, ID_SIMPLE_TAG, &simple_tag);
    }
    let mut tags_body = Vec::new();
    push_ebml_element(&mut tags_body, ID_TAG, &tag_body);
    let mut element = Vec::new();
    push_ebml_element(&mut element, ID_TAGS, &tags_body);
    element
}

/// The `GetMedia` operation.
///
/// Owns the shared engine (the constructor seam) and implements
/// [`smithy_server_runtime::Operation`]. Generic over the engine type `Eng`, so this
/// crate names no concrete engine type: a migrated service that needs state picks a
/// concrete engine where it mounts the service, not here.
pub struct GetMedia<Eng> {
    engine: ::std::sync::Arc<Eng>,
}

impl<Eng> GetMedia<Eng> {
    /// Construct the operation from the shared engine. INFALLIBLE — the engine is a
    /// construction-time dependency, so engine access is guaranteed by the time any
    /// request runs. Called by the generated `EmulatedService::new(engine)` constructor.
    pub fn new(engine: ::std::sync::Arc<Eng>) -> Self {
        Self { engine }
    }
}

impl<Eng: ::std::marker::Send + ::std::marker::Sync + 'static> smithy_server_runtime::Operation
    for GetMedia<Eng>
{
    type Input = GetMediaInput;
    type Output = GetMediaOutput;
    type Error = GetMediaError;

    const NAME: &'static str = "GetMedia";
    const INPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::GET_MEDIA_INPUT_SCHEMA;
    const OUTPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::GET_MEDIA_OUTPUT_SCHEMA;
    const SUCCESS_STATUS: u16 = 200u16;

    async fn handle(
        &self,
        input: Self::Input,
        ctx: &smithy_server_runtime::OperationContext<'_>,
    ) -> Result<Self::Output, Self::Error> {
        let Some(state) = crate::state::service_state(&*self.engine) else {
            // Unreachable in this emulator: the router builds one `InMemoryEngine` and
            // hands it to every service. There is no modelled server fault to answer
            // with, and inventing a client error would blame the caller for an
            // emulator that was assembled without this service's store.
            return Err(crate::scaffolded::Scaffolded::not_implemented("GetMedia"));
        };

        // The stream is identified by EXACTLY one of the two identifiers: "If you
        // don't specify the streamName, you must specify the streamARN", and the two
        // name one resource, so a request carrying both says nothing about which to
        // believe.
        let name = input.stream_name.as_deref();
        let arn = input.stream_arn.as_deref();
        match (name, arn) {
            (Some(_), None) | (None, Some(_)) => {}
            (Some(_), Some(_)) => {
                return Err(invalid_argument(
                    "Only one of StreamName and StreamARN may be specified.",
                ))
            }
            (None, None) => {
                return Err(invalid_argument(
                    "Either StreamName or StreamARN must be specified.",
                ))
            }
        }

        // IAM decides next, on this operation's own action and the stream the request
        // addresses: AWS evaluates authorization at the IAM layer ahead of the operation, so
        // a caller whose identity-based policies do not allow the call is refused with the
        // code and status AWS refuses an unauthorized call with — `AccessDeniedException`
        // (403), which this service's model declares no shape for (see `crate::scaffolded`)
        // — and nothing about the stream is resolved or served. Asked before this service's
        // store is locked, so only one store is ever held at a time.
        //
        // It follows the identifier check above only because the decision names the
        // resource the request addresses, and a request naming no single stream names none:
        // that check reads the request alone, exactly as the boundary validator that ran
        // before this handler does.
        let resource = crate::service::authorization::AddressedStream::of(
            &*self.engine,
            name,
            arn,
        );
        if let Some(refusal) = crate::service::authorization::authorization_refusal(
            &*self.engine,
            ctx,
            Self::NAME,
            &resource,
        ) {
            return Err(GetMediaError::access_denied(refusal));
        }

        // Each selector type carries its own companion member, per
        // `StartSelector$StartSelectorType`: FRAGMENT_NUMBER needs
        // `AfterFragmentNumber`, PRODUCER_TIMESTAMP / SERVER_TIMESTAMP need
        // `StartTimestamp`, CONTINUATION_TOKEN needs `ContinuationToken`, and NOW /
        // EARLIEST take nothing further.
        let selector = &input.start_selector;
        let selector_type = selector.start_selector_type.as_str();
        match selector_type {
            "FRAGMENT_NUMBER" if selector.after_fragment_number.is_none() => {
                return Err(invalid_argument(
                    "StartSelector.AfterFragmentNumber is required when StartSelectorType is FRAGMENT_NUMBER.",
                ))
            }
            "PRODUCER_TIMESTAMP" | "SERVER_TIMESTAMP" if selector.start_timestamp.is_none() => {
                return Err(invalid_argument(format!(
                    "StartSelector.StartTimestamp is required when StartSelectorType is {selector_type}."
                )))
            }
            "CONTINUATION_TOKEN" if selector.continuation_token.is_none() => {
                return Err(invalid_argument(
                    "StartSelector.ContinuationToken is required when StartSelectorType is CONTINUATION_TOKEN.",
                ))
            }
            "EARLIEST" | "NOW" | "FRAGMENT_NUMBER" | "PRODUCER_TIMESTAMP" | "SERVER_TIMESTAMP"
            | "CONTINUATION_TOKEN" => {}
            other => {
                // Defensive: the framework's schema validation refuses a value outside
                // the modelled `StartSelectorType` enum with a 400 before `handle`
                // runs (pinned by the refusal table in `tests/all/get_media.rs`), so
                // this arm answers a type no request can actually carry.
                return Err(invalid_argument(format!(
                    "StartSelector.StartSelectorType must be one of [{}], got {other}.",
                    START_SELECTOR_TYPE_VALUES.join(", ")
                )))
            }
        }

        // The caller's `StartTimestamp` in epoch milliseconds — computed BEFORE any
        // state is locked, and refused when it does not fit. The member carries no
        // modelled range, so a client may send any JSON number, and unchecked
        // arithmetic on it would either panic on the request path (inside the state
        // guard, had it been taken) or silently wrap into a different cursor.
        let start_timestamp_millis = match selector.start_timestamp.as_ref() {
            Some(timestamp) => Some(
                timestamp
                    .secs()
                    .checked_mul(1000)
                    .and_then(|millis| {
                        millis.checked_add(i64::from(timestamp.subsec_nanos() / 1_000_000))
                    })
                    .ok_or_else(|| {
                        invalid_argument("StartSelector.StartTimestamp is out of range.")
                    })?,
            ),
            None => None,
        };

        let mut store = state.lock();

        // Resolve the stream, the cursor the selector names, and how much of the
        // stream this session carries.
        let (
            stream_name,
            stream_arn,
            stream_id,
            content_type,
            newest_server_millis,
            start,
            chunks,
            bytes,
        ) = {
            // The stream is the Kinesis Video Streams control plane's resource, so it
            // is resolved in that registry in the shared engine; this store holds only
            // the media produced to it, which is empty for a stream just created.
            let stream = store
                .resolve_stream(&*self.engine, name, arn)
                .ok_or_else(|| {
                    // The one sentence this family states for a stream that is not there
                    // to serve, held by the archive read plane and stated by the control
                    // plane too: fixed text carrying nothing from the request.
                    resource_not_found(
                        ::service_com_amazonaws_kinesisvideoarchivedmedia_awsacuityreader::state::STREAM_NOT_FOUND_MESSAGE
                            .to_string(),
                    )
                })?;
            let fragments = stream.fragments;
            let start = match selector_type {
                // "EARLIEST - Start with earliest available chunk on the stream."
                "EARLIEST" => 0,
                // "NOW - Start with the latest chunk on the stream." An empty stream
                // has no chunk at all, so the cursor sits at its end.
                "NOW" => fragments.len().saturating_sub(1),
                // "FRAGMENT_NUMBER - Start with the chunk AFTER a specific fragment."
                "FRAGMENT_NUMBER" => {
                    let after = selector.after_fragment_number.as_deref().unwrap_or_default();
                    fragments
                        .iter()
                        .position(|fragment| fragment.fragment_number == after)
                        .map(|index| index + 1)
                        .ok_or_else(|| {
                            // The stream exists, so this is an invalid argument and
                            // not the one condition `ResourceNotFoundException`
                            // documents ("The stream with the given name does not
                            // exist").
                            invalid_argument(format!(
                                "Fragment number {after} is not on stream {}.",
                                stream.name
                            ))
                        })?
                }
                // "PRODUCER_TIMESTAMP or SERVER_TIMESTAMP - Start with the chunk
                // CONTAINING a fragment with the specified timestamp": fragment i
                // covers from its own timestamp until fragment i+1's, and the newest
                // fragment covers everything after its own. A timestamp older than
                // every fragment lands on the earliest one.
                "PRODUCER_TIMESTAMP" | "SERVER_TIMESTAMP" => {
                    let wanted = start_timestamp_millis.unwrap_or_default();
                    let producer = selector_type == "PRODUCER_TIMESTAMP";
                    fragments
                        .iter()
                        .rposition(|fragment| {
                            let at = crate::state::epoch_millis(if producer {
                                fragment.producer_timestamp
                            } else {
                                fragment.server_timestamp
                            });
                            at <= wanted
                        })
                        .unwrap_or(0)
                }
                // "CONTINUATION_TOKEN - Read using the specified continuation token":
                // the cursor is whatever this service recorded when it minted the
                // token, and a token it never minted (or one minted for another
                // stream) names no chunk.
                "CONTINUATION_TOKEN" => {
                    let token = selector.continuation_token.as_deref().unwrap_or_default();
                    store
                        .continuation(token, &stream.arn, fragments.len())
                        .ok_or_else(|| {
                            invalid_argument(UNDECODABLE_CONTINUATION_TOKEN_MESSAGE)
                        })?
                }
                // Every modelled selector type is handled above, and an unmodelled one
                // was already refused.
                _ => 0,
            };
            // A cursor at or past the end of the log names no chunk, which is an empty
            // payload rather than a failure.
            let start = start.min(fragments.len());
            // How much of the log this session carries: one session's worth of media,
            // and always at least one chunk so a stream of oversized fragments still
            // makes progress.
            let mut chunks = 0usize;
            let mut bytes = 0usize;
            for fragment in &fragments[start..] {
                let grown = bytes.saturating_add(fragment.chunk.len());
                // The framing this chunk will carry counts against the session too:
                // measuring media bytes alone would let a long run of tiny chunks
                // answer far more than one session's worth of payload.
                let framed = grown.saturating_add(
                    chunks.saturating_add(1).saturating_mul(TAGS_ELEMENT_BUDGET),
                );
                if chunks > 0 && framed > MAX_SESSION_BYTES {
                    break;
                }
                bytes = grown;
                chunks += 1;
            }
            let newest_server_millis = fragments
                .last()
                .map(|fragment| crate::state::epoch_millis(fragment.server_timestamp))
                .unwrap_or_default();
            (
                stream.name.clone(),
                stream.arn.clone(),
                stream.id,
                stream.content_type.clone(),
                newest_server_millis,
                start,
                chunks,
                bytes,
            )
        };

        // One continuation token per emitted chunk, each resuming immediately after
        // the chunk that carries it — which is what makes two sequential calls
        // neither repeat nor skip a fragment.
        let tokens: Vec<String> = (0..chunks)
            .map(|offset| store.record_continuation(&stream_arn, stream_id, start + offset + 1))
            .collect();

        // The payload: each stored chunk verbatim, followed by its own `Tags` element.
        // The media bytes are copied once, straight out of the store. The slice is
        // taken through `get`, so the cursor can only ever bound the run the block
        // above measured under this same lock — and a registered stream with no log
        // here carries no chunk at all, which is the empty payload.
        let mut payload =
            Vec::with_capacity(bytes.saturating_add(chunks.saturating_mul(TAGS_ELEMENT_BUDGET)));
        let emitted = store
            .streams
            .get(&stream_arn)
            .map_or(&[][..], |stream| stream.fragments.as_slice())
            .get(start..start.saturating_add(chunks));
        debug_assert!(
            emitted.is_some(),
            "the stream and its cursor were resolved from this same locked store"
        );
        if let Some(fragments) = emitted {
            for (fragment, token) in fragments.iter().zip(&tokens) {
                let server_millis = crate::state::epoch_millis(fragment.server_timestamp);
                payload.extend_from_slice(&fragment.chunk);
                payload.extend_from_slice(&kvs_tags_element(
                    &fragment.fragment_number,
                    server_millis,
                    crate::state::epoch_millis(fragment.producer_timestamp),
                    token,
                    // "how far behind the chunk returned in the response is from the
                    // latest chunk on the stream".
                    newest_server_millis.saturating_sub(server_millis).max(0),
                ));
            }
        }
        drop(store);

        tracing::debug!(
            operation = Self::NAME,
            stream = %stream_name,
            // The ARN the control-plane registry holds for this stream: the one identity
            // both planes address it by, whichever identifier the request carried.
            stream_arn = %stream_arn,
            start_selector_type = selector_type,
            cursor = start,
            chunks,
            payload_bytes = payload.len(),
            // Debug-formatted, not interpolated: the URI is raw request input, so it
            // reaches the log as a quoted, escaped value and cannot forge a log line.
            request_uri = ?ctx.parts().uri,
            "served a media session"
        );

        Ok(GetMediaOutput {
            content_type: Some(content_type),
            payload,
        })
    }
}
