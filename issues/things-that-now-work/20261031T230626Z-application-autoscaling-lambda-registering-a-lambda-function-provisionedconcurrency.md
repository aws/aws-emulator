Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: application-autoscaling__lambda.min-capacity-pulls-lambda-provisioned-concurrency-up
Issue-Key: application-autoscaling__lambda.min-capacity-pulls-lambda-provisioned-concurrency-up|_poll_until#36|Timeout|time.sleep(interval_s)
Issue-Commit: 025abd355378a9dd6de39ac3439e25eecad5e6c9

# application-autoscaling__lambda: registering a lambda:function:ProvisionedConcurrency target never drives Lambda's provisioned concurrency, so the floor/ceiling scenario never reaches a terminal state

> _Note: this issue was identified against the code base at commit `025abd355378a9dd6de39ac3439e25eecad5e6c9`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
RegisterScalableTarget

## Description
Cause 1 of 1: RegisterScalableTarget in the `lambda` namespace does not enforce the registered bounds on the target resource.
AWS: per the operation's own modeled documentation (Smithy: RegisterScalableTarget doc — "If the specified resource is not active in the target service, this operation does not change the resource's current capacity. Otherwise, it changes the resource's current capacity to a value that is inside of this range" and "Application Auto Scaling retrieves the current capacity of the resource. If it's below the minimum capacity or above the maximum capacity, Application Auto Scaling adjusts the capacity of the scalable target to place it within these bounds"), registering an alias `function:<name>:<ALIAS>` on dimension `lambda:function:ProvisionedConcurrency` with MinCapacity above the alias's live provisioned concurrency (0) makes Application Auto Scaling raise it with NO scaling policy present: Lambda's GetProvisionedConcurrencyConfig for that qualifier reaches Status READY with Requested = Allocated = Available = MinCapacity, and DescribeScalingActivities records one terminal activity with StatusCode `Successful`, Cause `minimum capacity was set to <MinCapacity>` (fixed text `minimum capacity was set to ` + the registered MinCapacity), a Description naming that number, a StatusMessage naming the fulfilling target service (`lambda`), and ScalableDimension `lambda:function:ProvisionedConcurrency`. Re-registering with a MaxCapacity BELOW the live allocation pushes Lambda's pool back down to that ceiling and records a second, distinct activity with Cause `maximum capacity was set to <MaxCapacity>`.
Emulator: the scenario never reaches any of those assertions. No assertion fired and no error was returned — the check was killed by the harness after 60s while inside one of its bounded polls, i.e. the condition it waits for never became true even though every API call was answered. Hypothesis: the lambda-namespace registration is recorded in the Application Auto Scaling store only and is never propagated to Lambda's provisioned-concurrency state, so the GetProvisionedConcurrencyConfig poll (and the terminal-scaling-activity poll behind it) spins forever; the earlier polls in the scenario (CreateFunction accepting a fresh role, the function reaching State Active) are the other candidates and the suite report does not isolate which one hung.
Why: a client that relies on Application Auto Scaling to hold a Lambda alias at a provisioned-concurrency floor sees the scalable target registered successfully while Lambda keeps 0 provisioned concurrency, and sees no scaling activity ever reach a terminal StatusCode — so it waits indefinitely or concludes capacity was provisioned when it was not.
Fix: the lambda-namespace branch of the RegisterScalableTarget handler in the application-autoscaling service (services/service-com-amazonaws-applicationautoscaling-anyscalefrontendservice/src/ops/register_scalable_target.rs) — the bound-enforcement write into the target service's state (Lambda's provisioned-concurrency config for the alias) and the scaling-activity record it must create for both directions.
Related but distinct: open issue 20261024T232810Z-application-autoscaling-registerscalabletarget-in-the-lambda-namespace-reports-a-service.md covers the lambda service-linked RoleARN not being minted in IAM; this issue is about the capacity itself never moving and the scenario never terminating.
Traces: application-autoscaling__lambda.min-capacity-pulls-lambda-provisioned-concurrency-up

## Evidence
Suite verdict: [mismatch] application-autoscaling__lambda.min-capacity-pulls-lambda-provisioned-concurrency-up — "Check timed out after 60.0s". No ClientError and no AssertionError were raised, so every call the scenario made was answered; one of its bounded polls never saw its condition.

Expected after RegisterScalableTarget(ServiceNamespace='lambda', ResourceId='function:<fn>:BLUE', ScalableDimension='lambda:function:ProvisionedConcurrency', MinCapacity=2, MaxCapacity=10) with no scaling policy:
  - lambda GetProvisionedConcurrencyConfig(FunctionName=<fn>, Qualifier='BLUE') -> Status 'READY', RequestedProvisionedConcurrentExecutions=2, AllocatedProvisionedConcurrentExecutions=2, AvailableProvisionedConcurrentExecutions=2
  - DescribeScalingActivities(IncludeNotScaledActivities=True)[0] -> StatusCode 'Successful', Cause 'minimum capacity was set to 2', Description naming 2, StatusMessage naming 'lambda', ScalableDimension 'lambda:function:ProvisionedConcurrency'
Then after RegisterScalableTarget(..., MinCapacity=1, MaxCapacity=1):
  - GetProvisionedConcurrencyConfig -> Status 'READY', Requested=Allocated=1
  - a NEW activity (different ActivityId) with Cause 'maximum capacity was set to 1'
Actual: none of these conditions was ever observed; the scenario hung and was terminated at the 60s budget.

## Steps to Reproduce
1. Create an IAM role Lambda can assume and attach AWSLambdaBasicExecutionRole.
2. CreateFunction (python3.12, trivial zip), wait for State Active, PublishVersion, CreateAlias(Name='BLUE').
3. Confirm GetProvisionedConcurrencyConfig(FunctionName, Qualifier='BLUE') raises ProvisionedConcurrencyConfigNotFoundException (nothing provisioned yet).
4. RegisterScalableTarget(ServiceNamespace='lambda', ResourceId='function:<fn>:BLUE', ScalableDimension='lambda:function:ProvisionedConcurrency', MinCapacity=2, MaxCapacity=10) and create NO scaling policy.
5. Poll GetProvisionedConcurrencyConfig for Status READY with Allocated=2, and DescribeScalingActivities for one terminal activity. Both must become true; on the emulator neither does.
6. RegisterScalableTarget again with MinCapacity=1, MaxCapacity=1 and poll for Allocated=1 plus a second activity whose Cause is 'maximum capacity was set to 1'.
