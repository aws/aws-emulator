Issue-Source: trace-sweep-divergence
Issue-Trace: route53-recovery-control-config-routing-control-name-and-arn-validation
Issue-Step: task#8
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: route53-recovery-control-config-routing-control-name-and-arn-validation
Issue-Shape-Id: com.amazonaws.route53recoverycontrolconfig#Route53RecoveryControlConfig
Issue-Key: route53-recovery-control-config|UpdateRoutingControl|4xx->2xx|$.message
Issue-Key: route53-recovery-control-config|UpdateRoutingControl|4xx->2xx|$headers.x-amzn-errortype
Issue-Key: route53-recovery-control-config|UpdateRoutingControl|4xx->2xx|status
Issue-Key: route53-recovery-control-config|UpdateRoutingControl|4xx->4xx|$.message
Issue-Key: route53-recovery-control-config|UpdateRoutingControl|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: route53-recovery-control-config|UpdateRoutingControl|4xx->4xx|status
Issue-Key: route53-recovery-control-config|DescribeRoutingControl|2xx->2xx|$.RoutingControl.Name
Issue-Key: route53-recovery-control-config|DescribeRoutingControl|2xx->2xx|$.RoutingControl.Status
Issue-Key: route53-recovery-control-config|DescribeRoutingControl|4xx->4xx|$.message
Issue-Key: route53-recovery-control-config|DescribeRoutingControl|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: route53-recovery-control-config|DescribeRoutingControl|4xx->4xx|status
Issue-Commit: 760ea4c235b0ff162849feaaf99865499e566e9b

# Routing control validation and deployment status diverge from AWS on route53-recovery-control-config

> _Note: this issue was identified against the code base at commit `760ea4c235b0ff162849feaaf99865499e566e9b`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
UpdateRoutingControl

## Description
Four related causes in the route53-recovery-control-config routing-control surface: the emulator does not enforce AWS's request-body/ARN validation on UpdateRoutingControl and DescribeRoutingControl, and it never advances a routing control's deployment Status. All four causes must be closed before this issue is submitted.

Cause 1 of 4 - UpdateRoutingControl accepts a routing control name AWS rejects (fixture route53-recovery-control-config-routing-control-name-and-arn-validation, step task#8).
AWS: RoutingControlName is validated server-side against a stricter character class than the modelled pattern. Smithy: UpdateRoutingControl.RoutingControlName -> __stringMin1Max64PatternS, @length min 1 max 64, @pattern "^\\S+$"; modelled error ValidationException (httpError 400, member Message with jsonName "message"). The modelled pattern alone admits a name containing ';', so the enforced rule is recorded behavior only: AWS answers HTTP 400 with x-amzn-errortype ValidationException and the message 'Invalid request body : [ECMA 262 regex "^((?![;'\\s<>&"])[\\u0021-\\u007E])+$" does not match input string "<RoutingControlName>"]'. Fixed text of that message is 'Invalid request body : [ECMA 262 regex "' + the regex literal + '" does not match input string "' + '"]'; the only variable part is the rejected name value taken verbatim from the request (the recording's "cell;east" is one such value, not part of the template). The rule: a routing control name must consist only of printable ASCII (U+0021..U+007E) and must not contain ';', '\'', whitespace, '<', '>', '&' or '"'.
Emulator: answers HTTP 200 with a successful UpdateRoutingControl response. No message body field and no x-amzn-errortype header are produced (replay: "status: expected 400, got 200"; "$.message: missing"; "$headers.x-amzn-errortype: missing"), and the invalid name is persisted.
Why: a client that relies on AWS rejecting illegal names gets a silent success, so names AWS would never store enter the emulator's state and any client-side error handling for ValidationException is never exercised. Downstream reads then return a resource shape AWS cannot produce.
Fix: input validation for UpdateRoutingControl - add the name character-class check to the operation's request validation and emit ValidationException (400) with the message above, before the handler mutates state.
Traces: route53-recovery-control-config-routing-control-name-and-arn-validation#task#8.

