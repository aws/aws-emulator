Issue-Severity: HIGH
Issue-Gap-Class: wrong-behavior
Issue-Commit: 73a5b33d15d7cd33df4610c3d59efb1de9382e21

# Every {identifier} operation resolves only an exact ARN, so the documented NAME form of InvestigationGroupIdentifier addresses nothing and is answered 404

_Filed: 2026-11-28T04:55:30Z_
_Operation: DeleteInvestigationGroup | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `73a5b33d15d7cd33df4610c3d59efb1de9382e21`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

All six operations that take an `{identifier}` path label — DeleteInvestigationGroup, GetInvestigationGroup, UpdateInvestigationGroup, PutInvestigationGroupPolicy, GetInvestigationGroupPolicy, DeleteInvestigationGroupPolicy — resolve the label through the single shared resolver `ComAmazonawsAiopsAiopsState::resolve_group_arn`, which first requires the text to parse as an investigation-group ARN (`group_id_from_arn`) and then requires it to equal the stored group's ARN byte for byte. A label that is the group's NAME therefore resolves to `None` and the operation answers the modelled `ResourceNotFoundException` (404) with `InvestigationGroup does not exist.`

The model says the opposite, in the documentation of the member itself, for every one of those six operations: the `identifier` member is documented as "Specify either the name or the ARN of the investigation group …". So a request that names a stored group by its name — a form the model documents as an address and whose shape the modelled pattern explicitly admits as the alternative `[\-_A-Za-z0-9]{1,512}` — is refused as if the group did not exist, for reads, updates, deletes and all three policy operations.

