Issue-Severity: MEDIUM
Issue-Gap-Class: fidelity
Issue-Commit: mainline

# The struct derive forces every `@required` member onto the wire, so an output member AWS documents as nullable cannot be omitted

_Filed: 2026-10-29T04:00:00Z_
_Operation: cross-service (every output member a model marks `@required` and documents as nullable) | Class: fidelity_

## Description

`smithy_macros::smithy(shape = "…")` derives a member's Rust optionality from the model:
`validate_field_type` (`vendor/smithy-macros/src/lib.rs:3047`) computes
`is_required = @required || usable_default(..)` and then REFUSES an `Option<T>` field for
such a member — "Field '<name>' is @required or @default in Smithy, so it must not be
Option<T>." (`vendor/smithy-macros/src/lib.rs:3070`). The generated serializer writes a
non-`Option` field unconditionally and wraps only an `Option` field in
`if let Some(..)` (`vendor/smithy-macros/src/lib.rs:2823`).

That is right for an INPUT: a required member absent from a request is a refusal. It is
wrong for an OUTPUT whose member the same model documents as nullable. AWS models carry
both on one member — several listings declare their response `nextToken` `@required`
while documenting it as "The token for the next set of results, or null if there are no
additional results" — and the real service omits the member on the last page, because a
restJson1 serializer writes no key for a null output member. A derived output struct
cannot express that: the field is non-`Option`, so the only "no value" it can report is
the type's zero, which for a string is `""`.

The zero is not always inside the member's own value space, and when it is not, the
emulator reports a value the model forbids. `com.amazonaws.iotsitewise#NextToken` is
`@length(min 1, max 4096)` + `@pattern ^[A-Za-z0-9+/=]+$`, so `""` is outside it, and the
SAME shape bounds the operation's input member — a paginator that feeds the reported token
back is refused by boundary validation before the handler runs, and the enumeration cannot
be walked to completion.

## Evidence

- The predicate and the refusal: `vendor/smithy-macros/src/lib.rs:3047-3078`.
- The write arms, which already emit `if let Some(..)` for an optional member and an
  unconditional write for a required one: `vendor/smithy-macros/src/lib.rs:2810-2831`.
- `unsupported = [..]` is not a substitute: it drops the member from the shape's schema
  and from the body ENTIRELY (`vendor/smithy-macros/src/lib.rs:2509`), which is correct for
  a member the service never reports (see
  `services/service-com-amazonaws-batch-awsbatchv20160810/src/ops/types/describe_jobs_types.rs`)
  but not for one that is present on some responses and absent on others.
- An affected member:
  `smithy_query(query="doc", shape="ListActionsResponse", member="nextToken", model_path="services/service-com-amazonaws-iotsitewise-awsiotsitewise/smithy-model/com-amazonaws-iotsitewise-awsiotsitewise.json")`
  -> "The token for the next set of results, or null if there are no additional results",
  on a member `smithy_query(query="operation", operation="ListActions")` reports as
  `"required": true` with `length {min 1, max 4096}` and `pattern ^[A-Za-z0-9+/=]+$`.

## What a fix needs

The real fix is in the generator, which is in the agent-immutable `vendor/` tree: let the
author declare, per shape, the members whose model `@required` must not force presence —
the attribute parser already accepts list-valued keys beside `unsupported` and
`skip_validation` (`vendor/smithy-macros/src/lib.rs:882`) — and for those members keep the
member in the schema, accept an `Option<T>` field, and emit the `if let Some(..)` write arm
the derive already produces for an optional member. Nothing below the macro has to change:
the codecs write only what `serialize_members` writes.

Until then the only remedy available inside `services/` is per-shape: write that one
shape's `Schema` + `SerializableStruct` out by hand, as
`services/service-com-amazonaws-iotsitewise-awsiotsitewise/src/ops/types/list_actions_types.rs`
does for `ListActionsResponse` (and as
`services/service-com-amazonaws-account-account/src/ops/types/get_account_information_types.rs`
does for a different generator gap). That does not scale — it is one hand-maintained copy
of generated code per affected shape — which is why this is filed rather than repeated.

## Steps to Reproduce

1. Pick an output shape with a member the model marks `@required` and documents as null on
   some responses, and give its Rust field type `Option<T>`.
2. `cargo check -p <that service crate>` -> "Field '<name>' is @required or @default in
   Smithy, so it must not be Option<T>."
3. Make the field non-`Option` instead and call the operation on a response where AWS omits
   the member: the body carries the type's zero (`""` for a string) where AWS carries no
   key at all.

## Resolution

Resolved WITHOUT the generator change above, which is still outstanding: `vendor/` is
read-only from a service crate, so the omission now happens one layer below the derive, at
the protocol seam a service already owns.

`services/common/src/unset_string_members.rs` states the rule once for every service of the
class — a response carries no key for a member that holds no value, and a string member
whose own `@length` minimum is 1 or more has no empty value to report — as the predicate
`forbids_the_empty_string` plus `OmittingUnsetStrings`, the `SerializableStruct` projection a
service wraps every output, error and event in at `Service::override_protocol`. The walk
re-enters nested structures and keeps every collection element as written, since a
collection member's `@length` bounds the element count rather than the element.

With that available, the two copies of the rule this report named are gone:
`services/service-com-amazonaws-iotsitewise-awsiotsitewise/src/ops/types/list_actions_types.rs`
is back on `#[smithy_macros::smithy(shape = ..)]` — the hand-written `Schema` and
`SerializableStruct` deleted, `nextToken` the plain `String` the `@required` member forces,
and the empty token dropped by the codec — and that crate's
`src/service/unset_string_members.rs` keeps only its own protocol and the input-shape hook,
delegating the rule to the shared module. `core/router/tests/all/iotsitewise_unset_output_member_is_omitted.rs`
pins the wire form through both operations.

A member whose own shape ADMITS the empty string is NOT covered: whether such a member is
omitted or reported as `""` is each service's own recorded fact (the WAFv2 crate records
`""` for a `@length(1..256)` description), so the shared rule keys on the value space rather
than on a list of member names. `get_account_information_types.rs` is untouched: it works
around a different gap (`@timestampFormat`), whose own shared remedy is
`services/common/src/date_time_json.rs`.
