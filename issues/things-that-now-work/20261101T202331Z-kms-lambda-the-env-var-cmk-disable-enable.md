Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: kms__lambda.deep-lambda-2-env-kms-disable-blocks-cold-invoke-then-enable-restores
Issue-Key: kms__lambda.deep-lambda-2-env-kms-disable-blocks-cold-invoke-then-enable-restores|_poll#12|Timeout|raise AssertionError(f"{what} not observed within {timeout_s}s; last={last!r}")
Issue-Commit: 62e4c5cd3531cb5ea16934ee4e8b2e43c8dd65a3

# kms__lambda: the env-var CMK disable/enable lifecycle is unobservable — a cold Invoke never refuses with KMSDisabledException and GetFunctionConfiguration never surfaces Environment.Error (1 cause)

> _Note: this issue was identified against the code base at commit `62e4c5cd3531cb5ea16934ee4e8b2e43c8dd65a3`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
Invoke

## Description
Attribution note: the grounding check id's leading token is the composite "kms__lambda", so this issue is filed under that token. Both constituent services are bootstrapped in this checkout (service-com-amazonaws-kms-trentservice and service-com-amazonaws-lambda-awsgirapiservice are in the bootstrapped set; only athena, cloudcontrol, macie2 and s3tables are pending), so the per-service gate is FILE.

Cause 1 of 1: a function whose environment variables are encrypted with a customer CMK does not react to that CMK being disabled, and the invoke path that would show it never produces a result at all. Failing check: kms__lambda.deep-lambda-2-env-kms-disable-blocks-cold-invoke-then-enable-restores (verdict: mismatch, "Check timed out after 60.0s").

AWS: three modeled facts are involved. (1) Smithy Invoke — POST /2015-03-31/functions/{FunctionName}/invocations, success code 200, output InvocationResponse{StatusCode (httpResponseCode), Payload (httpPayload), ExecutedVersion, FunctionError}; a function that exists and is Active is executed and the caller gets the handler's payload. NotImplemented is not in Invoke's modeled error set. (2) Smithy Invoke error KMSDisabledException — errorKind server, httpError 502, documented "Lambda couldn't decrypt the environment variables because the KMS key used is disabled. Check the Lambda function's KMS key settings." This is the answer to a COLD invoke (a new execution environment, e.g. after an UpdateFunctionConfiguration settles) once kms:DisableKey has been applied to the function's KMSKeyArn; a warm sandbox keeps its already-decrypted plaintext. Its message is a fixed envelope with values taken from the request/state: "Lambda was unable to decrypt the environment variables because the KMS key used is disabled. Please check the function's KMS key settings. KMS Exception: DisabledExceptionKMS Message: <KMSKeyArn of the function> is disabled. (Service: Kms, Status Code: 400, Request ID: <the relayed KMS request id>) (SDK Attempt Count: 1)" — fixed text is everything outside the angle brackets; the key ARN comes from the function's configuration and the request id is the only per-call token (never a requirement in itself). (3) Smithy GetFunctionConfiguration output member Environment -> EnvironmentResponse{Variables (sensitive map), Error -> EnvironmentError{ErrorCode, Message}}, documented "Error messages for environment variables that couldn't be applied." While the CMK is disabled AWS reports Environment.Error.ErrorCode = "DisabledException" with Environment.Error.Message "Lambda was unable to decrypt your environment variables because the KMS key used is disabled. Please check your KMS key settings. KMS Exception: DisabledException KMS Message: <KMSKeyArn of the function> is disabled." and OMITS Environment.Variables entirely, so the plaintext is not readable through the control plane. After kms:EnableKey both effects reverse: Environment.Error disappears, Variables are shown again, and the identical Invoke succeeds with the handler's payload.

Emulator: the check reaches no verdict — it is killed at the harness 60 s cap while polling, so none of the three transitions above is ever observed. The sibling checks in this same service group pin down why the invoke half cannot progress: kms__lambda.deep-lambda-2-env-kms-absent-key-rejected-and-state-unchanged, kms__lambda.kms-grant-authorizes-lambda-decrypt-then-revoke-denies and kms__lambda.kms-key-policy-deny-overrides-lambda-identity-allow all fail with botocore ClientError "An error occurred (NotImplemented) when calling the Invoke operation", i.e. a resolved, Active function is refused instead of executed. Hypothesis (neither the model nor this recording states it): because the refusal happens before any environment is built, the KMS state of the function's KMSKeyArn is never consulted, so DisableKey/EnableKey produce no change on either the data plane (no KMSDisabledException, no 502) or the control plane (GetFunctionConfiguration keeps answering without an Environment.Error and keeps exposing Environment.Variables), and the check's poll loops therefore never see a transition and run to the cap.

Why: a client that encrypts function environment variables with its own CMK cannot see the only two signals AWS gives it. Nothing distinguishes "my key is disabled" from "my function is broken": a caller gets no KMSDisabledException/502 to retry or alarm on, and GetFunctionConfiguration reports a healthy-looking configuration — while still handing back the decrypted variable value, which is exactly what AWS withholds once the key cannot be used. Recovery is equally invisible: after EnableKey there is no observable return to a successful invoke, so key-rotation/disable runbooks and any guard built on Environment.Error cannot be validated against the emulator.

