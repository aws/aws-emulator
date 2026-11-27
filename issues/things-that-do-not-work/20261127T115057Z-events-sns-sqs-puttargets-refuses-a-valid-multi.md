Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: events__sns__sqs.input-transformer-multi-line-template-reaches-the-queue-as-the-sns-message
Issue-Key: events__sns__sqs|PutTargets|ValidationException
Issue-Commit: 7e5e0efa347a2e327a9b3588e8ef7ff7fc53eee8

# events__sns__sqs: PutTargets refuses a valid multi-line InputTransformer template as "not valid JSON"

> _Note: this issue was identified against the code base at commit `7e5e0efa347a2e327a9b3588e8ef7ff7fc53eee8`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutTargets

## Description
Cause 1 of 1: an InputTemplate that is a multi-line (non-JSON-object) template is rejected, so an SNS target that must publish a multi-line message cannot be configured at all.

AWS: InputTemplate need not be a JSON object — Smithy documents it explicitly: "The InputTemplate can also be valid JSON with variables in quotes or out", and a quoted string template (including one containing newlines, with placeholders in <angle brackets>) is accepted; InputTemplate is a plain string @length 1..8192. The rendered template is then delivered as the SNS Message. A template that is genuinely invalid is refused with ValidationException `Invalid InputTemplate for target <target-id>` (plus the parser location when the template is a JSON object misusing a placeholder as a key).
Emulator: PutTargets is refused HTTP 400 `ValidationException: InputTemplate for target topic is not valid JSON.` — a legal template is rejected, and with an emulator-authored message whose premise (templates must be JSON) is wrong.
Why: a client cannot wire any non-JSON/multi-line transformer to a target; the whole input-transformer feature is restricted to JSON objects, and the refusal text sends the user to fix a template that is correct.
Fix: the InputTransformer validation in the events PutTargets handler (accept string templates; only refuse genuinely unparsable ones, with AWS's wording).
Traces: events__sns__sqs.input-transformer-multi-line-template-reaches-the-queue-as-the-sns-message.

## Evidence
Expected: PutTargets accepts the multi-line InputTemplate and the rendered text arrives as the SNS message on the subscribed queue.
Actual: `botocore.exceptions.ClientError: An error occurred (ValidationException) when calling the PutTargets operation: InputTemplate for target topic is not valid JSON.`

## Steps to Reproduce
1. sns CreateTopic + subscribe an SQS queue; events PutRule on a bus.
2. events PutTargets naming the topic with InputTransformer {InputPathsMap: {...}, InputTemplate: "\"line one <v>\nline two\""} (a multi-line quoted template).
3. Observe the ValidationException refusal.
