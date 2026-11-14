Issue-Source: trace-sweep-divergence
Issue-Trace: apigateway-api-key-value-length-bounds
Issue-Step: task#1
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: apigateway-api-key-value-length-bounds apigateway-iam-auth-get-method-verb-guessing apigateway-sstr-traj-rest-api-lambda-proxy-uri-forms
Issue-Shape-Id: com.amazonaws.apigateway#BackplaneControlService
Issue-Key: apigateway|CreateApiKey|4xx->4xx|$.message
Issue-Key: apigateway|GetMethod|4xx->2xx|$.message
Issue-Key: apigateway|GetMethod|4xx->2xx|$headers.x-amzn-errortype
Issue-Key: apigateway|GetMethod|4xx->2xx|status
Issue-Key: apigateway|PutIntegration|4xx->2xx|$.message
Issue-Key: apigateway|PutIntegration|4xx->2xx|$headers.x-amzn-errortype
Issue-Key: apigateway|PutIntegration|4xx->2xx|status
Issue-Key: apigateway|PutIntegration|2xx->4xx|$.cacheKeyParameters
Issue-Key: apigateway|PutIntegration|2xx->4xx|$.cacheNamespace
Issue-Key: apigateway|PutIntegration|2xx->4xx|$.httpMethod
Issue-Key: apigateway|PutIntegration|2xx->4xx|$.passthroughBehavior
Issue-Key: apigateway|PutIntegration|2xx->4xx|$.responseTransferMode
Issue-Key: apigateway|PutIntegration|2xx->4xx|$.timeoutInMillis
Issue-Key: apigateway|PutIntegration|2xx->4xx|$.type
Issue-Key: apigateway|PutIntegration|2xx->4xx|$.uri
Issue-Key: apigateway|PutIntegration|2xx->4xx|status
Issue-Key: apigateway|CreateDeployment|4xx->4xx|$.message
Issue-Commit: e5a3e8eb74dcb2b43ac4e38f0bb7bcf8e9a5cd29

# API Gateway REST validation refusals accept invalid input and reword AWS messages

> _Note: this issue was identified against the code base at commit `e5a3e8eb74dcb2b43ac4e38f0bb7bcf8e9a5cd29`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutIntegration

## Description
Five net-new root causes in the API Gateway REST control plane's request-validation layer: two refusals carry emulator-authored text instead of AWS's sentence, two requests AWS refuses are accepted, and one request AWS accepts is refused. All five are apigateway and are filed as one bundle; a fixer must close ALL FIVE before submitting.

Cause 1 of 5: CreateApiKey value-length refusal message.
AWS: CreateApiKey.value is a plain String in the model with no @length trait (Smithy: CreateApiKey input member `value` -> String; modeled error BadRequestException, httpError 400), so the bound is recorded behavior only. The recording shows AWS refusing a value shorter than the minimum with HTTP 400, x-amzn-errortype BadRequestException and the message `API Key value should be at least 20 characters`. That whole sentence is fixed text: it names the lower bound as a constant and interpolates nothing from the request.
Emulator: status 400 is right, but the body carries `value must be between 20 and 128 characters, got 19` - an emulator-authored generic range sentence that states both bounds and echoes the offending length.
Why: clients and test suites that match on AWS's refusal sentence (or that surface it to users) see an unknown string; nothing in the message identifies it as the API-key value rule.
Fix: the CreateApiKey handler's input validation in the apigateway service crate - the generic string-range validator must be replaced by the operation-specific sentence for the `value` member.
Traces: apigateway-api-key-value-length-bounds#task#1 (also task#2).

Cause 2 of 5: GetMethod answers 200 for a method that does not exist.
AWS: GetMethod is modeled with NotFoundException (Smithy: GetMethod errors NotFoundException httpError 404, TooManyRequestsException, UnauthorizedException; http GET /restapis/{restApiId}/resources/{resourceId}/methods/{httpMethod}). For an httpMethod that has no Method resource on the given resource, the recording shows HTTP 404 with message `Invalid Method identifier specified` - fixed text, no interpolated values.
Emulator: returns HTTP 200 with a Method representation, so a method that was never created appears to exist.
Why: a client probing which verbs are configured on a resource (verb discovery) concludes every verb is configured; code that relies on 404 to branch never takes the not-found path, and a subsequent PutMethod/DeleteMethod sequence is built on a phantom.
Fix: the GetMethod handler's lookup in the apigateway service crate - a missing method entry must raise NotFoundException rather than synthesising a default Method.
Traces: apigateway-iam-auth-get-method-verb-guessing#task#5.

Cause 3 of 5: PutIntegration accepts an AWS integration URI whose ARN has no path or action.
AWS: PutIntegration is modeled with BadRequestException (Smithy: PutIntegration errors BadRequestException httpError 400; input members `type` (IntegrationType: HTTP, AWS, MOCK, HTTP_PROXY, AWS_PROXY) and `uri`). The recording shows AWS refusing an AWS-service integration URI whose apigateway ARN ends without a `path/...` or `action/...` segment with HTTP 400 and the message `AWS ARN for integration must contain path or action` - entirely fixed text.
Emulator: answers 201 and stores the integration, performing no structural check on the ARN form of `uri` for AWS/AWS_PROXY types.
Why: a malformed Lambda-proxy or AWS-service integration is persisted and only fails much later (at invoke time, as a configuration error) instead of at configuration time, so the client never learns which field was wrong.
Fix: PutIntegration's input validation in the apigateway service crate - add the AWS-ARN URI form check for `type` in {AWS, AWS_PROXY}.
Traces: apigateway-sstr-traj-rest-api-lambda-proxy-uri-forms#task#4.

