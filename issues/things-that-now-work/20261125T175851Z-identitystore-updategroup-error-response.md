Issue-Severity: MEDIUM
Issue-Gap-Class: error-response
Issue-Commit: 915a01c0e95f5bf32f728fc00ffaecdea11d8d07

# UpdateGroup words its own ValidationException with the framework envelope, bypassing the service's three stated message rules (anchored pattern, echoed value fragment)

_Filed: 2026-11-25T17:58:51Z_
_Operation: UpdateGroup | Class: error-response_

> _Note: this issue was identified against the code base at commit `915a01c0e95f5bf32f728fc00ffaecdea11d8d07`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`UpdateGroup` is the one handler in this crate that builds framework `Violation`s itself: it rebuilds the modelled member schemas for `displayName`/`description` and drives `aws_smithy_schema_server::constraints::validate`, then renders the refusal message with the FRAMEWORK's `envelope(&violations)`.

The crate, however, states its own wording rules for a constraint violation and states them as universal: `src/service/validation.rs` declares that "Three rules distinguish it from the framework's default wording, and each holds for every member of every operation, not for the ones a report happened to cite" — rule 1 being that a reported `@pattern` is the expression the service APPLIES, with the `^`/`$` anchors stripped, and rule 2 that a `@sensitive` member's value fragment is reduced to a bare `Value`. The crate already exposes the renderer that applies them, `pub fn message(violations: &[Violation])`, and the same module records that the framework's `envelope` "words each clause through `Violation::message`" — i.e. the raw, unprocessed wording.

So for an `UpdateGroup` whose `AttributeValue` violates the modelled `@length` or `@pattern`, the service answers a message that (a) quotes the pattern ANCHORED (`Member must satisfy regular expression pattern: ^[\p{L}...]+$`, and with the `\x{A0}`/`\x{3000}` escapes this file writes instead of the model's literal characters), and (b) opens with `Value '***'` instead of the bare `Value` the service's rule 2 produces for a `@sensitive` member — while the identical violation class arriving at the request boundary (e.g. `CreateGroup` with the same bad `DisplayName`) is reported unanchored and without any value fragment. One operation reports a constraint two different ways depending on which code path noticed it, and the path that diverges is the one that skips the helper this crate already has.

## Evidence

The rule the crate states for EVERY member of EVERY operation — services/service-com-amazonaws-identitystore-awsidentitystore/src/service/validation.rs:13-21:
```
//! Three rules distinguish it from the framework's default wording, and each holds for
//! every member of every operation, not for the ones a report happened to cite:
//!
//! 1. a reported `@pattern` is the expression the service APPLIES, not the model's trait
//!    text: Smithy pattern semantics are a find, so the `^`/`$` anchors are not part of
//!    it, and an ARN pattern carries the partition grammar rather than the model's loose
//!    wildcard ([`runtime_pattern`]);
//! 2. the offending value is NOT echoed for a member the model marks `@sensitive`
//!    ([`SENSITIVE_MEMBERS`]) — the member path and the constraint still are;
```
The helper that applies them, available crate-wide — validation.rs:181: `pub fn message(violations: &[Violation]) -> Option<String>`; the unanchoring it performs — validation.rs:355-366 `fn runtime_pattern(...) { let unanchored = pattern.strip_prefix('^')... }`; the sensitive value fragment — validation.rs:287 `None if withheld => "Value".to_string(),` / :290 `_ if is_sensitive(path) => "Value".to_string(),`.
The crate's own statement about what `envelope` does — validation.rs:31-34: "the envelope's counting/joining is already rendered by the framework (`aws_smithy_schema_server::constraints::envelope`, which cannot be reused here because it words each clause through `Violation::message` with no hook for the redaction and list folding above)".

The path that bypasses it — src/ops/update_group.rs:115-117 `use ::aws_smithy_schema_server::constraints::{ envelope, validate, LengthTrait, PatternTrait, Violation, };`, the anchored trait text it reports — update_group.rs:173:
```rust
const GROUP_TEXT_PATTERN: &str = r"^[\p{L}\p{M}\p{S}\p{N}\p{P}\t\n\r \x{A0}\x{3000}]+$";
```
installed as the applied/reported pattern at update_group.rs:193 `traits.insert(Box::new(PatternTrait::new(GROUP_TEXT_PATTERN)));`, and the rendering at update_group.rs:338-353:
```rust
    let violations: Vec<Violation> = validate(
        &AttributeText { schema: attribute.member_schema(), text },
        &UPDATED_GROUP_ATTRIBUTES_SCHEMA,
    )
    .into_iter()
    .map(|violation| Violation { value: Some(REDACTED.to_string()), ..violation })
    .collect();
    if let Some(message) = envelope(&violations) {
        return Err(invalid(message));
    }
```
with `const REDACTED: &str = "***";` (update_group.rs:183) — a value fragment where rule 2 emits none.

Test coverage: the rule table pins the boundary path only and asserts exactly what the handler path breaks — tests/all/error_message_rules.rs:282-285:
```rust
        assert!(
            !message.contains("pattern: ^") && !message.ends_with('$'),
            "case {case:?}: expected the reported pattern to be unanchored, got {message:?}"
        );
```
and that table contains no `UpdateGroup` row (its rows are CreateUser/CreateGroup/DescribeGroup/GetUserId/UpdateUser/ListGroupMemberships). The UpdateGroup refusal suite asserts only status and code, never the message — tests/all/update_group.rs:550-559 (rows "a value carrying a character the modelled pattern excludes", "a value carrying a space character the modelled class omits") and tests/all/update_group.rs:571-583 `refused(&router, "UpdateGroup", ..., 400, "ValidationException", case)`. So nothing pins the current anchored/echoing wording.

## Steps to Reproduce

1. Create a group in the seeded identity store: CreateGroup(IdentityStoreId=<seed store>, DisplayName="g1").
2. Send CreateGroup(IdentityStoreId=<seed store>, DisplayName="bell\u0007value") and read Error.Message: the boundary renderer answers the service's wording — the pattern unanchored and no value fragment for the `@sensitive` member (the rule tests/all/error_message_rules.rs:282-285 pins).
3. Send UpdateGroup(IdentityStoreId=<seed store>, GroupId=<g1>, Operations=[{"AttributePath":"description","AttributeValue":"bell\u0007value"}]) and read Error.Message: the same constraint class is reported through `envelope` (src/ops/update_group.rs:351), so the clause quotes the anchored `^[\p{L}\p{M}\p{S}\p{N}\p{P}\t\n\r \x{A0}\x{3000}]+$` and opens with `Value '***'`.
4. Compare the two messages: one operation's refusals for one constraint class are worded two different ways, and the `UpdateGroup` wording contradicts rules 1 and 2 that src/service/validation.rs:13-21 states hold "for every member of every operation", even though `crate::service::validation::message` (validation.rs:181) is public and takes exactly the `Violation` list the handler already has.
