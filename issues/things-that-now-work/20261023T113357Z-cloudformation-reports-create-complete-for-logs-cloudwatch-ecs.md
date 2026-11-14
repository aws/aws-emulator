Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: logs.deploy-log-group-cfn-then-put-events-succeeds logs.deploy-log-group-cfn-rename-replaces-group-and-drops-events logs.deploy-log-group-cfn-retain-policy-keeps-group-and-events logs.deploy-logs-1-kms-log-group logs.deploy-subscription-filter-cfn-then-forwards-to-destination cloudformation__logs.log-group-invalid-retention-update-rejected-rolls-back cloudwatch__lambda__logs.cfn-deploy-logs-subscription-filter-lambda-positive apigateway__dynamodb__lambda__logs.deploy-cfn-lambda-log-group-retention-positive apigateway__dynamodb__lambda__logs.deploy-log-retention-set-via-template logs.deploy-log-group-cfn-invalid-retention-rejected logs.deploy-log-stream-cfn-into-missing-group-rejected cloudformation__cloudwatch__logs.deploy-logs-3-invalid-filter-pattern-rejected-rolls-back cloudformation__cloudwatch__logs.deploy-logs-3-invalid-pattern-update-rejected-keeps-old-filter cloudwatch__lambda__logs.cfn-deploy-lambda-cloudwatch-alarm-positive cloudwatch__lambda__logs.cfn-deploy-alarm-invalid-metric-rejected cloudwatch__ecs__elbv2__logs.deploy-cloudwatch-alarm-created-for-alb-5xx apigateway__dynamodb__lambda__logs.deploy-dynamodb-table-stream-arn-exported-correctly apigateway__dynamodb__lambda__logs.deploy-lambda-dynamodb-event-source-mapping-created cloudformation__ecs__logs.deploy-ecs-2-cfn-circuit-breaker-rolls-stack-back-to-last-good-token
Issue-Key: cloudwatch-logs|PutLogEvents|ResourceNotFoundException
Issue-Key: logs.deploy-logs-1-kms-log-group|run#20|AssertionError|assert lg is not None, f"log group {group_a!r} from CREATE_COMPLETE stack A is not described"
Issue-Key: cloudwatch-logs|DescribeSubscriptionFilters|ResourceNotFoundException
Issue-Key: cloudformation__logs.log-group-invalid-retention-update-rejected-rolls-back|_retention#9|AssertionError|raise AssertionError(f"log group {group_name!r} not found")
Issue-Key: cloudwatch-logs|CreateLogStream|ResourceNotFoundException
Issue-Key: apigateway__dynamodb__lambda__logs.deploy-cfn-lambda-log-group-retention-positive|run#7|AssertionError|assert len(matching) == 1, f"expected exactly one log group named {log_group_name}"
Issue-Key: apigateway__dynamodb__lambda__logs.deploy-log-retention-set-via-template|run#11|AssertionError|assert len(groups) == 1, f"expected exactly one log group named {log_group_name}, got {groups}"
Issue-Key: logs.deploy-log-group-cfn-invalid-retention-rejected|run#12|AssertionError|assert bad_status == "ROLLBACK_COMPLETE", (
Issue-Key: logs.deploy-log-stream-cfn-into-missing-group-rejected|run#11|AssertionError|assert status == "ROLLBACK_COMPLETE", (
Issue-Key: cloudformation__cloudwatch__logs.deploy-logs-3-invalid-filter-pattern-rejected-rolls-back|run#15|AssertionError|assert bad_status == "ROLLBACK_COMPLETE", (
Issue-Key: cloudformation__cloudwatch__logs.deploy-logs-3-invalid-pattern-update-rejected-keeps-old-filter|run#18|AssertionError|assert status == "UPDATE_ROLLBACK_COMPLETE", (
Issue-Key: cloudwatch__lambda__logs.cfn-deploy-lambda-cloudwatch-alarm-positive|run#8|IndexError|alarm_cfg = cw.describe_alarms(AlarmNames=[alarm_name])["MetricAlarms"][0]
Issue-Key: cloudwatch__lambda__logs.cfn-deploy-alarm-invalid-metric-rejected|run#11|KeyError|assert by_name[invalid_alarm]["StateValue"] == "INSUFFICIENT_DATA", (
Issue-Key: cloudwatch__ecs__elbv2__logs.deploy-cloudwatch-alarm-created-for-alb-5xx|run#8|IndexError|described = cloudwatch.describe_alarms(AlarmNames=[alarm_name])["MetricAlarms"][0]
Issue-Key: apigateway__dynamodb__lambda__logs.deploy-dynamodb-table-stream-arn-exported-correctly|_deploy_cfn#5|KeyError|outputs = {o["OutputKey"]: o["OutputValue"] for o in stack.get("Outputs", [])}
Issue-Key: apigateway__dynamodb__lambda__logs.deploy-lambda-dynamodb-event-source-mapping-created|_deploy_cfn#5|KeyError|outputs = {o["OutputKey"]: o["OutputValue"] for o in stack.get("Outputs", [])}
Issue-Key: cloudformation__ecs__logs|ListTasks|ServiceNotFoundException
Issue-Commit: 048778663a26e363156c6db724999c4fb229af36

