Issue-Severity: MEDIUM
Issue-Gap-Class: error-response
Issue-Commit: 37bf0949a3a203e7d1b7ebeb80108d58d67e6afd

# A scalar activityId whose value reads as a collection size ('0 items') loses its value from the @length clause, which this crate reserves for a collection's own @length

_Filed: 2026-11-27T22:20:32Z_
_Operation: GetAccountActivity | Class: error-response_

> _Note: this issue was identified against the code base at commit `37bf0949a3a203e7d1b7ebeb80108d58d67e6afd`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Every refusal of this service is worded by `src/service/validation.rs`, whose rule 2 states that "a clause states the value as SUBMITTED, the member's dotted path and the requirement it failed", and whose rule 5 reserves the value-less form for "a clause about a COLLECTION's own `@length` ... because a list and a map have no scalar rendering to quote".

`quoted()` decides which of the two forms a clause takes by asking `states_a_count()`, and that function tells a collection's count rendering from a caller's own bytes by the TEXT of the stored echo alone: a `MinLength`/`MaxLength` violation whose echo parses as `<digits> items` or `<digits> entries` is taken for a collection summary. `activityId` is a SCALAR string with `@length(32, 32)` — a MINIMUM of 32 — so any submitted value of the form `<digits> items` (7 characters upward) breaks that minimum while reading exactly like a list's count rendering. The clause for it then drops the submitted value, even though the member is a string and the caller really did send those bytes.

The result is self-contradictory inside ONE message: because `^[a-zA-Z0-9]+$` forbids the space in " items", such a value also breaks the `@pattern`, so the same refusal carries one clause that quotes the value (`Value '0 items' at 'activityId' …`) and one that does not (`Value at 'activityId' …`) for the same member and the same submitted bytes. The crate's own test pins the quoting form for the very same (member, requirement) pair with a different value (`activityId: "short"`).

The same mis-reading reaches the handler's own re-check of that member (`outside_declared_constraints`, which goes through the same `message()`/`quoted()` path), and the `@length` maximum side is reachable the same way with a 33+ character value ending in " items".

The shared reader this crate leans on documents the collision and argues it is unreachable, but the argument only considers `@length` MAXIMA: this service's `ActivityId` carries a `@length` MINIMUM of 32, which is broken by exactly the short strings that read as counts.

## Evidence

This crate's rules for the two clause forms — `services/service-com-amazonaws-freetier-awsfreetierservice/src/service/validation.rs:20-21` and `:28-30`:
```
//! 2. a clause states the value as SUBMITTED, the member's dotted path and the
//!    requirement it failed, in the framework's own words ([`framework_requirement`]);
...
//! 5. a clause about a COLLECTION's own `@length` states no value, because a list and a
//!    map have no scalar rendering to quote (`service_common::validation_message`'s
//!    readers of the framework's count rendering);
```

The code that picks the form, from the echo's text alone — `src/service/validation.rs:374-399`:
```
fn quoted(violation: &Violation, rendered_list: Option<&String>) -> Quoted {
    match (&violation.value, rendered_list) {
        (None, _) => Quoted::Null,
        (Some(_), Some(rendered)) => Quoted::Value(rendered.clone()),
        (Some(_), None) if states_a_count(violation) => Quoted::Counted,
        (Some(shown), None) => Quoted::Value(shown.clone()),
    }
}
...
fn states_a_count(violation: &Violation) -> bool {
    if !matches!(
        violation.kind,
        ViolationKind::MinLength(_) | ViolationKind::MaxLength(_)
    ) {
        return false;
    }
    violation.value.as_deref().is_some_and(|stored| {
        list_item_count(stored).is_some() || map_entry_count(stored).is_some()
    })
}
```
and the value-less subject it then states — `src/service/validation.rs:337-343`:
```
    fn subject(&self) -> String {
        match self {
            Self::Null => "Value null".to_string(),
            Self::Counted => "Value".to_string(),
            Self::Value(value) => format!("Value '{value}'"),
        }
    }
```

