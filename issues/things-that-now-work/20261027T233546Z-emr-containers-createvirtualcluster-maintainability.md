Issue-Severity: LOW
Issue-Gap-Class: maintainability
Issue-Commit: 3d1fad8f4871dd362a3fc79086918715269856ce

# Collapse the six copies of the spent-clientToken replay/conflict rule into one helper

_Filed: 2026-10-27T23:35:46Z_
_Operation: CreateVirtualCluster | Class: maintainability_

> _Note: this issue was identified against the code base at commit `3d1fad8f4871dd362a3fc79086918715269856ce`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

The `@idempotencyToken` rule of this crate — "look the token up in the scope, and if it already names a stored resource, answer with that resource when the request fingerprint matches, otherwise refuse with the modelled `ValidationException`" — is implemented SIX times, once per handler that takes a `clientToken`. The shared half (the index lookup, `state::replayed_idempotency`, and the digest, `state::request_fingerprint`) already lives in `src/state.rs`; the DECIDING half — `.cloned()`, `.and_then(<region-scoped lookup>)`, `if existing.request_fingerprint != fingerprint { refuse } else { replay }` — is written out again in each handler.

That is one rule in six places, so the copies can drift: a handler whose copy loses the `request_fingerprint` comparison silently answers a DIFFERENT request with the earlier resource and drops the parameters that call asked for, which is exactly the failure the rule exists to prevent, and nothing outside that one handler would stop it. The crate already took the opposite approach for its other cross-operation rules — `state::page_of` for the page bounds ("One rule for every paginated list here, beside the one cursor encoder, so the two halves of the pagination contract cannot drift apart", src/state.rs:507-508), `state::within_created_bounds`, `state::within_listed_states`, `state::configured_scheduler_limits` — so this is the one shared rule left per-handler.

A fix keeps every response and message exactly as it is: the helper decides only Replay / Conflict / Fresh (it can take the already-computed fingerprint and the caller's region-scoped lookup), while each handler keeps its own wording and builds its own output shape. No response body, status or message changes, and no existing test changes.

The six copies:
- `src/ops/create_virtual_cluster.rs:216-240`
- `src/ops/create_job_template.rs:181-206`
- `src/ops/create_security_configuration.rs:179-203`
- `src/ops/create_managed_endpoint.rs:218-241`
- `src/ops/start_job_run.rs:385-406`
- `src/ops/get_managed_endpoint_session_credentials.rs:246-261`

## Evidence

Copy 1 — services/service-com-amazonaws-emrcontainers-awschicagowebservice/src/ops/create_virtual_cluster.rs:216-224:
```
        let replayed = state
            .replayed_idempotency(
                &region,
                crate::state::CREATE_VIRTUAL_CLUSTER_SCOPE,
                &input.client_token,
            )
            .cloned();
        if let Some(existing) = replayed.and_then(|id| state.virtual_cluster(&region, &id)) {
            if existing.request_fingerprint != fingerprint {
                return Err(CreateVirtualClusterErrorModeled::ValidationException(
```

Copy 2 — services/service-com-amazonaws-emrcontainers-awschicagowebservice/src/ops/create_job_template.rs:181-190 (same shape, different scope const, lookup, error enum):
```
        let replayed = state
            .replayed_idempotency(
                &region,
                crate::state::CREATE_JOB_TEMPLATE_SCOPE,
                &input.client_token,
            )
            .cloned();
        if let Some(existing) = replayed.and_then(|id| state.job_template(&region, &id)) {
            if existing.request_fingerprint != fingerprint {
                return Err(CreateJobTemplateErrorModeled::ValidationException(
```

Copies 3-6, same four steps: src/ops/create_security_configuration.rs:179-188 (`replayed.and_then(|id| state.security_configuration(&region, &id))` / `if existing.request_fingerprint != fingerprint`), src/ops/create_managed_endpoint.rs:218-224, src/ops/start_job_run.rs:385-391 (`replayed.and_then(|id| state.job_run(&region, &input.virtual_cluster_id, &id))` / `if existing.request_fingerprint != fingerprint`), src/ops/get_managed_endpoint_session_credentials.rs:246-253.

The crate states the one-place principle for its other shared rules — src/state.rs:507-508: "One rule for every paginated list here, beside the one cursor encoder, so the two halves of the pagination contract cannot drift apart." — and src/state.rs:12-16: "Two things live here beside the records because several operations need them and a second copy would let two operations disagree".

Test coverage that pins the behaviour these copies produce, and that must stay green and unchanged:
- tests/all/spent_token_with_a_different_request.rs (header, lines 12-24: "a `clientToken` names ONE request ... one carrying different parameters is refused with the operation's modelled `ValidationException` (HTTP 400)"), tabled over four of them — StartJobRun (line 136), CreateVirtualCluster (line 226), CreateManagedEndpoint (line 290), CreateSecurityConfiguration (line 368).
- tests/all/job_template_idempotency.rs (line 15: "A `clientToken` already spent on this operation does not mint a second resource") for CreateJobTemplate.
- tests/all/managed_endpoint_session_credentials.rs (RULE 5, lines 503-614) for GetManagedEndpointSessionCredentials.

## Steps to Reproduce

1. Open services/service-com-amazonaws-emrcontainers-awschicagowebservice/src/ops/create_virtual_cluster.rs at line 216 and read the four-step replay block (lookup via state::replayed_idempotency, .cloned(), region-scoped resource lookup, `if existing.request_fingerprint != fingerprint` refuse / else return the stored resource).
2. Compare it with the same four steps at src/ops/create_job_template.rs:181, src/ops/create_security_configuration.rs:179, src/ops/create_managed_endpoint.rs:218, src/ops/start_job_run.rs:385 and src/ops/get_managed_endpoint_session_credentials.rs:246 — six copies of one rule, differing only in the scope, the lookup, the message and the output shape.
3. Note that src/state.rs only shares the lookup (replayed_idempotency, line 2775) and the digest (request_fingerprint, line 1362), while the deciding branch is per-handler, so deleting the fingerprint comparison in any ONE handler reintroduces 'a spent token answers a different request' for that operation alone.
4. Run `cargo test -p service-com-amazonaws-emrcontainers-awschicagowebservice` and confirm tests/all/spent_token_with_a_different_request.rs, tests/all/job_template_idempotency.rs and tests/all/managed_endpoint_session_credentials.rs pass; they must pass unchanged after the copies are collapsed into one helper.
