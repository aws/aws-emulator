// SPDX-License-Identifier: CC0-1.0

//! The `com-amazonaws-ivs-amazoninteractivevideoservice` service: how it claims requests and how it words the
//! failures the framework hands back.
//!
//! EDIT THIS FILE. It is emitted once with working defaults and is never
//! regenerated, so a choice made here survives adding another service or
//! re-running the generator.
//!
//! `EmulatedService` is what the router mounts (`EmulatedService::new(engine)`).
//! It wraps the generated [`ServiceDefinition`], which carries the model-derived half
//! of this service — its identity, its wire protocol, and its operation routing
//! table — built from the Smithy model at compile time. You do not edit the
//! entry; you edit the behavioral seams below.
//!
//! Five opt-in seams are yours to change; every one defaults to the framework's
//! own behaviour, so an unedited service still routes and answers correctly:
//!
//! - `can_handle` — which requests this service claims. The default matches the
//!   request's resolved route key against this service's signing name. Edit it
//!   for anything the model does not express: a signing name shared by several
//!   services, or routing that keys on the access-key id.
//! - `override_operation` — EDITED HERE. The key and the handler stay the generated
//!   table's; each operation is served through [`StatedEnvelope`], which states a
//!   constraint refusal the way IVS states it and names the request every response
//!   answers. See "How this service words a refusal" and "The envelope every response
//!   carries".
//! - `override_refusal` — how this service words a request the framework refused
//!   before any handler ran. The default returns `None`, keeping the framework's
//!   wording. See the inline note on that method for how to reword.
//! - `override_routing_error` — how this service words a request that reached it
//!   but named no modeled operation. The default returns `None`, so the entry
//!   renders the protocol-correct default (query -> 400 InvalidAction, awsJson ->
//!   404, …). Return `Some(resp)` to answer in your own envelope if desired.
//! - `override_framework_error` — how this service words a transport-level
//!   failure the orchestrator detects up front (e.g. a malformed `aws-chunked`
//!   body). The default returns `None`, keeping the entry's protocol-correct
//!   rendering. Return `Some(resp)` to reword it if desired.
//!
//! # The envelope every response carries
//!
//! AWS answers every IVS call — success or failure — with a request id, and answers a
//! failure with the error's code and text in the places a client reads them. The model
//! says which places: `cors.additionalExposedHeaders`
//! (`smithy_query(query="service_info")`) lists `x-amzn-requestid`, `x-amzn-errortype`
//! and `x-amzn-errormessage`, which are the response headers this service publishes to a
//! browser client and therefore the ones it sends. The protocol serializer already writes
//! `x-amzn-errortype` and the body's `__type` from the modelled error shape, and the body
//! carries the text under the `message` key an SDK error parser reads
//! (`crate::scaffolded`); what is added here is the error DOCUMENT a client of this
//! service reads a failure out of — `Code`, `Message` and `RequestID`, the three members
//! the recorded IVS failures carry — plus the request id on every response and the text
//! beside the code in the header.
//!
//! The id is a property of the RESPONSE, not of any modelled shape: no operation's output
//! declares it, so no handler can carry it, and a failure must carry one exactly as a
//! success does. Hence [`with_response_envelope`], applied at the one place every
//! response an operation of this service answers passes through — [`StatedEnvelope`],
//! the per-operation wrapper. What FORM that id takes is this endpoint's own, and is
//! stated once in [`response_id`].
//!
//! # How this service words a refusal
//!
//! IVS states a value outside a modelled constraint as `<Operation>.<member path>`
//! followed by the constraint it violated — "`CreateChannel.name does not match the
//! required pattern ^[a-zA-Z0-9-_]*$`",
//! "`CreateAdConfiguration.mediaTailorPlaybackConfigurations is required to have a
//! minimum length of 1, found 0`" — where the framework's own pre-dispatch envelope
//! words every service's refusal alike ("1 validation error detected: Value '…' at
//! '…' failed to satisfy constraint: …"), with no operation in it.
//!
//! The operation is not reachable at the framework's refusal seam:
//! [`Service::override_refusal`] is handed the violations alone, with no request and no
//! operation. So the operations are served through [`StatedEnvelope`], which collects the
//! refusal itself — from the framework's own deserializer and constraint walk, so not
//! one modelled constraint is restated here — and words each violation from the
//! OPERATION it refused, which a per-operation wrapper has in scope as `O::NAME`. The
//! sentences live in `crate::state`, beside the message rules the handlers state their
//! modelled failures with, and the body is the one key an SDK error parser reads
//! (`crate::scaffolded::StatedBody`). Nothing else about the request changes: a request
//! this service does not refuse is handed to the generated `ErasedOperation::run`, which
//! owns deserialize -> validate -> handle -> serialize, telemetry, and every modelled
//! error.
//!
//! Two costs of serving the operations this way are deliberate. The wrapper reads the
//! request a second time (`deserialize_input` + `constraints::validate`, which the
//! generated `run` then repeats), so a request the model accepts is parsed and validated
//! twice; the framework exposes no seam that hands a refusal to the service before
//! rendering it, and the correct refusal is worth one extra parse of a control-plane
//! request. And the wrapped set is spelled out in [`wrap_operations`]: an operation the
//! model gains later is NOT wrapped until it is listed there, and keeps the framework's
//! generic sentence until it is — the fallback is a worse message, never a wrong route,
//! because `override_operation` answers `None` for a key it does not hold and the
//! generated table serves it unchanged.
//!
//! # The one surface the framework does not authenticate, and how it authenticates it
//!
//! Every modelled operation of this service is served through the framework's signed front
//! door. One surface is not modelled: the PLAYBACK path a viewer follows, which carries no
//! credential of this emulator because AWS serves a public channel's playback to a player
//! that holds none — which is why it is answered at `override_routing_error`, the seam a
//! request naming no modelled operation reaches before authentication.
//!
//! That surface checks its own caller instead. A channel that is "private (enabled for
//! playback authorization)" is served only to a viewer presenting a playback token signed
//! with the private half of one of the account's imported playback key pairs, naming that
//! channel and inside its own validity window; anything else is refused `403` before a byte
//! of the channel's media is read. The verification is `crate::state::playback_authorization`
//! — the signature over the token's own bytes, then its window, then the channel it names —
//! and [`playback_route`] applies it on every one of the three paths a channel serves.
//!
//! A sixth seam, `override_protocol`, is one this service DOES take: the codec every
//! modelled response is rendered through. It is the model's own `restJson1` in every
//! respect but one: an
//! answer at the no-content status carries no body and states the length it has
//! (`content-length: 0`), which is what the recorded 204s of this service carry and what
//! the protocol's serializer cannot do, rendering a members-less output structure as the
//! empty JSON object `{}` whatever status it is sent at; the rule itself is the tree's
//! one reading of it (`service_common::no_content_responses`). It is applied here, at
//! the one place every response of this service passes through, rather than per
//! operation, because no handler can reach the rendering of its own output. The HTTP/1.1
//! writer this emulator is served on removes `content-length` from a no-content answer
//! again, which is below anything this crate decides.
//!
//! The model-derived half of this service — its identity, wire protocol, routing
//! table, and the default error renderers — lives in the generated `ServiceDefinition`
//! and is reached, not edited, through the one `definition` method below. You
//! do not write it; editing the entry would let this file drift from the model.

