Issue-Source: trace-sweep-divergence
Issue-Trace: imagebuilder-image-recipe-lifecycle-with-policy
Issue-Step: teardown#8
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: imagebuilder-image-recipe-lifecycle-with-policy imagebuilder-list-image-packages-nonexistent-image-returns-empty
Issue-Shape-Id: com.amazonaws.imagebuilder#imagebuilder
Issue-Key: imagebuilder|GetImageRecipe|4xx->4xx|$.message
Issue-Key: imagebuilder|GetImageRecipe|4xx->4xx|$.requestId
Issue-Key: imagebuilder|GetImageRecipe|4xx->4xx|status
Issue-Key: imagebuilder|ListImagePackages|4xx->4xx|$.message
Issue-Key: imagebuilder|ListImagePackages|4xx->4xx|$.requestId
Issue-Key: imagebuilder|ListImagePackages|4xx->4xx|$headers.x-amzn-errortype
Issue-Commit: 6fdfa0adfee5efefb12453212234969d8b9de734

# Image Builder error responses use the wrong status, error type and message and omit requestId

> _Note: this issue was identified against the code base at commit `6fdfa0adfee5efefb12453212234969d8b9de734`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetImageRecipe

## Description
TWO causes, both in the imagebuilder emulator's error mapping: a failed request is answered with a generic 400 ValidationException-style body instead of the Image Builder error AWS models for that condition, and the modeled requestId member is absent from error bodies. Both must be closed before this issue is submitted.

Cause 1 of 2: reading a deleted/absent image recipe returns 400 with a homegrown message instead of 404 ResourceNotFoundException.
AWS: Smithy: ResourceNotFoundException is a client error with httpError 404, documented 'At least one of the resources referenced by your request does not exist.', with a single message member (the operation's own modeled error list does not include it, so the 404 for GetImageRecipe on a missing recipe is recorded behavior only). The recording shows status 404 and message "The following required resource 'ImageRecipe' cannot be found: '<imageRecipeArn>'." - fixed text is "The following required resource '", "' cannot be found: '" and the trailing "'."; the variable parts are the resource type token (ImageRecipe) and the ARN taken verbatim from the request's imageRecipeArn.
Emulator: status 400 with message 'The image recipe <arn> does not exist.' - a message AWS never emits - and no $.requestId member (the recording carries one; its value is per-run and is not a requirement, only its presence).
Why: every SDK maps this condition by status/error type, so a client that catches ResourceNotFoundException (or treats 404 as 'gone' when polling a delete to completion) instead sees a 400 client error and aborts the workflow; error-message assertions and user-facing text are also wrong.
Fix: GetImageRecipe's handler lookup path must raise the not-found error (404, ResourceNotFoundException) with AWS's message template, and the imagebuilder error serializer must include requestId.
Traces: imagebuilder-image-recipe-lifecycle-with-policy#teardown#8 (GetImageRecipe after delete, expected 404, got 400).

Cause 2 of 2: ListImagePackages rejects a malformed imageBuildVersionArn with a generic ValidationException instead of Image Builder's invalid-parameter error.
AWS: Smithy: ListImagePackagesRequest.imageBuildVersionArn is required with pattern ^arn:aws[^:]*:imagebuilder:[^:]+:(?:[0-9]{12}|aws(?:-[a-z-]+)?):image/[a-z0-9-_]+/[0-9]+\.[0-9]+\.[0-9]+/[0-9]+$; InvalidParameterValueException is a client error with httpError 400 ('The value that you provided for the specified parameter is invalid.'). The recorded 400 body is "The value supplied for parameter 'imageBuildVersionArn' is not valid. imageBuildVersionArn must match pattern <pattern>" - fixed text is "The value supplied for parameter '", "' is not valid. ", " must match pattern "; the variable parts are the member name (imageBuildVersionArn, twice) and the service-side pattern for that member. The error-type header is the Image Builder error type for an invalid parameter value, not a generic ValidationException.
Emulator: 400 with the generic smithy constraint message "1 validation error detected: Value '<arn>' at 'imageBuildVersionArn' failed to satisfy constraint: Member must satisfy regular expression pattern: <pattern>", header x-amzn-errortype: ValidationException (diverging from the recorded error type), and no $.requestId member.
Why: clients that branch on the x-amzn-errortype / modeled error shape never see the Image Builder error, so retry and user-messaging logic keyed on it is dead, and the message text does not match what AWS returns for a bad ARN.
Fix: ListImagePackages' input validation in the imagebuilder handler must emit the Image Builder invalid-parameter error (400, InvalidParameterValueException shape and error-type header) with AWS's message template instead of deferring to the generic constraint validator, and the error serializer must include requestId.
Traces: imagebuilder-list-image-packages-nonexistent-image-returns-empty#task#2 (ListImagePackages with a malformed image ARN, 400->400 with wrong body/header).

Note: the open imagebuilder issue about CreateImagePipeline's imageTestsConfiguration.timeoutMinutes ValidationException is a different operation and does not cover either cause here.

## Evidence
1) GetImageRecipe (imagebuilder-image-recipe-lifecycle-with-policy#teardown#8): status expected 404, got 400; $.message expected "The following required resource 'ImageRecipe' cannot be found: '<imageRecipeArn>'." got 'The image recipe <imageRecipeArn> does not exist.'; $.requestId expected present, got None.
2) ListImagePackages (imagebuilder-list-image-packages-nonexistent-image-returns-empty#task#2): status 400 as expected, but $.message expected "The value supplied for parameter 'imageBuildVersionArn' is not valid. imageBuildVersionArn must match pattern ^arn:aws(?:-[a-z]+)*:imagebuilder:[a-z]{2,}(?:-[a-z]+)+-[0-9]+:(?:[0-9]{12}|aws(?:-[a-z-]+)?):image/[a-z0-9-_]+/[0-9]+\.[0-9]+\.[0-9]+/[0-9]+$" and got "1 validation error detected: Value '<arn>' at 'imageBuildVersionArn' failed to satisfy constraint: Member must satisfy regular expression pattern: ^arn:aws[^:]*:imagebuilder:[^:]+:(?:[0-9]{12}|aws(?:-[a-z-]+)?):image/[a-z0-9-_]+/[0-9]+\.[0-9]+\.[0-9]+/[0-9]+$"; $headers.x-amzn-errortype diverged from the recorded error type, got 'ValidationException'; $.requestId expected present, got None.
The recorded requestId values are per-run and are not themselves requirements - the defect is that the member is absent from error bodies altogether.

## Steps to Reproduce
1. Replay fixture imagebuilder-image-recipe-lifecycle-with-policy through teardown: create an image recipe, delete it, then call GetImageRecipe on the deleted recipe ARN (step teardown#8) and compare status (expect 404), $.message and $.requestId.
2. Replay fixture imagebuilder-list-image-packages-nonexistent-image-returns-empty step task#2: call ListImagePackages with an imageBuildVersionArn that lacks the trailing build-version segment, and compare $.message, the x-amzn-errortype header and $.requestId.