Cause 2 of 4 - UpdateRoutingControl answers 404 ResourceNotFoundException for a wrong-type ARN instead of 400 ValidationException (fixture route53-recovery-control-config-routing-control-name-and-arn-validation, step task#9).
AWS: RoutingControlArn must be a routing control ARN. Smithy: UpdateRoutingControl.RoutingControlArn -> __stringMin1Max256PatternAZaZ09, required, @length min 1 max 256, @pattern "^[A-Za-z0-9:\\/_-]*$"; modelled errors include ValidationException (400) and ResourceNotFoundException (404). A control-panel ARN satisfies the modelled pattern, so the distinction is recorded behavior only: when the supplied ARN is syntactically an ARN of the wrong resource type, AWS answers HTTP 400 ValidationException with message 'Malformed Arn <arn>' - fixed text 'Malformed Arn ' followed by the offending ARN copied verbatim from the request (the recording's arn:aws:route53-recovery-control::000000000000:controlpanel/<uuid> is one such value). It does NOT fall through to a lookup miss.
Emulator: answers HTTP 404 with x-amzn-errortype ResourceNotFoundException and message 'No record corresponding to routing control Arn(<arn>)' (replay: "status: expected 400, got 404"). It treats a malformed/wrong-type ARN as a missing resource.
Why: the two outcomes are differently retriable and differently classified by SDK error handling - a client sees a transient-looking not-found for what is a permanent, non-retriable client input error, and never learns the ARN itself is wrong.
Fix: UpdateRoutingControl request validation - classify the ARN's resource type before the state lookup and raise ValidationException (400, 'Malformed Arn <arn>') when it is not a routing control ARN.
Traces: route53-recovery-control-config-routing-control-name-and-arn-validation#task#9.

Cause 3 of 4 - DescribeRoutingControl returns a routing control whose Status never leaves PENDING (and whose Name reflects the update AWS rejected) (fixture route53-recovery-control-config-routing-control-name-and-arn-validation, step task#10).
AWS: Smithy: RoutingControl.Status -> Status enum, values PENDING | DEPLOYED | PENDING_DELETION, documented as 'PENDING: Amazon Route 53 Application Recovery Controller is creating the resource. DEPLOYED: The resource is deployed and ready to use.' A routing control that has finished being created is reported DEPLOYED, so a Describe after the create has settled returns Status DEPLOYED; RoutingControl.Name is the name last successfully set (Smithy: RoutingControl.Name, @length 1..64, @pattern "^\\S+$").
Emulator: returns Status PENDING indefinitely and returns the name from the update that AWS rejects (replay: "$.RoutingControl.Status: value_mismatch (expected 'DEPLOYED', got 'PENDING')"; "$.RoutingControl.Name: value_mismatch" - the recorded expected name is a generated per-run value and is not itself a requirement, but the fact that the rejected name was stored is). The Name half is a consequence of Cause 1; the Status half is independent - no resource ever reaches DEPLOYED.
Why: clients and waiters poll DescribeRoutingControl until Status is DEPLOYED before using the control. Against the emulator that poll never terminates, so any create-then-use workflow hangs or times out.
Fix: routing control state - have the create/update path settle a routing control to Status DEPLOYED once it exists (rather than leaving the stored status at PENDING), and make the DescribeRoutingControl handler serialize that settled status.
Traces: route53-recovery-control-config-routing-control-name-and-arn-validation#task#10.

Cause 4 of 4 - DescribeRoutingControl answers 404 ResourceNotFoundException for a wrong-type ARN instead of 400 ValidationException (fixture route53-recovery-control-config-routing-control-name-and-arn-validation, steps task#14 and task#15).
AWS: Smithy: DescribeRoutingControl.RoutingControlArn -> __string, required, bound as an @httpLabel on GET /routingcontrol/{RoutingControlArn}; modelled errors include ValidationException (400) and ResourceNotFoundException (404). Recorded behavior: given an ARN of the wrong resource type (a control panel ARN) AWS answers HTTP 400 ValidationException with message 'Malformed Arn <arn>' - fixed text 'Malformed Arn ' plus the offending ARN from the request path.
Emulator: answers HTTP 404 with x-amzn-errortype ResourceNotFoundException and message 'No record corresponding to routing control Arn(<arn>)' (replay: "status: expected 400, got 404").
Why: same misclassification as Cause 2 on the read path - a permanent client input error is reported as a missing resource, so clients retry or branch into not-found recovery instead of surfacing a bad ARN.
Fix: DescribeRoutingControl request validation - apply the same ARN resource-type check ahead of the state lookup; it should share one validator with UpdateRoutingControl (Cause 2).
Traces: route53-recovery-control-config-routing-control-name-and-arn-validation#task#14, route53-recovery-control-config-routing-control-name-and-arn-validation#task#15.

Related but NOT covered here: the open issue 20261121T124006Z-route53-recovery-control-config-createsafetyrule-answers-400-validationexception-instead-of-the-401.md already owns the CreateSafetyRule 401/400 divergence (fixture route53-recovery-control-config-validation-error-paths#task#4).

## Evidence
Replay of trace set simparity-wire-aws-v1 (48 steps, 42 passed, 6 mismatched, score 87.5%).

Cause 1 - route53-recovery-control-config-routing-control-name-and-arn-validation#task#8, UpdateRoutingControl:
  expected: HTTP 400, $.message = 'Invalid request body : [ECMA 262 regex "^((?![;'\\s<>&"])[\\u0021-\\u007E])+$" does not match input string "cell;east"]', $headers.x-amzn-errortype present (ValidationException)
  actual:   HTTP 200, $.message missing, $headers.x-amzn-errortype missing

Cause 2 - ...#task#9, UpdateRoutingControl:
  expected: HTTP 400, $.message = 'Malformed Arn arn:aws:route53-recovery-control::000000000000:controlpanel/2d2e6183-ff1d-9e36-c5d9-042e0f5214ed'
  actual:   HTTP 404, $.message = 'No record corresponding to routing control Arn(arn:aws:route53-recovery-control::000000000000:controlpanel/2d2e6183-ff1d-9e36-c5d9-042e0f5214ed)', $headers.x-amzn-errortype = 'ResourceNotFoundException'

Cause 3 - ...#task#10, DescribeRoutingControl (HTTP 200 both sides):
  expected: $.RoutingControl.Status = 'DEPLOYED', $.RoutingControl.Name = the name last successfully set (recorded run: 'sim2real-probe-arc-nvrc-85e63f2e')
  actual:   $.RoutingControl.Status = 'PENDING', $.RoutingControl.Name = 'cell;east' (the name the task#8 update should have been refused for)

Cause 4 - ...#task#14 (cluster also covers ...#task#15), DescribeRoutingControl:
  expected: HTTP 400, $.message = 'Malformed Arn arn:aws:route53-recovery-control::000000000000:controlpanel/2d2e6183-ff1d-9e36-c5d9-042e0f5214ed'
  actual:   HTTP 404, $.message = 'No record corresponding to routing control Arn(...)', $headers.x-amzn-errortype = 'ResourceNotFoundException'

Model citations: Smithy UpdateRoutingControl (PUT /routingcontrol) RoutingControlName @length 1..64 @pattern ^\\S+$, RoutingControlArn required @length 1..256 @pattern ^[A-Za-z0-9:\\/_-]*$, errors include ValidationException(400)/ResourceNotFoundException(404); DescribeRoutingControl (GET /routingcontrol/{RoutingControlArn}) same error set; RoutingControl.Status -> Status enum [PENDING, DEPLOYED, PENDING_DELETION]; ValidationException httpError 400 with required Message (jsonName 'message').

## Steps to Reproduce
1. Boot the emulator from this checkout and replay fixture route53-recovery-control-config-routing-control-name-and-arn-validation from trace set simparity-wire-aws-v1.
2. Cause 1 - step task#8: PUT /routingcontrol (UpdateRoutingControl) with a valid RoutingControlArn and RoutingControlName containing ';' (e.g. 'cell;east'). Expect HTTP 400 ValidationException 'Invalid request body : [ECMA 262 regex ... does not match input string "<name>"]'; observe HTTP 200 and the name stored.
3. Cause 2 - step task#9: PUT /routingcontrol (UpdateRoutingControl) passing a CONTROL PANEL ARN as RoutingControlArn. Expect HTTP 400 ValidationException 'Malformed Arn <arn>'; observe HTTP 404 ResourceNotFoundException 'No record corresponding to routing control Arn(<arn>)'.
4. Cause 3 - step task#10: GET /routingcontrol/{arn} (DescribeRoutingControl) for the routing control created earlier in the fixture. Expect $.RoutingControl.Status 'DEPLOYED' and the last successfully set Name; observe 'PENDING' and the name from the rejected update.
5. Cause 4 - steps task#14 and task#15: GET /routingcontrol/{arn} (DescribeRoutingControl) with a CONTROL PANEL ARN in the path label. Expect HTTP 400 ValidationException 'Malformed Arn <arn>'; observe HTTP 404 ResourceNotFoundException.
