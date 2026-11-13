Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: rolesanywhere.tag-resource-exceeds-tag-limit-rejected
Issue-Shape-Id: com.amazonaws.rolesanywhere#RolesAnywhere
Issue-Key: rolesanywhere.tag-resource-exceeds-tag-limit-rejected|_client_error#11|AssertionError|raise AssertionError(f"expected the call to be rejected, but it SUCCEEDED and returned {result!r}")
Issue-Commit: 4226368beaf76fbb89a0358ac027d2e5fc79db7f

# rolesanywhere TagResource enforces a 200-tag ceiling instead of AWS's 50-user-tag quota, so a 51st tag is accepted

> _Note: this issue was identified against the code base at commit `4226368beaf76fbb89a0358ac027d2e5fc79db7f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
TagResource

## Description
Cause 1 of 1: TagResource does not enforce the IAM Roles Anywhere per-resource user-tag quota, so a call that would carry a resource past 50 user tags is accepted and stored instead of being refused.

AWS: Smithy for this service models `TagResource` with input `tags` targeting `TagList` `@length(min 0, max 200)` and errors `AccessDeniedException` (403), `ResourceNotFoundException` (404), `TooManyTagsException` (400) and `ValidationException` (400); success status is `http.code 201` (`smithy_query(query="operation", operation="TagResource")`). The model therefore says nothing about a 50-tag quota — recorded behavior only: on real AWS a resource already holding 50 user tags refuses one more `TagResource` with HTTP 400, error code `ValidationException` (NOT the modeled `TooManyTagsException`) and message `The resultant tag set must not have more than 50 user tags.` That whole message is FIXED text for this error class — it has no variable parts taken from the request or from state (no tag key, value or ARN is interpolated, consistent with tag keys and values being `@sensitive`). The refused call is atomic: the extra tag is not persisted and `ListTagsForResource` still reports exactly 50 tags afterwards. Freeing a slot with `UntagResource` then makes the identical `TagResource` succeed.

Emulator: the 51st tag is ACCEPTED. The replayed check created a profile already at 50 tags (`ListTagsForResource` confirmed 50), then called `TagResource` with one more tag `{key: "one-too-many"}` and got a success response — HTTP 201 with an empty JSON body (`{'ResponseMetadata': {'HTTPStatusCode': 201, ...'content-length': '2'}}`) — rather than a 400 `ValidationException`. No refusal of any kind is produced at that boundary, so the tag is also written into the resource's tag list. The handler does have a ceiling, but it is set at 200: `crate::state::MAX_TAGS_PER_RESOURCE` is derived from the `TagList` `@length` max rather than from the service's user-tag quota, and when that ceiling IS crossed the handler answers the modeled `TooManyTagsException` with message `Too many tags.`, which is a second divergence on the same path — AWS answers `ValidationException` with the wording above for this quota.

Why: a client that relies on AWS's quota enforcement gets no signal that it has exceeded the per-resource user-tag limit. Tag-management code that catches `ValidationException`/`TooManyTagsException` to prune or batch tags never takes that branch, so it silently builds resources holding more than 50 user tags — a state real AWS cannot produce. Conversely, any client that does reach the emulator's 200 ceiling sees the wrong error code (`TooManyTagsException` instead of `ValidationException`) and wrong message, so error-code-driven retry/pruning logic written against AWS does not match. Tests and tooling that assert the quota refusal pass against AWS and fail against the emulator (and vice versa).

Fix: belongs in the `TagResource` handler's validation, together with the tag-quota constant it reads in this service's state module. The quota the refusal is computed against must be the 50-user-tag quota rather than the `TagList` `@length` bound, the comparison must be made on the MERGED tag set (re-tagging keys the resource already holds must stay accepted at the quota, as it is today), and the refusal must be emitted as `ValidationException` with the fixed message quoted above, before any tag is written, leaving the stored tag list unchanged. The success path must keep returning HTTP 201 with an empty body, which already matches the model.

Traces: rolesanywhere.tag-resource-exceeds-tag-limit-rejected. File/line citations: services/service-com-amazonaws-rolesanywhere-rolesanywhere/src/ops/tag_resource.rs lines 151-160 (the ceiling check and the `TooManyTagsException` it raises); services/service-com-amazonaws-rolesanywhere-rolesanywhere/src/state.rs lines 66-73 (`MAX_TAGS_PER_RESOURCE = 200`) and lines 313-322 (`merged_tag_count`).

Note: this check's positive control also calls `UntagResource`, whose own defect is already tracked by the open issue 20261030T161915Z-rolesanywhere-untagresource-does-not-remove-tags-from-resource.md; that issue does NOT cover the missing tag-quota refusal reported here, which is why this is filed separately. No other open rolesanywhere issue names the TagResource quota.

## Evidence
Check: rolesanywhere.tag-resource-exceeds-tag-limit-rejected (verdict: mismatch).

Setup (all succeeded): CreateProfile with 50 tags; ListTagsForResource returned 50 tags, so the resource was at the quota.

Expected (real AWS, per the recording the check encodes):
  HTTP 400
  Error.Code    = "ValidationException"
  Error.Message = "The resultant tag set must not have more than 50 user tags."   <- entirely fixed text; no request/state values interpolated
  and the extra tag is NOT persisted (ListTagsForResource still 50, no key "one-too-many").

Actual (emulator): the call SUCCEEDED.
  AssertionError: expected the call to be rejected, but it SUCCEEDED and returned
  {'ResponseMetadata': {'HTTPStatusCode': 201, 'HTTPHeaders': {'content-type': 'application/json', 'content-length': '2', ...}, 'RetryAttempts': 0}}

Secondary divergence on the same path (from source, reached once >200 tags are merged):
  Emulator raises TooManyTagsException / message "Too many tags."
  AWS raises ValidationException / message "The resultant tag set must not have more than 50 user tags."

Source: services/service-com-amazonaws-rolesanywhere-rolesanywhere/src/state.rs:73 -> `pub const MAX_TAGS_PER_RESOURCE: usize = 200;`
        services/service-com-amazonaws-rolesanywhere-rolesanywhere/src/ops/tag_resource.rs:151 -> `if crate::state::merged_tag_count(held, &input.tags) > crate::state::MAX_TAGS_PER_RESOURCE`

Model consulted: smithy_query(query="operation", operation="TagResource", model_path="services/service-com-amazonaws-rolesanywhere-rolesanywhere/smithy-model/com-amazonaws-rolesanywhere-rolesanywhere.json") -> tags: TagList @length(0,200); errors AccessDeniedException/403, ResourceNotFoundException/404, TooManyTagsException/400, ValidationException/400; http {POST /TagResource, code 201}. The model states no 50-tag quota, so the quota and its wording are recorded behavior only.

## Steps to Reproduce
1. Boot the emulator and point a rolesanywhere client at it.
2. account_id = sts.get_caller_identity()["Account"]; role_arn = f"arn:aws:iam::{account_id}:role/<any-role-name>".
3. CreateProfile(name=<unique>, roleArns=[role_arn], tags=[{"key": f"tag-{i}", "value": "v"} for i in range(50)]) -> note profileArn.
4. ListTagsForResource(resourceArn=profileArn) -> confirm 50 tags (the resource is at the quota).
5. TagResource(resourceArn=profileArn, tags=[{"key": "one-too-many", "value": "v"}]).
   Expected: HTTP 400, Code "ValidationException", Message "The resultant tag set must not have more than 50 user tags."
   Actual:   HTTP 201 with an empty JSON body; the call succeeds.
6. ListTagsForResource(resourceArn=profileArn) -> expected still 50 tags with no "one-too-many" key; the emulator has persisted the 51st tag.
7. Re-tagging an existing key at the quota, e.g. TagResource(resourceArn=profileArn, tags=[{"key": "tag-0", "value": "v2"}]), must STILL succeed after the fix (the merged set stays at 50).
