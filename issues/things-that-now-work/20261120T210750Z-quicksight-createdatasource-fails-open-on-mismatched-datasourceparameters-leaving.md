Issue-Source: trace-sweep-divergence
Issue-Trace: quicksight-data-source-creation-rejected-unroutable-host-and-mismatched-parameters
Issue-Step: task#3
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: quicksight-data-source-creation-rejected-unroutable-host-and-mismatched-parameters
Issue-Shape-Id: com.amazonaws.quicksight#QuickSight_20180401
Issue-Key: quicksight|CreateDataSource|4xx->2xx|$.Message
Issue-Key: quicksight|CreateDataSource|4xx->2xx|$.RequestId
Issue-Key: quicksight|CreateDataSource|4xx->2xx|$headers.x-amzn-errortype
Issue-Key: quicksight|CreateDataSource|4xx->2xx|status
Issue-Key: quicksight|DescribeDataSource|4xx->2xx|$.Message
Issue-Key: quicksight|DescribeDataSource|4xx->2xx|$.RequestId
Issue-Key: quicksight|DescribeDataSource|4xx->2xx|$headers.x-amzn-errortype
Issue-Key: quicksight|DescribeDataSource|4xx->2xx|status
Issue-Commit: f85f98f2ce22a299384412ddd2d0ac00fcbe668d

# QuickSight CreateDataSource fails open on mismatched DataSourceParameters, leaving a phantom data source

> _Note: this issue was identified against the code base at commit `f85f98f2ce22a299384412ddd2d0ac00fcbe668d`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateDataSource

## Description
The emulator's QuickSight data-source create path does not cross-check the DataSourceParameters union variant against the request's Type, so a create AWS rejects with a 400 is accepted and persisted. Everything downstream then disagrees with AWS: a DataSourceId that must not exist is describable with a 200. This issue bundles 2 causes; both must be closed.

Cause 1 of 2: CreateDataSource with a DataSourceParameters variant that does not correspond to Type is accepted instead of rejected.
AWS: Smithy (quicksight-20180401): CreateDataSource input members Type -> DataSourceType (enum, required; values include ATHENA, POSTGRESQL, PRESTO, REDSHIFT, ...) and DataSourceParameters -> DataSourceParameters, a UNION whose documentation states "This is a variant type structure. For this structure to be valid, only one of the attributes can be non-null"; CreateDataSource models InvalidParameterValueException (errorKind client, httpError 400), whose shape members are Message and RequestId and whose doc is "One or more parameters has a value that isn't valid." The variant must agree with Type, and when it does not AWS answers HTTP 400 with the x-amzn-errortype header naming InvalidParameterValueException and a body message whose template is "<Variant>Parameters should only be set for <TYPE> source" - fixed text " should only be set for " and " source", with the variable parts taken from the request: the name of the union member that was supplied and the data-source type that member belongs to. The recorded step supplies AthenaParameters on a non-ATHENA create and the recorded message is "AthenaParameters should only be set for ATHENA source"; the member name and type in it are that request's filling of the template, not literals to hard-code.
Emulator: answers HTTP 202 and creates the data source. The response has no x-amzn-errortype header, no $.Message member, and carries a synthetic $.RequestId. Replay detail: "status: expected 400, got 202 / $.Message: missing (expected 'AthenaParameters should only be set for ATHENA source', got None) / $headers.x-amzn-errortype: missing (expected '<shaped-not-asserted>', got None)".
Why: a client whose code sends the wrong parameter variant - the common mistake this validation exists to catch - gets a success against the emulator and a 400 against AWS, so the bug is masked in test and surfaces in production. Worse, the emulator's success is a 2xx creating a data source that AWS never created, so the whole subsequent flow (describe/update/delete, and anything resolving the ARN) runs against state AWS does not have. SDK error-handling paths for InvalidParameterValueException are never exercised.
Fix: CreateDataSource's input validation in the quicksight handler - the pre-persist check that must match the DataSourceParameters union variant against Type and raise InvalidParameterValueException (400) with the templated message before any state is written. Related but NOT covering: the fixed issue 20261109T121538Z-quicksight-accepts-rejected-creates-and-keeps-deleted-resources.md described the same class of fail-open for other clusters and is resolved, so it owns nothing here; the open 20261112T051305Z-quicksight-error-payloads-carry-a-synthetic-requestid-member.md owns only the stray RequestId member on DescribeDataSource/CreateFolder 4xx bodies.
Traces: quicksight-data-source-creation-rejected-unroutable-host-and-mismatched-parameters#task#3.

