Issue-Severity: LOW
Issue-Gap-Class: maintainability
Issue-Commit: 65ee76328c73fce07b2a3bd288dbf2c587c2316a

# state::unanchored hand-rolls the @pattern anchor-drop that service_common::validation already owns

_Filed: 2026-11-27T02:28:44Z_
_Operation: CreateInvestigationGroup | Class: maintainability_

> _Note: this issue was identified against the code base at commit `65ee76328c73fce07b2a3bd288dbf2c587c2316a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`crate::state` carries its own private `fn unanchored` to drop the `^`/`$` a model writes around a `@pattern` before a refusal quotes the expression. That rule is already implemented, documented and exported by `service_common::validation` — a crate this very file already calls into three times — and the shared home's own documentation says the per-crate copies are to be deleted in favour of it and that "nothing new should inline the rule again".

The local copy's own doc comment admits the duplication and even names the shared home, but justifies keeping the copy with "because none is reachable from another" — a reason that does not hold for `services/common`, which this same file reaches at state.rs:562, 602 and 806. So the two spellings of one rule can drift silently: a reword of the shared rendering (the one the framework-rendered boundary refusals this crate deliberately keeps in step with are worded through) leaves this crate stating the old one.

The migration is byte-for-byte output-preserving, and that is the point of filing it as maintainability rather than behaviour. The local rule drops the two ends INDEPENDENTLY (`^` whether or not a `$` follows, and a trailing unescaped `$` whether or not a `^` opened), which is exactly the composition of the two one-ended dialects the shared module exports: `validation::unanchored_at_the_close(&validation::unanchored_at_the_open(p))`. Both ends' escape rule is identical too — the local `count() % 2 == 0` keep-condition is the negation of the shared `ends_with_escape` (`count() % 2 == 1`). (The shared *pair*-positional `validation::unanchored` is NOT the drop-in: it declines the drop unless both ends carry an anchor, so it would change the `^costs\$` row this crate's own table test pins. Using the two one-ended entry points keeps every rendering identical.)

Every expression this service actually reaches the helper with is double-anchored (`^arn:.*$`, `^([\p{L}\p{Z}\p{N}_.:/=+\-@]+)$`, `^(?:[\-_A-Za-z0-9]{1,512}|arn:…)$`, …), so no response byte and no pinned clause changes either way.

## Evidence

The hand-rolled copy, whose own doc names the shared home and gives a reason that does not apply to it — services/service-com-amazonaws-aiops-aiops/src/state.rs:530-548:
```
/// A `@pattern` expression without the anchors a model spells it with: Smithy's
/// `@pattern` is an unanchored find, so `^`/`$` are how a model demands a whole-string
/// match rather than part of the expression a value has to satisfy.
///
/// A trailing `$` that is ESCAPED is a literal dollar sign inside the expression, not
/// the end anchor, so it stays: the run of backslashes before it decides.
///
/// (Sibling service crates carry their own copies of this helper — `grep -rln 'fn
/// unanchored' services/*/src` — because none is reachable from another;
/// `services/common/src/validation.rs` is the staged home for the one shared copy.)
fn unanchored(pattern: &str) -> &str {
    let body = pattern.strip_prefix('^').unwrap_or(pattern);
    match body.strip_suffix('$') {
        Some(trimmed) if trimmed.chars().rev().take_while(|c| *c == '\\').count() % 2 == 0 => {
            trimmed
        }
        _ => body,
    }
}
```

The shared home, which already exists and is public — services/common/src/validation.rs:1805-1856:
```
/// THE STAGED HOME for this rendering, which the module docs above list as one of the parts
/// of a validation message that are AWS's rather than any one service's. ~26 service crates
/// carry their own inlined copy (`grep -rln 'fn unanchored' services/*/src`), each pinned by
/// that service's own recordings; those copies are to be DELETED in favour of this one by the
/// change that owns that migration, and nothing new should inline the rule again.
…
pub fn unanchored(regex: &str) -> String {
```

The two one-ended entry points whose composition IS this crate's rule — services/common/src/validation.rs:2158-2184:
```
pub fn unanchored_at_the_close(regex: &str) -> String {
    match regex.strip_suffix('$') {
        Some(inner) if !ends_with_escape(inner) => inner.to_string(),
        _ => regex.to_string(),
    }
}
…
pub fn unanchored_at_the_open(regex: &str) -> String {
    match regex.strip_prefix('^') {
        Some(inner) => inner.to_string(),
        None => regex.to_string(),
    }
}
```
and the identical escape rule — services/common/src/validation.rs:2469-2473:
```
/// Whether `text` ends in an escape — an odd number of trailing backslashes, so the
/// character that followed them was escaped.
fn ends_with_escape(text: &str) -> bool {
    text.chars().rev().take_while(|c| *c == '\\').count() % 2 == 1
}
```

The shared module confirms `unanchored` owns exactly this span — services/common/src/validation_message.rs:551-553:
```
/// The finding that layer states for a string outside the regular expression its schema
/// declares: the expression as the MODEL writes it (anchors included — this is the schema's
/// own expression, not the per-member clause's rendering that [`unanchored`] owns) …
```

The crate already depends on `service_common` in this same file, three times — services/service-com-amazonaws-aiops-aiops/src/state.rs:561-567, 601-603, 806:
```
pub fn requirement_text(kind: ViolationKind) -> String {
    let requirement = ::service_common::validation_message::requirement(kind);
    match requirement.strip_prefix(PATTERN_REQUIREMENT) {
        Some(pattern) => format!("{PATTERN_REQUIREMENT}{}", applied_pattern(pattern)),
```
```
pub fn validation_envelope(clauses: &[String]) -> String {
    ::service_common::validation_message::envelope_over(clauses).unwrap_or_default()
}
```
```
            ::service_common::validation_message::map_value_set_requirement(&requirements)
```

The local helper's one caller — services/service-com-amazonaws-aiops-aiops/src/state.rs:523-528:
```
pub fn applied_pattern(modelled: &str) -> &str {
    APPLIED_PATTERNS
        .iter()
        .find(|(declared, _)| *declared == modelled)
        .map_or_else(|| unanchored(modelled), |(_, applied)| *applied)
}
```

Reached from this operation's handler: services/service-com-amazonaws-aiops-aiops/src/ops/create_investigation_group.rs:257-260 calls `crate::state::map_entry_rejection(input.tags.as_ref(), input.chatbot_notification_channel.as_ref())`, which builds its clause through `map_keys_clause` → `requirement_bundle` (state.rs:746-761 pushes `ViolationKind::Pattern`) → `requirement_text` → `applied_pattern` → `unanchored`, so a refused `tags` / `chatbotNotificationChannel` key quotes the expression this helper renders.

Existing tests that pin every rendering involved and must keep passing unchanged:
- services/service-com-amazonaws-aiops-aiops/tests/all/error_message_rules.rs:508-519 — the `applied_pattern` table, including `("an anchored expression with no override", r"^arn:.*$", ROLE_ARN_PATTERN)`, `("an already unanchored expression", r"[a-z]{1,4}", r"[a-z]{1,4}")` and `("an escaped trailing dollar is not an anchor", r"^costs\$", r"costs\$")`
- services/service-com-amazonaws-aiops-aiops/tests/all/shared_constraint_wording.rs:345, :373, :546
- services/service-com-amazonaws-aiops-aiops/tests/all/modelled_map_key_constraints.rs:76, :84, :93, :115 (e.g. `"Member must satisfy regular expression pattern: arn:.*"`)
- services/service-com-amazonaws-aiops-aiops/tests/all/map_member_refusal_paths.rs:45, :54
- services/service-com-amazonaws-aiops-aiops/tests/all/arn_field_split_is_shared.rs:77

## Steps to Reproduce

1. Open services/common/src/validation.rs and read the doc on `pub fn unanchored` (lines 1805-1809): it is declared THE STAGED HOME for the anchor-drop rendering, says ~26 service crates carry an inlined copy that is "to be DELETED in favour of this one", and says "nothing new should inline the rule again".
2. Open services/service-com-amazonaws-aiops-aiops/src/state.rs:540-548 and see the inlined copy, `fn unanchored`, whose own doc (lines 537-539) names that exact file as "the staged home for the one shared copy" and justifies the copy with "because none is reachable from another".
3. Note services/service-com-amazonaws-aiops-aiops/src/state.rs:562, :602 and :806 already call `::service_common::validation_message::…` from the same module, so the shared crate IS reachable and the stated reason for the local copy does not hold.
4. Compare the local rule with services/common/src/validation.rs:2158-2184: `unanchored_at_the_close(&unanchored_at_the_open(p))` performs the same two independent drops, and services/common/src/validation.rs:2471 `ends_with_escape` is the same odd-trailing-backslash test as the local `% 2 == 0` keep-condition — so substituting the two shared entry points changes no rendered expression, including the `^costs\$` → `costs\$` row pinned at tests/all/error_message_rules.rs:512.
5. Reword either copy (for example change the escape handling on one side only) and observe that nothing fails to compile: the two spellings of one rule drift silently, while tests/all/error_message_rules.rs:508-519 pins only this crate's side.
