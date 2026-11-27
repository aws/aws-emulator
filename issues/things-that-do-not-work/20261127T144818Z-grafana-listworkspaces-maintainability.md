Issue-Severity: LOW
Issue-Gap-Class: maintainability
Issue-Commit: a648c95e8281c95edbf892975ca6b5e5d5c3850b

# The maxResults page-size rule is implemented four times across the list handlers

_Filed: 2026-11-27T14:48:18Z_
_Operation: ListWorkspaces | Class: maintainability_

> _Note: this issue was identified against the code base at commit `a648c95e8281c95edbf892975ca6b5e5d5c3850b`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

The same page-size resolution rule — "an absent `maxResults` means the largest page the model allows; a value reaching the handler outside `1..=limit` is a validator bug, asserted in debug and clamped" — is written out verbatim four times, once per list handler, differing only in which per-operation limit constant it names. Four copies of one computation can drift: a change to the rule (for example dropping the clamp, or changing what an absent value means) applied to one handler and not the others would leave the four listings disagreeing while every copy still compiles.

The crate already keeps shared rules in `crate::state` (`out_of_bounds_tag`, `workspace_in_flight`, `decimal_id_order`, `service_accounts_unsupported`), so the fix is one helper there taking the request's `max_results` and the operation's limit and returning the `usize` page size. Every response and every existing test stays byte-identical: the computation is unchanged, only its single home moves.

## Evidence

Copy 1 — src/ops/list_workspaces.rs:178-192:
```
        // `maxResults` is modelled `@range(min: 1, max: 100)` and the boundary
        // validator refused anything outside it before this handler ran, so the
        // only case left is an absent value: serve the largest page the model
        // allows. ...
        let requested = input
            .max_results
            .unwrap_or(crate::state::WORKSPACE_PAGE_LIMIT);
        debug_assert!(
            (1..=crate::state::WORKSPACE_PAGE_LIMIT).contains(&requested),
            "maxResults {requested} should have been refused at the boundary"
        );
        let page_size = usize::try_from(requested.clamp(1, crate::state::WORKSPACE_PAGE_LIMIT))
            .expect("a page size clamped into 1..=100 fits in usize");
```

Copy 2 — src/ops/list_workspace_service_accounts.rs:194-203:
```
        let requested = input
            .max_results
            .unwrap_or(crate::state::SERVICE_ACCOUNT_PAGE_LIMIT);
        debug_assert!(
            (1..=crate::state::SERVICE_ACCOUNT_PAGE_LIMIT).contains(&requested),
            "maxResults {requested} should have been refused at the boundary"
        );
        let page_size =
            usize::try_from(requested.clamp(1, crate::state::SERVICE_ACCOUNT_PAGE_LIMIT))
                .expect("a page size clamped into 1..=100 fits in usize");
```

Copy 3 — src/ops/list_versions.rs:210-216:
```
        let requested = input.max_results.unwrap_or(crate::state::VERSION_PAGE_LIMIT);
        debug_assert!(
            (1..=crate::state::VERSION_PAGE_LIMIT).contains(&requested),
            "maxResults {requested} should have been refused at the boundary"
        );
        let page_size = usize::try_from(requested.clamp(1, crate::state::VERSION_PAGE_LIMIT))
            .expect("a page size clamped into 1..=100 fits in usize");
```

Copy 4 — src/ops/list_workspace_service_account_tokens.rs:197-206:
```
        let requested = input
            .max_results
            .unwrap_or(crate::state::SERVICE_ACCOUNT_TOKEN_PAGE_LIMIT);
        debug_assert!(
            (1..=crate::state::SERVICE_ACCOUNT_TOKEN_PAGE_LIMIT).contains(&requested),
            "maxResults {requested} should have been refused at the boundary"
        );
        let page_size =
            usize::try_from(requested.clamp(1, crate::state::SERVICE_ACCOUNT_TOKEN_PAGE_LIMIT))
                .expect("a page size clamped into 1..=100 fits in usize");
```

Tests that already cover the four copies, so a single-home refactor must leave them green: tests/all/workspace_lifecycle.rs:470 `list_workspaces_pages_over_one_stable_order`, tests/all/list_workspace_service_accounts.rs:305 `pages_bounded_by_max_results_cover_every_account_once` and :373 `a_max_results_outside_the_published_range_is_refused`, tests/all/list_versions.rs:280 `list_versions_pages_over_one_stable_order`, tests/all/service_account_tokens.rs:414 `the_read_pages_over_the_stored_tokens`.

## Steps to Reproduce

1. Open src/ops/list_workspaces.rs and read lines 178-192.
2. Open src/ops/list_workspace_service_accounts.rs lines 188-203, src/ops/list_versions.rs lines 204-216 and src/ops/list_workspace_service_account_tokens.rs lines 191-206.
3. Observe the identical unwrap_or / debug_assert / clamp / usize::try_from computation in all four, differing only in the limit constant named.
4. Note that `crate::state` already hosts this crate's shared rules (out_of_bounds_tag, workspace_in_flight, decimal_id_order), so one helper there would hold the rule once with no response or test change.
