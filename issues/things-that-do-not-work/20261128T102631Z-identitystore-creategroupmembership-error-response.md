Issue-Severity: MEDIUM
Issue-Gap-Class: error-response
Issue-Commit: 687333e2232d3fadb3887f8d91f372124a7f50c2

# The not-found for a MemberId that names no user echoes the probed identifier in ResourceId, which this crate's own rule for that class forbids

_Filed: 2026-11-28T10:26:31Z_
_Operation: CreateGroupMembership | Class: error-response_

> _Note: this issue was identified against the code base at commit `687333e2232d3fadb3887f8d91f372124a7f50c2`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`crate::state::not_found` is this crate's constructor for the `ResourceNotFoundException` of a resource a store does not hold, and its doc states the rule for the class: the refusal reports `ResourceType` and the message "and nothing else, so the probed identifier is not echoed back in a typed member" — which is why it sets `resource_id: None`, as `not_found_by_alternate_identifier` also does. `member_not_found`, the constructor for the OTHER end of a membership (a `MemberId.UserId` that names no user of the store), is the same error class with the same `ResourceType` (`USER`), but it sets `resource_id: Some(member_user_id.to_string())`, echoing the identifier the caller sent. So one request that names a missing user through `UserId` (DescribeUser/UpdateUser) answers with `ResourceId` absent, while another that names a missing user through `MemberId` (CreateGroupMembership, IsMemberInGroups, ListGroupMembershipsForMember) answers with it populated, for the same missing resource in the same store. The echoed value is also the RAW request text rather than the key the rest of the crate normalises an id to through `resource_key`, so a caller that sends the ARN form gets the ARN back in a typed member while the by-id path returns nothing. `member_not_found`'s own doc justifies only its distinct MESSAGE ("gets its own constant, naming that member"); it says nothing about widening the typed member its sibling deliberately leaves unset.

## Evidence

The rule and the sibling that applies it — services/service-com-amazonaws-identitystore-awsidentitystore/src/state.rs:1062-1075:
```
/// `ResourceId` is left unset for the same reason the message carries no id: a recorded
/// not-found of this class reports the `ResourceType` and the message and nothing else,
/// so the probed identifier is not echoed back in a typed member — the way
/// [`not_found_by_alternate_identifier`] already answers for a resource named by another
/// identifier.
pub fn not_found(kind: ResourceKind) -> ResourceNotFoundException {
    ResourceNotFoundException {
        resource_type: Some(kind.resource_type().to_string()),
        resource_id: None,
        ...
```
and state.rs:1125-1129 for the alternate-identifier form: "`ResourceId` is left unset rather than carrying what the caller sent".

The path that breaks it — state.rs:1085-1093:
```
pub fn member_not_found(member_user_id: &str) -> ResourceNotFoundException {
    ResourceNotFoundException {
        resource_type: Some(ResourceKind::User.resource_type().to_string()),
        resource_id: Some(member_user_id.to_string()),
        reason: None,
        message: Some("User with provided member ID not found".to_string()),
        request_id: Some(new_request_id()),
    }
}
```

Its callers, all passing the request's raw `MemberId.UserId`:
- src/ops/create_group_membership.rs:214-219 `crate::state::member_not_found(member)` (the `member` bound at line 212 from `let MemberId::UserId(member) = &input.member_id;`)
- src/ops/is_member_in_groups.rs:185-189 `crate::state::member_not_found(member)`
- src/ops/list_group_memberships_for_member.rs:194-198 `crate::state::member_not_found(member)`

Contrast the by-id path for the same missing user — src/ops/describe_user.rs:238-242 `crate::state::not_found(crate::state::ResourceKind::User)` (ResourceId absent) and src/ops/update_user.rs:213-217.

Test coverage: tests/all/exception_envelope.rs pins the envelope members (status, code, RequestId) for the handler-raised classes, and tests/all/group_membership_lookup.rs / list_group_memberships_for_member.rs / is_member_in_groups.rs drive the member-missing refusals, but none asserts `ResourceId` absent on them, so the divergence from the stated rule is unpinned.

## Steps to Reproduce

1. Create a user and a group in the seeded identity store (d-9067f3a1c2).
2. POST / with X-Amz-Target: AWSIdentityStore.CreateGroupMembership and body {"IdentityStoreId": "d-9067f3a1c2", "GroupId": "<the group>", "MemberId": {"UserId": "0123abcd-4567-89ab-cdef-0123456789ab"}} — a well-formed id no store ever minted.
3. Inspect the refusal body: ResourceNotFoundException with ResourceType USER, message "User with provided member ID not found" AND ResourceId set to the id just sent.
4. POST / with X-Amz-Target: AWSIdentityStore.DescribeUser and body {"IdentityStoreId": "d-9067f3a1c2", "UserId": "0123abcd-4567-89ab-cdef-0123456789ab"}: same class, same ResourceType, but ResourceId is absent.
5. Repeat step 2 with the ARN form "arn:aws:identitystore:::user/0123abcd-4567-89ab-cdef-0123456789ab": the typed member echoes the ARN verbatim, un-normalised by `resource_key`.
