Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: emr-containers.describe-job-run-driver-exception-reports-failed emr-containers.job-run-driver-exception-fails-with-user-error emr-containers.job-run-driver-exit-reports-user-error emr-containers.list-job-runs-unissued-next-token-rejected emr-containers.security-configuration-lake-formation-without-query-engine-role-rejected
Issue-Shape-Id: com.amazonaws.emrcontainers#AwsChicagoWebService
Issue-Key: emr-containers.describe-job-run-driver-exception-reports-failed|run#14|AssertionError|assert bad["state"] == "FAILED", (
Issue-Key: emr-containers.job-run-driver-exception-fails-with-user-error|run#10|AssertionError|assert bad["state"] == "FAILED", (
Issue-Key: emr-containers.job-run-driver-exit-reports-user-error|run#26|AssertionError|assert seen == ("FAILED", "USER_ERROR"), (
Issue-Key: emr-containers.list-job-runs-unissued-next-token-rejected|run#17|AssertionError|assert message == "Invalid NextToken Value", f"unexpected refusal message: {message!r}"
Issue-Key: emr-containers.security-configuration-lake-formation-without-query-engine-role-rejected|_refusal#16|AssertionError|raise AssertionError(f"the request was expected to be refused but was accepted: {answer!r}")
Issue-Commit: a79d1fca27aa19162430a7b50588de220866f92b

# emr-containers: 3 net-new root causes — job runs with a failing driver report COMPLETED, unissued-nextToken refusal message wrong, and a Lake Formation security configuration without queryEngineRoleArn is accepted

> _Note: this issue was identified against the code base at commit `a79d1fca27aa19162430a7b50588de220866f92b`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeJobRun

## Description
Three independent net-new root causes in the emr-containers emulator, found by replaying the behavioral suite. This issue is NOT closed until ALL THREE causes below are fixed and all five named checks replay clean. (The open issue 20261022T001417Z-emr-containers-error-paths-missing-input-validation-on.md covers CreateVirtualCluster/DescribeJobRun identifier validation on the WIRE corpus; none of the three causes here is that issue — they concern job-run terminal state, ListJobRuns token-refusal wording, and CreateSecurityConfiguration required-member validation.)

