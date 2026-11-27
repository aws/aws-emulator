Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: eff3d781d07d1ebfff8840fbfaaac8071cc43b20

# The UserName/DisplayName uniqueness claim is keyed case-exactly, though the crate's own rule says an identity name is not case-exact

_Filed: 2026-11-27T19:10:01Z_
_Operation: CreateUser | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `eff3d781d07d1ebfff8840fbfaaac8071cc43b20`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

This crate applies two contradictory rules to the SAME value — a user's `UserName` and a group's `DisplayName`.

The reserved-name rule folds case, and `crate::state::is_reserved_identity_name`'s own doc gives the reason: an identity name "is not case-exact", so another spelling of a reserved name would still be "an identity named a reserved name". The crate's suite pins that rule with the same reason.

The uniqueness claim the same handler makes two blocks later over the same member does NOT fold case: it asks a `HashMap<String, String>` keyed by the raw name (`store.user_name_index.contains_key(user_name)`), and the insert that takes the claim stores the raw name too. So the "Duplicate UserName" / "Duplicate GroupDisplayName" claim does not hold for the spellings the crate itself says name ONE identity: `CreateUser` with `UserName: "Bob"` followed by `CreateUser` with `UserName: "bob"` both answer 200 and the store then holds two users whose names differ only in case. `CreateGroup` behaves the same way through `group_display_name_index`, and `UpdateUser` / `UpdateGroup` make their claim through the same two indexes.

The divergence is also visible on the read side: `GetUserId` resolves `userName` through that same case-exact index, so after storing "Bob" a lookup of "BOB" answers `USER not found.` — while `GetGroupId`/`GetUserId` DO fold the attribute PATH. The crate is not missing a helper here: `is_reserved_identity_name` is the case-folding comparison of an identity name it already owns, and the claim path skips it.

Fixing it is one normalisation at the two index keys (and the matching lookups) in `crate::state`; the one place the two rules must agree is what an identity name's identity is.

## Evidence

The rule the crate states for an identity name — services/service-com-amazonaws-identitystore-awsidentitystore/src/state.rs:1256-1265:
```rust
/// Whether `name` is one of the [`RESERVED_IDENTITY_NAMES`].
///
/// Compared without regard to letter case: what is reserved is the NAME, and these are
/// SCIM identity names, which are not case-exact — a store that admitted another spelling
/// of one would hold an identity named a reserved name.
pub fn is_reserved_identity_name(name: &str) -> bool {
    RESERVED_IDENTITY_NAMES
        .iter()
        .any(|reserved| reserved.eq_ignore_ascii_case(name))
}
```

That rule applied in the handler — services/service-com-amazonaws-identitystore-awsidentitystore/src/ops/create_user.rs:219-232:
```rust
        // `UserName` NAMES the user, and a name this API reserves cannot name one: a
        // create carrying a reserved name is refused with the modelled
        // `ValidationException` and stores nothing.
        if input
            .user_name
            .as_deref()
            .is_some_and(crate::state::is_reserved_identity_name)
```

The SAME member's uniqueness claim, case-exact, 40 lines later — services/service-com-amazonaws-identitystore-awsidentitystore/src/ops/create_user.rs:270-284:
```rust
        // A `UserName` is a uniqueness claim within the identity store, so a second
        // create naming one that is taken conflicts rather than storing a duplicate.
        if input
            .user_name
            .as_deref()
            .is_some_and(|user_name| store.user_name_index.contains_key(user_name))
```
and the claim it takes, create_user.rs:331-333:
```rust
        if let Some(user_name) = user.user_name.clone() {
            store.user_name_index.insert(user_name, user_id.clone());
        }
```
The index is keyed by the raw name — services/service-com-amazonaws-identitystore-awsidentitystore/src/state.rs:519-525:
```rust
    /// `UserName` -> `UserId`. Backs `CreateUser`'s uniqueness claim and, in a later
    /// slice, `GetUserId` by `AlternateIdentifier`.
    pub user_name_index: HashMap<String, String>,
    ...
    /// `DisplayName` -> `GroupId`, backing `CreateGroup`'s uniqueness claim.
    pub group_display_name_index: HashMap<String, String>,
```
The group door does the same — services/service-com-amazonaws-identitystore-awsidentitystore/src/ops/create_group.rs:231-235:
```rust
        if input
            .display_name
            .as_deref()
            .is_some_and(|display_name| store.group_display_name_index.contains_key(display_name))
```
And the read side inherits it — services/service-com-amazonaws-identitystore-awsidentitystore/src/ops/get_user_id.rs:274-277:
```rust
            Self::UserName(user_name) => store
                .user_name_index
                .get(user_name)
                .and_then(|user_id| store.users.get(user_id)),
```

Test coverage: the reserved-name rule's reason is pinned verbatim — services/service-com-amazonaws-identitystore-awsidentitystore/tests/all/identity_store_lifecycle.rs:1597-1605:
```rust
/// Every spelling of `name` that names the same identity: an identity name is not
/// case-exact, so a reserved name is reserved however it is cased.
fn spellings(name: &str) -> [(&'static str, String); 3] {
```
while the duplicate-claim suite only ever retries the IDENTICAL spelling, so nothing pins case-variant coexistence — services/service-com-amazonaws-identitystore-awsidentitystore/tests/all/error_message_rules.rs:1043-1057 creates `user_name` and then re-sends the same `user_name` expecting 409 `"Duplicate UserName"`. `grep -n 'case.?sensitiv' tests/` matches nothing.

## Steps to Reproduce

1. Boot the emulator and address the seeded store (`state::SEED_IDENTITY_STORE_ID`, `d-9067f3a1c2`).
2. POST / with `X-Amz-Target: AWSIdentityStore.CreateUser` and body `{"IdentityStoreId":"d-9067f3a1c2","UserName":"Bob"}` → 200 with a minted `UserId`.
3. POST / with `X-Amz-Target: AWSIdentityStore.CreateUser` and body `{"IdentityStoreId":"d-9067f3a1c2","UserName":"bob"}` → 200 and a SECOND `UserId` (expected: 409 `ConflictException` / `Duplicate UserName`, since by the crate's own rule "bob" and "Bob" are one identity name).
4. POST / with `X-Amz-Target: AWSIdentityStore.ListUsers` and body `{"IdentityStoreId":"d-9067f3a1c2"}` → both users are listed, with `UserName` "Bob" and "bob".
5. POST / with `X-Amz-Target: AWSIdentityStore.GetUserId` and body `{"IdentityStoreId":"d-9067f3a1c2","AlternateIdentifier":{"UniqueAttribute":{"AttributePath":"userName","AttributeValue":"BOB"}}}` → 400 `ResourceNotFoundException` with message `USER not found.`, although the same request with `AttributePath` spelled `USERNAME` resolves fine (the path folds case, the value does not).
6. Same on the group door: `CreateGroup` with `DisplayName: "Admins"` then `DisplayName: "admins"` → two groups, and `GetGroupId` by `displayName: "ADMINS"` answers `GROUP not found.`