mod playback_route;
mod response_id;

use aws_smithy_runtime_api::http::{Request as RtRequest, Response as RtResponse};
use aws_smithy_schema::serde::SerdeError;
use aws_smithy_schema::Schema;
use aws_smithy_schema_server::{constraints, ProtocolError, ServerProtocol};
use smithy_server_runtime::{
    http, BoxFuture, DeserializableStruct, ErasedOperation, Operation, OperationContext,
    OperationRoutingTable, Refusal, Refused, ServerHttpProtocol, Service, Violation,
};
use std::sync::Arc;

use response_id::new_request_id;

/// The wire error code for a request this service refuses as invalid: the shape
/// `com.amazonaws.ivs#ValidationException`, which every operation of this service lists.
const VALIDATION_EXCEPTION: &str = "ValidationException";

/// The HTTP status that shape's `@httpError` declares.
const VALIDATION_EXCEPTION_STATUS: u16 = 400;

/// The response header AWS names the request it answers in, for this protocol, and one of
/// the headers this service's model publishes in `cors.additionalExposedHeaders`.
const REQUEST_ID_HEADER: &str = "x-amzn-requestid";

/// The response header the protocol serializer writes an error's CODE in, and the one
/// restJson1 resolves an error shape from. Read here (never written) to fill the error
/// document's own code member, so the code in the document is the model-derived one and
/// not a second derivation of it.
const ERROR_TYPE_HEADER: &str = "x-amzn-errortype";

/// The response header AWS states an error's text in, beside the code. Also from
/// `cors.additionalExposedHeaders`.
const ERROR_MESSAGE_HEADER: &str = "x-amzn-errormessage";

/// The body key an SDK error parser reads an error's text under, and the one
/// `crate::scaffolded::STATED_ERROR_SCHEMA` renders it as.
const SDK_MESSAGE_KEY: &str = "message";

/// The body key the protocol serializer writes the error shape name under.
const SDK_TYPE_KEY: &str = "__type";

/// The three members of the error document this service answers a failure with: the
/// error's code, its text, and the id of the request it answers.
///
/// Recorded: a refused IVS call answers a JSON document carrying `Code`, `Message` and
/// `RequestID`, which is what a client of this service reads a failure out of. They sit
/// beside the protocol serializer's own `__type` and the [`SDK_MESSAGE_KEY`] an SDK error
/// parser reads — the same code and the same text under the names each reader looks for,
/// never a second or different rendering of either.
const DOCUMENT_CODE_KEY: &str = "Code";
const DOCUMENT_MESSAGE_KEY: &str = "Message";
const DOCUMENT_REQUEST_ID_KEY: &str = "RequestID";

/// The lowest status that makes a response a failure, and so the responses the error
/// document belongs on.
const FIRST_ERROR_STATUS: u16 = 400;