# CloudFormation reports CREATE_COMPLETE for Logs/CloudWatch/ECS resources it never provisions, and invalid resource properties never fail the resource or roll the stack back (4 causes)

> _Note: this issue was identified against the code base at commit `048778663a26e363156c6db724999c4fb229af36`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateStack

## Description
Four net-new root causes in the CloudFormation emulator, all observed through log/metric stacks. ALL FOUR must be closed before submitting. Related open issues describe the same shape for other resource types - 20261022T235108Z-cloudformation-stack-outputs-carry-no-outputvalue-failing-resources.md, 20261023T004328Z-...-cloudformation-stacks-declaring-aws-lambda-function-reach-create.md, 20261022T234922Z-cloudwatch-ecr-ecs-a-cloudformation-stack-declaring-aws-ecr-repository-aws.md, 20261023T025808Z-cloudwatch-ecs-elbv2-logs-cfn-deployed-ecs-alb-observability-stacks-are-unusable.md - none of them carries the Logs/CloudWatch resource types or the check ids below.

Cause 1 of 4: AWS::Logs::LogGroup / AWS::Logs::LogStream / AWS::Logs::SubscriptionFilter declared by a template are never created in CloudWatch Logs.
AWS: CreateStack provisions each declared resource through the owning service before the stack reaches CREATE_COMPLETE; after that, DescribeLogGroups lists the group, PutLogEvents into it succeeds, and DescribeSubscriptionFilters resolves it. A RetainPolicy/Retain deletion policy keeps the real group and its events.
Emulator: the stack reaches CREATE_COMPLETE but nothing exists in Logs - PutLogEvents answers 400 ResourceNotFoundException 'The specified log group does not exist', DescribeSubscriptionFilters answers 400 ResourceNotFoundException, CreateLogStream answers ResourceNotFoundException, and DescribeLogGroups does not list the stack's group.
Why: every infrastructure-as-code workflow that declares its log destination is broken: the stack says success while the resource a client immediately writes to does not exist.
Fix: the resource-provisioning dispatch for the AWS::Logs::* types in the cloudformation service crate (call the logs service's create paths and record the real physical id).
Traces: logs.deploy-log-group-cfn-then-put-events-succeeds, logs.deploy-log-group-cfn-rename-replaces-group-and-drops-events, logs.deploy-log-group-cfn-retain-policy-keeps-group-and-events, logs.deploy-logs-1-kms-log-group, logs.deploy-subscription-filter-cfn-then-forwards-to-destination, cloudformation__logs.log-group-invalid-retention-update-rejected-rolls-back, cloudwatch__lambda__logs.cfn-deploy-logs-subscription-filter-lambda-positive, apigateway__dynamodb__lambda__logs.deploy-cfn-lambda-log-group-retention-positive, apigateway__dynamodb__lambda__logs.deploy-log-retention-set-via-template.

Cause 2 of 4: a resource whose properties the owning service refuses does not fail, so the stack never rolls back.
AWS: an invalid property is refused by the owning service during provisioning, the resource goes CREATE_FAILED and the stack ends ROLLBACK_COMPLETE (or UPDATE_ROLLBACK_COMPLETE for an update, keeping the previous resource state). Examples exercised: AWS::Logs::LogGroup RetentionInDays=4 (not one of the allowed retention values), AWS::Logs::MetricFilter FilterPattern '{ $.latency = }' (malformed), AWS::Logs::LogStream whose LogGroupName does not exist.
Emulator: the stack ends CREATE_COMPLETE / UPDATE_COMPLETE and the invalid resource is simply ignored.
Why: a template error that AWS surfaces as a failed deployment deploys 'successfully' here, so the defect is only discovered at runtime - and an update that AWS rolls back leaves the emulator's stack claiming the new, invalid configuration.
Fix: propagate the owning service's refusal out of resource provisioning in the cloudformation service crate and drive the stack's rollback state machine from it.
Traces: logs.deploy-log-group-cfn-invalid-retention-rejected, logs.deploy-log-stream-cfn-into-missing-group-rejected, cloudformation__cloudwatch__logs.deploy-logs-3-invalid-filter-pattern-rejected-rolls-back, cloudformation__cloudwatch__logs.deploy-logs-3-invalid-pattern-update-rejected-keeps-old-filter.

Cause 3 of 4: AWS::CloudWatch::Alarm declared by a template is never created in CloudWatch.
AWS: after CREATE_COMPLETE, DescribeAlarms returns the declared alarm (and an alarm with an invalid metric configuration fails the stack).
Emulator: DescribeAlarms returns nothing for the stack's alarm - the checks fail with IndexError on the empty MetricAlarms list and KeyError on the alarm name.
Why: alarm-based observability cannot be deployed declaratively; the stack reports success with no alarm in existence.
Fix: the resource-provisioning dispatch for AWS::CloudWatch::Alarm in the cloudformation service crate.
Traces: cloudwatch__lambda__logs.cfn-deploy-lambda-cloudwatch-alarm-positive, cloudwatch__lambda__logs.cfn-deploy-alarm-invalid-metric-rejected, cloudwatch__ecs__elbv2__logs.deploy-cloudwatch-alarm-created-for-alb-5xx.

Cause 4 of 4: stack Outputs carry no OutputValue, so a stack's exported resource identifiers are unusable.
AWS: DescribeStacks returns each declared Output with both OutputKey and OutputValue (e.g. a DynamoDB table StreamArn, an ECS service name), resolved from the provisioned resource.
Emulator: the Output entry has no OutputValue member at all (KeyError: 'OutputValue'), and an ECS service named by a stack is unknown to ECS (ListTasks answers ServiceNotFoundException 'Service not found.').
Why: any consumer that reads a stack Output to find the resource it must talk to crashes or addresses a resource that does not exist.
Fix: Output resolution in the cloudformation service crate (populate OutputValue from the provisioned physical resource) - the same defect the open issue 20261022T235108Z describes for other services.
Traces: apigateway__dynamodb__lambda__logs.deploy-dynamodb-table-stream-arn-exported-correctly, apigateway__dynamodb__lambda__logs.deploy-lambda-dynamodb-event-source-mapping-created, cloudformation__ecs__logs.deploy-ecs-2-cfn-circuit-breaker-rolls-stack-back-to-last-good-token.

## Evidence
Cause 1: botocore.errorfactory.ResourceNotFoundException: An error occurred (ResourceNotFoundException) when calling the PutLogEvents operation: The specified log group does not exist (after the declaring stack reached CREATE_COMPLETE); ... when calling the DescribeSubscriptionFilters operation: The specified log group does not exist; ... when calling the CreateLogStream operation: The specified log group does not exist; AssertionError: log group 'sim2real-xsvc-lgret-042c66c321f9' not found; AssertionError: expected exactly one log group named /aws/lambda/sim2real-xsvc-fn-665cb762131b, got [].
Cause 2: AssertionError: stack with invalid RetentionInDays=4 should roll back, got CREATE_COMPLETE; AssertionError: stack with a LogStream into a missing group ended 'CREATE_COMPLETE', expected ROLLBACK_COMPLETE; AssertionError: stack with FilterPattern '{ $.latency = }' ended 'CREATE_COMPLETE'; real AWS fails the MetricFilter and rolls back to ROLLBACK_COMPLETE; AssertionError: update to FilterPattern '{ $.latency = }' ended 'UPDATE_COMPLETE'; real AWS ... UPDATE_ROLLBACK_COMPLETE.
Cause 3: IndexError: list index out of range (DescribeAlarms['MetricAlarms'][0]); KeyError: 'sim2real-xsvc-invalidalarm-5e057ae2f703'.
Cause 4: KeyError: 'OutputValue'; botocore.errorfactory.ServiceNotFoundException: An error occurred (ServiceNotFoundException) when calling the ListTasks operation: Service not found.

## Steps to Reproduce
Cause 1: deploy a template declaring AWS::Logs::LogGroup (and a LogStream/SubscriptionFilter on it); wait for CREATE_COMPLETE; call PutLogEvents / DescribeSubscriptionFilters / DescribeLogGroups for that group -> ResourceNotFoundException / absent.
Cause 2: deploy the same template with RetentionInDays=4, or a MetricFilter with FilterPattern '{ $.latency = }', or a LogStream whose LogGroupName does not exist -> CREATE_COMPLETE instead of ROLLBACK_COMPLETE.
Cause 3: deploy a template declaring AWS::CloudWatch::Alarm; after CREATE_COMPLETE call DescribeAlarms -> empty.
Cause 4: deploy a template with Outputs (e.g. a table StreamArn) and read DescribeStacks -> Output entry has no OutputValue.
