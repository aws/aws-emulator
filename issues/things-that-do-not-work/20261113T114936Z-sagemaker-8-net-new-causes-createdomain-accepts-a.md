Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: sagemaker.create-domain-duplicate-name-rejected sagemaker.create-domain-malformed-subnet-rejected sagemaker.create-domain-invalid-vpc-config-rejected sagemaker.create-domain-malformed-vpc-config-rejected sagemaker.notebook-instance-create-stop-delete-lifecycle sagemaker.deploy-notebook-instance-cfn-behaves sagemaker.notebook-instance-delete-while-inservice-rejected sagemaker.deploy-notebook-instance-cfn-invalid-role-rejected sagemaker.deploy-notebook-instance-cfn-presigned-url-succeeds sagemaker.deploy-notebook-instance-cfn-presigned-url-works sagemaker.deploy-notebook-instance-cloudformation-presigned-url-accessible sagemaker.domain-pending-refuses-user-profile-space-and-presigned-url
Issue-Shape-Id: com.amazonaws.sagemaker#SageMaker
Issue-Key: sagemaker.create-domain-duplicate-name-rejected|_error#14|AssertionError|raise AssertionError(f"expected a ClientError, but the call SUCCEEDED and returned {result!r}")
Issue-Key: sagemaker.create-domain-malformed-subnet-rejected|_error#5|AssertionError|raise AssertionError(f"expected a ClientError, but the call succeeded: {result!r}")
Issue-Key: sagemaker.create-domain-invalid-vpc-config-rejected|run#11|AssertionError|assert bad_status == "Failed", (
Issue-Key: sagemaker.create-domain-malformed-vpc-config-rejected|run#17|AssertionError|assert bad_status == "Failed", (
Issue-Key: sagemaker.notebook-instance-create-stop-delete-lifecycle|run#17|AssertionError|assert gone_message == "RecordNotFound", gone_message
Issue-Key: sagemaker.deploy-notebook-instance-cfn-behaves|run#33|AssertionError|assert gone_message == "RecordNotFound", f"expected 'RecordNotFound', got {gone_message!r}"
Issue-Key: sagemaker.notebook-instance-delete-while-inservice-rejected|run#13|AssertionError|assert message == (
Issue-Key: sagemaker|DescribeNotebookInstance|ValidationException
Issue-Key: sagemaker.deploy-notebook-instance-cfn-presigned-url-succeeds|run#12|AssertionError|assert not (p := ctx.published_url_mismatch(url, labels=("notebook", ctx.region, "sagemaker"))), p
Issue-Key: sagemaker.deploy-notebook-instance-cfn-presigned-url-works|run#11|AssertionError|assert not (p := ctx.published_url_mismatch(url, labels=("notebook", ctx.region, "sagemaker"))), p
Issue-Key: sagemaker.deploy-notebook-instance-cloudformation-presigned-url-accessible|run#9|AssertionError|assert not (p := ctx.published_url_mismatch(url, labels=("notebook", ctx.region, "sagemaker"))), p
Issue-Key: sagemaker.domain-pending-refuses-user-profile-space-and-presigned-url|run#8|AssertionError|assert code == "ValidationException", f"CreateUserProfile on a Pending domain: expected ValidationException, got {code!r}"
Issue-Commit: d0f76c0ba03e1debbd23029512a6c9d3948c5ea6

# sagemaker: 8 net-new causes — CreateDomain accepts a duplicate name and malformed VPC/subnet ids, a cross-VPC domain reaches InService, notebook-instance refusal messages are reworded, a CFN notebook with an untrusted role leaves no Failed resource, presigned notebook URLs are unreachable, and CreateUserProfile is a 501

> _Note: this issue was identified against the code base at commit `d0f76c0ba03e1debbd23029512a6c9d3948c5ea6`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateDomain

## Description
EIGHT net-new behavioral root causes on the direct SageMaker control plane. None of the failing checks listed per cause is marked ALREADY FILED or OWNED by the replay, so no open issue carries their replay keys. All eight must be closed before this issue is submitted.

