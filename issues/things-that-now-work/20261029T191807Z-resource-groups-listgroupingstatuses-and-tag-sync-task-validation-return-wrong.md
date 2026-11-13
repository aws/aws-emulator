Issue-Source: trace-sweep-divergence
Issue-Trace: resource-groups-group-resources-wrong-target-rejected
Issue-Step: task#4
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: resource-groups-group-resources-wrong-target-rejected resource-groups-tag-sync-task-listing-and-validation
Issue-Shape-Id: com.amazonaws.resourcegroups#Ardi
Issue-Key: resource-groups|ListGroupingStatuses|4xx->2xx|$.Message
Issue-Key: resource-groups|ListGroupingStatuses|4xx->2xx|$headers.x-amzn-errortype
Issue-Key: resource-groups|ListGroupingStatuses|4xx->2xx|status
Issue-Key: resource-groups|ListGroupingStatuses|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: resource-groups|ListGroupingStatuses|4xx->4xx|status
Issue-Key: resource-groups|GetTagSyncTask|4xx->4xx|$.Message
Issue-Key: resource-groups|CancelTagSyncTask|4xx->4xx|$.Message
Issue-Commit: 907b6e75f03592bc638357f3d5c791839011739f

# ListGroupingStatuses and tag-sync-task validation return wrong status or omit Message

