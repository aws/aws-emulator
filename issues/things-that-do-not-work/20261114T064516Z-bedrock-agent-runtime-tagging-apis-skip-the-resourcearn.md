Issue-Source: trace-sweep-divergence
Issue-Trace: bedrock-agent-runtime-tag-operations-invalid-resource-arn
Issue-Step: task#2
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: bedrock-agent-runtime-tag-operations-invalid-resource-arn
Issue-Shape-Id: com.amazonaws.bedrockagentruntime#AmazonBedrockAgentRunTimeService
Issue-Key: bedrock-agent-runtime|TagResource|4xx->4xx|$.message
Issue-Key: bedrock-agent-runtime|TagResource|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: bedrock-agent-runtime|TagResource|4xx->4xx|status
Issue-Key: bedrock-agent-runtime|UntagResource|4xx->4xx|$.message
Issue-Key: bedrock-agent-runtime|UntagResource|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: bedrock-agent-runtime|UntagResource|4xx->4xx|status
Issue-Key: bedrock-agent-runtime|ListTagsForResource|4xx->4xx|$.message
Issue-Key: bedrock-agent-runtime|ListTagsForResource|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: bedrock-agent-runtime|ListTagsForResource|4xx->4xx|status
Issue-Commit: 2ed76ae7be9a75788628a2bcb5631d9ad9216ae7

# bedrock-agent-runtime tagging APIs skip the resourceArn pattern validation, answering ResourceNotFoundException instead of ValidationException

> _Note: this issue was identified against the code base at commit `2ed76ae7be9a75788628a2bcb5631d9ad9216ae7`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
TagResource

## Description
All three of this service's resource-tagging operations bind `resourceArn` as an @httpLabel whose target shape TaggableResourcesArn carries a @pattern (and @length 20..1011) restricting the ARN to this service's own taggable resource types. AWS enforces that constraint as INPUT VALIDATION, before any resource lookup: an ARN of the wrong resource type never reaches the store, it is refused by the protocol-level validator. The emulator has no such gate on any of the three operations — it parses the ARN, infers a resource type from it, looks that type up, misses, and reports the miss. The result is the wrong HTTP status class (404 instead of 400), the wrong error code (ResourceNotFoundException instead of ValidationException), the wrong errortype namespace, and an emulator-authored message in place of the standard Coral constraint-violation message. One missing validation gate, three operations, so three causes.

The required message for this error class is the standard Coral validation text, whose FIXED part is "<N> validation error detected: Value '<value>' at '<memberName>' failed to satisfy constraint: Member must satisfy regular expression pattern: <pattern>". Its VARIABLE parts are: N (the count of violations in the request), <value> (the rejected resourceArn taken verbatim from the request URI label), <memberName> (the input member, here always the literal `resourceArn`), and <pattern> (the service's regular expression for TaggableResourcesArn). Nothing in that message comes from emulator state, and the recorded ARN value is only the fixture's filler for <value>. The errortype header for this class is "ValidationException:http://internal.amazon.com/coral/com.amazon.coral.validate/" — the Coral validate namespace, not the service's own runtime namespace, because the refusal comes from the protocol validator rather than from service code.

Cause 1 of 3: TagResource accepts an out-of-pattern resourceArn and refuses it as a missing resource.
AWS: Smithy: TagResource.resourceArn -> TaggableResourcesArn, @required, @httpLabel, @length min 20 max 1011, @pattern "(^arn:aws(-[^:]+)?:bedrock:[a-zA-Z0-9-]+:[0-9]{12}:(session)/[a-f0-9]{8}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{12}$)"; modeled errors include ValidationException (httpError 400) and ResourceNotFoundException (httpError 404). An ARN naming a non-taggable resource type (e.g. an `agent/...` ARN) violates the pattern, so AWS answers 400 ValidationException with x-amzn-errortype "ValidationException:http://internal.amazon.com/coral/com.amazon.coral.validate/" and the constraint message above, quoting the submitted ARN at 'resourceArn' and echoing the service's live pattern (the recording shows the deployed pattern as the session|advanced-optimize-prompt-job alternation, i.e. the on-disk model's pattern is a narrower snapshot of the same rule; the requirement is that the message echo the pattern the service actually enforces).
Emulator: answers 404 with x-amzn-errortype "ResourceNotFoundException" and the message "Couldn't perform TagResource operation. Failed to retrieve resource type Agent because it doesn't exist. Check the resource ID and retry your request." — it has derived a resource type from the ARN and treated the request as a lookup miss.
Why: a client that validates its ARNs by catching ValidationException sees a retryable/absent-resource signal instead of a malformed-input signal. SDK error-type dispatch selects the wrong exception class entirely, so error-handling branches keyed on ValidationException never run, and a 404 invites "create it then retry" logic for an ARN that can never be tagged by this service.
Fix: TagResource's input validation — apply the TaggableResourcesArn @pattern/@length constraints to the resourceArn httpLabel in the operation's request-validation layer, ahead of any state lookup, and emit the Coral validation error (400, ValidationException, coral.validate errortype namespace) built from the member name, the submitted value and the enforced pattern.
Traces: bedrock-agent-runtime-tag-operations-invalid-resource-arn#task#2.

