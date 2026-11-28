Issue-Severity: LOW
Issue-Gap-Class: error-response
Issue-Commit: 7c00a94e367da23ce2c3f6798a9a0f3f9bd5c557

# The service-account quota refusal names the workspace's id with resourceType "ServiceAccount", contradicting its own comment and its API-key sibling

_Filed: 2026-11-28T02:20:20Z_
_Operation: CreateWorkspaceServiceAccount | Class: error-response_

> _Note: this issue was identified against the code base at commit `7c00a94e367da23ce2c3f6798a9a0f3f9bd5c557`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

When a workspace already holds `SERVICE_ACCOUNT_QUOTA` accounts, `CreateWorkspaceServiceAccount` answers `ServiceQuotaExceededException` whose `resourceId` is the WORKSPACE id while `resourceType` is `"ServiceAccount"`. The three parts of that one payload disagree: the comment directly above the two fields says "The quota is per workspace, so the workspace is the resource that reached it", the message says "workspace {id} already holds the maximum of {n} service accounts", and `resourceId` is `workspace.id` — but the type names a service account, so a client reading the pair `(resourceId, resourceType)` is told that a service account has the id of a workspace.

The sibling handler that refuses the analogous PER-WORKSPACE quota resolves it the other way: `CreateWorkspaceApiKey` pairs the workspace id with `WORKSPACE_RESOURCE_TYPE` under the identical comment. The third quota refusal in the crate (`CreateWorkspaceServiceAccountToken`, a per-ACCOUNT quota) is self-consistent: it pairs the service-account id with `SERVICE_ACCOUNT_RESOURCE_TYPE`. So one rule — "name the resource that reached the quota" — is implemented two different ways for the two per-workspace quotas, and the model's own docs make the pair one resource: `resourceId` is "The ID of the resource that is associated with the error" and `resourceType` "The type of the resource that is associated with the error".

## Evidence

The handler under review — services/service-com-amazonaws-grafana-awsgrafanacontrolplane/src/ops/create_workspace_service_account.rs:243-261:
```
        if workspace.service_accounts.len() >= crate::state::SERVICE_ACCOUNT_QUOTA {
            return Err(
                CreateWorkspaceServiceAccountErrorModeled::ServiceQuotaExceededException(
                    ServiceQuotaExceededException {
                        message: format!(
                            "workspace {} already holds the maximum of {} service accounts",
                            workspace.id,
                            crate::state::SERVICE_ACCOUNT_QUOTA
                        ),
                        // The quota is per workspace, so the workspace is the resource
                        // that reached it.
                        resource_id: workspace.id.clone(),
                        resource_type: crate::state::SERVICE_ACCOUNT_RESOURCE_TYPE.to_string(),
```

The sibling per-workspace quota refusal — src/ops/create_workspace_api_key.rs:240-252:
```
        if workspace.api_keys.len() >= crate::state::API_KEY_QUOTA {
...
                        // The quota is per workspace, so the workspace is the resource
                        // that reached it.
                        resource_id: input.workspace_id.clone(),
                        resource_type: crate::state::WORKSPACE_RESOURCE_TYPE.to_string(),
```

The per-account quota refusal, which IS self-consistent — src/ops/create_workspace_service_account_token.rs:264-267:
```
                        // The quota is per service account, so the account is the
                        // resource that reached it.
                        resource_id: input.service_account_id.clone(),
                        resource_type: crate::state::SERVICE_ACCOUNT_RESOURCE_TYPE.to_string(),
```

What the two constants are for — src/state.rs:102-104 and src/state.rs:161-162:
```
/// The `resourceType` a `ResourceNotFoundException` reports for a workspace id
/// that names no workspace.
pub const WORKSPACE_RESOURCE_TYPE: &str = "Workspace";
...
/// The `resourceType` an error reports for a service account of a workspace.
pub const SERVICE_ACCOUNT_RESOURCE_TYPE: &str = "ServiceAccount";
```

Model: `ServiceQuotaExceededException$resourceId` is "<p>The ID of the resource that is associated with the error.</p>" and `$resourceType` "<p>The type of the resource that is associated with the error.</p>" (`smithy_query(query="shape",shape="ServiceQuotaExceededException")`) — one resource, so the id and the type cannot name different kinds of thing.

Test coverage: tests/all/service_accounts.rs:490-520 drives the quota refusal but asserts only the status (402) and `__type`; it never reads `resourceId`/`resourceType` (the only `resourceType` assertion in that file is the name-conflict row at service_accounts.rs:291), so no test pins the current pairing and fixing it changes no asserted response.

## Steps to Reproduce

1. Start the emulator with this service mounted.
2. POST /workspaces with {"accountAccessType":"CURRENT_ACCOUNT","permissionType":"CUSTOMER_MANAGED","authenticationProviders":["AWS_SSO"]} and wait out state::LIFECYCLE_TRANSITION.
3. POST /workspaces/{id}/serviceaccounts state::SERVICE_ACCOUNT_QUOTA times with distinct names and grafanaRole VIEWER — all accepted with 200.
4. POST /workspaces/{id}/serviceaccounts once more — observe 402 ServiceQuotaExceededException whose body carries "resourceId":"g-xxxxxxxxxx" (a workspace id) together with "resourceType":"ServiceAccount".
5. POST /workspaces/{id}/apikeys state::API_KEY_QUOTA + 1 times — observe the sibling per-workspace quota refusal carrying the same kind of resourceId with "resourceType":"Workspace", i.e. the two handlers disagree about what the resource is.
