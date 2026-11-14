Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: codebuild.delete-source-credentials-invalid-arn
Issue-Shape-Id: com.amazonaws.codebuild#CodeBuild_20161006
Issue-Key: codebuild.delete-source-credentials-invalid-arn|run#12|AssertionError|assert message == "Invalid ARN: region region is invalid", message
Issue-Commit: 431058ad591415bfb50dfcb201144b6c4e439813

# DeleteSourceCredentials rejects a malformed credential ARN with the wrong message: emulator reports the server type instead of AWS's invalid-region segment

> _Note: this issue was identified against the code base at commit `431058ad591415bfb50dfcb201144b6c4e439813`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DeleteSourceCredentials

## Description
ONE net-new root cause in the codebuild crate (services/service-com-amazonaws-codebuild-codebuild-20161006). The emulator returns the right HTTP status and the right error code for DeleteSourceCredentials with a malformed credential ARN, but the WRONG message text, and it blames a different ARN segment than AWS does — so a client that branches on or logs AWS's wording misreads which part of the ARN is wrong. An error message is part of the contract.

Cause 1 of 1: DeleteSourceCredentials with a malformed source-credential ARN names the resource's server type instead of the first invalid ARN segment (the region).
AWS: Smithy DeleteSourceCredentials(arn @required NonEmptyString, @length min 1) -> {arn}; modeled errors InvalidInputException and ResourceNotFoundException (both errorKind client, httpError 400). Recorded behavior: HTTP 400, error code InvalidInputException, message 'Invalid ARN: region <region-segment> is invalid'. The fixed text is 'Invalid ARN: region ' and ' is invalid'; the single variable part is the region segment taken verbatim from the ARN in the request (in the recording that segment is literally the word 'region', because the ARN under test is arn:aws:codebuild:region:account:token/does_not_exist — that word is a request value filling the template, not part of the fixed text). AWS validates the ARN segments in order and refuses on the FIRST invalid one, so with an unparseable region it never reaches the resource part at all. The check's positive control (ImportSourceCredentials, ListSourceCredentials, then DeleteSourceCredentials on the real ARN) succeeds against the emulator, and the credential listing is unchanged after the rejection, so the message is the sole divergence.
Emulator: HTTP 400 with the correct code InvalidInputException but message 'Invalid token ARN: server type is invalid' — different fixed text ('Invalid token ARN: ' / ' is invalid'), a different subject (the resource's server type rather than the region segment), and no interpolation of any value from the request, so every malformed ARN produces the same string regardless of which segment is actually wrong.
Why: a caller that validates or repairs credential ARNs is told the server type is invalid when the server type was never examined; it will retry with a different serverType/resource and keep failing, because the real defect is the region segment. Exact-match error handling and log alerting keyed on AWS's 'Invalid ARN: region ... is invalid' phrasing never fire, and the message carries no indication of which segment to fix.
Fix: the ARN validation in the DeleteSourceCredentials handler — it must parse the credential ARN segment by segment and emit AWS's region-segment message for the first invalid segment, interpolating the offending region value from the request, rather than falling through to a constant server-type message.
Traces: codebuild.delete-source-credentials-invalid-arn.

Related open issue that does NOT cover this check: 20261029T204615Z-codebuild-error-message-wording-diverges-from-aws-on.md describes the same wording family (DeleteWebhook, UpdateProject, ListBuildBatchesForProject, GetReportGroupTrend, ListBuilds, CreateWebhook) but carries no replay key for DeleteSourceCredentials and does not touch ARN-segment validation, so this must be replayed separately.

## Evidence
Replay of suite simparity-behavioral-aws-v1 against the emulator built from this checkout: 69/182 passed; codebuild 68/136. Failing check codebuild.delete-source-credentials-invalid-arn, verdict mismatch:

  File "check_codebuild_delete_source_credentials_invalid_arn.py", line 65, in run
    assert message == "Invalid ARN: region region is invalid", message
  AssertionError: Invalid token ARN: server type is invalid

Expected (recorded AWS) for DeleteSourceCredentials(arn='arn:aws:codebuild:region:account:token/does_not_exist'): HTTP 400, code InvalidInputException, message 'Invalid ARN: region region is invalid'.
Actual (emulator): HTTP 400, code InvalidInputException (the check's `assert code == "InvalidInputException"` passed), message 'Invalid token ARN: server type is invalid'.

The preceding steps all passed against the emulator, which isolates the divergence to the message: ImportSourceCredentials(token=..., serverType=GITHUB, authType=PERSONAL_ACCESS_TOKEN, shouldOverwrite=True) returned an arn, ListSourceCredentials listed it, DeleteSourceCredentials on that real arn succeeded and removed it, and the listing after the rejected malformed-ARN delete equals the listing before it.

Smithy model consulted: services/service-com-amazonaws-codebuild-codebuild-20161006/smithy-model/com-amazonaws-codebuild-codebuild-20161006.json — DeleteSourceCredentials(arn @required NonEmptyString @length min 1) -> {arn}, errors InvalidInputException / ResourceNotFoundException, errorKind client, httpError 400 (defaulted).

## Steps to Reproduce
1. git clone the repo and boot the emulator built from this checkout.
2. ImportSourceCredentials(token='<any token>', serverType='GITHUB', authType='PERSONAL_ACCESS_TOKEN', shouldOverwrite=True); confirm ListSourceCredentials returns the new arn and DeleteSourceCredentials on that arn succeeds (positive control — these already pass).
3. Call DeleteSourceCredentials(arn='arn:aws:codebuild:region:account:token/does_not_exist').
4. Expect HTTP 400, error code InvalidInputException, message 'Invalid ARN: region region is invalid' (fixed text 'Invalid ARN: region ' / ' is invalid' around the region segment copied from the request ARN).
5. Observe HTTP 400, code InvalidInputException, message 'Invalid token ARN: server type is invalid'.
6. Confirm ListSourceCredentials is unchanged by the rejected call (it already is), so the message text is the only divergence.
7. Vary the region segment of the ARN to any other invalid region string and confirm the emulator emits the same constant message instead of interpolating that segment.
