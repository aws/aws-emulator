Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: f9465bc2186e307f05476a9cadaf16956fa09a02

# UpdateUser compares the Revision precondition as a string while DeleteUser compares it as a number

_Filed: 2026-11-25T17:17:16Z_
_Operation: UpdateUser | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `f9465bc2186e307f05476a9cadaf16956fa09a02`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`Revision` is the caller's expected current revision on both `UpdateUser` and `DeleteUser`. `DeleteUser` normalises it: it parses the member as a number and compares the numbers, and its comment states the rule for the member — "The member is a decimal string (`^[0-9]+$`), so it is compared as the number it denotes — `01` and `1` name one revision". `UpdateUser` skips that normalisation and compares the raw strings, so for one and the same user and one and the same precondition value the two operations disagree: `DeleteUser` with `Revision: "01"` deletes a user at revision 1 (pinned by this crate's own suite), while `UpdateUser` with `Revision: "01"` on the same user answers HTTP 409 ConflictException and applies nothing.

The member, its pattern and its meaning are identical on both operations, so one of the two comparisons is wrong; nothing in the crate justifies the divergence.

## Evidence

Normalising side — services/service-com-amazonaws-identitystore-awsidentitystore/src/ops/delete_user.rs:187-205:
```rust
// `Revision` is the caller's EXPECTED current revision ... The member is a decimal string (`^[0-9]+$`), so it is compared as
// the number it denotes — `01` and `1` name one revision — and a value too
// large to be any revision simply never matches.
if let Some(expected) = input.revision.as_deref() {
    if let Some(user) = store.users.get(&user_key) {
        if expected.parse::<u64>().ok() != Some(user.revision) {
            return Err(DeleteUserErrorModeled::ConflictException(...));
```

Non-normalising side — src/ops/update_user.rs:223-233:
```rust
// `Revision` is the caller's precondition: "the update is applied only if it
// matches the current revision of the user … If the value doesn't match, the
// operation fails with a ConflictException."
if let Some(expected) = input.revision.as_deref() {
    if expected != stored.revision() {
        return Err(UpdateUserErrorModeled::ConflictException(
            crate::state::revision_conflict(expected, &stored.revision()),
```
where `User::revision()` is `self.revision.to_string()` (src/state.rs:305-307), i.e. always the unpadded spelling.

The delete semantics are pinned by this crate's tests — tests/all/delete_user.rs:155-157 and :193:
```rust
/// The current revision padded with leading zeros — the same number, which the
...
("the current revision, zero-padded", Revision::CurrentPadded),
```
and tests/all/delete_group.rs:435 ("the current revision with a leading zero"). The update suites send only unpadded values (tests/all/update_group.rs:424-428, tests/all/identity_store_resolution_and_updates.rs:1026-1029), so nothing pins the string comparison.

## Steps to Reproduce

1. CreateUser in the seeded identity store; the response reports `Revision: "1"`.
2. Call UpdateUser with `Revision: "01"` and `Operations: [{"AttributePath": "displayName", "AttributeValue": "Ada"}]` → HTTP 409 ConflictException; DescribeUser shows the user still at revision 1 and unchanged.
3. Call DeleteUser for the same user with `Revision: "01"` → HTTP 200 and the user is gone (the behaviour tests/all/delete_user.rs pins as `Revision::CurrentPadded`).
4. Observe that one precondition value naming revision 1 is accepted by the delete and refused by the update.
