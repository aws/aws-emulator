Issue-Severity: LOW
Issue-Gap-Class: maintainability
Issue-Commit: a668be1c2b7e82cad4ee1de57e0ed4f21d5374ff

# Build the Update response from a stored record in one place, as the state module already claims it is

_Filed: 2026-11-27T15:16:07Z_
_Operation: UpdateConnectionRecordingPreferences | Class: maintainability_

> _Note: this issue was identified against the code base at commit `a668be1c2b7e82cad4ee1de57e0ed4f21d5374ff`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`crate::state::RecordingPrefsRecord::to_preferences` is documented as "The ONE place a response is built from the record, so Update and Get cannot report the same record differently", but it only owns HALF of that mapping: it converts the preferences, while the other member of the same response — `ClientToken`, taken from the record rather than from the request — is spelled out again at each response site. The Update handler therefore carries the full record->output mapping TWICE in one file (the idempotent-replay branch and the fresh-write branch), and the Get handler carries a third copy of the same two-member derivation for its own output shape.

The two copies inside `update_connection_recording_preferences.rs` are the drift risk the state module's doc says it has removed: they are the same computation (both members wrapped in `Some`, `client_token` read off the record and not off `input.client_token`), so a later change to what a response reports for a stored record has to be made in both branches or the replayed answer and the fresh answer stop agreeing — exactly the divergence `to_preferences` exists to prevent.

A fix is a single `RecordingPrefsRecord`-to-output constructor (or one local helper in the Update handler) that both branches call. No response byte and no existing test changes: the two copies are textually identical today.

## Evidence

THE RULE THE CRATE STATES FOR ITSELF — services/service-com-amazonaws-ssmguiconnect-ssmguiconnect/src/state.rs:398-400
    `/// The wire shape for what is stored. The ONE place a response is built from`
    `/// the record, so Update and Get cannot report the same record differently.`
    `pub fn to_preferences(&self) -> ConnectionRecordingPreferences {`

COPY 1 — the idempotent-replay branch, services/service-com-amazonaws-ssmguiconnect-ssmguiconnect/src/ops/update_connection_recording_preferences.rs:267-271
    `if let Some(StoredResponse::Updated(record)) = store.replayable_outcome(&key) {`
    `    let replayed = UpdateConnectionRecordingPreferencesOutput {`
    `        client_token: Some(record.client_token.clone()),`
    `        connection_recording_preferences: Some(record.to_preferences()),`
    `    };`

COPY 2 — the fresh-write branch of the SAME handler, services/service-com-amazonaws-ssmguiconnect-ssmguiconnect/src/ops/update_connection_recording_preferences.rs:305-308
    `Ok(UpdateConnectionRecordingPreferencesOutput {`
    `    client_token: Some(record.client_token.clone()),`
    `    connection_recording_preferences: Some(record.to_preferences()),`
    `})`

THE THIRD SITE, for Get's own output shape — services/service-com-amazonaws-ssmguiconnect-ssmguiconnect/src/ops/get_connection_recording_preferences.rs:203-206
    `Ok(GetConnectionRecordingPreferencesOutput {`
    `    client_token: Some(record.client_token.clone()),`
    `    connection_recording_preferences: Some(record.to_preferences()),`
    `})`

TEST COVERAGE that pins both Update branches and the Get answer, and that a single constructor leaves unchanged:
- services/service-com-amazonaws-ssmguiconnect-ssmguiconnect/tests/all/connection_recording_preferences.rs:176 `async fn update_then_get_reports_every_stored_member()`
- services/service-com-amazonaws-ssmguiconnect-ssmguiconnect/tests/all/connection_recording_preferences.rs:297 `async fn a_later_update_replaces_the_stored_record()`
- services/service-com-amazonaws-ssmguiconnect-ssmguiconnect/tests/all/connection_recording_preferences.rs:433 `async fn replaying_an_update_token_returns_the_first_outcome()`
- services/service-com-amazonaws-ssmguiconnect-ssmguiconnect/tests/all/connection_recording_preferences.rs:504 `async fn replaying_a_delete_token_returns_the_first_outcome()`

## Steps to Reproduce

1. Open services/service-com-amazonaws-ssmguiconnect-ssmguiconnect/src/state.rs:398-414 and read the claim that `to_preferences` is "The ONE place a response is built from the record".
2. Open services/service-com-amazonaws-ssmguiconnect-ssmguiconnect/src/ops/update_connection_recording_preferences.rs and compare lines 268-271 with lines 305-308: the record->output mapping (both members, `client_token` taken off the record) is written out twice in the same handler.
3. Change what a response reports for a stored record in only one of the two branches (for example report the request's token instead of the record's) and observe that nothing in the crate forces the other branch to agree — the replayed answer and the fresh answer can then differ for the same record.
4. Confirm the duplicated text is identical today, so collapsing both branches onto one constructor changes no response and no test named in the evidence.
