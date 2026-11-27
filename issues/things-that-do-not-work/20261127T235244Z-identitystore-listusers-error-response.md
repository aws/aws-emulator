Issue-Severity: MEDIUM
Issue-Gap-Class: error-response
Issue-Commit: 7e7f1782eb72d186b4a02ad6793c94a317471370

# ListUsers refuses an unsearchable Filters AttributePath only after the identity store is resolved, so a request invalid on its own face answers 404 instead of 400

_Filed: 2026-11-27T23:52:44Z_
_Operation: ListUsers | Class: error-response_

> _Note: this issue was identified against the code base at commit `7e7f1782eb72d186b4a02ad6793c94a317471370`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`ListUsers`'s only request-only validation — that `Filters[0].AttributePath` names the one attribute this listing searches — is performed AFTER `addressed_store_key` has already refused an `IdentityStoreId` the account does not have. So a single request that is BOTH invalid on its face (an unsearchable attribute path) and addressed to a store this emulator does not serve is answered with `ResourceNotFoundException` for the store, never with the `ValidationException` the handler itself builds for the path.

Every sibling handler in this crate that owns a request-only refusal states the opposite rule in a comment and implements it: the request-only check runs BEFORE any store is resolved or locked, so the 400 is answered "whether or not the identity store exists". `ListUsers` is the one place that rule is inverted, and the crate's own suite never covers the combination (the filter suite always uses the valid `STORE`, and the store suite never sends a filter), so nothing pins the current order.

## Evidence

The handler resolves the store first and validates the filter second — `services/service-com-amazonaws-identitystore-awsidentitystore/src/ops/list_users.rs:179-185`:
```rust
        let store_key =
            crate::state::addressed_store_key(&*self.engine, &input.identity_store_id)
                .map_err(ListUsersErrorModeled::ResourceNotFoundException)?;
        // `@readonly`: the store is RESOLVED and nothing is written, so listing users
        // can never change what a later caller sees.
        let state = state.lock();
        let store = state.resolve_store(&store_key);
```
and only then, at `list_users.rs:196-210`:
```rust
        if let Some(filter) = filter {
            if !filter
                .attribute_path
                .eq_ignore_ascii_case(crate::state::USER_FILTER_ATTRIBUTE_PATH)
            {
                return Err(ListUsersErrorModeled::ValidationException(
                    crate::state::validation(format!(
                        "AttributePath {:?} is not an attribute ListUsers can filter on; \
```

The rule the crate's siblings state for exactly this kind of refusal — `src/ops/get_user_id.rs:165-168`:
```rust
        // The identifier is understood BEFORE any state is touched: a path this service
        // resolves no user by is a syntax error in the request, which is a 400 whether
        // or not the identity store exists.
        let selector = UserSelector::from_identifier(&input.alternate_identifier)?;
```
and `src/ops/create_user.rs:219-222`:
```rust
        // `UserName` NAMES the user, and a name this API reserves cannot name one: a
        // create carrying a reserved name is refused with the modelled
        // `ValidationException` and stores nothing. Asked of the REQUEST alone, so it is
        // answered before any store is resolved or locked.
```
and `src/ops/update_group.rs:316-320`:
```rust
        // Resolved first, from the request alone: an operation this service cannot
        // apply refuses the whole request, so nothing below it is ever half-written.
```

Test coverage: `tests/all/list_users.rs:411` `an_attribute_path_other_than_user_name_is_refused` sends every unsupported path with the VALID store (`"IdentityStoreId": STORE`, line 430), and `tests/all/list_users.rs:442` `a_store_this_emulator_does_not_serve_is_not_found_and_an_unwritten_one_lists_nothing` sends no `Filters` at all — so the combined request is untested and the current precedence is not pinned.

## Steps to Reproduce

1. Call ListUsers with `{"IdentityStoreId": "<a well-formed IdentityStoreId IAM Identity Center does not report>", "Filters": [{"AttributePath": "Title", "AttributeValue": "x"}]}`
2. Observe `ResourceNotFoundException` (ResourceType IDENTITY_STORE) rather than the `ValidationException` list_users.rs:201 builds for the unsearchable attribute path
3. Repeat the same request against the valid store id and observe the `ValidationException` the handler does raise — proving the refusal exists and is only ordered behind the store lookup
4. Compare with GetUserId: `{"IdentityStoreId": "<the same unknown store>", "AlternateIdentifier": {"UniqueAttribute": {"AttributePath": "Title", "AttributeValue": "x"}}}` answers the 400 first, as get_user_id.rs:165-168 states
