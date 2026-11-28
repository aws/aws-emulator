Issue-Severity: MEDIUM
Issue-Gap-Class: error-response
Issue-Commit: b16feb5d898a7c21dbe5a600460e765efc5f70eb

# UpdateUser refuses the extensions AttributePath, so a user's Extensions can never change after CreateUser

_Filed: 2026-11-28T08:36:06Z_
_Operation: UpdateUser | Class: error-response_

> _Note: this issue was identified against the code base at commit `b16feb5d898a7c21dbe5a600460e765efc5f70eb`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`UpdateUser` resolves an `AttributeOperation`'s `AttributePath` through a fixed `match` over lower-cased attribute names, and the catch-all arm refuses everything the match does not name with `ValidationException` (`Updates for AttributePath: <path> is not supported`).

`extensions` is not in that match, and neither is the `aws:identitystore:<name>` path form. So both

  * `{"AttributePath": "extensions", "AttributeValue": {...}}`, and
  * `{"AttributePath": "aws:identitystore:enterprise", "AttributeValue": {...}}`

are answered 400 `ValidationException` and nothing is applied — even though `Extensions` is a caller-supplied attribute of a user that this very crate persists (`CreateUser`) and reports (`DescribeUser`/`ListUsers`). The result is that a user's `Extensions` map is WRITE-ONCE: whatever `CreateUser` stored can never be added to, replaced or removed for the lifetime of the user.

Two things make this a defect rather than a feature nobody attempted:

1. The model's own `UpdateUserRequest$Operations` sentence defines the set of changeable attributes by pointing at the `User` shape, and `User$Extensions` is a member of that shape. The modelled `AttributePath` `@pattern` carries a dedicated alternative for a three-colon path (`\p{L}+:\p{L}+:\p{L}+(?:\.\p{L}+){0,3}`), which is exactly the shape of an `ExtensionName` (`^aws:identitystore:[a-z]{1,20}$`) and matches no other attribute name in this API — so such a path passes the boundary validator and reaches the handler, which then refuses it.
2. The handler's OWN doc enumerates the three classes of path it refuses, and `extensions` is in none of them: it is not service-minted, not identity-provider-owned, not "an attribute a user does not have" (the stored record has an `extensions` field), and not a path into a multi-valued attribute (it is a map member set whole, exactly like `emails`/`photos`, which the handler does replace wholesale).

The crate already holds every piece needed: the `extensions` field on the record, the create path that stores a whole map, and the reader that selects requested names.

## Evidence

Smithy — `smithy_query(query="doc",shape="UpdateUserRequest",member="Operations")`:
```text
A list of AttributeOperation objects to apply to the requested user. These operations might add, replace, or remove an attribute. For more information on the attributes that can be added, replaced, or removed, see User.
```
and the `User` shape the sentence points at declares `Extensions` as one of its members — `smithy_query(query="shape",shape="User")`:
```text
"Extensions": {"target": "Extensions", "length": {"min": 1, "max": 10}, "type": "map", "doc": "<p>A map of explicitly requested attribute extensions associated with the user. Not populated if the user has no requested extensions.</p>"}
```
The modelled path form that carries an extension name — `smithy_query(query="shape",shape="AttributeOperation")`:
```text
"AttributePath": {"target": "AttributePath", "required": true, "pattern": "^(?:\\p{L}+:\\p{L}+:\\p{L}+(?:\\.\\p{L}+){0,3}|\\p{L}+(?:\\.\\p{L}+){0,2})$", "doc": "<p>A string representation of the path to a given attribute or sub-attribute. Supports JMESPath.</p>"}
```
which is the same three-colon form `ExtensionName` declares — `smithy_query(query="shape",shape="ExtensionName")`: `"pattern": "^aws:identitystore:[a-z]{1,20}$"`.

