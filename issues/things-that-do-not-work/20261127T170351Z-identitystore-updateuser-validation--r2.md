Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: cc8c6d4514b1c20009096c9d1c08086c7dabca5f

# Photos are stored with more than one Primary entry, though the model states only one photo can be designated as primary per user

_Filed: 2026-11-27T17:03:51Z_
_Operation: UpdateUser | Class: validation_

> _Note: this issue was identified against the code base at commit `cc8c6d4514b1c20009096c9d1c08086c7dabca5f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

The modelled `Photo$Primary` states the rule that at most one of a user's photos may be the primary one. `User$Photos` is modelled `length 1..3`, so a request can legitimately carry two or three photo entries — and neither write path checks the primary designation.

`UpdateUser`'s `photos` attribute replacement decodes each entry through `photo()`, which copies the entry's `Primary` straight through after checking only its JSON type, and through `entries()`, which checks only how MANY entries arrived (1..3). Nothing compares the entries to each other. `CreateUser` is looser still: it stores `input.photos` verbatim.

So a request that sets `photos` to two entries both carrying `Primary: true` answers 200, the emulator persists both, and `DescribeUser`/`ListUsers` then report a user with two photos designated primary — a state the model says cannot exist. Nothing in the crate mentions the rule: `grep -n '[Pp]rimary' .../src` matches only the member plumbing (`PHOTO_MEMBERS`, `flag`, the serde of the element types) and never a check, and no test pins acceptance of two primaries. Both write doors (the `photos` attribute operation and `CreateUser$Photos`) need the same single check.

## Evidence

Smithy — `smithy_query(query="shape",shape="Photo")`, member `Primary`:
```text
Specifies whether this is the user's primary photo. Default value is false. Only one photo can be designated as primary per user.
```
and the shape doc itself: "Contains information about a user's photo. Users can have up to 3 photos, with one designated as primary."

What the update path does — services/service-com-amazonaws-identitystore-awsidentitystore/src/ops/update_user.rs:436-438:
```rust
        "photos" => {
            user.photos = entries(value, path, PHOTOS_MAX, "Photo", PHOTO_MEMBERS, photo)?
        }
```
the only per-entry check, services/service-com-amazonaws-identitystore-awsidentitystore/src/ops/update_user.rs:706-716:
```rust
fn photo(
    members: &::std::collections::HashMap<String, &::aws_smithy_types::Document>,
    path: &str,
) -> Result<Photo, String> {
    Ok(Photo {
        value: required_text(members.get("Value").copied(), path, "Photo")?,
        r#type: optional_text(members.get("Type").copied(), path, TEXT)?,
        display: optional_text(members.get("Display").copied(), path, TEXT)?,
        primary: flag(members.get("Primary").copied(), path)?,
    })
}
```
and the only list-level check is the COUNT, services/service-com-amazonaws-identitystore-awsidentitystore/src/ops/update_user.rs:638-644:
```rust
    if !(1..=max).contains(&items.len()) {
        return Err(format!(
            "the AttributeValue for {path} carries {} entries, outside the 1..{max} \
             the model allows: omit the AttributeValue to remove the attribute",
            items.len()
        ));
    }
```
The create path stores the list unexamined, services/service-com-amazonaws-identitystore-awsidentitystore/src/ops/create_user.rs:310:
```rust
            photos: input.photos.unwrap_or_default(),
```

No crate-side justification: `flag`'s doc (update_user.rs:459-465) only explains that an unsupplied `Primary` stays ABSENT ("a `Primary` the request did not name stays ABSENT rather than taking the model's `@default`") and says nothing about how many entries may carry `true`.

Test coverage — the two suites that exercise multi-entry photos each use exactly ONE primary, so neither pins the invalid state:
- services/service-com-amazonaws-identitystore-awsidentitystore/tests/all/update_user_attributes.rs:456-463 ("photos, three entries at the modelled maximum"): `{ "Value": ".../1.png", "Type": "jpg", "Primary": true }, { "Value": ".../2.png", "Display": "second" }, { "Value": ".../3.gif" }`
- services/service-com-amazonaws-identitystore-awsidentitystore/tests/all/identity_store_lifecycle.rs:313-317: `{ "Value": "https://example.com/1.png", ..., "Primary": true }, { "Value": ".../2.png" }, { "Value": ".../3.png" }`

## Steps to Reproduce

1. Boot the emulator and use the seeded IdentityStoreId (`state::SEED_IDENTITY_STORE_ID`, `d-9067f3a1c2`).
2. POST / with `X-Amz-Target: AWSIdentityStore.CreateUser` and body `{"IdentityStoreId":"d-9067f3a1c2","UserName":"ada"}` → 200 with a minted `UserId`.
3. POST / with `X-Amz-Target: AWSIdentityStore.UpdateUser` and body `{"IdentityStoreId":"d-9067f3a1c2","UserId":"<id>","Operations":[{"AttributePath":"photos","AttributeValue":[{"Value":"https://example.invalid/1.png","Primary":true},{"Value":"https://example.invalid/2.png","Primary":true}]}]}` → HTTP 200 and `Revision` 2 (expected: a refusal, since the model allows only one photo to be designated primary).
4. POST / with `X-Amz-Target: AWSIdentityStore.DescribeUser` and body `{"IdentityStoreId":"d-9067f3a1c2","UserId":"<id>"}` → `Photos` reports BOTH entries with `"Primary": true`, so the invalid state was persisted and is reported.
5. Same on the create door: POST `CreateUser` with `{"IdentityStoreId":"d-9067f3a1c2","UserName":"grace","Photos":[{"Value":"https://example.invalid/a.png","Primary":true},{"Value":"https://example.invalid/b.png","Primary":true}]}` → 200, and `DescribeUser` reports two primary photos.
