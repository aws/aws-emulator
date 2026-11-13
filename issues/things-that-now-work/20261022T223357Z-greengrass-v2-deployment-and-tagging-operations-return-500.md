Issue-Source: trace-sweep-divergence
Issue-Trace: greengrassv2-deployment-error-paths
Issue-Step: task#1
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: greengrassv2-deployment-error-paths greengrassv2-deployment-tag-round-trip greengrassv2-list-variations greengrassv2-resource-tagging-error-paths
Issue-Shape-Id: com.amazonaws.greengrassv2#GreengrassV2
Issue-Key: greengrass|CreateDeployment|4xx->5xx|$.message
Issue-Key: greengrass|CreateDeployment|4xx->5xx|$headers.content-type
Issue-Key: greengrass|CreateDeployment|4xx->5xx|$headers.x-amzn-errortype
Issue-Key: greengrass|CreateDeployment|4xx->5xx|status
Issue-Key: greengrass|GetDeployment|4xx->5xx|$.message
Issue-Key: greengrass|GetDeployment|4xx->5xx|$headers.content-type
Issue-Key: greengrass|GetDeployment|4xx->5xx|$headers.x-amzn-errortype
Issue-Key: greengrass|GetDeployment|4xx->5xx|status
Issue-Key: greengrass|CancelDeployment|4xx->5xx|$.message
Issue-Key: greengrass|CancelDeployment|4xx->5xx|$headers.content-type
Issue-Key: greengrass|CancelDeployment|4xx->5xx|$headers.x-amzn-errortype
Issue-Key: greengrass|CancelDeployment|4xx->5xx|status
Issue-Key: greengrass|DeleteDeployment|4xx->5xx|$.message
Issue-Key: greengrass|DeleteDeployment|4xx->5xx|$headers.content-type
Issue-Key: greengrass|DeleteDeployment|4xx->5xx|$headers.x-amzn-errortype
Issue-Key: greengrass|DeleteDeployment|4xx->5xx|status
Issue-Key: greengrass|CreateDeployment|2xx->5xx|status
Issue-Key: greengrass|ListDeployments|2xx->5xx|$.deployments
Issue-Key: greengrass|ListDeployments|2xx->5xx|$headers.content-type
Issue-Key: greengrass|ListDeployments|2xx->5xx|status
Issue-Key: greengrass|TagResource|4xx->5xx|$.message
Issue-Key: greengrass|TagResource|4xx->5xx|$headers.content-type
Issue-Key: greengrass|TagResource|4xx->5xx|$headers.x-amzn-errortype
Issue-Key: greengrass|TagResource|4xx->5xx|status
Issue-Key: greengrass|UntagResource|4xx->5xx|$.message
Issue-Key: greengrass|UntagResource|4xx->5xx|$headers.content-type
Issue-Key: greengrass|UntagResource|4xx->5xx|$headers.x-amzn-errortype
Issue-Key: greengrass|UntagResource|4xx->5xx|status
Issue-Key: greengrass|ListTagsForResource|4xx->5xx|$.message
Issue-Key: greengrass|ListTagsForResource|4xx->5xx|$headers.content-type
Issue-Key: greengrass|ListTagsForResource|4xx->5xx|$headers.x-amzn-errortype
Issue-Key: greengrass|ListTagsForResource|4xx->5xx|status
Issue-Commit: 6e0b8b1107b57545eaa8864803a2ad1310139146

# Greengrass V2 deployment and tagging operations return 500 AmbiguousService before dispatch

> _Note: this issue was identified against the code base at commit `6e0b8b1107b57545eaa8864803a2ad1310139146`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateDeployment

## Description
This bundle carries 9 causes, all on the Greengrass V2 deployment and resource-tagging surface. Shared fault: the emulator's router keys emulated services by endpoint prefix, and Greengrass (2017-06-07) and GreengrassV2 (2020-11-30) both declare endpointPrefix/arnNamespace/sigv4Name 'greengrass' (Smithy: service_info on both models), so requests are refused before any handler runs with HTTP 500, content-type application/x-amz-json-1.1 and body {"__type": "AmbiguousService", "message": "2 emulated services all claim requests for 'greengrass': ... rejected rather than served by an arbitrary one"}. AWS co-hosts both API versions on greengrass.<region>.amazonaws.com and routes by the restJson1 @http method+uri binding. The open behavioral-corpus issue 20261022T092619Z-greengrassv2-every-request-rejected-with-ambiguousservice-because-greengrass.md names the same root cause but carries no wire replay key, so these fixture bindings are net-new.

