Issue-Severity: LOW
Issue-Gap-Class: maintainability
Issue-Commit: 73d9a57b34e2ffde7302e21028c70d385213f0b9

# Centralise the pagination-token refusal sentence the two listings each spell for themselves

_Filed: 2026-11-27T17:55:57Z_
_Operation: ListConnectors | Class: maintainability_

> _Note: this issue was identified against the code base at commit `73d9a57b34e2ffde7302e21028c70d385213f0b9`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

This crate deliberately keeps every refusal sentence it words in ONE place in `src/state.rs`, so two operations that state the same rule cannot drift into two sentences — `resources_not_active_message`, `resource_cannot_be_in_state_message`, `challenge_connector_type_message` and `tags_cannot_be_changed_message` are all built there, each documented as "the one place this service words that refusal". The refusal of a `NextToken` this service never minted is the single exception: the same rule (decode the cursor for this listing's scope, and word a decode failure as a `ValidationException` with `reason: None` and the sentence "Invalid pagination token: it was not issued by this service") is written out twice, once in each listing handler. The two copies can drift — one could gain a `Reason`, or be reworded — and nothing in the crate would catch it, because the existing tests assert only the status and the error code. The fix is to state the sentence (and the `MalformedCursor` -> message mapping) once in `src/state.rs` beside its sibling builders and have each handler lift it into its own error enum, exactly as both tagging writes already do with `tag_write_refusal`. Every response byte and every existing test stays unchanged.

## Evidence

Copy 1 — services/service-com-amazonaws-pcaconnectorscep-pcaconnectorscep/src/ops/list_connectors.rs:171-179:
```
        let scope = crate::state::cursor_scope_connectors(&region);
        let after = crate::state::decode_cursor(&scope, input.next_token.as_deref())
        .map_err(|_| {
            ListConnectorsErrorModeled::ValidationException(ValidationException {
                message: "Invalid pagination token: it was not issued by this service"
                    .to_string(),
                reason: None,
            })
        })?;
```
Copy 2 — services/service-com-amazonaws-pcaconnectorscep-pcaconnectorscep/src/ops/list_challenge_metadata.rs:183-190:
```
        let scope = crate::state::cursor_scope_challenges(&input.connector_arn);
        let after = crate::state::decode_cursor(&scope, input.next_token.as_deref()).map_err(|_| {
            ListChallengeMetadataErrorModeled::ValidationException(ValidationException {
                message: "Invalid pagination token: it was not issued by this service"
                    .to_string(),
                reason: None,
            })
        })?;
```
The crate's own stated convention for exactly this kind of sentence — services/service-com-amazonaws-pcaconnectorscep-pcaconnectorscep/src/state.rs:941-947:
```
/// The one place this service words that refusal, for every operation that has such a
/// precondition — `CreateConnector` over the `CertificateAuthorityArn` it links, and
/// `CreateChallenge` over the connector it mints a password under — so the two cannot state
/// one precondition in two sentences.
pub fn resources_not_active_message(resource_arn: &str) -> String {
```
and src/state.rs:930-935:
```
/// Every builder below states only its own clause and reaches the scaffold through here, so a
/// clause cannot be worded into a sentence that opens or closes differently from its siblings'
/// — whichever precondition, and whichever operation, refused the call.
fn precondition_message(clause: impl ::std::fmt::Display) -> String {
```
Test coverage that must stay green and does not read the sentence (so the move is response-neutral): tests/all/connector_lifecycle.rs:1234 `unminted_pagination_token_is_refused` and tests/all/connector_lifecycle.rs:1279 `pagination_token_of_another_listing_is_refused`, which assert only `answer.status == 400` and `answer.error_type == "ValidationException"` (connector_lifecycle.rs:1260-1270 and :1334-1339).

## Steps to Reproduce

1. Open services/service-com-amazonaws-pcaconnectorscep-pcaconnectorscep/src/ops/list_connectors.rs and read lines 171-179.
2. Open services/service-com-amazonaws-pcaconnectorscep-pcaconnectorscep/src/ops/list_challenge_metadata.rs and read lines 183-190: the same decode-and-word rule, with the same sentence and the same `reason: None`, written a second time.
3. Compare with services/service-com-amazonaws-pcaconnectorscep-pcaconnectorscep/src/state.rs:930-1008, where every other refusal sentence of this service is built once and documented as the one place it is worded.
4. Reword or add a `Reason` to one copy only: `cargo test -p service-com-amazonaws-pcaconnectorscep-pcaconnectorscep` still passes, because tests/all/connector_lifecycle.rs:1260-1270 and :1334-1339 assert only the status and the error code — nothing holds the two listings to one sentence.
