Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__dynamodb.cfn-dynamodb-provisioned-throughput-drift-reports-expected-and-actual
Issue-Key: cloudformation__dynamodb.cfn-dynamodb-provisioned-throughput-drift-reports-expected-and-actual|run#16|AssertionError|assert drifted["StackDriftStatus"] == "DRIFTED", (
Issue-Commit: d96d9e0689f67235cf5db5594b0e3f429b7ae97b

# cloudformation__dynamodb: DetectStackDrift reports IN_SYNC after a table's ProvisionedThroughput was changed out of band, so no expected/actual property difference is ever produced

> _Note: this issue was identified against the code base at commit `d96d9e0689f67235cf5db5594b0e3f429b7ae97b`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DetectStackDrift

## Description
Cause 1 of 1: a stack whose live resource property was changed outside CloudFormation still detects as IN_SYNC.
Failing check: cloudformation__dynamodb.cfn-dynamodb-provisioned-throughput-drift-reports-expected-and-actual
  AWS:      Smithy (cloudformation): DetectStackDrift input StackName (required) / output StackDriftDetectionId (required); the detection result is read through DescribeStackDriftDetectionStatus, whose StackDriftStatus is one of DRIFTED | IN_SYNC | UNKNOWN | NOT_CHECKED, and through DescribeStackResourceDrifts, whose required StackResourceDrifts list carries per resource a StackResourceDriftStatus of IN_SYNC | MODIFIED | DELETED | NOT_CHECKED | UNKNOWN | UNSUPPORTED plus ExpectedProperties, ActualProperties and PropertyDifferences. The model documents MODIFIED as "One or more resource properties differ from their expected values (as defined in the stack template and any values specified as template parameters)" and each PropertyDifference as PropertyPath + ExpectedValue (the template value) + ExpectedValue's counterpart ActualValue (the live value) + DifferenceType=NOT_EQUAL for a changed value. So after a template-declared throughput property is changed directly on the table, the stack must report StackDriftStatus=DRIFTED and the table MODIFIED with a PropertyDifference whose PropertyPath names that property and whose ExpectedValue/ActualValue are the template and live values.
  Emulator: after the table's provisioned read capacity was changed out of band away from the value the template declares, the detection run completes but reports StackDriftStatus 'IN_SYNC'; no MODIFIED resource and no ExpectedValue/ActualValue pair is produced. (The specific property and numbers are this recording's example; the requirement is that any template-declared property changed out of band is compared and reported.)
  Why:      a client uses drift detection to learn that a managed table no longer matches its template and to see which property diverged and by how much. An emulator that answers IN_SYNC reports an unmanaged capacity change as compliant, and remediation that keys off PropertyDifferences has nothing to act on.
  Fix:      the CloudFormation drift-detection path — the DetectStackDrift / DescribeStackDriftDetectionStatus / DescribeStackResourceDrifts handlers and the per-resource-type comparison that must read the live DynamoDB table configuration out of state and diff it against the stack's declared properties.
  Traces:   cloudformation__dynamodb.cfn-dynamodb-provisioned-throughput-drift-reports-expected-and-actual. Open issue 20261109T172142Z-cloudformation-dynamodb-5-net-new-causes-fresh-stacks.md covers other DynamoDB/CFN causes (including fresh stacks detecting as DRIFTED) but does not carry this check's replay key, so this cause is filed here.

## Evidence
Expected: after changing the live table's provisioned read capacity away from the template's declared value, DescribeStackDriftDetectionStatus reports StackDriftStatus=DRIFTED and DescribeStackResourceDrifts reports the AWS::DynamoDB::Table as StackResourceDriftStatus=MODIFIED with a PropertyDifference (PropertyPath naming the throughput property, ExpectedValue = template value, ActualValue = live value, DifferenceType=NOT_EQUAL).
Actual (replay): AssertionError: "stack drift status: expected DRIFTED, got 'IN_SYNC'" — the detection run finishes clean, so no resource drift row and no expected/actual values exist.

## Steps to Reproduce
1. CreateStack with an AWS::DynamoDB::Table declaring PROVISIONED billing and a known ReadCapacityUnits; wait for CREATE_COMPLETE.
2. Call dynamodb UpdateTable on that table to set a different ReadCapacityUnits (out of band, not through the stack).
3. Call cloudformation DetectStackDrift for the stack, poll DescribeStackDriftDetectionStatus until the detection completes, then call DescribeStackResourceDrifts.
Expected StackDriftStatus=DRIFTED with the table MODIFIED and a PropertyDifference carrying the template value as ExpectedValue and the live value as ActualValue; observed StackDriftStatus='IN_SYNC' and no drift rows.
