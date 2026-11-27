Issue-Severity: LOW
Issue-Gap-Class: maintainability
Issue-Commit: 15f47a4d1ebcac9f87407ace69f568de1ac33107

# state.rs re-derives the requirement sentence and inlines the map key/value constraint-set wording that service_common::validation_message already owns

_Filed: 2026-11-26T22:17:22Z_
_Operation: UpdateInvestigationGroup | Class: maintainability_

> _Note: this issue was identified against the code base at commit `15f47a4d1ebcac9f87407ace69f568de1ac33107`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`crate::state` hand-rolls three spans of refusal wording that the already-depended-on `service_common::validation_message` module provides, and that module's own documentation names `aiops-aiops` as a crate carrying an inlined copy to be migrated:

1. `MAP_VALUE_REQUIREMENT_PREFIX` plus the `["…", "…"]` join (used twice in this crate: once for a map VALUE the boundary walk refused, once for a map value COLLECTION this crate judges itself) re-spells `service_common::validation_message::map_value_set_requirement`, whose doc comment says this crate is to be migrated onto it "rather than copied again".
2. `requirement_text` re-derives the requirement half of a clause from `Violation::message` with its own `CONSTRAINT_SEPARATOR`, which is exactly the derivation `service_common::validation_message::requirement` exists to single-home — its doc names "each such crate re-deriving it from `Violation::message`" as the drift this `pub` function prevents.
3. `MAP_KEY_REQUIREMENT_PREFIX` plus the same bracketed join re-spells the `Map keys must satisfy constraint: ` + `bracketed_findings(...)` text inside `map_key_requirements_refusal`.

The sentences are byte-identical today, so the two copies can silently drift the moment either side is reworded: this crate's refusals are deliberately kept in step with the framework's boundary refusals (that is the stated reason `validation_envelope` already delegates to `service_common::validation_message::envelope_over` in the same file), and the requirement/constraint-set halves are the only parts of that grammar still spelled locally.

The fix is mechanical and must change no response byte: `requirement_text` keeps its signature and its `applied_pattern` restatement but computes the base sentence through `::service_common::validation_message::requirement`, and the two bundle call sites build their requirement through `map_value_set_requirement` / the shared key wording instead of the local prefixes.

## Evidence

Shared home, which NAMES this crate — services/common/src/validation_message.rs:2164-2187:
```
/// What a clause opens the VALUE shape's constraint set with, when the set is stated at the
/// MAP member whose values it is about — the map counterpart of [`ELEMENT_SET_OPEN`], …
const MAP_VALUE_SET_OPEN: &str = "Map value must satisfy constraint: [";
…
/// THE HOME for this sentence, under the same rule as [`element_set_requirement`]:
/// `sagemaker-sagemaker`, `aiops-aiops` and `pipes-pipes` each carry their own inlined copy,
/// written before this module had one, and are to be MIGRATED onto this one rather than
/// copied again.
pub fn map_value_set_requirement(requirements: &[String]) -> String {
    format!("{MAP_VALUE_SET_OPEN}{}]", requirements.join(", "))
}
```

This crate's inlined copy — services/service-com-amazonaws-aiops-aiops/src/state.rs:685-688:
```
/// How a clause states what a MAP ENTRY's value had to satisfy: …
const MAP_VALUE_REQUIREMENT_PREFIX: &str = "Map value must satisfy constraint: ";
```
used at services/service-com-amazonaws-aiops-aiops/src/state.rs:813:
```
            format!("{MAP_VALUE_REQUIREMENT_PREFIX}[{}]", requirements.join(", "))
```
and again at services/service-com-amazonaws-aiops-aiops/src/state.rs:1131-1136:
```
            constraint_clause(
                shape.member,
                &format!(
                    "{MAP_VALUE_REQUIREMENT_PREFIX}[{}]",
                    requirement_bundle(shape.value.traits()).join(", ")
```

