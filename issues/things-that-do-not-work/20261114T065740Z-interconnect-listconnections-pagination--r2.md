Issue-Severity: MEDIUM
Issue-Gap-Class: pagination
Issue-Commit: 7722f7a491f17adc9c16c0d1d844ff39f4bc9907

# ListConnections page cursors are not scoped to the filters that decided the sequence, so a token from one filtered listing resumes another

_Filed: 2026-11-14T06:57:40Z_
_Operation: ListConnections | Class: pagination_

> _Note: this issue was identified against the code base at commit `7722f7a491f17adc9c16c0d1d844ff39f4bc9907`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`ListConnections` authenticates its `nextToken` under the scope `conn:<region>` only (`crate::state::connection_cursor_scope`), which names the operation and the Region and NOTHING about the four filters (`state`, `environmentId`, `provider`, `attachPoint`) that decided the sequence the cursor names a position in. The sibling paginated listing in the same crate does the opposite for exactly this reason: `ListAttachPoints` folds its scoping member into the authenticated scope (`attach_point_cursor_scope(&region, &input.environment_id)`) "because those three decide the sequence a cursor names a position in", and the crate's own cursor suite demands that a cursor minted by a listing of ANOTHER `environmentId` be refused with HTTP 400.

The consequence is the outcome the ListConnections handler's own comment says must not happen. A cursor issued by `ListConnections(filter = F1)` is a cursor for the F1 sequence; passed back with `filter = F2` it verifies, so `crate::state::paginate` resumes the F2 sequence at "the first key strictly greater than" the F1 position and the call answers HTTP 200. Every connection matching F2 whose id sorts at or below that key is silently dropped from the walk, and when they are all below it the caller gets an empty page with no `nextToken` — which the same comment says a caller "cannot tell from 'no more results'". So one and the same member value (`environmentId`) is a cursor-scoping input for `ListAttachPoints` and is ignored by the cursor of `ListConnections`, and the connection listing silently advances a walk it never performed instead of refusing a token it did not issue for that sequence.

## Evidence

The sibling listing binds the member that scopes its sequence into the authenticated cursor scope, and states the rule — services/service-com-amazonaws-interconnect-interconnect/src/ops/list_attach_points.rs:184-192:
```
        // A cursor this operation did not issue is refused, rather than answered
        // with an empty page the caller cannot tell from "no more results" — or with a
        // silently restarted walk. ... The scope the
        // authenticator covers names this operation, the Region it answered in and the
        // Environment the listing was scoped by, because those three decide the sequence
        // a cursor names a position in.
        let scope = crate::state::attach_point_cursor_scope(&region, &input.environment_id);
```
and src/state.rs:1900-1906:
```
/// The scope a `ListAttachPoints` cursor is issued under: this operation's tag, the
/// regional endpoint that served the page, and the Environment the listing was scoped
/// by — the attach points of one Environment are the sequence it walked, so a cursor
/// from a listing of another Environment is not one this listing issued.
pub fn attach_point_cursor_scope(region: &str, environment_id: &str) -> String {
    format!("{ATTACH_POINT_CURSOR_TAG}{region}{CURSOR_FIELD_SEPARATOR}{environment_id}")
}
```

The connection listing skips that: its filters decide the sequence (src/ops/list_connections.rs:218-238 filters on `state`, `environment_id`, `provider`, `attach_point`), yet the cursor scope carries none of them — src/ops/list_connections.rs:245-251:
```
        // A cursor this operation did not issue is refused, rather than answered
        // with an empty page the caller cannot tell from "no more results" — or with a
        // silently restarted walk. The position a cursor names travels INSIDE the token,
        // ...
        let scope = crate::state::connection_cursor_scope(&region);
```
and src/state.rs:1884-1892:
```
/// The scope a `ListConnections` cursor is issued under: this operation's tag and the
/// regional endpoint that served the page.
///
/// A cursor names the last key of ONE walked sequence, and the sequence this listing
/// walks is the connections of the Region the request was signed for, ...
pub fn connection_cursor_scope(region: &str) -> String {
    format!("{CONNECTION_CURSOR_TAG}{region}")
}
```

What the accepted token then does — src/state.rs:1972-1978 resumes after the key in whatever sequence the new filters produced:
```
    let start = match cursor {
        Some(key_after) => items
            .iter()
            .position(|(key, _)| key.as_str() > key_after)
            .unwrap_or(items.len()),
        None => 0,
    };
```

Test coverage proves the crate treats the cross-scope case as a refusal for the sibling and never exercises it for the connection listing — tests/all/page_cursors_are_only_what_the_listing_minted.rs:256-259 lists, among the tokens that MUST be refused for `ListAttachPoints`:
```
                (
                    "a listing of an Environment this Region does not hold".to_string(),
                    attach_point_cursor_scope(region, &other_environment),
                ),
```
with the assertion at line 438-441:
```
            assert_eq!(
                status, 400,
                "case {label:?}: expected HTTP 400 for a cursor this listing did not issue, got {status} with {body:?}"
            );
```
The `ListConnections` row of that same table (lines 202-214) lists only another Region's scope and the two sibling operations' scopes as foreign — no row varies a FILTER, so nothing covers a cursor reused across `state` / `environmentId` / `provider` / `attachPoint` values.

## Steps to Reproduce

1. Boot the emulator, point an Interconnect client at one region, and create a Direct Connect gateway to attach to.
2. CreateConnection twice on the seeded environment and wait ~700 ms so both connections reach 'available'; note the two ids (call them X and Y).
3. CreateConnection a third time and do NOT wait: it reports state 'requested' (id Z).
4. Call ListConnections(state="available", maxResults=1). It returns one connection and a nextToken T; T is minted over the scope "conn:<region>" plus the id of the connection just returned (src/state.rs:1890).
5. Call ListConnections(state="requested", nextToken=T) — a cursor issued for the 'available' sequence, replayed on the 'requested' sequence. Observed: HTTP 200, and the page is the 'requested' sequence resumed after that id, so Z is reported only when its id happens to sort above it and is silently omitted otherwise (empty page, no nextToken, indistinguishable from "no more results").
6. Repeat with the environmentId filter: ListConnections(environmentId=<seeded id>, maxResults=1) then ListConnections(environmentId="mce-aws-gcp-xxx", nextToken=T) — also HTTP 200.
7. Compare the sibling: ListAttachPoints(environmentId=A) mints a cursor that ListAttachPoints(environmentId=B) refuses with HTTP 400 InterconnectValidationException (tests/all/page_cursors_are_only_what_the_listing_minted.rs:256-259), because its scope names the member that decided the sequence.
