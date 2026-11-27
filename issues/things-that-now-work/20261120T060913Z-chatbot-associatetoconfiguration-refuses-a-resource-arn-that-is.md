Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: chatbot.associate-wrong-resource-type-rejected
Issue-Shape-Id: com.amazonaws.chatbot#WheatleyOrchestration_20171011
Issue-Key: chatbot.associate-wrong-resource-type-rejected|run#5|AssertionError|assert code == "InvalidRequestException", f"unexpected code={code!r}"
Issue-Commit: e12f9e5cb3515bb2dfa0c1f17fb3c3b71d90760b

# Chatbot AssociateToConfiguration refuses a Resource ARN that is not a custom-action ARN with 403 UnauthorizedException instead of 400 InvalidRequestException "Invalid request body"

> _Note: this issue was identified against the code base at commit `e12f9e5cb3515bb2dfa0c1f17fb3c3b71d90760b`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
AssociateToConfiguration

## Description
Cause 1 of 1: AssociateToConfiguration misclassifies a `Resource` value that violates the member's modeled pattern (an ARN of the wrong resource type, e.g. the caller's own chat-configuration ARN) as an authorization failure instead of malformed client input. Failing check: chatbot.associate-wrong-resource-type-rejected.

AWS: Smithy (services/service-com-amazonaws-chatbot-wheatleyorchestration-20171011/smithy-model/com-amazonaws-chatbot-wheatleyorchestration-20171011.json) models AssociateToConfiguration.Resource -> ResourceIdentifier with @required, @length min 1 max 1011 and @pattern ^arn:aws:chatbot:[A-Za-z0-9_/.-]{0,63}:[A-Za-z0-9_/.-]{0,63}:custom-action/[a-zA-Z0-9_-]{1,64}$; the operation models InvalidRequestException (HTTP 400, "Your request input doesn't meet the constraints required by AWS Chatbot."), UnauthorizedException (HTTP 403) and InternalServiceError (HTTP 500). A chat-configuration ARN (…:chat-configuration/chime-webhook/<name>) cannot satisfy that pattern, so the value never reaches an authorization decision. The recorded behavior for such an input is HTTP 400, error code `InvalidRequestException`, message `Invalid request body` — the message is fixed text in full for this error class (no request-, state- or run-derived substitutions), and no association is created. The ARN the check passes is whatever ChatConfigurationArn the configuration created in the same run carries; the requirement is "any Resource outside the modeled pattern", not that one value.

Emulator: the call is refused with code `UnauthorizedException` (HTTP 403) and the service's fixed unauthorized message, so the input-constraint class is never reached. The replay fails at `assert code == "InvalidRequestException", f"unexpected code={code!r}"` with `AssertionError: unexpected code='UnauthorizedException'`. The wrong-resource-type ARN is correctly absent from ListAssociations afterwards, and the positive control (CreateCustomAction/GetCustomAction) succeeds, so only the refusal's class, status and message diverge.

Why: a client cannot distinguish "I sent the wrong kind of ARN" from "my credentials are not valid for the target resource". UnauthorizedException/403 drives credential-refresh, role-assumption and alarm paths and is commonly treated as non-retryable-but-environmental, while InvalidRequestException/400 is a programming error the caller must fix by changing the request. SDK error-type branching (`except chatbot.exceptions.InvalidRequestException`) does not fire at all, and the absent `Invalid request body` message removes the only hint about what was malformed.

Fix: the operation's input validation / handler for AssociateToConfiguration. The modeled @pattern on Resource must be enforced at the centralized constraint boundary and refused 400 InvalidRequestException with the fixed message `Invalid request body`; the resource-scoped UnauthorizedException path must be reserved for ARNs that DO satisfy the pattern but name no custom action this account can hold. Today a pattern-violating ARN falls through the boundary into the handler's mintable-ARN check, which answers UnauthorizedException. Note the pre-existing open issue 20261102T105418Z-chatbot-associatetoconfiguration-stub ("Implement chatbot AssociateToConfiguration: unimplemented scaffold stub") does not cover this: the handler is now implemented (services/service-com-amazonaws-chatbot-wheatleyorchestration-20171011/src/ops/associate_to_configuration.rs, lines 163-175 are the refusal taken here) and answers a modeled error, not 501. It is also the mirror image of the two open DisassociateFromConfiguration issues (20261119T212914Z, 20261119T235624Z), which report that operation answering InvalidRequestException where UnauthorizedException is expected — the two operations classify a malformed Resource differently and must not be collapsed into one rule.

Traces: chatbot.associate-wrong-resource-type-rejected (AssociateToConfiguration with the chat configuration's own ARN as Resource).

## Evidence
Check chatbot.associate-wrong-resource-type-rejected [mismatch]:

  File "check_chatbot_associate_wrong_resource_type_rejected.py", line 38, in run
    assert code == "InvalidRequestException", f"unexpected code={code!r}"
  AssertionError: unexpected code='UnauthorizedException'

Expected (recorded AWS): HTTP 400, Error.Code = "InvalidRequestException", Error.Message = "Invalid request body" (fixed text, no variable parts), and no association created for that Resource.
Actual (emulator): Error.Code = "UnauthorizedException" (HTTP 403) with the service's fixed unauthorized message.

Request shape: AssociateToConfiguration(Resource=<ChatConfigurationArn of a Chime webhook configuration>, ChatConfiguration=<same ARN>). That Resource value cannot match the modeled Resource @pattern ^arn:aws:chatbot:...:custom-action/[a-zA-Z0-9_-]{1,64}$.

The subsequent assertions in the check (ListAssociations does not contain the ARN; CreateCustomAction + GetCustomAction positive control) are not reached, but nothing indicates they would fail: only the refusal's class/status/message diverges.

## Steps to Reproduce
1. Boot the chatbot emulator.
2. Create a Chime webhook configuration (CreateChimeWebhookConfiguration with an IAM role ARN and an SNS topic ARN) and take its ChatConfigurationArn.
3. Call AssociateToConfiguration(Resource=<that ChatConfigurationArn>, ChatConfiguration=<that ChatConfigurationArn>) — i.e. a Resource that is not a custom-action ARN and therefore violates the member's modeled @pattern.
4. Observe HTTP 403 with Error.Code "UnauthorizedException"; AWS answers HTTP 400 with Error.Code "InvalidRequestException" and Error.Message "Invalid request body".
5. Re-replay chatbot.associate-wrong-resource-type-rejected to confirm the fix (it also asserts the ARN never appears in ListAssociations and that a normal CreateCustomAction/GetCustomAction lifecycle still succeeds).
