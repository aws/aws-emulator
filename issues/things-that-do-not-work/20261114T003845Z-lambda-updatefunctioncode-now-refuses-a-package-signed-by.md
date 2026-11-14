Issue-Severity: CRITICAL
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: lambda.update-function-code-invalid-signature-rejected
Issue-Shape-Id: com.amazonaws.lambda#AWSGirApiService
Issue-Key: lambda|UpdateFunctionCode|CodeVerificationFailedException
Issue-Commit: 2ac91d382b67c33b3b1ad8ceed805222f48956bf

# lambda: UpdateFunctionCode now refuses a package signed by an AllowedPublishers profile with CodeVerificationFailedException, so no deployment under an Enforce CodeSigningConfig can succeed

> _Note: this issue was identified against the code base at commit `2ac91d382b67c33b3b1ad8ceed805222f48956bf`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
UpdateFunctionCode

## Description
REOPENS 20261029T233239Z-lambda-code-signing-is-never-enforced-and-code.md: its fix left the call refused.

Cause 1 of 1: with an Enforce CodeSigningConfig attached, UpdateFunctionCode now raises CodeVerificationFailedException for EVERY package, including one signed by a signing profile that is listed in the config's AllowedPublishers.
AWS: Smithy binds CodeVerificationFailedException (httpError 400, members {Type, Message}) to UpdateFunctionCode, and documents it as raised when "the code signature failed one or more of the validation checks for signature mismatch or expiry, and the code signing policy is set to ENFORCE". A package signed by a profile inside AllowedPublishers passes those checks, so UpdateFunctionCode answers 200, LastUpdateStatus settles to `Successful`, CodeSha256 equals the deployed bytes' own SHA-256, and the function then executes the newly deployed code. Only a package signed outside AllowedPublishers (or unsigned) is refused, with the message `Lambda cannot deploy the function. The function or layer might be signed using a signature that the client is not configured to accept. Check the provided signature for <resource ARN>.`
Emulator: the correctly-signed update — the positive control, performed immediately after attaching the Enforce config and before any mismatched signer is involved — is refused with 400 CodeVerificationFailedException and exactly that message naming the function ARN. The referenced fixed issue made CreateFunction/UpdateFunctionCode enforce the policy; the enforcement now rejects valid signatures as well as invalid ones, so the whole code-signing path is closed rather than gated.
Why: this is worse than the original gap. A client that adopts code signing can no longer deploy at all: every UpdateFunctionCode under an Enforce config fails with a signature error even when the artifact was signed by the exact profile the config allows, and the refusal is indistinguishable from a genuine signature mismatch, so there is no way to tell a misconfigured publisher from a broken verifier.
Fix: the UpdateFunctionCode code-signing verification in the lambda crate — it must verify the submitted signature against the attached CodeSigningConfig's AllowedPublishers and admit a package whose signing profile is listed, refusing only on an actual mismatch/expiry.
Traces: lambda.update-function-code-invalid-signature-rejected.

## Evidence
lambda.update-function-code-invalid-signature-rejected — botocore.errorfactory.CodeVerificationFailedException: An error occurred (CodeVerificationFailedException) when calling the UpdateFunctionCode operation: Lambda cannot deploy the function. The function or layer might be signed using a signature that the client is not configured to accept. Check the provided signature for arn:aws:lambda:<region>:<account>:function:<fn>.
The refused call is the check's positive control at line 312676 of the report's listing: `lam.update_function_code(FunctionName=fn["name"], ZipFile=v2_signed)` where v2_signed was signed by `allowed_profile`, a profile inside the CodeSigningConfig's AllowedPublishers.
Expected per AWS: HTTP 200, LastUpdateStatus "Successful", CodeSha256 == SHA-256 of the deployed bytes, and the function running the new code. Smithy: CodeVerificationFailedException httpError 400, documented for signature mismatch/expiry under ENFORCE only.

## Steps to Reproduce
1. Create two Signer signing profiles; create a Lambda CodeSigningConfig with UntrustedArtifactOnDeployment=Enforce whose AllowedPublishers lists only the first profile, and attach it to a function.
2. Sign a zip with the ALLOWED profile and call UpdateFunctionCode with it: AWS answers 200 and settles LastUpdateStatus Successful; the emulator refuses with 400 CodeVerificationFailedException.
3. Only then sign a zip with the other (mismatched) profile and call UpdateFunctionCode: AWS refuses with 400 CodeVerificationFailedException naming the function ARN.
