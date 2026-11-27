Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: emr.list-notebook-executions-unissued-marker-rejected emr.list-notebook-executions-rejects-a-token-another-operation-issued emr.start-notebook-execution-rejects-missing-notebook-object
Issue-Shape-Id: com.amazonaws.emr#ElasticMapReduce
Issue-Key: emr.list-notebook-executions-unissued-marker-rejected|run#5|AssertionError|assert (code, status, message) == (
Issue-Key: emr.list-notebook-executions-rejects-a-token-another-operation-issued|run#11|AssertionError|assert (code, status, message.replace(token, "<marker>")) == (
Issue-Key: emr.start-notebook-execution-rejects-missing-notebook-object|run#32|AssertionError|raise AssertionError(f"a notebook key with no object was accepted as execution {accepted!r}")
Issue-Commit: 96db52f5b4a4b596ab227bd8becc0930845733a7

# emr: 2 net-new causes — an invalid ListNotebookExecutions Marker is refused as InvalidRequestException instead of ValidationException, and StartNotebookExecution accepts a NotebookS3Location whose S3 object does not exist

> _Note: this issue was identified against the code base at commit `96db52f5b4a4b596ab227bd8becc0930845733a7`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListNotebookExecutions

## Description
Two independent net-new defects in the emr emulator's notebook-execution surface. They are unrelated causes bundled into one issue because they share a service; each must be closed on its own.

Cause 1 of 2: ListNotebookExecutions refuses an invalid/unissued Marker with the wrong error code.
  AWS: Smithy (com-amazonaws-emr-elasticmapreduce) models ListNotebookExecutions with Marker as both inputToken and outputToken (paginated: inputToken=Marker, outputToken=Marker, items=NotebookExecutions) and declares only InvalidRequestException (client, httpError 400) besides InternalServerError. The model does NOT state the wire code used for a page-token rejection, so for the code this is recorded behavior only: the recording shows EMR refusing a Marker it never issued with HTTP 400 and error code `ValidationException`, message `Marker '<marker>' is not valid.` — fixed text `Marker '` ... `' is not valid.`, with the single variable part being the Marker string copied verbatim from the request. A page token is bound to the operation that minted it: a NextToken returned by ListReleaseLabels is still a valid NextToken for ListReleaseLabels, yet is not a valid Marker for ListNotebookExecutions.
  Emulator: the HTTP status (400) and the message are right, but the error code is `InvalidRequestException` instead of `ValidationException`. Replay shows ('InvalidRequestException', 400, "Marker 'Zm9yZ2VkLXBhZ2UtdG9rZW4=' is not valid.") for a forged Marker, and ('InvalidRequestException', 400, "Marker 'ZW1yL3YxL3JlbGVhc2VMYWJlbHMvMDp8MDovZW1yLTcuMTQuMA==' is not valid.") for a ListReleaseLabels NextToken presented as a Marker. (Those Marker values are the particular tokens of one run and are not part of the requirement; the requirement is that the refused Marker is quoted back.)
  Why: clients branch on `Error.Code`, not on message text. Code that retries or resets pagination on `ValidationException`, and treats `InvalidRequestException` as a non-retryable request-shape problem (or vice versa), takes the wrong branch; botocore/SDK error-shape matching and `except emr.exceptions.*` handlers never match. A paginator fed a stale or cross-operation token therefore surfaces a different exception class than on AWS.
  Fix: the error classification used by the ListNotebookExecutions handler's Marker/page-token validation — it must emit error code `ValidationException` with HTTP 400 while keeping the existing message template. The two token-rejection paths (a token no call issued, and a token another operation issued) must both be classified the same way.
  Traces: emr.list-notebook-executions-unissued-marker-rejected, emr.list-notebook-executions-rejects-a-token-another-operation-issued.

