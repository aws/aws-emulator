Issue-Severity: LOW
Issue-Gap-Class: maintainability
Issue-Commit: 24d9212b2d5787e53436d23b0d50a5e8ebcde596

# Derive the idempotency key in one place instead of four

_Filed: 2026-11-20T09:57:56Z_
_Operation: DeleteConnectionRecordingPreferences | Class: maintainability_

> _Note: this issue was identified against the code base at commit `24d9212b2d5787e53436d23b0d50a5e8ebcde596`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

The rule "the idempotency store is keyed by (operation, ClientToken)" is implemented four separate times in this crate: twice in the handlers (to build the LOOKUP key for the replay check) and twice inside `src/state.rs` (to build the RECORD key for the write). The read side and the write side of the same key therefore live in different modules and can drift: if key derivation ever changes (e.g. trimming or case-normalising the token, or splitting `IdempotentOperation`), a handler's lookup key and the state's record key would silently disagree, every replay would miss, and both `@idempotent` operations would mutate a second time instead of replaying — with no compile error, because `IdempotencyKey` is a bare tuple alias (`pub type IdempotencyKey = (IdempotentOperation, String);`, src/state.rs:132).

This is the one piece of idempotency policy the crate did NOT centralise, and the handlers' own comments say the rest of it deliberately was: "The cap is applied by the state API this handler writes through, not here, because both mutating operations must share one policy" (delete handler:179-181, update handler:225-227). The same argument applies to the key the cap is applied under.

Fix is a single constructor on the state module (e.g. `IdempotentOperation::key(&token)` / `fn idempotency_key(op, token) -> IdempotencyKey`) called by `put_preferences`, `remove_preferences` and both handlers. Every response and every existing test stays byte-for-byte unchanged.

## Evidence

Copy 1 — read key, Delete handler:
services/service-com-amazonaws-ssmguiconnect-ssmguiconnect/src/ops/delete_connection_recording_preferences.rs:187
    `let key = (IdempotentOperation::Delete, client_token.clone());`

Copy 2 — write key for the SAME operation+token, in the state module:
services/service-com-amazonaws-ssmguiconnect-ssmguiconnect/src/state.rs:186
    `let key = (IdempotentOperation::Delete, client_token.to_string());`

Copy 3 — read key, Update handler:
services/service-com-amazonaws-ssmguiconnect-ssmguiconnect/src/ops/update_connection_recording_preferences.rs:232
    `let key = (IdempotentOperation::Update, client_token.clone());`

Copy 4 — write key for the SAME operation+token, in the state module:
services/service-com-amazonaws-ssmguiconnect-ssmguiconnect/src/state.rs:170
    `let key = (IdempotentOperation::Update, record.client_token.clone());`

The crate already states the centralisation principle it skipped here — src/ops/delete_connection_recording_preferences.rs:179-181:
    `// at \`crate::state::MAX_IDEMPOTENCY_OUTCOMES\` with FIFO eviction ... The cap`
    `// is applied by the state API this handler writes through, not here, because`
    `// both mutating operations must share one policy.`
and src/state.rs:59-63:
    `/// Every field is private and reached through the methods below: the singleton and`
    `/// the idempotency record have to move together ...`

Existing tests that cover both sides of the key and must stay green unchanged, in services/service-com-amazonaws-ssmguiconnect-ssmguiconnect/tests/all/connection_recording_preferences.rs:
  :433 `replaying_an_update_token_returns_the_first_outcome`
  :504 `replaying_a_delete_token_returns_the_first_outcome`
  :683 `a_replay_whose_resource_changed_is_not_answered_from_the_first_outcome`
  :769 `the_idempotency_store_stays_bounded`

## Steps to Reproduce

1. Open services/service-com-amazonaws-ssmguiconnect-ssmguiconnect/src/ops/delete_connection_recording_preferences.rs and read line 187: the handler builds `(IdempotentOperation::Delete, client_token.clone())` to look a replay up.
2. Open services/service-com-amazonaws-ssmguiconnect-ssmguiconnect/src/state.rs and read line 186: `remove_preferences` builds the same key again as `(IdempotentOperation::Delete, client_token.to_string())` to record the outcome.
3. Repeat for the Update pair: src/ops/update_connection_recording_preferences.rs:232 vs src/state.rs:170.
4. Observe that `IdempotencyKey` (src/state.rs:132) is a bare tuple alias, so changing derivation on one side alone compiles cleanly while every `replayable_outcome` lookup starts missing — turning both @idempotent operations into repeat-mutating ones.
5. Introduce one shared key constructor in src/state.rs, call it from all four sites, and confirm the four tests listed in the evidence still pass with identical responses.
