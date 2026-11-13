Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: logs.op-contract-describeLogGroups
Issue-Shape-Id: com.amazonaws.cloudwatchlogs#Logs_20140328
Issue-Key: logs.op-contract-describeLogGroups|run#14|AssertionError|assert message == (
Issue-Commit: 9ca58d7cf30a6ec744c8cbd01b69502e157676e2

# DescribeLogGroups' InvalidParameterException message for logGroupIdentifiers combined with another filter is not AWS's wording

> _Note: this issue was identified against the code base at commit `9ca58d7cf30a6ec744c8cbd01b69502e157676e2`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeLogGroups

## Description
Cause 1 of 1: DescribeLogGroups correctly refuses a request that passes logGroupIdentifiers together with logGroupNamePrefix, and correctly uses InvalidParameterException, but the error message text differs from AWS's. Failing check: logs.op-contract-describeLogGroups.

AWS: Smithy (services/service-com-amazonaws-cloudwatchlogs-logs-20140328): DescribeLogGroups input carries accountIdentifiers, logGroupNamePrefix, logGroupNamePattern, nextToken, limit, includeLinkedAccounts, logGroupClass and logGroupIdentifiers (list, length 1..50), and models InvalidParameterException with httpError 400. Combining logGroupIdentifiers with any other input filter is refused with code InvalidParameterException and message "No other input filter parameter can be provided with LogGroupIdentifiers" — the whole sentence is fixed text; it interpolates nothing from the request (the member name appears in it as the literal `LogGroupIdentifiers`). Status 400, per the modelled httpError.

Emulator: the call is refused with InvalidParameterException (correct) but the message is 'If you specify logGroupIdentifiers, the only other filter that you can choose to specify is includeLinkedAccounts' — different fixed text, and it describes a different (prose) rule rather than AWS's refusal sentence.

Why: an error message is part of the contract clients match on. Tooling and tests that assert on, log or surface the exact refusal text (a very common pattern for parameter-combination errors, which carry no structured member field) see a string that never occurs on AWS, so the same code path behaves differently against the emulator and against AWS.

Fix: the DescribeLogGroups handler's input validation in the cloudwatchlogs crate — the branch that rejects logGroupIdentifiers combined with another input filter must emit AWS's fixed message; the error shape, code and 400 status it already uses are correct. The rest of the check's assertions (the created group returned once with its logGroupName, storedBytes 0 and an arn ending in `log-group:<name>:*`) passed.

Traces: logs.op-contract-describeLogGroups. No open issue covers this message; the open DescribeLogGroups issue 20261106T184140Z-logs-describeloggroups-reports-live-summed-storedbytes-instead-of-the.md is about storedBytes metering, a different cause.

## Evidence
Expected (AWS): logs.DescribeLogGroups(logGroupIdentifiers=[<arn of the group>], logGroupNamePrefix=<that group's name>) -> HTTP 400, Error.Code = "InvalidParameterException", Error.Message = "No other input filter parameter can be provided with LogGroupIdentifiers" (fixed text, no per-request values).

Actual (emulator): Error.Code = "InvalidParameterException" (correct) but
  AssertionError: unexpected message: 'If you specify logGroupIdentifiers, the only other filter that you can choose to specify is includeLinkedAccounts'

## Steps to Reproduce
1. logs.CreateLogGroup(logGroupName=<unique>, tags={"Sim2RealOwner":"behavioral-suite"}).
2. logs.DescribeLogGroups(logGroupNamePrefix=<that name>) and take logGroups[0].logGroupArn (the group itself is returned correctly, with storedBytes 0 and arn ending in `log-group:<name>:*`).
3. logs.DescribeLogGroups(logGroupIdentifiers=[<that logGroupArn>], logGroupNamePrefix=<that name>).
4. Expected: InvalidParameterException with message "No other input filter parameter can be provided with LogGroupIdentifiers". Observed: InvalidParameterException with message "If you specify logGroupIdentifiers, the only other filter that you can choose to specify is includeLinkedAccounts".
