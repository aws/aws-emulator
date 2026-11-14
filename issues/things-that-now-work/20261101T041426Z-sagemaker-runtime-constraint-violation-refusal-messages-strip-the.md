Issue-Source: trace-sweep-divergence
Issue-Trace: sagemakerruntime-invoke-endpoint-response-stream-rejections
Issue-Step: task#2
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: sagemakerruntime-invoke-endpoint-response-stream-rejections
Issue-Shape-Id: com.amazonaws.sagemakerruntime#AmazonSageMakerRuntime
Issue-Key: sagemaker-runtime|InvokeEndpointWithResponseStream|4xx->4xx|$.Message
Issue-Commit: 4c59ac9c595fa3e7ec5eb8e44c5c46e964fce040

# sagemaker-runtime constraint-violation refusal messages strip the model @pattern anchors

> _Note: this issue was identified against the code base at commit `4c59ac9c595fa3e7ec5eb8e44c5c46e964fce040`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
InvokeEndpointWithResponseStream

## Description
One root cause (Cause 1 of 1). Fixture sagemakerruntime-invoke-endpoint-response-stream-rejections step task#2.

Related but NON-COVERING open issues (the wire replay reports this step as unowned, i.e. no open issue carries its replay key): 20261031T024432Z-sagemaker-runtime-validationerror-constraint-message-renders-the-member.md describes the same anchor-trimming defect but was filed from the BEHAVIORAL corpus (Issue-Corpus: simparity-behavioral-aws-v1) and carries no wire fixture/step linkage, so replaying this fixture is not gated by it; 20261029T203709Z-sagemaker-runtime-request-constraint-refusals-use-error-code.md owns the error-CODE divergence (ValidationException vs ValidationError) and the sensitive-CustomAttributes echo; 20261029T233857Z-sagemaker-runtime-drops-errorcode-message-from-refusal-bodies.md owns missing ErrorCode/Message in refusal bodies and the lookup-before-validation ordering. None of those covers this step: here the HTTP status, the error code and the validate-before-lookup ordering are all already correct, and only the rendered pattern text inside the message diverges.

Cause 1 of 1: the shared constraint-violation message formatter does not interpolate the violated member's @pattern verbatim — it emits the regex with its anchors trimmed (here the trailing '$' is dropped).

AWS: Smithy com.amazonaws.sagemakerruntime#InvokeEndpointWithResponseStream input member SessionId targets SessionIdHeader (httpHeader X-Amzn-SageMaker-Session-Id, @length min 0 max 256, @pattern "^[a-zA-Z0-9](-*[a-zA-Z0-9])*$"); the operation's modeled client error is ValidationError (errorKind client, httpError 400). Unlike InvokeEndpoint.SessionId (@pattern "^(NEW_SESSION)$|^[a-zA-Z0-9](-*[a-zA-Z0-9])*$"), the streaming operation's SessionId does NOT admit the NEW_SESSION sentinel, so SessionId=NEW_SESSION is a pattern violation on this operation. AWS refuses with HTTP 400, error code "ValidationError" and the standard constraint template. Fixed text of that template: "1 validation error detected: Value '", "' at '", "' failed to satisfy constraint: Member must satisfy regular expression pattern: ". Variable parts, all taken from the request and the model: <value> is the rejected value from the request, <memberName> is the wire member name (lowerCamel, here "sessionId"), and <pattern> is the member's @pattern copied VERBATIM from the Smithy model, anchors included. The requirement is that rule, not one recording's literal; for this member it renders as "1 validation error detected: Value 'NEW_SESSION' at 'sessionId' failed to satisfy constraint: Member must satisfy regular expression pattern: ^[a-zA-Z0-9](-*[a-zA-Z0-9])*$".

Emulator: status 400 and the error code are correct, and the refusal is correctly evaluated before the endpoint lookup, but the pattern slot is rendered without the trailing anchor: "1 validation error detected: Value 'NEW_SESSION' at 'sessionId' failed to satisfy constraint: Member must satisfy regular expression pattern: ^[a-zA-Z0-9](-*[a-zA-Z0-9])*" (no closing '$'). Hypothesis: the validator normalises/compiles the regex (stripping anchors because the matcher anchors the match itself) and then formats the message from the normalised regex instead of from the model's original pattern string.

