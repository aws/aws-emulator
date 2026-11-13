Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: lambda.blueprint-tag-tag-resource-disallowed-key-character-rejected lambda.unzipped-package-over-250mb-rejected lambda.layer-function-update-rejected-for-a-layer-in-another-region lambda.layer-function-update-rejected-for-two-versions-of-one-layer lambda.layer-is-invisible-from-another-region lambda.log-lambda-report-log-level-with-text-format-rejected lambda.delete-function-version-referenced-by-alias-conflict lambda.concurrent-function-update-conflict lambda.tags-lifecycle lambda.layer-publish-rejected-for-more-than-fifteen-compatible-runtimes
Issue-Shape-Id: com.amazonaws.lambda#AWSGirApiService
Issue-Key: lambda.blueprint-tag-tag-resource-disallowed-key-character-rejected|_error#4|AssertionError|raise AssertionError("the call succeeded; real AWS rejects it, so there is no error code")
Issue-Key: lambda.unzipped-package-over-250mb-rejected|_error#6|AssertionError|raise AssertionError(f"expected the call to be rejected, but it SUCCEEDED and returned {result!r}")
Issue-Key: lambda.layer-function-update-rejected-for-a-layer-in-another-region|_error#10|AssertionError|raise AssertionError("the call succeeded; real AWS rejects it, so there is no error code")
Issue-Key: lambda.layer-function-update-rejected-for-two-versions-of-one-layer|_error#8|AssertionError|raise AssertionError("the call succeeded; real AWS rejects it, so there is no error code")
Issue-Key: lambda.layer-is-invisible-from-another-region|_error#6|AssertionError|raise AssertionError("the call succeeded; real AWS rejects it, so there is no error code")
Issue-Key: lambda.log-lambda-report-log-level-with-text-format-rejected|_error#6|AssertionError|raise AssertionError(f"expected the call to be refused, but it succeeded with {result!r}")
Issue-Key: lambda.delete-function-version-referenced-by-alias-conflict|_client_error#17|AssertionError|raise AssertionError(f"expected the call to be rejected, but it succeeded: {result!r}")
Issue-Key: lambda.concurrent-function-update-conflict|_client_error#8|AssertionError|raise AssertionError(f"{on_success}; the call SUCCEEDED and returned {result!r}")
Issue-Key: lambda.tags-lifecycle|_error#12|AssertionError|raise AssertionError(f"expected the call to be rejected, but it SUCCEEDED and returned {result!r}")
Issue-Key: lambda.layer-publish-rejected-for-more-than-fifteen-compatible-runtimes|run#11|AssertionError|assert msg == expected, f"expected the exact ValidationException message {expected!r}, got {msg!r}"
Issue-Commit: f04105fb2faf5f6bd82b29a86b394ae1fc089f7b

# lambda: ten control-plane writes are accepted that AWS refuses (validation and conflict checks missing)

> _Note: this issue was identified against the code base at commit `f04105fb2faf5f6bd82b29a86b394ae1fc089f7b`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateFunction

## Description
Ten net-new root causes, all of the same family: the emulator accepts a write that real Lambda refuses (or refuses it without the request-derived diagnosis), so a client's invalid request is stored instead of rejected.

Cause 1 of 10 — TagResource accepts a tag key containing a disallowed character.
AWS: Smithy (lambda) TagResource takes a Tags map and models InvalidParameterValueException (400); AWS rejects keys outside the allowed tag-key character set. Emulator: the call succeeds. Why: an unusable tag is stored and later tag queries/policies keyed on it behave unpredictably. Fix: TagResource handler validation in the lambda service. Traces: lambda.blueprint-tag-tag-resource-disallowed-key-character-rejected.

Cause 2 of 10 — CreateFunction accepts a deployment package whose unzipped size exceeds the 250 MB limit.
AWS: the request is refused InvalidParameterValueException (400) naming the unzipped-size limit. Emulator: CreateFunction answers 201 with State=Pending for the oversize package. Why: a function that can never run is created, and the client learns nothing at the only point it could fix the package. Fix: CreateFunction package validation in the lambda service. Traces: lambda.unzipped-package-over-250mb-rejected.

Cause 3 of 10 — UpdateFunctionConfiguration accepts a layer ARN from another region.
AWS: layers are regional; a cross-region layer ARN is refused InvalidParameterValueException (400). Emulator: the call succeeds. Why: the function references a layer that cannot exist at runtime. Fix: layer-ARN region validation in the UpdateFunctionConfiguration/CreateFunction handler. Traces: lambda.layer-function-update-rejected-for-a-layer-in-another-region.

Cause 4 of 10 — UpdateFunctionConfiguration accepts two versions of the same layer in Layers.
AWS: the request is refused InvalidParameterValueException (400) because a function may reference one version per layer. Emulator: the call succeeds. Why: the resulting /opt content is undefined. Fix: duplicate-layer detection in the same handler. Traces: lambda.layer-function-update-rejected-for-two-versions-of-one-layer.

