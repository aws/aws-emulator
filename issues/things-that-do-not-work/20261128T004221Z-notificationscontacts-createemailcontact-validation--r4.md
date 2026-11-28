Issue-Severity: LOW
Issue-Gap-Class: validation
Issue-Commit: 73e1e77630833aa0f20a2b43ded051b89907df1b

# The suite that owns hand-applied @pattern refusals states this service hand-applies TWO patterns, while its own handler says three

_Filed: 2026-11-28T00:42:21Z_
_Operation: CreateEmailContact | Class: validation_

> _Note: this issue was identified against the code base at commit `73e1e77630833aa0f20a2b43ded051b89907df1b`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

This crate applies THREE modelled `@pattern`s inside handler code instead of leaving them to the framework's centralized constraint walk: `EmailContactName`'s `[\w-.~]+`, `TagKey`'s `^(?!aws:).{1,128}$`, and `EmailContactAddress`'s `^(.+)@(.+)$` (the last one because the walk's `regex` engine reads `.` as excluding a line FEED alone, where the modelled ECMA-262 `.` excludes CR, U+2028 and U+2029 too). All three are refused through the one renderer `crate::state::pattern_validation_failure`, which the crate documents as "The ONE refusal path for a `@pattern` this service has to apply by hand, so no caller of it can word such a refusal its own way" (src/state.rs:229-241, 770-808).

The suite that OWNS the wording of those refusals — `tests/all/hand_applied_pattern_refusals.rs`, headed "End-to-end HTTP tests for how this service WORDS the refusal of a modelled `@pattern` it has to apply by hand" and generalising "whichever member and whichever expression it is" — states that only TWO patterns reach this service by hand, names them, and claims its table "spans BOTH, and every member each travels through", listing four members. The third hand-applied pattern and its member (`CreateEmailContact$emailAddress`) are absent from that statement, from the table's own `Request` enum (which has no address variant) and from the row-variation comment ("which of the two patterns is broken").

The defect is the stale enumeration, not the responses: the address refusal's wording is asserted by the sibling suite `tests/all/email_contact_address_pattern.rs`. But a reader (or a fixer) of the suite that owns this rule is told the hand-applied set is two patterns over four members, while the handler under test says "this handler applies all three" — so the suite's stated scope contradicts the code it is the authority for, and the one member the rule could drift on sits outside the span it claims to be exhaustive over.

## Evidence

Stated rule — tests/all/hand_applied_pattern_refusals.rs:18-23:
```
//! Two modelled patterns reach this service by hand, for reasons that are the
//! framework's rather than the model's: `EmailContactName`'s `[\w-.~]+`, whose character
//! class the walk's regex engine refuses, and `TagKey`'s `^(?!aws:).{1,128}$`, a map-key
//! constraint the walk does not visit and a look-ahead it cannot compile. The table below
//! spans BOTH, and every member each travels through: `CreateEmailContact$name`,
//! `CreateEmailContact$tags`, `TagResource$tags` and `UntagResource$tagKeys`.
```
and the same file's row-variation comment, tests/all/hand_applied_pattern_refusals.rs:86-88:
```
/// Every row is one request refused by a pattern this service applies by hand, the rows
/// differing along what the rule generalises over: which of the two patterns is broken,
/// which member the value travels through (a map key, a list element, a plain string),
```
and its `Request` enum, which carries no address case — tests/all/hand_applied_pattern_refusals.rs:57-66:
```
enum Request {
    /// `CreateEmailContact` carrying this `name`.
    Name(String),
    /// `CreateEmailContact` carrying a tag map with these keys.
    CreateTags(Vec<String>),
    /// `TagResource` carrying a tag map with these keys.
    TagResourceTags(Vec<String>),
    /// `UntagResource` naming these keys, in this order.
    UntagTagKeys(Vec<String>),
}
```

Contradicted by the handler the suite drives — src/ops/create_email_contact.rs:157-169:
```
        // Every modelled `@pattern` in this input is applied HERE
        // instead, because the walk's engine does not reproduce the modelled expression
        // for any of them: ... and it reads the `.` of
        // `emailAddress`'s `^(.+)@(.+)$` as excluding a line feed alone, where the
        // modelled ECMA-262 expression's `.` excludes every line terminator. So this
        // handler applies all three (see `crate::state::check_email_contact_name`,
        // `crate::state::check_email_contact_address` and
        // `crate::state::satisfies_tag_key_pattern`) before it touches the store,
```
and by the third pattern's own home — src/state.rs:300-305:
```
/// The modelled `EmailContactAddress` pattern, verbatim
/// (`smithy_query(query="shape",shape="EmailContactAddress")`).
///
/// Stated here because this one, too, has to be applied by hand: see
/// [`satisfies_email_contact_address_pattern`].
pub const EMAIL_CONTACT_ADDRESS_PATTERN: &str = r"^(.+)@(.+)$";
```
src/state.rs:320-323:
```
/// therefore applied here as the parts it is, by the one operation that accepts an
/// address (`CreateEmailContact`): no character of the value is one `.` does not match,
```
and that refusal travels the same single renderer as the other two — src/state.rs:348-356:
```
pub fn check_email_contact_address(
    member: &str,
    address: &str,
) -> Result<(), ValidationException> {
    refuse_outside_pattern(
        EMAIL_CONTACT_ADDRESS_PATTERN,
        (!satisfies_email_contact_address_pattern(address)).then(|| member.to_string()),
    )
}
```
(`refuse_outside_pattern` → `pattern_validation_failure`, src/state.rs:233-241 and 781-808 — documented at 231-232 as "The ONE refusal path for a `@pattern` this service has to apply by hand").

Test coverage showing the RESPONSE side is already pinned and only the enumeration/scope is stale: tests/all/email_contact_address_pattern.rs:298 `the_refusal_blames_the_member_and_states_the_modelled_expression` asserts the address refusal's message, member path and published expression; tests/all/hand_applied_pattern_refusals.rs asserts the same rendering for `name`, `tags` and `tagKeys`.

## Steps to Reproduce

1. Read tests/all/hand_applied_pattern_refusals.rs lines 3-4 and 18-23: the suite declares itself the owner of how this service words a `@pattern` it applies by hand, states that TWO such patterns exist, and claims its table spans both patterns and 'every member each travels through' — listing `CreateEmailContact$name`, `CreateEmailContact$tags`, `TagResource$tags`, `UntagResource$tagKeys`.
2. Read src/ops/create_email_contact.rs lines 157-178: the handler states 'this handler applies all three' and calls `check_email_contact_name`, `check_email_contact_address` and `check_tag_map_keys` — a third hand-applied pattern, on `emailAddress`.
3. Read src/state.rs lines 300-305 and 320-356: `EMAIL_CONTACT_ADDRESS_PATTERN` is 'Stated here because this one, too, has to be applied by hand', is applied 'by the one operation that accepts an address (`CreateEmailContact`)', and its refusal goes through the same `refuse_outside_pattern`/`pattern_validation_failure` path the other two use.
4. Conclude the suite's stated scope is the stale side: three patterns over five member sites are hand-applied, so the sentence 'Two modelled patterns reach this service by hand', the 'which of the two patterns is broken' row-variation note and the four-member enumeration describe a crate that no longer exists — while `CreateEmailContact$emailAddress`, whose refusal is worded by the very renderer this suite owns, sits outside the span it claims is exhaustive (its wording is asserted only in tests/all/email_contact_address_pattern.rs:298).
