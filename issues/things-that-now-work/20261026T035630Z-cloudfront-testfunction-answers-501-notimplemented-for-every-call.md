Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudfront.function-live-stage-before-publish-rejected
Issue-Shape-Id: com.amazonaws.cloudfront#Cloudfront2020_05_31
Issue-Key: cloudfront.function-live-stage-before-publish-rejected|run#10|AssertionError|assert code == "NoSuchFunctionExists", f"testing LIVE before publish returned {code!r}"
Issue-Commit: 632275b31d4caea8230a511abd27a180321ca025

# CloudFront TestFunction answers 501 NotImplemented for every call, so the LIVE-stage-before-publish NoSuchFunctionExists refusal never happens

> _Note: this issue was identified against the code base at commit `632275b31d4caea8230a511abd27a180321ca025`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
TestFunction

## Description
Cause 1 of 1: `TestFunction` is still a scaffold stub and refuses every invocation with 501 `NotImplemented`, so neither its success path nor any of its modeled refusals exist. The behavioral check reaches it while asserting the stage contract of a freshly created CloudFront Function.

AWS: Smithy (com.amazonaws.cloudfront#TestFunction) binds `POST /2020-05-31/function/{Name}/test`, success status 200, input `Name` (@httpLabel, required, @length 1..64, pattern `^[a-zA-Z0-9-_]{1,64}$`), `IfMatch` (required, If-Match header), `Stage` (enum FunctionStage: DEVELOPMENT | LIVE), `EventObject` (required, @sensitive blob, @length 0..40960); output `TestResult` (@httpPayload). Modeled errors: `InvalidArgument` (400), `InvalidIfMatchVersion` (400), `NoSuchFunctionExists` (404), `TestFunctionFailed` (500), `UnsupportedOperation` (400). A function created by CreateFunction exists only in the DEVELOPMENT stage (Status UNPUBLISHED) until PublishFunction promotes it, so a TestFunction call naming `Stage=LIVE` before publish addresses a stage that does not exist and AWS refuses it with HTTP 404, error code `NoSuchFunctionExists`, message `The specified function does not exist.` — that message is entirely fixed text for this error class (it carries no request- or state-derived values; it names neither the function nor the stage). Smithy documents `NoSuchFunctionExists` as "The function does not exist." On the accepted path (an existing stage plus a matching If-Match ETag) AWS runs the function against the supplied event object and returns 200 with a `TestResult` payload.

Emulator: every `TestFunction` request is refused with HTTP 501 and error code `NotImplemented`. The replayed check asserted the refusal code and got `AssertionError: testing LIVE before publish returned 'NotImplemented'` — the client saw code `NotImplemented` where AWS returns `NoSuchFunctionExists`, and consequently no `The specified function does not exist.` message at all. The handler is an untouched scaffold: `handle()` binds its inputs and returns `Err(crate::scaffolded::Scaffolded::not_implemented("TestFunction"))`, so the status/code/message are independent of the request. Note the sibling operation is NOT at fault: `DescribeFunction(Name, Stage="LIVE")` before publish already answers the correct `NoSuchFunctionExists` / `The specified function does not exist.` in the same check (those assertions passed immediately before the failure), so the divergence is confined to `TestFunction`.

Why: a client that uses TestFunction the way the CloudFront console and CI pipelines do — dry-run a function against a synthetic viewer-request event before and after publishing — cannot use it at all. It cannot obtain a TestResult (function output, logs, compute utilization) for a DEVELOPMENT function, and it cannot distinguish "this stage is not published yet" (404 NoSuchFunctionExists, a retry-after-publish condition) from a server-side outage: 501 is not a modeled outcome of this operation, so SDK error handling that branches on `NoSuchFunctionExists` falls through to a generic failure and stage-promotion workflows abort instead of publishing. The existing open issue 20261024T182555Z-cloudfront-publishfunction-stub.md covers PublishFunction only and does not cover TestFunction; no open or fixed issue names TestFunction.

Fix: the operation's handler — implement TestFunction in services/service-com-amazonaws-cloudfront-cloudfront2020-05-31/src/ops/test_function.rs (the `Err(..)` in `TestFunction::handle`, and delete the `SCAFFOLDED, NOT IMPLEMENTED` marker line). It needs the function store reached through `crate::state::service_state`: resolve `Name` plus the requested `Stage` (defaulting to DEVELOPMENT), refuse an absent function OR an absent stage with the modeled `NoSuchFunctionExists` carrying AWS's fixed message, refuse a non-matching/absent `IfMatch` with `InvalidIfMatchVersion`, and on the accepted path run the stage's code over `EventObject` and serialize a `TestResult` @httpPayload with success status 200.

Traces: cloudfront.function-live-stage-before-publish-rejected (the TestFunction(Stage="LIVE") call on an unpublished function).

## Evidence
Check: cloudfront.function-live-stage-before-publish-rejected [mismatch]

Expected (AWS): HTTP 404, Error.Code = "NoSuchFunctionExists", Error.Message = "The specified function does not exist." (fixed text, no variable parts) for TestFunction(Name=<new function>, Stage="LIVE") issued before PublishFunction.

Actual (emulator): HTTP 501, Error.Code = "NotImplemented" — returned for every TestFunction request regardless of Name/Stage/IfMatch/EventObject.

Replay output:
  File "check_cloudfront_function_live_stage_before_publish_rejected.py", line 41, in run
    assert code == "NoSuchFunctionExists", f"testing LIVE before publish returned {code!r}"
  AssertionError: testing LIVE before publish returned 'NotImplemented'

Preceding assertions in the same check PASSED, bounding the defect to TestFunction:
  - DescribeFunction(Name) -> Status "UNPUBLISHED", FunctionMetadata.Stage "DEVELOPMENT"
  - DescribeFunction(Name, Stage="LIVE") -> NoSuchFunctionExists / "The specified function does not exist."

Source of the 501 (handler, not a routing or model gap):
  services/service-com-amazonaws-cloudfront-cloudfront2020-05-31/src/ops/test_function.rs:3-6   `//! @op TestFunction` + `//! SCAFFOLDED, NOT IMPLEMENTED: this operation answers HTTP 501 NotImplemented`
  services/service-com-amazonaws-cloudfront-cloudfront2020-05-31/src/ops/test_function.rs:175    `Err(crate::scaffolded::Scaffolded::not_implemented("TestFunction"))`

Model (smithy_query, com-amazonaws-cloudfront-cloudfront2020-05-31.json): TestFunction http POST /2020-05-31/function/{Name}/test code 200; errors InvalidArgument(400), InvalidIfMatchVersion(400), NoSuchFunctionExists(404), TestFunctionFailed(500), UnsupportedOperation(400). 501/NotImplemented is not a modeled outcome.

## Steps to Reproduce
1. Point an AWS SDK/CLI client at the emulator's CloudFront endpoint.
2. CreateFunction(Name="<name>", FunctionConfig={Comment:"...", Runtime:"cloudfront-js-2.0"}, FunctionCode=<a viewer-request handler>) and keep the returned ETag. Do NOT call PublishFunction.
3. Confirm the pre-state: DescribeFunction(Name="<name>") returns Status=UNPUBLISHED / Stage=DEVELOPMENT, and DescribeFunction(Name="<name>", Stage="LIVE") is refused 404 NoSuchFunctionExists "The specified function does not exist." (both already correct today).
4. Call TestFunction(Name="<name>", IfMatch=<the ETag from step 2>, Stage="LIVE", EventObject=<a JSON viewer-request event>).
   Expected: HTTP 404, Code "NoSuchFunctionExists", Message "The specified function does not exist."
   Observed: HTTP 501, Code "NotImplemented".
5. Also call TestFunction(... Stage="DEVELOPMENT") with the same ETag.
   Expected: HTTP 200 with a TestResult payload. Observed: HTTP 501 "NotImplemented".
6. Replay the grounding check: run_behavioral_suite -> cloudfront.function-live-stage-before-publish-rejected.
