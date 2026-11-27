Issue-Severity: MEDIUM
Issue-Commit: fae8da064c2d4dbbf13168e02f63a16c4cc6fce8

# Chatbot DisassociateFromConfiguration rejects a malformed Resource with 400 InvalidRequestException instead of 403 UnauthorizedException "Unauthorized"

> _Note: this issue was identified against the code base at commit `fae8da064c2d4dbbf13168e02f63a16c4cc6fce8`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DisassociateFromConfiguration

## Description
Cause 1 of 1: DisassociateFromConfiguration orders input-syntax validation ahead of the resource-scoped authorization check, so a `Resource` value that is not a Chatbot custom-action ARN is refused as a constraint violation rather than failing closed as unauthorized. Failing check ids: chatbot.disassociate-from-configuration-malformed-resource-arn-rejected.

AWS: DisassociateFromConfiguration models `Resource` (target ResourceIdentifier) as required with @length min 1 / max 1011 and @pattern `^arn:aws:chatbot:[A-Za-z0-9_/.-]{0,63}:[A-Za-z0-9_/.-]{0,63}:custom-action/[a-zA-Z0-9_-]{1,64}$`, and declares the errors InternalServiceError (500, server), InvalidRequestException (400, "Your request input doesn't meet the constraints required by AWS Chatbot.") and UnauthorizedException (403, "The request was rejected because it doesn't have valid credentials for the target resource."). The model does not state which check runs first; recorded behavior only for the ordering: AWS evaluates the resource-scoped authorization check on the supplied `Resource` BEFORE applying that pattern, so a `Resource` that is not an ARN at all is refused with HTTP 403, error code `UnauthorizedException` and the message `Unauthorized`. That message is entirely fixed text for this error class — it carries no value taken from the request or from service state (it does not echo the offending `Resource`, the `ChatConfiguration`, the account or the member name). The operation is also @idempotent with a 204 success code, and the check's positive control confirms the other half of the contract: a well-formed custom-action ARN that was never associated disassociates as a harmless no-op (no error, 204) and ListAssociations then lists no association for it.

Emulator: the same call is refused with error code `InvalidRequestException` (HTTP 400) — the emulator applies the `Resource` @pattern constraint and returns the generic input-constraint error, never reaching the authorization decision. Replay output: `AssertionError: unexpected code='InvalidRequestException'` where `UnauthorizedException` was required. The accompanying message assertion (`message == "Unauthorized"`) is consequently unreachable, so the emulator's message for this case is wrong too: it must be the fixed string `Unauthorized`, not a constraint-violation description.

Why: a client that distinguishes "this call was denied" from "this input is malformed" is misrouted. On real AWS a caller handling a malformed/foreign `Resource` sees a 403 denial and treats it as a permissions or cross-account problem (re-assume a role, escalate, stop retrying); against the emulator it sees a 400 input error and "fixes" the request instead. The status class differs as well (403 vs 400), so retry and alarm policies keyed on 4xx subclass, and SDK error-shape handlers that catch `UnauthorizedException` specifically, take the wrong branch. Any negative test that asserts Chatbot fails closed on an unverifiable resource passes against AWS and fails against the emulator.

Fix: the DisassociateFromConfiguration handler's request-validation ordering — the resource-scoped authorization check on `Resource` must be evaluated before the ResourceIdentifier @length/@pattern constraint, and when it denies, the handler must serialize UnauthorizedException with HTTP 403 and the message `Unauthorized`. The pattern check remains for inputs that pass authorization. The idempotent no-op path for a well-formed, never-associated custom-action ARN (204, no error, absent from ListAssociations) must be preserved.

Traces: chatbot.disassociate-from-configuration-malformed-resource-arn-rejected (Resource="not-a-valid-arn-at-all" against a freshly created Chime webhook configuration's ChatConfigurationArn). Note: the open issue 20261030T111746Z-chatbot-untagresource-on-a-missing-resource-returns-403.md concerns the opposite direction on a different operation (UntagResource returning 403 where 404 is required) and does not cover this one.

## Evidence
Expected (real AWS, per the recorded check): chatbot.disassociate_from_configuration(Resource="not-a-valid-arn-at-all", ChatConfiguration=<chime webhook config ARN>) -> HTTP 403, Error.Code = "UnauthorizedException", Error.Message = "Unauthorized" (fixed text, no request- or state-derived parts).

Actual (emulator): HTTP 400, Error.Code = "InvalidRequestException" (the ResourceIdentifier @pattern constraint error), so the message is a constraint description rather than "Unauthorized".

Replay excerpt:
  File "check_chatbot_disassociate_from_configuration_malformed_resource_arn_rejected.py", line 30, in run
    assert code == "UnauthorizedException", f"unexpected code={code!r}"
  AssertionError: unexpected code='InvalidRequestException'

Smithy (com.amazonaws.chatbot): DisassociateFromConfiguration.Resource -> ResourceIdentifier, required, @length 1..1011, @pattern ^arn:aws:chatbot:[A-Za-z0-9_/.-]{0,63}:[A-Za-z0-9_/.-]{0,63}:custom-action/[a-zA-Z0-9_-]{1,64}$; errors InvalidRequestException (httpError 400), UnauthorizedException (httpError 403), InternalServiceError (500); @idempotent, http {method POST, uri /disassociate-from-configuration, code 204}.

The check's positive control passed-path requirement (unexercised because the call above failed first): a well-formed custom-action ARN never associated to the configuration disassociates without error and ListAssociations(ChatConfiguration=...) returns no association whose Resource equals it.

## Steps to Reproduce
1. Create an SNS topic and an IAM role trusted by chatbot.amazonaws.com.
2. chatbot.create_chime_webhook_configuration(WebhookDescription=..., WebhookUrl=<https://hooks.chime.aws/incomingwebhooks/...>, SnsTopicArns=[<topic arn>], IamRoleArn=<role arn>, ConfigurationName=<unique name>) and keep WebhookConfiguration.ChatConfigurationArn.
3. Call chatbot.disassociate_from_configuration(Resource="not-a-valid-arn-at-all", ChatConfiguration=<that ChatConfigurationArn>).
4. Observe the refusal: the emulator answers HTTP 400 with Error.Code "InvalidRequestException"; real AWS answers HTTP 403 with Error.Code "UnauthorizedException" and Error.Message exactly "Unauthorized".
5. Control (must keep working): chatbot.create_custom_action(ActionName=<[a-zA-Z0-9_-]{1,64}>, Definition={"CommandText": "echo ctl"}), then disassociate_from_configuration(Resource=<that CustomActionArn>, ChatConfiguration=<ChatConfigurationArn>) must succeed as a no-op (204) and list_associations(ChatConfiguration=...) must not list that Resource.

Replay: run the behavioral suite check chatbot.disassociate-from-configuration-malformed-resource-arn-rejected.
