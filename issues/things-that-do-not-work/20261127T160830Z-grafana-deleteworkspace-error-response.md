Issue-Severity: MEDIUM
Issue-Gap-Class: error-response
Issue-Commit: 2453612a9812cc17d3b11d7e571f512669259467

# DeleteWorkspace skips the in-flight ConflictException gate its two 202 siblings apply

_Filed: 2026-11-27T16:08:30Z_
_Operation: DeleteWorkspace | Class: error-response_

> _Note: this issue was identified against the code base at commit `2453612a9812cc17d3b11d7e571f512669259467`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

DeleteWorkspace is one of the three operations this model gives BOTH a 202 success status and a `ConflictException` (the other two are `UpdateWorkspace` and `UpdateWorkspaceConfiguration`). Its two siblings consult the crate's shared gate `crate::state::workspace_in_flight` and refuse a workspace whose previous accepted change has not settled with HTTP 409, naming the status. `DeleteWorkspace`'s handler never calls that gate: it consults only `Workspace::deletion_in_flight()`, which covers the `@idempotent` repeat-delete case, and therefore accepts a delete of a workspace that is still `CREATING` or `UPDATING`.

Two consequences inside this crate:

1. The same documented failure mode (`ConflictException`: "A resource was in an inconsistent state during an update or a deletion") is answered 409 by `PUT /workspaces/{id}` and `PUT /workspaces/{id}/configuration` but 202 by `DELETE /workspaces/{id}` for the identical workspace state, so `DeleteWorkspace` can never produce the error its own `#[operation_error]` enum declares.
2. The accepted delete OVERWRITES the pending transition the earlier 202 owed the record (`workspace.pending = Some(... OwedChange::Removal)` replaces the `OwedChange::Status(ACTIVE)` a create or update installed), so a create that was accepted and not yet settled is silently discarded by the delete rather than refused.

The suite that documents the gate also mis-states the model: it says the rule covers "the two the model gives both a 202 success status and a `ConflictException`", while the model gives three — `DeleteWorkspace` carries `-> 202` and `ConflictException (HTTP 409)` too.

## Evidence

The model gives DeleteWorkspace both 202 and ConflictException — services/service-com-amazonaws-grafana-awsgrafanacontrolplane/OPERATIONS.md:195-201:
```
## DeleteWorkspace

- **Route:** `DELETE /workspaces/{workspaceId} -> 202`
- **Traits:** @idempotent
- **Input:** `workspaceId` (string, pattern ^g-[0-9a-f]{10}$, @httpLabel, required)
- **Output:** `workspace` (WorkspaceDescription, required)
- **Errors:** `AccessDeniedException (HTTP 403)`, `ConflictException (HTTP 409)`, ...
```

The shared gate, and what the crate says it is for — src/state.rs:1098-1119:
```
/// The modelled `ConflictException` for a workspace whose previous accepted
/// change is still in flight, or `None` when it rests in a status a further
/// change is accepted in.
///
/// `ConflictException` documents "A resource was in an inconsistent state during
/// an update or a deletion", which is exactly what a workspace is between the
/// `202` that started a change and the status the clock owes it
pub fn workspace_in_flight(workspace: &Workspace) -> Option<ConflictException> {
```

Sibling 1 calls it — src/ops/update_workspace.rs:318-320:
```
        if let Some(conflict) = crate::state::workspace_in_flight(workspace) {
            return Err(UpdateWorkspaceErrorModeled::ConflictException(conflict).into());
        }
```

Sibling 2 calls it — src/ops/update_workspace_configuration.rs:226-228:
```
        if let Some(conflict) = crate::state::workspace_in_flight(workspace) {
            return Err(UpdateWorkspaceConfigurationErrorModeled::ConflictException(conflict).into());
        }
```

The handler under review does not — src/ops/delete_workspace.rs:185-198 (the only state check it makes):
```
        // `DeleteWorkspace` is `@idempotent`: a delete of a workspace whose deletion
        // is ALREADY in flight is accepted again and answers the same record, still
        // `DELETING`, ...
        if !workspace.deletion_in_flight() {
            workspace.status = crate::state::WORKSPACE_DELETING.to_string();
            workspace.pending = Some(crate::state::PendingTransition {
                due: now + crate::state::LIFECYCLE_TRANSITION,
                owed: crate::state::OwedChange::Removal,
            });
            workspace.modified = now;
        }
```
`grep Conflict src/ops/delete_workspace.rs` finds only the doc line and the enum variant (delete_workspace.rs:65, :94) — the variant is never constructed.

The test that states the rule, and its incorrect count — tests/all/workspace_mutation_gating.rs:19-24:
```
//! 2. **A change to a workspace whose previous change is still in flight is a conflict.**
//!    Every operation that mutates a workspace asynchronously (the two the model gives
//!    both a 202 success status and a `ConflictException`) refuses a workspace in one of
//!    the progressive `WorkspaceStatus` values with HTTP 409 `ConflictException`
```
and tests/all/workspace_mutation_gating.rs:212-224 drives only `UpdateWorkspace` and `UpdateWorkspaceConfiguration` (`fn mutations()`). No test in the crate issues a `DeleteWorkspace` against a `CREATING` or `UPDATING` workspace (grep for `CREATING|UPDATING` across tests/ shows the in-flight rows only in that file's two mutations), so nothing pins the current acceptance.

## Steps to Reproduce

1. Start the emulator with this service mounted.
2. POST /workspaces with {"accountAccessType":"CURRENT_ACCOUNT","permissionType":"CUSTOMER_MANAGED","authenticationProviders":["AWS_SSO"]} — observe 202 and workspace.status == "CREATING" (the clock owes it ACTIVE for state::LIFECYCLE_TRANSITION).
3. Immediately (inside that span) PUT /workspaces/{id} with {"workspaceDescription":"x"} — observe 409 ConflictException "Workspace operation failed. Workspace is currently in the CREATING state." from crate::state::workspace_in_flight.
4. Repeat steps 2 and immediately DELETE /workspaces/{id} — observe 202 with status DELETING: the same in-flight state that refuses the update is accepted here, and the pending CREATING→ACTIVE transition installed by the create is replaced by OwedChange::Removal.
5. Observe that no input and no workspace state can make DeleteWorkspace emit the ConflictException its error enum declares (delete_workspace.rs:94).
