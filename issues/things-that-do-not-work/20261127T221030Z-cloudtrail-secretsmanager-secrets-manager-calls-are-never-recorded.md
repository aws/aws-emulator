Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudtrail__secretsmanager.cloudtrail-records-a-refused-get-secret-value-with-its-error-code-and-no-secret-value
Issue-Key: cloudtrail__secretsmanager.cloudtrail-records-a-refused-get-secret-value-with-its-error-code-and-no-secret-value|run#15|Timeout|time.sleep(pause)
Issue-Commit: 1a8d6da9d066bb6b81849373d117fc388454286a

# cloudtrail__secretsmanager: Secrets Manager calls are never recorded in CloudTrail event history, so the audit poll for CreateSecret/GetSecretValue never completes

> _Note: this issue was identified against the code base at commit `1a8d6da9d066bb6b81849373d117fc388454286a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
LookupEvents

## Description
Cause 1 of 1: no CloudTrail event-history record is written for Secrets Manager management calls, so a client cannot audit a secret's reads at all.
AWS:      Secrets Manager is a CloudTrail-integrated service: CreateSecret and GetSecretValue (served and refused alike) are recorded in CloudTrail event history as management events with `eventSource` = `secretsmanager.amazonaws.com`, the operation's `eventName`, the `requestParameters` actually sent (`secretId` for GetSecretValue, `name` for CreateSecret, `versionStage` when supplied), `readOnly` true for GetSecretValue, and `responseElements` null for GetSecretValue. A refused read additionally carries `errorCode` and `errorMessage` equal to the SDK's Error.Code/Error.Message; a served read carries no `errorCode`. Secret material is never recorded: no record contains the secret value, and CreateSecret's `requestParameters` omits `secretString` entirely. Smithy: CloudTrail LookupEvents is the read path (LookupAttributes EventName/EventSource, paginated, modelled errors include InvalidLookupAttributesException and ThrottlingException).
Emulator: the Secrets Manager calls themselves are served — CreateSecret returns the secret ARN, GetSecretValue(SecretId=<arn>) returns the stored SecretString, and GetSecretValue(SecretId=<arn>, VersionStage='AWSPENDING') is refused ('ResourceNotFoundException', "Secrets Manager can't find the specified secret value for staging label: AWSPENDING") — but LookupEvents returns no record for any of them. The check's poll, `LookupEvents(LookupAttributes=[{EventName:'GetSecretValue'}], StartTime=since)` and the same for `CreateSecret`, retried under a ~900 s delivery budget, never reaches 2 GetSecretValue records and 1 CreateSecret record, so the check runs to the harness cap and reports `Check timed out after 60.0s` instead of ever reaching its record assertions.
Why:      every audit/compliance client is broken: a Secrets Manager read cannot be attributed, a denied or not-found read cannot be distinguished from a successful one in the audit log, and the redaction guarantees AWS makes (no secret value in any record, no `secretString` in the CreateSecret record) cannot be verified. Because LookupEvents answers an empty page rather than an error, a client waiting for delivery hangs on its own retry budget instead of failing fast.
Fix:      the CloudTrail event-history ingestion path for the secretsmanager service — Secrets Manager's request-handling layer must emit a management-event record (eventSource `secretsmanager.amazonaws.com`, eventName, requestID, readOnly, requestParameters with `secretString` withheld, `responseElements` null for GetSecretValue, and on refusal errorCode/errorMessage matching the served error) into the store LookupEvents reads.
Traces:   cloudtrail__secretsmanager.cloudtrail-records-a-refused-get-secret-value-with-its-error-code-and-no-secret-value.

No open issue covers this. The many open secretsmanager issues concern rotation, validation messages, authorization and CloudFormation; the open per-service CloudTrail audit gaps (ssm 20261127T125407Z-..., lightsail 20261127T194606Z-..., cognito-idp 20261122T054242Z-..., eventbridgev2 20261127T111942Z-..., dynamodb 20261127T024328Z-...) each name their own service and none names secretsmanager.

## Evidence
cloudtrail__secretsmanager.cloudtrail-records-a-refused-get-secret-value-with-its-error-code-and-no-secret-value — Error: Check timed out after 60.0s.

The Secrets Manager legs preceding the poll all behave, so the hang is the audit read:
  CreateSecret(Name=<unique>, SecretString=<value>) -> returns ARN
  GetSecretValue(SecretId=<arn>) -> returns the stored SecretString
  GetSecretValue(SecretId=<arn>, VersionStage='AWSPENDING') -> ('ResourceNotFoundException', "Secrets Manager can't find the specified secret value for staging label: AWSPENDING")

The check then loops on:
  LookupEvents(LookupAttributes=[{AttributeKey:'EventName', AttributeValue:'GetSecretValue'}], StartTime=now-2m)  filtered on requestParameters.secretId == <arn>
  LookupEvents(LookupAttributes=[{AttributeKey:'EventName', AttributeValue:'CreateSecret'}], StartTime=now-2m)   filtered on requestParameters.name == <name>
  Expected: >= 2 GetSecretValue records (one with errorCode None, one with errorCode 'ResourceNotFoundException' and errorMessage equal to the served message, requestParameters.versionStage == 'AWSPENDING'), each with eventSource 'secretsmanager.amazonaws.com', readOnly True and responseElements None; >= 1 CreateSecret record whose requestParameters omit 'secretString'; and no record containing the secret value.
  Actual: 0 GetSecretValue and 0 CreateSecret records are ever returned; the poll exhausts its budget.

## Steps to Reproduce
1. secretsmanager.CreateSecret(Name=<unique>, SecretString=<value>, Tags=[...]) -> note the ARN.
2. secretsmanager.GetSecretValue(SecretId=<arn>) -> succeeds.
3. secretsmanager.GetSecretValue(SecretId=<arn>, VersionStage='AWSPENDING') -> refused ResourceNotFoundException.
4. cloudtrail.LookupEvents(LookupAttributes=[{AttributeKey:'EventName', AttributeValue:'GetSecretValue'}], StartTime=<a few minutes ago>), paginated, and the same for EventName='CreateSecret'.
   Expected: two GetSecretValue records (served and refused, the refused one carrying errorCode 'ResourceNotFoundException' and the served message as errorMessage, requestParameters.versionStage 'AWSPENDING') plus one CreateSecret record with no 'secretString' parameter, all with eventSource 'secretsmanager.amazonaws.com' and no record containing the secret value.
   Actual: no matching events are returned at all, so a delivery poll never terminates.
