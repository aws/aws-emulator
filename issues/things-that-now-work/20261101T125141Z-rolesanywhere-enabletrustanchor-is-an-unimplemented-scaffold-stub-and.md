Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: rolesanywhere.trust-anchor-delete-removes-resource
Issue-Shape-Id: com.amazonaws.rolesanywhere#RolesAnywhere
Issue-Key: rolesanywhere.trust-anchor-delete-removes-resource|run#15|AssertionError|assert mutation_error["Code"] == "ResourceNotFoundException", (
Issue-Commit: 733fcacb825dbc0b76bb386a5407a251c14222ac

# rolesanywhere EnableTrustAnchor is an unimplemented scaffold stub and answers 501 NotImplemented

> _Note: this issue was identified against the code base at commit `733fcacb825dbc0b76bb386a5407a251c14222ac`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
EnableTrustAnchor

## Description
Cause 1 of 1: `EnableTrustAnchor` is not implemented in the rolesanywhere emulator. Every call to it — on a live trust anchor id or on a deleted one — is answered with the scaffold's HTTP 501 `NotImplemented` instead of the modeled success or the modeled 404 refusal.

AWS: Smithy model (services/service-com-amazonaws-rolesanywhere-rolesanywhere/smithy-model/com-amazonaws-rolesanywhere-rolesanywhere.json) binds `EnableTrustAnchor` to `POST /trustanchor/{trustAnchorId}/enable`, success code 200, input `ScalarTrustAnchorRequest` with required `trustAnchorId` (Uuid, @length 36..36, pattern `[a-f0-9]{8}-([a-z0-9]{4}-){3}[a-z0-9]{12}`, @httpLabel), output `TrustAnchorDetailResponse` with required member `trustAnchor` (TrustAnchorDetail). Modeled errors: `AccessDeniedException` (HTTP 403) and `ResourceNotFoundException` (HTTP 404 — "The resource could not be found."). Doc: "Enables a trust anchor. When enabled, certificates in the trust anchor chain are authorized for trust validation." So on a trust anchor id that no longer exists (after `DeleteTrustAnchor`), AWS refuses with HTTP 404, error code `ResourceNotFoundException`. Recorded behavior for the message: the recording asserts the fixed text `TrustAnchor not found` (the whole message is fixed text for this error class in this service — it carries no value from the request or state; the sibling `GetTrustAnchor` refusal in the same recorded check uses that identical wording, and the emulator already satisfies it there).

Emulator: the handler body is the untouched generated stub — `Err(crate::scaffolded::Scaffolded::not_implemented("EnableTrustAnchor"))` — so the response is HTTP 501 with error code `NotImplemented`. Replay: `assert mutation_error["Code"] == "ResourceNotFoundException"` ... `AssertionError: expected ResourceNotFoundException from EnableTrustAnchor on a deleted id, got 'NotImplemented'`. Note the preceding steps of the same check pass: `CreateTrustAnchor`, `GetTrustAnchor`, `ListTrustAnchors`, `DeleteTrustAnchor` and the `GetTrustAnchor` 404-after-delete (code and message) all behave, so this is isolated to the `EnableTrustAnchor` handler.

Why: a client cannot re-enable a trust anchor at all, and cannot distinguish a deleted/unknown trust anchor from a transport-level server fault. `NotImplemented`/501 is not in the operation's modeled error set, so botocore raises a generic `ClientError` the caller's `ResourceNotFoundException` branch never matches; retry logic treats 501 as non-retryable server failure rather than the permanent not-found it is, and enable/disable lifecycle flows (and any code that probes liveness of a deleted id with a mutation) break.

Fix: implement the `EnableTrustAnchor` handler at services/service-com-amazonaws-rolesanywhere-rolesanywhere/src/ops/enable_trust_anchor.rs — look the id up in the service state via `crate::state::service_state`, return `ResourceNotFoundException` (404, message `TrustAnchor not found`) when absent, otherwise set the stored trust anchor's `enabled` flag true, bump its `updatedAt`, and serialize the full `TrustAnchorDetail` in `trustAnchor`; then delete the `//! SCAFFOLDED, NOT IMPLEMENTED:` marker at the top of that file. The wording of the refusal belongs with the existing open issue 20261029T192145Z-rolesanywhere-not-found-refusals-carry-emulator-authored-messages.md (AWS not-found wording) — that issue does NOT cover this operation, which never reaches a refusal at all because it is a stub; the sibling stub issues on file (DisableTrustAnchor, UpdateTrustAnchor, DisableProfile, DisableCrl, UpdateCrl, UntagResource, ListTagsForResource, PutAttributeMapping) likewise do not name `EnableTrustAnchor`.

Traces: rolesanywhere.trust-anchor-delete-removes-resource (assertion at check_rolesanywhere_trust_anchor_delete_removes_resource.py line 49).

## Evidence
Expected (AWS / recorded): EnableTrustAnchor on a deleted trustAnchorId -> HTTP 404, Error.Code = "ResourceNotFoundException", Error.Message = "TrustAnchor not found" (message is entirely fixed text for this error class). EnableTrustAnchor on a live trustAnchorId -> HTTP 200 with required member `trustAnchor` (TrustAnchorDetail) and `enabled` = true.

Actual (emulator): HTTP 501, Error.Code = "NotImplemented". Replay failure:
  File "check_rolesanywhere_trust_anchor_delete_removes_resource.py", line 49, in run
    assert mutation_error["Code"] == "ResourceNotFoundException"
  AssertionError: expected ResourceNotFoundException from EnableTrustAnchor on a deleted id, got 'NotImplemented'

Source: services/service-com-amazonaws-rolesanywhere-rolesanywhere/src/ops/enable_trust_anchor.rs line 4 (`//! SCAFFOLDED, NOT IMPLEMENTED:`) and line 155 (`Err(crate::scaffolded::Scaffolded::not_implemented("EnableTrustAnchor"))`).

## Steps to Reproduce
1. Boot the emulator and point a botocore rolesanywhere client at it.
2. CreateTrustAnchor(name=<unique>, source={sourceType: CERTIFICATE_BUNDLE, sourceData: {x509CertificateData: <self-signed CA PEM>}}) and capture trustAnchorId.
3. DeleteTrustAnchor(trustAnchorId=<that id>).
4. Call EnableTrustAnchor(trustAnchorId=<that id>).
   Observed: HTTP 501 with Error.Code "NotImplemented".
   Expected: HTTP 404 with Error.Code "ResourceNotFoundException" and Error.Message "TrustAnchor not found".
5. Also: create a fresh trust anchor and call EnableTrustAnchor on the LIVE id — observed 501 NotImplemented; expected HTTP 200 carrying trustAnchor.enabled = true.
