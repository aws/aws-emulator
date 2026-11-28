Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 44393db1f79b587bcbd2de03c9db6137eee45723

# A user created with an empty Name object is reported as `Name: {}`, though this crate's own rule is that a name with no members is no name at all

_Filed: 2026-11-28T15:04:16Z_
_Operation: DescribeUser | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `44393db1f79b587bcbd2de03c9db6137eee45723`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

This crate states — and implements, and pins with a test — the rule that a `Name` whose every member is absent is NOT a name: `UpdateUser`'s `optional_name` drops such an object, and `set_name_part` drops the whole `Name` once its last sub-attribute is removed, "so it is reported absent rather than as an empty object".

`CreateUser` skips that normalisation: it stores the request's `Name` verbatim (`name: input.name`). Every member of the modelled `Name` shape is optional, so `{"Name": {}}` passes the request boundary untouched, and the record then holds a `Some(Name)` with six `None` members. `DescribeUser` (and `ListUsers`, through `User::to_user`) reports that record faithfully, which on the awsJson wire is the empty object `"Name":{}` — exactly the response the sibling path exists to prevent.

So one and the same end state is reported two different ways depending on which door was used:

* `CreateUser` with `"Name": {}` -> `DescribeUser` answers `"Name": {}`;
* `UpdateUser` with `[{"AttributePath":"name","AttributeValue":{}}]` on the same user -> `optional_name` folds the all-absent object away and `DescribeUser` answers with no `Name` member at all;
* `UpdateUser` setting `name.givenName` and then removing it -> no `Name` member (pinned by the suite).

The defect is the response this operation produces for a state the sibling normalisation declares impossible; the cause is the missing fold at `create_user.rs:294`. Either end of that pair (normalise on create, or fold when rendering) removes the contradiction; nothing in the crate justifies keeping the empty object, and no test pins it.

## Evidence

The rule, stated and implemented — services/service-com-amazonaws-identitystore-awsidentitystore/src/ops/update_user.rs:565-567:
```rust
    // A name every member of which was removed is no name at all, so it is reported
    // absent rather than as an empty object.
    Ok((!empty_name(&name)).then_some(name))
```
and the same fold on the sub-attribute path — services/service-com-amazonaws-identitystore-awsidentitystore/src/ops/update_user.rs:598-602:
```rust
    let name = user.name.get_or_insert_with(Name::default);
    *part(name) = text;
    if empty_name(name) {
        user.name = None;
    }
```
(its doc, update_user.rs:586-587: "The `Name` object springs into being when a sub-attribute is set on a user that has none, and goes away again once its last member is removed.")

The create path, with no such fold — services/service-com-amazonaws-identitystore-awsidentitystore/src/ops/create_user.rs:288-294:
```rust
        let user = crate::state::User {
            user_arn: crate::state::resource_arn(crate::state::ResourceKind::User, &user_id),
            user_id: user_id.clone(),
            // A create is revision 1; an UpdateUser bumps it from there.
            revision: 1,
            user_name: input.user_name,
            name: input.name,
```

The reader that then reports it — services/service-com-amazonaws-identitystore-awsidentitystore/src/ops/describe_user.rs:251:
```rust
            name: user.name.clone(),
```
and the listing's copy — services/service-com-amazonaws-identitystore-awsidentitystore/src/state.rs:350:
```rust
            name: self.name.clone(),
```

Every member of `Name` is optional, so `{"Name": {}}` is accepted by the boundary validator: `smithy_query(query="shape",shape="User")` marks only `IdentityStoreId`, `UserId`, `UserArn` and `Revision` as `required`, and `Name`'s own members (`Formatted`, `FamilyName`, `GivenName`, `MiddleName`, `HonorificPrefix`, `HonorificSuffix`) carry no `@required` — mirrored by the handler's own table, update_user.rs:336-343.

Test coverage: the update side of the rule is pinned — services/service-com-amazonaws-identitystore-awsidentitystore/tests/all/update_user_attributes.rs:604-605 and 627-628:
```rust
    // Removing ONE sub-attribute leaves its siblings alone; removing the last one
    // leaves no name at all.
...
    let described = describe(&router, &user, case).await;
    assert_absent(&described, "Name", "the last sub-attribute removed");
```
Nothing pins the create side: `grep -n 'Name": *{}' tests/` matches nothing, so no existing test asserts the empty object.

## Steps to Reproduce

1. Boot the emulator and address the seeded identity store (`state::SEED_IDENTITY_STORE_ID` = `d-9067f3a1c2`).
2. POST / with `X-Amz-Target: AWSIdentityStore.CreateUser` and body `{"IdentityStoreId":"d-9067f3a1c2","UserName":"ada","Name":{}}` -> 200 with a minted `UserId`.
3. POST / with `X-Amz-Target: AWSIdentityStore.DescribeUser` and body `{"IdentityStoreId":"d-9067f3a1c2","UserId":"<UserId>"}` -> the response carries `"Name":{}` (expected, by the crate's own rule: no `Name` member at all).
4. POST / with `X-Amz-Target: AWSIdentityStore.ListUsers` and body `{"IdentityStoreId":"d-9067f3a1c2"}` -> the listed user carries the same `"Name":{}`.
5. POST / with `X-Amz-Target: AWSIdentityStore.UpdateUser` and body `{"IdentityStoreId":"d-9067f3a1c2","UserId":"<UserId>","Operations":[{"AttributePath":"name","AttributeValue":{}}]}` -> 200, and a repeat of step 3 now reports NO `Name` member: the same value, sent through the update door, is normalised away.
