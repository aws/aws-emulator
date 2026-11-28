Issue-Source: trace-sweep-divergence
Issue-Trace: elasticbeanstalk-environment-mutation-operations-not-found-errors
Issue-Step: task#11
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: elasticbeanstalk-environment-mutation-operations-not-found-errors
Issue-Shape-Id: com.amazonaws.elasticbeanstalk#AWSElasticBeanstalkService
Issue-Key: elasticbeanstalk|AssociateEnvironmentOperationsRole|4xx->4xx|$.ErrorResponse.Error.Message
Issue-Commit: 50a80390bb1e2ddab0fc937c65e59ad0dcd543f4

# Elastic Beanstalk AssociateEnvironmentOperationsRole validates the operations role before the environment exists

> _Note: this issue was identified against the code base at commit `50a80390bb1e2ddab0fc937c65e59ad0dcd543f4`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
AssociateEnvironmentOperationsRole

## Description
Cause 1 of 1: when AssociateEnvironmentOperationsRole names an environment that does not exist AND an operations role that does not exist, the emulator reports the role problem; AWS reports the missing environment.
AWS: Smithy (elasticbeanstalk, AssociateEnvironmentOperationsRole: input EnvironmentName (required, @length 4..40) + OperationsRole (required, @length 1..256), output Unit, modeled error InsufficientPrivilegesException/403). The not-found behavior is not modeled - recorded behavior only: the recording shows AWS answering HTTP 400 with the query-protocol ErrorResponse whose Error.Message is "No Environment found for EnvironmentName = '<EnvironmentName>'." The fixed text is "No Environment found for EnvironmentName = " plus the name in single quotes and a trailing period; the only variable part is the EnvironmentName copied from the request. AWS resolves the environment first, so a request that is wrong in both respects is rejected for the missing environment regardless of whether the supplied operations role exists.
Emulator: the same request answers 400 with the same error code but Message "Operations role ARN '<arn>' isn't valid. Specify an existing IAM role." - i.e. the handler validates OperationsRole before looking the environment up, so the environment-not-found message is never produced for this request. Only Error.Message diverges (expected 400, actual 400).
Why: a client distinguishes 'I targeted an environment that is gone' from 'my role is wrong' by the message, since both come back as 400 with the same code. Retry/repair logic keyed on the message (recreate or re-resolve the environment vs fix the IAM role) takes the wrong branch, and a test that asserts the AWS not-found wording fails against the emulator. The wrong remedy is also advertised to a human reading the error.
Fix: the AssociateEnvironmentOperationsRole handler's validation order - resolve EnvironmentName against state and raise the environment-not-found error before any check of the OperationsRole value. The role-validation message itself stays, for the case where the environment does exist.
Traces: elasticbeanstalk-environment-mutation-operations-not-found-errors#task#11.

Not covered by an open issue: the replay marks this cluster unowned (no stub_path, no owned_by). The nearest open elasticbeanstalk issues concern the environment data plane and DescribeConfigurationSettings, not this operation's error precedence.

## Evidence
Replay of trace set simparity-wire-aws-v1 (emulator up, 157 steps, 11 mismatched). Fixture elasticbeanstalk-environment-mutation-operations-not-found-errors, step task#11, service elasticbeanstalk, operation AssociateEnvironmentOperationsRole, expected 400 / actual 400, verdict mismatch, owned_by [], stub_path "":
  $.ErrorResponse.Error.Message: value_mismatch (expected "No Environment found for EnvironmentName = 'sim2real-probe-missing-env-b0211e0a'.", got "Operations role ARN 'arn:aws:iam::000000000000:role/sim2real-probe-missing-ops-role' isn't valid. Specify an existing IAM role.")
The quoted environment name and role ARN are the values this run's request carried; the requirement is the message template, with the EnvironmentName taken from the request.

Model grounding: Smithy elasticbeanstalk AssociateEnvironmentOperationsRole -> input AssociateEnvironmentOperationsRoleMessage{EnvironmentName (required, length 4..40), OperationsRole (required, length 1..256)}, output Unit, errors [InsufficientPrivilegesException, client, httpError 403, awsQueryError code InsufficientPrivilegesException]. No not-found error is modeled for this operation, so the 400 + "No Environment found for EnvironmentName = '...'" response is recorded behavior only.

## Steps to Reproduce
1. Clone the repo and boot the emulator built from it.
2. Replay the wire corpus (trace set simparity-wire-aws-v1), fixture elasticbeanstalk-environment-mutation-operations-not-found-errors.
3. At step task#11, call AssociateEnvironmentOperationsRole with an EnvironmentName that was never created and an OperationsRole ARN that names no IAM role.
4. Both AWS and the emulator answer HTTP 400 with the same error code; compare ErrorResponse.Error.Message. AWS returns "No Environment found for EnvironmentName = '<name>'." (the requested name substituted); the emulator returns "Operations role ARN '<arn>' isn't valid. Specify an existing IAM role."
5. Confirm the ordering by repeating with an environment that DOES exist and the same bogus role: the role message is then the correct answer, which shows the handler checks the role first.