/// The error document this service answers `resp` with, when `resp` is a failure carrying
/// a JSON body: the document the renderer already wrote, with the code, the text and the
/// request id added under the names a client of this service reads them by.
///
/// NOTHING is derived twice. The code is the one the protocol serializer put in
/// [`ERROR_TYPE_HEADER`] — model-derived, from the modelled error shape the operation
/// declares — falling back to the body's own `__type` when a renderer wrote only that.
/// The text is the one the renderer wrote under [`SDK_MESSAGE_KEY`], built by
/// `crate::state`'s message rules. A member the document already carries is left alone.
///
/// `None` when there is nothing to restate: a success, a response with no body (the
/// modelled 204 of `DeleteChannel`, say), a streamed body, a body that is not a JSON
/// object, or an error whose code cannot be read at all — in which case the response goes
/// out exactly as the renderer wrote it rather than with a document this service made up.
fn error_document(resp: &RtResponse, request_id: &str) -> Option<RestatedError> {
    if resp.status().as_u16() < FIRST_ERROR_STATUS {
        return None;
    }
    // An error body is built in memory by its renderer, so the bytes are readable here;
    // a streamed body is not an error document and is left alone.
    let bytes = resp.body().bytes()?;
    let mut document: serde_json::Map<String, serde_json::Value> =
        serde_json::from_slice(bytes).ok()?;
    let code = resp
        .headers()
        .get(ERROR_TYPE_HEADER)
        .map(str::to_string)
        .or_else(|| {
            document
                .get(SDK_TYPE_KEY)
                .and_then(serde_json::Value::as_str)
                .map(str::to_string)
        })?;
    let message = document
        .get(SDK_MESSAGE_KEY)
        .or_else(|| document.get(DOCUMENT_MESSAGE_KEY))
        .and_then(serde_json::Value::as_str)
        .unwrap_or_default()
        .to_string();
    document
        .entry(DOCUMENT_CODE_KEY)
        .or_insert_with(|| serde_json::Value::String(code));
    if !message.is_empty() {
        document
            .entry(DOCUMENT_MESSAGE_KEY)
            .or_insert_with(|| serde_json::Value::String(message.clone()));
    }
    document
        .entry(DOCUMENT_REQUEST_ID_KEY)
        .or_insert_with(|| serde_json::Value::String(request_id.to_string()));
    Some(RestatedError {
        body: serde_json::Value::Object(document).to_string(),
        message,
    })
}

/// One failure's error document and the text it states, so the header and the body carry
/// the same text and not two renderings of it.
struct RestatedError {
    body: String,
    message: String,
}

/// The response as this service sends it: naming the request it answers, and — on a
/// failure — stating the error's code, text and request id in the document a client reads
/// a failure out of.
///
/// The status and every member the renderer wrote are left exactly as they were, and a
/// header already present is left alone, so the one writer of each is whoever wrote it
/// first. Every header insert is FALLIBLE: a header the HTTP layer cannot represent is
/// dropped rather than turned into a transport failure, which on the error path would
/// replace a modelled 4xx with a 500 an SDK retries.
///
/// ONE id names one response: an id a renderer already put in [`REQUEST_ID_HEADER`] is the
/// one the document states too, and a fresh one ([`new_request_id`]) is minted only when
/// the response names no request yet — so the header and the document can never carry two
/// different ids for the same response.
fn with_response_envelope(mut resp: RtResponse) -> RtResponse {
    let request_id = resp
        .headers()
        .get(REQUEST_ID_HEADER)
        .map(str::to_string)
        .unwrap_or_else(new_request_id);
    if let Some(restated) = error_document(&resp, &request_id) {
        // Only a renderer that advertised a length needs it corrected; the framework's
        // error renderers set `content-type` alone and leave framing to the transport.
        if resp.headers().get("content-length").is_some() {
            let length = restated.body.len().to_string();
            let _ = resp.headers_mut().try_insert("content-length", length);
        }
        *resp.body_mut() = aws_smithy_types::body::SdkBody::from(restated.body);
        if resp.headers().get(ERROR_MESSAGE_HEADER).is_none() && !restated.message.is_empty() {
            let _ = resp
                .headers_mut()
                .try_insert(ERROR_MESSAGE_HEADER, restated.message);
        }
    }
    if resp.headers().get(REQUEST_ID_HEADER).is_none() {
        let _ = resp.headers_mut().try_insert(REQUEST_ID_HEADER, request_id);
    }
    resp
}

/// Whether the request these `parts` came from matches this service's SigV4 signing
/// name AND the modelled URI path space its generated `ROUTE_PATH_SEGMENTS` declares.
///
/// `emulator_server::can_handle_based_on_route_key_and_path` is the agent-immutable
/// owner of that rule, and it is CALLED here rather than re-implemented: the request's
/// route key must match this service's signing name AND its first URI path segment must
/// be one the generated, model-derived `ROUTE_PATH_SEGMENTS` declares, by exact segment
/// equality. That path conjunct is what separates two services sharing one SigV4 signing
/// name — this low-latency IVS endpoint (`ivs`) and the IVS RealTime endpoint
/// (`ivsrealtime`) both sign as `ivs`, so the key alone matches both and the router
/// rejects every such request as `500 AmbiguousService`.
///
/// The helper takes a whole `http::Request` while [`Service::can_handle`] is handed only
/// the request's parts, so the two things it reads — the URI and the headers — are
/// re-presented to it as a bodiless request. It reads neither the body nor the method, so
/// nothing it decides on is missing.
fn claims_route_key_and_path(parts: &http::request::Parts) -> bool {
    let Ok(mut request) = http::Request::builder()
        .uri(parts.uri.clone())
        .body(bytes::Bytes::new())
    else {
        return false;
    };
    *request.headers_mut() = parts.headers.clone();
    emulator_server::can_handle_based_on_route_key_and_path(
        &request,
        crate::ROUTE_KEY,
        crate::ROUTE_PATH_SEGMENTS,
    )
}

