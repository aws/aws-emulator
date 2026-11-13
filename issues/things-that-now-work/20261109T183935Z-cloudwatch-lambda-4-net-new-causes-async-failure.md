Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudwatch__lambda.metrics-lambda-dead-letter-errors-counted-only-when-dlq-delivery-fails cloudwatch__lambda.obs-lambda-1-deleted-on-failure-queue-meters-delivery-failure-and-event-age cloudwatch__lambda.metrics-lambda-function-url-invoke-meters-the-same-as-the-invoke-api cloudwatch__lambda.recursive-invocations-dropped-is-emitted-only-when-the-guardrail-terminates
Issue-Key: cloudwatch__lambda.metrics-lambda-dead-letter-errors-counted-only-when-dlq-delivery-fails|run#26|AssertionError|assert sorted(bodies, key=lambda b: b["seq"]) == payloads, (
Issue-Key: cloudwatch__lambda.obs-lambda-1-deleted-on-failure-queue-meters-delivery-failure-and-event-age|run#30|AssertionError|assert len(records) == 1, (
Issue-Key: cloudwatch__lambda.metrics-lambda-function-url-invoke-meters-the-same-as-the-invoke-api|_create_url#5|AssertionError|assert not (p := ctx.published_url_mismatch(cfg["FunctionUrl"])), p
Issue-Key: cloudwatch__lambda.recursive-invocations-dropped-is-emitted-only-when-the-guardrail-terminates|run#12|AssertionError|assert allow_depth == _MAX_DEPTH, (
Issue-Commit: c33038d6637516191c9271b81207503148b3a194

# cloudwatch__lambda: 4 net-new causes — async failure plumbing is inert (no DLQ delivery, no on-failure destination record), the function URL is published on an unreachable on.aws host, and RecursiveLoop=Allow does not permit recursion

> _Note: this issue was identified against the code base at commit `c33038d6637516191c9271b81207503148b3a194`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
Invoke

## Description
Cause 1 of 4: a failed asynchronous invocation is never delivered to the function's dead-letter queue.
AWS: when an async (Event) invocation exhausts its retries and the function has DeadLetterConfig pointing at an SQS queue, the event payload is written to that queue; AWS also counts DeadLetterErrors only when the DLQ delivery itself fails. Smithy (lambda 2015-03-31): FunctionConfiguration$DeadLetterConfig$TargetArn is the configured destination.
Emulator: the live DLQ receives nothing — the two failing events never appear — so the 'DLQ delivery failed' differential the check exists for cannot be produced.
Why: async events that fail are silently lost. A customer who configured a DLQ specifically to capture them gets an empty queue and no indication that anything was dropped.
Fix: the asynchronous invocation retry/terminal-failure path must write the event to the configured DeadLetterConfig target.
Failing check: cloudwatch__lambda.metrics-lambda-dead-letter-errors-counted-only-when-dlq-delivery-fails.

Cause 2 of 4: an OnFailure destination receives no record for a failed async invocation.
AWS: PutFunctionEventInvokeConfig with DestinationConfig.OnFailure delivers an invocation record (requestContext + responsePayload) to the destination for each async invocation that ends in failure, and deleting the destination resource makes the delivery fail and meters the failure. Smithy (lambda 2015-03-31): DestinationConfig$OnFailure$Destination.
Emulator: the live destination received 0 records in 300 s where exactly 1 was expected.
Why: the modern replacement for DLQs does nothing, so failed async invocations produce no record anywhere and failure-handling pipelines built on destinations never run.
Fix: the asynchronous invocation completion path must emit the invocation record to the configured OnFailure destination (and meter a failed delivery).
Failing check: cloudwatch__lambda.obs-lambda-1-deleted-on-failure-queue-meters-delivery-failure-and-event-age.