Cause 1 of 9 - CreateDeployment against a target that does not exist.
AWS: Smithy GreengrassV2.CreateDeployment, POST /greengrass/v2/deployments, success code 201; targetArn is @required and matches ^arn:[^:]*:iot:[^:]*:[0-9]+:(thing|thinggroup)/.+$; modeled errors include ValidationException httpError 400 and ResourceNotFoundException 404. Recorded response: 400, x-amzn-errortype ValidationException, content-type application/json, message (example) 'Invalid Input: Target not found'.
Emulator: 500, x-amzn-errortype AmbiguousService.
Why: a client cannot distinguish a bad target from a server fault, so it retries a request that can never succeed.
Fix: the request router / service-registry dispatch layer (no handler is reached), plus the restJson1 error serializer emitting x-amz-json-1.1.
Traces: greengrassv2-deployment-error-paths#task#1 (also #task#2).

Cause 2 of 9 - GetDeployment on a nonexistent deployment.
AWS: ResourceNotFoundException modeled with httpError 404. Recorded response: 404, x-amzn-errortype ResourceNotFoundException, message (example) 'Deployment not found'.
Emulator: 500 / AmbiguousService.
Why: deployment status polling cannot tell 'no such deployment' from a transient failure.
Fix: same dispatch layer; then GetDeployment's lookup.
Traces: greengrassv2-deployment-error-paths#task#3.

Cause 3 of 9 - CancelDeployment on a nonexistent deployment.
AWS: ResourceNotFoundException modeled, httpError 404. Recorded response: 404, x-amzn-errortype ResourceNotFoundException, message (example) 'Deployment not found'.
Emulator: 500 / AmbiguousService.
Why: rollback paths cannot treat an absent deployment as already cancelled.
Fix: same dispatch layer; then CancelDeployment's lookup.
Traces: greengrassv2-deployment-error-paths#task#4.

Cause 4 of 9 - DeleteDeployment on a nonexistent deployment.
AWS: ResourceNotFoundException modeled, httpError 404. Recorded response: 404, x-amzn-errortype ResourceNotFoundException, message (example) 'Deployment not found'.
Emulator: 500 / AmbiguousService.
Why: cleanup cannot be made idempotent.
Fix: same dispatch layer; then DeleteDeployment's lookup.
Traces: greengrassv2-deployment-error-paths#task#5.

Cause 5 of 9 - CreateDeployment happy path.
AWS: Smithy CreateDeployment success code 201 with output deploymentId (and iotJobId/iotJobArn when a job is created). Recorded response: 201.
Emulator: 500.
Why: no deployment can be created, so every deployment read, tag and cancel path is unreachable too.
Fix: same dispatch layer; then CreateDeployment's handler/state.
Traces: greengrassv2-deployment-tag-round-trip#setup#2 (also greengrassv2-list-variations#setup#2, #setup#3).

Cause 6 of 9 - ListDeployments.
AWS: answers 200 application/json with a deployments list whose entries carry targetArn, deploymentId, revisionId, creationTimestamp, deploymentStatus and isLatestForTarget. Recorded response: 200 with one such entry (deploymentStatus ACTIVE, isLatestForTarget true).
Emulator: 500 / AmbiguousService, deployments absent.
Why: a client cannot enumerate deployments or determine which revision is latest for a target - the only way to find the live deployment.
Fix: same dispatch layer; then ListDeployments' handler/serializer.
Traces: greengrassv2-list-variations#task#4 (also #task#5).

