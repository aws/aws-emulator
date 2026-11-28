Issue-Source: trace-sweep-divergence
Issue-Trace: resiliencehub-grouping-recommendation-task-refusals
Issue-Step: teardown#10
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: resiliencehub-grouping-recommendation-task-refusals
Issue-Shape-Id: com.amazonaws.resiliencehub#AwsResilienceHub
Issue-Key: resiliencehub|DescribeApp|4xx->4xx|$.__type
Issue-Commit: ff49842cf7656b5a010943ef9d0fd100550d134a

# resiliencehub DescribeApp 404 error document carries the wrong __type token form

> _Note: this issue was identified against the code base at commit `ff49842cf7656b5a010943ef9d0fd100550d134a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeApp

## Description
One cause, replayed from the wire corpus: the error-type discriminator in resiliencehub's 404 error document does not have the form the recording asserts.

Cause 1 of 1 - DescribeApp's ResourceNotFoundException body reports a `__type` token the recording does not accept (fixture resiliencehub-grouping-recommendation-task-refusals, step teardown#10).

AWS: DescribeApp (Smithy: com.amazonaws.resiliencehub#DescribeApp, protocol restJson1, POST /describe-app, success 200) models `appArn` (Arn, required) in and `app` (App, required) out, and declares the errors AccessDeniedException (403), InternalServerException (500), ResourceNotFoundException (404), ThrottlingException (429), ValidationException (400). A DescribeApp of an appArn that was never created or has been deleted is the ResourceNotFoundException case - Smithy: com.amazonaws.resiliencehub#ResourceNotFoundException, errorKind client, httpError 404, members message (String500, @length 1..500), resourceId, resourceType; doc "This exception occurs when the specified resource could not be found." The HTTP status (404) and the error's identity (ResourceNotFoundException) are the requirements, and so is the FORM of the token the restJson1 error document uses to carry that identity: the recording asserts `$.__type` structurally (the replay reports the expectation as '<shaped-not-asserted>', i.e. the recorded token's shape is the requirement and its literal text is not), and the emulator's token fails that assertion. Hypothesis: AWS carries the discriminator as the error's fully-qualified Smithy shape id - `com.amazonaws.resiliencehub#ResourceNotFoundException` - rather than the bare shape name, which is the usual AWS JSON-family form and the only form that differs structurally from what the emulator emits here; the alternative is that AWS's restJson1 document omits `__type` entirely and conveys the identity in the `x-amzn-errortype` header. The fixture's own recorded 404 body for teardown#10 states which, so read it before changing the serializer. The message is not what diverges here: for this error class the fixed text is AWS's not-found wording and its variable part is the appArn taken from the request, and that member passed.

Emulator: the replayed DescribeApp of a missing application answers the right status, 404, and the right error class, but the body's discriminator is the bare name: `"__type": "ResourceNotFoundException"`, reported as `$.__type: value_mismatch (expected '<shaped-not-asserted>', got 'ResourceNotFoundException')`. Nothing else in the response diverges - status, message, resourceId and resourceType all match.

Why: `__type` is how a restJson1 client decides WHICH modeled error it was handed. A client that reads the discriminator out of the body and matches it against the error shape ids it knows - a strict/raw HTTP client, a cross-language or hand-rolled client, a proxy or recorder that keys on the qualified shape id, or any assertion that the token has the service-qualified form - fails to resolve the token to com.amazonaws.resiliencehub#ResourceNotFoundException and surfaces the 404 as an unknown or unmodeled error. A teardown/cleanup loop that distinguishes "already gone" (ResourceNotFoundException, benign) from any other 4xx then cannot make that distinction and reports a failed cleanup for an application that was correctly deleted.

Fix: the service's restJson1 error serialization - the error channel for this operation (services/service-com-amazonaws-resiliencehub-awsresiliencehub/src/ops/describe_app.rs, `DescribeAppErrorModeled::ResourceNotFoundException` built by `app_not_found`) is constructing the right error; what needs changing is the shared restJson1 error serializer that writes the `__type` member, so that the discriminator it emits has the form AWS uses for this service's errors (and the `x-amzn-errortype` header agrees with it). The fix belongs in the serializer, not in the handler, because it applies to every modeled error of every operation in this service, not just this 404.

Traces: resiliencehub-grouping-recommendation-task-refusals#teardown#10.

## Evidence
Replay of trace set simparity-wire-aws-v1 against the emulator built from this checkout (emulator_up true, /__admin/health 200 and the data listener accepting; 9 fixtures replayed, 99 steps, 82 passed, score 82.8%).

Cause 1 - service=resiliencehub, operation DescribeApp, verdict mismatch, expected status 404, actual status 404:
  divergence_key: service=resiliencehub|DescribeApp|status=404->404|keys=$.__type
  detail: "$.__type: value_mismatch (expected '<shaped-not-asserted>', got 'ResourceNotFoundException')"
  fixture_id: resiliencehub-grouping-recommendation-task-refusals, step_key: teardown#10 (affected: resiliencehub-grouping-recommendation-task-refusals#teardown#10, affected_count 1)
  stub_path: "" (no unimplemented operation blocks it), owned_by: [] (no open issue carries its replay key)

Reading: the status and the error class are right; the single diverging member is the error document's `__type` discriminator, whose recorded value is asserted by shape rather than literally, and the bare name the emulator emits does not satisfy that shape. The fixed part of this requirement is the error identity ResourceNotFoundException and the 404 status; the variable parts of the accompanying message are the appArn from the request and the resource type from state, and those did not diverge.

Model references used: smithy_query(query="operation", operation="DescribeApp") and smithy_query(query="shape", shape="ResourceNotFoundException") on services/service-com-amazonaws-resiliencehub-awsresiliencehub/smithy-model/com-amazonaws-resiliencehub-awsresiliencehub.json; service_info reports sdkId resiliencehub, protocol restJson1, version 2020-04-30.

Related but NOT covering this: open issue 20261122T154253Z-resiliencehub-app-plane-responses-diverge-updateapp-omits-the.md owns the UpdateApp `$.app.tags` and ResolveAppVersionResources `$.resolutionId` clusters (both reported owned_by that file in this replay and therefore skipped here). It says nothing about error-document serialization and does not carry this step's replay key.

## Steps to Reproduce
1. git clone the project and boot the emulator built from that checkout.
2. Replay the wire corpus of trace set simparity-wire-aws-v1.
3. Replay fixture resiliencehub-grouping-recommendation-task-refusals and stop at step teardown#10 (DescribeApp). Equivalent by hand: POST /describe-app with an `appArn` that is well-formed for this service but names no existing application (e.g. an app the same flow already deleted).
4. Inspect the raw 404 response body. It is {"__type": "ResourceNotFoundException", "message": ..., "resourceId": <the appArn from the request>, "resourceType": "App"}; the `__type` token is the bare shape name and does not have the form the fixture's recorded 404 asserts for the discriminator. Compare it with the recorded body for teardown#10 and with the `x-amzn-errortype` header the same response sets.