Cause 2 of 2: DescribeDataSource returns 200 for a data source whose create AWS rejected.
AWS: Smithy: DescribeDataSource GET /accounts/{AwsAccountId}/data-sources/{DataSourceId} models ResourceNotFoundException (errorKind client, httpError 404; members Message, ResourceType, RequestId). Because the create was rejected, no data source with that id exists, so AWS answers HTTP 404 with x-amzn-errortype naming ResourceNotFoundException and the not-found message template "DataSource <arn> is not found" - fixed text "DataSource " and " is not found", with the variable part being the data source's ARN assembled from partition, region, account id and the requested DataSourceId. The recording's "DataSource arn:aws:quicksight:us-west-2:000000000000:datasource/sim2real-probe-qs-ds-bfb99767 is not found" is one run's filling of that template; the region, account id and generated data-source id are environment values, not requirements.
Emulator: answers HTTP 200 with a DataSource body - no error at all. Replay detail: "status: expected 404, got 200 / $.Message: missing (expected 'DataSource ... is not found', got None) / $headers.x-amzn-errortype: missing (expected '<shaped-not-asserted>', got None)", plus a synthetic $.RequestId.
Why: the emulator reports a resource that does not exist on AWS, so a client's existence check and its not-found branch invert between the emulator and AWS; cleanup and idempotency logic keyed on a 404 never runs. This is the state consequence of Cause 1 but is a distinct requirement: Describe must 404 for an id that was never successfully created, so it must be re-replayed on its own after the validation fix lands.
Fix: CreateDataSource's validation (so the record is never written) together with DescribeDataSource's state lookup in the quicksight handler, which must raise ResourceNotFoundException (404) with the ARN-based message when the id is absent from the store.
Traces: quicksight-data-source-creation-rejected-unroutable-host-and-mismatched-parameters#task#4.

## Evidence
Replay of trace set simparity-wire-aws-v1 against the emulator booted from this checkout (emulator_up true, /__admin/health 200, data listener accepting). 49 steps replayed, 12 mismatched, score 75.5%.

Cause 1 - fixture `quicksight-data-source-creation-rejected-unroutable-host-and-mismatched-parameters`, step `task#3`, operation CreateDataSource, verdict mismatch, expected_status 400, actual_status 202:
  status: expected 400, got 202
  $.Message: missing (expected 'AthenaParameters should only be set for ATHENA source', got None)
  $headers.x-amzn-errortype: missing (expected '<shaped-not-asserted>', got None)
  $.RequestId: type_mismatch (expected None, got '00000000-0000-4000-8000-000000000003')
Expected (AWS): 400 InvalidParameterValueException, x-amzn-errortype set, body {"Message": "<Variant>Parameters should only be set for <TYPE> source"}.
Actual (emulator): 202 with a successful create payload and no error members.

Cause 2 - same fixture, step `task#4`, operation DescribeDataSource, verdict mismatch, expected_status 404, actual_status 200:
  status: expected 404, got 200
  $.Message: missing (expected 'DataSource arn:aws:quicksight:us-west-2:000000000000:datasource/sim2real-probe-qs-ds-bfb99767 is not found', got None)
  $headers.x-amzn-errortype: missing (expected '<shaped-not-asserted>', got None)
  $.RequestId: type_mismatch (expected None, got '00000000-0000-4000-8000-000000000004')
Expected (AWS): 404 ResourceNotFoundException with the ARN-based not-found message.
Actual (emulator): 200 with a DataSource description for an id AWS never created.

Model references used: CreateDataSource -> errors include InvalidParameterValueException (client, 400); input Type -> DataSourceType enum (required), DataSourceParameters -> union, doc "only one of the attributes can be non-null". DescribeDataSource -> errors include ResourceNotFoundException (client, 404), http GET /accounts/{AwsAccountId}/data-sources/{DataSourceId}.

## Steps to Reproduce
1. Boot the emulator from this checkout and replay trace set simparity-wire-aws-v1.
2. Cause 1: replay fixture `quicksight-data-source-creation-rejected-unroutable-host-and-mismatched-parameters` through step `task#3` - POST /accounts/<account>/data-sources with a Type that is not ATHENA and DataSourceParameters carrying AthenaParameters. Observe HTTP 202 and a created data source instead of 400 InvalidParameterValueException with message "AthenaParameters should only be set for ATHENA source" and the x-amzn-errortype header.
3. Cause 2: continue the same fixture to step `task#4` - GET /accounts/<account>/data-sources/<the same DataSourceId>. Observe HTTP 200 with a DataSource body instead of 404 ResourceNotFoundException with message "DataSource <arn of that id> is not found".
4. Re-replay BOTH steps after fixing: step task#3 must answer 400 with the templated message, and step task#4 must then answer 404 - a create-side fix alone does not prove the describe-side 404 path, and a describe-side fix alone does not prove the rejection.
