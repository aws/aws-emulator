// SPDX-License-Identifier: CC0-1.0

//! The header a refusal of this service reports its correlation id in.
//!
//! Every error shape Directory Service models carries a `RequestId` member, and
//! `crate::state::refusal` is what fills it — so a refusal this service words already
//! STATES the id of the call it answers, in the body and at the end of `Message`. AWS
//! reports that same id in the response's own `x-amzn-requestid` header, which is where an
//! SDK's response metadata reads a failed call's request id from, so a response that
//! states an id in its document must carry it in that header too: a client correlating on
//! the header and one reading the document have to see ONE value.
//!
//! The framework's error renderers write the document and no such header
//! (`vendor/` is read-only), so this module adds it, from the id the document already
//! states — it mints nothing and rewrites no byte of the body. A response that states no
//! id (a success, whose envelope has no request-id member) is returned untouched.
//!
//! This service's model declares TWO wire protocols side by side, `awsJson1_1` and
//! `rpcv2Cbor`
//! (`smithy_query(query="service_info",model_path="services/service-com-amazonaws-directoryservice-directoryservice-20150416/smithy-model/com-amazonaws-directoryservice-directoryservice-20150416.json")`),
//! and a client may call every operation in either, so each request is read and answered in
//! the one it arrived in: the codec this wrapper resolves the route and the refusals below
//! with is the one the framework selected for THIS request
//! ([`::smithy_server_runtime::ErasedServiceDefinition::wire_protocol`]), never the first
//! protocol the model happens to declare. The id is read back out of the answered document
//! with the codec that wrote it ([`stated_request_id`]), so a client of either protocol
//! correlates on the same value.
//!
//! It reaches every answer the routing table's operation produces — a handler-returned
//! modelled error, the scaffold's 501 and a refusal raised while that operation was being
//! served — through [`RequestIdHeaderOperation`], the operation wrapper
//! `Service::override_operation` hands back.
//!
//! That wrapper is also where a refusal this service states from the REQUEST is answered,
//! for the two classes the centralized walk cannot answer on its own: a constraint a list's
//! ELEMENTS break, stated at the list member quoting the list the request sent
//! ([`crate::service::list_element_refusals`]), and a `@pattern` whose look-around that
//! walk's regex engine cannot compile, which it therefore never applied
//! ([`crate::service::model_pattern_refusals`]). A request that breaks neither is handed to
//! the routing table's operation untouched. A request that breaks BOTH is ONE refusal
//! whichever of the two renders it: the clauses of both classes are counted in the one
//! envelope, which is why the first statement below is asked first rather than instead.

use aws_smithy_runtime_api::http::{Request as RtRequest, Response as RtResponse};
use aws_smithy_schema::codec::Codec;
use aws_smithy_schema::serde::{SerdeError, ShapeDeserializer};
use aws_smithy_schema::{Schema, ShapeId, ShapeType};
use aws_smithy_schema_server::{CborCodec, DeserializableStruct, ProtocolError};
use smithy_server_runtime::{BoxFuture, ErasedOperation, OperationContext, Service};

/// The header AWS reports a response's own request id in.
const REQUEST_ID_HEADER: &str = "x-amzn-requestid";

/// The member every error shape of this service states that id in; see
/// `crate::state::refusal`.
const REQUEST_ID_MEMBER: &str = "RequestId";

/// The media type the `rpcv2Cbor` protocol labels every body it writes with, a failure's
/// included: what tells this module which codec wrote the document it is reading.
const CBOR_MEDIA_TYPE: &str = "application/cbor";

/// Whether `resp` was written by this service's `rpcv2Cbor` codec.
///
/// The media type alone decides, read as a media type — the part before any parameter,
/// case-insensitively — because that is what the header states and its case is not
/// significant.
fn answered_in_cbor(resp: &RtResponse) -> bool {
    resp.headers().get("content-type").is_some_and(|header| {
        header
            .split(';')
            .next()
            .unwrap_or_default()
            .trim()
            .eq_ignore_ascii_case(CBOR_MEDIA_TYPE)
    })
}

/// The one member read out of an answered document: the id the answer states. Declared in
/// the `smithy.framework` namespace because it is no shape of this service's model — it is
/// the member every error shape of the model declares, named once so a document written
/// from any of them can be read back the same way.
static STATED_REQUEST_ID_MEMBER: Schema = Schema::new_member(
    ShapeId::from_static(
        "smithy.framework#StatedRequestId$RequestId",
        "smithy.framework",
        REQUEST_ID_MEMBER,
    ),
    ShapeType::String,
    REQUEST_ID_MEMBER,
    0,
)
.with_json_name(REQUEST_ID_MEMBER);

/// The document as far as this module reads it: the stated id and nothing else, so every
/// other member the renderer wrote is read past rather than named here.
static STATED_REQUEST_ID_SCHEMA: Schema = Schema::new_struct(
    ShapeId::from_static(
        "smithy.framework#StatedRequestId",
        "smithy.framework",
        "StatedRequestId",
    ),
    ShapeType::Structure,
    &[&STATED_REQUEST_ID_MEMBER],
);

/// The id an answered document states, as the codec that wrote it reads it back.
struct StatedRequestId(Option<String>);

