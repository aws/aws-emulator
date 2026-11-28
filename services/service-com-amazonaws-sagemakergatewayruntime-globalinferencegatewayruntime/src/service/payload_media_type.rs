// SPDX-License-Identifier: CC0-1.0

//! The media type the buffered completion is announced under.
//!
//! `InvokeGlobalInferenceGateway` answers with the completion document itself: its output
//! binds one member as the response's whole `@httpPayload`, and the model types that
//! member as a blob carrying `@mediaType("application/json")`
//! (`smithy_query(query="shape", shape="ResponsePayloadBlob")`). Smithy maps a blob
//! payload to `application/octet-stream`, which is what the framework's serializer labels
//! the response with — the media type of an opaque download — so the media type the MODEL
//! declares for those bytes is announced here instead. The body is an OpenAI completion
//! JSON a caller parses, not an opaque download.
//!
//! The re-announcement itself is the shared rule of
//! [`::service_common::response_media_type`] — this service states only its own pair,
//! [`PAYLOAD_MEDIA_TYPE`] -> [`ANNOUNCED_MEDIA_TYPE`], so every other response (every
//! modelled refusal this `restJson1` renderer already stamps `application/json`, and the
//! streaming route's event-stream body) is left exactly as its producer wrote it.
//!
//! Taken in a layer in FRONT of the operation rather than in the handler, because the
//! handler answers the modelled output and which media type the wire carries it under is a
//! property of the response the protocol built from it.

use ::aws_smithy_runtime_api::http::{Request as RtRequest, Response as RtResponse};
use ::service_common::response_media_type::ResponseMediaType;
use ::smithy_server_runtime::{BoxFuture, ErasedOperation, OperationContext, Service};

/// The media type the `restJson1` renderer stamps on a response whose body is a blob
/// `@httpPayload` — Smithy's default for a blob, and the only value this service
/// re-announces.
const PAYLOAD_MEDIA_TYPE: &str = "application/octet-stream";

/// The media type the model declares for the completion payload.
const ANNOUNCED_MEDIA_TYPE: &str = "application/json";

/// The pair this service's completion payload is re-announced across.
const MEDIA_TYPE: ResponseMediaType =
    ResponseMediaType::new(PAYLOAD_MEDIA_TYPE, ANNOUNCED_MEDIA_TYPE);

/// One operation whose payload is announced as the media type its model declares.
pub struct AnnouncedAsModelled<Inner> {
    inner: Inner,
}

impl<Inner> AnnouncedAsModelled<Inner> {
    /// Serve `inner`, announcing the media type the model declares for its payload.
    pub fn new(inner: Inner) -> Self {
        Self { inner }
    }
}

impl<Inner: ErasedOperation> ErasedOperation for AnnouncedAsModelled<Inner> {
    fn run<'r>(
        &'r self,
        svc: &'r dyn Service,
        req: &'r RtRequest,
        ctx: &'r OperationContext<'r>,
    ) -> BoxFuture<'r, RtResponse> {
        Box::pin(async move {
            let response = self.inner.run(svc, req, ctx).await;
            // The status, the body and every other header are the renderer's; only the
            // media type the payload declares is this service's own, and only where the
            // renderer stamped the payload-implied one.
            MEDIA_TYPE.announce(response)
        })
    }

    fn auth_schemes(&self) -> Option<&'static [&'static str]> {
        // The layer is not an operation of its own: the request is signed as the MODEL says
        // for the operation it names, so authentication is decided by the operation this
        // layer stands in front of.
        self.inner.auth_schemes()
    }
}
