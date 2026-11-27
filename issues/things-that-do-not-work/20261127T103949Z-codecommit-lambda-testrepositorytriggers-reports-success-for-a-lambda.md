Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: codecommit__lambda.trigger-to-function-without-codecommit-grant-rejected
Issue-Key: codecommit__lambda.trigger-to-function-without-codecommit-grant-rejected|run#17|AssertionError|assert tested["successfulExecutions"] == ["granted"], (
Issue-Commit: ab3264a14a30dede061951437467343aaef14e5b

# codecommit__lambda: TestRepositoryTriggers reports success for a Lambda destination whose resource policy does not let CodeCommit invoke it

> _Note: this issue was identified against the code base at commit `ab3264a14a30dede061951437467343aaef14e5b`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
TestRepositoryTriggers

## Description
Cause 1 of 1: TestRepositoryTriggers does not evaluate a Lambda destination's resource policy, so a trigger that CodeCommit is not permitted to invoke is reported as a successful execution.
AWS: Smithy TestRepositoryTriggers returns successfulExecutions and failedExecutions (members trigger, failureMessage). A repository trigger can reach a Lambda function only when the function's resource policy grants lambda:InvokeFunction to the codecommit.amazonaws.com principal for that repository ARN (https://docs.aws.amazon.com/codecommit/latest/userguide/how-to-notify-lambda.html). With two identical triggers — one pointing at a function that carries such a grant, one at a function with no resource policy at all — AWS answers successfulExecutions == ['granted'] and failedExecutions == [{'trigger': 'withheld', 'failureMessage': "AWS CodeCommit does not have access to the destination, the destination's KMS key or the destination does not exist."}] (the failureMessage is entirely fixed text). After AddPermission grants the second function, the identical TestRepositoryTriggers call reports both in successfulExecutions and no failures.
Emulator: both triggers come back in successfulExecutions (['granted', 'withheld']) with failedExecutions == [], so the missing invoke grant is never reported. PutRepositoryTriggers/GetRepositoryTriggers stored both triggers correctly, so only the authorization evaluation during the test call is missing.
Why: the whole purpose of TestRepositoryTriggers is to prove, before the trigger is relied on, that the destination is reachable. Reporting a function CodeCommit may not invoke as 'successful' makes a misconfigured notification path look healthy; the subsequent grant-then-retest workflow (which is how operators confirm a fix) also cannot be distinguished from the broken state.
Fix: the destination-authorization evaluation in services/service-com-amazonaws-codecommit-codecommit-20150413/src/ops/test_repository_triggers.rs — resolve a lambda destinationArn against that function's resource policy for the codecommit.amazonaws.com principal and the repository ARN, placing the trigger in failedExecutions with AWS's fixed no-access failureMessage when no statement permits the invoke.
Traces: codecommit__lambda.trigger-to-function-without-codecommit-grant-rejected.

## Evidence
codecommit__lambda.trigger-to-function-without-codecommit-grant-rejected [mismatch]: AssertionError 'the granted twin did not succeed: {\'successfulExecutions\': [\'granted\', \'withheld\'], \'failedExecutions\': [], ... HTTPStatusCode: 200}' at `assert tested['successfulExecutions'] == ['granted']`. Expected: successfulExecutions == ['granted'] and failedExecutions == [('withheld', "AWS CodeCommit does not have access to the destination, the destination's KMS key or the destination does not exist.")]; then lambda.get_policy on the ungranted function answers ResourceNotFoundException 'The resource you requested does not exist.', and after lambda.add_permission(Action='lambda:InvokeFunction', Principal='codecommit.amazonaws.com', SourceArn=<repo arn>) the retest reports both functions successful with no failures. Actual: both triggers reported successful on the first call, so the grant/no-grant distinction is invisible and the assertions behind it are unverified. Function and repository names are per-run values.

## Steps to Reproduce
1. Boot the emulator and point botocore at it.
2. Create an execution role and two Lambda functions, GRANTED and WITHHELD.
3. cc.create_repository(repositoryName=R) and note repositoryMetadata.Arn.
4. lambda.add_permission(FunctionName=GRANTED, StatementId='cc', Action='lambda:InvokeFunction', Principal='codecommit.amazonaws.com', SourceArn=<repo arn>, SourceAccount=<account>); leave WITHHELD with no resource policy.
5. cc.put_repository_triggers(repositoryName=R, triggers=[{'name':'granted','destinationArn':<GRANTED arn>,'events':['all'],'branches':[]}, {'name':'withheld','destinationArn':<WITHHELD arn>,'events':['all'],'branches':[]}]).
6. cc.test_repository_triggers(repositoryName=R, triggers=<the same two>) -> expect successfulExecutions == ['granted'] and failedExecutions == [{'trigger':'withheld','failureMessage':"AWS CodeCommit does not have access to the destination, the destination's KMS key or the destination does not exist."}]; observe both names in successfulExecutions and failedExecutions empty.
7. lambda.add_permission(...) for WITHHELD, then repeat step 6 -> expect both in successfulExecutions and failedExecutions == [].
8. Re-replay codecommit__lambda.trigger-to-function-without-codecommit-grant-rejected.