/// The one first URI path segment the two services that sign as `ivs` BOTH bind, so the
/// model-derived path discriminator alone cannot separate them there.
///
/// Each of the two models binds its three tagging operations to `/tags/{resourceArn}`
/// (`ListTagsForResource` `GET`, `TagResource` `POST`, `UntagResource` `DELETE`); every
/// other operation of either service binds a URI whose first segment is its own
/// operation name, and those are disjoint. Source: `smithy_query(query="digest",
/// model_path="services/service-com-amazonaws-ivs-amazoninteractivevideoservice/\
/// smithy-model/com-amazonaws-ivs-amazoninteractivevideoservice.json")` and the same
/// call on the `…-ivsrealtime-amazoninteractivevideoservicerealtime` model.
const SHARED_TAGGING_PATH_SEGMENT: &str = "tags";

/// The ARN resource types the SIBLING `ivsrealtime` endpoint owns — the resources its own
/// model creates and therefore the only ones its tagging operations can address.
///
/// Both services' ARNs live in the one `ivs` arnNamespace, so the resource TYPE is what
/// names the owning endpoint. Source: the `ivsrealtime` model's ARN shapes, whose
/// patterns pin the resource segment —
/// `smithy_query(query="shape", shape="CompositionArn,EncoderConfigurationArn,\
/// IngestConfigurationArn,PublicKeyArn,StageArn,StorageConfigurationArn",
/// model_path="services/service-com-amazonaws-ivsrealtime-\
/// amazoninteractivevideoservicerealtime/smithy-model/\
/// com-amazonaws-ivsrealtime-amazoninteractivevideoservicerealtime.json")`. That model's
/// `ChannelArn` is deliberately NOT here: it is a REFERENCE to a low-latency channel
/// (RealTime has no operation that creates one), and a channel is tagged through this
/// endpoint.
const SIBLING_ENDPOINT_RESOURCE_TYPES: &[&str] = &[
    "composition",
    "encoder-configuration",
    "ingest-configuration",
    "public-key",
    "stage",
    "storage-configuration",
];

/// The `arnNamespace` both services share, and so the only service field an ARN either
/// of their tagging operations addresses can carry. From
/// `smithy_query(query="service_info")` on either model (`awsService.arnNamespace`:
/// `ivs` for both).
const SHARED_ARN_NAMESPACE: &str = "ivs";

/// What a request path says about the `/tags/{resourceArn}` space both `ivs`-signing
/// services publish.
#[derive(Debug)]
enum TaggingTarget {
    /// Not on the shared path at all: the modelled path space has already decided the
    /// claim, and this discriminator adds nothing.
    NotShared,
    /// On the shared path, naming an IVS resource of this type (lowercased).
    Resource(String),
    /// On the shared path, naming no IVS resource either model could recognise.
    Unrecognised,
}

/// Read the shared tagging path: whether this request is on it, and which IVS resource
/// type its `{resourceArn}` label names.
///
/// One function rather than a path-segment helper plus an ARN helper, so the whole
/// shared-path decision is read in one place. The `{resourceArn}` label is
/// percent-decoded (it is a non-greedy `@httpLabel`, so a client encodes its `:` and
/// `/`) and its resource type is lowercased, so a non-canonical ARN cannot land a
/// request on the wrong endpoint. The `tags` segment itself is compared RAW, because
/// the framework's path conjunct has already matched that segment against the model's
/// `ROUTE_PATH_SEGMENTS` by exact equality: decoding it here could only disagree with
/// the conjunct that runs first.
///
/// The label counts as naming a resource only when it is an ARN in the `ivs` namespace
/// the two services share — `arn:<partition>:ivs:<region>:<account>:<type>/<id>`, the
/// form every ARN shape in either model pins — so another service's ARN, or a string
/// that merely has a sixth `:`-field, names nothing. Reading that field and cutting it
/// at `/` also handles a client that sent the ARN's separators raw, because the ARN's
/// own `/` then just starts the next path segment.
fn tagging_target(path: &str) -> TaggingTarget {
    let mut segments = path.trim_start_matches('/').split('/');
    if segments.next() != Some(SHARED_TAGGING_PATH_SEGMENT) {
        return TaggingTarget::NotShared;
    }
    let Some(label) = segments.next() else {
        return TaggingTarget::Unrecognised;
    };
    let arn = smithy_server_runtime::percent_decode(label);
    let fields: Vec<&str> = arn.split(':').collect();
    // arn : <partition> : <service> : <region> : <account> : <resource-type>/<id>
    let [prefix, _partition, service, region, account, resource, ..] = fields.as_slice() else {
        return TaggingTarget::Unrecognised;
    };
    let kind = resource
        .split('/')
        .next()
        .unwrap_or_default()
        .trim()
        .to_ascii_lowercase();
    if !prefix.eq_ignore_ascii_case("arn")
        || !service.eq_ignore_ascii_case(SHARED_ARN_NAMESPACE)
        || region.is_empty()
        || account.is_empty()
        || kind.is_empty()
    {
        return TaggingTarget::Unrecognised;
    }
    TaggingTarget::Resource(kind)
}

