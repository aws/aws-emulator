Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: iam__lambda.capacity-provider-operator-role-without-ec2-policy-rejected iam__lambda.nodejs24-asynclocalstorage-keeps-its-store-across-an-await-under-concurrent-contexts iam__lambda.nodejs24-global-fetch-reads-a-data-url-it-encoded-itself
Issue-Key: iam__lambda.capacity-provider-operator-role-without-ec2-policy-rejected|_refusal#18|AssertionError|raise AssertionError(f"the call was accepted: {result!r}")
Issue-Key: iam__lambda.nodejs24-asynclocalstorage-keeps-its-store-across-an-await-under-concurrent-contexts|_call#3|AssertionError|assert error is None, f"the function raised ({error}): {raw[:600]!r}"
Issue-Key: iam__lambda.nodejs24-global-fetch-reads-a-data-url-it-encoded-itself|_call#4|AssertionError|assert error is None, f"the function raised ({error}): {raw[:600]!r}"
Issue-Commit: cc8c6d4514b1c20009096c9d1c08086c7dabca5f

# lambda: CreateCapacityProvider accepts an operator role without the required EC2 policy, and the nodejs24 runtime loses AsyncLocalStorage context across an await and cannot fetch() a data: URL

> _Note: this issue was identified against the code base at commit `cc8c6d4514b1c20009096c9d1c08086c7dabca5f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateCapacityProvider

## Description
Three net-new root causes under the `iam__lambda` heading.

=== Cause 1 of 3: CreateCapacityProvider is accepted with an operator role that cannot operate EC2 ===
AWS: `CreateCapacityProvider` validates `PermissionsConfig.CapacityProviderOperatorRoleArn` before accepting the provider — the role must be assumable by Lambda AND carry the managed EC2-operator grant (`AWSLambdaManagedEC2ResourceOperator`). Without it Lambda cannot read the VPC configuration it was handed and the call is REFUSED, with the message class the check enumerates (the role refusal, or the security-group read failure `One or more security group IDs are invalid. Check that the IDs are correct and try again.` when the role cannot describe them). Recorded behavior only for the exact wording; the requirement is that the create is refused and no capacity provider is left behind.
Emulator: the call answers 200 and returns a provider: `{'CapacityProviderArn': 'arn:aws:lambda:<region>:<account>:capacity-provider:<name>', 'State': 'Pending', 'VpcConfig': {...}, 'PermissionsConfig': {'CapacityProviderOperatorRoleArn': '<role arn>'}, 'InstanceRequirements': {'Architectures': ['x86_64']}, ...}` — the role's permissions are never checked.
Why: the provider is accepted and sits in `Pending` forever instead of telling the caller its operator role is unusable. A client gets no error to react to at create time and no signal that the role, not the request, is at fault.
Fix: `CreateCapacityProvider`'s input validation — resolve the operator role, confirm the Lambda trust and the EC2-operator grant, and refuse (rather than store) when the VPC configuration cannot be read with it.
Traces: iam__lambda.capacity-provider-operator-role-without-ec2-policy-rejected.

=== Cause 2 of 3: the nodejs24 runtime does not propagate an AsyncLocalStorage store across an await ===
AWS: recorded behavior only (runtime behavior, not modelled). On the `nodejs24.x` managed runtime, `AsyncLocalStorage.run(store, fn)` keeps `getStore()` returning the SAME store after an `await` inside `fn`, and keeps distinct stores separate across concurrently running contexts — this is Node's core async-context propagation. The handler the check deploys reads `als.getStore().id` after an await and must get back the id it entered with, for each concurrent context.
Emulator: the invocation fails with an unhandled error — `TypeError: Cannot read properties of undefined (reading 'id')` at `readBackAfterAwait (/var/task/index.js:7:25)` — i.e. `getStore()` returned `undefined` after the await: the store is lost.
Why: AsyncLocalStorage is the standard mechanism for request-scoped context (trace ids, tenant ids, loggers) in Node handlers. Against the emulator any such handler throws on its first read after an await, so the whole family of context-propagating libraries (OpenTelemetry, pino/async context, request-id middleware) fails.
Fix: the nodejs24 runtime's async-context propagation (`async_hooks` / `AsyncLocalStorage` support in the emulated Node runtime).
Traces: iam__lambda.nodejs24-asynclocalstorage-keeps-its-store-across-an-await-under-concurrent-contexts.

=== Cause 3 of 3: the nodejs24 runtime's global fetch() cannot read a data: URL ===
AWS: recorded behavior only (runtime behavior, not modelled). On `nodejs24.x`, `fetch` is a global and supports the `data:` scheme, so a handler that base64-encodes text into a `data:` URL and fetches it back gets the original text — no network is involved.
Emulator: the invocation fails unhandled with `TypeError: fetch failed` (trace frames in `node:internal/webglobals`, `node:internal/http`), so the `data:` scheme is routed through the HTTP path instead of being handled in-process.
Why: a handler that uses the global `fetch` for a non-network scheme fails, and the failure looks like a network problem, so a client misdiagnoses it as egress. More broadly, the runtime's `fetch` does not implement the schemes Node's does.
Fix: the nodejs24 runtime's global `fetch` implementation — handle the `data:` scheme in-process rather than dispatching it to the HTTP client.
Traces: iam__lambda.nodejs24-global-fetch-reads-a-data-url-it-encoded-itself.

## Evidence
From one replay of the behavioral suite (service heading `### iam__lambda`; the other failure under that heading is owned by an already-filed issue).

