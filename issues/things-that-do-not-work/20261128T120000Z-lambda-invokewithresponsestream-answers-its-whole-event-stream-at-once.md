Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: lambda.deep-lambda-runtime-truth-stream-error-after-first-chunk-rejected-in-trailer
Issue-Shape-Id: com.amazonaws.lambda#AWSGirApiService
Issue-Key: lambda.deep-lambda-runtime-truth-stream-error-after-first-chunk-rejected-in-trailer|run#10|AssertionError|assert [r["seq"] for _, r in ok_records] == [1, 2, 3], f"control records {ok_records!r}"
Issue-Commit: c607a2586d7f91da73a3e70d6fec1fead9553942

# lambda: InvokeWithResponseStream answers its whole event stream at once, so no chunk reaches the client before the handler returns

> _Provenance: this was cause 1 of 7 of
> `20261110T121649Z-lambda-7-net-new-causes-invokewithresponsestream-delivers-no.md`. The other
> six causes of that issue are fixed and that report now sits under
> `issues/things-that-now-work/`; this cause is unfixed and is re-filed here so its check is not
> recorded as working. Its `Issue-Key` and its `Issue-Trace-Id` moved with it. The same
> outstanding work is also named by
> `20261027T180000Z-lambda-invokewithresponsestream-refuses-every-real-streaming-invocation.md`
> ("PayloadChunk frames emitted AS the handler writes them, so the first chunk reaches the
> client before the handler returns"), which carries this trace id too._

## Operation
InvokeWithResponseStream

## Description
AWS: Smithy lambda `InvokeWithResponseStream` (POST
`/2021-11-15/functions/{FunctionName}/response-streaming-invocations`) returns the output member
`EventStream` (`@streaming`, `@httpPayload`) carrying `PayloadChunk` events AS THE HANDLER
WRITES THEM, followed by the terminating `InvokeComplete`. The delivery is progressive: "your
function can start returning data to the client while it is still running", so a client reads
the first chunk before the handler has finished, and an observer watching the invocation while
it runs sees the handler's progress.

Emulator: every chunk the handler wrote, and the terminating event, are correct — but they all
arrive at once, when the invocation is answered. Nothing of the stream can be observed while the
handler is still running, so time to first byte is zero-information and an observer that looks
at the invocation's progress before the call returns sees nothing.

What is NOT the defect (verified end to end by
`services/service-com-amazonaws-lambda-awsgirapiservice/tests/all/response_stream_control_leg.rs`,
which drives Lambda, Amazon SQS and CloudWatch Logs on one engine): the `PayloadChunk` framing
and the one-chunk-per-write mapping; the terminating `InvokeComplete` and the error it names for
a handler that failed after its writes; the records the handler reported through the AWS SDK its
runtime supplies (read back in Amazon SQS and in the function's own tags); every line the
handler wrote (read back in its CloudWatch Logs group); and the execution environment's state
after the writes, read by the next invocation served in it.

Why: progressive delivery is the whole point of response streaming, and the AWS behaviour a
client builds on — a long-running handler that reports as it goes, a reader that acts on the
first chunk, an observer that watches an invocation's progress — is unobservable while a stream
arrives in one piece.

## The ONE blocker, for whoever picks this up
No service crate can deliver a response incrementally. The emulator's service contract hands the
whole response over as ONE value after the operation returns:

* `vendor/server/src/lib.rs:183` — `EmulatorService::handle_request` is typed
  `-> impl Future<Output = http::Response<bytes::Bytes>>`, a fully buffered body.
* `vendor/smithy-server-runtime/src/multi_service_router.rs:602` —
  `http_response_from_runtime_api` collects the operation's body into `bytes::Bytes` before the
  transport writes it.

Both are in the read-only `vendor/` tree. The core change this needs: a response body an
operation can write into as it produces bytes (a streaming body or a channel the transport
polls) reachable from `EmulatorService::handle_request` and preserved by the router's
conversion, so an event-stream operation can hand its frames over as it frames them. Until then
the operation's answer is complete and correct but not progressive, and re-wording or re-timing
anything inside `services/` cannot change that.

## Evidence
Replay output (behavioral suite, service heading ### lambda):

1) deep-lambda-runtime-truth-stream-error-after-first-chunk-rejected-in-trailer [mismatch]
   AssertionError: control records []

Established by experiment against the replay, so whoever picks this up need not repeat it:

* a diagnostic `PayloadChunk` carrying `{"seq":99,…}` injected into EVERY streaming response
  changed neither the assertion nor the (empty) list it prints — so the records the check reads
  are not taken from the chunks of the finished response;
* refusing the operation outright moved the first divergence to
  `…|_stream#8|Timeout|resp = lam.invoke_with_response_stream(`, so the check's step 8 is the
  call itself and its step 10 asserts afterwards — the records are read from a leg that is
  observed WHILE the invocation is in flight, which only progressive delivery can feed.

## Steps to Reproduce
1. Build and boot the emulator from the checkout, then replay the behavioral suite
   (run_behavioral_suite) and re-run the check named in `Issue-Trace-Ids`.
2. Manually: create a `nodejs22.x` function whose handler is wrapped in
   `awslambda.streamifyResponse` and writes three chunks with a pause between them, then call
   `InvokeWithResponseStream` and read the event stream. Every chunk and the terminating event
   arrive, but none of them before the handler has returned.