/// Whether this service claims a request on the `/tags/{resourceArn}` path space both
/// `ivs`-signing services publish.
///
/// The ARN in the path names the resource, and a resource belongs to exactly one of the
/// two endpoints, so the resource type decides: this endpoint claims every tagging
/// request EXCEPT one naming a resource type the sibling `ivsrealtime` endpoint owns.
/// Stated as a complement rather than as "my own types" on purpose — it makes this
/// service, whose `endpointPrefix` is the shared signing name `ivs`, the single claimant
/// for a label that names no recognisable IVS resource, so a malformed or unknown ARN is
/// answered by a modelled 4xx from a real service instead of the router's
/// `404 UnknownService`.
fn claims_shared_tagging_path(parts: &http::request::Parts) -> bool {
    match tagging_target(parts.uri.path()) {
        TaggingTarget::NotShared | TaggingTarget::Unrecognised => true,
        TaggingTarget::Resource(kind) => {
            !SIBLING_ENDPOINT_RESOURCE_TYPES.contains(&kind.as_str())
        }
    }
}

/// The `com-amazonaws-ivs-amazoninteractivevideoservice` emulated service: the generated [`ServiceDefinition`] plus
/// the behavioral seams this file owns.
///
/// Generic over the engine type `Eng` so this file names no concrete engine; the
/// router builds one shared engine and hands it to `new`.
pub struct EmulatedService<Eng: Send + Sync + 'static> {
    entry: crate::ServiceDefinition<Eng>,
    /// The shared engine, so the seams this file owns that are NOT a modelled operation —
    /// the front-door route a channel's playback URL is fetched on
    /// ([`playback_route`]) — reach this service's store the way a handler does.
    engine: Arc<Eng>,
    /// The same modelled operations the generated table holds, each wrapped in
    /// [`StatedEnvelope`] so a constraint refusal is stated the way IVS states it and
    /// every response names the request it answers. Keyed by the routing key the protocol
    /// resolves — the key the generated table uses — and built from the SAME shared
    /// engine, so a wrapped operation reads and writes the one store.
    stated_refusals: OperationRoutingTable,
}

/// Wire one `crate::ops::<module>::<Operation>` per line into `table`, each wrapped in
/// [`StatedEnvelope`] and keyed by the operation's own `Operation::NAME` — the
/// model-derived routing key `ServerHttpProtocol::routing_key` resolves and the
/// generated table is keyed by, so no key is spelled by hand here.
macro_rules! wrap_operations {
    ($table:expr, $engine:expr, $($module:ident :: $operation:ident),+ $(,)?) => {
        $(
            $table.add_operation(
                <crate::ops::$module::$operation<Eng> as Operation>::NAME,
                ::std::boxed::Box::new(StatedEnvelope::new(
                    crate::ops::$module::$operation::new(::std::sync::Arc::clone(&$engine)),
                )) as ::std::boxed::Box<dyn ErasedOperation>,
            );
        )+
    };
}

impl<Eng: Send + Sync + 'static> EmulatedService<Eng> {
    /// Build this service from the shared engine. The engine is threaded into the
    /// generated [`ServiceDefinition`], which constructs each operation from it; the
    /// operation's behaviour lives in that operation's own `impl Operation`.
    pub fn new(engine: Arc<Eng>) -> Self {
        let mut stated_refusals = OperationRoutingTable::new();
        // EVERY operation the model binds. A declared constraint is a property of the
        // INPUT SHAPE, not of a handler, so the wrapper goes in front of all of them —
        // implemented or still scaffolded: a request that violates the model is refused
        // the same way whoever would have served it.
        wrap_operations!(
            stated_refusals,
            engine,
            batch_get_channel::BatchGetChannel,
            batch_get_stream_key::BatchGetStreamKey,
            batch_start_viewer_session_revocation::BatchStartViewerSessionRevocation,
            create_ad_configuration::CreateAdConfiguration,
            create_channel::CreateChannel,
            create_failover_slate_configuration::CreateFailoverSlateConfiguration,
            create_playback_restriction_policy::CreatePlaybackRestrictionPolicy,
            create_recording_configuration::CreateRecordingConfiguration,
            create_stream_key::CreateStreamKey,
            delete_ad_configuration::DeleteAdConfiguration,
            delete_channel::DeleteChannel,
            delete_failover_slate_configuration::DeleteFailoverSlateConfiguration,
            delete_playback_key_pair::DeletePlaybackKeyPair,
            delete_playback_restriction_policy::DeletePlaybackRestrictionPolicy,
            delete_recording_configuration::DeleteRecordingConfiguration,
            delete_stream_key::DeleteStreamKey,
            get_ad_configuration::GetAdConfiguration,
            get_channel::GetChannel,
            get_failover_slate_configuration::GetFailoverSlateConfiguration,
            get_playback_key_pair::GetPlaybackKeyPair,
            get_playback_restriction_policy::GetPlaybackRestrictionPolicy,
            get_recording_configuration::GetRecordingConfiguration,
            get_stream::GetStream,
            get_stream_key::GetStreamKey,
            get_stream_session::GetStreamSession,
            import_playback_key_pair::ImportPlaybackKeyPair,
            insert_ad_break::InsertAdBreak,
            list_ad_configurations::ListAdConfigurations,
            list_channels::ListChannels,
            list_failover_slate_configurations::ListFailoverSlateConfigurations,
            list_playback_key_pairs::ListPlaybackKeyPairs,
            list_playback_restriction_policies::ListPlaybackRestrictionPolicies,
            list_recording_configurations::ListRecordingConfigurations,
            list_stream_keys::ListStreamKeys,
            list_stream_sessions::ListStreamSessions,
            list_streams::ListStreams,
            list_tags_for_resource::ListTagsForResource,
            put_metadata::PutMetadata,
            start_viewer_session_revocation::StartViewerSessionRevocation,
            stop_stream::StopStream,
            tag_resource::TagResource,
            untag_resource::UntagResource,
            update_ad_configuration::UpdateAdConfiguration,
            update_channel::UpdateChannel,
            update_failover_slate_configuration::UpdateFailoverSlateConfiguration,
            update_playback_restriction_policy::UpdatePlaybackRestrictionPolicy,
        );
        Self {
            entry: crate::ServiceDefinition::new(Arc::clone(&engine)),
            engine,
            stated_refusals,
        }
    }
}

