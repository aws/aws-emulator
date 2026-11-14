Issue-Source: trace-sweep-divergence
Issue-Trace: redshift-redshift-idc-application-lifecycle
Issue-Step: task#3
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: redshift-redshift-idc-application-lifecycle redshift-qev2-idc-application-lifecycle
Issue-Shape-Id: com.amazonaws.redshift#RedshiftServiceVersion20121201
Issue-Key: redshift|CreateRedshiftIdcApplication|4xx->4xx|$.ErrorResponse.Error.Message
Issue-Key: redshift|CreateQev2IdcApplication|4xx->4xx|$.ErrorResponse.Error.Message
Issue-Commit: 029c732db8092362c2d6a7aa89bcc4c3ae279d73

# Redshift IdC application create refusals drop the nested SsoAdmin service wrapper from the DependentServiceAccessDenied message

> _Note: this issue was identified against the code base at commit `029c732db8092362c2d6a7aa89bcc4c3ae279d73`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateRedshiftIdcApplication

## Description
When Redshift's IdC-application create calls fail because the dependent IAM Identity Center service refused the nested `sso:CreateApplication`, AWS returns the downstream exception's full text: the authorization sentence FOLLOWED BY the dependent-service wrapper `(Service: SsoAdmin, Status Code: <code>, Request ID: <id>) (SDK Attempt Count: <n>)`. The emulator returns the authorization sentence only and drops the wrapper entirely. This issue bundles 2 causes; both must be closed.

Cause 1 of 2: CreateRedshiftIdcApplication.
AWS: Smithy: CreateRedshiftIdcApplication(IdcInstanceArn, RedshiftIdcApplicationName, IdcDisplayName, IamRoleArn required); modeled error DependentServiceAccessDeniedFault (httpError 403, awsQueryError code DependentServiceAccessDenied). Recorded message template: `User: <caller ARN> is not authorized to perform: sso:CreateApplication on resource: <application resource ARN> because no resource-based policy allows the sso:CreateApplication action (Service: SsoAdmin, Status Code: <downstream status>, Request ID: <downstream request id>) (SDK Attempt Count: <n>)`. The fixed text is `User: ` / ` is not authorized to perform: sso:CreateApplication on resource: ` / ` because no resource-based policy allows the sso:CreateApplication action` and the wrapper literals `(Service: SsoAdmin, Status Code: `, `, Request ID: `, `) (SDK Attempt Count: `, `)`; the caller ARN, resource ARN, status code, request id and attempt count are per-call values (the request id in particular differs every run and is NOT a requirement — the presence and shape of the wrapper is).
Emulator: HTTP 403 with the correct code, and the message stops after `...allows the sso:CreateApplication action` — no `(Service: SsoAdmin, ...)` segment at all.
Why: a client or operator diagnosing the failure cannot tell which dependent service refused the call, cannot read the downstream status code and has no request id to quote in a support case; tooling that parses the `(Service: X, Status Code: N, Request ID: ...)` tail — the standard AWS SDK dependent-exception form — finds nothing and reports the refusal as originating in Redshift.
Fix: the DependentServiceAccessDeniedFault message construction for CreateRedshiftIdcApplication in the redshift handler — append the dependent-service wrapper (service name SsoAdmin, the downstream status code, the generated downstream request id and the attempt count) to the propagated authorization message.
Traces: redshift-redshift-idc-application-lifecycle#task#3.

Cause 2 of 2: CreateQev2IdcApplication.
AWS: same modeled refusal class — DependentServiceAccessDeniedFault, HTTP 403, code DependentServiceAccessDenied (recorded behavior for this operation; the nested action is likewise `sso:CreateApplication`) — with the same wrapper tail appended to the authorization sentence.
Emulator: identical truncation: the authorization sentence without the `(Service: SsoAdmin, Status Code: ..., Request ID: ...) (SDK Attempt Count: ...)` wrapper.
Why: as cause 1, on the Q/QEv2 IdC-application path.
Fix: the same message construction for CreateQev2IdcApplication's dependent-service refusal.
Traces: redshift-qev2-idc-application-lifecycle#task#3.

Note: only the wrapper's presence/shape is being filed, not the per-run request id inside it. Related but NOT covering these: 20261029T192501Z-redshift-resource-policy-idc-application-and-event-subscription.md and 20261029T200555Z-redshift-namespace-reserved-node-idc-application-hsm-refusals.md; the replay reports both clusters here unowned.

## Evidence
Replay of trace set simparity-wire-aws-v1; both clusters reported with empty owned_by and empty stub_path; status (403) and Error.Code matched, only $.ErrorResponse.Error.Message diverged:

1. redshift-redshift-idc-application-lifecycle#task#3 CreateRedshiftIdcApplication — expected 'User: arn:aws:iam::000000000000:user/emulator is not authorized to perform: sso:CreateApplication on resource: arn:aws:sso::*:application/ssoins-sim2realprobered/* because no resource-based policy allows the sso:CreateApplication action (Service: SsoAdmin, Status Code: 400, Request ID: e34d720e-43a3-45f9-958d-feabdc8e0e55) (SDK Attempt Count: 1)', got the same sentence truncated at '...allows the sso:CreateApplication action'.
2. redshift-qev2-idc-application-lifecycle#task#3 CreateQev2IdcApplication — expected the same wording with '(Service: SsoAdmin, Status Code: 400, Request ID: 0bfa9034-539d-4acb-a6a2-1c16ca4228ce) (SDK Attempt Count: 1)', got the truncated sentence.

The two recorded request IDs differ per run and are not the requirement; the missing wrapper (literal '(Service: SsoAdmin, Status Code: <n>, Request ID: <id>) (SDK Attempt Count: <n>)') is.

## Steps to Reproduce
1. Boot the emulator from this checkout and replay the wire corpus for trace set simparity-wire-aws-v1.
2. Call CreateRedshiftIdcApplication with an IdcInstanceArn whose IdC instance does not grant sso:CreateApplication (fixture redshift-redshift-idc-application-lifecycle, step task#3). Observe HTTP 403 DependentServiceAccessDenied whose message ends at '...allows the sso:CreateApplication action'.
3. Call CreateQev2IdcApplication the same way (fixture redshift-qev2-idc-application-lifecycle, step task#3); same truncation.
4. Compare with the recorded responses, which append '(Service: SsoAdmin, Status Code: <n>, Request ID: <id>) (SDK Attempt Count: <n>)'.
5. Re-replay both fixture steps after the fix.