[mismatch] iam__lambda.capacity-provider-operator-role-without-ec2-policy-rejected
  File "...capacity_provider_operator_role_without_ec2_policy_rejected.py", line 127, in _refusal
    raise AssertionError(f"the call was accepted: {result!r}")
  AssertionError: the call was accepted: {'CapacityProviderArn': 'arn:aws:lambda:<region>:<account>:capacity-provider:<generated-name>', 'State': 'Pending', 'VpcConfig': {'SubnetIds': ['subnet-...'], 'SecurityGroupIds': ['sg-...']}, 'PermissionsConfig': {'CapacityProviderOperatorRoleArn': 'arn:aws:iam::<account>:role/<generated-role>'}, 'InstanceRequirements': {'Architectures': ['x86_64']}, 'CapacityProviderScalingConfig': {'MaxVCpuCount': 16}, 'LastModified': '...'}
  (arns, ids and timestamp are per-run; the requirement is that the create is REFUSED)

[mismatch] iam__lambda.nodejs24-asynclocalstorage-keeps-its-store-across-an-await-under-concurrent-contexts
  AssertionError: the function raised (Unhandled): b'{"errorMessage":"Cannot read properties of undefined (reading \'id\')","errorType":"TypeError","trace":["TypeError: Cannot read properties of undefined (reading \'id\')","    at readBackAfterAwait (/var/task/index.js:7:25)"]}'

[mismatch] iam__lambda.nodejs24-global-fetch-reads-a-data-url-it-encoded-itself
  AssertionError: the function raised (Unhandled): b'{"errorMessage":"fetch failed","errorType":"TypeError","trace":["TypeError: fetch failed","    at failed (node:internal/webglobals:570:35)","    at apply (native)","    at <anonymous> (node:internal/events:94:38)","    at raise (node:internal/http:472:27)","    at answer (node:internal/http:327:43)","    at send (node:internal/http:314:21)","    at <anonymous> (node:internal/http:235:14)"]}'

## Steps to Reproduce
1. Cause 1: create an IAM role trusted by lambda.amazonaws.com WITHOUT the `AWSLambdaManagedEC2ResourceOperator` managed policy, plus a VPC subnet and security group. Call `CreateCapacityProvider` with `PermissionsConfig.CapacityProviderOperatorRoleArn` = that role, the VPC config, and `InstanceRequirements.Architectures=['x86_64']`. The call must be refused and no provider created; observed 200 with `State: Pending`. Attaching the managed policy must then let the identical call succeed.
2. Cause 2: CreateFunction on `nodejs24.x` whose handler does `als.run({id}, async () => { await something(); return als.getStore().id; })` for several concurrent ids, and Invoke it. Each context must read back its own id; observed the invocation raises `TypeError: Cannot read properties of undefined (reading 'id')` — `getStore()` is undefined after the await.
3. Cause 3: CreateFunction on `nodejs24.x` whose handler builds `data:text/plain;base64,<base64 of the input text>` and does `await (await fetch(url)).text()`, and Invoke it. It must return the original text; observed the invocation raises `TypeError: fetch failed`.
