Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: controltower.list-tags-for-resource-malformed-arn-validation
Issue-Shape-Id: com.amazonaws.controltower#AWSControlTowerApis
Issue-Key: controltower.list-tags-for-resource-malformed-arn-validation|run#8|AssertionError|assert wellformed_code == "NotFoundException", (
Issue-Commit: 0e0bbca79734649b97ab70170a0bc1f3e6da2e9e

# ListTagsForResource answers a well-formed but nonexistent resourceArn with ResourceNotFoundException instead of AWS's NotFoundException "Resource was not found"

> _Note: this issue was identified against the code base at commit `0e0bbca79734649b97ab70170a0bc1f3e6da2e9e`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListTagsForResource

## Description
Cause 1 of 1: the Control Tower tag plane's not-found rejection carries the wrong error code (and wrong message) for a resourceArn that is syntactically valid but names no existing resource.

AWS: Smithy (com.amazonaws.controltower#AWSControlTowerApis, ListTagsForResource): input resourceArn is @required @httpLabel targeting Arn with @length 20..2048 and @pattern ^arn:aws[0-9a-zA-Z_\-:\/]+$; http GET /tags/{resourceArn}, success 200; declared errors InternalServerException (500), ResourceNotFoundException (404), ValidationException (400). The declared error set is NOT what the service puts on the wire here — recorded behavior only: for a well-formed enabled-control ARN that resolves to nothing (arn:aws:controltower:<region>:<account>:enabledcontrol/<32-char id>), AWS refuses with error code NotFoundException and message "Resource was not found". The message is entirely fixed text — no request- or state-derived values are interpolated into it (the offending ARN is not echoed). This is also distinct from the malformed-ARN refusal on the same operation, which is BadRequestException "Invalid input resource arn: bad syntax" (fixed text as well); the two rejection classes must stay distinguishable by code.

Emulator: for the same well-formed, nonexistent ARN the emulator returns error code ResourceNotFoundException — the code the Smithy model declares rather than the code the service emits. The replayed check asserts the code first and stops there, so the emulator's accompanying message was not captured by this check; the message must also be exactly "Resource was not found", so any emulator-authored prose (for example the "no resource matches resourceArn <arn>" wording this handler has used on its miss path) is part of the same defect. The malformed-ARN half of the same operation is now answered correctly (BadRequestException with AWS's wording), so only the not-found branch diverges.

Why: a client that branches on the wire error code never matches NotFoundException and falls through to a generic/unexpected-error path, or retries, instead of treating the resource as absent. Code-based error handling (botocore error-code dispatch, catch-by-code in SDK wrappers, retry classifiers) keys on the string the service sends, not on the modeled shape name, so an emulator that emits the modeled name is unusable for exercising the absent-resource path. Message-matching clients also see emulator prose rather than AWS's fixed sentence.

Fix: the not-found branch of the ListTagsForResource handler's resourceArn resolution — the error it raises after ARN validation passes must serialize the wire code NotFoundException with message "Resource was not found", instead of the model-declared ResourceNotFoundException. The same wire code should be shared with the sibling tag-plane operations (TagResource/UntagResource) so all three agree on the absent-resource rejection. Note the open issue 20261029T183207Z-controltower-control-tower-tag-plane-arn-validation-is-skipped.md: it covers the *malformed / cross-account ARN* front-door refusal (BadRequestException, 400) on the tag plane and does not cover this one — it in fact assumes the model's ResourceNotFoundException is the correct not-found code, which this check shows it is not.

Traces: behavioral check controltower.list-tags-for-resource-malformed-arn-validation.

## Evidence
Behavioral suite simparity-behavioral-aws-v1, check controltower.list-tags-for-resource-malformed-arn-validation (verdict: mismatch).

AssertionError: expected NotFoundException for a well-formed but nonexistent resourceArn, got ResourceNotFoundException
  (check_controltower_list_tags_for_resource_malformed_arn_validation.py line 38)

Expected (AWS): ListTagsForResource on arn:aws:controltower:<region>:<account>:enabledcontrol/AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA -> rejection with Error.Code == "NotFoundException" and Error.Message == "Resource was not found" (message is wholly fixed text).
Actual (emulator): rejection with Error.Code == "ResourceNotFoundException".

The earlier assertions in the same check passed, so the malformed-input branch is correct: resourceArn "totally-not-an-arn-but-long-enough" -> BadRequestException "Invalid input resource arn: bad syntax". The check additionally requires that the malformed-input code and the not-found code differ; with the emulator's ResourceNotFoundException they do differ, but the not-found code itself is wrong.

## Steps to Reproduce
1. Boot the emulator from this checkout and replay the behavioral suite check controltower.list-tags-for-resource-malformed-arn-validation.
2. Equivalently, by hand: call sts:GetCallerIdentity to get the account id, then call controltower:ListTagsForResource with resourceArn = "arn:aws:controltower:<region>:<account>:enabledcontrol/AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA" (a well-formed ARN for an enabled control that was never created).
3. Observe the rejection's Error.Code is "ResourceNotFoundException"; AWS returns Error.Code "NotFoundException" with Error.Message "Resource was not found".
4. For contrast, call the same operation with resourceArn = "totally-not-an-arn-but-long-enough"; this correctly yields BadRequestException "Invalid input resource arn: bad syntax".
