Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 1ade082ac7b155d62ba7589cf4124913a2bd40fe

# An endpoint created without revisionWeights is pointed at the newest revision whatever its state, so a create made after a failed build publishes an endpoint that serves nothing

_Filed: 2026-11-27T00:45:24Z_
_Operation: CreateWebFunctionEndpoint | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `1ade082ac7b155d62ba7589cf4124913a2bd40fe`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`CreateWebFunctionEndpoint` must supply `revisionWeights` itself when the request omits them, because every endpoint this service reports carries a `@required` non-empty `revisionWeights`. The helper that does it, `revision_weights_for`, takes `function.latest_revision()` and weights it 100 — with NO test of that revision's state. `WebFunction::latest_revision()` returns whatever `latest_revision_id` names, and `latest_revision_id` is set by every create, including one whose build failed.

Two things in this crate contradict that choice:

1. The helper's own doc comment justifies the fallback by claiming it is "the one an `autoDeploymentMode` of `LatestRevision` would move it onto anyway". It is not: `WebFunction::deploy_latest_revision`, which is what that mode does, selects only revisions whose `state == RevisionState::Active`. Whenever the newest revision is `Pending` or `Failed`, the fallback and the mode pick different revisions — so for a `Disabled` endpoint (the modelled default) the create picks a revision the mode would refuse to pick, and for a `LatestRevision` endpoint the create's own 202 response reports the failed revision as the endpoint's weight while the very next call replaces it with the newest Active one.

2. The sibling `UpdateWebFunctionEndpoint` refuses exactly this situation. When a request asks for `autoDeploymentMode: LatestRevision`, that handler calls `crate::state::latest_revision_switch_conflict` and answers 409 ConflictException ("Only revisions in Active state can be deployed. Create a new revision."). The create path never calls that helper at all — neither in its pre-flight block nor in the block under the deciding guard — so a create that asks for the same mode while the newest revision is `Failed` is answered 202.

The consequence of (1) is silent and terminal: the crate's own data plane serves only a revision it finds `Active`, so the endpoint the create just published answers nothing for every request, while `GetWebFunctionEndpoint` keeps reporting it `Active` with the failed revision as its only weight. A `Failed` revision never leaves that state (`Revision::materialise_due` only moves a revision out of `Pending`), and a `Disabled` endpoint is never re-pointed by the clock, so the endpoint never recovers.

This is the create-path twin of the already-filed UpdateWebFunctionEndpoint/validation gap, reached through the IMPLICIT weight default rather than through weights the caller named, and it additionally skips the mode-switch conflict the update applies.

## Evidence

The fallback, with no state test — services/service-com-amazonaws-lambdaweb-lambdaweb/src/ops/create_web_function_endpoint.rs:306-318:
```
    let Some(requested) = requested else {
        let Some(latest) = function.latest_revision() else {
            return Err(validation(format!(
                "The revisionWeights parameter is required because web function {} has no \
                 revisions: an endpoint serves at least one revision.",
                function.function_name
            )));
        };
        return Ok(vec![RevisionWeight {
            revision_id: latest.revision_id.clone(),
            weight: crate::state::FULL_WEIGHT,
        }]);
    };
```

Its own doc comment, which states the rule the code breaks — create_web_function_endpoint.rs:289-293:
```
/// `revisionWeights` is optional on the request and `@required` with at least one
/// entry on every endpoint this service reports, so an endpoint created without one
/// serves the function's newest revision with all of the traffic — the same revision
/// `CreateWebFunction`'s initial endpoint is pointed at, and the one an
/// `autoDeploymentMode` of `LatestRevision` would move it onto anyway.
```

What that mode actually moves an endpoint onto — services/service-com-amazonaws-lambdaweb-lambdaweb/src/state.rs:422-431 (`deploy_latest_revision`):
```
        let Some(newest) = self
            .revisions
            .values()
            .filter(|revision| revision.state == RevisionState::Active)
            .max_by_key(|revision| (revision.created_at, revision.revision_id.clone()))
```
and what the fallback reads instead — state.rs:384-388:
```
    pub fn latest_revision(&self) -> Option<&Revision> {
        self.latest_revision_id
            .as_ref()
            .and_then(|id| self.revisions.get(id))
    }
```