/// One modelled operation, served with the constraint refusals it owes stated the way
/// IVS states them — the OPERATION, the dotted member path, and the constraint the
/// request violated — and every response carrying the envelope IVS answers in.
///
/// The refusal is built from the structured [`Violation`]s the framework's own walk
/// collects, never from a rendered response and never from a constraint restated here,
/// so the wrapper cannot find a different set of violations from the walk that follows
/// it. What the wrapper adds is the one thing that walk cannot know: which operation was
/// refused, which a per-operation wrapper has as `O::NAME`.
struct StatedEnvelope<O> {
    operation: O,
}

impl<O> StatedEnvelope<O> {
    /// Wrap one operation, already constructed from the shared engine.
    fn new(operation: O) -> Self {
        Self { operation }
    }
}

impl<O> ErasedOperation for StatedEnvelope<O>
where
    O: Operation + ErasedOperation,
{
    fn run<'r>(
        &'r self,
        svc: &'r dyn Service,
        req: &'r RtRequest,
        ctx: &'r OperationContext<'r>,
    ) -> BoxFuture<'r, RtResponse> {
        Box::pin(async move {
            // Read the protocol out before the await: `&dyn ErasedServiceDefinition` is
            // not `Sync`, and this future is. The protocol itself is.
            let protocol: &dyn ServerHttpProtocol = svc.definition().protocol();
            let violations = refused_by::<O>(protocol, req);
            if let Some(refusal) = validation_exception(protocol, O::NAME, &violations) {
                return with_response_envelope(refusal);
            }
            with_response_envelope(self.operation.run(svc, req, ctx).await)
        })
    }

    /// The wrapped operation's modelled auth schemes: wrapping an operation must not
    /// change which schemes sign it.
    fn auth_schemes(&self) -> Option<&'static [&'static str]> {
        self.operation.auth_schemes()
    }
}

/// Every violation that refuses `req` before `O`'s handler runs: a `@required` member
/// absent, or a value outside a constraint the model declares (`@length`, `@range`,
/// `@pattern`, an enum's value set, `@uniqueItems`). Empty when the request satisfies
/// the model, and empty for a failure that is NOT a refusal — a body the codec cannot
/// read at all is a serialization failure, which the framework words per protocol.
///
/// Both classes come from the framework's own machinery (the protocol's deserializer and
/// `constraints::validate` over the built input), which is the same pair the centralized
/// sequence uses, so no rule is restated here. Each violation's member path is then
/// spelled the way IVS spells one (`crate::state::ElementSteps`), which needs the request
/// the walk ran over and so happens here, where it is still in scope.
fn refused_by<O: Operation>(protocol: &dyn ServerHttpProtocol, req: &RtRequest) -> Vec<Violation> {
    let Ok(mut deserializer) = protocol.deserialize_input(req, O::INPUT_SCHEMA) else {
        return Vec::new();
    };
    match <O::Input as DeserializableStruct>::deserialize(&mut *deserializer) {
        Ok(input) => {
            let violations = constraints::validate(&input, O::INPUT_SCHEMA);
            if violations.is_empty() {
                return violations;
            }
            let steps = crate::state::element_steps(&input, O::INPUT_SCHEMA);
            violations
                .into_iter()
                .map(|violation| Violation {
                    path: steps.respell(&violation.path),
                    ..violation
                })
                .collect()
        }
        // Absence is found while READING the request and a bad value while walking what
        // was read; to a caller they are one refusal, so they are one here — exactly as
        // `smithy_server_runtime::refusal_of` classifies them.
        Err(SerdeError::MissingMember { member_name }) => {
            vec![crate::state::missing_member_violation(member_name)]
        }
        Err(_) => Vec::new(),
    }
}

