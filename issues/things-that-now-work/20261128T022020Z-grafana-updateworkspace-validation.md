Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: 7c00a94e367da23ce2c3f6798a9a0f3f9bd5c557

# UpdateWorkspace accepts permissionType CUSTOMER_MANAGED with no workspaceRoleArn, and organizationRoleName on a SERVICE_MANAGED workspace

_Filed: 2026-11-28T02:20:20Z_
_Operation: UpdateWorkspace | Class: validation_

> _Note: this issue was identified against the code base at commit `7c00a94e367da23ce2c3f6798a9a0f3f9bd5c557`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`UpdateWorkspace` enforces exactly one of the three cross-member rules its own request members document. It judges `accountAccessType: ORGANIZATION` against the POST-patch record and refuses with the modelled `ValidationException` when no organizational unit is named — but the two rules the model states about `permissionType` are not checked at all:

1. `UpdateWorkspaceRequest$permissionType` documents "If you specify this as CUSTOMER_MANAGED, you must also specify a workspaceRoleArn that the workspace will use for accessing Amazon Web Services resources", and `UpdateWorkspaceRequest$workspaceRoleArn` repeats it from the other side ("If this workspace has permissionType CUSTOMER_MANAGED, then this role is required"). The handler writes `workspace.permission_type = value` unconditionally (update_workspace.rs:331-333) without looking at `input.workspace_role_arn` or at the stored `workspace.workspace_role_arn`, so `PUT /workspaces/{id}` with `{"permissionType":"CUSTOMER_MANAGED"}` and no role ARN — in the request or on the record — answers 202 and persists a CUSTOMER_MANAGED workspace with `workspaceRoleArn` absent, which `DescribeWorkspace` then reports.
2. `UpdateWorkspaceRequest$organizationRoleName` documents "This can only be used with a workspace that has the permissionType set to CUSTOMER_MANAGED". The handler stores it unconditionally (update_workspace.rs:337-339), so the member is accepted and persisted for a workspace whose stored `permissionType` is `SERVICE_MANAGED`.

Both are the same KIND of rule the handler already implements for `accountAccessType`, judged on the post-patch record and refused before anything is written — the helper shape (`ValidationException` + `ValidationExceptionField`) is already there in the same function.

## Evidence

Model, `UpdateWorkspaceRequest$permissionType` (`smithy_query(query="doc",shape="UpdateWorkspaceRequest",member="permissionType")`):
```
<p>If you specify this as <code>CUSTOMER_MANAGED</code>, you must also specify a <code>workspaceRoleArn</code> that the workspace will use for accessing Amazon Web Services resources.</p>
```
Model, `UpdateWorkspaceRequest$workspaceRoleArn`:
```
<p>Specifies an IAM role that grants permissions to Amazon Web Services resources that the workspace accesses... If this workspace has <code>permissionType</code> <code>CUSTOMER_MANAGED</code>, then this role is required.</p>
```
Model, `UpdateWorkspaceRequest$organizationRoleName`:
```
<p>The name of an IAM role that already exists to use to access resources through Organizations. This can only be used with a workspace that has the <code>permissionType</code> set to <code>CUSTOMER_MANAGED</code>.</p>
```

The handler DOES enforce the sibling cross-member rule, on the post-patch values — services/service-com-amazonaws-grafana-awsgrafanacontrolplane/src/ops/update_workspace.rs:287-309:
```
        let access_after = input
            .account_access_type
            .as_deref()
            .unwrap_or(&workspace.account_access_type);
        let units_after = input
            .workspace_organizational_units
            .as_ref()
            .unwrap_or(&workspace.organizational_units);
        if access_after == "ORGANIZATION" && units_after.is_empty() {
            return Err(UpdateWorkspaceErrorModeled::ValidationException(
                ValidationException {
                    message: "workspaceOrganizationalUnits must name at least one \
                              organizational unit when accountAccessType is ORGANIZATION"
```

The two permissionType rules are not enforced — update_workspace.rs:331-339 (every place the handler touches these members):
```
        if let Some(value) = input.permission_type {
            workspace.permission_type = value;
        }
        if let Some(value) = input.workspace_role_arn {
            workspace.workspace_role_arn = Some(value);
        }
        if let Some(value) = input.organization_role_name {
            workspace.organization_role_name = Some(value);
        }
```
`grep -n "CUSTOMER_MANAGED" src/ops/update_workspace.rs` matches only the doc header (update_workspace.rs:71, :72-80), never the handler body.

Test coverage: tests/all/workspace_lifecycle.rs:589-594 is the only row that sends `permissionType` through `UpdateWorkspace`, and it sends `SERVICE_MANAGED`:
```
        (
            "the permission type",
            json!({"permissionType": "SERVICE_MANAGED"}),
            "/permissionType",
            json!("SERVICE_MANAGED"),
        ),
```
Its baseline already carries `"workspaceRoleArn": "arn:aws:iam::000000000000:role/Baseline"` (workspace_lifecycle.rs:556), and no test anywhere sends `organizationRoleName` or `permissionType: CUSTOMER_MANAGED` on an update (grep for `permissionType` across tests/ shows update bodies only at workspace_lifecycle.rs:591), so nothing pins the current acceptance. The ORGANIZATION rule's own test states the pattern — tests/all/workspace_lifecycle.rs:1038-1039: "`accountAccessType: ORGANIZATION` documents that the organizational units must be named with it; a create that leaves them out is refused rather than stored."

## Steps to Reproduce

1. Start the emulator with this service mounted.
2. POST /workspaces with {"accountAccessType":"CURRENT_ACCOUNT","permissionType":"SERVICE_MANAGED","authenticationProviders":["AWS_SSO"]} — observe 202 and note the workspace id; the record holds no workspaceRoleArn.
3. Wait out state::LIFECYCLE_TRANSITION so the workspace reports ACTIVE (otherwise the in-flight ConflictException gate answers first).
4. PUT /workspaces/{id} with {"permissionType":"CUSTOMER_MANAGED"} and no workspaceRoleArn — observe HTTP 202 instead of the ValidationException the member's own documentation requires ("you must also specify a workspaceRoleArn").
5. GET /workspaces/{id} — observe permissionType CUSTOMER_MANAGED with workspaceRoleArn absent, a combination the model documents as invalid.
6. Wait out LIFECYCLE_TRANSITION again, then PUT /workspaces/{id} with {"permissionType":"SERVICE_MANAGED","organizationRoleName":"some-role"} — observe 202 and GET /workspaces/{id} reporting organizationRoleName on a SERVICE_MANAGED workspace, although the member documents it "can only be used with a workspace that has the permissionType set to CUSTOMER_MANAGED".
7. Contrast with PUT /workspaces/{id} {"accountAccessType":"ORGANIZATION"} on a workspace with no organizational units, which is refused 400 ValidationException by update_workspace.rs:295-309.