Cause 1 of 8: CreateDomain accepts a second domain that reuses an in-use DomainName.
AWS: Smithy (services/service-com-amazonaws-sagemaker-sagemaker/smithy-model/com-amazonaws-sagemaker-sagemaker.json): CreateDomain input DomainName (required, @length 0..63, pattern ^[a-zA-Z0-9](-*[a-zA-Z0-9]){0,62}$), modeled errors ResourceInUse (client, HTTP 400) and ResourceLimitExceeded. While a domain still holds a name, a second CreateDomain naming it is refused with ResourceInUse and the sentence "The Domain Name '<DomainName>' already exists in this account in this region." — fixed text "The Domain Name '" ... "' already exists in this account in this region." with the request's DomainName as the only variable part; no second domain is created, and the name only becomes reusable after DeleteDomain.
Emulator: the second, byte-identical CreateDomain answers HTTP 200 with a brand-new DomainArn/DomainId/Url, so two domains now share one DomainName and the ResourceInUse branch is unreachable.
Why: a client that uses CreateDomain as its idempotent 'ensure the domain exists' step (catching ResourceInUse and reading the existing domain back) silently provisions a duplicate domain every run, then resolves the name to an arbitrary one of them; name-collision handling and the account's one-domain-per-name invariant never hold.
Fix: CreateDomain's handler/validation in the sagemaker crate must look the DomainName up in domain state before allocating a DomainId and raise ResourceInUse/400 with the sentence above.
Traces: sagemaker.create-domain-duplicate-name-rejected.

Cause 2 of 8: CreateDomain accepts syntactically malformed VpcId and SubnetIds.
AWS: Smithy: CreateDomain VpcId (@length 0..32, pattern ^[-0-9a-zA-Z]+$) and SubnetIds (Subnets, @length 1..16 of SubnetId @length 0..32). On top of the model, the service validates the identifier forms and refuses a malformed pair up front with ValidationException/400 and the standard multi-error sentence: "2 validation errors detected: Value '<vpcId>' at 'vpcId' failed to satisfy constraint: Member must satisfy regular expression pattern: vpc-[a-z0-9]{8,}; Value '[<subnetIds>]' at 'subnetIds' failed to satisfy constraint: Member must satisfy constraint: [Member must have length less than or equal to 32, Member must have length greater than or equal to 0, Member must satisfy regular expression pattern: subnet-[a-z0-9]{8,}]" (recorded behavior for the sentence; the error count, the quoted values and the member paths come from the request, the rest is fixed text). No domain is persisted.
Emulator: answers HTTP 200 and creates the domain (DomainArn/DomainId/Url returned) for VpcId "not-a-real-vpc-id" and SubnetIds ["not-a-real-subnet-id"], so the malformed identifiers are stored and the ValidationException branch is unreachable.
Why: a client that depends on a synchronous refusal for a typo'd or templated-wrong network identifier instead gets an 'InService' domain whose networking cannot exist, and the broken domain must be found and cleaned up later; the negative test of every infrastructure template passes against AWS and mutates state here.
Fix: CreateDomain's input validation in the sagemaker crate — validate VpcId/SubnetIds identifier form before creating the domain and emit the multi-error ValidationException sentence. The open wire issue 20261113T083839Z-sagemaker-studio-domain-app-paths-diverge-missing.md covers a different CreateDomain gap (neither VpcId nor SubnetIds supplied at all) and does not carry this check.
Traces: sagemaker.create-domain-malformed-subnet-rejected.

Cause 3 of 8: a domain whose SubnetIds do not belong to the declared VpcId still reaches Status=InService.
AWS: Smithy: DescribeDomain output Status -> DomainStatus (enum, includes Pending/InService/Failed/Update_Failed) plus FailureReason (@length 0..1024). AWS accepts the cross-VPC request synchronously (HTTP 200, Status=Pending) and resolves the subnets against the declared VPC during async provisioning; the mismatch is terminal, so DescribeDomain and ListDomains settle on Status=Failed with a FailureReason, and the domain never becomes usable. A self-consistent VPC/subnet/security-group pairing settles on InService.
Emulator: the mismatched domain settles on terminal Status 'InService' with FailureReason absent — the subnets are never resolved against VpcId, so the asynchronous-failure path of domain provisioning has no reachable trigger at all.
Why: a client that provisions a domain and then branches on the terminal Status (fail the deployment, roll back, surface FailureReason) is told a domain whose networking is impossible is healthy; Studio/app traffic later fails with no diagnosis, and no test can exercise the Failed branch.
Fix: the domain-provisioning state machine behind CreateDomain/DescribeDomain in the sagemaker crate must validate the declared VpcId against each SubnetId's VPC (and the security groups' VPC) while the domain is Pending and settle it in Failed with a FailureReason naming the mismatch.
Traces: sagemaker.create-domain-invalid-vpc-config-rejected, sagemaker.create-domain-malformed-vpc-config-rejected.

