Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: a648c95e8281c95edbf892975ca6b5e5d5c3850b

# CreateWorkspace stores tag keys its sibling TagResource refuses as out of bounds

_Filed: 2026-11-27T14:48:18Z_
_Operation: CreateWorkspace | Class: validation_

> _Note: this issue was identified against the code base at commit `a648c95e8281c95edbf892975ca6b5e5d5c3850b`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`CreateWorkspace` writes the request's `tags` map straight onto the new workspace record without running the crate's own tag-bounds check, while the sibling handler that writes the same map (`TagResource`) refuses a key outside `TagKey`'s modelled `@length(min: 1, max: 128)` with the modelled `ValidationException` through `crate::state::out_of_bounds_tag`.

The crate states explicitly that this check is the HANDLER's job, because the request-boundary validator does not descend into a map's keys: `state::out_of_bounds_tag`'s own doc comment and the `resource_tags` test both say so. `CreateWorkspace` is the second handler in this crate that writes a `TagMap` and it skips the helper, so `POST /workspaces` with `tags` carrying a 129-character key (or the empty key, which `TagKey`'s `min: 1` excludes) answers 202 and persists the pair, after which `ListTagsForResource` and `DescribeWorkspace`/`ListWorkspaces` report a tag that the same service refuses to accept through `TagResource` on the very same workspace. One documented rule is therefore enforced on one path and not on the other.

## Evidence

Model: `TagKey` is `{"shape_id": "com.amazonaws.grafana#TagKey", "type": "string", "length": {"min": 1, "max": 128}}` and `CreateWorkspaceRequest$tags` targets `TagMap` whose `key` is that shape (`smithy_query(query="shape", shape="TagKey,TagMap")`).

The crate says the handler must enforce it — services/service-com-amazonaws-grafana-awsgrafanacontrolplane/src/state.rs:1320-1323:
```
/// The request-boundary validator refuses a modelled constraint on a member, but
/// does not walk a map's KEY and VALUE constraints, so the handler that writes
/// tags is what enforces these two. Lengths are counted in characters, as
/// `@length` on a string is defined.
```

The sibling that writes tags DOES enforce it — src/ops/tag_resource.rs:185-198:
```
        // The request-boundary validator refuses a modelled constraint on a member,
        // but does not walk a MAP's key and value constraints, so `TagKey` and
        // `TagValue` are enforced here. ...
        for (key, value) in &submitted {
            if let Some(refusal) = crate::state::out_of_bounds_tag(key, value) {
                return Err(TagResourceErrorModeled::ValidationException(refusal).into());
            }
        }
```

The handler under review does not — src/ops/create_workspace.rs:374 (the only place it touches `input.tags`):
```
            tags: input.tags.unwrap_or_default().into_iter().collect(),
```
No call to `out_of_bounds_tag` appears anywhere in create_workspace.rs.

Test coverage: tests/all/resource_tags.rs:503-510 pins the rule for `TagResource` only, and states which layer raises which refusal — "a tag VALUE is walked by the service's request-body validator, which refuses the body as a bad request, while a tag KEY is a map key the walk does not descend into, so the handler refuses it with the modelled `ValidationException`." The table at resource_tags.rs:513-556 drives these rows through `tag()` (`POST /tags/{arn}`) only; `create_workspace()` (resource_tags.rs:138-145) is never called with an out-of-bounds key, so no test pins the create path.

## Steps to Reproduce

1. Start the emulator with this service mounted.
2. POST /workspaces with body {"accountAccessType":"CURRENT_ACCOUNT","permissionType":"CUSTOMER_MANAGED","authenticationProviders":["AWS_SSO"],"tags":{"<129 'k' characters>":"v"}} — observe HTTP 202 and the workspace id.
3. GET /tags/<percent-encoded workspace ARN> — observe the 129-character tag key reported back, i.e. it was persisted.
4. POST /tags/<percent-encoded workspace ARN> with body {"tags":{"<129 'k' characters>":"v"}} — observe HTTP 400 ValidationException from crate::state::out_of_bounds_tag for the identical pair, proving the two writers of the same map disagree.
5. Repeat step 2 with the empty tag key "" (excluded by TagKey's min: 1) for the same divergence.
