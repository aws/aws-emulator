Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: quicksight.datasource-lifecycle-create-update-delete
Issue-Shape-Id: com.amazonaws.quicksight#QuickSight_20180401
Issue-Key: quicksight.datasource-lifecycle-create-update-delete|run#18|AssertionError|assert err["Message"] == source_gone, err
Issue-Commit: 1184b33254953722da367e2321295173ed6a80b9

# quicksight data-source ResourceNotFoundException message is invented wording ("Data source with ID <id> not found.") instead of AWS's ARN-based "DataSource <arn> is not found"

> _Note: this issue was identified against the code base at commit `1184b33254953722da367e2321295173ed6a80b9`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeDataSource

## Description
One net-new root cause in the QuickSight emulator.

Cause 1 of 1: the ResourceNotFoundException raised for a data source that does not exist carries emulator-invented message text, so a client that reads the message (or matches on it) sees something real QuickSight never emits. Failing check: quicksight.datasource-lifecycle-create-update-delete.

AWS: DescribeDataSource (Smithy: GET /accounts/{AwsAccountId}/data-sources/{DataSourceId}; errors include ResourceNotFoundException, httpError 404, documented "One or more resources can't be found."). The model constrains only the error shape and status, not the text, so the message text is recorded behavior only: for a deleted/absent data source QuickSight answers HTTP 404, Code `ResourceNotFoundException`, and a message whose template is the fixed text `DataSource ` + the data source's ARN + ` is not found`. Fixed text: "DataSource " and " is not found". Variable part: the ARN, built from the request and service identity as `arn:aws:quicksight:<region>:<AwsAccountId>:datasource/<DataSourceId>` — region from the endpoint the request was served on, account from the request's AwsAccountId path label, id from the DataSourceId path label. The recorded id/account in the trace (`s2r-lifecycle-src-30228342f640`) only fills that template and is not part of it. The same ARN-based not-found template applies to the other data-source operations keyed on an absent id (UpdateDataSource, DeleteDataSource), which the same check asserts immediately after the describe.

Emulator: the create/update/delete state machine and the error Code are correct (the check gets through CREATION_SUCCESSFUL, UPDATE_SUCCESSFUL, the renamed Name, and the delete, and the post-delete describe does return Code `ResourceNotFoundException`), but the message is `Data source with ID s2r-lifecycle-src-30228342f640 not found.` — i.e. the template `Data source with ID {DataSourceId} not found.` with a trailing period and no ARN. Replay output: `AssertionError: {'Message': 'Data source with ID s2r-lifecycle-src-30228342f640 not found.', 'Code': 'ResourceNotFoundException'}` where the expected message was `DataSource arn:aws:quicksight:<region>:<account>:datasource/<id> is not found`.

Why: a client that surfaces, logs or pattern-matches QuickSight's not-found message gets text that never appears in AWS; anything that parses the ARN out of the message (to identify which resource of a batch is missing, or to resolve region/account) finds no ARN at all and either misreports or fails. Because the same wording is used by the mutating paths, a caller cannot distinguish the emulator's responses from AWS's on any message-sensitive path.

Fix: the not-found error construction in the data-source operation handlers — it should render the resource ARN rather than a bare id, and share one not-found message builder across the data-source handlers so describe, update and delete agree. Belongs in those handlers' error paths (see Traces for file/line citations); the ARN is already the form the handlers return in DataSource.Arn, so the builder should reuse that ARN derivation.

Traces: failing check quicksight.datasource-lifecycle-create-update-delete (assertion at check line 66, `assert err["Message"] == source_gone`). Message constructed in services/service-com-amazonaws-quicksight-quicksight-20180401/src/ops/describe_data_source.rs line 164 and, identically, services/service-com-amazonaws-quicksight-quicksight-20180401/src/ops/update_data_source.rs line 194. This is distinct from open issue 20261029T191733Z-quicksight-createdatasource-never-runs-the-connection-test-an-athena.md, which concerns CreateDataSource not running the connection test; here creation succeeds as expected and only the not-found message diverges.

## Evidence
Check quicksight.datasource-lifecycle-create-update-delete, after a successful create -> update -> delete cycle:

Request: DescribeDataSource(AwsAccountId=<account>, DataSourceId=<deleted id>)

Expected (AWS): HTTP 404, Error.Code = "ResourceNotFoundException", Error.Message = "DataSource arn:aws:quicksight:<region>:<account>:datasource/<DataSourceId> is not found" (fixed text "DataSource " ... " is not found"; variable part is the ARN assembled from region, the request's AwsAccountId and DataSourceId).

Actual (emulator): Error.Code = "ResourceNotFoundException" (correct), Error.Message = "Data source with ID <DataSourceId> not found." — template `Data source with ID {DataSourceId} not found.`, no ARN, trailing period.

Replay excerpt:
  AssertionError: {'Message': 'Data source with ID s2r-lifecycle-src-30228342f640 not found.', 'Code': 'ResourceNotFoundException'}

Source of the divergence:
  describe_data_source.rs:164  message: Some(format!("Data source with ID {} not found.", key.1))
  update_data_source.rs:194    message: Some(format!("Data source with ID {} not found.", key.1))

Smithy citation: DescribeDataSource errors -> ResourceNotFoundException (errorKind client, httpError 404); UpdateDataSource errors -> ResourceNotFoundException (httpError 404). The model does not fix the message text, so the template above is recorded behavior only.

## Steps to Reproduce
1. Ensure the QuickSight account subscription exists (CreateAccountSubscription / DescribeAccountSubscription) and an assumable QuickSight service role ARN is available.
2. CreateDataSource(AwsAccountId=<account>, DataSourceId=<id>, Name=<id>, Type="ATHENA", DataSourceParameters={AthenaParameters:{WorkGroup:"primary", RoleArn:<role>}}) and poll DescribeDataSource until Status == CREATION_SUCCESSFUL.
3. UpdateDataSource with a new Name, poll until Status == UPDATE_SUCCESSFUL.
4. DeleteDataSource(AwsAccountId=<account>, DataSourceId=<id>).
5. Call DescribeDataSource(AwsAccountId=<account>, DataSourceId=<id>) and read Error.Message: the emulator answers "Data source with ID <id> not found." where AWS answers "DataSource arn:aws:quicksight:<region>:<account>:datasource/<id> is not found".
6. Call UpdateDataSource on the same deleted id and read Error.Message: the same invented wording is returned.
