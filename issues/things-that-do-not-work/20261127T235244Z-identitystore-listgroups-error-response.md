Issue-Severity: MEDIUM
Issue-Gap-Class: error-response
Issue-Commit: 7e7f1782eb72d186b4a02ad6793c94a317471370

# ListGroups refuses an unsearchable Filters AttributePath only after the identity store is resolved, so a request invalid on its own face answers 404 instead of 400

_Filed: 2026-11-27T23:52:44Z_
_Operation: ListGroups | Class: error-response_

> _Note: this issue was identified against the code base at commit `7e7f1782eb72d186b4a02ad6793c94a317471370`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Same defect as on the sibling listing: `ListGroups`'s only request-only validation — that `Filters[0].AttributePath` names `DisplayName`, the one attribute this listing searches — runs AFTER `addressed_store_key` has refused an `IdentityStoreId` the account does not have. A request that is both invalid on its face and addressed to a store this emulator does not serve is therefore answered with the store's `ResourceNotFoundException`, and the `ValidationException` the handler builds for the path is unreachable in that case.

The crate states the opposite rule for request-only refusals in four sibling handlers, which resolve them from the request alone before any store is touched.

## Evidence

Store first — `services/service-com-amazonaws-identitystore-awsidentitystore/src/ops/list_groups.rs:170-176`:
```rust
        let store_key =
            crate::state::addressed_store_key(&*self.engine, &input.identity_store_id)
                .map_err(ListGroupsErrorModeled::ResourceNotFoundException)?;
        // `@readonly`: the store is RESOLVED and nothing is written, so listing groups
        // can never change what a later caller sees.
        let state = state.lock();
        let store = state.resolve_store(&store_key);
```
filter second — `list_groups.rs:187-201`:
```rust
        if let Some(filter) = filter {
            if !filter
                .attribute_path
                .eq_ignore_ascii_case(crate::state::GROUP_FILTER_ATTRIBUTE_PATH)
            {
                return Err(ListGroupsErrorModeled::ValidationException(
                    crate::state::validation(format!(
                        "AttributePath {:?} is not an attribute ListGroups can filter on; \
```

The sibling rule it contradicts — `src/ops/get_group_id.rs:169-172`:
```rust
        // Which group the alternate identifier names. Resolved BEFORE the store is
        // reached, so an identifier this operation cannot interpret is refused as a bad
        // request whether or not the named store exists.
        let lookup = GroupLookup::from_identifier(&input.alternate_identifier)?;
```
and `src/ops/create_group.rs:175-178`:
```rust
        // `DisplayName` NAMES the group, and a name this API reserves cannot name one:
        // a create carrying a reserved name is refused with the modelled
        // `ValidationException` and stores nothing. Asked of the REQUEST alone, so it is
        // answered before any store is resolved or locked.
```

Test coverage: the `ListGroups` filter suite in `tests/all/list_groups.rs` exercises unsupported attribute paths only against the valid `STORE`, so the combination is untested and the current ordering is not pinned.

## Steps to Reproduce

1. Call ListGroups with `{"IdentityStoreId": "<a well-formed IdentityStoreId IAM Identity Center does not report>", "Filters": [{"AttributePath": "UserName", "AttributeValue": "x"}]}`
2. Observe `ResourceNotFoundException` (ResourceType IDENTITY_STORE) instead of the `ValidationException` list_groups.rs:192 builds for the unsearchable attribute path
3. Send the same `Filters` with the valid store id and observe the `ValidationException` the handler does raise
4. Compare with GetGroupId, which per get_group_id.rs:169-172 answers the 400 first for the same class of request-only defect
