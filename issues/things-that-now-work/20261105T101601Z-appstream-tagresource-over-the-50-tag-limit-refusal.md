Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: appstream.tag-resource-limit-exceeded-rejected
Issue-Shape-Id: com.amazonaws.appstream#PhotonAdminProxyService
Issue-Key: appstream.tag-resource-limit-exceeded-rejected|run#11|AssertionError|assert got == ("LimitExceededException", expected_msg), got
Issue-Commit: f9b3c9022f94c1cc6cbcc4d2fcdd3a22bf2863ff

# appstream TagResource over-the-50-tag-limit refusal carries the generic LimitExceededException doc sentence instead of AWS's tag-limit message

> _Note: this issue was identified against the code base at commit `f9b3c9022f94c1cc6cbcc4d2fcdd3a22bf2863ff`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
TagResource

## Description
Cause 1 of 1: TagResource refuses a request that would push a resource past its 50-tag ceiling with the right error code but the wrong message text.

AWS: Smithy (services/service-com-amazonaws-appstream-photonadminproxyservice/smithy-model/com-amazonaws-appstream-photonadminproxyservice.json) models TagResource with errors InvalidAccountStatusException (HTTP 400), LimitExceededException (HTTP 400) and ResourceNotFoundException (HTTP 404); TagResourceRequest.Tags is a map with @length min 1 / max 50. The model's prose for the LimitExceededException SHAPE is only the generic "The requested limit exceeds the permitted limit for an account." — it does not state the message of this particular refusal. Recorded behavior: when a TagResource would leave the resource carrying more than 50 tags, AWS answers HTTP 400, error code LimitExceededException, message "The resource already has the maximum of 50 tags. Delete tags from the specified resource as needed." That whole sentence is FIXED text for this error class except the tag ceiling (50), which is the per-resource tag limit the service enforces; nothing in it comes from the request (no ARN, key or value is interpolated). The refusal is state-based — the request itself carried a single, legal tag — and it persists nothing, which the emulator already gets right.

Emulator: the code and status are correct, but the message is the generic shape-documentation sentence: ('LimitExceededException', 'The requested limit exceeds the permitted limit for an account.'). The handler builds it from crate::state::tags_limit_exceeded_message(), which hardcodes the LimitExceededException shape's doc text rather than the tag-limit wording for this refusal, so the limit the caller actually crossed is never named.

Why: a client that distinguishes AppStream's limit refusals by message — a quota-handling branch, a retry/backoff decision, an operator log or an assertion on the surfaced error text — cannot tell "this resource is already at its 50-tag maximum, delete tags" from any other account-limit refusal, because the emulator collapses every LimitExceededException onto one sentence. LimitExceededException is also the emulator's own internal store-ceiling refusal (see state.rs MAX_RECORDS_PER_STORE), so the two are textually indistinguishable to a caller.

Fix: the TagResource handler's limit-refusal path and the message helper it calls — the per-resource tag ceiling needs its own message distinct from the generic shape sentence, with the ceiling value substituted from the enforced limit.

Traces: appstream.tag-resource-limit-exceeded-rejected. Citations: services/service-com-amazonaws-appstream-photonadminproxyservice/src/ops/tag_resource.rs lines 172-184 (the limit check and the LimitExceededException it returns); services/service-com-amazonaws-appstream-photonadminproxyservice/src/state.rs lines 245-261 (MAX_TAGS_PER_RESOURCE and tags_limit_exceeded_message()). No open issue covers this: the nearest ones (20261102T092115Z-appstream-createstack-updatestack-request-validation-diverges-in-class.md, 20261102T120557Z-appstream-ec2-7-net-new-appstream-root-causes.md) are about CreateStack/UpdateStack AccessEndpoints validation, not tagging.

## Evidence
Expected (AWS, from the recording the check replays): TagResource on a resource that already carries 50 tags, adding one new key -> HTTP 400, Error.Code "LimitExceededException", Error.Message "The resource already has the maximum of 50 tags. Delete tags from the specified resource as needed."

Actual (emulator): HTTP 400, Error.Code "LimitExceededException" (correct), Error.Message "The requested limit exceeds the permitted limit for an account."

Replay output:
  File "check_appstream_tag_resource_limit_exceeded_rejected.py", line 33, in run
    assert got == ("LimitExceededException", expected_msg), got
  AssertionError: ('LimitExceededException', 'The requested limit exceeds the permitted limit for an account.')

Message anatomy: the fixed text is "The resource already has the maximum of " + <per-resource tag limit> + " tags. Delete tags from the specified resource as needed."; the only variable part is the limit (50 here, the ceiling the service enforces). No request value (ResourceArn, tag key or value) appears in it.

Side effects were correct: the rejected 51st key did not leak and the resource still listed exactly 50 tags, so only the message diverges.

## Steps to Reproduce
1. appstream.create_stack(Name=<unique>) and take Stack.Arn.
2. appstream.tag_resource(ResourceArn=arn, Tags={50 distinct keys}) -> succeeds; list_tags_for_resource reports 50 tags.
3. appstream.tag_resource(ResourceArn=arn, Tags={"<one new key>": "v"}).
4. Observe the ClientError: code is LimitExceededException (correct) but the message is "The requested limit exceeds the permitted limit for an account." instead of AWS's "The resource already has the maximum of 50 tags. Delete tags from the specified resource as needed."
5. list_tags_for_resource still reports exactly the original 50 tags (correct), and a net-zero overwrite of an existing key still succeeds (correct).
