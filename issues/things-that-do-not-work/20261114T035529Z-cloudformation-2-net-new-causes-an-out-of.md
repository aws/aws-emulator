Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation.cdk-diff-sees-template-change-not-out-of-band-drift cloudformation.cli-script-cfn-deploy-and-verify
Issue-Shape-Id: com.amazonaws.cloudformation#CloudFormation
Issue-Key: cloudformation.cdk-diff-sees-template-change-not-out-of-band-drift|run#35|AssertionError|assert rc == 1, (
Issue-Key: cloudformation.cli-script-cfn-deploy-and-verify|_run_shell#6|Timeout|result = subprocess.run(
Issue-Commit: d96d9e0689f67235cf5db5594b0e3f429b7ae97b

# cloudformation: 2 net-new causes — an out-of-band resource property change is never reported as stack drift, and the CLI deploy-and-verify flow never returns a verdict

> _Note: this issue was identified against the code base at commit `d96d9e0689f67235cf5db5594b0e3f429b7ae97b`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DetectStackDrift

## Description
Two net-new root causes in the CloudFormation service, both grounded in failing behavioral checks that no open issue carries the replay key for.

Cause 1 of 2: drift detection reports IN_SYNC / "no drift" for a stack whose live resource property was changed outside CloudFormation.
Failing check: cloudformation.cdk-diff-sees-template-change-not-out-of-band-drift
  AWS:      Smithy (cloudformation): DetectStackDrift input StackName (required, length min 1, pattern ^([a-zA-Z][-a-zA-Z0-9]*)|(arn:...)$) and output StackDriftDetectionId (required); DescribeStackDriftDetectionStatus reports StackDriftStatus from the enum DRIFTED | IN_SYNC | UNKNOWN | NOT_CHECKED; DescribeStackResourceDrifts output StackResourceDrifts (required) carries, per resource, StackResourceDriftStatus from IN_SYNC | MODIFIED | DELETED | NOT_CHECKED | UNKNOWN | UNSUPPORTED. The model documents MODIFIED as "One or more resource properties differ from their expected values (as defined in the stack template and any values specified as template parameters)", and PropertyDifferences (present only for MODIFIED resources) as PropertyPath + ExpectedValue + ActualValue + DifferenceType, with DifferenceType NOT_EQUAL for "The current property value differs from its expected value". So when a property of a resource that the template declares is changed out of band, AWS must detect that resource as MODIFIED and the stack as DRIFTED, and must not count that resource as unchecked. The CDK drift command reads exactly this API and exits non-zero under --fail when any resource is drifted.
  Emulator: the live table's point-in-time-recovery setting was switched on out of band while the stack template declares it off. Drift detection finds nothing: `cdk drift --fail` exits 0 and its output says "No drift detected" and "Number of resources with drift: 0 (1 unchecked)" — the declared resource is reported as unchecked instead of being compared against its live configuration, so the stack never becomes DRIFTED and no PropertyDifference with ExpectedValue/ActualValue is produced. (The resource type and property here are just this recording's example; the requirement is that any template-declared property changed out of band is compared.)
  Why:      a client relying on AWS's behavior uses drift detection as the guard that a stack's live estate still matches its template — CI gates, `cdk drift --fail`, and remediation tooling all key off DRIFTED/MODIFIED plus ExpectedValue/ActualValue. An emulator that always answers "no drift" turns that guard into an unconditional pass, so unmanaged changes are reported as safe.
  Fix:      the drift-detection path of the CloudFormation service: the DetectStackDrift / DescribeStackDriftDetectionStatus / DescribeStackResourceDrifts handlers and the per-resource comparison they drive, which must read each declared resource's live configuration from the owning service's state and classify it rather than leaving it unchecked.
  Traces:   cloudformation.cdk-diff-sees-template-change-not-out-of-band-drift. Open issue 20261029T171940Z-cloudformation-drift-reporting-returns-live-resource-attributes-and.md describes a related drift-reporting defect but does not carry this check's replay key, so this cause is filed here.

Cause 2 of 2: the CLI deploy-and-verify flow never produces a response — the check is killed at its budget with no AWS reply at all.
Failing check: cloudformation.cli-script-cfn-deploy-and-verify
  AWS:      recorded behavior only. The recording drives `aws cloudformation` deploy-then-read-back from a shell script; every call returns and the stack reaches a terminal status, so the check can assert on the deployed stack. The replay output carries no HTTP status and no error code, because nothing was returned.
  Emulator: the check yields only "Check timed out after 60.0s". Some call in the deploy-and-verify sequence never completes (or the stack never leaves a non-terminal status), so no status, error code or message is ever served for it.
  Why:      a client that deploys a stack and then waits for it blocks indefinitely instead of being told the outcome; a CLI wrapper or CI step built on this sequence hangs rather than succeeding or failing.
  Fix:      the CloudFormation stack create/update provisioning loop and the status-transition path that the describe calls read, in the cloudformation service — the request that does not complete must answer, and the stack must reach a terminal status.
  Traces:   cloudformation.cli-script-cfn-deploy-and-verify. Note the only diagnosis the replay gives for this cause is the timeout itself; no response was served to classify.

## Evidence
Cause 1 — expected: a stack whose declared resource property was changed out of band detects as DRIFTED, with that resource StackResourceDriftStatus=MODIFIED and a PropertyDifference carrying PropertyPath/ExpectedValue/ActualValue and DifferenceType=NOT_EQUAL; `cdk drift --fail` exits 1 with a non-zero drifted-resource count. Actual (replay): AssertionError: "`cdk drift --fail` exited 0 while the deployed table's PITR was ENABLED and the template says disabled, expected 1. Output tail: '... No drift detected ... Number of resources with drift: 0 (1 unchecked)'".

Cause 2 — expected: every call in the deploy-and-verify script returns and the stack reaches a terminal status, letting the check assert on it. Actual (replay): "Error: Check timed out after 60.0s" — no HTTP status, error code or message was served.

## Steps to Reproduce
Cause 1: create a stack that declares a resource with a property set to a known value; after CREATE_COMPLETE, change that same property on the live resource through the owning service's API (out of band); call DetectStackDrift for the stack, poll DescribeStackDriftDetectionStatus to completion and call DescribeStackResourceDrifts. Expected StackDriftStatus=DRIFTED with the resource MODIFIED and a PropertyDifference (ExpectedValue = template value, ActualValue = live value, DifferenceType=NOT_EQUAL); observed no drift and the resource counted as unchecked. Equivalently, run `cdk drift --fail` against the deployed stack: expected exit 1, observed exit 0.

Cause 2: run the behavioral check cloudformation.cli-script-cfn-deploy-and-verify (a shell script that runs `aws cloudformation deploy` and then reads the stack back). Expected each call to return and the stack to reach a terminal status; observed the whole flow never completing inside the 60 s check budget.