Cause 2 of 3: UntagResource accepts an out-of-pattern resourceArn and refuses it as a missing resource.
AWS: Smithy: UntagResource.resourceArn -> TaggableResourcesArn, @required, @httpLabel, same @length 20..1011 and same @pattern; errors include ValidationException (400) and ResourceNotFoundException (404); DELETE /tags/{resourceArn}. The pattern violation is refused with 400 ValidationException, errortype "ValidationException:http://internal.amazon.com/coral/com.amazon.coral.validate/", and the same constraint message with 'resourceArn' as the member and the submitted ARN as the value.
Emulator: answers 404 with x-amzn-errortype "ResourceNotFoundException" and "Couldn't perform UntagResource operation. Failed to retrieve resource type Agent because it doesn't exist. Check the resource ID and retry your request."
Why: same inversion as cause 1, and worse for an idempotent op: UntagResource is @idempotent, so a caller reasonably treats 404 as "already untagged / nothing to do" and proceeds, silently swallowing a malformed-ARN bug instead of surfacing it.
Fix: UntagResource's input validation — same resourceArn constraint gate ahead of the lookup, in that operation's request-validation layer.
Traces: bedrock-agent-runtime-tag-operations-invalid-resource-arn#task#3.

Cause 3 of 3: ListTagsForResource accepts an out-of-pattern resourceArn and refuses it as a missing resource.
AWS: Smithy: ListTagsForResource.resourceArn -> TaggableResourcesArn, @required, @httpLabel, same @length 20..1011 and same @pattern; errors include ValidationException (400) and ResourceNotFoundException (404); GET /tags/{resourceArn}, @readonly. The pattern violation is refused with 400 ValidationException, errortype "ValidationException:http://internal.amazon.com/coral/com.amazon.coral.validate/", and the same constraint message.
Emulator: answers 404 with x-amzn-errortype "ResourceNotFoundException" and "Couldn't perform ListTagsForResource operation. Failed to retrieve resource type ListTagsForResource because it doesn't exist. Check the resource ID and retry your request." (recorded actual names the resource type as Agent in the same shape of message) — again a lookup miss rather than a constraint violation.
Why: same inversion as cause 1; a read path that reports a malformed ARN as an absent resource makes a client's ARN-construction defect indistinguishable from a deleted session.
Fix: ListTagsForResource's input validation — same resourceArn constraint gate ahead of the lookup.
Traces: bedrock-agent-runtime-tag-operations-invalid-resource-arn#task#4.

Related but NOT covering: open issue 20261029T214059Z-bedrock-agent-runtime-answers-successful-and-failed-session.md covers the emulator-authored bodies on the session/tagging/rerank SUCCESS and lookup paths, and 20261029T171944Z-bedrock-agent-runtime-ten-net-new-causes-session.md covers re-worded session/invocation refusal messages; neither owns these three steps (the replay reports them with no owner) and neither addresses the absent resourceArn pattern gate that turns a 400 ValidationException into a 404 ResourceNotFoundException.

