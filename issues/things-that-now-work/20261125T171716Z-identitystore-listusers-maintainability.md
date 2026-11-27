Issue-Severity: LOW
Issue-Gap-Class: maintainability
Issue-Commit: f9465bc2186e307f05476a9cadaf16956fa09a02

# The NextToken resume/page/next-token computation is written out four times across the list handlers

_Filed: 2026-11-25T17:17:16Z_
_Operation: ListUsers | Class: maintainability_

> _Note: this issue was identified against the code base at commit `f9465bc2186e307f05476a9cadaf16956fa09a02`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

All four paginated handlers in this crate carry their own copy of the same three-step cursor computation: resolve `NextToken` through `crate::state::decode_cursor` and `position(...)` or refuse it, slice `items[start..].iter().take(page_size)`, and mint the next token from `items.get(start + page.len())` through `crate::state::encode_cursor`. Only the collection, the id accessor and the refusal sentence differ. The crate already centralises the token CODEC in `state.rs` (`encode_cursor`/`decode_cursor`), but not the walk that uses it, so the four copies can drift independently — the same way the `MaxResults` bound handling has already drifted (`ListUsers`/`ListGroups` refuse an out-of-range value, `ListGroupMemberships`/`ListGroupMembershipsForMember` `.clamp(1, MAX_MAX_RESULTS)` it).

A single helper in `state.rs` parametrised by the id accessor and the per-operation refusal message would leave every response byte-identical, since the only request-visible difference between the copies is that message text.

## Evidence

Copy 1 — services/service-com-amazonaws-identitystore-awsidentitystore/src/ops/list_users.rs:246-267:
```rust
let start = match input.next_token.as_deref() {
    None => 0,
    Some(token) => {
        let resume_at = crate::state::decode_cursor(token)
            .and_then(|user_id| users.iter().position(|user| user.user_id == user_id));
        resume_at.ok_or_else(|| ... validation("NextToken is not a cursor this service issued for this listing"))?
    }
};
let page: Vec<User> = users[start..].iter().take(page_size).map(...).collect();
let next_token = users.get(start + page.len()).map(|user| crate::state::encode_cursor(&user.user_id));
```
Copy 2 — src/ops/list_groups.rs:238-260 (same shape, `group.group_id`, "…issued for this listing").
Copy 3 — src/ops/list_group_memberships.rs:209-235 (same shape, `membership.membership_id`, "…issued for this group").
Copy 4 — src/ops/list_group_memberships_for_member.rs:232-258 (same shape, `membership.membership_id`, "…issued for this member").
The already-drifted neighbour: list_users.rs:231-240 / list_groups.rs:222-231 refuse an out-of-range `MaxResults`, while list_group_memberships.rs:200-203 and list_group_memberships_for_member.rs:223-226 write `.unwrap_or(crate::state::DEFAULT_MAX_RESULTS).clamp(1, crate::state::MAX_MAX_RESULTS)`.
Shared codec already factored out — src/state.rs:1378-1386 `pub fn encode_cursor` / `pub fn decode_cursor`.
Existing tests that pin the paging behaviour (and must stay unchanged): tests/all/list_users.rs (page walk, token shape at :118, bad-token refusal at :552), tests/all/list_groups.rs, tests/all/list_group_memberships_for_member.rs, tests/all/group_membership_lookup.rs, tests/all/identity_store_lifecycle.rs:1047.

## Steps to Reproduce

1. Read src/ops/list_users.rs:246-267, src/ops/list_groups.rs:238-260, src/ops/list_group_memberships.rs:209-235 and src/ops/list_group_memberships_for_member.rs:232-258 side by side: the three-step cursor computation is identical in all four, differing only in the id accessor and the refusal sentence.
2. Compare src/ops/list_users.rs:231-240 with src/ops/list_group_memberships.rs:200-203 to see the same `MaxResults` rule already implemented two different ways in these copies.
3. Confirm the codec half is already shared in src/state.rs:1378-1386, so only the walk is duplicated.
