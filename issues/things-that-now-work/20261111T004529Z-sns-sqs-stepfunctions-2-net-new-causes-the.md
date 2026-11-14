Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: sns__sqs__stepfunctions.sfn-sns-publish-missing-permission-catch-denied-negative sns__sqs__stepfunctions.sns-publish-attributes-reach-subscriber-and-invalid-attribute-fails-task
Issue-Key: sns__sqs__stepfunctions.sfn-sns-publish-missing-permission-catch-denied-negative|run#39|AssertionError|assert masked_cause == expected_cause, (
Issue-Key: sns__sqs__stepfunctions.sns-publish-attributes-reach-subscriber-and-invalid-attribute-fails-task|run#28|AssertionError|assert bad["status"] == "FAILED", (
Issue-Commit: f28dfafa5fc9e548e0b0df8415a880c6df425385

# sns__sqs__stepfunctions: 2 net-new causes — the sns:publish AuthorizationError names the role instead of its assumed-role session, lower-cases SNS:Publish and drops Request ID; and an uncastable Number message attribute is published instead of refused

> _Note: this issue was identified against the code base at commit `f28dfafa5fc9e548e0b0df8415a880c6df425385`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
Publish

## Description
Two net-new root causes on the optimized `arn:aws:states:::sns:publish` integration.

Cause 1 of 2: the AuthorizationError relayed into the Task's Cause is mis-worded in three fixed ways.
AWS: Smithy — Publish declares AuthorizationErrorException with httpError 403 and awsQueryError code 'AuthorizationError'. The message AWS words for this error class is: `User: <principal> is not authorized to perform: SNS:Publish on resource: <topic-arn> because no identity-based policy allows the SNS:Publish action (Service: AmazonSNS; Status Code: 403; Error Code: AuthorizationError; Request ID: <request-id>; Proxy: null)`. Its FIXED text is `User: `, ` is not authorized to perform: `, ` on resource: `, ` because no identity-based policy allows the `, ` action (Service: AmazonSNS; Status Code: 403; Error Code: AuthorizationError; Request ID: `, `; Proxy: null)`. Its VARIABLE parts come from the request and state: the principal, which for a Step Functions task is the execution role's ASSUMED-ROLE SESSION ARN `arn:aws:sts::<account-id>:assumed-role/<role-name>/<session-name>` (not the IAM role ARN); the action, spelled by SNS with an upper-cased service prefix as `SNS:Publish` in BOTH positions; the topic ARN from the request; and the request id of that SNS call.
Emulator: the Cause is `User: arn:aws:iam::<account-id>:role/<role-name> is not authorized to perform: sns:Publish on resource: <topic-arn> because no identity-based policy allows the sns:Publish action (Service: AmazonSNS; Status Code: 403; Error Code: AuthorizationError; Proxy: null)`. Three divergences: (a) the principal is rendered as the IAM role ARN `arn:aws:iam::<acct>:role/<role>` instead of the assumed-role session ARN `arn:aws:sts::<acct>:assumed-role/<role>/<session>`; (b) the action is `sns:Publish` in both positions instead of SNS's `SNS:Publish`; (c) the `Request ID: <id>; ` segment is absent from the trailing SDK diagnostic. Status 403, Error Code AuthorizationError and the identity-based-policy clause are correct.
Why: the principal string is what an operator pastes into a policy or a CloudTrail lookup to find the denied identity; the role ARN is a different principal from the session ARN, and the missing request id removes the only handle for correlating the refusal with the SNS call. Test suites and runbooks that match AWS's exact denial text (the action spelling included) do not match.
Fix: the SNS authorization path's AuthorizationError message rendering, plus the Step Functions optimized-integration relay that builds the TaskFailed Cause (it must carry the caller's assumed-role session identity and the SNS call's request id).
Traces: sns__sqs__stepfunctions.sfn-sns-publish-missing-permission-catch-denied-negative. Named for contrast: the open issues 20261110T231031Z-sns-2-net-new-causes-publish-s-authorizationerror.md and 20261110T231031Z-iam-sns-sns-publish-s-authorizationerror-blames-the.md concern WHICH policy the message blames (resource- vs identity-based) — a different defect; on this path the identity-based clause is already right.

