Issue-Severity: LOW
Issue-Gap-Class: maintainability
Issue-Commit: eef1405ac0806ef267777bba6aefe2b3b0754197

# The single-Filter rule (supported attribute path, refusal, exact-value match) is written out twice across ListGroups and ListUsers

_Filed: 2026-11-27T12:45:41Z_
_Operation: ListGroups | Class: maintainability_

> _Note: this issue was identified against the code base at commit `eef1405ac0806ef267777bba6aefe2b3b0754197`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`ListGroups` and `ListUsers` each carry their own copy of one rule: take the first (and only, `Filters` is modelled `length 0..1`) filter, refuse the request with a `ValidationException` when its `AttributePath` is not the one attribute path that listing searches (compared case-insensitively), then keep only the records whose `attribute(path)` equals the filter's `AttributeValue` exactly. The two copies differ only in the per-operation constant (`USER_FILTER_ATTRIBUTE_PATH` / `GROUP_FILTER_ATTRIBUTE_PATH`), the operation name inside the message, and the record type being walked — exactly the shape of difference the crate already factored out for the paged walk (`state/pagination.rs:46 pub fn page_of`, parameterised by the records, the id accessor, the renderer and the refusal sentence, after the earlier "NextToken ... written out four times" issue). Because the rule lives in two places, the two listings can drift in how they validate a filter, and the identical refusal sentence can drift in wording, while nothing in the crate holds the rule once. A helper in `crate::state` parameterised by the operation name and the supported attribute path would leave every response byte-identical, since the only request-visible difference between the copies is the operation name inside the message.

## Evidence

Copy 1 — services/service-com-amazonaws-identitystore-awsidentitystore/src/ops/list_users.rs:195-223:
```rust
let filter = input.filters.as_deref().unwrap_or_default().first();
if let Some(filter) = filter {
    if !filter
        .attribute_path
        .eq_ignore_ascii_case(crate::state::USER_FILTER_ATTRIBUTE_PATH)
    {
        return Err(ListUsersErrorModeled::ValidationException(
            crate::state::validation(format!(
                "AttributePath {:?} is not an attribute ListUsers can filter on; \
                 the supported attribute path is {}",
                filter.attribute_path,
                crate::state::USER_FILTER_ATTRIBUTE_PATH
            )),
        )
        .into());
    }
}
...
    .filter(|user| {
        filter.is_none_or(|filter| {
            user.attribute(&filter.attribute_path) == Some(filter.attribute_value.as_str())
        })
    })
```
Copy 2 — src/ops/list_groups.rs:186-214:
```rust
let filter = input.filters.as_deref().unwrap_or_default().first();
if let Some(filter) = filter {
    if !filter
        .attribute_path
        .eq_ignore_ascii_case(crate::state::GROUP_FILTER_ATTRIBUTE_PATH)
    {
        return Err(ListGroupsErrorModeled::ValidationException(
            crate::state::validation(format!(
                "AttributePath {:?} is not an attribute ListGroups can filter on; \
                 the supported attribute path is {}",
                filter.attribute_path,
                crate::state::GROUP_FILTER_ATTRIBUTE_PATH
            )),
        )
        .into());
    }
}
...
    .filter(|group| {
        filter.is_none_or(|filter| {
            group.attribute(&filter.attribute_path) == Some(filter.attribute_value.as_str())
        })
    })
```
The precedent for the shared form, in this same crate — src/state/pagination.rs:46-53:
```rust
pub fn page_of<Record, Item>(
    records: &[Record],
    max_results: Option<i32>,
    next_token: Option<&str>,
    unresumable_token_message: &str,
    id_of: impl Fn(&Record) -> &str,
    render: impl FnMut(&Record) -> Item,
) -> Result<Page<Item>, ValidationException> {
```
Existing tests that pin this behaviour and must stay unchanged: tests/all/list_users.rs:341 `a_filter_narrows_the_listing_to_one_user_name`, tests/all/list_users.rs:411 `an_attribute_path_other_than_user_name_is_refused`, tests/all/list_groups.rs:422 `a_filter_selects_by_display_name`.

## Steps to Reproduce

1. Read src/ops/list_users.rs:195-223 and src/ops/list_groups.rs:186-214 side by side: the same three-step filter rule (first filter, case-insensitive supported-path check with the same refusal sentence, exact-value match) appears in both, differing only in the constant, the operation name in the message and the record type.
2. Note that the crate already holds the analogous per-listing rule once in src/state/pagination.rs:46 (`page_of`), parameterised by the per-operation differences.
3. Confirm the responses the two copies produce are identical in shape, so hoisting the rule into `crate::state` changes no response and no existing test (tests/all/list_users.rs:341, :411 and tests/all/list_groups.rs:422).