The reader it consults, with the premise this service falsifies — `services/common/src/validation_message.rs:186-193`:
```
/// why that crate cannot be changed from here. The accepted collision is a STRING member
/// whose submitted value is literally `<digits> items`/`<digits> entries` and whose
/// `@length` bound that string breaks: such a refusal is stated as a collection's. No
/// emulated member admits that today (the shortest `@length` maximum is far above the
/// seven characters the shortest such string takes), and the alternative — reading the
/// shape kind — needs the frozen crate.
fn collection_size(stored: &str, suffix: &str) -> Option<usize> {
    stored.strip_suffix(suffix)?.parse::<usize>().ok()
}
```
The member that admits it is this service's, and the bound is a MINIMUM rather than a maximum — `smithy_query(query="shape", shape="ActivityId")`:
```
"ActivityId": {"type": "string", "length": {"min": 32, "max": 32}, "pattern": "^[a-zA-Z0-9]+$"}
```
(`OPERATIONS.md:29`: `activityId` (string, len 32..32, pattern ^[a-zA-Z0-9]+$, required)).

The crate's own test pins the quoting form for the SAME member and the SAME requirement with a value that does not read as a count — `tests/all/validation_message_rules.rs:579-586`:
```
        (
            "a @length minimum on a scalar member",
            "GetAccountActivity",
            json!({"activityId": "short"}),
            "1 validation error detected: Value 'short' at 'activityId' failed to satisfy \
             constraint: Member must have length greater than or equal to 32"
                .to_string(),
        ),
```
and the test's own doc states the split this defect breaks — `tests/all/validation_message_rules.rs:550-553`:
```
/// RULE 2 on a member that is NOT a list element: the clause states the value as
/// submitted, the member's dotted path and the one requirement it failed — a `@pattern`
/// without the anchors the model writes the whole-value match with, and a COLLECTION's own
/// `@length` with no value at all, because a list has no scalar rendering to quote.
```
No test drives an `activityId` of the colliding form: `grep -n 'items' tests/all/validation_message_rules.rs` matches nothing, and the only `items` hits in the suite are pagination helpers' `items_member` locals.

## Steps to Reproduce

1. Read the model's bound for the member: smithy_query(query="shape", shape="ActivityId") reports "length": {"min": 32, "max": 32} on a STRING shape — a @length MINIMUM, so any 7-plus-character value breaks it.
2. Read src/service/validation.rs:20-21 (rule 2: a clause states the value as SUBMITTED) and :28-30 (rule 5: only a COLLECTION's own @length states no value).
3. Read src/service/validation.rs:389-399: `states_a_count` returns true for any MinLength/MaxLength violation whose stored echo parses as `<digits> items` or `<digits> entries`, with no check that the member is a collection; :378 then turns that into `Quoted::Counted`, whose subject is the bare "Value" (:340).
4. POST / with `X-Amz-Target: AWSFreeTierService.GetAccountActivity` and body {"activityId": "0 items"}. The 400 ValidationException message states the length clause WITHOUT the value — `Value at 'activityId' failed to satisfy constraint: Member must have length greater than or equal to 32` — beside a pattern clause for the same member that DOES quote it: `Value '0 items' at 'activityId' failed to satisfy constraint: Member must satisfy regular expression pattern: [a-zA-Z0-9]+`.
5. Compare with the pinned row in tests/all/validation_message_rules.rs:579-586: {"activityId": "short"} is required to answer `Value 'short' at 'activityId' failed to satisfy constraint: Member must have length greater than or equal to 32` — the same member and requirement, quoting the value.
6. Repeat on the maximum side with a 33-character value whose digits end in " items" (e.g. {"activityId": "1000000000000000000000000 items"} padded past 32 characters): the MaxLength clause likewise drops the submitted value.