## Evidence
Replay of fixture bedrock-agent-runtime-tag-operations-invalid-resource-arn (3 diverging steps, 1 step each, no stub_path, no owned_by). Each step submits a tagging call whose resourceArn is an ARN of a non-taggable resource type.

Step task#2 — TagResource:
  expected status 400, actual 400? no: expected 400, got 404
  expected $headers.x-amzn-errortype = 'ValidationException:http://internal.amazon.com/coral/com.amazon.coral.validate/'
  actual   $headers.x-amzn-errortype = 'ResourceNotFoundException'
  expected $.message = "1 validation error detected: Value 'arn:aws:bedrock:us-east-1:000000000000:agent/NOTASESSION' at 'resourceArn' failed to satisfy constraint: Member must satisfy regular expression pattern: arn:aws(-[^:]+)?:bedrock:[a-zA-Z0-9-]+:[0-9]{12}:(session|advanced-optimize-prompt-job)/[a-zA-Z0-9-]+"
  actual   $.message = "Couldn't perform TagResource operation. Failed to retrieve resource type Agent because it doesn't exist. Check the resource ID and retry your request."

Step task#3 — UntagResource: expected 400, got 404; expected errortype 'ValidationException:http://internal.amazon.com/coral/com.amazon.coral.validate/', got 'ResourceNotFoundException'; expected the same '1 validation error detected: ... at 'resourceArn' ...' message, got "Couldn't perform UntagResource operation. Failed to retrieve resource type Agent because it doesn't exist. Check the resource ID and retry your request."

Step task#4 — ListTagsForResource: expected 400, got 404; expected errortype 'ValidationException:http://internal.amazon.com/coral/com.amazon.coral.validate/', got 'ResourceNotFoundException'; expected the same '1 validation error detected: ... at 'resourceArn' ...' message, got "Couldn't perform ListTagsForResource operation. Failed to retrieve resource type Agent because it doesn't exist. Check the resource ID and retry your request."

Model evidence (smithy_query query=operation on com-amazonaws-bedrockagentruntime-amazonbedrockagentruntimeservice.json): all three operations bind resourceArn as a @required @httpLabel targeting TaggableResourcesArn with @length {min:20,max:1011} and @pattern "(^arn:aws(-[^:]+)?:bedrock:[a-zA-Z0-9-]+:[0-9]{12}:(session)/[a-f0-9]{8}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{12}$)", and all three model ValidationException with httpError 400 alongside ResourceNotFoundException with httpError 404. An ARN that violates the member's pattern is therefore a 400 ValidationException, never a 404.

The ARN, region and account in the recorded messages are the fixture's filler for the message template's <value> slot; the requirement is the template and the constraint rule, not those literals.

## Steps to Reproduce
1. Boot the emulator from this checkout and replay fixture bedrock-agent-runtime-tag-operations-invalid-resource-arn.
2. Cause 1 (step task#2): POST /tags/{resourceArn} (TagResource) with resourceArn set to an ARN of a resource type this service cannot tag — e.g. arn:aws:bedrock:<region>:<account>:agent/<id> — and any valid non-empty tags map. Observe 404 / x-amzn-errortype ResourceNotFoundException with a "Couldn't perform TagResource operation. Failed to retrieve resource type ... because it doesn't exist." body, where AWS answers 400 / ValidationException:http://internal.amazon.com/coral/com.amazon.coral.validate/ with "1 validation error detected: Value '<the submitted arn>' at 'resourceArn' failed to satisfy constraint: Member must satisfy regular expression pattern: <the service's TaggableResourcesArn pattern>".
3. Cause 2 (step task#3): DELETE /tags/{resourceArn}?tagKeys=<key> (UntagResource) with the same out-of-pattern ARN. Same expected-vs-actual inversion.
4. Cause 3 (step task#4): GET /tags/{resourceArn} (ListTagsForResource) with the same out-of-pattern ARN. Same expected-vs-actual inversion.
5. Re-replay all three steps after fixing: each must answer 400 with the Coral validate errortype and the constraint message naming member 'resourceArn', the submitted ARN, and the enforced pattern.