The refusal the SIBLING applies for the same mode switch — services/service-com-amazonaws-lambdaweb-lambdaweb/src/ops/update_web_function_endpoint.rs:350-354:
```
            if switches_to_latest_revision(endpoint, input.auto_deployment_mode.as_deref()) {
                if let Some(message) = crate::state::latest_revision_switch_conflict(function) {
                    return Err(conflict(message));
                }
            }
```
with the rule stated at services/service-com-amazonaws-lambdaweb-lambdaweb/src/state/endpoints.rs:449-454: "only a revision the service reports `Active` is \"ready to serve traffic\" (`RevisionState`) — so putting an endpoint into that mode while the newest revision is still building, or while its build failed, would route the endpoint's traffic onto a revision that cannot take it: a conflict with the current state of the resource rather than a change the service can make."

The create path's two refusal blocks, which never call it — create_web_function_endpoint.rs:407-425 and :452-467:
```
            let function = require_function(&store, &function_name, &input.function_name)?;
            refuse_if_taken(function, &input.endpoint_name)?;
            refuse_if_at_the_quota(function)?;
            revision_weights_for(function, input.revision_weights.as_deref())?;
```
(`grep` shows `latest_revision_switch_conflict` has exactly one caller: update_web_function_endpoint.rs:351.)

What the published endpoint then serves — services/service-com-amazonaws-lambdaweb-lambdaweb/src/state/endpoint_plane.rs:682-685:
```
        .and_then(|revision_id| function.revisions.get(revision_id))
        // "Active: the revision is ready to serve traffic" (`RevisionState`), so a revision
        // that is still building or whose build failed takes none.
        .filter(|revision| revision.state == RevisionState::Active)
```

Smithy, for what the states mean (`smithy_query(query="shape", shape="RevisionState")` as quoted in state/endpoints.rs:450-451 and endpoint_plane.rs:683): `Active` – the revision is ready to serve traffic; `Failed` – the revision build failed.

Test coverage: every row of tests/all/web_function_endpoint.rs:908-979 (`create_cases`) omits `revisionWeights` and relies on this fallback, but each is created on a function whose only revision builds successfully, so no row exercises a non-Active latest revision. tests/all/revision_build_outcome.rs:371-461 (`an_auto_deploying_endpoint_switches_onto_a_built_revision_and_never_onto_a_failed_one`) asserts that "a revision whose build failed must take no traffic" — for endpoints that ALREADY exist; it never creates a new endpoint while the failed revision is the newest. Nothing pins the acceptance.

## Steps to Reproduce

1. Start the emulator with an object store; stage a readable zip at s3://<bucket>/deploy-good.zip and a non-archive object at s3://<bucket>/deploy-bad.zip
2. PUT /2025-03-07/web-functions with {"functionName":"f","revisionConfig":{"buildConfig":{..."s3Object" -> deploy-good.zip},"serviceConfig":{"executionRoleArn":"..."}}} (no endpointConfig)
3. POST /2025-03-07/web-functions/f/revisions naming deploy-bad.zip; poll GET /2025-03-07/web-functions/f/revisions/{thatRevisionId} until it reports state "Failed" — it is now the function's newest revision
4. PUT /2025-03-07/web-functions/f/endpoints with {"endpointName":"ep","endpointType":"HomeRegion","authType":"ApplicationManaged"} — no revisionWeights, autoDeploymentMode left at its default Disabled
5. Observe 202 with revisionWeights naming the FAILED revision at weight 100, instead of a refusal or the newest Active revision
6. GET /2025-03-07/web-functions/f/endpoints/ep and observe state "Active" with the failed revision as the only weight; issue any HTTP request to the reported domainName and observe that no handler response is served — permanently, since a Failed revision is terminal and a Disabled endpoint is never re-pointed
7. Repeat step 4 with "autoDeploymentMode":"LatestRevision" and observe 202, where PATCH /2025-03-07/web-functions/f/endpoints/<an existing endpoint> with {"autoDeploymentMode":"LatestRevision"} in the same store answers 409 ConflictException "... Only revisions in Active state can be deployed. Create a new revision."
