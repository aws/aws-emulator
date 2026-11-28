Issue-Severity: LOW
Issue-Gap-Class: maintainability
Issue-Commit: 0388f653d2751ec651f0d8e502033cb825c7615b

# Hoist the "registry is unavailable" InternalServerException into handler_refusal, which claims to own every refusal more than one handler answers

_Filed: 2026-11-28T03:54:23Z_
_Operation: GetRoutingControlState | Class: maintainability_

> _Note: this issue was identified against the code base at commit `0388f653d2751ec651f0d8e502033cb825c7615b`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

This crate centralises every refusal that more than one handler can answer in `src/service/handler_refusal.rs`, and that module's own documentation states the rule and then enumerates the refusals it holds as exactly three. The store-unavailable refusal is a fourth: all four handlers of this service map a missing store to `InternalServerException` with the same sentence and the same `RetryAfterSeconds`, and each one builds that structure inline instead of calling a shared constructor. So one refusal of this API is decided in four places, which is precisely what the module doc says must not happen, and the four copies can drift (one could change the sentence, the retry hint or even the modelled class while the other three keep the old answer) without any test noticing. The fix is to move the construction of the `InternalServerException` payload into `crate::service::handler_refusal` (each handler still lifts it into its own generated error enum, exactly as it does for `routing_control_not_found`) and to correct the module doc's count. No response changes and no existing test changes.

## Evidence

The rule the crate states, `src/service/handler_refusal.rs:3-18`:
```
//! The modelled refusals this API's HANDLERS construct, each shaped and worded in ONE place.
//!
//! A refusal that more than one handler answers is one refusal of this API, so its modelled
//! class, its `Reason`, which members it reports in `Fields` and the words it carries are
//! decided here and nowhere else
...
//! Three refusals of this API are reached from more than one handler, so they live here:
//!
//! * [`safety_rule_violation`] — both state updates;
//! * [`routing_control_not_found`] — the state read and both state updates;
//! * [`routing_control_state_outside_value_set`] — both state updates.
```
The fourth such refusal, built inline in each of the four handlers with identical class, sentence and retry hint:

`src/ops/get_routing_control_state.rs:199-208`:
```
        let stores = crate::state::stores(&*self.engine).ok_or_else(|| {
            GetRoutingControlStateError::from(
                GetRoutingControlStateErrorModeled::InternalServerException(
                    InternalServerException {
                        message: "the routing control registry is unavailable".to_string(),
                        retry_after_seconds: 0,
                    },
```
`src/ops/list_routing_controls.rs:281-283`:
```
                    InternalServerException {
                        message: "the routing control registry is unavailable".to_string(),
                        retry_after_seconds: 0,
```
`src/ops/update_routing_control_state.rs:240-242`:
```
                    InternalServerException {
                        message: "the routing control registry is unavailable".to_string(),
                        retry_after_seconds: 0,
```
`src/ops/update_routing_control_states.rs:257-259`:
```
                    InternalServerException {
                        message: "the routing control registry is unavailable".to_string(),
                        retry_after_seconds: 0,
```
Contrast the pattern the same handlers already follow for a shared refusal — `src/ops/update_routing_control_state.rs:298-302` wraps the central constructor and nothing else:
```
fn not_found(routing_control_arn: &str) -> UpdateRoutingControlStateError {
    UpdateRoutingControlStateErrorModeled::ResourceNotFoundException(
        crate::service::handler_refusal::routing_control_not_found(routing_control_arn),
    )
```
Test coverage: the three centralised refusals are pinned by the module's own unit tests `the_enum_refusal_states_one_requirement_whatever_member_carried_the_value` (`src/service/handler_refusal.rs:102`), `the_not_found_refusal_reports_the_arn_the_request_carried` (`:149`) and `the_safety_rule_refusal_carries_the_violation_sentence_and_names_no_member` (`:185`); the store-unavailable branch is exercised by no test because every router the suites build mounts both planes over one engine that carries both stores (`tests/all/common.rs:59-66`, `pub fn emulator() -> MultiServiceRouter`), so hoisting the constructor leaves every response and every existing test byte-identical.

## Steps to Reproduce

1. Read `src/service/handler_refusal.rs:3-18`: it states that a refusal more than one handler answers is "decided here and nowhere else" and lists three such refusals.
2. grep the crate for `InternalServerException {`: four hits, one per handler (`get_routing_control_state.rs:202`, `list_routing_controls.rs:281`, `update_routing_control_state.rs:240`, `update_routing_control_states.rs:257`), each carrying the same message "the routing control registry is unavailable" and `retry_after_seconds: 0`.
3. Note that this refusal is therefore a fourth refusal answered by more than one handler, decided in four places rather than in `handler_refusal`, and that changing the sentence in one handler would leave the other three disagreeing with no test to catch it.