The crate is aware of the model text and decides against it on a recording claim (`state.rs:9-16`: "that alternative is a WELL-FORMEDNESS rule, not an address: recorded live responses resolve a group from its ARN alone"). That reason is exactly what the quoted member documentation contradicts: the model sentence does not describe what a name MEANS, it states what a caller may specify. Nothing else in the crate offers a second address: `resolve_group_arn` is the one resolver, so fixing it once fixes all six operations (and leaves the tagging operations' `resourceArn`, which the model really does document as "the ARN of the resource", untouched).

Filed against DeleteInvestigationGroup because it is the first operation on the checklist whose handler calls the shared resolver (`remove_group` -> `resolve_group_arn`); the same single defect is observable through all six identifier operations.

## Evidence

Smithy, quoted verbatim — the member documentation that itself states the behaviour (one per identifier operation):
- `smithy_query(query="doc",shape="DeleteInvestigationGroupRequest",member="identifier")`: "Specify either the name or the ARN of the investigation group that you want to delete."
- `smithy_query(query="doc",shape="GetInvestigationGroupRequest",member="identifier")`: "Specify either the name or the ARN of the investigation group that you want to view. This is used to set the name of the investigation group."
- `smithy_query(query="doc",shape="UpdateInvestigationGroupRequest",member="identifier")`: "Specify either the name or the ARN of the investigation group that you want to modify."
- `smithy_query(query="doc",shape="PutInvestigationGroupPolicyRequest",member="identifier")`: "Specify either the name or the ARN of the investigation group that you want to assign the policy to."
- `smithy_query(query="doc",shape="GetInvestigationGroupPolicyRequest",member="identifier")`: "Specify either the name or the ARN of the investigation group that you want to view the policy of."
The shape admits the bare-name form: `smithy_query(query="shape",shape="InvestigationGroupIdentifier")` -> `"pattern": "^(?:[\\-_A-Za-z0-9]{1,512}|arn:(aws|aws-us-gov|aws-cn|aws-iso|aws-iso-b):aiops:[a-zA-Z0-9-]*:[0-9]{12}:investigation-group\\/[A-Za-z0-9]{16})$"`.

The code that does the opposite — services/service-com-amazonaws-aiops-aiops/src/state.rs:243-249:
```
    pub fn resolve_group_arn(&self, resource_arn: &str, request_region: &str) -> Option<String> {
        let id = group_id_from_arn(resource_arn)?;
        self.investigation_groups
            .get(id)
            .filter(|group| group.arn == resource_arn && group.region == request_region)
            .map(|group| group.id.clone())
    }
```
`group_id_from_arn` returns `None` for anything that is not a six-field ARN of this service — services/service-com-amazonaws-aiops-aiops/src/state.rs:1794-1801:
```
pub fn group_id_from_arn(identifier: &str) -> Option<&str> {
    let [_arn, _partition, service, _region, _account, resource] = arn_fields(identifier)?;
    if service != ARN_SERVICE {
        return None;
    }
    let id = resource.strip_prefix(GROUP_ARN_RESOURCE_PREFIX)?;
    is_group_id(id).then_some(id)
}
```

The crate's own stated reason, which the quoted member docs contradict — services/service-com-amazonaws-aiops-aiops/src/state.rs:9-16:
```
//! A group is addressed by its ARN and by nothing else. The modelled
//! `InvestigationGroupIdentifier` admits a bare `[\-_A-Za-z0-9]{1,512}` label beside
//! the ARN, but that alternative is a WELL-FORMEDNESS rule, not an address: recorded
//! live responses resolve a group from its ARN alone and answer the modelled 404 for
//! every bare label — the label a stored group carries as its name included.
```

The 404 the handlers answer — services/service-com-amazonaws-aiops-aiops/src/ops/delete_investigation_group.rs:181-186:
```
        if store.remove_group(&input.identifier, &region).is_none() {
            return Err(DeleteInvestigationGroupErrorModeled::ResourceNotFoundException(
                crate::state::investigation_group_not_found(),
            )
            .into());
        }
```
and services/service-com-amazonaws-aiops-aiops/src/ops/get_investigation_group.rs:188-192:
```
        let group = store.group(&input.identifier, &region).ok_or_else(|| {
            GetInvestigationGroupErrorModeled::ResourceNotFoundException(
                crate::state::investigation_group_not_found(),
            )
        })?;
```
(the same resolver backs `group_mut` at state.rs:264-271 and `policy_group`/`policy_group_mut` at state.rs:279-295, so Update, Put/Get/DeleteInvestigationGroupPolicy answer the same way).

Test coverage that pins the current behaviour and must move with the fix — services/service-com-amazonaws-aiops-aiops/tests/all/investigation_group_name_identity.rs:10-16:
```
//! The rule under test: a group is addressed by its ARN alone, so a bare label — the
//! label a stored group carries as its name included — is a well-formed identifier that
//! resolves no group and is answered with the modelled `ResourceNotFoundException`
//! (404). The modelled pattern
//! (`smithy_query(query="shape",shape="InvestigationGroupIdentifier")`) admits the bare
//! label beside the ARN ... which is a well-formedness rule and not an address.
```
Its stated reason is the pattern's role plus the recording claim in state.rs:9-16; the member documentation quoted above is what contradicts that reason. The breadth of the rule is tabulated in tests/all/investigation_group_identifier_forms.rs and asserted per-operation in tests/all/investigation_group_name_identity.rs:251-284 (`a_name_addresses_no_group_however_it_is_formed`).

## Steps to Reproduce

1. POST /investigationGroups signed for us-west-2 with {"name":"my-group","roleArn":"arn:aws:iam::000000000000:role/AIOpsAssistantRole"} -> 201, note the returned arn
2. GET /investigationGroups/my-group (the stored NAME as the {identifier} label — a form the modelled pattern admits, so it reaches the handler) -> 404 ResourceNotFoundException "InvestigationGroup does not exist.", although GetInvestigationGroupRequest$identifier documents "Specify either the name or the ARN of the investigation group that you want to view."
3. GET /investigationGroups/<url-encoded arn> -> 200 with the group, proving the record exists and only the documented NAME form fails to address it
4. Repeat with the name for PATCH /investigationGroups/my-group, DELETE /investigationGroups/my-group, POST /investigationGroups/my-group/policy, GET /investigationGroups/my-group/policy and DELETE /investigationGroups/my-group/policy -> all 404, each against a member doc that says the name may be specified
5. Trace the single cause: every one of those handlers resolves through crate::state::resolve_group_arn (state.rs:243-249), whose first step group_id_from_arn (state.rs:1794-1801) answers None for any text that is not a six-field aiops investigation-group ARN
