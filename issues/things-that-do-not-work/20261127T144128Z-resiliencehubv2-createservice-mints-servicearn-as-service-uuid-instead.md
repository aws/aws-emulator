Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: resiliencehubv2.put-test-sources-rejects-success-alarm-the-invoker-role-cannot-read
Issue-Shape-Id: com.amazonaws.resiliencehubv2#NGRHServiceCore
Issue-Key: resiliencehubv2.put-test-sources-rejects-success-alarm-the-invoker-role-cannot-read|run#12|AssertionError|assert re.fullmatch(svc_shape, svc), "the service ARN is not service/<name>:<id> in this account"
Issue-Commit: fc909f682d274824e0181cbb84eb6914c2fe111f

# resiliencehubv2 CreateService mints serviceArn as service/<uuid> instead of service/<name>:<6-char id>, so every service ARN loses the service name

> _Note: this issue was identified against the code base at commit `fc909f682d274824e0181cbb84eb6914c2fe111f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateService

## Description
One net-new root cause in resiliencehubv2.

Cause 1 of 1: the resource segment of a minted service ARN has the wrong form, so the ARN does not name the service that was created.
Failing check ids: resiliencehubv2.put-test-sources-rejects-success-alarm-the-invoker-role-cannot-read

AWS: Smithy (services/service-com-amazonaws-resiliencehubv2-ngrhservicecore/smithy-model/com-amazonaws-resiliencehubv2-ngrhservicecore.json) models CreateService.name as EntityName (@length 2..60, @pattern ^[A-Za-z0-9][A-Za-z0-9_\-]{1,59}$) and documents it as "Resource name (used in ARN — no spaces allowed)"; Service$serviceArn targets Arn (@length min 31, the generic ^arn:(aws|...):...:[0-9]{12}:[A-Za-z0-9/][A-Za-z0-9:_/+.-]{0,1023}$ pattern), and the model's IamServiceResource fixes the resource type token as `service`. The model does not spell out the resource segment's internal layout beyond that, so for the layout this is recorded behavior only: the recording shows AWS returning a serviceArn of the general form arn:aws:resiliencehub:<region>:<account>:service/<name>:<id>, where <name> is the `name` the request supplied and <id> is a 6-character lower-case alphanumeric ([a-z0-9]{6}) suffix the service mints. Both parts are template, not literals: <region> and <account> are the caller's, <name> comes from the request, <id> is freshly minted per create (so re-creating a service under a previously used name yields a different ARN).

Emulator: CreateService returns serviceArn = arn:aws:resiliencehub:<region>:<account>:service/<uuid> — the resource segment is a bare version-4 UUID with no name and no 6-character suffix, e.g. the replay's `arn:aws:resiliencehub:us-west-2:000000000000:service/100f4f65-f80e-461d-8531-07b39fc0f08c` (that specific uuid/account/region is one run's fill, not the requirement). The check asserts the returned ARN matches arn:aws:resiliencehub:<region>:<account>:service/<requested-name>:[a-z0-9]{6} and fails with "the service ARN is not service/<name>:<id> in this account"; the HTTP status is 200 and no error is involved — only the minted value diverges. Because the ARN is minted before the rest of the check runs, the PutTestSources success-criteria-alarm permission assertions in the same check were never reached, so this report says nothing yet about those; they are expected to be re-examined after the ARN form is corrected.

Why: the serviceArn is the service's identity and is the only handle clients, IAM policies, CloudFormation templates and operator tooling have for it. A client that derives the service name from the ARN, matches an ARN against an IAM resource pattern such as arn:aws:resiliencehub:*:*:service/<name>:*, or correlates a log line or an event ARN back to a named service, gets nothing usable from the emulator: the name is absent from the ARN entirely. ARN-prefix-scoped IAM policies and name-scoped resource matching therefore behave differently against the emulator than against AWS, and tests pinned to AWS's ARN form fail for a reason unrelated to the behavior under test.

Fix: the service ARN minting path — the ARN helper that CreateService calls to produce the record's identity (it must interpolate the request's `name` plus a freshly minted 6-character lower-case alphanumeric id instead of a UUID), together with the ARN-parsing/resolution path that reads a service's resource segment back, so GetService / UpdateService / DeleteService / PutTestSources / ListTestSources still resolve ARNs of the new form, and the ARN-shape validation that decides whether an ARN names a `service` resource accepts a resource segment containing the `<name>:<id>` colon.

Traces: resiliencehubv2.put-test-sources-rejects-success-alarm-the-invoker-role-cannot-read. Code citations: services/service-com-amazonaws-resiliencehubv2-ngrhservicecore/src/state.rs:683-694 (mint_service_arn, documented as "arn:aws:resiliencehub:<region>:<account>:service/<uuid>"), src/state.rs:719-754 (mint_uuid), src/ops/create_service.rs:274 (the call site). Note: the open issues 20261125T192751Z-resiliencehubv2-test-surface-diverges-createtest-refuses-every-template.md and 20261125T185751Z-resiliencehubv2-createtest-refuses-with-a-blanket-multi-az.md concern CreateTest's refusal wording and do not cover the ARN form minted by CreateService.

## Evidence
Expected (recorded AWS behavior): CreateService(name=<name>, regions=[<region>], permissionModel={invokerRoleName:<role>}) -> 200 with service.serviceArn matching arn:aws:resiliencehub:<region>:<account>:service/<name>:[a-z0-9]{6} — the requested name followed by a colon and a freshly minted 6-character lower-case alphanumeric id.

Actual (emulator): 200 with service.serviceArn = arn:aws:resiliencehub:<region>:<account>:service/<uuid>, a bare lower-case 8-4-4-4-12 version-4 UUID and no name segment. Replay output:

  File "check_resiliencehubv2_put_test_sources_rejects_success_alarm_the_invoker_role_cannot_read.py", line 144, in run
    assert re.fullmatch(svc_shape, svc), "the service ARN is not service/<name>:<id> in this account"
  AssertionError: the service ARN is not service/<name>:<id> in this account

where svc_shape = rf"arn:aws:resiliencehub:{ctx.region}:{ctx.account}:service/{svc_name}:[a-z0-9]{{6}}". The same bare-UUID form is visible in other resiliencehubv2 replay output, e.g. CreateTest's refusal text quoting 'arn:aws:resiliencehub:us-west-2:000000000000:service/100f4f65-f80e-461d-8531-07b39fc0f08c' (that uuid, account and region are one run's values, not requirements).

Source: services/service-com-amazonaws-resiliencehubv2-ngrhservicecore/src/state.rs:688 `pub fn mint_service_arn(region: &str) -> String { ::engine::emulator_arn(ARN_SERVICE_NAMESPACE, region, &format!("{SERVICE_RESOURCE}/{}", mint_uuid(region))) }` — the request's `name` is never interpolated into the ARN.

## Steps to Reproduce
1. Boot the emulator and point a botocore resiliencehubv2 client at it (region <region>, emulator account).
2. iam.create_role(RoleName=<role>, AssumeRolePolicyDocument=<trust policy allowing resiliencehub.amazonaws.com to sts:AssumeRole>).
3. rh.create_service(name=<name>, regions=[<region>], permissionModel={"invokerRoleName": <role>}).
4. Read service.serviceArn from the 200 response.
   Expected: arn:aws:resiliencehub:<region>:<account>:service/<name>:<6 lower-case alphanumerics>.
   Actual: arn:aws:resiliencehub:<region>:<account>:service/<version-4 UUID> — no name, no 6-character suffix.
5. Equivalently, replay the behavioral check resiliencehubv2.put-test-sources-rejects-success-alarm-the-invoker-role-cannot-read: it fails at the serviceArn shape assertion before reaching its PutTestSources assertions.