The requirement derivation, duplicated. services/common/src/validation_message.rs:1376-1396:
```
/// `pub` because a service that states its own clauses around a requirement — one whose
/// dialect this module's whole-refusal entry points do not cover — needs the same sentence
/// for the same constraint: the alternative is each such crate re-deriving it from
/// [`Violation::message`], which is how two crates come to word one constraint two ways.
pub fn requirement(kind: ViolationKind) -> String {
    const CLAUSE_SEPARATOR: &str = "failed to satisfy constraint: ";
    let clause = Violation { path: String::new(), kind, value: None, sensitive: false }.message();
    match clause.split_once(CLAUSE_SEPARATOR) { … }
}
```
versus services/service-com-amazonaws-aiops-aiops/src/state.rs:489 and 559-573:
```
const CONSTRAINT_SEPARATOR: &str = " failed to satisfy constraint: ";
…
pub fn requirement_text(kind: ViolationKind) -> String {
    let rendered = Violation { path: String::new(), kind, value: None, sensitive: false }.message();
    let requirement = rendered
        .split_once(CONSTRAINT_SEPARATOR)
        .map_or(rendered.as_str(), |(_, requirement)| requirement);
```

The map-key wording, duplicated. services/service-com-amazonaws-aiops-aiops/src/state.rs:884:
```
const MAP_KEY_REQUIREMENT_PREFIX: &str = "Map keys must satisfy constraint: ";
```
versus services/common/src/validation_message.rs:1351-1354 (+ `bracketed_findings`, validation_message.rs:1133-1135):
```
    let clause = format!(
        "{stated_value} '{member_path}' failed to satisfy constraint: Map keys must satisfy \
         constraint: {}",
        bracketed_findings(requirements)
```

The crate already depends on this module for the sibling span — services/service-com-amazonaws-aiops-aiops/src/state.rs:608-610:
```
pub fn validation_envelope(clauses: &[String]) -> String {
    ::service_common::validation_message::envelope_over(clauses).unwrap_or_default()
}
```

Reached from this operation's handler — services/service-com-amazonaws-aiops-aiops/src/ops/update_investigation_group.rs:204-206:
```
        if let Some(why) =
            crate::state::map_entry_rejection(None, input.chatbot_notification_channel.as_ref())
```

Existing tests that pin every sentence involved, and must keep passing unchanged:
- services/service-com-amazonaws-aiops-aiops/tests/all/modelled_map_key_constraints.rs:232 (`"Value at '{member_path}' failed to satisfy constraint: Map keys must satisfy …"`) and :250 (`"… Map value must satisfy …"`)
- services/service-com-amazonaws-aiops-aiops/tests/all/error_message_rules.rs:179, :184, :514 (clause grammar and `state::applied_pattern`)
- services/service-com-amazonaws-aiops-aiops/tests/all/cross_account_configuration_names_an_account.rs:32, :279 (calls `state::requirement_text` directly)
- services/service-com-amazonaws-aiops-aiops/tests/all/map_member_refusal_paths.rs:143, :152

## Steps to Reproduce

1. Open services/common/src/validation_message.rs and read the doc comment on `map_value_set_requirement` (lines 2180-2183): it names `aiops-aiops` as a crate carrying its own inlined copy of the `Map value must satisfy constraint: [ … ]` sentence, to be migrated onto the shared one.
2. Open services/service-com-amazonaws-aiops-aiops/src/state.rs:688 and see `MAP_VALUE_REQUIREMENT_PREFIX`, the inlined copy, assembled with the same `join(", ")` at lines 813 and 1134.
3. Compare services/service-com-amazonaws-aiops-aiops/src/state.rs:559-573 (`requirement_text`) with services/common/src/validation_message.rs:1381-1396 (`requirement`): the same empty-path `Violation` render plus split at the clause separator, implemented twice.
4. Compare services/service-com-amazonaws-aiops-aiops/src/state.rs:884 (`MAP_KEY_REQUIREMENT_PREFIX` + bracketed join at line 1013) with services/common/src/validation_message.rs:1351-1354 and 1133-1135.
5. Note services/service-com-amazonaws-aiops-aiops/src/state.rs:609 already calls `::service_common::validation_message::envelope_over`, so the shared module is already a dependency of this very file — the remaining copies are the only parts of one grammar still spelled locally.
6. Reword either copy (e.g. change the bracket punctuation on one side) and observe that nothing fails to compile: the two spellings drift silently while services/service-com-amazonaws-aiops-aiops/tests/all/modelled_map_key_constraints.rs:232/:250 only pin this crate's side.