Cause 4 of 8: DescribeNotebookInstance's not-found message carries an extra ARN clause.
AWS: Smithy: DescribeNotebookInstance input NotebookInstanceName (required); the operation models no errors, and a name that identifies no notebook instance (never created, or already deleted) is refused with the service-common ValidationException/400 whose Message is the bare token "RecordNotFound" — the whole message is fixed text with nothing interpolated from the request or state.
Emulator: refuses with the right code but appends a clause naming the resource: "RecordNotFound. Notebook Instance Arn: arn:aws:sagemaker:<region>:<account>:notebook-instance/<name>". The ARN is a per-run value; the divergence is that AWS's message for this class is only the fixed token.
Why: a client that distinguishes 'this notebook is gone' from any other ValidationException by comparing Message to "RecordNotFound" (the documented sentinel, and how deletion is confirmed) never matches, so delete-then-poll loops treat a completed deletion as an unknown error and either hang or abort teardown.
Fix: the not-found path of the DescribeNotebookInstance handler's message template in the sagemaker crate.
Traces: sagemaker.notebook-instance-create-stop-delete-lifecycle, sagemaker.deploy-notebook-instance-cfn-behaves.

Cause 5 of 8: DeleteNotebookInstance's wrong-state refusal does not render the allowed-status set as a list.
AWS: Smithy: DeleteNotebookInstance input NotebookInstanceName (required), no modeled errors and no output; an instance that is not Stopped or Failed cannot be deleted and the call is refused with ValidationException/400 whose Message is "Status (<current status>) not in ([Stopped, Failed]). Unable to transition to (Deleting) for Notebook Instance (<notebook-instance-arn>)." — fixed text "Status (" ... ") not in ([Stopped, Failed]). Unable to transition to (Deleting) for Notebook Instance (" ... ").", with the instance's current NotebookInstanceStatus and its ARN as the only variable parts. The instance is left untouched.
Emulator: refuses with the right code and leaves the instance alone, but drops the brackets around the allowed set: "Status (InService) not in (Stopped, Failed). Unable to transition to (Deleting) for Notebook Instance (<arn>)."
Why: the allowed-status set is the machine-readable part of this refusal; a client or operator that parses the bracketed list to decide which transition to perform first (stop, then delete) sees a different shape and falls back to generic retry, so the stop-then-delete repair never runs.
Fix: the state-guard message template in the DeleteNotebookInstance handler.
Traces: sagemaker.notebook-instance-delete-while-inservice-rejected.

Cause 6 of 8: a CloudFormation-declared notebook instance whose role does not trust SageMaker leaves no notebook instance behind, even with rollback disabled.
AWS: CreateNotebookInstance is accepted synchronously for a well-formed RoleArn and the AssumeRole failure surfaces during provisioning: the instance is created and sticks in NotebookInstanceStatus='Failed' (Smithy: DescribeNotebookInstance output NotebookInstanceStatus -> NotebookInstanceStatus enum incl. Failed, plus FailureReason @length 0..1024). With CreateStack(DisableRollback=True) the stack ends CREATE_FAILED and the failed physical resource is retained, so DescribeNotebookInstance on it still answers 200 with Status='Failed'.
Emulator: the stack does end CREATE_FAILED, but DescribeNotebookInstance on the declared NotebookInstanceName is refused with ValidationException 'RecordNotFound...' — no notebook instance exists at all, so the role-trust failure is reported only as a stack-level failure and the Failed resource state is never observable. (Hypothesis: the create is refused synchronously in the resource handler, or the resource is torn down despite DisableRollback.)
Why: a client or operator debugging a failed deployment reads the resource's own Status/FailureReason to learn why provisioning failed; here the resource is simply absent, so the diagnosis is lost and DisableRollback — whose entire purpose is to retain failed resources for inspection — has no effect.
Fix: the CreateNotebookInstance provisioning path must accept a well-formed RoleArn and settle the instance in Failed with a FailureReason naming the AssumeRole denial, and the CloudFormation AWS::SageMaker::NotebookInstance provisioner must retain that resource when rollback is disabled.
Traces: sagemaker.deploy-notebook-instance-cfn-invalid-role-rejected.

