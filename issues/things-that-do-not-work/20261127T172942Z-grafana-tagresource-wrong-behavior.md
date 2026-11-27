Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 7703f69e9c77821dec580f43e82f27cc08c90ba5

# Tagging authorizes against the ARN as spelled by the caller but resolves the workspace ignoring region and account

_Filed: 2026-11-27T17:29:42Z_
_Operation: TagResource | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `7703f69e9c77821dec580f43e82f27cc08c90ba5`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

The three tagging operations take the IAM decision on the literal `resourceArn` string the request carried (`authorize(..., &input.resource_arn)`), while the workspace the call then reads or writes is resolved by `crate::state::workspace_id_in_arn`, which deliberately does NOT match the ARN's region or account segments. The two halves therefore disagree about which resource the call is about: the write lands on workspace `g-xxxxxxxxxx` of this emulator whatever region or account the ARN names, but the authorization decision was taken on the foreign ARN string.

This crate's authorization module states the rule the other handlers follow: the resource an operation is judged against is `crate::state::workspace_arn(...)` — the canonical `arn:aws:grafana:<region>:<account>:/workspaces/<id>` built by the crate. Every non-tagging handler passes exactly that. As a result the same workspace is judged under two different resource ARNs depending on which operation is called, and an identity-based statement scoped to the canonical ARN of a workspace (including an explicit `Deny`) does not apply to a tag write that reaches that same workspace through an ARN spelled with another region or account — the tags are written anyway.

## Evidence

Rule this crate states — services/service-com-amazonaws-grafana-awsgrafanacontrolplane/src/service/authorization.rs:41-44:
```
//! identity-based one alone. The resource an operation is judged against is the ARN of the
//! workspace the request NAMES ([`crate::state::workspace_arn`]), or of the workspaces
//! collection ([`crate::state::workspaces_arn`]) for one that addresses no single
//! workspace.
```
What a sibling handler does — src/ops/update_workspace.rs:233-242: `authorize(..., &crate::state::workspace_arn(&crate::state::request_region(ctx.parts()), &input.workspace_id))`.

What the tag handler does — src/ops/tag_resource.rs:159-164:
```
        crate::service::authorization::authorize(
            &*self.engine,
            ctx,
            Self::NAME,
            &input.resource_arn,
        )
```
(the same in src/ops/untag_resource.rs:156-161 and src/ops/list_tags_for_resource.rs).

How the resource is then resolved — src/state.rs:1294-1313:
```
/// region and the account segments are not matched: this emulator serves one
/// account, and the workspace id is what identifies the resource within it.
pub fn workspace_id_in_arn(arn: &str) -> Option<&str> {
...
    let _region = fields.next()?;
    let _account = fields.next()?;
```
and src/ops/tag_resource.rs:178-183 / :202-238 — the id that comes out of it is what `settled_mut` loads and what `workspace.tags = merged;` writes.

Test coverage: tests/all/workspace_authorization.rs:691-753 (`an_allow_is_scoped_to_the_resource_the_request_addresses`) pins resource scoping for `DescribeWorkspace` only — no row drives TagResource/UntagResource, so the tagging path's resource ARN is unpinned.

## Steps to Reproduce

1. Boot the emulator with the IAM and STS services registered and create a session principal whose role policy contains {"Effect":"Allow","Action":"grafana:*","Resource":"*"} plus {"Effect":"Deny","Action":"grafana:TagResource","Resource":"arn:aws:grafana:eu-west-1:000000000000:/workspaces/<id>"}.
2. CreateWorkspace in eu-west-1 and note the workspace id <id>.
3. Call POST /tags/<percent-encoded arn:aws:grafana:eu-west-1:000000000000:/workspaces/<id>> — refused 403 AccessDeniedException, as the deny intends.
4. Call POST /tags/<percent-encoded arn:aws:grafana:us-east-1:999999999999:/workspaces/<id>> with the same tags.
5. The deny does not match the ARN string, so the call is allowed, and GET /workspaces/<id> reports the new tags on the eu-west-1 workspace: the decision was taken on one resource and the write happened on another.
