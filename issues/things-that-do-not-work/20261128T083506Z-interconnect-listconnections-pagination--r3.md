Issue-Severity: MEDIUM
Issue-Gap-Class: pagination
Issue-Commit: b16feb5d898a7c21dbe5a600460e765efc5f70eb

# Page cursors carry the request's scope text VERBATIM, so an issued nextToken can grow past the modelled 1024-character NextToken bound and the next page becomes unfetchable

_Filed: 2026-11-28T08:35:05Z_
_Operation: ListConnections | Class: pagination_

> _Note: this issue was identified against the code base at commit `b16feb5d898a7c21dbe5a600460e765efc5f70eb`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

All three paginated listings of this crate mint their `nextToken` through `crate::state::encode_cursor`, which authenticates the payload `"{scope}\u{1f}{last_key}"` and renders it in hex — so the scope string travels INSIDE the token, verbatim. `connection_cursor_scope` builds that scope out of five pieces of request-derived text (the signing region, and the `state`, `environmentId`, `provider` and `attachPoint` filters), each embedded in full. The size of an issued token is therefore a function of the REQUEST, not of the service, and the token is 2 bytes of hex per payload byte on top.

`NextToken` is modelled `@length(min: 1, max: 1024)` on this operation's input, so once the scope text pushes the hex rendering past 1024 characters the token the listing just handed out is a value the service's own boundary validator refuses: the caller is given a cursor it can never send back, and the second page of that listing cannot be fetched at all.

The shared module this crate already depends on for its page-token key documents exactly this hazard and ships the remedy — `service_common::page_tokens::scope_digest`, a fixed-width (64-character) stand-in for the scope that "identifies the sequence just as exactly". The crate hand-rolls the verbatim-scope payload instead. The crate's own justification for embedding the fields verbatim is also untrue of one of them: `cursor_scope_field` claims the scope's members "are held to a `@length` in the model", but the region field comes from the SigV4 credential scope, which no model member bounds at all.

Note this is NOT the already-fixed "cursors are not scoped to the filters that decided the sequence" gap, and the fix must not reverse it: the scope must keep distinguishing every filtered sequence — it just has to travel as its digest rather than as the caller's text, which also leaves the existing hex token form (and the tests that mint through `encode_cursor`) untouched.

## Evidence

The rule, stated by the module this crate already imports — services/common/src/page_tokens.rs:126-150 (`scope_digest`):
  "A fixed-width stand-in for the SEQUENCE a cursor belongs to: the SHA-256 digest of `scope`, lowercase hexadecimal — the same 64 characters whatever the scope holds. … That identity is built from REQUEST text, and a model leaves some of it unbounded, so a scope travelling VERBATIM inside the token makes the token a service hands out grow with the request that asked for it, past the length the model declares for the member it travels on — and a page named by a token the service's own request walk refuses cannot be fetched at all.  Digesting the scope is what keeps the SIZE of an issued token a property of the service rather than of the request."

This crate already depends on that module — services/service-com-amazonaws-interconnect-interconnect/src/state.rs:45:
  `pub use ::service_common::page_tokens::PageTokenKey;`

What the crate does instead — state.rs:2249-2253:
```
pub fn encode_cursor(token_key: &PageTokenKey, scope: &str, last_key: &str) -> String {
    ::hex::encode(token_key.mint(&format!(
        "{scope}{CURSOR_FIELD_SEPARATOR}{last_key}"
    )))
}
```
and state.rs:2271 reads it back by stripping the verbatim scope again:
  `let key = payload.strip_prefix(&format!("{scope}{CURSOR_FIELD_SEPARATOR}"))?;`

The scope that travels in a ListConnections token — state.rs:2207-2215:
```
    let region = cursor_scope_field(region);
    let state = optional_cursor_scope_field(state);
    let environment_id = optional_cursor_scope_field(environment_id);
    let provider = provider_cursor_scope_field(provider);
    let attach_point = attach_point_cursor_scope_field(attach_point);
    format!(
        "{CONNECTION_CURSOR_TAG}{region}{CURSOR_FIELD_SEPARATOR}{state}…{attach_point}"
    )
```
and the handler hands it to both the cursor verdict and the page cut — src/ops/list_connections.rs:267-287 (`connection_cursor_scope(...)`, `judge_cursor(..., &scope)`, `paginate(matching, cursor.as_deref(), input.max_results, &token_key, &scope)`).

The bound the token has to satisfy to be usable again (Smithy model, ListConnectionsRequest):
  `nextToken` -> `NextToken`, `@length(min: 1, max: 1024)` — the same member the output hands back (OPERATIONS.md:97-98: "`nextToken` (string, len 1..1024)" on input and output).

The crate's own premise for embedding verbatim, contradicted by one of the fields it embeds — state.rs:2142-2146:
  "The members a scope is built from (a Region id, an `environmentId`, a partner name) are held to a `@length` in the model and to no `@pattern` at all…"
vs state.rs:612-615, where the region is whatever the request's credential scope spelled:
```
pub fn request_region(ctx: &smithy_server_runtime::OperationContext<'_>) -> String {
    ::emulator_server::sigv4_credential_scope(&ctx.parts().headers)
        .map_or_else(|| DEFAULT_REGION.to_string(), |scope| scope.region)
}
```
and vendor/server/src/lib.rs:1985-1992, where the only rule on that text is that it is not empty:
  `if access_key_id.is_empty() || date.is_empty() || region.is_empty() || service.is_empty() { return None; }` … `region: region.to_string(),`

Test coverage that the digest form leaves untouched: tests/all/page_cursors_are_only_what_the_listing_minted.rs builds every expected token through the crate's own `encode_cursor` (lines 376-389, 897, 1106, 1172, 1212, 1238, 1341) and asserts only that an issued cursor is hex-decodable (line 313: `let bytes = ::hex::decode(issued).expect("an issued cursor is rendered in hex");`), so carrying `scope_digest(scope)` in the payload keeps the token form and every one of those assertions.

## Steps to Reproduce

1. Sign requests for a long region id — the SigV4 credential scope's region field is arbitrary non-empty text (vendor/server/src/lib.rs:1975-1992), e.g. `Credential=AKID/20260101/<500 'a' characters>/interconnect/aws4_request`.
2. Call CreateConnection twice through that endpoint, so two connections belong to that region (state.rs:294 `visible_in`).
3. Call ListConnections with `maxResults: 1`: the page is cut and a `nextToken` is issued as `hex(32-byte authenticator + scope + separator + connection id)`, where the scope embeds the ~500-character region verbatim (state.rs:2207, 2249) — well over 1024 hex characters.
4. Send that `nextToken` back to ListConnections: the boundary validator refuses it against `NextToken`'s `@length(max: 1024)` (ValidationException, "Value at 'nextToken' failed to satisfy constraint: Member must have length less than or equal to 1024" per src/service/validation.rs:52-54), so the second page of a listing the service itself paged can never be fetched.
5. Compare with services/common/src/page_tokens.rs:147 `scope_digest`, which the crate's own dependency provides precisely so the issued token's size does not depend on the request text.