Cause 4 of 5: PutIntegration refuses a well-formed AWS_PROXY Lambda invocation URI.
AWS: the canonical Lambda-proxy URI form is `arn:aws:apigateway:<region>:lambda:path/2015-03-31/functions/<function-arn>/invocations`; AWS accepts it and answers HTTP 201 with the Integration representation (Smithy: PutIntegration http code 201, output shape Integration with members type, httpMethod, uri, passthroughBehavior, cacheNamespace, cacheKeyParameters, timeoutInMillis, responseTransferMode). The recording shows the created integration carrying type AWS_PROXY, httpMethod GET, passthroughBehavior WHEN_NO_MATCH, timeoutInMillis 29000, responseTransferMode BUFFERED, an empty cacheKeyParameters list and a cacheNamespace derived from the resource id (the concrete function ARN, region, account and cacheNamespace value in the recording are environment-specific and are not part of the requirement).
Emulator: answers HTTP 400 and returns no Integration at all, so every modeled output member is absent.
Why: the standard Lambda proxy wiring - the single most common REST integration - cannot be configured at all; combined with Cause 3 the validator is inverted, letting malformed ARNs through while rejecting the documented form.
Fix: PutIntegration's URI parsing/validation in the apigateway service crate - the `lambda:path/...` ARN form must parse and be accepted.
Traces: apigateway-sstr-traj-rest-api-lambda-proxy-uri-forms#task#6.

Cause 5 of 5: CreateDeployment stageName refusal message.
AWS: CreateDeployment.stageName is a plain String in the model with no @pattern (Smithy: CreateDeployment input member `stageName` -> String; modeled error BadRequestException httpError 400), so the character rule is recorded behavior only. For a stageName containing a character outside the permitted alphabet, the recording shows HTTP 400 with the message `Stage name only allows a-zA-Z0-9_` - fixed text that states the permitted alphabet and interpolates nothing from the request.
Emulator: status 400 is right, but the message is `stageName may contain only alphanumeric characters, hyphens and underscores, got "$default"` - emulator-authored wording that additionally allows hyphens (which AWS's stated alphabet does not) and echoes the rejected value.
Why: the message misstates the rule to the caller (a hyphenated stage name would be read as legal) and does not match the sentence AWS clients match on.
Fix: CreateDeployment's stageName validation in the apigateway service crate - use AWS's alphabet and its exact sentence.
Traces: apigateway-sstr-traj-rest-api-lambda-proxy-uri-forms#task#9.

Related open issues that do NOT cover these steps (the replay report attributes no owner to any of the five): 20261031T012129Z-apigateway-api-gateway-400-refusals-replace-aws-validation-messages.md, 20261029T193654Z-apigateway-api-gateway-control-plane-is-over-permissive-6.md, 20261029T200808Z-apigateway-api-gateway-lookup-refusals-name-the-wrong-resource.md.

## Evidence
Cause 1 (CreateApiKey, apigateway-api-key-value-length-bounds#task#1): expected 400 + $.message 'API Key value should be at least 20 characters'; actual 400 + $.message 'value must be between 20 and 128 characters, got 19'.
Cause 2 (GetMethod, apigateway-iam-auth-get-method-verb-guessing#task#5): expected status 404, $.message 'Invalid Method identifier specified', x-amzn-errortype set; actual status 200 with a Method body and no message/errortype header.
Cause 3 (PutIntegration, apigateway-sstr-traj-rest-api-lambda-proxy-uri-forms#task#4): expected status 400, $.message 'AWS ARN for integration must contain path or action'; actual status 201 with no message.
Cause 4 (PutIntegration, apigateway-sstr-traj-rest-api-lambda-proxy-uri-forms#task#6): expected status 201 with $.type=AWS_PROXY, $.httpMethod=GET, $.passthroughBehavior=WHEN_NO_MATCH, $.timeoutInMillis=29000, $.responseTransferMode=BUFFERED, $.cacheKeyParameters=[], $.cacheNamespace and $.uri present; actual status 400 and all of those members absent.
Cause 5 (CreateDeployment, apigateway-sstr-traj-rest-api-lambda-proxy-uri-forms#task#9): expected 400 + $.message 'Stage name only allows a-zA-Z0-9_'; actual 400 + $.message 'stageName may contain only alphanumeric characters, hyphens and underscores, got "$default"'.

## Steps to Reproduce
1. Boot the emulator from this checkout and replay the wire corpus (trace set simparity-wire-aws-v1).
2. Cause 1: replay fixture apigateway-api-key-value-length-bounds, step task#1 - CreateApiKey with a 19-character `value`; compare $.message.
3. Cause 2: replay fixture apigateway-iam-auth-get-method-verb-guessing, step task#5 - GetMethod for an httpMethod never put on the resource; compare status and $.message.
4. Cause 3: replay fixture apigateway-sstr-traj-rest-api-lambda-proxy-uri-forms, step task#4 - PutIntegration with type AWS_PROXY and an apigateway ARN uri lacking a path/action segment; compare status and $.message.
5. Cause 4: same fixture, step task#6 - PutIntegration with the canonical `arn:aws:apigateway:<region>:lambda:path/2015-03-31/functions/<fn-arn>/invocations` uri; expect 201 and the Integration body.
6. Cause 5: same fixture, step task#9 - CreateDeployment with stageName '$default'; compare $.message.
