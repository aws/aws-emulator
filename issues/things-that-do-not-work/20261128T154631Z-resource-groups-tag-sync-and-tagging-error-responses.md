Issue-Source: trace-sweep-divergence
Issue-Trace: cancel-tag-sync-task-refusals
Issue-Step: task#1
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: cancel-tag-sync-task-refusals group-tag-and-update-refusals
Issue-Shape-Id: com.amazonaws.resourcegroups#Ardi
Issue-Key: resource-groups|CancelTagSyncTask|4xx->5xx|$.Message
Issue-Key: resource-groups|CancelTagSyncTask|4xx->5xx|$headers.x-amzn-errortype
Issue-Key: resource-groups|CancelTagSyncTask|4xx->5xx|status
Issue-Key: resource-groups|CancelTagSyncTask|4xx->4xx|$.Message
Issue-Key: resource-groups|CancelTagSyncTask|4xx->2xx|$
Issue-Key: resource-groups|CancelTagSyncTask|4xx->2xx|$headers.x-amzn-errortype
Issue-Key: resource-groups|CancelTagSyncTask|4xx->2xx|status
Issue-Key: resource-groups|GetTagSyncTask|4xx->4xx|$.Message
Issue-Key: resource-groups|Tag|4xx->4xx|$.Message
Issue-Commit: 2ad04cd39860f70eb0b7d33f623aafe0280db657

# Resource Groups tag-sync and tagging error responses diverge from AWS status and message

> _Note: this issue was identified against the code base at commit `2ad04cd39860f70eb0b7d33f623aafe0280db657`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CancelTagSyncTask

## Description
The refusal paths of the tag-sync-task operations and of Tag answer with the wrong status or with messages the emulator words itself instead of the way AWS words them, and one refusal is missing entirely (a cancel of a non-existent task succeeds). Five causes, all in resource-groups refusal handling; all five must be closed.

Cause 1 of 5 - unroutable method/path answers 501 instead of 403.
AWS: recorded behavior only (the front door, not an operation in the model): a request that does not resolve to a bound operation - here GET /cancel-tag-sync-task, where Smithy binds CancelTagSyncTask to POST /cancel-tag-sync-task - is refused with HTTP 403 and the fixed message "Unable to determine service/operation name to be authorized" (no variable parts) plus an x-amzn-errortype header. Emulator: HTTP 501 with x-amzn-errortype NotImplemented and the emulator's own message "operation GET /cancel-tag-sync-task is not implemented in this emulator". Why: a client probing or mis-signing a request sees a server-side "not implemented" (and SDK retry/classification for 501 differs from 403), and an emulator-specific sentence leaks into client logs. Fix: the service's router fallback for unmatched method/URI, which must emit the 403 refusal rather than a NotImplemented stub. Trace: cancel-tag-sync-task-refusals#task#1.

Cause 2 of 5 - null-member validation message renders a value token AWS omits.
AWS: Smithy - CancelTagSyncTaskInput.TaskArn is @required; the refusal is BadRequestException (httpError 400). The message's fixed text is "<n> validation error detected: Value at '<member>' failed to satisfy constraint: Member must not be null", with the member name taken from the request shape (taskArn) and the count from the number of violations; for a null/absent member AWS renders NO value token after "Value". Emulator: same 400, but the message is "1 validation error detected: Value null at 'taskArn' failed to satisfy constraint: Member must not be null" - it inserts "null" where AWS prints nothing. Why: clients and tests that match the AWS validation-message template fail to parse or match it. Fix: the shared input-validation message formatter (omit the value rendering when the value is absent). Trace: cancel-tag-sync-task-refusals#task#2.

Cause 3 of 5 - cancelling a non-existent tag-sync task succeeds instead of 404.
AWS: recorded behavior (the model lists NotFoundException for GetTagSyncTask but not for CancelTagSyncTask): a CancelTagSyncTask whose TaskArn matches the ARN pattern but names no existing task is refused with HTTP 404, an x-amzn-errortype header and the message "Cannot find task: <TaskArn>" - fixed text "Cannot find task: " followed by the TaskArn exactly as supplied in the request. Emulator: HTTP 200 with an empty body and no error type header, i.e. it reports a successful cancel of a task that does not exist. Why: a client treats a bogus or already-deleted task ARN as cancelled and never learns the task is gone, hiding typos and masking cleanup bugs. Fix: the CancelTagSyncTask handler must look the task up in state and raise the not-found refusal before reporting success. Trace: cancel-tag-sync-task-refusals#task#3.

