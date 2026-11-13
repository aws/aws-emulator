Issue-Severity: MEDIUM
Issue-Gap-Class: pagination
Issue-Commit: 274c242a07c2045a47799c7436640bb31a4ba660

# A forged nextToken carrying the operation's own cursor tag is accepted, so ListConnections/ListAttachPoints answer a silently empty or restarted page instead of the refusal the crate says it gives

_Filed: 2026-11-08T10:41:06Z_
_Operation: ListConnections | Class: pagination_

> _Note: this issue was identified against the code base at commit `274c242a07c2045a47799c7436640bb31a4ba660`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`crate::state::decode_cursor` is the crate's single gate on a pagination cursor, and its doc states that anything this operation did not issue — "a hand-made token" included — must come back `None` so the handler can refuse it. The implementation only strips the cursor TAG and requires a non-empty remainder: `token.strip_prefix(tag).filter(|key| !key.is_empty())`. The tag is not a secret — `paginate` builds every issued token as `format!("{token_tag}{key}")` from the public constants `CONNECTION_CURSOR_TAG = "conn:"` / `ATTACH_POINT_CURSOR_TAG = "ap:"` — so any caller-invented string that merely starts with that prefix is treated as a cursor this endpoint issued, and the key behind it is used as the "last item handed out" with no check that the endpoint ever handed it out.

Two consequences, both the answer the handlers' own comments say must never be given:

* `ListConnections(nextToken="conn:zzzzzzzz")` — no stored id is lexicographically greater than `zzzzzzzz` (ids are `mcc-…`/`lmcc-…`), so `paginate` starts past the end and returns HTTP 200 with an empty `connections` and no `nextToken`: exactly "an empty page the caller cannot tell from 'no more results'", which list_connections.rs:241-242 says is what the refusal exists to avoid.
* `ListConnections(nextToken="conn:aaaaaaaa")` — every id is greater than `aaaaaaaa`, so the walk silently RESTARTS at the first page, and a client resuming from a stale/forged cursor re-reads records it already saw instead of being told the cursor is not one this operation issued.
* `ListAttachPoints(nextToken="ap:zzzz")` — same gate, same helper: the required `attachPoints` list comes back empty, which the crate itself calls a positive claim it must never publish without knowing it is true ("`attachPoints` is `@required`, so an empty — or silently short — page asserts that those are all the attach points the account has, which is a claim nothing is known to support", list_attach_points.rs:207-210).

The sibling list operation in the same crate does guard the key: `ListEnvironments` filters the decoded key through `environment_in_region(&region, key).is_some()` and refuses the token when it names nothing this Region's catalog offers, with the comment spelling out exactly the reasoning the other two skip ("this endpoint only ever handed out ids of its OWN Region's catalog, so one naming an Environment this Region does not offer is not a cursor it issued either"). So the rule is implemented for one list operation and skipped by the other two, which share the same helper.

## Evidence

