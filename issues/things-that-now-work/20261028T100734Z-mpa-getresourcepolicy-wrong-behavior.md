Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 7959e6ca7cb81d5c32ad32f8e1e29786f97ad065

# The "does this ARN name a stored resource" decision excludes identity sources the store holds, so GetResourcePolicy answers 404 for a resource ListTagsForResource reports

_Filed: 2026-10-28T10:07:34Z_
_Operation: GetResourcePolicy | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `7959e6ca7cb81d5c32ad32f8e1e29786f97ad065`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`ComAmazonawsMpaAwsfluffycoreserviceState::records_resource` is documented as the ONE place an ARN-addressed operation decides whether a resource exists, and as the reason "the tagging and the resource-policy operations agree on it". Neither half holds in the code under it:

* The tag family does not call it at all — `TagResource`, `UntagResource` and `ListTagsForResource` resolve a `ResourceArn` through `resource_tags` / `resource_tags_mut`, whose own doc states the opposite inventory ("the approval team AND the identity source") and whose body resolves `identity_source(arn)`.
* `records_resource` itself resolves only teams, and its doc asserts "The approval team is the only resource this service stores" — stale since `CreateIdentitySource` began storing identity sources (with their tags) in this same store.

The result is observable: for the ARN of an identity source the store HOLDS, `ListTagsForResource` answers 200 with that source's tags while `GetResourcePolicy` — the only caller of `records_resource` — answers 404 `ResourceNotFoundException` with this service's "The requested resource does not exist." sentence. Two ARN-addressed reads of the same service therefore disagree about whether one stored resource exists, which is precisely what `records_resource`' own doc promises cannot happen. Either the existence decision has to resolve the resources the store actually holds (letting the policy reads answer on the attachment, which is what `resource_policies` already decides for an identity source), or the single-home claim and the inventory sentence in its doc are false; as written both the claim and the answer are wrong.

## Evidence

The claim, in `services/service-com-amazonaws-mpa-awsfluffycoreservice/src/state.rs:2790-2800`:
```
    /// Whether this store records a RESOURCE under `arn`.
    ///
    /// The one place an operation addressing a resource by ARN decides whether it
    /// exists, so the tagging and the resource-policy operations agree on it. The
    /// approval team is the only resource this service stores, so an ARN naming anything
    /// else names no resource; when another resource type is added here, every operation
    /// that resolves a resource ARN picks it up from this one function.
    #[must_use]
    pub fn records_resource(&self, arn: &str) -> bool {
        self.team(arn).is_some()
    }
```
The sibling resolver the tag family actually uses, contradicting the inventory, `state.rs:2774-2788`:
```
    /// The tags of the resource `arn` names, or `None` when this store holds no
    /// resource under that ARN.
    ///
    /// The one place a tagging operation resolves a resource ARN, so `TagResource` and
    /// `ListTagsForResource` agree on which resources exist and read the same map. The
    /// resources this service stores are the approval team and the identity source, so
    /// an ARN naming anything else is not found.
    pub fn resource_tags(&self, arn: &str) -> Option<&BTreeMap<String, String>> {
        if let Some(team) = self.team(arn) {
            return Some(&team.tags);
        }
        self.identity_source(arn).map(|source| &source.tags)
    }
```
The two call sites that diverge:
- `src/ops/get_resource_policy.rs:226` — `if !state.records_resource(&input.resource_arn) {` ... `return Err(not_found().into());` (404, `crate::state::resource_not_found()`, message `RESOURCE_NOT_FOUND_MESSAGE` = "The requested resource does not exist.", `state.rs:3365`).
- `src/ops/list_tags_for_resource.rs:189` — `let Some(tags) = state.resource_tags(&input.resource_arn) else { ... }` → 200 for the same identity-source ARN.

That identity sources are stored, tagged resources: `src/ops/create_identity_source.rs:338-355` builds `crate::state::IdentitySource::creating(... input.tags ...)` and calls `state.insert_identity_source(source)`.

The drift is visible in the tests too: `tests/all/resource_tags.rs:540` still labels a row "an identity-source ARN, a resource type this service does not store" — and it passes only because that row's ARN was never created, so no test pins either read for a STORED identity source.

## Steps to Reproduce

1. Start the emulator and record the management account so the gated creates are served (as `tests/all/approval_team_lifecycle.rs:194` does).
2. `POST /identity-sources` with `IdentitySourceParameters.IamIdentityCenter` and `Tags: {"env":"prod"}`; note the returned `IdentitySourceArn`.
3. `GET /tags/{IdentitySourceArn}` → 200 with `{"Tags":{"env":"prod"}}`: this read says the resource exists.
4. `POST /GetResourcePolicy` with that same `ResourceArn` plus any `PolicyName`/`PolicyType` → 404 `ResourceNotFoundException` "The requested resource does not exist.", because `records_resource` resolves teams only.
5. Same divergence via `POST /resource-policies/{ResourceArn}/?List` (`list_resource_policies.rs:227`, `state.resource_policies` → `self.team(resource_arn)?`), which answers 404 for the stored source the tag read just reported.
