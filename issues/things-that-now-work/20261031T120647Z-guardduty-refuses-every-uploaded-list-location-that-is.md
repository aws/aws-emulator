Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: guardduty.ipset-lifecycle-active-then-deleted guardduty.threatintelset-lifecycle guardduty.deploy-ipset-cfn-behaves
Issue-Shape-Id: com.amazonaws.guardduty#GuardDutyAPIService
Issue-Key: guardduty|CreateIPSet|BadRequestException
Issue-Key: guardduty|CreateThreatIntelSet|BadRequestException
Issue-Key: guardduty.deploy-ipset-cfn-behaves|_deploy_cfn#6|WaiterError|cfn.get_waiter("stack_create_complete").wait(
Issue-Commit: 6c2a412138b2fb0170715c3d0a682813a233dd77

# GuardDuty refuses every uploaded-list Location that is not an amazonaws.com S3 URL, so the IPSet / ThreatIntelSet lifecycles and the CloudFormation AWS::GuardDuty::IPSet provisioner are unreachable

> _Note: this issue was identified against the code base at commit `6c2a412138b2fb0170715c3d0a682813a233dd77`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateIPSet

## Description
REOPENS 20261029T183757Z-guardduty-accepts-input-real-aws-refuses-off-scale.md: its fix left the call refused.

This issue carries 1 root cause, in the guardduty crate's shared uploaded-list `Location` validator. The earlier issue asked for CreateIPSet to stop accepting free text as a `Location`; the validator that was added now also refuses well-formed S3 object URLs, so the create it was meant to harden refuses valid input and three lifecycles that depend on it never start.

Cause 1 of 1: the shared `Location` syntax check only admits `s3://bucket/key` and HTTPS URLs whose host is an `*.amazonaws.com` S3 endpoint, so a well-formed S3 object URL that addresses the S3 endpoint actually in use is refused.

AWS: Smithy `CreateIPSet.Location` and `CreateThreatIntelSet.Location` both target the shared `Location` shape — required, `@length` min 1 max 300, NO `@pattern` — and the member is documented as "The URI of the file that contains the IPSet" / its threat-list sibling. The model therefore states no host or scheme rule at all; the only recorded rule is that a syntactically valid S3 object URL of the form `<s3-endpoint>/<bucket>/<key>` is ACCEPTED (HTTP 200 with the minted `ipSetId` / `threatIntelSetId`, the list then proceeding ACTIVATING -> ACTIVE and reading back with the `Location` verbatim), while free text that is no S3 URL at all is refused HTTP 400 `BadRequestException` with the fixed text `The request is rejected because the parameter ipSetLocation has an invalid value.` (and `...parameter threatIntelSetLocation has an invalid value.` for the threat list). The fixed text and the parameter name in those two messages are correct as the emulator writes them; what is wrong is WHICH requests they are applied to. AWS resolves the object URL against the S3 endpoint the caller's URL names — it does not require a literal `amazonaws.com` host — so the equivalent object URL for whichever S3 endpoint is in force is valid input.

Emulator: `crate::state::is_s3_location` (services/service-com-amazonaws-guardduty-guarddutyapiservice/src/state.rs, the `is_s3_location` / `s3_endpoint_prefix` pair) accepts only the `s3` scheme and, for `https`, only hosts it recognises as `s3[.-]<region>.amazonaws.com` or `<bucket>.s3[.-]<region>.amazonaws.com`. Every other host — including the S3 endpoint this emulator itself serves and hands to clients, which is the ONLY S3 reachable in the emulated world — returns false, and the handler answers HTTP 400 `BadRequestException` with the malformed-Location text. Replay shows: `CreateIPSet` -> `BadRequestException: The request is rejected because the parameter ipSetLocation has an invalid value.`; `CreateThreatIntelSet` -> `BadRequestException: The request is rejected because the parameter threatIntelSetLocation has an invalid value.`, for an object the check had just written with `PutObject` through that same endpoint. The same refusal reaches CloudFormation: `AWS::GuardDuty::IPSet` has a real provisioner (services/service-com-amazonaws-cloudformation-cloudformation/src/state/provisioner.rs, `Provisioner::GuardDutyIpSet` -> `create_guardduty_ip_set`, which calls `CreateIPSet`), so a stack declaring that type with the same `Location` parameter never reaches `CREATE_COMPLETE` and settles in `ROLLBACK_COMPLETE` instead.

Why: a client cannot create a trusted IP list or a threat list at all unless its S3 object URL happens to be spelled against a public AWS S3 endpoint. Every downstream state transition the API exposes — ACTIVATING -> ACTIVE, `GetIPSet` / `GetThreatIntelSet`, rename via `UpdateThreatIntelSet`, `DeleteIPSet` and the DELETED terminal status — is unreachable, and so is the `UpdateIPSet`/`UpdateThreatIntelSet`-on-deleted refusal that open issue 20261029T183757Z-guardduty-lifecycle-state-is-wrong-missing-dry-run.md (Cause 2) is about: that issue's checks now fail EARLIER, on the create, so fixing it cannot be verified until this is fixed. Infrastructure-as-code is affected the same way: a CloudFormation template declaring `AWS::GuardDuty::IPSet` rolls back, and the caller is told the stack failed rather than that one property was rejected.

