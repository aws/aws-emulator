Issue-Source: trace-sweep-divergence
Issue-Trace: cognito-idp-osc-dashboards-callback-repoint-to-alb
Issue-Step: task#4
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: cognito-idp-osc-dashboards-callback-repoint-to-alb
Issue-Shape-Id: com.amazonaws.cognitoidentityprovider#AWSCognitoIdentityProviderService
Issue-Key: cognito-idp|UpdateUserPoolClient|4xx->2xx|$.__type
Issue-Key: cognito-idp|UpdateUserPoolClient|4xx->2xx|$.message
Issue-Key: cognito-idp|UpdateUserPoolClient|4xx->2xx|$headers.x-amzn-errortype
Issue-Key: cognito-idp|UpdateUserPoolClient|4xx->2xx|status
Issue-Commit: e893cd19d3c9bb8dc2178e48ce28ce64f933efef

# UpdateUserPoolClient accepts a scheme-less CallbackURL instead of refusing it with 400 InvalidParameterException

> _Note: this issue was identified against the code base at commit `e893cd19d3c9bb8dc2178e48ce28ce64f933efef`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
UpdateUserPoolClient

## Description
Cause 1 of 1: UpdateUserPoolClient performs no absolute-URI validation on the redirect URL members (CallbackURLs, and by the same rule LogoutURLs / DefaultRedirectURI), so a scheme-less URL is accepted and written into the app client instead of being refused.

AWS: Amazon Cognito validates every entry of CallbackURLs before mutating the client. Smithy: UpdateUserPoolClient.CallbackURLs -> CallbackURLsListType (@length min 0 max 100); modeled error InvalidParameterException (client, httpError 400). The member's own documentation states a redirect URI "must ... Be an absolute URI", must "Not include a fragment component", and that "Amazon Cognito requires HTTPS over HTTP except for http://localhost". A value with no scheme is therefore not an absolute URI and the call is refused with HTTP 400, JSON body __type "InvalidParameterException", header x-amzn-errortype "InvalidParameterException", and message of the form "<offending-url> must contain a scheme." — fixed text is " must contain a scheme."; the variable part is the rejected redirect URL exactly as supplied in the request (in the recording it was the scheme-less ALB host-plus-path value sent for CallbackURLs). The refusal is atomic: the client is left unchanged, so the later DescribeUserPoolClient still shows the previously registered CallbackURLs/LogoutURLs.

Emulator: the request succeeds with HTTP 200 and an empty error-free UserPoolClient payload — status expected 400, got 200; $.__type missing (expected 'InvalidParameterException'); $.message missing (expected the "... must contain a scheme." text); $headers.x-amzn-errortype missing. The scheme-less value is then persisted, which is why the follow-on read of the same client (step task#6 of the same fixture) serves the scheme-less CallbackURL and has lost LogoutURLs.

Why: a client that relies on AWS's validation never learns its redirect-URI configuration is invalid. Against AWS the bad update is rejected and the working hosted-UI configuration survives; against the emulator the update silently lands, so the app client is left with a redirect URI managed login can never match, and any test asserting the refusal (or asserting that the prior configuration is intact) passes where it should fail. Error-handling code paths for InvalidParameterException are never exercised.

Fix: input validation for UpdateUserPoolClient (the redirect-URL validation shared with CreateUserPoolClient) in the cognito-idp service crate — validate each CallbackURLs/LogoutURLs entry and DefaultRedirectURI as an absolute URI (scheme required, HTTPS except http://localhost, no fragment) and raise InvalidParameterException (400) with the "<url> must contain a scheme." message before any state mutation, so the stored client is untouched on refusal.

Traces: cognito-idp-osc-dashboards-callback-repoint-to-alb#task#4 (representative) and cognito-idp-osc-dashboards-callback-repoint-to-alb#task#5 — the same cluster, both UpdateUserPoolClient calls carrying a scheme-less redirect URL.

## Evidence
Fixture cognito-idp-osc-dashboards-callback-repoint-to-alb, step task#4 (UpdateUserPoolClient), cluster service=cognito-idp|UpdateUserPoolClient|status=400->200.

Expected (recorded AWS response): HTTP 400; body {"__type": "InvalidParameterException", "message": "<callback-url> must contain a scheme."}; header x-amzn-errortype present (InvalidParameterException).
Actual (emulator): HTTP 200, no __type, no message, no x-amzn-errortype header.

Replay detail verbatim:
  status: expected 400, got 200
  $.__type: missing (expected 'InvalidParameterException', got None)
  $.message: missing (expected 'internal-sim2real-probe-osc-24c60c4f-1234567890.us-west-2.elb.amazonaws.com/_dashboards/app/home must contain a scheme.', got None)
  $headers.x-amzn-errortype: missing (expected '<shaped-not-asserted>', got None)

(The host/id in the recorded message is a per-run value; the requirement is the fixed text " must contain a scheme." prefixed by the request's own rejected URL.)

Also affected in this cluster: cognito-idp-osc-dashboards-callback-repoint-to-alb#task#5. Downstream, the accepted-but-invalid value shows up in step task#6 (DescribeUserPoolClient serves the scheme-less CallbackURL and omits LogoutURLs); that read-path cluster is already owned by 20261102T111936Z-cognito-idp-response-payloads-diverge-read-paths-omit.md, which does not cover this missing 400 gate.

## Steps to Reproduce
1. Replay fixture cognito-idp-osc-dashboards-callback-repoint-to-alb against the emulator (replay_wire_corpus).
2. At step task#4 the fixture calls UpdateUserPoolClient for the pool/app client created earlier in the fixture, supplying CallbackURLs whose single entry has no URI scheme (a bare host-plus-path such as <alb-dns-name>/_dashboards/app/home) together with the matching LogoutURLs.
3. Observe the emulator answers HTTP 200 with a UserPoolClient payload; AWS answers HTTP 400 with __type InvalidParameterException and message "<that url> must contain a scheme." and leaves the client unchanged.
4. Step task#5 repeats the same scheme-less update and diverges identically; step task#6's DescribeUserPoolClient then shows the invalid value was persisted.
