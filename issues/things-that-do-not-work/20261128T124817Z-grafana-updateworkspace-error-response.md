Issue-Severity: MEDIUM
Issue-Gap-Class: error-response
Issue-Commit: 405f53c3c47d4a5551f1b3a13eff3cb64fdeab94

# UpdateWorkspace's set/remove conflict refusal reports reason OTHER where every other ValidationException in the crate reports FIELD_VALIDATION_FAILED

_Filed: 2026-11-28T12:48:17Z_
_Operation: UpdateWorkspace | Class: error-response_

> _Note: this issue was identified against the code base at commit `405f53c3c47d4a5551f1b3a13eff3cb64fdeab94`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`UpdateWorkspace` raises two cross-member `ValidationException`s, and the two classify the same kind of failure differently on the wire.

The `vpcConfiguration`/`removeVpcConfiguration` (and `networkAccessControl`/`removeNetworkAccessConfiguration`) refusal built by the local `conflicting_members` helper sets `reason: "OTHER"` while still carrying a `fieldList` that names the two members at fault. The ORGANIZATION/`workspaceOrganizationalUnits` refusal a few lines below it — in the SAME handler, also a cross-member rule, also carrying a `fieldList` — sets `reason: "FIELD_VALIDATION_FAILED"`. Every one of the other ten `ValidationException` payloads this crate builds (the shared ones in `state.rs` and the per-handler ones in `CreateWorkspace`, `CreateWorkspaceApiKey`, `TagResource`, `ListVersions`, `ListWorkspaceServiceAccounts`, `ListWorkspaceServiceAccountTokens`, `UpdateWorkspaceConfiguration`) sets `FIELD_VALIDATION_FAILED`, and `state.rs` states that as the crate's rule in prose: "`ValidationException$reason` is `FIELD_VALIDATION_FAILED`, and the field list names the member at fault, exactly as the other refusals in this service do." The `conflicting_members` payload breaks exactly that stated rule: it names the members at fault in its field list and then classifies the reason as `OTHER`, so a client that branches on `reason` (a `@required` member of the error) is told this one refusal is not a field validation failure while an otherwise identical refusal of the same operation is.

## Evidence

The lone `OTHER` — services/service-com-amazonaws-grafana-awsgrafanacontrolplane/src/ops/update_workspace.rs:209-227:
```
        /// A `ValidationException` for a request that both sets and removes the
        /// same member: the two instructions cannot both be applied.
        fn conflicting_members(set: &str, remove: &str) -> UpdateWorkspaceError {
            UpdateWorkspaceErrorModeled::ValidationException(ValidationException {
                message: format!("{set} and {remove} cannot both be specified"),
                reason: "OTHER".to_string(),
                field_list: Some(vec![
                    ValidationExceptionField {
                        name: set.to_string(),
                        message: format!("conflicts with {remove}"),
                    },
```

The other cross-member refusal of the SAME handler — update_workspace.rs:296-307:
```
            return Err(UpdateWorkspaceErrorModeled::ValidationException(
                ValidationException {
                    message: "workspaceOrganizationalUnits must name at least one \
                              organizational unit when accountAccessType is ORGANIZATION"
                        .to_string(),
                    reason: "FIELD_VALIDATION_FAILED".to_string(),
                    field_list: Some(vec![ValidationExceptionField {
                        name: "workspaceOrganizationalUnits".to_string(),
```

The crate's own statement of the rule — services/service-com-amazonaws-grafana-awsgrafanacontrolplane/src/state.rs:1353-1355 (doc of `not_a_workspace_arn`):
```
/// `ValidationException$reason` is `FIELD_VALIDATION_FAILED`, and the field list
/// names the member at fault, exactly as the other refusals in this service do.
```

Every other `ValidationException` in the crate agrees with that statement (`grep -n '"OTHER"|FIELD_VALIDATION_FAILED'`): state.rs:1074, state.rs:1221, state.rs:1342, state.rs:1358; ops/create_workspace.rs:254; ops/create_workspace_api_key.rs:206; ops/tag_resource.rs:223; ops/list_versions.rs:231; ops/list_workspace_service_accounts.rs:221; ops/list_workspace_service_account_tokens.rs:223; ops/update_workspace_configuration.rs:209 — all `FIELD_VALIDATION_FAILED`. The only `"OTHER"` in the crate is ops/update_workspace.rs:214.

Test coverage: nothing pins the current value. `grep -n 'reason|fieldList' tests/` matches only unrelated prose and a local helper named `reason` in tests/all/workspace_authorization.rs:313; no test asserts the `reason` or `fieldList` members of a `ValidationException`, so no existing assertion depends on `OTHER`.

Model (`smithy_query(query="shape", shape="ValidationException,ValidationExceptionReason")`): `reason` is `@required` and targets `ValidationExceptionReason` with values `["UNKNOWN_OPERATION", "CANNOT_PARSE", "FIELD_VALIDATION_FAILED", "OTHER"]`, and `fieldList` is "A list of fields that might be associated with the error" — so both values are admissible and the choice is the crate's own, which is why the two refusals of one operation disagreeing is the defect.

## Steps to Reproduce

1. Start the emulator with this service mounted.
2. POST /workspaces with {"accountAccessType":"CURRENT_ACCOUNT","permissionType":"CUSTOMER_MANAGED","authenticationProviders":["AWS_SSO"]} and note the workspace id.
3. Wait out state::LIFECYCLE_TRANSITION so the workspace reports ACTIVE.
4. PUT /workspaces/{id} with {"removeVpcConfiguration":true,"vpcConfiguration":{"securityGroupIds":["sg-1"],"subnetIds":["subnet-1","subnet-2"]}} — observe 400 ValidationException whose body carries "reason":"OTHER" together with a fieldList naming vpcConfiguration and removeVpcConfiguration (ops/update_workspace.rs:214).
5. PUT /workspaces/{id} with {"accountAccessType":"ORGANIZATION"} on the same workspace (it holds no organizational units) — observe 400 ValidationException whose body carries "reason":"FIELD_VALIDATION_FAILED" with a fieldList naming workspaceOrganizationalUnits (ops/update_workspace.rs:301).
6. Compare the two bodies: the same operation classifies two cross-member refusals, both carrying field lists, under two different ValidationExceptionReason values, and the first one contradicts the rule state.rs:1353-1355 states for this service.
