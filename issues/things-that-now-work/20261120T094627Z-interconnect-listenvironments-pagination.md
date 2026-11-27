Issue-Severity: LOW
Issue-Gap-Class: pagination
Issue-Commit: 657b97543b8e608c75aab9edda765f25072992b5

# ListEnvironments page cursors are not scoped to the provider filter that decided the sequence, unlike the sibling listing's

_Filed: 2026-11-20T09:46:27Z_
_Operation: ListEnvironments | Class: pagination_

> _Note: this issue was identified against the code base at commit `657b97543b8e608c75aab9edda765f25072992b5`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Resolution

The code base had drifted by the time this was picked up: the scope a `ListEnvironments`
cursor is authenticated under already names the operation, the Region whose endpoint
answered AND the `provider` the catalog was narrowed by
(`state::environment_cursor_scope`, applied in `ops/list_environments.rs`), rendering the
union member beside the name so two listings share a scope exactly when the filter selects
the same records for both — the rule the sibling listings follow. No behaviour change was
needed; the rule is now pinned by
`tests/all/page_cursors_are_only_what_the_listing_minted.rs`'s
`an_environment_cursor_is_served_only_by_the_provider_that_minted_it`, which drives every
pairing of two Regions with six provider selections and demands HTTP 400 for a cursor of
any other selection, beside the window and `location` inputs that scope no cursor.

## Description

`ListEnvironments` authenticates its `nextToken` under the scope `env:<region>` only (`crate::state::environment_cursor_scope`), which names the operation and the Region and NOTHING about the `provider` filter that narrows the sequence the cursor names a position in. The handler itself does narrow on `provider` two statements earlier, and its own comment asserts that `provider` "narrows the catalog exactly" — yet the comment that chooses the cursor scope claims the sequence is decided by Region alone ("the sequence a cursor names a position in is this Region's own catalog"). Those two statements in the same function cannot both be true.

The third paginated listing in the same crate does the opposite for exactly this reason: `ListAttachPoints` folds the member that scopes its sequence into the authenticated scope (`attach_point_cursor_scope(&region, &input.environment_id)`), "because those three decide the sequence a cursor names a position in", and the crate's cursor suite demands that a cursor minted by a listing of another `environmentId` be refused with HTTP 400. `ListEnvironments` is the sibling that skips that.

This is a latent defect with no behavioral impact against the catalog the crate publishes today: `crate::state::environment_catalog` returns exactly one `Environment` per Region, and `crate::state::paginate` mints a token only when an item remains after the page, so `ListEnvironments` currently never issues a `nextToken` at all and no caller can hold one to replay across filters. The scoping rule is nevertheless written wrong here relative to its two siblings: the moment the catalog publishes more than one offering for a Region, a cursor issued by `ListEnvironments(provider = P)` verifies when replayed with `provider = Q` (or with no provider), and `paginate` resumes the Q sequence at "the first key strictly greater than" the P position — silently dropping every matching Environment whose id sorts at or below it, and answering HTTP 200 with a page (possibly empty, with no `nextToken`) the caller cannot tell from "no more results", which is precisely the outcome the handler's own comment says must not happen.

Note: this is the same rule the open issue 20261114T065740Z-interconnect-listconnections-pagination--r2 reports for `ListConnections`; that issue names only `ListConnections` and quotes only `connection_cursor_scope`, so the `ListEnvironments` half is a separate (operation, class) pair and is not covered by it.

## Evidence

The handler narrows the sequence by `provider`, and says so — services/service-com-amazonaws-interconnect-interconnect/src/ops/list_environments.rs:182-185, 203-211:
```
        // `provider` narrows the catalog exactly: the member filters results "to only
        // include Environment objects that connect to the Provider", and an Environment
        // names one partner on one side of that union, so an Environment is listed only
        // when it names the same partner on the same side.
...
        let matching: Vec<(String, Environment)> = crate::state::environment_catalog(&region)
            .into_iter()
            .filter(|(_, environment)| {
                input
                    .provider
                    .as_ref()
                    .is_none_or(|wanted| crate::state::provider_eq(&environment.provider, wanted))
            })
            .collect();
```

