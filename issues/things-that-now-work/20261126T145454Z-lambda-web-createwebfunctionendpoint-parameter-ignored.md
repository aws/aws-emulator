Issue-Severity: MEDIUM
Issue-Gap-Class: parameter-ignored
Issue-Commit: df4bbbc9513f0b2cf967fd54f27fb98fd6606b7d

# An endpoint created with autoDeploymentMode LatestRevision keeps the requested revisionWeights only until the next call, which silently replaces them

_Filed: 2026-11-26T14:54:54Z_
_Operation: CreateWebFunctionEndpoint | Class: parameter-ignored_

> _Note: this issue was identified against the code base at commit `df4bbbc9513f0b2cf967fd54f27fb98fd6606b7d`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`CreateWebFunctionEndpoint` validates the request's `revisionWeights` (404 for a revision the function does not have, 400 for a revision named twice), stores them through `ComAmazonawsLambdawebLambdawebState::add_endpoint`, and reports them in its 202 response. Unlike the sibling write path, `add_endpoint` never runs the auto-deployment switch: `update_endpoint` calls `function.deploy_latest_revision()` right after applying a change, precisely so that an endpoint in the `LatestRevision` mode is routed "at once instead of at whatever call comes next", and `add_endpoint` has no such call.

The switch is nevertheless owed to that endpoint, because `WebFunction::materialise_due` calls `deploy_latest_revision()` on every function and every handler calls `materialise_due_transitions` first. So for an endpoint created with `autoDeploymentMode: "LatestRevision"` and explicit `revisionWeights` that are not exactly `[{newest Active revision, 100}]`, the VERY NEXT request to this service (including a plain `GetWebFunctionEndpoint`) rewrites the stored weights to `[{newest, 100}]`. The create answered 202 reporting weights the service does not keep, so the create's response contradicts every later read of the same endpoint — which is exactly what the handler's own comment above the response claims cannot happen ("what this create reports is what the endpoint readers of this service report for the same endpoint"). Either the mode should be applied where the endpoint is stored (so the create reports what it will serve) or the requested weights should be refused/kept; today the validated parameter is accepted, echoed, and then dropped with nothing in the response saying so.

This is the create-path counterpart of the already-filed UpdateWebFunctionEndpoint/parameter-ignored defect, but it is reached through a different handler and a different state helper (`add_endpoint`, which has no `deploy_latest_revision` call at all), so it survives any fix confined to `update_endpoint`.

## Evidence

The update path takes the switch where it writes, services/service-com-amazonaws-lambdaweb-lambdaweb/src/state/endpoints.rs:639-642 + 656-657:
```
    /// An endpoint left in the [`AUTO_DEPLOYMENT_MODE_LATEST_REVISION`] mode serves the
    /// newest revision that has built, so the switch the clock owes every endpoint in
    /// that mode is taken here too — an update that turns the mode on routes the
    /// endpoint onto that revision at once instead of at whatever call comes next.
...
        endpoint.apply(update, now);
        function.deploy_latest_revision();
```

The create path stores the requested weights and never takes it, endpoints.rs:615-618 (`add_endpoint`):
```
            auto_deployment_mode: new
                .auto_deployment_mode
                .unwrap_or_else(|| DEFAULT_AUTO_DEPLOYMENT_MODE.to_string()),
            revision_weights: new.revision_weights,
```
(the whole function, endpoints.rs:593-634, contains no `deploy_latest_revision` call)

The claim the create's response makes, services/service-com-amazonaws-lambdaweb-lambdaweb/src/ops/create_web_function_endpoint.rs:500-501 and :530:
```
        // Every member below comes out of the stored record, so what this create reports
        // is what the endpoint readers of this service report for the same endpoint.
...
            revision_weights: endpoint.revision_weights.clone(),
```

What the next call does to the stored record, services/service-com-amazonaws-lambdaweb-lambdaweb/src/state.rs:433-445 (`deploy_latest_revision`), reached from `WebFunction::materialise_due` (state.rs:409 `self.deploy_latest_revision();`) which `materialise_due_transitions` (state.rs:501-505) runs for every handler:
```
        for endpoint in self.endpoints.values_mut() {
            if endpoint.auto_deployment_mode != AUTO_DEPLOYMENT_MODE_LATEST_REVISION {
                continue;
            }
            let serving_only_newest = endpoint.revision_weights.len() == 1
                && endpoint.revision_weights[0].revision_id == revision_id;
            if serving_only_newest {
                continue;
            }
            endpoint.revision_weights = vec![crate::ops::types::RevisionWeight {
                revision_id: revision_id.clone(),
                weight: FULL_WEIGHT,
            }];
```

Test coverage: the create rows that send explicit weights never name `autoDeploymentMode` (so they run in the default `Disabled` mode) — tests/all/web_function_endpoint.rs:1243-1272 (`revision_weights_name_the_functions_own_revisions`, request body at :1246-1251 carries only `endpointName`/`endpointType`/`authType`/`revisionWeights`, and the assertion at :1266-1271 is what passes today) — while the rows that DO send `"autoDeploymentMode": "LatestRevision"` send no weights (tests/all/web_function_endpoint.rs:177-193, :930-936). The combination is unpinned.

## Steps to Reproduce

1. PUT /2025-03-07/web-functions with a functionName and a revisionConfig, creating revision R1; wait until R1 reports Active
2. POST /2025-03-07/web-functions/{functionName}/revisions to create revision R2; wait until R2 reports Active
3. PUT /2025-03-07/web-functions/{functionName}/endpoints with {"endpointName":"canary","endpointType":"HomeRegion","authType":"IamAuth","autoDeploymentMode":"LatestRevision","revisionWeights":[{"revisionId":"<R1>","weight":100}]}
4. Observe HTTP 202 whose revisionWeights report [{R1,100}]
5. GET /2025-03-07/web-functions/{functionName}/endpoints/canary and observe revisionWeights now [{R2,100}] — the weights the create validated, stored and reported were replaced by the first call that followed, so the create's response describes an endpoint no reader of the service agrees with
