Issue-Source: trace-sweep-divergence
Issue-Trace: efs-mount-target-security-group-limits
Issue-Step: task#5
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: efs-mount-target-security-group-limits
Issue-Shape-Id: com.amazonaws.efs#MagnolioAPIService_v20150201
Issue-Key: elasticfilesystem|ModifyMountTargetSecurityGroups|4xx->2xx|$
Issue-Key: elasticfilesystem|ModifyMountTargetSecurityGroups|4xx->2xx|$headers.content-length
Issue-Key: elasticfilesystem|ModifyMountTargetSecurityGroups|4xx->2xx|$headers.x-amzn-errortype
Issue-Key: elasticfilesystem|ModifyMountTargetSecurityGroups|4xx->2xx|status
Issue-Key: elasticfilesystem|DescribeMountTargetSecurityGroups|2xx->2xx|$.SecurityGroups[*]
Issue-Commit: ae5d8a902292096d9ed4c36fc40ed704ee301e98

# EFS mount target security groups: per-interface limit not enforced and set in effect diverges

> _Note: this issue was identified against the code base at commit `ae5d8a902292096d9ed4c36fc40ed704ee301e98`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ModifyMountTargetSecurityGroups

## Description
Two causes, both in the EFS mount-target security-group path. The emulator applies any SecurityGroups list the boundary accepts (modelled list length 0..100) without enforcing the per-network-interface security-group quota, so an over-limit write succeeds AND replaces the set in effect — which the matching read then reports.

Cause 1 of 2: ModifyMountTargetSecurityGroups does not raise SecurityGroupLimitExceeded when the request names more security groups than the account's per-interface quota allows.
AWS: Smithy: ModifyMountTargetSecurityGroups models error SecurityGroupLimitExceeded (errorKind client, httpError 400) whose documentation reads "Returned if the number of SecurityGroups specified in the request is greater than the limit, which is based on account quota. Either delete some security groups or request that the account quota be raised." Input member SecurityGroups targets com.amazonaws.efs#SecurityGroups (list, @length 0..100, member @pattern ^sg-[0-9a-f]{8,40}$) — the modelled 0..100 is the API ceiling, NOT the quota, which is the VPC per-network-interface security-group limit (default 5). The recorded response for an over-limit request is HTTP 400 with body {"ErrorCode": "SecurityGroupLimitExceeded", "Message": "The maximum number of security groups per interface has been reached."}, a non-zero content-length and an x-amzn-errortype header naming the error. In that message the whole sentence is FIXED text — it carries no value from the request or from state (no mount target id, no count, no limit), so it is reproduced verbatim; the only request/state-derived parts of the response are the error-type header and body ErrorCode, both the literal error shape name SecurityGroupLimitExceeded.
Emulator: answers HTTP 204 with an empty body and no content-length / x-amzn-errortype header, i.e. the modelled success of the operation, and stores the over-limit list. services/service-com-amazonaws-efs-magnolioapiservice-v20150201/src/ops/modify_mount_target_security_groups.rs checks MountTargetNotFound and IncorrectMountTargetState only, then at the assignment of mount_target.security_groups writes the requested list unconditionally; its comment asserts "the count limit are modelled ... so the boundary has already refused ... an over-long list", which is only true of the 100-element API ceiling, not of the quota the error exists for. SecurityGroupLimitExceeded is in the generated error enum but is never constructed anywhere in the handler.
Why: a client that provisions mount-target security groups and relies on the 400 SecurityGroupLimitExceeded to detect quota exhaustion sees a success instead. It believes an ENI configuration AWS would have refused is in effect, never runs its back-off/quota-increase path, and any assertion on the error code or on that message text fails. The over-limit write also silently corrupts the set in effect (Cause 2).
Fix: the ModifyMountTargetSecurityGroups handler's validation in services/service-com-amazonaws-efs-magnolioapiservice-v20150201/src/ops/modify_mount_target_security_groups.rs — count the requested SecurityGroups against the per-interface quota before mutating state and return SecurityGroupLimitExceeded (ErrorCode "SecurityGroupLimitExceeded", the documented message) with HTTP 400, leaving the stored set untouched.
Traces: efs-mount-target-security-group-limits#task#5 and #task#6 (same cluster; both expected 400, got 204).