Cause 2 of 2: StartNotebookExecution does not verify that the notebook object named by NotebookS3Location exists, and accepts the request.
  AWS: Smithy models StartNotebookExecution with NotebookS3Location (NotebookS3LocationFromInput, Bucket+Key) and declares InvalidRequestException (client, httpError 400). Recorded behavior: when the Bucket exists but no object exists at Key, AWS refuses the call with HTTP 400, error code `InvalidRequestException`, and a message sentence of the form `<bucket>/<key> does not exist in S3.` — fixed text `/` between the two values and the trailing ` does not exist in S3.`, with the bucket name and the object key taken from the request's NotebookS3Location; a parenthesised suffix may follow the sentence. No execution is created: the refused NotebookExecutionName does not appear in ListNotebookExecutions for the cluster. Once the object is put at that key, the identical request succeeds and DescribeNotebookExecution reports that NotebookS3Location and a new execution id.
  Emulator: the request is accepted. StartNotebookExecution returns a NotebookExecutionId for a key that has no S3 object (replay: accepted as execution 'ex-0000000000002' — that id is one run's value, not a requirement), so the missing-object precondition is never checked and no refusal is produced.
  Why: a client relying on AWS's behavior uses this refusal as its existence check for the notebook it is about to run. On the emulator the bad request is accepted, so the caller gets an execution id it believes is runnable, the typo/ordering bug (notebook uploaded after the start call) is never reported, and the execution then lingers or fails later with an unrelated error instead of failing fast at the API with InvalidRequestException. Negative-path tests asserting the refusal pass on AWS and fail here, and the cluster's execution list gains an entry AWS would never have created.
  Fix: the StartNotebookExecution handler's input validation — before creating any execution state, resolve NotebookS3Location.Bucket/Key against S3 state and, when the object is absent, refuse with HTTP 400 / `InvalidRequestException` / the `<bucket>/<key> does not exist in S3.` message, creating no execution record (so ListNotebookExecutions for the cluster is unchanged by the refused call).
  Traces: emr.start-notebook-execution-rejects-missing-notebook-object.

## Evidence
Cause 1 of 2 — ListNotebookExecutions Marker refusal error code.
  Check emr.list-notebook-executions-unissued-marker-rejected:
    Expected: ('ValidationException', 400, "Marker '<forged>' is not valid.")
    Actual:   ('InvalidRequestException', 400, "Marker 'Zm9yZ2VkLXBhZ2UtdG9rZW4=' is not valid.")
    AssertionError: a forged Marker drew ('InvalidRequestException', 400, "Marker 'Zm9yZ2VkLXBhZ2UtdG9rZW4=' is not valid.")
  Check emr.list-notebook-executions-rejects-a-token-another-operation-issued:
    Expected: ('ValidationException', 400, "Marker '<marker>' is not valid.")
    Actual:   ('InvalidRequestException', 400, "Marker 'ZW1yL3YxL3JlbGVhc2VMYWJlbHMvMDp8MDovZW1yLTcuMTQuMA==' is not valid.")
    AssertionError: the ListReleaseLabels token as a Marker drew ('InvalidRequestException', 400, "Marker 'ZW1yL3YxL3JlbGVhc2VMYWJlbHMvMDp8MDovZW1yLTcuMTQuMA==' is not valid.")
  Status (400) and message both match; only Error.Code diverges. Smithy for ListNotebookExecutions declares errors [InternalServerError(500), InvalidRequestException(400)] and paginated {inputToken: Marker, outputToken: Marker, items: NotebookExecutions}; the wire code for a token rejection is recorded behavior only.

Cause 2 of 2 — StartNotebookExecution missing-object precondition.
  Check emr.start-notebook-execution-rejects-missing-notebook-object:
    Expected: ClientError with Error.Code == 'InvalidRequestException', HTTP 400, message whose leading sentence is '<bucket>/in/later.ipynb does not exist in S3.'; and the refused NotebookExecutionName absent from ListNotebookExecutions for the cluster.
    Actual:   StartNotebookExecution returned success.
    AssertionError: a notebook key with no object was accepted as execution 'ex-0000000000002'
  The same request succeeds on AWS only after s3:PutObject at that key; Smithy StartNotebookExecution declares InvalidRequestException (client, httpError 400) and InternalServerException (500).

## Steps to Reproduce
Cause 1 of 2:
  1. emr.list_notebook_executions(Status='FINISHED') — confirm it answers with a NotebookExecutions list.
  2. emr.list_notebook_executions(Status='FINISHED', Marker='Zm9yZ2VkLXBhZ2UtdG9rZW4=') (any Marker no call issued).
  3. Inspect the ClientError: AWS gives Error.Code 'ValidationException', HTTP 400, "Marker '<that marker>' is not valid."; the emulator gives Error.Code 'InvalidRequestException' with the same status and message.
  4. Cross-operation variant: token = emr.list_release_labels(MaxResults=1)['NextToken']; then emr.list_notebook_executions(Marker=token) — same wrong code. Confirm emr.list_release_labels(MaxResults=1, NextToken=token) still pages normally afterwards.

Cause 2 of 2:
  1. Create an S3 bucket; put a valid .ipynb at key 'in/answer.ipynb'. Create the EMR service role, the EC2 instance profile and a notebook service role trusted by elasticmapreduce.amazonaws.com with s3:* on the bucket.
  2. emr.run_job_flow(... ReleaseLabel='emr-7.9.0', Applications Hadoop/Spark/Livy/JupyterEnterpriseGateway, one m5.xlarge MASTER, KeepJobFlowAliveWhenNoSteps=True) and wait for the cluster to reach WAITING.
  3. emr.start_notebook_execution(ExecutionEngine={'Id': <cluster>, 'Type': 'EMR'}, ServiceRole=<notebook role arn>, NotebookS3Location={'Bucket': <bucket>, 'Key': 'in/answer.ipynb'}, OutputNotebookS3Location={'Bucket': <bucket>, 'Key': 'out/'}, OutputNotebookFormat='HTML', NotebookExecutionName=<name1>) — succeeds on both.
  4. Repeat step 3 with NotebookS3Location Key='in/later.ipynb', an object that was never put, and a fresh NotebookExecutionName.
  5. AWS raises ClientError InvalidRequestException / 400 with the leading sentence '<bucket>/in/later.ipynb does not exist in S3.' and lists no execution under that name; the emulator returns a NotebookExecutionId instead.
  6. Put an object at 'in/later.ipynb' and repeat step 4 — it must now succeed and DescribeNotebookExecution must report NotebookS3Location {'Bucket': <bucket>, 'Key': 'in/later.ipynb'} with a new execution id.