The gate, and the rule it states it enforces — services/service-com-amazonaws-interconnect-interconnect/src/state.rs:1814-1821:
```
/// Read the key out of a page cursor this operation issued.
///
/// `None` for anything else — a hand-made token, or one from the other list
/// operation — which the handler refuses with the modelled validation error rather
/// than silently answering with an empty page.
pub fn decode_cursor<'a>(token: &'a str, tag: &str) -> Option<&'a str> {
    token.strip_prefix(tag).filter(|key| !key.is_empty())
}
```
The tag is public and appears in every token the service hands out — src/state.rs:1807-1812 and src/state.rs:1859-1863:
```
/// The tag `ListConnections` stamps on the cursors it issues.
pub const CONNECTION_CURSOR_TAG: &str = "conn:";
/// The tag `ListAttachPoints` stamps on the cursors it issues.
pub const ATTACH_POINT_CURSOR_TAG: &str = "ap:";
```
```
    let token = if rest.next().is_some() {
        last_key.map(|key| format!("{token_tag}{key}"))
```
and the cursor is then used as a bare "start after this key" with no existence check — src/state.rs:1838-1844:
```
    let start = match cursor {
        Some(key_after) => items
            .iter()
            .position(|(key, _)| key.as_str() > key_after)
            .unwrap_or(items.len()),
        None => 0,
    };
```
The handler that believes the gate — src/ops/list_connections.rs:241-259:
```
        // A cursor this operation did not issue is refused, rather than answered
        // with an empty page the caller cannot tell from "no more results".
        let cursor = match input.next_token.as_deref() {
            Some(token) => {
                let Some(key) =
                    crate::state::decode_cursor(token, crate::state::CONNECTION_CURSOR_TAG)
                else {
                    return Err(ListConnectionsErrorModeled::InterconnectValidationException(
                        InterconnectValidationException {
                            message: "nextToken is not a pagination token issued by ListConnections"
```
The sibling that DOES validate the key, in the same crate — src/ops/list_environments.rs:201-218:
```
        // A cursor this operation did not issue is refused, rather than answered
        // with an empty page the caller cannot tell from "no more results". A cursor
        // carries the last `environmentId` handed out, and this endpoint only ever
        // handed out ids of its OWN Region's catalog, so one naming an Environment
        // this Region does not offer is not a cursor it issued either.
        let cursor = match input.next_token.as_deref() {
            Some(token) => {
                let key = crate::state::decode_cursor(token, crate::state::ENVIRONMENT_CURSOR_TAG)
                    .filter(|key| crate::state::environment_in_region(&region, key).is_some());
```
The same "never an empty page" rule stated for the other affected operation — src/ops/list_attach_points.rs:206-214:
```
        // An account whose gateways this emulator could not read to the end is a
        // DIFFERENT answer from one that holds none: `attachPoints` is `@required`, so
        // an empty — or silently short — page asserts that those are all the attach
        // points the account has, which is a claim nothing is known to support.
```
Test coverage: every existing cursor-refusal row uses a token WITHOUT the operation's own tag, which is why the gap survives — tests/all/connection_lifecycle.rs:1036-1045 (`for token in ["not-a-cursor", "mcc-00000000", "env:env-csp-azure-us-east-1"]`, with the comment "Same cursor rule here: a token ListConnections did not issue is refused."), tests/all/list_connections_environment_filter.rs:460-464 (`json!({ "nextToken": "not-a-cursor" })`), tests/all/attach_points.rs:299-308 (`"env:env-csp-azure-us-east-1", "conn:mcc-00000000", "ap:"` — the one tagged row has an EMPTY key, which the `!key.is_empty()` filter already catches), tests/all/attach_point_listing_needs_a_resolved_account.rs:241-246 (`Some("not-a-token-this-service-minted")`). No row sends `"conn:<forged key>"` or `"ap:<forged key>"`.

## Steps to Reproduce

1. Boot the emulator and point an Interconnect client at it; create at least two connections in one Region so ListConnections has several pages.
2. Call ListConnections(maxResults=1) and note the issued token, e.g. "conn:mcc-ab12cd34" — the tag "conn:" is now known to the caller.
3. Call ListConnections(nextToken="conn:zzzzzzzz") — a token this endpoint never issued. Observe HTTP 200 with an empty `connections` array and no `nextToken`, indistinguishable from "no more results", instead of the 400 InterconnectValidationException "nextToken is not a pagination token issued by ListConnections" the handler comment promises.
4. Call ListConnections(nextToken="conn:aaaaaaaa"). Observe HTTP 200 returning the FIRST page again: the paged walk silently restarts on a forged cursor.
5. Contrast with the sibling: ListEnvironments(nextToken="env:not-an-environment-id") is refused with 400 InterconnectValidationException, because that handler also checks the decoded key names an Environment of the called Region.
6. Call ListAttachPoints(environmentId=<an id from ListEnvironments>, nextToken="ap:zzzz"). Observe HTTP 200 with an empty required `attachPoints` list — the "the account holds none" assertion the handler's own comment says must never be published on a claim nothing supports.
