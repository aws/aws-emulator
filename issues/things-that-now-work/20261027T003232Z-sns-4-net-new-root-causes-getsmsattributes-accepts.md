Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: sns.get-sms-attributes-unknown-name-rejected sns.get-sms-sandbox-account-status-throttle-error-shape sns.authorization-error-explicit-deny-returns-correct-code sns.deploy-cfn-kms-encrypted-topic-and-queue-deliver-plaintext
Issue-Shape-Id: com.amazonaws.sns#AmazonSimpleNotificationService
Issue-Key: sns.get-sms-attributes-unknown-name-rejected|run#4|AssertionError|raise AssertionError(
Issue-Key: sns.get-sms-sandbox-account-status-throttle-error-shape|run#6|AssertionError|assert codes, "expected the burst to exceed the 10 TPS hard quota and throttle"
Issue-Key: sns.authorization-error-explicit-deny-returns-correct-code|_poll#17|Timeout|time.sleep(interval_s)
Issue-Key: sns.deploy-cfn-kms-encrypted-topic-and-queue-deliver-plaintext|_receive#13|Timeout|resp = sqs.receive_message(QueueUrl=url, MaxNumberOfMessages=10, WaitTimeSeconds=5)
Issue-Commit: d220b5129ac92467b2bf094da95d24c404aff436

# sns: 4 net-new root causes — GetSMSAttributes accepts an unknown attribute filter name, the GetSMSSandboxAccountStatus hard quota never throttles, an explicit identity-policy Deny is not enforced, and a CFN KMS-encrypted topic/queue pipeline delivers nothing

> _Note: this issue was identified against the code base at commit `d220b5129ac92467b2bf094da95d24c404aff436`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetSMSAttributes

## Description
Four distinct net-new root causes in the SNS emulator, none of them carried by an open issue's replay key. A fixer must close ALL FOUR before submitting.

Cause 1 of 4: GetSMSAttributes accepts a filter naming an attribute SNS does not define and answers 200 with that key simply absent, instead of refusing the request.
  AWS: GetSMSAttributes takes `attributes` (ListString) and answers `attributes` (MapStringToString). Smithy (services/service-com-amazonaws-sns-amazonsimplenotificationservice): GetSMSAttributes modeled errors are AuthorizationErrorException (awsQueryError AuthorizationError, 403), InternalErrorException (InternalError, 500), InvalidParameterException (awsQueryError InvalidParameter, HTTP 400) and ThrottledException (Throttled, 429). AWS refuses a filter naming an undefined attribute with error code `InvalidParameter` and message "Unknown attribute requested: <name>" — fixed text "Unknown attribute requested: " plus the offending attribute name taken verbatim from the request's attributes list. A rejected call changes nothing: a later GetSMSAttributes(attributes=["MonthlySpendLimit"]) reads back exactly what it read before.
  Emulator: the call SUCCEEDS. GetSMSAttributes(attributes=["NotARealSmsAttribute"]) returns HTTP 200 with attributes == {} (an empty map) and no error. The positive control passed (a real name, MonthlySpendLimit, is answered normally), so the filter argument itself works — unknown names are silently ignored rather than validated.
  Why: a caller that typos or uses an attribute from a newer API version is told the attribute exists and is simply unset. Configuration-audit and drift code reads "absent" instead of catching InvalidParameter, so a misspelled attribute name becomes a silent no-signal instead of a hard failure.
  Fix: GetSMSAttributes' request validation in the sns service crate — the requested-names filter must be checked against the set of defined SMS attributes and an unknown name must raise InvalidParameterException (awsQueryError InvalidParameter, HTTP 400) with the message above, leaving account state untouched.
  Traces: sns.get-sms-attributes-unknown-name-rejected (mismatch).

