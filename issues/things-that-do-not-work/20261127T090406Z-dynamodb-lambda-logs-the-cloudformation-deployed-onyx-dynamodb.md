Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: dynamodb__lambda__logs.e2e-diagnose-onyx-lambda-processing-failure-lifecycle
Issue-Key: dynamodb__lambda__logs.e2e-diagnose-onyx-lambda-processing-failure-lifecycle|_reap_log_group#10|Timeout|return
Issue-Commit: 891bdb4ec3b1dd6099e92756c6128712280d1708

# dynamodb__lambda__logs: the CloudFormation-deployed Onyx DynamoDB-Streams→Lambda estate never reaches a graded verdict — the check is killed at the 60 s cap (1 cause)

> _Note: this issue was identified against the code base at commit `891bdb4ec3b1dd6099e92756c6128712280d1708`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateStack

## Description
Cause 1 of 1: a CloudFormation-deployed estate of two stream-enabled DynamoDB tables, an execution role, a Lambda function and two LATEST DynamoDB-Streams event source mappings never progresses far enough for the behavioral check to reach any assertion — the check is killed at the harness cap with no graded phase.

AWS: the check (`e2e-diagnose-onyx-lambda-processing-failure-lifecycle`, GROUP `deploy-e2e-lifecycle`) drives a chain that real AWS completes end to end:
  * CloudFormation `CreateStack` (Smithy: CreateStack input StackName required, TemplateBody @length min 1, Capabilities, Parameters, Tags; errors AlreadyExistsException/InsufficientCapabilitiesException/LimitExceededException/TokenAlreadyExistsException, all HTTP 400) followed by `DescribeStacks` polling (Smithy: DescribeStacks → Stacks[].StackStatus) must reach the terminal status `CREATE_COMPLETE`, and `describe_stacks` must then publish the template's Outputs (function name, log group, both table names, both stream ARNs).
  * Lambda `ListEventSourceMappings` must report exactly the two mappings the stack declares, each with `StartingPosition=LATEST`, `BatchSize=1`, `MaximumRetryAttempts=3` (Smithy: EventSourceMappingConfiguration.StartingPosition is enum EventSourcePosition, "Required for Amazon Kinesis and Amazon DynamoDB Stream event sources"; BatchSize @range 1..10000; MaximumRetryAttempts @range -1..10000; State is one of Creating/Enabling/Enabled/Disabling/Disabled/Updating/Deleting).
  * Once a LATEST mapping is armed, a `PutItem` on either table must deliver a DynamoDB-Streams record to the function, and the handler's stdout must land in the function's `/aws/lambda/<function>` log group so `FilterLogEvents` can read it back. Records must carry the modelled image set for a `NEW_AND_OLD_IMAGES` stream: MODIFY records carry an `OldImage`, INSERT records carry none.
  * `UpdateEventSourceMapping(Enabled=false)` must drive both mappings to `State="Disabled"`, `DeleteStack` must reach `DELETE_COMPLETE`, and `DescribeStackResources` for the deleted stack must then be refused with error code `ValidationError`.
Recorded behavior only for the per-phase timings: the check budgets 600 s for stack convergence (`_STACK_DEADLINE_S`) and 420 s for LATEST-mapping arming (`_ARMING_DEADLINE_S`), i.e. on AWS these stages are expected to converge, not to hang.

Emulator: the replay never reaches any of those assertions. The verdict is `[mismatch]` with the single detail `Error: Check timed out after 60.0s` — the check is killed at the harness cap while still inside a poll loop, so no phase of PHASES (`deploy`, `setup`, `pre_invoke`, `solve`, `cleanup`, `destroy`) completes and no claim is graded. Hypothesis (not stated by the model or by the recording, and the replay output does not narrow it): the block is in one of the two long polls the check enters first — `DescribeStacks` never reporting a terminal `CREATE_COMPLETE` for this template, or both LATEST DynamoDB-Streams mappings never arming so no stream record ever reaches the handler's log group. Both stages are independently known-weak here: this run's other DynamoDB-stream ESM failures are carried by the open issues `20261124T183824Z-dynamodb-lambda-sqs-a-dynamodb-stream-event-source--r2.md` and `20261109T172142Z-dynamodb-lambda-sqs-dynamodb-stream-records-never-reach-net-java-or.md`, and non-terminal CloudFormation stack status by `20261121T082758Z-apigateway-cloudwatch-dynamodb-lambda-lo-apigateway-cloudwatch-dynamodb-lambda-logs-sqs-the-plain.md`. None of those issues carries this check's replay key, so none of them covers it.

