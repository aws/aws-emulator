Issue-Severity: HIGH
Issue-Gap-Class: event-propagation
Issue-Commit: 794b6570c5e12ac0c4a18da454471fcedcd24ce4

# Control-plane revocations never reach the data plane's revocation registry, so GetRevocationStatus always reports nothing revoked

_Filed: 2026-11-28T07:08:42Z_
_Operation: GetRevocationStatus | Class: event-propagation_

> _Note: this issue was identified against the code base at commit `794b6570c5e12ac0c4a18da454471fcedcd24ce4`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

The data-plane handler documents that its registry is written by the Signer control plane's `RevokeSignature` / `RevokeSigningProfile`, and `state.rs` documents `ComAmazonawsSignerdataSignerdataplaneState::revoke` as "the only way a revocation enters the store". Nothing in the repository ever calls that seam outside this crate's own tests: the control-plane crate does not depend on this crate at all, and its revoke operations write only their own store. In the mounted router (`core/router/src/lib.rs:944` registers this service on the shared engine) the three registries of `ComAmazonawsSignerdataSignerdataplaneState` are therefore permanently empty, so every `GET /revocations` this endpoint serves computes an EMPTY `revokedEntities` list however many revocations the control plane has recorded — including for an artifact whose signing job was just revoked. The same request sent with the `signer` credential scope is served by the control plane's own `GetRevocationStatus`, which reads its own store and reports the revoked job ARN, so the two endpoints that this handler's doc says publish "ONE contract for this route" answer differently for the same signature. The gap is the missing propagation (or shared read) between the control plane's revoke operations and this registry, which the handler's own comment asserts already exists.

## Evidence

Handler doc asserting the wiring — `services/service-com-amazonaws-signerdata-signerdataplane/src/ops/get_revocation_status.rs:5-7`:
```
//! The data plane
//! holds no revocation of its own: the Signer CONTROL plane's `RevokeSignature` /
//! `RevokeSigningProfile` write that registry, and this read reports it.
```
and :96-99: "the two endpoints publish ONE contract for this route, so a rule stated twice could come to refuse — or admit — different requests at the two of them."

The only write seam — `services/service-com-amazonaws-signerdata-signerdataplane/src/state.rs:169-171`:
```
/// Record `record` as the revocation of `key` in the `kind` registry, returning the
/// revocation that key held before, if any. This is the only way a revocation
/// enters the store: the data plane models no write operation.
```
Nobody calls it in production: a repo-wide grep for `ComAmazonawsSignerdataSignerdataplaneState` finds it only in this crate (`src/state.rs`, `src/lib.rs:15`) and in this crate's own tests (`tests/all/revocation_status.rs:109` and `:117`); `core/router/src/lib.rs:944` only does `router.register_service(service_com_amazonaws_signerdata_signerdataplane::EmulatedService::new(engine.clone()));`.

The control plane cannot reach it — `services/service-com-amazonaws-signer-wallabyservice/src/ops/revoke_signature.rs:269` writes only its own store:
```
        job.revoke(&input.reason, revoked_by, now);
```
and `services/service-com-amazonaws-signer-wallabyservice/Cargo.toml:9-55` lists no `service-com-amazonaws-signerdata-signerdataplane` dependency (deps are chrono, engine, smithy-*, service-common, sts, iam, s3, zip, …).

The control plane's own read DOES report it — `services/service-com-amazonaws-signer-wallabyservice/src/ops/get_revocation_status.rs:354-359`:
```
        if store
            .job(&region, job_id_named)
            .is_some_and(|job| job.arn() == input.job_arn && job.is_revoked())
        {
            revoked_entities.push(input.job_arn.clone());
        }
```

Test coverage: this crate's suites populate the registry through the host seam only (`tests/all/revocation_status.rs:104-112`, "the seam that stands in for the Signer control plane's `RevokeSignature` / `RevokeSigningProfile`, since the data plane models no write"), and the cross-service suite that drives the data-plane endpoint asserts only status/shape, never the list after a revoke — `core/router/tests/all/signer_identity_authorization.rs:816-822` (`get_revocation_status(&router, AMBIENT_KEY, DATA_SIGNER, &profile_version_arn)` … `assert_eq!(status, 200, …)`). So no test pins the always-empty answer.

## Steps to Reproduce

1. Build the full router (both Signer services on one engine) as `core/router/src/lib.rs` and `core/router/tests/all/signer_identity_authorization.rs:91-113` do.
2. Create a signing profile and a signing job through the control plane (`PutSigningProfile`, then `SignPayload`/`StartSigningJob`) signed for the `signer` scope.
3. Call `RevokeSignature` on that job through the control plane — it succeeds and `revoke_signature.rs:269` records the revocation in the control plane's store.
4. Send `GET /revocations?signatureTimestamp=<an instant after the revocation>&platformId=Notation-OCI-SHA384-ECDSA&profileVersionArn=<the profile version ARN>&jobArn=<the revoked job ARN>&certificateHashes=<192 hex chars>` with the credential scope naming `data-signer`, so this crate serves it (same request shape as `signer_identity_authorization.rs:380-406`).
5. Observe HTTP 200 with `revokedEntities: []` — the revoked job is not reported, because `ComAmazonawsSignerdataSignerdataplaneState` was never written.
6. Send the identical request with the credential scope naming `signer` (control plane) and observe `revokedEntities` containing the job ARN: the two endpoints disagree about the same signature.