Fix: the shared uploaded-list `Location` acceptance predicate in the guardduty crate — `is_s3_location` and the `s3_endpoint_prefix` host test it delegates to in services/service-com-amazonaws-guardduty-guarddutyapiservice/src/state.rs — must admit any well-formed S3 object URL (an http/https URL naming a bucket and a non-empty object key, whatever its host and port) and keep refusing only input that is no S3 URL at all, which is what the earlier issue asked for. Its callers in src/ops/create_ip_set.rs and src/ops/create_threat_intel_set.rs (and the entity-set and update siblings that share it) need no change beyond inheriting the corrected predicate; the refusal's status, error code and message text stay exactly as they are.

Traces: guardduty.ipset-lifecycle-active-then-deleted, guardduty.threatintelset-lifecycle, guardduty.deploy-ipset-cfn-behaves.

## Evidence
guardduty.ipset-lifecycle-active-then-deleted — `botocore.errorfactory.BadRequestException: An error occurred (BadRequestException) when calling the CreateIPSet operation: The request is rejected because the parameter ipSetLocation has an invalid value.` The call was `CreateIPSet(DetectorId=<detector>, Name=<name>, Format='TXT', Location='<s3-endpoint>/<bucket>/list.txt', Activate=True)`, where the object had just been written with `PutObject` and made publicly readable through that same endpoint. Expected: HTTP 200 with `IpSetId`, then `GetIPSet` reporting Status ACTIVATING -> ACTIVE with `Name` and `Location` as submitted.

guardduty.threatintelset-lifecycle — `botocore.errorfactory.BadRequestException: An error occurred (BadRequestException) when calling the CreateThreatIntelSet operation: The request is rejected because the parameter threatIntelSetLocation has an invalid value.` Same `Location` form. Expected: HTTP 200 with `ThreatIntelSetId` and the set reaching ACTIVE.

guardduty.deploy-ipset-cfn-behaves — `botocore.exceptions.WaiterError: Waiter StackCreateComplete failed: Waiter encountered a terminal failure state: For expression "Stacks[].StackStatus" we matched expected path: "ROLLBACK_COMPLETE" at least once.` The stack declares a single `AWS::GuardDuty::IPSet` with `Format: TXT` and `Location` set to the same `<s3-endpoint>/<bucket>/<key>` object URL; its provisioner calls `CreateIPSet`, which is refused as above, so the stack rolls back instead of reaching CREATE_COMPLETE.

In each message the fixed text (`The request is rejected because the parameter `, ` has an invalid value.`) and the parameter name (`ipSetLocation` / `threatIntelSetLocation`) are AWS's own and are correct; the defect is that the refusal fires for input AWS accepts. The host and port of the S3 endpoint are environment-specific and are not part of any requirement — the requirement is that any well-formed S3 object URL naming a bucket and a non-empty key is accepted.

## Steps to Reproduce
1. Replay the three checks named in trace_ids: guardduty.ipset-lifecycle-active-then-deleted, guardduty.threatintelset-lifecycle, guardduty.deploy-ipset-cfn-behaves.
2. Manual repro of the create refusal: CreateDetector(Enable=True) (or adopt the account's detector); create an S3 bucket and PutObject a text list into it through the S3 endpoint the clients are configured with; then call CreateIPSet(DetectorId=<detector>, Name='ipset-1', Format='TXT', Location='<that S3 endpoint>/<bucket>/<key>', Activate=True). Observe HTTP 400 BadRequestException 'The request is rejected because the parameter ipSetLocation has an invalid value.'; expected HTTP 200 with an IpSetId.
3. Repeat step 2 with CreateThreatIntelSet(..., Format='TXT', Location=<same URL>, Activate=True): observe the same refusal naming threatIntelSetLocation.
4. Confirm the predicate, not the message, is at fault: repeat step 2 with Location='s3://<bucket>/<key>' — accepted — and with Location='this is not a valid s3 uri or arn' — refused, which is the behavior the earlier fixed issue asked for and must be preserved.
5. CloudFormation path: CreateStack with a template whose single resource is AWS::GuardDuty::IPSet (DetectorId, Name, Format: TXT, Location = the object URL from step 2, Activate: true) and an Output returning !Ref on it. Observe the stack settle in ROLLBACK_COMPLETE; DescribeStackEvents shows the resource's CREATE_FAILED carrying GuardDuty's ipSetLocation refusal. Expected CREATE_COMPLETE with the Output carrying the minted IpSetId, GetIPSet reporting ACTIVE, and the stack's deletion driving the IPSet to the terminal DELETED status.
