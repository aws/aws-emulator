Issue-Severity: LOW
Issue-Gap-Class: maintainability
Issue-Commit: 7025894f4ed0ee1d4db632d0ee60b8e35b979463

# The "map the AlternateContactType or refuse it" decision is written out three times instead of once beside its sibling member rules in state.rs

_Filed: 2026-11-28T03:01:11Z_
_Operation: PutAlternateContact | Class: maintainability_

> _Note: this issue was identified against the code base at commit `7025894f4ed0ee1d4db632d0ee60b8e35b979463`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

This crate states each rule the model cannot express ONCE in `state.rs`, as a function that returns the decision and the refusal together, and lets every handler that needs it keep only the mapping into its own operation's error enum. `region_opt_target` does that for the Region-opt-target rule (`EnableRegion`, `DisableRegion`) and `validated_contact_information` does it for the primary-contact address rules (`PutContactInformation`).

The alternate-contact-type rule is the exception: the pairing of the decision (`AlternateContactType::from_wire`) with its refusal (`invalid_alternate_contact_type`) is written out three times, once in each of `PutAlternateContact`, `GetAlternateContact` and `DeleteAlternateContact`. Both halves already live in `state.rs`; what is duplicated is the branch that joins them, so the three copies can drift — one handler could be changed to admit a value the others refuse, or to word/populate the refusal differently, and nothing in `state.rs` would state the rule any more.

The fix is the shape the two siblings already have: one `state.rs` function returning `Result<AlternateContactType, ValidationException>`, with each handler keeping only `.map_err(<Op>ErrorModeled::ValidationException)`. No response and no existing test changes: the decision, the refusal payload and the order in which each handler takes it all stay exactly as they are.

## Evidence

The convention this crate states for a rule the model cannot express — services/service-com-amazonaws-account-account/src/state.rs:682-694:

```
/// The ONE statement of that rule. `EnableRegion` and `DisableRegion` both
/// decide through it — after the account-target rule, ... — so which Regions count
/// as opt targets, and which refusal every other case earns, changes here alone
/// and cannot drift between the two operations.
pub fn region_opt_target(region_name: &str) -> Result<(), ValidationException> {
```

and its second instance, state.rs:301-308:

```
/// The ONE statement of the rules the model cannot express about a
/// `ContactInformation` — the counterpart of [`region_opt_target`] — so
/// `PutContactInformation` decides through it ...
pub fn validated_contact_information(
    contact: &ContactInformation,
) -> Result<(), ValidationException> {
```

The same decision written out three times instead. src/ops/put_alternate_contact.rs:199-206:

```
        let Some(contact_type) =
            crate::state::AlternateContactType::from_wire(&input.alternate_contact_type)
        else {
            return Err(PutAlternateContactErrorModeled::ValidationException(
                crate::state::invalid_alternate_contact_type(&input.alternate_contact_type),
            )
            .into());
        };
```

src/ops/get_alternate_contact.rs:174-181:

```
        let Some(contact_type) =
            crate::state::AlternateContactType::from_wire(&input.alternate_contact_type)
        else {
            return Err(GetAlternateContactErrorModeled::ValidationException(
                crate::state::invalid_alternate_contact_type(&input.alternate_contact_type),
            )
            .into());
        };
```

src/ops/delete_alternate_contact.rs:193-200:

```
        let Some(contact_type) =
            crate::state::AlternateContactType::from_wire(&input.alternate_contact_type)
        else {
            return Err(DeleteAlternateContactErrorModeled::ValidationException(
                crate::state::invalid_alternate_contact_type(&input.alternate_contact_type),
            )
            .into());
        };
```

Both halves the copies join already live in one place — state.rs:609-616 (`AlternateContactType::from_wire`) and state.rs:1881-1891 (`invalid_alternate_contact_type`, whose own doc says "`reason` is `fieldValidationFailed` ... and `fieldList` names the member").

Test coverage that must stay unchanged: tests/all/enum_input_validation.rs:348-362, `the_alternate_contact_type_refuses_a_value_the_enum_does_not_admit`, which drives all three operations ("`AlternateContactType` refuses a value the enum does not admit, on every operation that carries it"); the success and 404 paths are pinned by tests/all/alternate_contact_delete_precedence.rs:278-324 and tests/all/absent_subresource_is_not_found.rs:1-27.

## Steps to Reproduce

1. Open services/service-com-amazonaws-account-account/src/state.rs:682-694 and :301-308 and read how this crate states a rule the model cannot express: one function per rule, returning the decision together with its `ValidationException`.
2. Grep the handlers for the alternate-contact-type rule: `rg -n 'AlternateContactType::from_wire' services/service-com-amazonaws-account-account/src/ops` — three hits, in put_alternate_contact.rs:200, get_alternate_contact.rs:175 and delete_alternate_contact.rs:194, each followed by its own copy of the same refusal branch.
3. Observe that no function in state.rs pairs `from_wire` with `invalid_alternate_contact_type`, so the rule "a value the enum does not admit earns this refusal" exists only as three copies inside the handlers.
4. Confirm the copies are interchangeable today: tests/all/enum_input_validation.rs:348-362 drives the identical refusal over all three operations, so collapsing them into one state.rs function mapped per operation leaves every response and every test unchanged.
