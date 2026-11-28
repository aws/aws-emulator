Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__cloudwatch__route53.deploy-route53-healthcheck-invalid-insufficient-data-status-rejected
Issue-Key: cloudformation__cloudwatch__route53.deploy-route53-healthcheck-invalid-insufficient-data-status-rejected|run#21|AssertionError|assert got == _REJECTED, f"handler reason {got!r}, expected {_REJECTED!r}"
Issue-Commit: 801cadf1810ef429e63e86d31482b11009b2d2b5

# cloudformation__route53: the reason a stack records for an `AWS::Route53::HealthCheck` whose `InsufficientDataHealthStatus` is outside the modelled set is worded differently from the recorded one

> _Note: this issue was identified against the code base at commit `801cadf1810ef429e63e86d31482b11009b2d2b5`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateStack (the `AWS::Route53::HealthCheck` resource provider's refusal reporting)

## Description
A stack that declares an `AWS::Route53::HealthCheck` whose `HealthCheckConfig.InsufficientDataHealthStatus` is a value outside the set that member admits is correctly REFUSED: the resource fails, the stack rolls back, and nothing is created in Route 53. What diverges is the SENTENCE the failed resource's `ResourceStatusReason` carries, which is what a client reads to learn what to fix.

The emulator quotes the Route 53 service's own constraint-violation envelope inside the handler envelope — `Resource handler returned message: "1 validation error detected: Value '<value>' at 'healthCheckConfig.insufficientDataHealthStatus' failed to satisfy constraint: Member must satisfy enum value set: [...]" (…)`, built by `service-com-amazonaws-route53-awsdnsv20130401`'s `override_refusal` and wrapped by the CloudFormation provisioner's SDK diagnostic (`src/state/provisioning/sdk_diagnostics.rs`). The recorded reason for this check is worded differently, so the check's `assert got == _REJECTED` fails on the reason even though the refusal itself is due.

Two wordings are plausible for this class and the recording decides between them, so a fixer should read the recorded reason through the replay harness before changing anything:

* the owning service's own refusal, quoted as it came (what the emulator does now), or
* the provider's own MODEL VALIDATION of the declaration, which refuses a property whose value the type's schema constrains to a published set before any call is made — the stage this crate already implements for other types in `src/state/provisioning/model_validation.rs` (`Model validation failed (#/<property>: failed validation constraint for keyword [enum])`), whose catalogue carries no row for this type.

Whichever it is, the HTTP status, the error code and the quoted sentence are requirements, and the `HandlerErrorCode` the envelope closes with belongs to the same answer.

## Why it matters
A client that deploys a template with an unusable enum value reads the reason to find the property to correct. A reason worded for a different stage of the deployment names a different place to look, and an assertion that gates the rest of a deploy-then-correct flow on that sentence reads the wrong one.

## Evidence
The rejection arm of `cloudformation__cloudwatch__route53.deploy-route53-healthcheck-invalid-insufficient-data-status-rejected` now reaches its own negative assertion (its positive control alarm is created, which was the earlier cause, fixed) and fails on `assert got == _REJECTED` at run#21, with the emulator's reason opening `1 validation error detected: Value '<value>' at 'healthCheckConfig.insufficientDataHealthStatus' failed to satisfy constraint: Member must satisfy enum value set: [...] (Service: Route53, Status Code: 400, Request ID: …) (SDK Attempt Count: …)`.

## Steps to Reproduce
1. `CreateStack` with a template declaring an `AWS::Route53::HealthCheck` whose `HealthCheckConfig` carries `InsufficientDataHealthStatus` set to a value outside the set that member admits.
2. Wait for the stack to settle (it rolls back).
3. Read `DescribeStackEvents` and take the `ResourceStatusReason` of the health check's own failed event.
4. Compare that sentence with the recorded one.