Cause 2 of 2: after the two over-limit modifies, the set DescribeMountTargetSecurityGroups reports is not the set the last ACCEPTED modify left.
AWS: Smithy: DescribeMountTargetSecurityGroups returns DescribeMountTargetSecurityGroupsResponse.SecurityGroups (required, list of ^sg-[0-9a-f]{8,40}$), and ModifyMountTargetSecurityGroups is documented to "replace the security groups in effect ... with the SecurityGroups provided in the request". So the read must answer exactly the client-supplied set of the last request AWS accepted, element for element; a request AWS rejected with SecurityGroupLimitExceeded leaves the previous set in effect. Recorded behavior only for the specific membership: the recording shows HTTP 200 with the set established by the last accepted modify.
Emulator: answers HTTP 200 but with different membership — the replay reports $.SecurityGroups[0] and $.SecurityGroups[3] both differing from the expected set, and the actual list contains an id the client never supplied for that mount target (an emulator-minted sg id of the ^sg-[0-9a-f]{8,40}$ form, e.g. one generated when the mount target was created) where the expected element is a client-supplied id. Hypothesis: this is downstream of Cause 1 — because the over-limit modifies at task#5/#6 were applied instead of rejected, the stored list is the one from the request AWS refused rather than the one left by the last accepted modify; the read itself (services/service-com-amazonaws-efs-magnolioapiservice-v20150201/src/ops/describe_mount_target_security_groups.rs, which clones mount_target.security_groups) is a faithful read of state. If rejecting the over-limit writes does not by itself restore the expected membership, the remaining difference is in how the mount target's security-group set is seeded/ordered in state.
Why: a client reads back the groups in effect to confirm what it applied. It sees a membership it never requested — including an id it did not supply — so it concludes a change it never made took effect, and configuration drift checks and idempotent re-applies both misfire. Note the exact id strings in the recording are per-environment values filling the template; the requirement is that the list equal, element for element and in order, the set the last accepted modify supplied.
Fix: the ModifyMountTargetSecurityGroups handler's state write (reject over-limit requests without mutating state) and, if the divergence persists, the mount target security-group field in services/service-com-amazonaws-efs-magnolioapiservice-v20150201/src/state.rs; DescribeMountTargetSecurityGroups' own handler and serializer are correct as written.
Traces: efs-mount-target-security-group-limits#task#7.

Not covered by the one open elasticfilesystem issue (20261125T033651Z-efs-lambda-removing-elasticfilesystem-clientmount-from-a-function.md), which is about Lambda cold-invoke EFSMountFailureException on IAM ClientMount removal and carries no replay key for these steps.

## Evidence
Replay of fixture efs-mount-target-security-group-limits (trace set simparity-wire-aws-v1), 2 divergence clusters, 3 steps:

Cause 1 — step task#5 (cluster also covers task#6), ModifyMountTargetSecurityGroups:
  expected: HTTP 400, body {"ErrorCode": "SecurityGroupLimitExceeded", "Message": "The maximum number of security groups per interface has been reached."}, headers content-length and x-amzn-errortype present
  actual:   HTTP 204, empty body, content-length missing, x-amzn-errortype missing
  detail: "status: expected 400, got 204 / $: empty_body (expected {'ErrorCode': 'SecurityGroupLimitExceeded', 'Message': 'The maximum number of security groups per interface has been reached.'}, got {})"

Cause 2 — step task#7, DescribeMountTargetSecurityGroups:
  expected: HTTP 200, SecurityGroups[0] = the client-supplied id recorded there, SecurityGroups[3] = the client-supplied id recorded there
  actual:   HTTP 200, SecurityGroups[0] = a different client-supplied id, SecurityGroups[3] = an emulator-minted sg id the request never named
  detail: "$.SecurityGroups[0]: missing (expected 'sg-000000000000000bc', got 'sg-000000000000000b4') / $.SecurityGroups[3]: missing (expected 'sg-000000000000000ba', got 'sg-e2b31b781befc5982') / $.SecurityGroups[3]: extra"

Code: modify_mount_target_security_groups.rs assigns mount_target.security_groups = security_groups with no quota check and never constructs SecurityGroupLimitExceeded; describe_mount_target_security_groups.rs returns that stored list verbatim.

## Steps to Reproduce
1. Replay the wire corpus (trace set simparity-wire-aws-v1) against the emulator; fixture efs-mount-target-security-group-limits diverges at task#5, task#6 and task#7.
2. By hand: create a file system and a mount target, then PUT /2015-02-01/mount-targets/{MountTargetId}/security-groups with a SecurityGroups list longer than the per-interface quota (more than 5 ids of the form sg-[0-9a-f]{8,40}, but 100 or fewer so the modelled @length does not refuse it).
   Expected: HTTP 400 with x-amzn-errortype naming SecurityGroupLimitExceeded and body {"ErrorCode":"SecurityGroupLimitExceeded","Message":"The maximum number of security groups per interface has been reached."}. Actual: HTTP 204, empty body.
3. GET /2015-02-01/mount-targets/{MountTargetId}/security-groups.
   Expected: HTTP 200 whose SecurityGroups equals, element for element and in order, the set supplied by the last modify AWS ACCEPTED (the over-limit one having been rejected). Actual: HTTP 200 whose list is the over-limit request's set and contains an id the client never supplied.
