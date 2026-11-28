Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 687333e2232d3fadb3887f8d91f372124a7f50c2

# The workspace data plane answers 404/503 with workspace state to a caller presenting no bearer credential, contradicting its own 401 rule

_Filed: 2026-11-28T10:21:52Z_
_Operation: CreateWorkspaceApiKey | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `687333e2232d3fadb3887f8d91f372124a7f50c2`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

The native Grafana HTTP API this service serves for a workspace (`src/service/workspace_api.rs`, the destination of the credential `CreateWorkspaceApiKey` mints) states one rule for a request that presents no bearer key: answer 401 and say nothing about why. `serve` breaks that rule for the two addressings that name the workspace in the ADDRESS (the endpoint path `/_aws/grafana/<workspaceId>/...` and the host the real service publishes): for those, the credential is never looked at until AFTER the store has been read, so a request carrying NO `Authorization` header at all is answered

* `404 {"message":"no workspace <id> is served at this endpoint"}` when the id names no workspace, and
* `503 {"message":"workspace <id> is <STATUS> and its Grafana HTTP API is not serving"}` when the workspace exists but is `CREATING` / `UPDATING` / `DELETING`,

both of which report stored state (which workspace ids exist, and the lifecycle status each is in) to an unauthenticated caller. The same route at the ANNOUNCED endpoint refuses first (`401` at workspace_api.rs:276-278), and the very same path-addressed request against an ACTIVE workspace with no credential falls through to the 401 at workspace_api.rs:307-309 — so the answer an unauthenticated caller gets depends on the workspace's stored status, which is exactly what the module says is never disclosed.

The fix is an ordering one inside `serve`: resolve the credential before reporting anything about the record for the address-named addressings, as the credential-named addressing already does. (Attributed to `CreateWorkspaceApiKey` because that is the operation that mints the credential this plane authenticates — workspace_api.rs:6-8 — and no modelled handler calls `serve` directly; it is reached from `EmulatedService::override_routing_error`, src/service.rs:312-322.)

## Evidence

The rule this module states — services/service-com-amazonaws-grafana-awsgrafanacontrolplane/src/service/workspace_api.rs:345-349:
```
/// The message the Grafana HTTP API states a refused credential in: a request with no
/// bearer key, with one no workspace of this emulator holds, or with one that is not a live
/// credential of the workspace it addressed, is answered 401 and nothing is said about which
/// of those it was.
const UNAUTHORIZED: &str = "Unauthorized";
```

What `serve` does — workspace_api.rs:270-305 (the credential is read but only USED on the `Addressed::Credential` branch; the `Addressed::Workspace` branch falls straight through to the store):
```
    let presented = bearer_token(authorization);
    let workspace_id = match &request.addressed {
        Addressed::Workspace(id) => id.clone(),
        Addressed::Credential => {
            let Some(presented) = presented.as_deref() else {
                return json(401, UNAUTHORIZED);
            };
            ...
        }
    };
    let Some(workspace) = store.settled_mut(&workspace_id, now) else {
        return json(
            404,
            &format!("no workspace {workspace_id} is served at this endpoint"),
        );
    };

    if workspace.status != ACTIVE {
        return json(
            503,
            &format!(
                "workspace {} is {} and its Grafana HTTP API is not serving",
                workspace.id, workspace.status
            ),
        );
    }
```
Only after those two state-reporting answers is the credential required — workspace_api.rs:307-312:
```
    let Some(presented) = presented else {
        return json(401, UNAUTHORIZED);
    };
    if !workspace.authorize_bearer_digest(&crate::state::bearer_token_digest(&presented), now) {
        return json(401, UNAUTHORIZED);
    }
```

A credential-less request on these addressings IS claimed and served (so it reaches the code above): `addressed_workspace` returns `Addressed::Workspace` from the path alone, with no credential in the decision — workspace_api.rs:194-206:
```
    if let Some((workspace_id, route)) = crate::state::workspace_at_announced_path(dialled.path)
    {
        return Some((Addressed::Workspace(workspace_id), route));
    }
```

Test coverage: every row that exercises a non-ACTIVE workspace or an unknown id presents a VALID key, so the credential-less case is unpinned — tests/all/workspace_grafana_api.rs:869, :896, :919 all pass `Some(&bearer(&key))`:
```
    let early = native(&emulator, &endpoint, "/api/org", Some(&bearer(&key))).await;
    ...
    let absent = native(&emulator, &unknown, "/api/org", Some(&bearer(&key))).await;
    ...
    let deleting = native(&emulator, &endpoint, "/api/org", Some(&bearer(&key))).await;
```
The only credential-less rows run against an ACTIVE workspace and are pinned at 401 — tests/all/workspace_grafana_api.rs:327-329:
```
/// workspace in the ADDRESS are exercised with no credential at all as well: there the
/// workspace is known without one, so the Grafana API itself refuses the request.
```

## Steps to Reproduce

1. Start the emulator with this service mounted (router reached at <emulator-host>).
2. POST /workspaces with {"accountAccessType":"CURRENT_ACCOUNT","permissionType":"CUSTOMER_MANAGED","authenticationProviders":["AWS_SSO"]} — observe 202 and note the id; the workspace is CREATING for state::LIFECYCLE_TRANSITION.
3. Immediately GET /_aws/grafana/<id>/api/org with NO Authorization header — observe 503 and the body "workspace <id> is CREATING and its Grafana HTTP API is not serving": an unauthenticated caller is told the workspace exists and which lifecycle status it is in.
4. Wait out LIFECYCLE_TRANSITION and repeat the identical credential-less request — observe 401 "Unauthorized" (workspace_api.rs:307-309), the answer the module's rule says such a request always gets.
5. GET /_aws/grafana/g-0123456789/api/org with no Authorization header — observe 404 "no workspace g-0123456789 is served at this endpoint", so an unauthenticated caller can enumerate which workspace ids exist by the 404-vs-401 difference.
6. DELETE /workspaces/<id>, then repeat the credential-less GET /_aws/grafana/<id>/api/org — observe 503 naming DELETING.
7. Contrast the announced-endpoint addressing: GET /api/org at <emulator-host> with no Authorization header — observe 401 with nothing said about any workspace (workspace_api.rs:276-278).