impl DeserializableStruct for StatedRequestId {
    fn deserialize(deserializer: &mut dyn ShapeDeserializer) -> Result<Self, SerdeError> {
        let mut stated = None;
        deserializer.read_struct(&STATED_REQUEST_ID_SCHEMA, &mut |member, reader| {
            if member.member_name() == Some(REQUEST_ID_MEMBER) {
                stated = Some(reader.read_string(member)?);
            }
            Ok(())
        })?;
        Ok(Self(stated))
    }
}

/// The request id the response's document states, or `None` when it states none.
///
/// Read with the codec that WROTE the document — the one the request's own protocol pairs
/// with — because this service declares two and an answer in one of them cannot be read as
/// the other: a CBOR map is no JSON document. A document that states no id — a success, or
/// a framework-level refusal, whose envelope has no such member — yields `None` on either
/// wire.
///
/// An error envelope is built in memory by its renderer, so the bytes are readable here; a
/// streamed body is not one and yields `None`.
fn stated_request_id(resp: &RtResponse) -> Option<String> {
    let bytes = resp.body().bytes()?;
    if answered_in_cbor(resp) {
        let codec = CborCodec::default();
        let mut deserializer = codec.create_deserializer(bytes);
        return StatedRequestId::deserialize(&mut deserializer).ok()?.0;
    }
    let document: serde_json::Value = serde_json::from_slice(bytes).ok()?;
    document
        .get(REQUEST_ID_MEMBER)?
        .as_str()
        .map(str::to_string)
}

/// The response with its stated request id in [`REQUEST_ID_HEADER`].
///
/// Status, body and every other header are left exactly as the renderer wrote them, and a
/// response that already carries the header, or states no id, is returned untouched — so
/// this is idempotent and cannot affect the success path.
pub fn with_request_id_header(mut resp: RtResponse) -> RtResponse {
    if resp.headers().get(REQUEST_ID_HEADER).is_some() {
        return resp;
    }
    let Some(request_id) = stated_request_id(&resp) else {
        return resp;
    };
    // Dropped rather than fatal if the HTTP layer cannot carry it: the document already
    // states the id, and a header is not worth failing a modelled answer for.
    let _ = resp.headers_mut().try_insert(REQUEST_ID_HEADER, request_id);
    resp
}

/// The modelled operation this service's routing table names for this request, or `None`
/// when the request names none.
fn modeled_operation<'s>(svc: &'s dyn Service, req: &RtRequest) -> Option<&'s dyn ErasedOperation> {
    let definition = svc.definition();
    // The protocol of THIS request, not the first the model declares: a `rpcv2Cbor` call
    // names its operation in the URI and an `awsJson1_1` one in a header, so a key resolved
    // with the wrong codec names no operation at all.
    let key = definition
        .wire_protocol(req)
        .protocol()
        .routing_key(req)
        .ok()?;
    definition.routing_table().get(key)
}

/// Serves a modelled operation through the framework exactly as the routing table would,
/// then reports the answer's stated request id in the header AWS carries it in.
///
/// It stores no operation of its own: the one it delegates to is the one the service's
/// routing table names for the request, so the framework still owns the whole
/// deserialize → validate → handle → serialize sequence — and every status, code and
/// message it answers with — while this wrapper owns one header.
pub struct RequestIdHeaderOperation;

impl ErasedOperation for RequestIdHeaderOperation {
    fn run<'r>(
        &'r self,
        svc: &'r dyn Service,
        req: &'r RtRequest,
        ctx: &'r OperationContext<'r>,
    ) -> BoxFuture<'r, RtResponse> {
        Box::pin(async move {
            // The refusals this service states from the request itself, for the two classes
            // the centralized walk cannot answer on its own: a constraint a list's ELEMENTS
            // break, stated at the list member quoting the list the request sent, and a
            // `@pattern` that walk could not compile and so never applied. `None` from both
            // leaves the whole sequence below exactly as it was. Both are collected with the
            // codec the request's own protocol pairs with, so a CBOR call is refused in the
            // same words an awsJson one is. The definition is read, used and dropped inside
            // the block: `&dyn ErasedServiceDefinition` is not `Sync`, so holding it across
            // the await below would make this future non-`Send`.
            let refused = {
                let protocol = svc.definition().wire_protocol(req);
                crate::service::list_element_refusals::stated(protocol.protocol(), req).or_else(
                    || crate::service::model_pattern_refusals::stated(protocol.protocol(), req),
                )
            };
            if let Some((violations, stated)) = refused {
                return with_request_id_header(svc.definition().refusal_response(
                    ProtocolError::Invalid(violations),
                    Some(stated),
                    req,
                ));
            }
            match modeled_operation(svc, req) {
                Some(op) => with_request_id_header(op.run(svc, req, ctx).await),
                // Unreachable through `Service::override_operation`, which claims a
                // request only when the routing table holds its key; answered as the
                // framework answers an unroutable request rather than guessed at.
                None => svc.definition().routing_error_response(req),
            }
        })
    }

    fn auth_schemes(&self) -> Option<&'static [&'static str]> {
        // `None` is "defer to the modelled operation the routing key names" — the
        // operation this wrapper delegates to, so its model-declared schemes decide and
        // this wrapper cannot weaken them.
        None
    }
}