The cursor scope in the SAME function then claims the Region alone decides that sequence, and carries no filter — src/ops/list_environments.rs:213-221:
```
        // A cursor this operation did not issue is refused, rather than answered
        // with an empty page the caller cannot tell from "no more results" — or with a
        // silently restarted walk. ... The scope the
        // authenticator covers names this operation AND the Region it answered in,
        // because the sequence a cursor names a position in is this Region's own
        // catalog: a cursor another Region's endpoint issued is not one this one did.
        let scope = crate::state::environment_cursor_scope(&region);
```
and src/state.rs:2009-2013:
```
/// The scope a `ListEnvironments` cursor is issued under: this operation's tag and the
/// regional endpoint that served the page, whose catalog is the sequence it walked.
pub fn environment_cursor_scope(region: &str) -> String {
    format!("{ENVIRONMENT_CURSOR_TAG}{region}")
}
```

The sibling listing in this crate binds the member that scopes its sequence into the scope, and states the rule — src/ops/list_attach_points.rs:166-172:
```
        // ... The scope the
        // authenticator covers names this operation, the Region it answered in and the
        // Environment the listing was scoped by, because those three decide the sequence
        // a cursor names a position in — and all three are read off the REQUEST, so this
        // verdict needs nothing the request names to have been resolved first.
        let scope = crate::state::attach_point_cursor_scope(&region, &input.environment_id);
```
and src/state.rs:2015-2021:
```
/// The scope a `ListAttachPoints` cursor is issued under: this operation's tag, the
/// regional endpoint that served the page, and the Environment the listing was scoped
/// by — the attach points of one Environment are the sequence it walked, so a cursor
/// from a listing of another Environment is not one this listing issued.
pub fn attach_point_cursor_scope(region: &str, environment_id: &str) -> String {
    format!("{ATTACH_POINT_CURSOR_TAG}{region}{CURSOR_FIELD_SEPARATOR}{environment_id}")
}
```

What an accepted cross-filter token then does — src/state.rs:2087-2093 resumes after the key in whatever sequence the new filters produced:
```
    let start = match cursor {
        Some(key_after) => items
            .iter()
            .position(|(key, _)| key.as_str() > key_after)
            .unwrap_or(items.len()),
        None => 0,
    };
```

Test coverage: the crate's cursor suite builds a `ListEnvironments` row whose `foreign_scopes` vary only the Region and the two sibling operations — tests/all/page_cursors_are_only_what_the_listing_minted.rs:219-242 — while the `ListAttachPoints` row beside it adds the scoping-member row "a listing of an Environment this Region does not hold" (lines 256-259) and asserts HTTP 400 for it. No row of that table varies the `provider` filter for `ListEnvironments`, so nothing covers a cursor reused across provider values.

Why this is latent today: src/state.rs:2251-2279 (`environment_catalog`) builds `let catalog = vec![Environment { ... }]` — a single entry per Region — and src/state.rs:2108-2112 mints a token only `if rest.next().is_some()`, so `ListEnvironments` never hands one out.

## Steps to Reproduce

1. Read services/service-com-amazonaws-interconnect-interconnect/src/ops/list_environments.rs:203-211 and confirm the sequence handed to `crate::state::paginate` is narrowed by `input.provider`.
2. Read the next statement, src/ops/list_environments.rs:213-221, and confirm the cursor scope is `crate::state::environment_cursor_scope(&region)` — region only — under a comment asserting the sequence is "this Region's own catalog".
3. Compare src/ops/list_attach_points.rs:166-172, where the sibling listing folds its scoping member (`input.environment_id`) into the scope "because those three decide the sequence a cursor names a position in".
4. Confirm the gap is latent against today's catalog: src/state.rs:2251-2279 publishes exactly one Environment per Region, and src/state.rs:2108-2112 mints a `nextToken` only when an item remains after the page, so `ListEnvironments` issues none. Boot the emulator and call ListEnvironments(maxResults=1) — the response carries no `nextToken`.
5. To see the behaviour the scope must refuse, add a second entry to `environment_catalog` (or mint a cursor with the store's `page_token_key` under `environment_cursor_scope(region)`, the way tests/all/page_cursors_are_only_what_the_listing_minted.rs:288-296 does), then call ListEnvironments(provider=<partner A>, maxResults=1) to obtain token T and replay it as ListEnvironments(provider=<partner B>, nextToken=T). Observed: HTTP 200 with the B sequence resumed after the A position, instead of the HTTP 400 InterconnectValidationException the sibling answers for a cursor of a sequence it never walked.