Fix: Lambda's Invoke handler execution path (an execution of the deployment package, or at minimum a faithful 200 + Payload path for resolved Active functions, and never NotImplemented for a function that resolved), plus an environment-build step in that handler that resolves the function's KMSKeyArn against KMS state and maps a disabled key to KMSDisabledException with HTTP 502 and the message envelope above; and the function-configuration projection used by GetFunctionConfiguration, which must carry EnvironmentResponse.Error{ErrorCode,Message} and drop Variables while the key is unusable, clearing both when the key is re-enabled.

Traces: kms__lambda.deep-lambda-2-env-kms-disable-blocks-cold-invoke-then-enable-restores. Related open issues that do NOT cover this check (none of them carries its replay key, and none covers the Environment.Error / KMSDisabledException contract): 20261022T213144Z-lambda-invoke-refuses-every-real-invocation-with-notimplemented.md and 20261029T233239Z-lambda-synchronous-invoke-data-plane-answers-501-notimplemented.md (both describe the generic Invoke NotImplemented refusal).

## Evidence
Suite: simparity-behavioral-aws-v1 — kms__lambda 0/4 passed.

Failing check (verdict mismatch):
  kms__lambda.deep-lambda-2-env-kms-disable-blocks-cold-invoke-then-enable-restores
  Error: Check timed out after 60.0s

Supporting evidence from the same service group (these three are already owned by the generic Invoke issue, and are quoted only to show what the invoke path answers):
  botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the Invoke operation
  — kms__lambda.deep-lambda-2-env-kms-absent-key-rejected-and-state-unchanged
  — kms__lambda.kms-grant-authorizes-lambda-decrypt-then-revoke-denies
  — kms__lambda.kms-key-policy-deny-overrides-lambda-identity-allow

Expected (per the Smithy model and the check's assertions), with <key-arn> = the function's KMSKeyArn and <marker> = the environment variable value supplied at CreateFunction:
  1. With the key enabled: GetFunctionConfiguration returns KMSKeyArn == <key-arn>, Environment.Variables.SECRET == <marker>, and no Environment.Error; Invoke returns HTTP 200, StatusCode 200, no FunctionError, Payload {"secret": "<marker>"}.
  2. After kms:DisableKey: GetFunctionConfiguration returns Environment.Error.ErrorCode == "DisabledException" and Environment.Error.Message == "Lambda was unable to decrypt your environment variables because the KMS key used is disabled. Please check your KMS key settings. KMS Exception: DisabledException KMS Message: <key-arn> is disabled.", with Environment.Variables ABSENT.
  3. A cold Invoke (after an UpdateFunctionConfiguration settles) raises error code KMSDisabledException with HTTP status 502 and message "Lambda was unable to decrypt the environment variables because the KMS key used is disabled. Please check the function's KMS key settings. KMS Exception: DisabledExceptionKMS Message: <key-arn> is disabled. (Service: Kms, Status Code: 400, Request ID: <kms request id>) (SDK Attempt Count: 1)" — fixed text as written, <key-arn> from state, <kms request id> a per-call token; every subsequent Invoke answers identically.
  4. After kms:EnableKey: the identical Invoke succeeds again with Payload {"secret": "<marker>"}, and Environment.Error clears while Environment.Variables.SECRET reappears.
Actual: no step past (1) is ever reached — the check is killed at the 60 s cap, and the invoke path in this service group answers error code NotImplemented for a resolved Active function.

Smithy citations: Invoke (POST /2015-03-31/functions/{FunctionName}/invocations, code 200; errors include KMSDisabledException errorKind=server httpError=502, KMSAccessDeniedException/KMSInvalidStateException/KMSNotFoundException 502; NotImplemented is NOT modeled); GetFunctionConfiguration output member Environment -> EnvironmentResponse{Variables sensitive, Error -> EnvironmentError{ErrorCode, Message}}.

## Steps to Reproduce
1. KMS CreateKey (symmetric, ENCRYPT_DECRYPT) and note its Arn; create an IAM role trusted by lambda.amazonaws.com with an inline policy allowing kms:Decrypt on that key ARN.
2. Lambda CreateFunction with Runtime=python3.12, a zip whose handler returns {'secret': os.environ.get('SECRET')}, Role=<role>, KMSKeyArn=<key arn>, Environment={'Variables': {'SECRET': '<marker>'}}; wait for State=Active.
3. GetFunctionConfiguration — expect KMSKeyArn echoed, Environment.Variables.SECRET == '<marker>', no Environment.Error. Invoke(InvocationType=RequestResponse, Payload=b'{}') — expect HTTP 200 and Payload {"secret": "<marker>"}.
4. kms:DisableKey on the key. Poll GetFunctionConfiguration: expect Environment.Error.ErrorCode == 'DisabledException' with the "Lambda was unable to decrypt your environment variables ... KMS Message: <key arn> is disabled." message and NO Environment.Variables. The emulator keeps reporting the configuration unchanged.
5. Force a cold environment (UpdateFunctionConfiguration changing Timeout, wait for LastUpdateStatus=Successful), then Invoke with retries disabled: expect error code KMSDisabledException, HTTP 502 and the message quoted in Evidence, repeated identically on a second Invoke. The emulator never produces it (the invoke path answers NotImplemented for a resolved function).
6. kms:EnableKey and repeat the Invoke: expect the original success and Payload {"secret": "<marker>"}, and Environment.Error to clear.
7. Full grounding: replay kms__lambda.deep-lambda-2-env-kms-disable-blocks-cold-invoke-then-enable-restores, which currently reaches no verdict (killed at the 60 s cap).
