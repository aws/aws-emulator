Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: lambda.update-function-code-invalid-signature-rejected
Issue-Shape-Id: com.amazonaws.lambda#AWSGirApiService
Issue-Key: lambda.update-function-code-invalid-signature-rejected|_reject#24|AssertionError|raise AssertionError(
Issue-Commit: 1945ee51cfaab7e95ed50a7b8bc82cc22a5254e2

# lambda: UpdateFunctionCode ignores an attached ENFORCE CodeSigningConfig — a package signed by a profile outside AllowedPublishers is deployed with 200 instead of being refused 400 CodeVerificationFailedException

> _Note: this issue was identified against the code base at commit `1945ee51cfaab7e95ed50a7b8bc82cc22a5254e2`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
UpdateFunctionCode

## Description
One net-new root cause, grounded in 1 failing check.

Cause 1 of 1: with a CodeSigningConfig whose CodeSigningPolicies.UntrustedArtifactOnDeployment is 'Enforce' attached to the function, UpdateFunctionCode accepts a ZipFile signed by a signing profile that is NOT in the config's AllowedPublishers.SigningProfileVersionArns, and stores it. Failing check: lambda.update-function-code-invalid-signature-rejected.

AWS: Smithy (com-amazonaws-lambda-awsgirapiservice) binds UpdateFunctionCode to PUT /2015-03-31/functions/{FunctionName}/code with success code 200 and models CodeVerificationFailedException among its errors — errorKind client, httpError 400 — documented as "The code signature failed one or more of the validation checks for signature mismatch or expiry, and the code signing policy is set to ENFORCE. Lambda blocks the deployment." (the sibling InvalidCodeSignatureException, also 400, is what an entirely unsigned package raises, which is why the check signs with a mismatched profile rather than leaving the artifact unsigned). The check pins the refusal exactly: error code `CodeVerificationFailedException` and the message "Lambda cannot deploy the function. The function or layer might be signed using a signature that the client is not configured to accept. Check the provided signature for <function ARN>." — every word of that is fixed text except the trailing function ARN, which is the ARN of the function named in the request. It further pins that the refusal is clean: GetFunctionConfiguration must still report the previous CodeSha256, and an Invoke must still run the previously deployed code; and that the refusal is attributable to the signer alone, since the identical payload re-signed by the allowed profile then deploys (LastUpdateStatus 'Successful', CodeSha256 equal to the deployed bytes' own hash) and runs.

Emulator: the call returns HTTP 200 with a full FunctionConfiguration and the new CodeSha256, i.e. the mismatched-signer package is deployed. The check reports "expected the call to be refused for a signature mismatch, but it SUCCEEDED and returned {... 'HTTPStatusCode': 200 ... 'CodeSha256': ..., 'LastUpdateStatus': 'InProgress' ...}" (the request id, function name/ARN, hash and timestamps in that payload are per-run values; the defect is the 200 itself). The positive control earlier in the same check — a correctly signed update — succeeds, and PutFunctionCodeSigningConfig/GetFunctionCodeSigningConfig confirm the Enforce config is attached, so the configuration is stored and simply not consulted at deploy time.

Why: a client relying on AWS's behavior loses the code-signing control entirely on the update path. Code signing exists so that only artifacts signed by an approved publisher can reach a function; here any signer is accepted, so a supply-chain guard that blocks a deployment in AWS silently ships it in the emulator, and a caller never sees the CodeVerificationFailedException it is written to handle. Because the rejected package is also stored, the function ends up running code AWS would never have deployed.

Fix: the UpdateFunctionCode handler's validation stage (the operation's handler under the Lambda service crate, services/service-com-amazonaws-lambda-awsgirapiservice/src/ops/update_function_code.rs) — before the new package is written to function state it must resolve the function's attached CodeSigningConfig, verify the artifact's signing-profile version ARN against AllowedPublishers.SigningProfileVersionArns, and when UntrustedArtifactOnDeployment is 'Enforce' refuse with CodeVerificationFailedException (400) and that exact message, leaving CodeSha256 and the running code untouched.

Traces: the failing check id above. Open issues about code signing that do NOT carry this check's replay key and therefore do not cover it: 20261026T070343Z-lambda-code-signing-is-not-enforced-updatefunctioncode-and.md, 20261029T233239Z-lambda-code-signing-is-never-enforced-and-code.md, 20261103T165236Z-lambda-createfunction-skips-package-and-code-signing-validation.md and 20261107T091747Z-lambda-code-signing-not-enforced-no-in-flight.md.

## Evidence
Expected (AWS): UpdateFunctionCode with a package signed by a profile absent from the attached Enforce CodeSigningConfig is refused HTTP 400 with error code `CodeVerificationFailedException` and message "Lambda cannot deploy the function. The function or layer might be signed using a signature that the client is not configured to accept. Check the provided signature for <function ARN>." Afterwards GetFunctionConfiguration still reports the previous CodeSha256 and Invoke still returns the previous code's result; re-signing the same bytes with the allowed profile deploys successfully.

Actual (emulator): HTTP 200. AssertionError: expected the call to be refused for a signature mismatch, but it SUCCEEDED and returned {'ResponseMetadata': {'HTTPStatusCode': 200, ...}, 'FunctionName': 'sim2real-behav-fn-...', 'CodeSha256': 'QKVC/kMVwhbkOs7yxjkd43p4YNAxXRtd1OoyXYA3XWA=', 'LastUpdateStatus': 'InProgress', 'LastUpdateStatusReasonCode': 'Creating', ...} — the names, hash, request id and timestamps are that run's values; the divergence is the 200 and the stored new code where a 400 CodeVerificationFailedException was required. The in-check positive control (same update, correctly signed) succeeds, and GetFunctionCodeSigningConfig confirms the Enforce config is attached.

## Steps to Reproduce
1. Create two signer signing profiles, A (allowed) and B (mismatched).
2. lambda create_code_signing_config(AllowedPublishers={'SigningProfileVersionArns': [<A version ARN>]}, CodeSigningPolicies={'UntrustedArtifactOnDeployment': 'Enforce'}).
3. Create a python3.12 function and lambda put_function_code_signing_config(CodeSigningConfigArn=<csc>, FunctionName=<fn>); confirm get_function_code_signing_config returns that ARN.
4. Positive control: sign a zip with profile A and lambda update_function_code(FunctionName=<fn>, ZipFile=<signed>) — succeeds, LastUpdateStatus 'Successful'. Note the resulting CodeSha256 and invoke result.
5. Sign a different zip with profile B and call lambda update_function_code(FunctionName=<fn>, ZipFile=<B-signed>).
6. AWS: 400 CodeVerificationFailedException with the message above, CodeSha256 unchanged, Invoke still runs the step-4 code. Emulator: 200 and the new CodeSha256 is stored.
7. Re-replay lambda.update-function-code-invalid-signature-rejected to confirm.