Cause 2 of 4: the hard, non-adjustable per-account/region request quota on GetSMSSandboxAccountStatus is not enforced — a concurrent burst far above it is answered entirely with successes, so no throttling error is ever produced.
  AWS: GetSMSSandboxAccountStatus is quota-limited (AWS documents a hard 10 TPS per account per region for it). Smithy: GetSMSSandboxAccountStatus takes no input, returns IsInSandbox (required boolean), and models ThrottledException (awsQueryError code "Throttled", httpError 429) alongside AuthorizationErrorException and InternalErrorException. Recorded behavior for this quota: a burst of 150 calls at 30-way concurrency (retries disabled) yields a mix in which every FAILED call carries error code `Throttling` at HTTP 400 — never InternalError or any other code — while the calls that do succeed still return a well-formed boolean IsInSandbox.
  Emulator: no call in the burst is ever refused. AssertionError: "expected the burst to exceed the 10 TPS hard quota and throttle" — the collected error-code list is empty, i.e. all 150 concurrent calls succeeded.
  Why: a client cannot exercise or validate its retry/backoff path against the emulator: code that must handle the documented throttling shape (and that AWS will throttle in production at this rate) sees an API with no quota at all, so a load profile that is refused on AWS passes locally and the client's handling of the throttle error class is never exercised.
  Fix: request-rate accounting for the SMS sandbox read operations in the sns service crate — GetSMSSandboxAccountStatus must enforce the per-account/region hard quota and refuse excess calls with the throttling error class (recorded code `Throttling`, HTTP 400; the model's ThrottledException/awsQueryError "Throttled" is the modeled shape), while concurrent calls within the quota continue to return IsInSandbox as a boolean.
  Traces: sns.get-sms-sandbox-account-status-throttle-error-shape (mismatch).

Cause 3 of 4: SNS performs no IAM authorization on its control plane — an IAM user whose own identity policy carries an explicit Deny for sns:GetSMSSandboxAccountStatus keeps being served successfully, so the refusal never happens.
  AWS: a real signed call from an identity whose identity-based policy contains an explicit Deny for the action is refused regardless of a sibling explicit Allow for the same action. SNS answers error code `AuthorizationError` (AuthorizationErrorException, HTTP 403 per the model) with message "User: <caller-arn> is not authorized to perform: SNS:GetSMSSandboxAccountStatus on resource: arn:<partition>:sns:<region>:<account>:* with an explicit deny in an identity-based policy" — fixed text "User: ", " is not authorized to perform: SNS:GetSMSSandboxAccountStatus on resource: ", " with an explicit deny in an identity-based policy"; the variable parts are the caller's user ARN and the account-wide SNS resource ARN built from the request's partition, region and account. An unrestricted identity's identical call succeeds at the same moment, and a sibling-allowed sts:GetCallerIdentity on the same credentials succeeds throughout.
  Emulator: the restricted identity's GetSMSSandboxAccountStatus keeps returning 200. The check polls for any non-success for 60 s and never sees one, so it never completes and the replay reports `Check timed out after 60.0s`; the expected AuthorizationError code and message are never produced.
  Why: a client cannot test least-privilege or deny-based guardrails: every call is authorized no matter what the caller's policy says, so a policy change that locks an application out on AWS is invisible locally, and code paths that handle AuthorizationErrorException (and distinguish it from a generic 403 or InternalError) are never exercised.
  Fix: authorization for SNS operations in the sns service crate — the signed caller's identity-based policy must be evaluated (explicit Deny overriding any Allow) and a denied call must be refused with AuthorizationErrorException carrying the message above; see also the open issues 20261025T000319Z-iam-sns-sns-performs-no-iam-authorization-publish.md (Publish, service token iam__sns) and 20261025T025317Z-sns-authorizes-nothing-on-the-data-plane-topic.md (topic-policy authorization), neither of which carries this check's key nor covers identity-policy deny on the SMS-sandbox read.
  Traces: sns.authorization-error-explicit-deny-returns-correct-code (mismatch, timed out at the harness cap while polling for the refusal).

Cause 4 of 4: the standard compliance pipeline — one customer-managed KMS key, an SNS topic and an SQS queue both encrypted with it, the key policy granting sns.amazonaws.com kms:GenerateDataKey*/kms:Decrypt, a queue policy allowing sns.amazonaws.com sqs:SendMessage — never gets a published message to the encrypted queue.
  AWS: recorded behavior only. The recording shows: the stack reaches CREATE_COMPLETE; GetTopicAttributes and GetQueueAttributes both report a KmsMasterKeyId resolving to the stack's key ARN; a Publish with a UTF-8 body and one String message attribute is delivered, and the subscriber reads the envelope with Message equal to the published body byte-for-byte, TopicArn equal to the topic, and MessageAttributes.kind == {"Type": "String", "Value": "sim2real-order"} — i.e. encryption is transparent to the subscriber.
  Emulator: the check never completes and the replay reports `Check timed out after 60.0s`. Its only unbounded waits are the stack_create_complete waiter and the 90 s receive loop that polls the queue for the published MessageId, so either the stack never converges or the publish never reaches the KMS-encrypted queue; no envelope is ever read. Hypothesis: SNS->SQS delivery does not complete when the destination queue (and the topic) are encrypted with a customer-managed key, the same silence already filed for a different service token as 20261025T055445Z-kms-sns-sqs-sns-sqs-delivery-ignores-the-destination-queue-s... which does not carry this check's replay key.
  Why: every encrypted-at-rest SNS/SQS deployment — the default for compliance-driven estates — looks correctly provisioned (both resources report the key) and then delivers nothing, so a consumer built against it receives no messages at all and the failure is invisible from the control plane.
  Fix: the SNS->SQS delivery path in the sns service crate where the topic/queue carry a KmsMasterKeyId (and the CloudFormation provisioning of AWS::KMS::Key / AWS::SNS::Topic / AWS::SQS::Queue / AWS::SNS::Subscription that arms it) — the data-key grant in the key policy must be used to write into the encrypted queue, and the subscriber must read the plaintext body and message attributes.
  Traces: sns.deploy-cfn-kms-encrypted-topic-and-queue-deliver-plaintext (mismatch, timed out at the harness cap).

## Evidence
Cause 1 of 4 — expected vs actual.
  Expected (AWS): sns.get_sms_attributes(attributes=["NotARealSmsAttribute"]) -> ClientError with Error.Code "InvalidParameter" (HTTP 400) and Error.Message "Unknown attribute requested: NotARealSmsAttribute"; a following get_sms_attributes(attributes=["MonthlySpendLimit"]) is unchanged.
  Actual (emulator): the call SUCCEEDED. AssertionError: "GetSMSAttributes with an unknown attribute filter name was expected to be rejected, but it SUCCEEDED with {}". The positive control get_sms_attributes(attributes=["MonthlySpendLimit"]) returned a value, so the filter path works.

Cause 2 of 4 — expected vs actual.
  Expected (AWS, recorded): 150 get_sms_sandbox_account_status calls over 30 worker threads with retries disabled (max_attempts=1) produce at least one ClientError, every error code == "Throttling", and the successful calls return IsInSandbox as a bool.
  Actual (emulator): zero errors. AssertionError: "expected the burst to exceed the 10 TPS hard quota and throttle" (the collected code list is empty).

Cause 3 of 4 — expected vs actual.
  Expected (AWS): for an IAM user whose inline policy is {Allow sts:GetCallerIdentity *, Allow sns:GetSMSSandboxAccountStatus *, Deny sns:GetSMSSandboxAccountStatus *}, sts:GetCallerIdentity on its keys succeeds, and sns:GetSMSSandboxAccountStatus is refused with code "AuthorizationError" and message "User: <user-arn> is not authorized to perform: SNS:GetSMSSandboxAccountStatus on resource: arn:<partition>:sns:<region>:<account>:* with an explicit deny in an identity-based policy", stably across repeated samples, while the unrestricted identity's identical call returns a boolean IsInSandbox at the same moments.
  Actual (emulator): the restricted identity's call keeps succeeding; the poll for any non-success never resolves and the check is reported as `Check timed out after 60.0s`.

Cause 4 of 4 — expected vs actual.
  Expected (AWS, recorded): the stack reaches CREATE_COMPLETE; topic and queue KmsMasterKeyId both resolve to the stack key's ARN; sns.publish(Message=<utf-8 body>, MessageAttributes={kind:{DataType:String,StringValue:"sim2real-order"}}) is received from the queue within 90 s with envelope Message == the body, TopicArn == the stack's topic, and MessageAttributes.kind == {"Type":"String","Value":"sim2real-order"}.
  Actual (emulator): no envelope for the published MessageId is ever received; the check does not complete and is reported as `Check timed out after 60.0s`.

## Steps to Reproduce
Cause 1 of 4:
  1. sns.get_sms_attributes(attributes=["MonthlySpendLimit"]) -> a value (control).
  2. sns.get_sms_attributes(attributes=["NotARealSmsAttribute"]) -> expect InvalidParameter "Unknown attribute requested: NotARealSmsAttribute"; the emulator returns 200 with {}.
  Replay: sns.get-sms-attributes-unknown-name-rejected.

Cause 2 of 4:
  1. Build an SNS client with retries={"max_attempts": 1, "mode": "standard"}.
  2. Fire 150 get_sms_sandbox_account_status calls across 30 threads.
  3. Expect at least one ClientError and every error code == "Throttling"; the emulator answers all 150 successfully.
  Replay: sns.get-sms-sandbox-account-status-throttle-error-shape.

Cause 3 of 4:
  1. iam.create_user; iam.put_user_policy with {Allow sts:GetCallerIdentity *, Allow sns:GetSMSSandboxAccountStatus *, Deny sns:GetSMSSandboxAccountStatus *}; iam.create_access_key.
  2. With those credentials, sts.get_caller_identity -> succeeds (control).
  3. With those credentials, sns.get_sms_sandbox_account_status -> expect AuthorizationError with the explicit-deny message; the emulator keeps returning 200.
  Replay: sns.authorization-error-explicit-deny-returns-correct-code.

Cause 4 of 4:
  1. cloudformation.create_stack with AWS::KMS::Key (key policy: account root kms:*, plus sns.amazonaws.com allowed kms:GenerateDataKey*/kms:Decrypt), AWS::SNS::Topic{KmsMasterKeyId: Ref Key}, AWS::SQS::Queue{KmsMasterKeyId: Ref Key}, AWS::SQS::QueuePolicy allowing sns.amazonaws.com sqs:SendMessage with aws:SourceArn = the topic, AWS::SNS::Subscription (sqs protocol, GetAtt Queue.Arn); Outputs KeyId/TopicArn/QueueUrl. Wait for stack_create_complete.
  2. Confirm GetTopicAttributes and GetQueueAttributes both report a KmsMasterKeyId resolving to the stack key ARN.
  3. sns.publish(TopicArn=<out>, Message=<utf-8 body>, MessageAttributes={kind:{DataType:String,StringValue:"sim2real-order"}}) and poll the queue for that MessageId for 90 s. AWS delivers the plaintext envelope; the emulator delivers nothing.
  Replay: sns.deploy-cfn-kms-encrypted-topic-and-queue-deliver-plaintext (allow more than the 60 s suite cap).
