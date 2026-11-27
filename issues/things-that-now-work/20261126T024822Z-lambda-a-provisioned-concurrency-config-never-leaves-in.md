Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: lambda.stream-provisioned-environment-reports-provisioned-initialization-type
Issue-Shape-Id: com.amazonaws.lambda#AWSGirApiService
Issue-Key: lambda.stream-provisioned-environment-reports-provisioned-initialization-type|_wait_provisioned_ready#16|AssertionError|raise AssertionError(
Issue-Commit: fa14d914f5c194a73e1a5e80eef1b3fda3ddba93

# lambda: a provisioned-concurrency config never leaves IN_PROGRESS — Allocated/Available stay 0, so no environment ever reports AWS_LAMBDA_INITIALIZATION_TYPE=provisioned-concurrency

> _Note: this issue was identified against the code base at commit `fa14d914f5c194a73e1a5e80eef1b3fda3ddba93`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutProvisionedConcurrencyConfig

## Description
Cause 1 of 1: provisioned-concurrency allocation never completes. Failing check: lambda.stream-provisioned-environment-reports-provisioned-initialization-type.

AWS: PutProvisionedConcurrencyConfig (Smithy: PUT /2019-09-30/functions/{FunctionName}/provisioned-concurrency, http code 202; output members RequestedProvisionedConcurrentExecutions, AllocatedProvisionedConcurrentExecutions, AvailableProvisionedConcurrentExecutions, Status, StatusReason, LastModified) accepts the request and the configuration is then allocated asynchronously; GetProvisionedConcurrencyConfig (Smithy: GET on the same URI, http code 200) reports Status from ProvisionedConcurrencyStatusEnum = IN_PROGRESS | READY | FAILED, and a successful allocation converges on Status=READY with AllocatedProvisionedConcurrentExecutions == AvailableProvisionedConcurrentExecutions == RequestedProvisionedConcurrentExecutions. Only once the allocation is READY does the platform pre-warm environments, and a handler served by one of them reads the environment variable AWS_LAMBDA_INITIALIZATION_TYPE as 'provisioned-concurrency' (spillover beyond the provisioned count reads 'on-demand'). Recorded behavior only for the convergence latency: the emulator is the only thing bounding it here.

Emulator: PutProvisionedConcurrencyConfig for a published version with ProvisionedConcurrentExecutions=1 is accepted, and every GetProvisionedConcurrencyConfig read answers HTTP 200 with Status='IN_PROGRESS', AllocatedProvisionedConcurrentExecutions=0 and AvailableProvisionedConcurrentExecutions=0 (RequestedProvisionedConcurrentExecutions=1, LastModified set). The allocation is never advanced to READY (nor to FAILED with a StatusReason) while the check can observe it, so the readiness poll ends with AssertionError 'provisioned concurrency for <function>:<version> never became READY' and the downstream leg — that provisioned environments report AWS_LAMBDA_INITIALIZATION_TYPE='provisioned-concurrency' while burst spillover reports 'on-demand' — is never reached. The emulator has no asynchronous allocator driving the config out of IN_PROGRESS, so the state the API advertises is terminal in practice.

Why: a client that provisions concurrency and waits for Status=READY before cutting traffic over (the pattern AWS's own waiters and the CLI 'wait' helpers implement, and what application-autoscaling targets on lambda:function:ProvisionedConcurrency depend on) blocks forever against the emulator; it can never observe AllocatedProvisionedConcurrentExecutions reaching the requested value, and code that distinguishes pre-warmed from on-demand environments via AWS_LAMBDA_INITIALIZATION_TYPE never sees 'provisioned-concurrency'.

Fix: the provisioned-concurrency state in the Lambda service's function/version state plus the PutProvisionedConcurrencyConfig handler that creates it — the config must be settled (Allocated/Available raised to the requested count and Status moved to READY, or FAILED with AWS's StatusReason) and the pre-warmed environments it represents must be the ones the invoke path hands out, with AWS_LAMBDA_INITIALIZATION_TYPE set to 'provisioned-concurrency' in them; GetProvisionedConcurrencyConfig then simply reports that settled state.

Traces: lambda.stream-provisioned-environment-reports-provisioned-initialization-type (mismatch) — the only unowned grounding; the sibling check lambda.provisioned-concurrency-invocation-reports-no-init-duration fails on a different cause already carried by 20261119T000000Z-lambda-7-of-the-parent-reports-10-causes-remain....md, and 20261031T230626Z-application-autoscaling-lambda-registering-a-lambda-function-provisionedconcurrency.md (fixed, scaling-target driven) does not cover a direct PutProvisionedConcurrencyConfig that never settles.

## Evidence
Check: lambda.stream-provisioned-environment-reports-provisioned-initialization-type [mismatch]

Expected (AWS): after PutProvisionedConcurrencyConfig(FunctionName=<fn>, Qualifier=<published version>, ProvisionedConcurrentExecutions=1) is accepted (202), GetProvisionedConcurrencyConfig converges on HTTP 200 with Status='READY', AllocatedProvisionedConcurrentExecutions=1 and AvailableProvisionedConcurrentExecutions=1 (== RequestedProvisionedConcurrentExecutions). Invocations served by that pre-warmed environment then read AWS_LAMBDA_INITIALIZATION_TYPE='provisioned-concurrency'; concurrent invocations beyond the provisioned count read 'on-demand'.

Actual (emulator): every GetProvisionedConcurrencyConfig read is HTTP 200 with
  {'RequestedProvisionedConcurrentExecutions': 1,
   'AvailableProvisionedConcurrentExecutions': 0,
   'AllocatedProvisionedConcurrentExecutions': 0,
   'Status': 'IN_PROGRESS',
   'LastModified': '<timestamp>'}
and the readiness poll fails:
  AssertionError: provisioned concurrency for <function-name>:<version> never became READY ...; last={... 'Status': 'IN_PROGRESS', 'AllocatedProvisionedConcurrentExecutions': 0, 'AvailableProvisionedConcurrentExecutions': 0 ...}
(the function name, version, request id and LastModified above are per-run values; the requirement is Status/Allocated/Available converging on READY/1/1, not those literals). No StatusReason is ever set, so the config is neither READY nor FAILED.

## Steps to Reproduce
1. CreateFunction (Runtime=python3.12, Handler=index.handler, MemorySize=512, Timeout=60) and wait for State=Active.
2. PublishVersion, and wait for the qualified version to be Active.
3. PutProvisionedConcurrencyConfig(FunctionName=<fn>, Qualifier=<version>, ProvisionedConcurrentExecutions=1).
4. Poll GetProvisionedConcurrencyConfig(FunctionName=<fn>, Qualifier=<version>): the emulator keeps answering Status='IN_PROGRESS' with AllocatedProvisionedConcurrentExecutions=0 and AvailableProvisionedConcurrentExecutions=0; it never reaches Status='READY' with Allocated/Available == 1 (nor FAILED with a StatusReason).
5. Consequently, invoking the qualified version concurrently never yields a handler whose os.environ['AWS_LAMBDA_INITIALIZATION_TYPE'] is 'provisioned-concurrency'.

Replay: run the behavioral suite and re-run check lambda.stream-provisioned-environment-reports-provisioned-initialization-type.
