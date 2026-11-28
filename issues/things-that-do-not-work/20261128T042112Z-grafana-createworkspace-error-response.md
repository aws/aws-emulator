Issue-Severity: LOW
Issue-Gap-Class: error-response
Issue-Commit: 818028b58c482d0d7e7e5628b8ad1015f5ddfbc2

# The workspace-quota refusal pairs the account id with resourceType "Workspace", so the id and the type name different kinds of resource

_Filed: 2026-11-28T04:21:12Z_
_Operation: CreateWorkspace | Class: error-response_

> _Note: this issue was identified against the code base at commit `818028b58c482d0d7e7e5628b8ad1015f5ddfbc2`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

When the store already holds `WORKSPACE_QUOTA` workspaces, `CreateWorkspace` answers the modelled `ServiceQuotaExceededException` with `resourceId` set to the EMULATOR ACCOUNT id (`::engine::emulator_account_id()`, i.e. "000000000000") and `resourceType` set to `crate::state::WORKSPACE_RESOURCE_TYPE` ("Workspace"). The model makes that pair describe ONE resource (`resourceId` is "The ID of the resource that is associated with the error", `resourceType` "The type of the resource that is associated with the error"), so a client reading the pair is told that a *Workspace* has the id "000000000000" — a value `WorkspaceId`'s own `^g-[0-9a-f]{10}$` pattern admits nowhere.

The three parts of this one payload disagree with each other: the comment sitting directly above the two fields says "The quota is per account, so the account is the resource that reached it" and the message says "this account already holds the maximum of N workspaces" — both naming the ACCOUNT — while `resourceType` names a workspace. The constant used is also documented for a different purpose: `WORKSPACE_RESOURCE_TYPE`'s own doc says it is "The `resourceType` a `ResourceNotFoundException` reports for a workspace id that names no workspace".

The sibling quota refusals in the same crate are each self-consistent about this: `CreateWorkspaceApiKey` pairs the workspace id with `WORKSPACE_RESOURCE_TYPE`, and `CreateWorkspaceServiceAccountToken` pairs the service-account id with `SERVICE_ACCOUNT_RESOURCE_TYPE`. Only this handler reports an id of one kind of thing under the type of another.

## Evidence

The handler under review — services/service-com-amazonaws-grafana-awsgrafanacontrolplane/src/ops/create_workspace.rs:303-318:
```
        if store.workspaces.len() >= crate::state::WORKSPACE_QUOTA {
            return Err(CreateWorkspaceErrorModeled::ServiceQuotaExceededException(
                ServiceQuotaExceededException {
                    message: format!(
                        "this account already holds the maximum of {} workspaces",
                        crate::state::WORKSPACE_QUOTA
                    ),
                    // The quota is per account, so the account is the resource
                    // that reached it.
                    resource_id: ::engine::emulator_account_id().to_string(),
                    resource_type: crate::state::WORKSPACE_RESOURCE_TYPE.to_string(),
```
The comment says the ACCOUNT is the resource; the `resource_type` on the next line says "Workspace".

What that constant is documented for — src/state.rs:102-104:
```
/// The `resourceType` a `ResourceNotFoundException` reports for a workspace id
/// that names no workspace.
pub const WORKSPACE_RESOURCE_TYPE: &str = "Workspace";
```

The self-consistent sibling refusals:
- src/ops/create_workspace_api_key.rs:249-252 — per-workspace quota, workspace id with the workspace type:
```
                        // The quota is per workspace, so the workspace is the resource
                        // that reached it.
                        resource_id: input.workspace_id.clone(),
                        resource_type: crate::state::WORKSPACE_RESOURCE_TYPE.to_string(),
```
- src/ops/create_workspace_service_account_token.rs:264-267 — per-account quota, account id with the account type:
```
                        // The quota is per service account, so the account is the
                        // resource that reached it.
                        resource_id: input.service_account_id.clone(),
                        resource_type: crate::state::SERVICE_ACCOUNT_RESOURCE_TYPE.to_string(),
```

Model (`smithy_query(query="shape",shape="ServiceQuotaExceededException")`) — the two members describe ONE resource:
```
"resourceId": {"target": "String", "required": true, "doc": "<p>The ID of the resource that is associated with the error.</p>"},
"resourceType": {"target": "String", "required": true, "doc": "<p>The type of the resource that is associated with the error.</p>"}
```
and a Grafana workspace's id is constrained — `smithy_query(query="shape",shape="WorkspaceDescription")`, member `id`: `"pattern": "^g-[0-9a-f]{10}$"` — which "000000000000" does not satisfy.

Test coverage: tests/all/workspace_lifecycle.rs:809-820 is the only test that drives this refusal and it asserts nothing but the status and the code, so the current pairing is unpinned and correcting it changes no asserted response:
```
    let refused = create(&router, minimal_create()).await;
    assert_eq!(
        refused.status, 402,
...
    assert_eq!(
        refused.error_code(),
        "ServiceQuotaExceededException",
```

## Steps to Reproduce

1. Start the emulator with this service mounted.
2. POST /workspaces with {"accountAccessType":"CURRENT_ACCOUNT","permissionType":"CUSTOMER_MANAGED","authenticationProviders":["AWS_SSO"]} repeatedly, state::WORKSPACE_QUOTA times — each answered 202.
3. POST /workspaces once more — observe 402 ServiceQuotaExceededException whose body carries "resourceId":"000000000000" (the account) together with "resourceType":"Workspace", so the reported resource type does not describe the reported id (which cannot be a WorkspaceId at all, per ^g-[0-9a-f]{10}$).
4. Contrast with the per-workspace quota refusal of src/ops/create_workspace_api_key.rs (POST /workspaces/{id}/apikeys past state::API_KEY_QUOTA), whose resourceId IS a workspace id under the same "Workspace" resourceType, and with src/ops/create_workspace_service_account_token.rs, whose resourceId is a service-account id under "ServiceAccount".