Cause 7 of 9 - TagResource on a resource ARN that does not exist.
AWS: Smithy GreengrassV2.TagResource, POST /tags/{resourceArn}, success 200, resourceArn @required matching ^arn:[^:]*:greengrass:[^:]*:(aws|[0-9]+):(components|deployments|coreDevices):, modeled errors ValidationException 400 / ResourceNotFoundException 404 / InternalServerException 500. Recorded response: 404 with x-amzn-errortype NotFoundException, content-type application/json, message (example) 'Resource was not found'.
Emulator: 500, x-amzn-errortype AmbiguousService.
Why: tagging a stale ARN surfaces as a server fault, so a client retries instead of reporting the missing resource. The status 404 and the error code NotFoundException on the wire are the requirement here (recorded behavior; note the recorded code differs from the model's ResourceNotFoundException).
Fix: same dispatch layer; then TagResource's resource lookup and its error serializer.
Traces: greengrassv2-resource-tagging-error-paths#task#1.

Cause 8 of 9 - UntagResource on a resource ARN that does not exist.
AWS: recorded behavior - 404 with x-amzn-errortype NotFoundException, content-type application/json, message (example) 'Resource was not found'.
Emulator: 500 / AmbiguousService.
Why: tag removal cannot be made idempotent against deleted resources.
Fix: same dispatch layer; then UntagResource's resource lookup.
Traces: greengrassv2-resource-tagging-error-paths#task#2.

Cause 9 of 9 - ListTagsForResource on a resource ARN that does not exist.
AWS: recorded behavior - 404 with x-amzn-errortype NotFoundException, content-type application/json, message (example) 'Resource was not found'.
Emulator: 500 / AmbiguousService.
Why: tag reads cannot distinguish an untagged-but-present resource from an absent one.
Fix: same dispatch layer; then ListTagsForResource's resource lookup.
Traces: greengrassv2-resource-tagging-error-paths#task#3.

## Evidence
Expected (AWS / recorded): CreateDeployment with a missing target -> 400 ValidationException; GetDeployment / CancelDeployment / DeleteDeployment on a missing deployment -> 404 ResourceNotFoundException; CreateDeployment happy path -> 201; ListDeployments -> 200 with a deployments list carrying deploymentId/revisionId/deploymentStatus/isLatestForTarget; TagResource / UntagResource / ListTagsForResource on an absent ARN -> 404 NotFoundException. All with content-type application/json.

Actual (emulator): all nine answer HTTP 500 with content-type application/x-amz-json-1.1 and, where an error type is present, x-amzn-errortype AmbiguousService and body {"__type":"AmbiguousService","message":"2 emulated services all claim requests for 'greengrass': greengrass, greengrass; the request is rejected rather than served by an arbitrary one"}. The 200/201 steps return none of their expected members (deployments absent; CreateDeployment returns no deploymentId). Replay: 47 mismatched steps, score 6.8%, state_reset_failures 0.

Model citation: smithy_query operation CreateDeployment/TagResource on services/service-com-amazonaws-greengrassv2-greengrassv2 -> CreateDeployment POST /greengrass/v2/deployments code 201 with ValidationException 400 / ResourceNotFoundException 404 / ConflictException 409; TagResource POST /tags/{resourceArn} code 200 with ResourceNotFoundException 404. service_info shows endpointPrefix/arnNamespace/sigv4Name 'greengrass' on BOTH the greengrass and greengrassv2 models, which is what the router refuses to disambiguate. AmbiguousService is not a modeled error of either service.

## Steps to Reproduce
1. Boot the emulator from this checkout with its default service set (greengrass and greengrassv2 both claim the 'greengrass' endpoint prefix).
2. Replay fixture greengrassv2-deployment-error-paths, step task#1 (CreateDeployment with a nonexistent target): expected 400 ValidationException, observed 500 AmbiguousService.
3. Replay the same fixture's task#3 / task#4 / task#5 (GetDeployment, CancelDeployment, DeleteDeployment, expected 404 ResourceNotFoundException).
4. Replay greengrassv2-deployment-tag-round-trip#setup#2 (CreateDeployment, expected 201) and greengrassv2-list-variations#task#4 (ListDeployments, expected 200 with deployments).
5. Replay greengrassv2-resource-tagging-error-paths#task#1 / #task#2 / #task#3 (TagResource, UntagResource, ListTagsForResource, expected 404 NotFoundException).
6. Observe every step returns 500 with content-type application/x-amz-json-1.1 and errortype AmbiguousService. All 9 causes must answer with their modeled status/code/shape before this issue is closed.