Cause 2 of 2: a Number message attribute whose value cannot be cast is accepted and published.
AWS: Smithy — Publish declares InvalidParameterValueException with httpError 400 and awsQueryError code 'ParameterValueInvalid'. A MessageAttributes entry with DataType 'Number' and a StringValue that is not a number is refused: HTTP 400, Error Code ParameterValueInvalid, message `Could not cast message attribute '<attribute-name>' value to number.` — fixed text `Could not cast message attribute '` / `' value to number.`, with the attribute name taken from the request. Through the optimized integration the Task fails with Error `SNS.InvalidParameterValueException` and Cause `Could not cast message attribute '<attribute-name>' value to number. (Service: AmazonSNS; Status Code: 400; Error Code: ParameterValueInvalid; Request ID: <request-id>; Proxy: null)`, nothing is published, and no subscriber ever receives the message.
Emulator: no validation happens. The execution ends SUCCEEDED, so no Error/Cause is produced at all and the publish goes through (the check's next beat asserts the rejected message never reaches the subscribed queue).
Why: a malformed attribute is accepted silently, so a client's bad payload is delivered to every subscriber instead of being refused at the API boundary; filter policies and consumers that assume a Number attribute is numeric then receive a value they cannot parse, and the state machine takes its success branch for a publish AWS never performed.
Fix: SNS Publish's request validation (MessageAttributes DataType/StringValue consistency — Number must be castable) and the error it raises, which the Step Functions sns:publish integration then relays as SNS.InvalidParameterValueException.
Traces: sns__sqs__stepfunctions.sns-publish-attributes-reach-subscriber-and-invalid-attribute-fails-task.

## Evidence
Cause 1 — check sns__sqs__stepfunctions.sfn-sns-publish-missing-permission-catch-denied-negative, verdict mismatch:
AssertionError: TaskFailed cause must be SNS's exact AuthorizationError for this role and topic;
  expected 'User: arn:aws:sts::000000000000:assumed-role/sim2real-xsvc-sfnsns-role-1717ab5c69f5/<session> is not authorized to perform: SNS:Publish on resource: arn:aws:sns:us-west-2:000000000000:sim2real-xsvc-topic-7ecdd8493614 because no identity-based policy allows the SNS:Publish action (Service: AmazonSNS; Status Code: 403; Error Code: AuthorizationError; Request ID: <request-id>; Proxy: null)'
  got      'User: arn:aws:iam::000000000000:role/sim2real-xsvc-sfnsns-role-1717ab5c69f5 is not authorized to perform: sns:Publish on resource: arn:aws:sns:us-west-2:000000000000:sim2real-xsvc-topic-7ecdd8493614 because no identity-based policy allows the sns:Publish action (Service: AmazonSNS; Status Code: 403; Error Code: AuthorizationError; Proxy: null)'
(account id, role/topic names and session are per-run values; the divergences are the arn:aws:iam role form vs the arn:aws:sts assumed-role session form, 'sns:Publish' vs 'SNS:Publish' in both positions, and the missing 'Request ID: <id>; ' segment.)

Cause 2 — check sns__sqs__stepfunctions.sns-publish-attributes-reach-subscriber-and-invalid-attribute-fails-task, verdict mismatch:
AssertionError: a Number attribute that cannot be cast must fail the state, got 'SUCCEEDED'
Request: Task arn:aws:states:::sns:publish with Parameters TopicArn=<topic>, Message=<msg>, MessageAttributes={"count": {"DataType": "Number", "StringValue": "not-a-number"}}.
Expected: execution FAILED, error 'SNS.InvalidParameterValueException', cause "Could not cast message attribute 'count' value to number. (Service: AmazonSNS; Status Code: 400; Error Code: ParameterValueInvalid; Request ID: <request-id>; Proxy: null)", and the message never delivered to the subscribed queue.
Actual: execution SUCCEEDED (no error, no cause).

## Steps to Reproduce
Cause 1:
1. Create an SNS topic and a states.amazonaws.com role with NO sns:Publish grant.
2. Create a state machine with a Task on arn:aws:states:::sns:publish for that topic and a Catch, and StartExecution.
3. Read the TaskFailed cause (or the Catch output): compare against AWS's AuthorizationError text — the principal must be arn:aws:sts::<acct>:assumed-role/<role>/<session>, the action must read SNS:Publish in both positions, and the diagnostic must contain 'Request ID: <id>; ' before 'Proxy: null)'.

Cause 2:
1. Create an SNS topic, an SQS queue subscribed to it with raw delivery off, and a role granting sns:Publish on the topic.
2. Create a state machine with a Task on arn:aws:states:::sns:publish passing Message.$ and MessageAttributes.$ from the execution input.
3. StartExecution with input {"message": "invalid-<n>", "attributes": {"count": {"DataType": "Number", "StringValue": "not-a-number"}}}.
4. DescribeExecution: AWS reports FAILED with error SNS.InvalidParameterValueException and the "Could not cast message attribute 'count' value to number." cause and the queue stays empty; the emulator reports SUCCEEDED.