Cause 3 of 4: the function URL the emulator publishes is on the real AWS domain and is not a host the emulator serves.
AWS: CreateFunctionUrlConfig returns a FunctionUrl that the caller is expected to send HTTP requests to in order to invoke the function, and requests to it reach that function (and are metered identically to the Invoke API). Smithy (lambda 2015-03-31): FunctionUrlConfig$FunctionUrl. The requirement is that the announced host is the one the emulator is reached on; the literal id in any one run is not part of it.
Emulator: the published URL is 'https://<32-hex>.lambda-url.<region>.on.aws/' while the emulator serves on its own host:port, so the harness refuses it — 'its host is not one of the emulator's own hosts; a customer of the emulator needs http(s)://127.0.0.1:<port>/<resource path>, or http(s)://<own host>:<port>/<path> on a per-resource listener'. No URL invoke is possible, so the 'URL invoke meters the same as the Invoke API' comparison cannot be made.
Why: this is an egress refusal owned by the emulator: it announced an address other than the one it answers on. A client that follows the service's own published URL sends its request outside the emulator, so function URLs are unusable.
Fix: the function-URL serializer/state that mints FunctionUrl — publish the emulator's own reachable host (or a per-resource listener) instead of an on.aws name.
Failing check: cloudwatch__lambda.metrics-lambda-function-url-invoke-meters-the-same-as-the-invoke-api.

Cause 4 of 4: a function configured with RecursiveLoop=Allow does not actually recurse.
AWS: PutFunctionRecursionConfig with RecursiveLoop='Allow' disables the recursion guardrail, so a self-invoking chain runs to whatever ceiling the function imposes on itself; with 'Terminate' the chain is stopped at the guardrail's depth and RecursiveInvocationsDropped is emitted. Smithy (lambda 2015-03-31): PutFunctionRecursionConfig$RecursiveLoop with values Allow/Terminate.
Emulator: the Allow chain reaches depth 0 — the self-invocation never progresses — so Allow is not a control and the absence of RecursiveInvocationsDropped on the Terminate twin proves nothing. (The previously fixed issue 20261026T065236Z-cloudwatch-lambda-lambda-s-recursion-guardrail-is-inert-no-aws.md covered this guardrail; the chain still does not progress.)
Why: a customer cannot run a legitimate recursive workflow after opting in, and cannot tell the guardrail apart from a broken self-invoke path.
Fix: the Lambda self-invocation/recursion-detection component — honour RecursiveLoop=Allow by letting the chain proceed, and emit RecursiveInvocationsDropped only when Terminate stops it.
Failing check: cloudwatch__lambda.recursive-invocations-dropped-is-emitted-only-when-the-guardrail-terminates.

## Evidence
Cause 1 — Expected: exactly the 2 failed events in the DLQ. Actual: 'AssertionError: the live DLQ received [], expected exactly the 2 events [{\'fail\': True, \'seq\': 0}, {\'fail\': True, \'seq\': 1}]'.
Cause 2 — Expected: 1 invocation record at the OnFailure destination. Actual: 'AssertionError: the live destination received 0 records within 300.0s, expected 1: []'.
Cause 3 — Expected: FunctionUrl is an endpoint the emulator serves. Actual: 'AssertionError: the emulator published https://<32-hex>.lambda-url.us-west-2.on.aws/: its host is not one of the emulator\'s own hosts; a customer of the emulator needs http(s)://127.0.0.1:<port>/<resource path>'.
Cause 4 — Expected: the Allow chain runs to its self-imposed ceiling of 24. Actual: 'AssertionError: the RecursiveLoop=Allow chain on <fn> reached depth 0, expected it to run to its own self-imposed ceiling of 24'.

## Steps to Reproduce
Cause 1: create a function with DeadLetterConfig -> SQS; async-invoke it twice with payloads that always fail; poll the DLQ.
Cause 2: PutFunctionEventInvokeConfig with DestinationConfig.OnFailure -> SQS; async-invoke a failing handler once; poll the destination.
Cause 3: CreateFunctionUrlConfig and issue an HTTP request to the returned FunctionUrl; observe the host is not one the emulator answers on.
Cause 4: PutFunctionRecursionConfig(RecursiveLoop='Allow') on a handler that invokes itself with a depth counter; invoke once and read the reached depth.