Why: the pattern inside the message is the only part of the refusal that tells a client WHICH constraint it violated, and SDK users, negative tests and conformance suites compare it against the model's pattern. A caller or test written against AWS fails against the emulator even though the refusal is otherwise correct, and an operator is shown a regex that is not the service's constraint — an unanchored "[a-zA-Z0-9](-*[a-zA-Z0-9])*" matches any string merely CONTAINING such a run, a materially weaker rule than the anchored one.

Fix: the sagemaker-runtime request-validation layer's constraint-message formatter — it must interpolate the member's @pattern exactly as declared in the model (preserving any leading '^' and trailing '$') independently of whatever normalised form the matcher uses internally. The anchored matching behaviour itself is correct and must not change; this is a serialization/message-rendering fix, not a validation-semantics change.

Traces: sagemakerruntime-invoke-endpoint-response-stream-rejections#task#2 (the only diverging step in that fixture; the fixture's other steps, including the absent-endpoint ValidationError control, pass, isolating the divergence to the pattern rendering).

## Evidence
Wire replay of fixture sagemakerruntime-invoke-endpoint-response-stream-rejections, step task#2, verdict mismatch (affected: sagemakerruntime-invoke-endpoint-response-stream-rejections#task#2).

Expected status 400, actual status 400 (status and error code agree). Diverging body key $.Message:
  expected: "1 validation error detected: Value 'NEW_SESSION' at 'sessionId' failed to satisfy constraint: Member must satisfy regular expression pattern: ^[a-zA-Z0-9](-*[a-zA-Z0-9])*$"
  actual:   "1 validation error detected: Value 'NEW_SESSION' at 'sessionId' failed to satisfy constraint: Member must satisfy regular expression pattern: ^[a-zA-Z0-9](-*[a-zA-Z0-9])*"

Fixed text vs variable parts of the expected message: fixed text is "1 validation error detected: Value '" + "' at '" + "' failed to satisfy constraint: Member must satisfy regular expression pattern: "; the variable parts are the rejected value ('NEW_SESSION', from the request header X-Amzn-SageMaker-Session-Id), the wire member name ('sessionId'), and the pattern string, which must be the model's @pattern verbatim. Only the last variable part diverges: the emulator's rendering is missing its trailing '$' anchor.

Model evidence (smithy_query, model_path services/service-com-amazonaws-sagemakerruntime-amazonsagemakerruntime/smithy-model/com-amazonaws-sagemakerruntime-amazonsagemakerruntime.json):
  InvokeEndpointWithResponseStream.SessionId -> SessionIdHeader, httpHeader X-Amzn-SageMaker-Session-Id, @length 0..256, @pattern "^[a-zA-Z0-9](-*[a-zA-Z0-9])*$"
  InvokeEndpointWithResponseStream errors include ValidationError (client, httpError 400)
  Contrast: InvokeEndpoint.SessionId @pattern "^(NEW_SESSION)$|^[a-zA-Z0-9](-*[a-zA-Z0-9])*$" — the NEW_SESSION sentinel is operation-scoped, so it is a violation on the streaming operation.
  Other patterned members on the same operation that share the formatter: EndpointName, TargetVariant, TargetContainerHostname (@pattern "^[a-zA-Z0-9](-*[a-zA-Z0-9])*$"), ContentType/Accept/CustomAttributes (@pattern "^\\p{ASCII}*$"), InferenceComponentName, InferenceId, PrefixAwareId — each will render with its anchors trimmed by the same code path.

## Steps to Reproduce
1. Boot the emulator from this checkout and replay fixture sagemakerruntime-invoke-endpoint-response-stream-rejections, step task#2.
2. Equivalently by hand: call sagemaker-runtime InvokeEndpointWithResponseStream with EndpointName=<a well-formed endpoint name>, Body=b"x" and the header X-Amzn-SageMaker-Session-Id: NEW_SESSION (the sentinel is valid only on InvokeEndpoint, so it violates SessionIdHeader's @pattern on the streaming operation).
3. Observe HTTP 400 with error code ValidationError and Message "1 validation error detected: Value 'NEW_SESSION' at 'sessionId' failed to satisfy constraint: Member must satisfy regular expression pattern: ^[a-zA-Z0-9](-*[a-zA-Z0-9])*" — the trailing '$' of the model's @pattern is missing.
4. Compare against AWS, whose message ends with the model's pattern verbatim: "... pattern: ^[a-zA-Z0-9](-*[a-zA-Z0-9])*$".
5. Generalise: violate any other patterned member on this service (e.g. TargetVariant="bad_variant", or a non-ASCII Accept header) and confirm the rendered pattern is likewise missing its '^'/'$' anchors relative to the Smithy @pattern.
