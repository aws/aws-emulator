// SPDX-License-Identifier: CC0-1.0

//! The HTTP status a mutating operation of this service ANSWERS, where that is not the
//! status the model's `@http` trait declares for it.
//!
//! A `@http code` is the status an operation is modelled to answer, and for most of this
//! service it is the status API Gateway really answers. For a handful of writes it is not:
//! the service answers a status of its own, and a client or waiter that asserts what the
//! service sends sees the model's code as an unexpected one. The status on the wire is the
//! requirement, so those operations answer it, and each constant below says which
//! operations it governs and what it means to a caller.
//!
//! It is a per-operation fact, not a property of the HTTP method or of the kind of write:
//! `UpdateApiMapping`, `UpdateStage`, `UpdateIntegration` and the other patches whose
//! answer agrees with their `@http code` keep it, and every operation not named below is
//! left answering the status its own `@http code` declares.
//!
//! Each `impl Operation` names the constant that governs it in its `SUCCESS_STATUS`, so the
//! statuses that differ from the model are stated once, here, rather than as a bare number
//! in each handler.

/// `201 Created`, the status a PATCH that rewrites an api or one of its children is
/// answered with: `UpdateApi` (`PATCH /v2/apis/{ApiId}`), `UpdateRoute`
/// (`PATCH /v2/apis/{ApiId}/routes/{RouteId}`), `UpdateRouteResponse`
/// (`PATCH …/routeresponses/{RouteResponseId}`) and `UpdateModel`
/// (`PATCH /v2/apis/{ApiId}/models/{ModelId}`), each modelled `code: 200`. The body is
/// the rewritten resource either way; only the status differs.
pub const REWRITTEN_RESOURCE_STATUS: u16 = 201;

/// `200 OK`, the status a write that answers with the whole resource it wrote is answered
/// with: `ImportApi` (`PUT /v2/apis`), `ReimportApi` (`PUT /v2/apis/{ApiId}`) and
/// `CreateApiMapping` (`POST /v2/domainnames/{DomainName}/apimappings`), each modelled
/// `code: 201`.
pub const WRITTEN_RESOURCE_OK_STATUS: u16 = 200;

/// `200 OK`, the status a call that FLUSHES data held for a resource — rather than removing
/// a resource or a member of one — is answered with: `ResetAuthorizersCache`
/// (`DELETE /v2/apis/{ApiId}/stages/{StageName}/cache/authorizers`), modelled `code: 204`.
/// Nothing the stage is configured with is gone afterwards, only the decisions its
/// authorizers had already returned, and `200` is what tells a caller the flush was carried
/// out: the answer has no members at all, so the status line is the only signal it carries,
/// and a `204` would let an intermediary that treats one as "no entity, no entity headers"
/// present the same reply as a no-op. The answer still carries no content, and states the
/// length of that nothing like every other empty answer of this service
/// (`crate::service`'s framing seam).
pub const FLUSHED_CACHE_OK_STATUS: u16 = 200;

/// `204 No Content`, the status a tag WRITE is answered with: `TagResource`
/// (`POST /v2/tags/{ResourceArn}`), modelled `code: 201`. The answer has no members at all
/// — a caller reads the collection it wrote back through `GetTags` — so there is no created
/// entity for the answer to report, and `204` is what tells a caller the write was applied.
/// It is the status this model itself declares for the other write of the tag plane,
/// `UntagResource` (`smithy_query(query="operation", operation="UntagResource")`:
/// `code: 204`). The answer carries no content, and states the length of that nothing like
/// every other empty answer of this service (`crate::service`'s framing seam).
pub const TAG_WRITE_NO_CONTENT_STATUS: u16 = 204;

/// `202 Accepted`, the status a teardown that CONTINUES after the answer is answered with:
/// `DeleteDomainName` (`DELETE /v2/domainnames/{DomainName}`), modelled `code: 204`. The
/// domain's own record and the api mappings under it are gone when the call returns, while
/// the endpoints it published are still being torn down — which is why the certificate
/// references it held are released as DUE rather than as finished
/// ([`super::DOMAIN_NAME_DELETION`], the span that teardown takes) — and `202` is what tells
/// a caller that the work it asked for was accepted rather than already complete. It is the
/// status this model itself declares for the other teardown of a resource with endpoints to
/// withdraw, `DeleteVpcLink`
/// (`smithy_query(query="operation", operation="DeleteVpcLink")`: `code: 202`).
pub const ACCEPTED_TEARDOWN_STATUS: u16 = 202;
