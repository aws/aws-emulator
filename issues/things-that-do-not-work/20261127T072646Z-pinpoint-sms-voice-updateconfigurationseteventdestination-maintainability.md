Issue-Severity: LOW
Issue-Gap-Class: maintainability
Issue-Commit: 4b1e9c3fe42084e4e303fed95aa2e32f912a310a

# The "resolve the configuration set, else the modelled NotFoundException" rule is written out five times instead of once, unlike every other rule in this crate

_Filed: 2026-11-27T07:26:46Z_
_Operation: UpdateConfigurationSetEventDestination | Class: maintainability_

> _Note: this issue was identified against the code base at commit `4b1e9c3fe42084e4e303fed95aa2e32f912a310a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

This crate's discipline is that a rule a caller can observe lives in exactly ONE place, and the `types/` modules exist for that reason alone: `refusal_wording` is "the single home of the two sentences, so two operations cannot word one class of refusal differently", `member_form` is "the single home of one predicate per shape", `authorization` holds the one entitlement question "so the rule lives here rather than once per operation and no two of them can disagree about it", and `event_destination_records` writes the per-member destination mapping "ONCE, here, in each direction" so it "cannot be carried by one and dropped by another".

The resource-resolution refusal is the one rule that escaped that discipline. The step "look the named configuration set up in the store and, when it is absent, answer the modelled `NotFoundException` worded `configuration set {} does not exist`" is implemented verbatim in FIVE handlers (`CreateConfigurationSetEventDestination`, `UpdateConfigurationSetEventDestination`, `DeleteConfigurationSetEventDestination`, `GetConfigurationSetEventDestinations`, `DeleteConfigurationSet`), differing only in which operation's error enum wraps the payload. The second step of the two-step address — "look the destination up inside that set and, when it is absent, answer `NotFoundException` worded `event destination {} does not exist in configuration set {}`" — is implemented verbatim TWICE (`UpdateConfigurationSetEventDestination`, `DeleteConfigurationSetEventDestination`).

So the one answer a caller gets for "this name resolves to nothing" is decided in five places, and those copies can drift: a change to which modelled error that case gets, or to how it is worded, has to be made five times, and a handler that is missed diverges silently — exactly the failure mode the four single-home modules above were written to prevent (and exactly the failure mode already fixed once in this crate for the destination conversions, issue 20261115T110850Z-pinpoint-sms-voice-createconfigurationseteventdestination-maintainability).

Fix shape, which changes no response and no test: state the two refusal payloads once (a `types::` module beside `refusal_wording`, answering `NotFoundException` for "no such configuration set" and for "no such event destination in this set"), and let each handler wrap that one payload in its own `...ErrorModeled::NotFoundException` variant, the way each handler already wraps `authorization`'s single refusal and hands its nested destinations to the single `From` impl. Every sentence, status and code stays byte-identical.

## Evidence

The crate's own stated discipline, which this rule is the exception to —

services/service-com-amazonaws-pinpointsmsvoice-pinpointsmsvoice/src/ops/types/refusal_wording.rs:8-10:
    //! so the status and the code are the same for every such refusal and only the sentence
    //! differs. This module is the single home of the two sentences, so two operations cannot
    //! word one class of refusal differently.

services/service-com-amazonaws-pinpointsmsvoice-pinpointsmsvoice/src/ops/types/authorization.rs:62-63:
    /// Every handler of a resource-scoped action asks this ONE question, so the rule lives
    /// here rather than once per operation and no two of them can disagree about it.

services/service-com-amazonaws-pinpointsmsvoice-pinpointsmsvoice/src/ops/types/event_destination_records.rs:13-15:
    //! is therefore written ONCE, here, in each direction: a member added to one of those
    //! shapes, or a renamed state field, is carried by every handler that converts it or by
    //! none — it cannot be carried by one and dropped by another.

Copy 1 — src/ops/create_configuration_set_event_destination.rs:239-251:
        let set = store
            .configuration_sets
            .get_mut(&input.configuration_set_name)
            .ok_or_else(|| {
                CreateConfigurationSetEventDestinationErrorModeled::NotFoundException(
                    NotFoundException {
                        message: Some(format!(
                            "configuration set {} does not exist",
                            input.configuration_set_name
                        )),
                    },
                )
            })?;

Copy 2 — src/ops/update_configuration_set_event_destination.rs:201-213 (same block, other enum):
        let set = store
            .configuration_sets
            .get_mut(&input.configuration_set_name)
            .ok_or_else(|| {
                UpdateConfigurationSetEventDestinationErrorModeled::NotFoundException(
                    NotFoundException {
                        message: Some(format!(
                            "configuration set {} does not exist",
                            input.configuration_set_name
                        )),
                    },
                )
            })?;

Copy 3 — src/ops/delete_configuration_set_event_destination.rs:182-194 (same block, other enum):
        let set = store
            .configuration_sets
            .get_mut(&input.configuration_set_name)
            .ok_or_else(|| {
                DeleteConfigurationSetEventDestinationErrorModeled::NotFoundException(
                    NotFoundException {
                        message: Some(format!(
                            "configuration set {} does not exist",
                            input.configuration_set_name
                        )),
                    },
                )
            })?;

Copy 4 — src/ops/get_configuration_set_event_destinations.rs:180-192 (`.get` instead of `.get_mut`, same refusal):
        let set = store
            .configuration_sets
            .get(&input.configuration_set_name)
            .ok_or_else(|| {
                GetConfigurationSetEventDestinationsErrorModeled::NotFoundException(
                    NotFoundException {
                        message: Some(format!(
                            "configuration set {} does not exist",
                            input.configuration_set_name
                        )),

Copy 5 — src/ops/delete_configuration_set.rs:172-186 (`.remove`, same refusal):
        if store
            .configuration_sets
            .remove(&input.configuration_set_name)
            .is_none()
        {
            return Err(DeleteConfigurationSetErrorModeled::NotFoundException(
                NotFoundException {
                    message: Some(format!(
                        "configuration set {} does not exist",
                        input.configuration_set_name
                    )),
                },
            )
            .into());
        }

And the second step of the address, twice —

src/ops/update_configuration_set_event_destination.rs:214-226:
        let destination = set
            .event_destinations
            .get_mut(&input.event_destination_name)
            .ok_or_else(|| {
                UpdateConfigurationSetEventDestinationErrorModeled::NotFoundException(
                    NotFoundException {
                        message: Some(format!(
                            "event destination {} does not exist in configuration set {}",
                            input.event_destination_name, input.configuration_set_name
                        )),
                    },
                )
            })?;

src/ops/delete_configuration_set_event_destination.rs:198-214:
        if set
            .event_destinations
            .remove(&input.event_destination_name)
            .is_none()
        {
            return Err(
                DeleteConfigurationSetEventDestinationErrorModeled::NotFoundException(
                    NotFoundException {
                        message: Some(format!(
                            "event destination {} does not exist in configuration set {}",
                            input.event_destination_name, input.configuration_set_name
                        )),
                    },
                )
                .into(),
            );
        }

Existing tests that pin these answers and must stay unchanged by the fix:
- tests/all/configuration_set_lifecycle.rs:564-568 `event_destinations_resolve_through_their_configuration_set` ("a destination lives inside a configuration set — an unknown set is" ...)
- tests/all/event_destination_updates.rs:373-377 `an_update_or_delete_of_something_absent_is_not_found` ("both operations resolve through the configuration set that owns the destination")
- tests/all/event_destination_deletion.rs:184-195 `a_delete_removes_the_destination_only_from_the_set_that_owns_it`
- tests/all/parameter_refusal_wording.rs:737-744 `a_name_that_resolves_to_nothing_is_not_found_in_the_same_member`
- tests/all/served_operations.rs:265 (every bound operation answers its own modelled result)

## Steps to Reproduce

1. Open src/ops/create_configuration_set_event_destination.rs:239-251, src/ops/update_configuration_set_event_destination.rs:201-213, src/ops/delete_configuration_set_event_destination.rs:182-194, src/ops/get_configuration_set_event_destinations.rs:180-192 and src/ops/delete_configuration_set.rs:172-186 side by side: the same lookup, the same modelled error and the same sentence `configuration set {} does not exist` appear five times, differing only in the operation's error-enum name.
2. Open src/ops/update_configuration_set_event_destination.rs:214-226 beside src/ops/delete_configuration_set_event_destination.rs:198-214: the second step of the two-step address, with the sentence `event destination {} does not exist in configuration set {}`, appears twice.
3. Contrast with the rules this crate holds in one place: src/ops/types/authorization.rs:62-63 (one entitlement question for all seven resource-scoped handlers), src/ops/types/refusal_wording.rs:8-10 (one home for the bad-request sentences), src/ops/types/event_destination_records.rs:13-15 (the destination mapping written once in each direction).
4. Change the wording or the modelled error of "this name resolves to nothing" in one of the five copies and run `cargo test -p service-com-amazonaws-pinpointsmsvoice-pinpointsmsvoice`: the other four keep the old answer, so the service can word one class of refusal in two ways — the drift the four single-home modules were written to prevent.