/// This service's answer to `violations` for `operation`: HTTP 400 `ValidationException`
/// with the prose under the body key an SDK error parser reads.
///
/// `None` when there is nothing to refuse — and also when the refusal cannot be
/// serialized, which leaves the request to the framework's walk rather than answering a
/// 400 with no reason at all.
fn validation_exception(
    protocol: &dyn ServerHttpProtocol,
    operation: &str,
    violations: &[Violation],
) -> Option<RtResponse> {
    let refused = crate::scaffolded::StatedBody::new(crate::state::stated_violations(
        operation, violations,
    )?);
    protocol
        .serialize_error(
            &refused,
            &crate::scaffolded::STATED_ERROR_SCHEMA,
            VALIDATION_EXCEPTION,
            VALIDATION_EXCEPTION_STATUS,
        )
        // A refusal that will not serialize leaves the request to the framework, which
        // refuses it in the generic envelope — the very divergence this wrapper removes,
        // so it is logged rather than passed over silently.
        .inspect_err(|error| {
            tracing::warn!(
                %error,
                "the stated ValidationException would not serialize; this refusal falls \
                 back to the framework's generic envelope"
            );
        })
        .ok()
}

impl<Eng: Send + Sync + 'static> Service for EmulatedService<Eng> {
    fn definition(&self) -> &dyn smithy_server_runtime::ErasedServiceDefinition {
        // The model-derived half of this service — identity, protocol, routing
        // table, and the default error renderers — lives in the generated entry.
        // The router reads it through this one accessor. Do not edit the entry.
        &self.entry
    }

    fn can_handle(&self, parts: &http::request::Parts) -> bool {
        // A request that FOLLOWED the playback URL this service announced for a channel is
        // claimed by the store that announced it, whatever else the request carries: a
        // viewer fetches it with no signature at all, and the model binds no operation to
        // that path, so nothing modelled is diverted here. A path naming a channel this
        // store does not hold is not claimed. CLAIMING it is not serving it: which viewer
        // is served is decided by the credential check on that route below
        // (`override_routing_error`), so a private channel's URL is claimed in order to be
        // REFUSED here rather than answered by the router as an address nobody published.
        // See `crate::state::playback_plane` and `crate::state::playback_authorization`.
        if playback_route::claims_playback_path(&*self.engine, parts) {
            return true;
        }
        // This service signs as `ivs` — a signing name it SHARES with IVS RealTime
        // (`services/service-com-amazonaws-ivsrealtime-amazoninteractivevideoservicerealtime`),
        // whose endpoint is `ivsrealtime` while this one's is `ivs`. The router resolves a
        // request's routing key from its SigV4 credential scope first, so BOTH services
        // see the key `ivs`: claiming on the signing name alone makes each one claim the
        // other's traffic, which the router answers `500 AmbiguousService` instead of
        // serving either.
        //
        // So the claim is narrowed to this service's own request space, in the two steps
        // the request itself can be read by: its modelled URI path space
        // (`claims_route_key_and_path`), and — for the one path space both models publish,
        // `/tags/{resourceArn}` — the resource the ARN in that path names
        // (`claims_shared_tagging_path`).
        claims_route_key_and_path(parts) && claims_shared_tagging_path(parts)
    }

    fn override_operation(
        &self,
        req: &aws_smithy_runtime_api::http::Request,
    ) -> Option<&dyn ErasedOperation> {
        // NOT a re-route: the key is the one the generated table's own protocol
        // resolves, and the operation it names is the one served — wrapped in
        // `StatedEnvelope` so a constraint refusal is stated the way IVS states it
        // (naming the operation and the member path) rather than in the framework's
        // pre-dispatch envelope, which words every service's refusal alike, and so every
        // response carries the request id and error document IVS answers with. A key the
        // model does not name resolves to `None` here exactly as it does in the
        // generated table, so the router's routing-error path is unchanged.
        let key = self.entry.protocol().routing_key(req).ok()?;
        self.stated_refusals.get(key)
    }

    fn override_protocol(&self) -> Option<&dyn smithy_server_runtime::ServerHttpProtocol> {
        // The model's own protocol, with the no-content framing below applied to every
        // modelled response. Implemented on the service itself, so the protocol it
        // delegates to is the generated entry's own and there is no second routing table
        // to drift from the model.
        Some(self)
    }

    fn override_refusal(&self, refusal: &Refusal) -> Option<Refused> {
        // `StatedEnvelope` states a constraint refusal itself, from the operation that
        // was refused, so this seam is only reached for a refusal NO operation owns —
        // one the router answers before a key is resolved (an unauthenticated caller),
        // or the serialization fallback in `validation_exception`. Neither carries an
        // operation to name, and IVS's sentence is built from one, so this keeps the
        // framework's wording rather than inventing a sentence with the operation
        // missing from it.
        let _ = refusal;
        None
    }

    fn override_routing_error(
        &self,
        req: &aws_smithy_runtime_api::http::Request,
    ) -> Option<aws_smithy_runtime_api::http::Response> {
        // The router calls this when a request reached this service but named no
        // modeled operation — neither `override_operation` nor the generated
        // routing table resolved it. Returning `None` renders the protocol-correct
        // default for this service's envelope (query -> 400 InvalidAction text/xml,
        // awsJson -> 404, rpcv2Cbor -> 404, rest -> 501). Return `Some(resp)` to
        // word that routing failure in your own envelope instead.
        //
        // A request that followed the playback URL this service PUBLISHED for a channel is
        // exactly that case by construction — the model binds no operation to a playback
        // path — so this is where that plane is served: the seam is reached BEFORE
        // authentication precisely for a request that names no modelled operation, which
        // is what lets a viewer fetch a PUBLIC channel's playlist with no credentials at
        // all, as AWS serves one. It is the only surface of this service the framework does
        // not authenticate, so it CHECKS ITS OWN CALLER before it reads anything: a channel
        // "private (enabled for playback authorization)" is served only to a viewer
        // presenting a playback token that one of the account's imported playback key pairs
        // verifies for that channel, and is refused `403` otherwise. The verification is
        // `crate::state::playback_authorization`, applied by the route below before any
        // media of the channel is read; nothing on this path treats the mere presence of a
        // token, of a channel id or of a header as proof. Anything else keeps the
        // framework's default.
        playback_route::serve_authorized_playback_path(&*self.engine, req)
    }

    fn override_framework_error(
        &self,
        error: &aws_smithy_schema_server::ProtocolError,
    ) -> Option<aws_smithy_runtime_api::http::Response> {
        // The router calls this on a transport-level failure it detects up front —
        // before (or instead of) naming an operation — such as a malformed
        // `aws-chunked` body. Returning `None` keeps the entry's protocol-correct
        // rendering. Return `Some(resp)` to reword that failure in your own
        // envelope. The error is borrowed, so the default renderer can still
        // consume it on `None`.
        let _ = error;
        None
    }
}

