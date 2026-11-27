Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: bdca4f331e3f84a5ee858acb1e0644b3d6ffabda

# RegisterSlackWorkspaceForOrganization requires a Support Center console authorization from a member account, which the model exempts member accounts from

_Filed: 2026-11-20T17:24:18Z_
_Operation: RegisterSlackWorkspaceForOrganization | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `bdca4f331e3f84a5ee858acb1e0644b3d6ffabda`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

The handler applies the console-authorization precondition to EVERY caller, before it knows whether the caller is the organization's management account or one of its member accounts. The model's own operation documentation says the console step is the MANAGEMENT account's step and that member accounts do not take it: "After the management account authorizes the Slack workspace, member accounts can call this API to authorize the same Slack workspace for their individual accounts. Member accounts don't need to authorize the Slack workspace manually through the Amazon Web Services Support Center."

Consequence in the emulator: a member account's registration is refused with `ResourceNotFoundException` (404) whose message instructs that member account to do exactly the manual console step the model says it needn't do — unless the Support-Center authorization set happens to hold the `teamId`. That set is state NO operation of this API writes (the crate says so itself), and it is seeded with exactly one id, so a member account has no wire-reachable path to satisfy a precondition the model does not place on it. The documented condition for this situation — `ConflictException`, "Call the `RegisterSlackWorkspaceForOrganization` API from a member account, but the management account hasn't registered that workspace yet for the organization" — is therefore answered as a 404 resource miss instead, and the crate's own conflict branch for the member case (`!registers_for_the_organization && !already_registered`) is only reachable when a caller reaches around the API into the state struct.

The member-account branch should be decided by what the model makes it depend on: whether the management account has registered that workspace for the organization (the registry presence the handler already reads), not by a console authorization the model exempts member accounts from.

## Evidence

Smithy, `smithy_query(query="doc",shape="RegisterSlackWorkspaceForOrganization")`:
  "After the management account authorizes the Slack workspace, member accounts can call this API to authorize the same Slack workspace for their individual accounts. Member accounts don't need to authorize the Slack workspace manually through the Amazon Web Services Support Center."
and `ConflictException`'s documented conditions include:
  "Call the RegisterSlackWorkspaceForOrganization API from a member account, but the management account hasn't registered that workspace yet for the organization."

Handler — the console gate runs for every caller, BEFORE the account's role is read:
services/service-com-amazonaws-supportapp-supportapp/src/ops/register_slack_workspace_for_organization.rs:250-263
```
        let not_found = {
            let store = state.lock();
            store.unauthorized_workspace_not_found(&input.team_id)
        };
        if let Some(message) = not_found {
            return Err(
                RegisterSlackWorkspaceForOrganizationErrorModeled::ResourceNotFoundException(
```
the account type (management vs member) is only read afterwards, at line 271:
```
        let account_type = crate::state::caller_account_type(&*self.engine)
```
and the member-account conflict the model documents is decided at line 311:
```
        if !registers_for_the_organization && !already_registered {
```

The refusal tells a member account to perform the console step the model exempts it from —
services/service-com-amazonaws-supportapp-supportapp/src/state.rs:836-841:
```
pub fn slack_workspace_not_authorized_message(team_id: &str) -> String {
    format!(
        "Slack workspace {team_id} not found for account {account}. Sign in to the Amazon Web \
         Services Support Center and authorize the Slack workspace before you register it.",
```

and the crate states that nothing in this API can satisfy that gate —
services/service-com-amazonaws-supportapp-supportapp/src/state.rs:55-57, 70:
```
/// ... and NO
/// operation of this API performs that flow, so which workspaces it authorized is state no
/// call of this service writes.
pub const CONSOLE_AUTHORIZED_SLACK_WORKSPACES: [&str; 1] = ["T012ABCDEFG"];
```

Test coverage showing the member path is only reachable by writing state directly —
services/service-com-amazonaws-supportapp-supportapp/tests/all/organization_account_role.rs:84-96:
```
/// The workspace IS authorized in the console, the precondition this operation settles
/// ahead of anything about the calling account: without it every row below would be
/// answered with the `ResourceNotFoundException` that
/// [`crate::console_authorized_workspace`] covers, and this suite's rule would never be
/// reached.
async fn register(emulator: &Emulator, team_id: &str) -> Reply {
    emulator.authorize_workspace_in_console(team_id);
```
and tests/all/console_authorized_workspace.rs:205-258 asserts the 404 for the `ManagedByAnotherAccount` (member-account) topology too, on the stated reason that "one reaches this API only through the interactive flow in the Amazon Web Services Support Center console" (module doc, lines 13-20) — the reason the quoted model sentence contradicts for member accounts.

## Steps to Reproduce

1. Boot the emulator with this service and the Organizations service on one engine.
2. Put the account in an organization managed by ANOTHER account id, so `crate::state::caller_account_type` reports `member` (as tests/all/organization_account_role.rs does via `hand_organization_to`).
3. POST /control/register-slack-workspace-for-organization with {"teamId": "T3790D96EA9"} (any id outside the seeded CONSOLE_AUTHORIZED_SLACK_WORKSPACES set).
4. Observe HTTP 404 ResourceNotFoundException with message "Slack workspace T3790D96EA9 not found for account <id>. Sign in to the Amazon Web Services Support Center and authorize the Slack workspace before you register it." — i.e. the member account is told to perform the manual console authorization the model says member accounts don't need, and the modelled ConflictException for "a member account, but the management account hasn't registered that workspace yet for the organization" is never reached.
5. Note there is no API call that can add a workspace to `console_authorized_slack_workspaces` (src/state.rs:55-57), so no wire sequence lets a member account past the gate for any id other than the one seeded value.