Cause 7 of 8: CreatePresignedNotebookInstanceUrl publishes an AuthorizedUrl on a host the emulator does not serve.
AWS: Smithy: CreatePresignedNotebookInstanceUrl input NotebookInstanceName (required) + SessionExpirationDurationInSeconds (@range 1800..43200); output AuthorizedUrl -> NotebookInstanceUrl, the URL a caller then fetches to obtain an authenticated notebook session. Recorded behavior only for the host's literal form; what IS a requirement is that the published address is reachable by the client that received it and serves the authenticated session.
Emulator: returns "https://<notebook-name>.notebook.<region>.sagemaker.aws/auth?authToken=<token>" — a real-AWS hostname the emulator neither owns nor serves. The harness refuses it: "its host is not one of the emulator's own hosts; a customer of the emulator needs http(s)://127.0.0.1:<port>/<resource path>, or http(s)://<own host>:<port>/<path> on a per-resource listener". The emulator announced an address other than the one it is reached on.
Why: every CFN-deployed notebook flow that does what the API exists for — fetch the AuthorizedUrl to open the notebook — leaves the emulator and fails with a DNS/network error instead of an authenticated session.
Fix: the AuthorizedUrl construction in the CreatePresignedNotebookInstanceUrl handler must derive host and port from the emulator's own serving endpoint (or a per-resource listener it owns) and the emulator must serve that auth path. The open issue 20261109T134927Z-sagemaker-5-net-new-causes-presigned-notebook-urls.md describes the same construction for two OTHER checks (the TTL checks) and does not carry the three replay keys below.
Traces: sagemaker.deploy-notebook-instance-cfn-presigned-url-succeeds, sagemaker.deploy-notebook-instance-cfn-presigned-url-works, sagemaker.deploy-notebook-instance-cloudformation-presigned-url-accessible.