What the handler does with it — services/service-com-amazonaws-identitystore-awsidentitystore/src/ops/update_user.rs:373-374 and :439-445 (no `extensions` arm anywhere in the match):
```rust
    let key = path.to_ascii_lowercase();
    match key.as_str() {
...
        // Every other path is one an update does not own: the attributes the service
        // mints or an identity provider owns, a path into a multi-valued attribute, and
        // anything that is not an attribute of a user at all. AWS refuses each with one
        // message built from the path it was given, which `crate::state` states once
        // for both update operations.
        _ => return Err(crate::state::unsupported_attribute_path_message(path)),
```
The handler's own doc of what that arm is FOR — update_user.rs:350-355:
```rust
/// A path this operation does not own — one the service mints or an identity provider
/// owns (`userId`, `userArn`, `identityStoreId`, `revision`, `externalIds`, the audit
/// members), an attribute a user does not have, or a path INTO a multi-valued attribute,
/// which AWS resolves with JMESPath over the stored entries and this slice does not
/// evaluate — is refused with the message AWS builds from the requested path, and
/// nothing is applied.
```
`extensions` is none of those: the record HAS the attribute — services/service-com-amazonaws-identitystore-awsidentitystore/src/state.rs:288:
```rust
    pub extensions: HashMap<String, ::aws_smithy_types::Document>,
```
the create door stores a whole map into it — src/ops/create_user.rs:315:
```rust
            extensions: input.extensions.unwrap_or_default(),
```
and the read door reports it — src/ops/describe_user.rs:272-275:
```rust
            // Only the extensions this request asked for: `Extensions` is "a collection
            // of extension names indicating what extensions the service should
            // retrieve alongside other user attributes".
            extensions: user.requested_extensions(input.extensions.as_deref()),
```
Meanwhile sibling multi-valued attributes ARE replaced wholesale by the same handler — update_user.rs:410-412:
```rust
        "emails" => {
            user.emails = entries(value, path, SINGLE_ENTRY_MAX, "Email", CONTACT_MEMBERS, email)?
        }
```

Test coverage: the update suite's refusal table covers unknown paths, service-minted paths and list sub-attribute paths, but no row names `extensions` or `aws:identitystore:…` at all — services/service-com-amazonaws-identitystore-awsidentitystore/tests/all/update_user_attributes.rs:774-785:
```rust
    let rows: Vec<(&str, Value)> = vec![
        ("a path no user attribute has", set("nosuchattribute", json!("x"))),
        ("a sub-attribute of a list", set("emails.value", json!("x"))),
        // Attributes the identity store owns: the ids, the ARN, the revision and the
        // audit timestamps are minted by the service, never set by the caller.
        ("the user id", set("userId", json!("x"))),
```
and the only extension suite exercises the create and read doors only — tests/all/list_users.rs:618-645 (`only_the_extensions_the_request_asks_for_are_reported`), so nothing pins the refusal this issue is about.

## Steps to Reproduce

1. Boot the emulator and address the seeded store (`state::SEED_IDENTITY_STORE_ID`, `d-9067f3a1c2`).
2. POST / with `X-Amz-Target: AWSIdentityStore.CreateUser` and body `{"IdentityStoreId":"d-9067f3a1c2","UserName":"alice","Extensions":{"aws:identitystore:enterprise":{"CostCenter":"4100"}}}` → 200 with a minted `UserId`.
3. POST / with `X-Amz-Target: AWSIdentityStore.UpdateUser` and body `{"IdentityStoreId":"d-9067f3a1c2","UserId":"<id>","Operations":[{"AttributePath":"aws:identitystore:enterprise","AttributeValue":{"CostCenter":"5200"}}]}` → 400 `ValidationException` `Updates for AttributePath: aws:identitystore:enterprise is not supported` (the path passes the modelled `AttributePath` pattern, so it reached the handler).
4. Repeat with `{"AttributePath":"extensions","AttributeValue":{"aws:identitystore:enterprise":{"CostCenter":"5200"}}}` → 400 with the same class of message.
5. POST / with `X-Amz-Target: AWSIdentityStore.DescribeUser` and body `{"IdentityStoreId":"d-9067f3a1c2","UserId":"<id>","Extensions":["aws:identitystore:enterprise"]}` → 200 still reporting `{"CostCenter":"4100"}`: the attribute the model lists among `User`'s members cannot be added, replaced or removed by any operation of this service.
6. Contrast with a sibling multi-valued attribute: `{"AttributePath":"emails","AttributeValue":[{"Value":"a@b.invalid"}]}` → 200 and the revision moves on.