Cause 1 of 3: every job run reaches COMPLETED, so a run whose driver fails is never reported as FAILED/USER_ERROR.
Failing checks: emr-containers.describe-job-run-driver-exception-reports-failed, emr-containers.job-run-driver-exception-fails-with-user-error, emr-containers.job-run-driver-exit-reports-user-error.
AWS: Smithy: DescribeJobRun (GET /virtualclusters/{virtualClusterId}/jobruns/{id}) returns a JobRun whose `state` targets the JobRunState enum [PENDING, SUBMITTED, RUNNING, FAILED, CANCELLED, CANCEL_PENDING, COMPLETED], whose `failureReason` targets the FailureReason enum [INTERNAL_ERROR, USER_ERROR, VALIDATION_ERROR, CLUSTER_UNAVAILABLE] (doc: "The reasons why the job run has failed"), and whose `stateDetails` (String256) carries "Additional details of the job run state". AWS drives the terminal state from the submitted driver's outcome: a run whose Spark driver exits non-zero or throws (e.g. SparkPi submitted with a non-numeric slice argument, which makes the driver raise) ends in state FAILED with failureReason USER_ERROR — a user-caused driver failure, not an EMR fault — and stateDetails describing the driver failure rather than completion. A run whose driver succeeds ends COMPLETED with no failureReason. Recorded behavior confirms the FAILED/USER_ERROR pair for a driver that raises.
Emulator: the emulator ignores the submitted jobDriver entirely and marks every run COMPLETED. The three checks observe state 'COMPLETED' where 'FAILED' is required, failureReason absent (None) where 'USER_ERROR' is required, and stateDetails 'Job run completed.' for a run whose driver raised.
Why: EMR-on-EKS clients branch on the terminal state: a caller that polls DescribeJobRun/ListJobRuns until a terminal state and then checks state == 'FAILED' plus failureReason to decide whether to alarm, retry, or surface a user error sees unconditional success. Bad jobs are reported as successful, so failure handling, retry policy, and the FailureReason classification path never execute, and a test suite asserting failure sees a green run.
Fix: the job-run lifecycle/state machine in the emr-containers service crate (StartJobRun's handler and the state that advances a run to its terminal state, read back by DescribeJobRun and ListJobRuns) — the terminal state must be derived from the submitted driver's outcome, setting state=FAILED, failureReason=USER_ERROR and a stateDetails describing the driver failure when the driver fails, and COMPLETED with no failureReason only when it succeeds.
Traces: emr-containers.describe-job-run-driver-exception-reports-failed (SparkPi with slices 'two' ended 'COMPLETED', expected 'FAILED'); emr-containers.job-run-driver-exception-fails-with-user-error (run whose driver throws ended 'COMPLETED'); emr-containers.job-run-driver-exit-reports-user-error (ended ('COMPLETED', None), expected ('FAILED', 'USER_ERROR'), stateDetails='Job run completed.').

Cause 2 of 3: ListJobRuns refuses an unissued nextToken with the wrong error message.
Failing check: emr-containers.list-job-runs-unissued-next-token-rejected.
AWS: Smithy: ListJobRuns (GET /virtualclusters/{virtualClusterId}/jobruns) is @paginated (inputToken/outputToken nextToken, items jobRuns, pageSize maxResults); `nextToken` targets NextToken (@length 1..1024, @pattern \\S); its only client error is ValidationException at httpError 400. A syntactically valid nextToken the service never issued is refused with HTTP 400, error code ValidationException and the fixed message "Invalid NextToken Value" — the message is entirely fixed text, with no part taken from the request (the offending token is deliberately not echoed).
Emulator: status 400 and code ValidationException are correct, but the message is 'nextToken is not a valid pagination token'.
Why: the message is part of the error contract. A client or test that matches on the refusal text (logging, assertion, or operator-facing surfacing of why pagination broke) does not match, and the divergent wording suggests a different failure class (malformed token) from the one AWS reports (a token this service never issued).
Fix: the nextToken decode/validation path of ListJobRuns in the emr-containers service crate — raise ValidationException with the exact message "Invalid NextToken Value" for a well-formed token the emulator did not issue, leaving the 400/ValidationException classification as it already is. The positive-control assertions around it (page one issues a token, that token pages to the rest, and it still pages after the refusal) already pass and must keep passing.
Traces: emr-containers.list-job-runs-unissued-next-token-rejected.

Cause 3 of 3: CreateSecurityConfiguration accepts a Lake Formation configuration with no queryEngineRoleArn instead of refusing it.
Failing check: emr-containers.security-configuration-lake-formation-without-query-engine-role-rejected.
AWS: Smithy: CreateSecurityConfiguration (POST /securityconfigurations) takes a required `securityConfigurationData` (SecurityConfigurationData -> authorizationConfiguration -> lakeFormationConfiguration) and models ValidationException at httpError 400 as its only client error. Inside LakeFormationConfiguration the members authorizedSessionTagValue, secureNamespaceInfo and queryEngineRoleArn are not individually @required in the model, but AWS enforces queryEngineRoleArn whenever a lakeFormationConfiguration is supplied (the QueryEngine role is what assumes the JobExecutionRole for the Lake Formation calls, per the member's own documentation): the request is refused with HTTP 400, error code ValidationException and the message "Request is missing required parameter: '<member path>'", where the fixed text is "Request is missing required parameter: " and the quoted variable part is the absent member's full request path — here securityConfigurationData.authorizationConfiguration.lakeFormationConfiguration.queryEngineRoleArn. Nothing is created.
Emulator: answers HTTP 200 with a created security configuration (id/name/arn in the body) and registers it, i.e. it performs no cross-member validation of lakeFormationConfiguration at all.
Why: a caller gets a security-configuration id for a Lake Formation setup that cannot work — the FGAC path has no query engine role — so the misconfiguration surfaces only later, as an unexplained job-run failure, instead of at create time. Negative-path handling for a missing required parameter never runs, and the invalid configuration then appears in ListSecurityConfigurations as if valid.
Fix: CreateSecurityConfiguration's input validation in the emr-containers service crate — when securityConfigurationData.authorizationConfiguration.lakeFormationConfiguration is present, require queryEngineRoleArn and return the modeled ValidationException (400) with the missing-required-parameter message naming that member path, before any state is inserted.
Traces: emr-containers.security-configuration-lake-formation-without-query-engine-role-rejected.

## Evidence
Replay of behavioral suite simparity-behavioral-aws-v1 (emulator up and serving; emr-containers 21/26 checks passed, 5 failed).

Cause 1 — job-run terminal state (3 checks, all [mismatch]):
  emr-containers.describe-job-run-driver-exception-reports-failed:
    AssertionError: SparkPi run with non-numeric slices 'two' ended 'COMPLETED', expected 'FAILED'
  emr-containers.job-run-driver-exception-fails-with-user-error:
    AssertionError: the job run whose driver throws ended 'COMPLETED', not 'FAILED'
  emr-containers.job-run-driver-exit-reports-user-error:
    AssertionError: a run whose driver raised ended ('COMPLETED', None), expected ('FAILED', 'USER_ERROR'); stateDetails='Job run completed.'
  expected: DescribeJobRun -> jobRun.state = 'FAILED', jobRun.failureReason = 'USER_ERROR', stateDetails describing the driver failure
  actual:   jobRun.state = 'COMPLETED', jobRun.failureReason absent, stateDetails = 'Job run completed.'

Cause 2 — ListJobRuns unissued nextToken ([mismatch] emr-containers.list-job-runs-unissued-next-token-rejected):
  request: ListJobRuns(virtualClusterId=<vc>, maxResults=1, nextToken='QQQ...' — a token of the issued token's length that was never issued)
  expected: HTTP 400, Error.Code 'ValidationException', Error.Message 'Invalid NextToken Value'
  actual:   HTTP 400, Error.Code 'ValidationException', Error.Message 'nextToken is not a valid pagination token'
  (status and code match; only the message diverges. Fixed text of the required message: the whole string 'Invalid NextToken Value' — no variable part.)

Cause 3 — CreateSecurityConfiguration ([mismatch] emr-containers.security-configuration-lake-formation-without-query-engine-role-rejected):
  request: CreateSecurityConfiguration(name=<unique>, securityConfigurationData={authorizationConfiguration:{lakeFormationConfiguration:{authorizedSessionTagValue, secureNamespaceInfo}}}) with queryEngineRoleArn omitted
  expected: HTTP 400, Error.Code 'ValidationException', Error.Message "Request is missing required parameter: 'securityConfigurationData.authorizationConfiguration.lakeFormationConfiguration.queryEngineRoleArn'"; nothing created
  actual:   HTTP 200 with body {'id': <generated id>, 'name': <requested name>, 'arn': 'arn:aws:emr-containers:<region>:<account>:/securityconfigurations/<id>'} — the check reported: "the request was expected to be refused but was accepted" (ids/names/timestamps in that body are per-run values, not requirements)

Model evidence (services/service-com-amazonaws-emrcontainers-awschicagowebservice/smithy-model/com-amazonaws-emrcontainers-awschicagowebservice.json):
  JobRun.state -> JobRunState enum [PENDING, SUBMITTED, RUNNING, FAILED, CANCELLED, CANCEL_PENDING, COMPLETED]; JobRun.failureReason -> FailureReason enum [INTERNAL_ERROR, USER_ERROR, VALIDATION_ERROR, CLUSTER_UNAVAILABLE]; JobRun.stateDetails -> String256.
  ListJobRuns: paginated {inputToken: nextToken, outputToken: nextToken, items: jobRuns, pageSize: maxResults}; nextToken -> NextToken @length 1..1024 @pattern \\S; errors = {InternalServerException 500, ValidationException 400}.
  CreateSecurityConfiguration: POST /securityconfigurations, securityConfigurationData required; LakeFormationConfiguration members = {authorizedSessionTagValue, secureNamespaceInfo, queryEngineRoleArn}; errors = {InternalServerException 500, ValidationException 400}.

## Steps to Reproduce
1. git clone the repo and boot the emulator built from it.
2. Replay the behavioral suite simparity-behavioral-aws-v1 and look at the emr-containers section.

Cause 1 (checks describe-job-run-driver-exception-reports-failed, job-run-driver-exception-fails-with-user-error, job-run-driver-exit-reports-user-error):
3. Create an EKS cluster and an EMR-on-EKS virtual cluster over it, then StartJobRun with a Spark driver that fails — e.g. entryPoint local:///usr/lib/spark/examples/jars/spark-examples.jar, --class org.apache.spark.examples.SparkPi, with the single argument 'two' (non-numeric slices), so the driver raises.
4. Poll DescribeJobRun until the run is terminal. Expected jobRun.state 'FAILED' with jobRun.failureReason 'USER_ERROR' and a stateDetails describing the driver failure; observed state 'COMPLETED', no failureReason, stateDetails 'Job run completed.'

Cause 2 (check list-job-runs-unissued-next-token-rejected):
5. Submit two job runs into a virtual cluster, then call ListJobRuns(virtualClusterId=<vc>, maxResults=1) to obtain an issued nextToken.
6. Call ListJobRuns(virtualClusterId=<vc>, maxResults=1, nextToken=<a token of the same length that was never issued, e.g. 'Q'*len(issued)>). Expected HTTP 400 ValidationException with message 'Invalid NextToken Value'; observed the same status and code with message 'nextToken is not a valid pagination token'.

Cause 3 (check security-configuration-lake-formation-without-query-engine-role-rejected):
7. Call CreateSecurityConfiguration with securityConfigurationData.authorizationConfiguration.lakeFormationConfiguration present (authorizedSessionTagValue and secureNamespaceInfo set) but queryEngineRoleArn omitted. Expected HTTP 400 ValidationException with message "Request is missing required parameter: 'securityConfigurationData.authorizationConfiguration.lakeFormationConfiguration.queryEngineRoleArn'" and no configuration created; observed HTTP 200 with a created configuration.

8. Re-replay all five checks listed in trace_ids to confirm the fix; all five must pass.