Cause 8 of 8: CreateUserProfile answers NotImplemented, so a Pending domain's refusal contract is unreachable.
AWS: Smithy: CreateUserProfile input DomainId (required) + UserProfileName (required), modeled errors ResourceInUse and ResourceLimitExceeded; a domain that has not finished provisioning cannot take user profiles, spaces or presigned URLs, and the call is refused with the service-common ValidationException/400 (the sibling presigned-URL refusal on a Pending domain reads "Domain must be in either InService or Update_Failed state and not in: Pending", where the trailing status is the domain's current Status and the rest is fixed text).
Emulator: CreateUserProfile answers error code 'NotImplemented', so the domain-state guard cannot be exercised at all and a client cannot tell 'not yet provisioned' from 'operation absent'.
Why: any Studio onboarding sequence (create domain, then user profile, then space/app) stops at the first call, and the retry-until-InService behavior clients implement against ValidationException never runs.
Fix: implement CreateUserProfile (services/service-com-amazonaws-sagemaker-sagemaker/src/ops/create_user_profile.rs) and add the domain-state guard that refuses while the domain is Pending. This cause is gated by the already-filed CreateUserProfile scaffold-stub issue (the replay reports that stub as blocking 12 other checks, and 20261024T160943Z-sagemaker-control-plane-gaps-createuserprofile-and-deleteaibenchmarkjob-are.md names the 501); neither carries this check's replay key, so close the state-guard behavior here together with that stub.
Traces: sagemaker.domain-pending-refuses-user-profile-space-and-presigned-url.

## Evidence
Cause 1 — sagemaker.create-domain-duplicate-name-rejected [mismatch]:
  code, message = _error(_create)
  AssertionError: expected a ClientError, but the call SUCCEEDED and returned {'DomainArn': 'arn:aws:sagemaker:<region>:<account>:domain/d-<id>', 'DomainId': 'd-<id>', 'Url': 'https://d-<id>.studio.<region>.sagemaker.aws', 'ResponseMetadata': {'HTTPStatusCode': 200, ...}}
The check expected ResourceInUse with "The Domain Name '<domain-name>' already exists in this account in this region." The DomainId/ARN/Url are per-run values.

Cause 2 — sagemaker.create-domain-malformed-subnet-rejected [mismatch]:
  code, message = _error(lambda: sm.create_domain(... SubnetIds=['not-a-real-subnet-id'], VpcId='not-a-real-vpc-id'))
  AssertionError: expected a ClientError, but the call succeeded: {'DomainArn': ..., 'DomainId': 'd-<id>', 'Url': ..., 'HTTPStatusCode': 200}
Expected code ValidationException and message "2 validation errors detected: Value 'not-a-real-vpc-id' at 'vpcId' failed to satisfy constraint: Member must satisfy regular expression pattern: vpc-[a-z0-9]{8,}; Value '[not-a-real-subnet-id]' at 'subnetIds' failed to satisfy constraint: Member must satisfy constraint: [Member must have length less than or equal to 32, Member must have length greater than or equal to 0, Member must satisfy regular expression pattern: subnet-[a-z0-9]{8,}]" (the quoted values come from the request).

Cause 3 — sagemaker.create-domain-invalid-vpc-config-rejected [mismatch]:
  assert bad_status == "Failed"
  AssertionError: expected the cross-VPC subnet/security-group mismatch to fail domain provisioning (Status=Failed), got 'InService'
and sagemaker.create-domain-malformed-vpc-config-rejected [mismatch]:
  AssertionError: expected an internally inconsistent VPC config to fail asynchronously, got terminal status 'InService' (reason=None)
Both checks' positive controls (a self-consistent VPC/subnet pairing) are expected to reach InService, so the divergence is only the mismatched pairing's terminal status and its absent FailureReason.

Cause 4 — sagemaker.notebook-instance-create-stop-delete-lifecycle [mismatch]:
  assert gone_message == "RecordNotFound", gone_message
  AssertionError: RecordNotFound. Notebook Instance Arn: arn:aws:sagemaker:<region>:<account>:notebook-instance/<name>
and sagemaker.deploy-notebook-instance-cfn-behaves [mismatch]:
  AssertionError: expected 'RecordNotFound', got 'RecordNotFound. Notebook Instance Arn: arn:aws:sagemaker:<region>:<account>:notebook-instance/<name>'
The code (ValidationException) matched in both; the ARN in the appended clause is a per-run value — AWS's message for this class is the fixed token alone.

Cause 5 — sagemaker.notebook-instance-delete-while-inservice-rejected [mismatch]:
  assert message == ("Status (InService) not in ([Stopped, Failed]). Unable to transition to (Deleting) for Notebook Instance (<arn>).")
  AssertionError: Status (InService) not in (Stopped, Failed). Unable to transition to (Deleting) for Notebook Instance (arn:aws:sagemaker:<region>:<account>:notebook-instance/<name>).
Only the bracketing of the allowed-status set diverges; the status and ARN are request/state values.

Cause 6 — sagemaker.deploy-notebook-instance-cfn-invalid-role-rejected [error]:
  bad_described = sm.describe_notebook_instance(NotebookInstanceName=bad_notebook_name)
  botocore.exceptions.ClientError: An error occurred (ValidationException) when calling the DescribeNotebookInstance operation: RecordNotFound. Notebook Instance Arn: arn:aws:sagemaker:<region>:<account>:notebook-instance/<name>
The stack was created with DisableRollback=True and did reach CREATE_FAILED (that assertion passed); the check then expected NotebookInstanceStatus 'Failed' on the retained instance.

Cause 7 — sagemaker.deploy-notebook-instance-cfn-presigned-url-succeeds / -works / sagemaker.deploy-notebook-instance-cloudformation-presigned-url-accessible [all mismatch]:
  assert not (p := ctx.published_url_mismatch(url, labels=("notebook", ctx.region, "sagemaker"))), p
  AssertionError: the emulator published https://<notebook-name>.notebook.<region>.sagemaker.aws/auth?authToken=<hex>: its host is not one of the emulator's own hosts; a customer of the emulator needs http(s)://127.0.0.1:<port>/<resource path>, or http(s)://<own host>:<port>/<path> on a per-resource listener
The notebook name, token and port are per-run values; the requirement is that the published host/port is one the emulator serves.

Cause 8 — sagemaker.domain-pending-refuses-user-profile-space-and-presigned-url [mismatch]:
  assert code == "ValidationException", f"CreateUserProfile on a Pending domain: expected ValidationException, got {code!r}"
  AssertionError: CreateUserProfile on a Pending domain: expected ValidationException, got 'NotImplemented'

Smithy consulted (services/service-com-amazonaws-sagemaker-sagemaker/smithy-model/com-amazonaws-sagemaker-sagemaker.json): CreateDomain (DomainName/AuthMode/DefaultUserSettings required; SubnetIds @length 1..16; VpcId @length 0..32 pattern ^[-0-9a-zA-Z]+$; errors ResourceInUse, ResourceLimitExceeded, both client/400); DescribeDomain (output Status -> DomainStatus, FailureReason @length 0..1024; error ResourceNotFound); DescribeNotebookInstance (no modeled errors; output NotebookInstanceStatus, FailureReason); DeleteNotebookInstance (no modeled errors, empty output); CreateUserProfile (DomainId/UserProfileName required; errors ResourceInUse, ResourceLimitExceeded); CreatePresignedNotebookInstanceUrl (output AuthorizedUrl -> NotebookInstanceUrl; SessionExpirationDurationInSeconds @range 1800..43200).

## Steps to Reproduce
Replay the behavioral suite (run_behavioral_suite) and re-run the nine checks listed in trace_ids. Manually, per cause:

Cause 1: CreateDomain(DomainName=<n>, AuthMode='IAM', DefaultUserSettings={'ExecutionRole': <role>}, VpcId=<vpc>, SubnetIds=[<subnet>]), wait for Status=InService, then issue the identical CreateDomain again. Expect ResourceInUse/400 with "The Domain Name '<n>' already exists in this account in this region." and exactly one domain named <n> in ListDomains; the emulator returns 200 and a second DomainId. After DeleteDomain the identical call must succeed again.

Cause 2: CreateDomain with VpcId='not-a-real-vpc-id' and SubnetIds=['not-a-real-subnet-id']. Expect ValidationException/400 with the two-error sentence and no domain in ListDomains; the emulator returns 200 and persists the domain.

Cause 3: create two VPCs each with a subnet; CreateDomain with VpcId=<vpcA> but SubnetIds=[<subnet of vpcB>] (and SecurityGroups from vpcA). The call is accepted (200, Pending); poll DescribeDomain for a terminal Status. Expect 'Failed' with a FailureReason, and ListDomains to show it Failed; the emulator settles on 'InService' with no FailureReason. The self-consistent pairing must still reach InService.

Cause 4: CreateNotebookInstance, wait for InService, StopNotebookInstance, wait Stopped, DeleteNotebookInstance, then poll DescribeNotebookInstance until it is refused. Expect ValidationException with Message exactly "RecordNotFound"; the emulator appends ". Notebook Instance Arn: <arn>".

Cause 5: CreateNotebookInstance, wait for InService, then DeleteNotebookInstance. Expect ValidationException with "Status (InService) not in ([Stopped, Failed]). Unable to transition to (Deleting) for Notebook Instance (<arn>)."; the emulator omits the brackets. The instance must remain InService.

Cause 6: create an IAM role whose trust policy allows only ec2.amazonaws.com, then CreateStack(DisableRollback=True) for a template with one AWS::SageMaker::NotebookInstance using that RoleArn. The stack ends CREATE_FAILED; DescribeNotebookInstance on the declared name must then answer 200 with NotebookInstanceStatus='Failed'. The emulator refuses it with ValidationException RecordNotFound.

Cause 7: deploy a notebook instance (CFN or direct), wait for InService, call CreatePresignedNotebookInstanceUrl and inspect AuthorizedUrl's host — it must be an address the emulator serves, and fetching it must return an authenticated session.

Cause 8: CreateDomain and, while DescribeDomain still reports Status='Pending', call CreateUserProfile(DomainId=<id>, UserProfileName=<n>). Expect ValidationException/400 naming the domain's state; the emulator answers NotImplemented.