Cause 5 of 10 — a layer published in one region is readable from another.
AWS: GetLayerVersion/GetLayerVersionByArn in a different region refuses ResourceNotFoundException (404) — layers are not global. Emulator: the read succeeds. Why: region isolation is not enforced, so a client cannot detect that it is pointing at the wrong region. Fix: region scoping of the layer store/reads in the lambda service. Traces: lambda.layer-is-invisible-from-another-region.

Cause 6 of 10 — CreateFunction accepts LoggingConfig with ApplicationLogLevel while LogFormat=Text.
AWS: ApplicationLogLevel/SystemLogLevel are only valid with LogFormat=JSON; the combination is refused InvalidParameterValueException (400). Emulator: CreateFunction answers 200 and echoes LoggingConfig {'LogFormat': 'Text', 'ApplicationLogLevel': 'WARN'}. Why: the client believes level filtering is active when it cannot be. Fix: LoggingConfig cross-member validation in the CreateFunction/UpdateFunctionConfiguration handler. Traces: lambda.log-lambda-report-log-level-with-text-format-rejected.

Cause 7 of 10 — DeleteFunction removes a version that an alias still references.
AWS: Smithy (lambda) DeleteFunction models ResourceConflictException (409); deleting a version an alias points at is refused with it. Emulator: the call succeeds with HTTP 204. Why: the alias is left dangling and every invoke through it fails later, instead of the delete being refused now. Fix: alias-reference check in the DeleteFunction handler. Traces: lambda.delete-function-version-referenced-by-alias-conflict.

Cause 8 of 10 — UpdateFunctionConfiguration is accepted while an UpdateFunctionCode is still in progress.
AWS: while LastUpdateStatus=InProgress a further configuration update is refused ResourceConflictException (409). Emulator: the call returns 200 with LastUpdateStatus='InProgress'. Why: two overlapping updates race and the client cannot serialise them as AWS tells it to. Fix: in-progress guard in the lambda update handlers/state. Traces: lambda.concurrent-function-update-conflict.

Cause 9 of 10 — a tag write AWS refuses succeeds in the tag lifecycle.
AWS: the refused call returns a client error (400); recorded behavior only for which member is at fault. Emulator: it succeeds with HTTP 204. Why: the client cannot rely on tag validation at all. Fix: TagResource/UntagResource validation in the lambda service. Traces: lambda.tags-lifecycle.

Cause 10 of 10 — the >15 CompatibleRuntimes refusal does not echo the rejected value.
AWS: Smithy (lambda) PublishLayerVersion.CompatibleRuntimes is @length(max:15); the validation error names the member and the submitted value, e.g. "Value '[nodejs18.x, ...]' at 'compatibleRuntimes' failed to satisfy constraint: Member must have length less than or equal to 15" — the message is built from the request, so echoing the value is the rule. Emulator: the same constraint is reported as "Value '16 items' at 'compatibleRuntimes' ...". Why: a client parsing or logging the offending value cannot see which runtimes it sent. Fix: the validation-error renderer for list members in the lambda service. Traces: lambda.layer-publish-rejected-for-more-than-fifteen-compatible-runtimes.

Also note: the open issue 20261023T114649Z-lambda-function-urls-unrouted-configuration-blocks-dropped-from.md covers neighbouring CreateFunction validation gaps but lists none of these ten checks.

## Evidence
Causes 1, 3, 4, 5: 'AssertionError: the call succeeded; real AWS rejects it, so there is no error code'.
Cause 2: 'AssertionError: expected the call to be rejected, but it SUCCEEDED and returned {... HTTPStatusCode 201 ... CodeSize 262748 ...}'.
Cause 6: 'AssertionError: expected the call to be refused, but it succeeded with {... LoggingConfig {LogFormat: Text, ApplicationLogLevel: WARN}}'.
Cause 7: 'AssertionError: expected the call to be rejected, but it succeeded: {... HTTPStatusCode 204 ...}'.
Cause 8: 'AssertionError: a configuration update issued while UpdateFunctionCode was still in progress was accepted; the call SUCCEEDED ... LastUpdateStatus: InProgress'.
Cause 9: 'AssertionError: expected the call to be rejected, but it SUCCEEDED and returned {... HTTPStatusCode 204 ...}'.
Cause 10: expected "Value '[nodejs18.x, nodejs20.x, ...]' at 'compatibleRuntimes' failed to satisfy constraint: Member must have length less than or equal to 15", got "Value '16 items' at 'compatibleRuntimes' ...".

## Steps to Reproduce
1. TagResource with a key containing a disallowed character.
2. CreateFunction with a zip whose unzipped size exceeds 250 MB.
3. UpdateFunctionConfiguration with a layer ARN from another region, then with two versions of one layer.
4. GetLayerVersion from a different region than the one the layer was published in.
5. CreateFunction with LoggingConfig {LogFormat: Text, ApplicationLogLevel: WARN}.
6. Publish a version, point an alias at it, DeleteFunction with that Qualifier.
7. UpdateFunctionCode with a large package and immediately UpdateFunctionConfiguration.
8. PublishLayerVersion with 16 CompatibleRuntimes and read the error message.