Cause 4 of 5 - GetTagSyncTask not-found message is not AWS's.
AWS: Smithy - GetTagSyncTask errors include NotFoundException (httpError 404, member Message); the recorded message is "Cannot find task: <TaskArn>", whose fixed text is "Cannot find task: " and whose only variable part is the TaskArn from the request. Emulator: correct 404, but Message "The specified tag-sync task does not exist" - different wording and it drops the ARN. Why: a client that surfaces or matches the service message loses the identifier that says WHICH task was missing. Fix: the GetTagSyncTask handler's not-found error construction. Trace: cancel-tag-sync-task-refusals#task#4.

Cause 5 of 5 - Tag reserved-prefix message is not AWS's.
AWS: Smithy - Tag (PUT /resources/{Arn}/tags) refuses a reserved tag key with BadRequestException (httpError 400); the recorded message is the fixed sentence "Tag keys must not start with 'aws:'", with no variable parts. Emulator: correct 400, but Message "Tags is not valid: the aws: prefix is reserved, so the tag key aws:edge is not one a caller can change" - its own prose, which also interpolates the offending key where AWS interpolates nothing. Why: clients keying off the AWS sentence (or asserting it in tests) do not match. Fix: the Tag handler's tag-key validation error message. Trace: group-tag-and-update-refusals#task#2 (same cluster: task#3).

## Evidence
cancel-tag-sync-task-refusals#task#1 (CancelTagSyncTask, GET /cancel-tag-sync-task): status expected 403, got 501; `$.Message` expected 'Unable to determine service/operation name to be authorized', got 'operation GET /cancel-tag-sync-task is not implemented in this emulator'; `x-amzn-errortype` expected present (shape not asserted), got 'NotImplemented'.
cancel-tag-sync-task-refusals#task#2 (CancelTagSyncTask, missing taskArn): status 400/400; `$.Message` expected "1 validation error detected: Value at 'taskArn' failed to satisfy constraint: Member must not be null", got "1 validation error detected: Value null at 'taskArn' failed to satisfy constraint: Member must not be null".
cancel-tag-sync-task-refusals#task#3 (CancelTagSyncTask, unknown task ARN): status expected 404, got 200; body expected {'Message': 'Cannot find task: <TaskArn>'}, got {}; `x-amzn-errortype` expected present, got None.
cancel-tag-sync-task-refusals#task#4 (GetTagSyncTask, unknown task ARN): status 404/404; `$.Message` expected 'Cannot find task: <TaskArn>', got 'The specified tag-sync task does not exist'.
group-tag-and-update-refusals#task#2 and #task#3 (Tag with a key under the aws: prefix): status 400/400; `$.Message` expected "Tag keys must not start with 'aws:'", got 'Tags is not valid: the aws: prefix is reserved, so the tag key <key> is not one a caller can change'.

## Steps to Reproduce
1. Boot the emulator and replay trace set simparity-wire-aws-v1.
2. Send GET /cancel-tag-sync-task (fixture cancel-tag-sync-task-refusals, step task#1): 501 NotImplemented instead of 403 'Unable to determine service/operation name to be authorized'.
3. POST /cancel-tag-sync-task with no TaskArn (step task#2): the 400 message contains the extra 'null' value token.
4. POST /cancel-tag-sync-task with a well-formed TaskArn naming no existing task (step task#3): 200 empty body instead of 404 'Cannot find task: <TaskArn>'.
5. POST /get-tag-sync-task with the same ARN (step task#4): 404 but with the emulator's own message instead of 'Cannot find task: <TaskArn>'.
6. PUT /resources/{Arn}/tags with a tag key beginning 'aws:' (fixture group-tag-and-update-refusals, steps task#2 and task#3): 400 but with the emulator's prose instead of "Tag keys must not start with 'aws:'".