> _Note: this issue was identified against the code base at commit `907b6e75f03592bc638357f3d5c791839011739f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListGroupingStatuses

## Description
This issue bundles 4 causes with one root: request validation on these operations either does not run at all, picks the wrong error class, or raises the right status with no Message body. All four must be closed before this issue is submitted.

Cause 1 of 4: ListGroupingStatuses accepts a group that is not an application group.
AWS: Smithy: ListGroupingStatuses input Group is required (GroupStringV2) and the operation declares BadRequestException (httpError 400, member Message). Recorded response: HTTP 400 with $.Message exactly 'The target must be an application group' (fixed text, no interpolation) and the x-amzn-errortype header set for the error shape.
Emulator: HTTP 200 with a normal (empty) listing body, no $.Message and no x-amzn-errortype header — the eligibility check on the target group is simply absent.
Why: a client that calls ListGroupingStatuses on a non-application group is told the call succeeded and that there are no grouping statuses, instead of learning the group type is wrong; the error path is unreachable, so callers cannot test it.
Fix: the ListGroupingStatuses handler's target validation, before the listing is evaluated.
Traces: resource-groups-group-resources-wrong-target-rejected#task#4.

Cause 2 of 4: ListGroupingStatuses answers 400 instead of 404 for a group that does not exist.
AWS: Recorded behavior only for the status mapping (the on-disk model binds BadRequestException/Forbidden/InternalServerError/MethodNotAllowed/TooManyRequests to this operation and does not bind NotFoundException): the recording shows HTTP 404 with an error-shaped body for a Group that does not exist, i.e. a not-found condition, not a malformed request.
Emulator: HTTP 400 with x-amzn-errortype 'BadRequestException'.
Why: a client distinguishing 'you asked wrongly' (400, do not retry, fix the request) from 'the group is gone' (404, treat as absent) takes the wrong branch — a reconciliation loop that tolerates a deleted group instead reports a client error.
Fix: the ListGroupingStatuses handler's group-lookup miss path, which must map to the not-found error class/404 rather than to BadRequestException.
Traces: resource-groups-group-resources-wrong-target-rejected#task#5.

Cause 3 of 4: GetTagSyncTask rejects a malformed TaskArn with no Message.
AWS: Smithy: GetTagSyncTask input TaskArn is required, targets TagSyncTaskArn with length 12..1600 and pattern ^arn:aws(-[a-z]+)*:resource-groups:[a-z]{2}(-[a-z]+)+-\d{1}:[0-9]{12}:group/[a-zA-Z0-9_\.-]{1,150}/[a-z0-9]{26}/tag-sync-task/[a-z0-9]{26}$; the operation declares BadRequestException (400) with a Message member. Recorded response: HTTP 400 whose $.Message is the standard constraint-violation report: '1 validation error detected: Value at '<member>' failed to satisfy constraint: Member must satisfy regular expression pattern: <the member's modelled pattern>'. The fixed parts are the count-and-phrase template and the 'failed to satisfy constraint: Member must satisfy regular expression pattern:' clause; the variable parts are the member name taken from the request ('taskArn') and the pattern taken from the model.
Emulator: HTTP 400 (status correct) with $.Message entirely absent, so the error body carries no explanation.
Why: a client surfacing the validation text to a user or a log gets an empty reason, and a test asserting which member failed validation cannot tell taskArn from any other member.
Fix: the GetTagSyncTask input validation, which must emit the constraint-violation Message when the TaskArn pattern does not match.
Traces: resource-groups-tag-sync-task-listing-and-validation#task#2.

Cause 4 of 4: CancelTagSyncTask rejects a malformed TaskArn with no Message.
AWS: Smithy: CancelTagSyncTask input TaskArn required, same TagSyncTaskArn pattern and length as cause 3; BadRequestException (400) with Message. Recorded response: HTTP 400 with the same '1 validation error detected: Value at 'taskArn' failed to satisfy constraint: Member must satisfy regular expression pattern: <pattern>' Message, built the same way.
Emulator: HTTP 400 with $.Message absent.
Why: same as cause 3.
Fix: the CancelTagSyncTask input validation (shared TagSyncTaskArn validator with cause 3).
Traces: resource-groups-tag-sync-task-listing-and-validation#task#3.

Related but not covering: open issue 20261029T094111Z-resource-groups-listgroupingstatuses-validation concerns a missing per-request Filters bound and lock handling, not the target check or the not-found status mapping.

## Evidence
Replay of simparity-wire-aws-v1 (all verdict=mismatch):
- resource-groups-group-resources-wrong-target-rejected#task#4 ListGroupingStatuses: status expected 400, got 200; $.Message missing (expected 'The target must be an application group'); $headers.x-amzn-errortype missing.
- resource-groups-group-resources-wrong-target-rejected#task#5 ListGroupingStatuses: status expected 404, got 400; $headers.x-amzn-errortype shape_mismatch (got 'BadRequestException').
- resource-groups-tag-sync-task-listing-and-validation#task#2 GetTagSyncTask, 400->400: $.Message missing (expected "1 validation error detected: Value at 'taskArn' failed to satisfy constraint: Member must satisfy regular expression pattern: arn:aws(-[a-z]+)*:resource-groups(-(test|beta|gamma))?:[a-z]{2,4}(-[a-z]+)+-\\d{1}:[0-9]{12}:group/[a-zA-Z0-9_\\.-]{1,150}/[a-z0-9]{26}/tag-sync-task/[a-z0-9]{26}").
- resource-groups-tag-sync-task-listing-and-validation#task#3 CancelTagSyncTask, 400->400: $.Message missing, same expected text.

## Steps to Reproduce
1. Replay fixture resource-groups-group-resources-wrong-target-rejected step task#4: ListGroupingStatuses on a group that is not an application group must answer 400 BadRequestException with $.Message 'The target must be an application group' and an x-amzn-errortype header; the emulator answers 200 with an empty listing.
2. Same fixture, step task#5: ListGroupingStatuses for a group that does not exist must answer 404 (not-found error shape); the emulator answers 400 BadRequestException.
3. Replay fixture resource-groups-tag-sync-task-listing-and-validation steps task#2 and task#3: GetTagSyncTask and CancelTagSyncTask with a TaskArn that does not match the modelled TagSyncTaskArn pattern must answer 400 with the '1 validation error detected: Value at 'taskArn' failed to satisfy constraint: Member must satisfy regular expression pattern: <pattern>' Message; the emulator returns 400 with no Message.
