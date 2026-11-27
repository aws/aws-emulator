Issue-Severity: LOW
Issue-Gap-Class: wrong-behavior
Issue-Commit: eef1405ac0806ef267777bba6aefe2b3b0754197

# UpdateUser releases a UserName uniqueness claim without the holder check its three siblings in this crate apply

_Filed: 2026-11-27T12:45:41Z_
_Operation: UpdateUser | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `eef1405ac0806ef267777bba6aefe2b3b0754197`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

When an update changes a user's `UserName`, `UpdateUser` removes the OLD name from `store.user_name_index` unconditionally — it never checks that the index still records THIS user as the holder of that name. Three other places in the same crate perform the same release and all three guard it on the holder's id, two of them with a comment stating why: `IdentityStoreData::remove_user`, `IdentityStoreData::remove_group`, and the sibling `UpdateGroup`, whose comment says outright that "a request must never drop another group's uniqueness claim". So one of the two update handlers implements the rule and the other does not: whenever the index entry for the released name belongs to a different user, `UpdateUser` drops that other user's uniqueness claim, after which a second user could claim a `UserName` that is still stored on the first one. The same handler also mixes the two spellings of the user's own key where `UpdateGroup` uses one: the ownership comparison reads `updated.user_id` (update_user.rs:264) while the index insert writes `user_key` (update_user.rs:291), whereas `UpdateGroup` uses `updated.group_id` on both sides (update_group.rs:394 and :426).

## Evidence

The unguarded release — services/service-com-amazonaws-identitystore-awsidentitystore/src/ops/update_user.rs:284-293:
```rust
// The `UserName` index follows the name: the old claim is released and the new
// one taken, so a later create or update can reuse the name this user dropped.
if updated.user_name != stored.user_name {
    if let Some(released) = stored.user_name.as_deref() {
        store.user_name_index.remove(released);
    }
    if let Some(claimed) = updated.user_name.clone() {
        store.user_name_index.insert(claimed, user_key.clone());
    }
}
```
The sibling that guards it — src/ops/update_group.rs:410-422:
```rust
if previous_display_name != updated.display_name {
    // Released only when the index still records THIS group as the name's
    // owner, mirroring the ownership test the claim above makes: a request
    // must never drop another group's uniqueness claim.
    if let Some(released) = previous_display_name {
        if store
            .group_display_name_index
            .get(&released)
            .is_some_and(|owner| *owner == updated.group_id)
        {
            store.group_display_name_index.remove(&released);
        }
    }
```
The state module applies the same guard for BOTH claims — src/state.rs:725-738 (`remove_user`):
```rust
// The index IS the store's uniqueness claim on the name, so dropping the
// holder frees the name for a later create. Guarded on the holder's id in case
// the entry has since been re-pointed at another user.
let claimed_name = user
    .user_name
    .clone()
    .filter(|user_name| {
        self.user_name_index
            .get(user_name)
            .is_some_and(|holder| holder == user_id)
    });
```
and src/state.rs:689-696 (`remove_group`, same comment and same `is_some_and(|holder| holder == group_id)` test).
Test coverage of the claim this index backs: tests/all/update_user_attributes.rs:959 `a_user_name_an_update_takes_stays_unique`, tests/all/delete_user.rs:243 `deleting_a_user_takes_its_memberships_and_frees_its_user_name`, tests/all/update_group.rs:608 `a_display_name_stays_a_uniqueness_claim_within_one_identity_store` — none of them exercises a release whose index entry points at another record, which is exactly the case the three guarded copies defend against and this one does not.

## Steps to Reproduce

1. Read src/ops/update_user.rs:286-289 and src/ops/update_group.rs:410-422 side by side: the group path releases the old name only when the index still names this group as the owner; the user path removes the entry unconditionally.
2. Read src/state.rs:725-738 and src/state.rs:689-696: both `remove_user` and `remove_group` state the guard as the crate's rule ("Guarded on the holder's id in case the entry has since been re-pointed at another user").
3. Compare src/ops/update_user.rs:264 (`*taken_by != updated.user_id`) with src/ops/update_user.rs:291 (`insert(claimed, user_key.clone())`): the ownership test and the index write use two different spellings of the same key, where update_group.rs:394 and :426 use one.
