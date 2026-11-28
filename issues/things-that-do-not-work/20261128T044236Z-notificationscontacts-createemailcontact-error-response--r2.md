Issue-Severity: LOW
Issue-Gap-Class: error-response
Issue-Commit: b47827fe487a7b68f5bd7ca07ac85170ae40cd62

# A create breaking TWO hand-applied @patterns is refused with a one-clause envelope, where the walk this crate claims to match aggregates both

_Filed: 2026-11-28T04:42:36Z_
_Operation: CreateEmailContact | Class: error-response_

> _Note: this issue was identified against the code base at commit `b47827fe487a7b68f5bd7ca07ac85170ae40cd62`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`CreateEmailContact` applies three modelled `@pattern`s by hand (`name`, `emailAddress`, the `tags` keys) because the centralized constraint walk cannot reproduce them. The crate states, twice, that such a by-hand refusal is worded exactly as the walk would have worded it. But the walk COLLECTS every violation of one request and states them in a single `N validation error(s) detected: <clause>; <clause>` envelope, while the hand-applied path can only ever state ONE clause: the handler returns on the first failing check with `?`, and `crate::state::pattern_validation_failure` builds its envelope from `std::slice::from_ref(&clause)` — a one-element slice by construction.

So a create whose `name` is outside `[\w-.~]+` AND whose `tags` carry a reserved `aws:` key (both of which pass the walk, which is exactly why the handler applies them) is answered `1 validation error detected: Value at 'name' ...`, naming only `name` and silently dropping the second violation; `fieldList` likewise blames one member. The same request breaking two constraints the walk CAN apply is answered `2 validation errors detected: ...; ...` — the crate's own suite asserts such a two-clause answer. The two therefore do not "read alike", which is the rule the code under those docs is supposed to keep, and a caller must make a second round trip to discover the second broken pattern.

No test pins the single clause for this case: every row of the suite that owns these refusals breaks exactly one pattern (its rows with several offending values are several values of the SAME member and pattern, justified there as "one request always reads the same way however the map happened to hand its keys over").

## Evidence

Stated rule #1 — src/state.rs:770-777:
```
/// The modelled 400 for a value outside the `@pattern` its member declares.
///
/// The sentence is NOT written here: the expression a clause advertises, the clause, the
/// envelope and the requirement half are the framework's and the cross-service
/// validation-message renderings', so a `@pattern` this service has to apply itself is
/// refused in exactly the words a `@pattern` the centralized walk could apply is.
```
Stated rule #2 — src/service/validation.rs:23-26:
```
//! Nothing here is keyed to an operation or to a submitted value, so one rule covers every
//! constrained member of every operation of this service; the `@pattern`s this service has
//! to apply ITSELF (`crate::state::pattern_validation_failure`) are worded through the
//! same renderings, so a constraint the walk applies and one a handler applies read alike.
```

Code side #1 — the envelope is built over exactly one clause, src/state.rs:798-801:
```
    let clause = violation.message();
    ValidationException {
        message: ::service_common::validation_message::envelope_over(std::slice::from_ref(&clause))
            .unwrap_or(clause),
```
Code side #2 — the handler stops at the first hand-applied pattern, src/ops/create_email_contact.rs:171-178:
```
        crate::state::check_email_contact_name("name", &input.name)
            .map_err(CreateEmailContactErrorModeled::ValidationException)?;
        crate::state::check_email_contact_address("emailAddress", &input.email_address)
            .map_err(CreateEmailContactErrorModeled::ValidationException)?;
        if let Some(tags) = input.tags.as_ref() {
            crate::state::check_tag_map_keys("tags", tags.keys().map(String::as_str))
                .map_err(CreateEmailContactErrorModeled::ValidationException)?;
        }
```
(and src/ops/create_email_contact.rs:168-169: "So this handler applies all three ... They are judged in the order the walk visits the input's members in.")

Proof the walk aggregates several clauses into ONE envelope — tests/all/error_message_templates.rs:217-233:
```
        Case {
            what: "a sensitive activation code shorter than the only length it admits",
            request: Refused::Activate(padding(ACTIVATION_CODE_LEN - 1)),
            // Too short AND outside the expression, which is one clause each.
            expected: envelope(&[
                clause(... "Member must have length greater than or equal to {ACTIVATION_CODE_LEN}"),
                clause("code", "Member must satisfy regular expression pattern: [a-z0-9]{7}"),
            ]),
```
with the envelope's plural form at tests/all/error_message_templates.rs:61-66:
```
fn envelope(clauses: &[String]) -> String {
    match clauses.len() {
        1 => format!("1 validation error detected: {}", clauses[0]),
        n => format!("{n} validation errors detected: {}", clauses.join("; ")),
```

The suite that owns hand-applied refusals states the same `N`-shaped rule but can only transcribe `1` — tests/all/hand_applied_pattern_refusals.rs:10-13 ("refused in the same words every constraint refusal of this service carries — the `N validation error detected:` envelope around ...") and tests/all/hand_applied_pattern_refusals.rs:271-276:
```
fn transcribed(path: &str, pattern: &str) -> (String, String) {
    let requirement = format!("Member must satisfy regular expression pattern: {pattern}");
    let envelope =
        format!("1 validation error detected: Value at '{path}'{CONSTRAINT_SEPARATOR}{requirement}");
```
Every row of that suite's table (tests/all/hand_applied_pattern_refusals.rs:93-213) breaks exactly ONE pattern, so the two-pattern case is unpinned by any test.

## Steps to Reproduce

1. Read src/state.rs:770-777 and src/service/validation.rs:23-26 — both state that a `@pattern` this service applies by hand is refused in exactly the words the centralized walk would use.
2. Read tests/all/error_message_templates.rs:217-233 with the `envelope` helper at lines 61-66 — a single request breaking two constraints the walk applies is answered with ONE message carrying BOTH clauses (`2 validation errors detected: ...; ...`).
3. Read src/state.rs:798-801 — the hand-applied path builds its envelope from `std::slice::from_ref(&clause)`, so it can never carry more than one clause.
4. Read src/ops/create_email_contact.rs:171-178 — the three hand-applied checks are sequenced with `?`, so the handler returns on the first failure and never looks at the rest.
5. POST /2022-09-19/emailcontacts with `{"name":"on call","emailAddress":"oncall@example.com","tags":{"aws:owner":"x"}}`: both the name pattern and the tag-key pattern are broken and both pass the walk, yet the answer is `1 validation error detected: Value at 'name' failed to satisfy constraint: Member must satisfy regular expression pattern: [\\w-.~]+`, with a single-entry `fieldList`; the reserved tag key is not reported until the caller fixes the name and retries.