Why: a client deploying this shape gets no usable emulator at all — `CreateStack` either never settles or the stream-driven function never runs, so a CloudFormation deploy loop polling `DescribeStacks` for `CREATE_COMPLETE`, or a stream consumer waiting on its first record, hangs indefinitely instead of converging. On AWS both converge, and the subsequent reads (Outputs, ListEventSourceMappings, FilterLogEvents, DescribeTable StreamViewType, the `ValidationError` after delete) all answer. A hang is worse than a wrong answer: there is no error code to branch on.

Fix: the CloudFormation stack-progression state machine for this resource set (AWS::DynamoDB::Table with StreamSpecification, AWS::IAM::Role, AWS::Lambda::Function, AWS::Lambda::EventSourceMapping) must drive the stack to a terminal CREATE_COMPLETE and publish its Outputs; and the Lambda DynamoDB-Streams event-source-mapping poller must arm a LATEST mapping and deliver shard records to the function, with the handler's output reaching its CloudWatch Logs group. Re-run this check with a per-phase journal to pin which of the two stages blocks before changing either.

Traces: dynamodb__lambda__logs.e2e-diagnose-onyx-lambda-processing-failure-lifecycle — `[mismatch] ... Error: Check timed out after 60.0s`.

## Evidence
Suite report, section `### dynamodb__lambda__logs`:

  #### [mismatch] dynamodb__lambda__logs.e2e-diagnose-onyx-lambda-processing-failure-lifecycle
  Error: Check timed out after 60.0s

This is the only net-new failure in the run (2550/2938 passed; every other failing check is marked OWNED by an already-filed issue). The check is not marked ALREADY FILED and not marked OWNED, and no open issue carries its replay key.

Expected (AWS): the check completes all six phases — stack reaches CREATE_COMPLETE and publishes Outputs; ListEventSourceMappings reports the two declared mappings with (StartingPosition, BatchSize, MaximumRetryAttempts) == (LATEST, 1, 3); both LATEST mappings arm and deliver stream records so the seeded BAT request is processed and the directly-inserted IN_PROGRESS regression requests are skipped (logged `Skipping record - no OldImage present (event type: INSERT)`); DescribeTable reports StreamViewType NEW_AND_OLD_IMAGES for both tables; both mappings reach State "Disabled"; DeleteStack reaches DELETE_COMPLETE and DescribeStackResources on the deleted stack is refused with error code ValidationError.

Actual (emulator): no phase completes. The harness kills the check at its 60 s cap while still inside a poll loop, with no phase verdict and no assertion reached. The check itself budgets 600 s for stack convergence and 420 s for LATEST-mapping arming, so it is sitting in one of those two polls when the cap ends it.

## Steps to Reproduce
1. Boot the emulator and replay the behavioral suite.
2. Run the single check `dynamodb__lambda__logs.e2e-diagnose-onyx-lambda-processing-failure-lifecycle` (deployments/onyx-stream-handler/template.json), with a cap above the check's own budgets (> ~1100 s) so it can reach a real verdict rather than the harness cap.
3. Observe the `deploy` phase: `CreateStack` with the Onyx template (Capabilities=[CAPABILITY_IAM], Parameters Prefix/MetricNamespace), then poll `DescribeStacks` for a terminal StackStatus. Expect CREATE_COMPLETE plus Outputs OnyxFunctionName / OnyxLogGroup / {Basalt,Regression}RequestsTableName / {Basalt,Regression}RequestsStreamArn.
4. If the stack settles, observe arming: `PutItem` a fresh `arm-NNN` key into each table and poll the function's `/aws/lambda/<function>` log group via FilterLogEvents until a record from BOTH stream ARNs has reached the handler. Expect both to deliver.
5. Whichever of steps 3 and 4 does not converge is the blocked stage; the printed phase journal (registered first, so it dumps even after a timeout) names it.