/// The protocol this service answers modelled requests in: the model's own, with the one
/// framing rule below.
///
/// Implemented on the service itself because the model-derived protocol it delegates to is
/// owned by the generated entry and reached by reference (`self.entry.protocol()`), and
/// [`Service::override_protocol`] hands back a borrow of `&self` — so there is one
/// protocol instance, the generated one, and no second routing table to drift from the
/// model.
impl<Eng: Send + Sync + 'static> ServerProtocol for EmulatedService<Eng> {
    type Request = RtRequest;
    type Response = RtResponse;

    fn protocol_id(&self) -> &aws_smithy_schema::ShapeId {
        self.entry.protocol().protocol_id()
    }

    fn routing_key<'a>(&self, request: &'a RtRequest) -> Result<&'a str, ProtocolError> {
        self.entry.protocol().routing_key(request)
    }

    fn deserialize_input<'a>(
        &self,
        request: &'a RtRequest,
        input_schema: &Schema,
    ) -> Result<Box<dyn aws_smithy_schema::serde::ShapeDeserializer + 'a>, ProtocolError> {
        // The REQUEST side is the framework's own.
        self.entry
            .protocol()
            .deserialize_input(request, input_schema)
    }

    fn serialize_output(
        &self,
        output: &dyn aws_smithy_schema::serde::SerializableStruct,
        output_schema: &Schema,
        operation_name: &str,
        success_status: u16,
    ) -> Result<RtResponse, ProtocolError> {
        // The status, the headers and every modelled member stay the framework's: only an
        // answer at the no-content status is REFRAMED — it carries no body and states the
        // length it has, which is what the recorded `204`s of this service (the deletes,
        // `PutMetadata` and the two tagging writes) carry. The rule is the tree's one
        // reading of it (`service_common::no_content_responses`), applied here because the
        // protocol's serializer renders a members-less output structure as `{}` whatever
        // status it is sent at.
        let rendered = self.entry.protocol().serialize_output(
            output,
            output_schema,
            operation_name,
            success_status,
        )?;
        Ok(::service_common::no_content_responses::without_content(
            rendered,
        ))
    }

    fn serialize_error(
        &self,
        error: &dyn aws_smithy_schema::serde::SerializableStruct,
        error_schema: &Schema,
        error_code: &str,
        http_status: u16,
    ) -> Result<RtResponse, ProtocolError> {
        // No error shape of this model carries a channel, so a failure is rendered exactly
        // as the framework renders it.
        self.entry
            .protocol()
            .serialize_error(error, error_schema, error_code, http_status)
    }

    fn serialize_event_stream_message(
        &self,
        event_type: &str,
        is_exception: bool,
        event: &dyn aws_smithy_schema::serde::SerializableStruct,
        event_schema: &Schema,
    ) -> Result<RtResponse, ProtocolError> {
        self.entry.protocol().serialize_event_stream_message(
            event_type,
            is_exception,
            event,
            event_schema,
        )
    }
}

/// `ServerProtocol` requires `Debug`; the generated entry is not `Debug`, so this names
/// the protocol being served rather than the service's internals.
impl<Eng: Send + Sync + 'static> std::fmt::Debug for EmulatedService<Eng> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("EmulatedService")
            .field("protocol", self.entry.protocol().protocol_id())
            .finish()
    }
}
