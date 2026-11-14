Issue-Source: trace-sweep-divergence
Issue-Trace: connectparticipant-participant-session-lifecycle
Issue-Step: task#11
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: connectparticipant-participant-session-lifecycle
Issue-Shape-Id: com.amazonaws.connectparticipant#AmazonConnectParticipantServiceLambda
Issue-Key: connectparticipant|DescribeView|4xx->4xx|$.message
Issue-Commit: 22d0a813c75ce3e8d355f134bff43e3d58146669

# DescribeView 403 AccessDeniedException body omits the required message member

> _Note: this issue was identified against the code base at commit `22d0a813c75ce3e8d355f134bff43e3d58146669`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeView

## Description
One net-new cause on connectparticipant, found by replaying wire trace set simparity-wire-aws-v1 against the emulator built from this checkout. Smithy model consulted: services/service-com-amazonaws-connectparticipant-amazonconnectparticipantservicelambda/smithy-model/com-amazonaws-connectparticipant-amazonconnectparticipantservicelambda.json (sdkId ConnectParticipant, restJson1).

Cause 1 of 1: DescribeView's denial is served with the right status and error class but with NO message member in its body.

AWS: Smithy DescribeView (GET /participant/views/{ViewToken}, ConnectionToken bound to the X-Amz-Bearer header) declares AccessDeniedException (errorKind client, httpError 403); AccessDeniedException models member Message as REQUIRED, documented "You do not have sufficient access to perform this action". For this service's denial class the recorded 403 body is {"message": "Access denied"} — that text is entirely FIXED for the class: it contains no view token, no connection token, no contact id and no other value taken from the request or from emulator state, because the refusal deliberately tells the caller nothing about which credential failed. A restJson1 error response also carries the error-shape name in the x-amzn-errortype header, and that header IS present here.

Emulator: HTTP 403 is correct and x-amzn-errortype is present, but the replayed response body carries no message member at all ($.message missing, expected 'Access denied'). The handler does construct the modeled error with the right wording — services/.../src/ops/describe_view.rs builds AccessDeniedException { message: crate::state::ACCESS_DENIED_MESSAGE } for both refusal paths (an unknown/expired/disconnected connection token, and a view token naming no view of that participant's own chat), and services/.../src/state.rs defines ACCESS_DENIED_MESSAGE = "Access denied" — so the correct text is produced in the handler and then lost on the way to the wire. Hypothesis: the restJson1 error serializer for modeled exceptions writes the member under the modeled name 'Message' (or drops members of error structures entirely) while AWS wires it as lowercase 'message' for this service's exceptions; note that the framework's own request-validation 400 on this same service DOES surface a lowercase $.message, so the defect is specific to the serializer path taken by handler-returned modeled errors.

Why: every AWS SDK reads an error's reason out of that member, so a client calling DescribeView with a stale connection token or a view token it does not own receives an AccessDeniedException whose message is empty. Callers cannot log, classify, display or assert on the reason; retry/branch logic keyed on the message text never matches, and the response violates the model's own requiredness for AccessDeniedException$Message.

Fix: the restJson1 error serializer for modeled exceptions in this service's response path (member name/wire casing for error structure members), not the DescribeView handler's wording — the handler already carries AWS's exact fixed text.

Traces: connectparticipant-participant-session-lifecycle#task#11 (DescribeView, expected 403 / actual 403, $.message missing).

Related open issue: 20261109T180950Z-connectparticipant-error-paths-miss-accessdenied-and-aws-message.md describes the same serializer symptom for SendMessage as its Cause 3, but it carries NO replay key for this DescribeView step, so this divergence is filed here with its own key. Fixing the serializer should close both; this step must be re-replayed to confirm.

## Evidence
Replay of trace set simparity-wire-aws-v1 (emulator up; /__admin/health answered 200 and the data listener accepts): 26 steps total, 18 passed, 8 mismatched, 6 divergence clusters, score 69.2%.

Cluster (verdict mismatch), fixture connectparticipant-participant-session-lifecycle, step task#11, service connectparticipant, operation DescribeView, expected status 403 / actual status 403:
  "$.message: missing (expected 'Access denied', got None)"

Expected: HTTP 403, x-amzn-errortype naming AccessDeniedException, body member message = 'Access denied' (wholly fixed text for this denial class; no request- or state-derived parts).
Actual: HTTP 403 with the errortype header but a body carrying no message member.

Model: DescribeView errors = AccessDeniedException(403), InternalServerException(500), ResourceNotFoundException(404), ThrottlingException(429), ValidationException(400); AccessDeniedException.Message required.
Code: services/service-com-amazonaws-connectparticipant-amazonconnectparticipantservicelambda/src/ops/describe_view.rs lines 172-186 (the shared `denied()` closure returning AccessDeniedException { message: ACCESS_DENIED_MESSAGE } from both the connection-token and view-token arms); services/service-com-amazonaws-connectparticipant-amazonconnectparticipantservicelambda/src/state.rs line 80 (ACCESS_DENIED_MESSAGE = "Access denied").

## Steps to Reproduce
1. Boot the emulator from this checkout and point a ConnectParticipant client (or raw HTTP) at its data listener.
2. Obtain a ConnectionToken via CreateParticipantConnection on a chat session.
3. Call DescribeView with that ConnectionToken in the X-Amz-Bearer header and a ViewToken this emulator holds no view for (any unknown token, or one minted for another contact) — alternatively use a connection token that was never issued.
4. Inspect the RAW HTTP response: the status is 403 and x-amzn-errortype names AccessDeniedException, but the JSON body has no lowercase `message` member. AWS returns {"message": "Access denied"}.
5. Re-replay fixture step connectparticipant-participant-session-lifecycle#task#11 to confirm a fix; it must pass with $.message == 'Access denied'.
