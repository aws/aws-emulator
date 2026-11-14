Issue-Source: trace-sweep-divergence
Issue-Trace: cognito-idp-osc-dashboards-client-callback-diagnosis
Issue-Step: task#11
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: cognito-idp-osc-dashboards-client-callback-diagnosis cognito-idp-start-import-job-no-csv-precondition-error
Issue-Shape-Id: com.amazonaws.cognitoidentityprovider#AWSCognitoIdentityProviderService
Issue-Key: cognito-idp|DescribeUserPoolClient|4xx->4xx|$.message
Issue-Key: cognito-idp|StartUserImportJob|4xx->4xx|$.message
Issue-Commit: 580c4da031f2890ff11393cc3a031fb2021c1c07

# Request-validation and precondition refusals for app-client reads and user-import starts carry the wrong message text

> _Note: this issue was identified against the code base at commit `580c4da031f2890ff11393cc3a031fb2021c1c07`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeUserPoolClient

## Description
This bundle carries 2 causes, both of the same fault: the HTTP status and error code of the refusal are right but the emulator substitutes its own prose for the message AWS returns. Both causes must be closed before submitting.

Cause 1 of 2: DescribeUserPoolClient with a null UserPoolId. AWS: UserPoolId is a required member (Smithy: DescribeUserPoolClient.UserPoolId -> UserPoolIdType, required, @length min 1 max 55, @pattern ^[\w-]+_[0-9a-zA-Z]+$; modeled error InvalidParameterException, httpError 400). For this operation the recorded refusal is HTTP 400 with the message 'UserPoolId must not be null' — the whole string is fixed text apart from the member name 'UserPoolId', which is taken from the missing input member; it is NOT the generic multi-error 'N validation error(s) detected: ...' rendering AWS uses for constraint violations on a member that WAS supplied. Emulator: HTTP 400 with message "1 validation error detected: Value null at 'userPoolId' failed to satisfy constraint: Member must not be null" — the generic constraint-violation template applied to a missing required member. Why: a client that branches on the refusal text (or an operator reading it) is told a supplied value failed a constraint rather than that a required parameter is absent, and any assertion on AWS's wording fails; the two renderings also disagree on the member's capitalization ('UserPoolId' vs 'userPoolId'). Fix: the DescribeUserPoolClient input-validation path must emit the missing-required-member message for an absent UserPoolId, instead of routing it through the generic constraint-violation renderer. Traces: cognito-idp-osc-dashboards-client-callback-diagnosis#task#11.

Cause 2 of 2: StartUserImportJob on a job whose CSV was never uploaded. AWS: recorded behavior only for the exact wording — the model gives StartUserImportJob the precondition error (Smithy: StartUserImportJob errors include PreconditionNotMetException, httpError 400; JobId -> UserImportJobIdType, required, @pattern ^import-[0-9a-zA-Z-]+$) but does not state the text. The recording shows HTTP 400 with message 'No csv file was uploaded for <JobId>': the fixed text is 'No csv file was uploaded for ' and the trailing token is the request's JobId (the recorded run's 'import-7nrJDoQjeU' is just one run's value, not part of the template). Emulator: HTTP 400 with message "No users are available for import. Upload the CSV file to the job's pre-signed URL before starting it." — different fixed text, and it names no job, so the message does not identify which import job was refused. Why: a caller driving several import jobs cannot tell from the refusal which job is unready, and text assertions against AWS's wording fail. Fix: the StartUserImportJob handler's precondition check must render AWS's message with the request's JobId interpolated. Traces: cognito-idp-start-import-job-no-csv-precondition-error#task#4.

Related already-open issues, neither of which covers these causes: 20261102T111936Z-cognito-idp-user-pool-refusal-messages-and-error.md reports the same null-UserPoolId rendering for DescribeUserPool (and other user-pool message texts) but carries no replay key for the DescribeUserPoolClient step above; 20261029T184956Z-cognito-idp-validation-and-auth-refusal-message-rendering.md covers other validation message renderings and not StartUserImportJob.

## Evidence
Replayed against the emulator booted from this checkout (trace set simparity-wire-aws-v1). Both clusters are verdict=mismatch with expected_status == actual_status == 400 and the error code matching — only $.message differs.

Cause 1 — DescribeUserPoolClient, fixture cognito-idp-osc-dashboards-client-callback-diagnosis step task#11:
  $.message: expected 'UserPoolId must not be null'
                got "1 validation error detected: Value null at 'userPoolId' failed to satisfy constraint: Member must not be null"
  Fixed text vs values: AWS's string is entirely fixed except the member name 'UserPoolId', taken from the absent required input member.
  (The identical rendering fault on DescribeUserPool, fixture cognito-idp-acr-traj-pool-id-mistakes#task#2, is already owned by 20261102T111936Z-cognito-idp-user-pool-refusal-messages-and-error.md and is not re-filed here.)

Cause 2 — StartUserImportJob, fixture cognito-idp-start-import-job-no-csv-precondition-error step task#4:
  $.message: expected 'No csv file was uploaded for import-7nrJDoQjeU'
                got "No users are available for import. Upload the CSV file to the job's pre-signed URL before starting it."
  Fixed text vs values: 'No csv file was uploaded for ' is fixed; the trailing 'import-7nrJDoQjeU' is the JobId from the request (per-run value), so the requirement is the template with the request's JobId, not that literal id.

## Steps to Reproduce
1. Replay the wire corpus against the emulator built from this checkout (replay_wire_corpus on trace set simparity-wire-aws-v1).
2. Cause 1: replay fixture cognito-idp-osc-dashboards-client-callback-diagnosis step task#11 — a DescribeUserPoolClient call with UserPoolId null. The response is HTTP 400 as expected, but $.message is the generic "1 validation error detected: Value null at 'userPoolId' ..." string instead of 'UserPoolId must not be null'.
3. Cause 2: replay fixture cognito-idp-start-import-job-no-csv-precondition-error step task#4 — CreateUserImportJob followed by StartUserImportJob with no CSV uploaded to the job's pre-signed URL. The response is HTTP 400 as expected, but $.message is "No users are available for import. Upload the CSV file to the job's pre-signed URL before starting it." instead of 'No csv file was uploaded for <JobId>' with the request's JobId.
