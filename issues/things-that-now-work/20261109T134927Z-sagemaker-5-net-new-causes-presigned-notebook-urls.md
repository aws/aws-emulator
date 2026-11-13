Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: sagemaker.presigned-notebook-url-expires-after-ttl sagemaker.presigned-notebook-instance-url-expires-after-ttl sagemaker.get-lineage-group-policy-missing-not-found sagemaker.get-lineage-group-policy-missing sagemaker.get-lineage-group-policy-not-found sagemaker.batch-add-cluster-nodes-wrong-state-rejected sagemaker.batch-describe-model-package-matches-individual-creates
Issue-Shape-Id: com.amazonaws.sagemaker#SageMaker
Issue-Key: sagemaker.presigned-notebook-url-expires-after-ttl|run#10|AssertionError|assert not (p := ctx.published_url_mismatch(url, labels=("notebook", ctx.region, "sagemaker"))), p
Issue-Key: sagemaker.presigned-notebook-instance-url-expires-after-ttl|_routed_request#8|PublishedUrlRejected|target, route = ctx.endpoint_route(url)
Issue-Key: sagemaker.get-lineage-group-policy-missing-not-found|run#2|AssertionError|assert _lineage_group_exists(sm), (
Issue-Key: sagemaker.get-lineage-group-policy-missing|run#8|AssertionError|assert message == (
Issue-Key: sagemaker.get-lineage-group-policy-not-found|run#8|AssertionError|assert message == (
Issue-Key: sagemaker.batch-add-cluster-nodes-wrong-state-rejected|run#20|AssertionError|assert message == expected_message, f"expected message {expected_message!r}, got {message!r}"
Issue-Key: sagemaker.batch-describe-model-package-matches-individual-creates|run#21|AssertionError|assert (bogus_error["ErrorCode"], bogus_error["ErrorResponse"]) == expected, (
Issue-Commit: e901ea078181297e70b2ac3a5c8be54768ac76d3

# sagemaker: 5 net-new causes — presigned notebook URLs point at an unreachable aws.com host, the default lineage group is never provisioned, and three refusals carry emulator-worded codes/messages

> _Note: this issue was identified against the code base at commit `e901ea078181297e70b2ac3a5c8be54768ac76d3`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreatePresignedNotebookInstanceUrl

## Description
Five net-new behavioral root causes on the SageMaker surface. None of the failing checks below is marked ALREADY FILED or OWNED by the replay, so no open issue carries their replay keys.

Cause 1 of 5: CreatePresignedNotebookInstanceUrl publishes an AuthorizedUrl whose host is not an address the emulator serves.
AWS: Smithy: CreatePresignedNotebookInstanceUrl input NotebookInstanceName (required) + SessionExpirationDurationInSeconds (@range 1800..43200); output AuthorizedUrl -> NotebookInstanceUrl, a URL the caller then fetches to obtain an authenticated notebook session, which stops authenticating once the session window has elapsed. The host in that URL must be a host that actually serves the authenticated session for the caller. Recorded behavior only for the host's form; what is a requirement is that the published URL is reachable by the client that received it.
Emulator: returns AuthorizedUrl "https://<notebook-name>.notebook.<region>.sagemaker.aws/auth?authToken=<token>" — a real-AWS hostname the emulator does not serve and the client cannot resolve or reach. The harness refuses it: "its host is not one of the emulator's own hosts; a customer of the emulator needs http(s)://127.0.0.1:<port>/<resource path>, or http(s)://<own host>:<port>/<path> on a per-resource listener". Because the URL is unreachable, neither the fresh-URL-authenticates nor the URL-stops-authenticating-after-its-window behavior can be observed at all.
Why: a client that does what the API exists for — fetch the AuthorizedUrl to open a notebook session — is sent to an address outside the emulator (or to the real AWS name), so every presigned-URL flow fails with a network/DNS error instead of an authenticated session, and the session-expiry contract is unverifiable. The emulator announced an address other than the one it is reached on.
Fix: the CreatePresignedNotebookInstanceUrl handler's URL construction must publish the emulator's own externally reachable host and port (the same endpoint the request arrived on, or a per-resource listener it owns) with the notebook auth path and token, and the emulator must serve that path: honouring a fresh token and refusing one past SessionExpirationDurationInSeconds. The same construction backs the Domain/Studio presigned-URL handlers.
Traces: sagemaker.presigned-notebook-url-expires-after-ttl, sagemaker.presigned-notebook-instance-url-expires-after-ttl.

Cause 2 of 5: the account's default lineage group is never provisioned, so lineage reads have nothing to resolve.
AWS: creating any lineage entity (CreateContext/CreateArtifact/CreateAction) without an explicit lineage group associates it with the account's implicit default lineage group, whose name is the fixed literal "sagemaker-default-lineage-group"; that group then appears in ListLineageGroups, DescribeLineageGroup(LineageGroupName="sagemaker-default-lineage-group") succeeds, and GetLineageGroupPolicy on it returns 200 with ResourcePolicy "{}" when no policy is attached (Smithy: GetLineageGroupPolicy output LineageGroupArn + ResourcePolicy, @length 0..20480).
Emulator: after CreateContext succeeds, the default lineage group still does not exist — ListLineageGroups stays empty and DescribeLineageGroup("sagemaker-default-lineage-group") does not resolve, so the positive control and the empty-policy read are both unreachable.
Why: every lineage traversal has no entry point: a client that creates lineage entities and then lists or describes their group, or reads the group's resource policy, is told the group does not exist for an account that in AWS always has one. This compounds the already-open issue 20261109T123745Z-sagemaker-lineage-entity-describes-report-a-wrong-default.md (describes return a region/account-derived group name instead of the fixed one): even the correct name resolves to nothing here, and that issue does not own this check.
Fix: the lineage state must materialize the default lineage group under its fixed name on first use (whenever a lineage entity is created without an explicit group), and ListLineageGroups/DescribeLineageGroup/GetLineageGroupPolicy must read it from there.
Traces: sagemaker.get-lineage-group-policy-missing-not-found.

Cause 3 of 5: GetLineageGroupPolicy's not-found message is reworded.
AWS: Smithy: GetLineageGroupPolicy input LineageGroupName (required, name or ARN), sole modeled error ResourceNotFound (client, HTTP 400), whose Message for an absent group is the sentence "LineageGroup '<lineage-group-arn>' does not exist." — fixed text "LineageGroup '" ... "' does not exist." with the ARN arn:aws:sagemaker:<region>:<account>:lineage-group/<name> built from the request's name and the caller's region/account.
Emulator: the refusal and the ResourceNotFound code are right, and the interpolated ARN is right, but the leading noun is spelled "Lineage group" (two words, lower-case g) instead of the resource type name "LineageGroup": "Lineage group '<arn>' does not exist."
Why: a client or operator matching this refusal text to distinguish a missing lineage group from a missing policy does not match, so the branch that would create or bootstrap the group never runs.
Fix: the GetLineageGroupPolicy handler's not-found path message template.
Traces: sagemaker.get-lineage-group-policy-missing, sagemaker.get-lineage-group-policy-not-found.

Cause 4 of 5: BatchAddClusterNodes against a terminating HyperPod cluster is refused with the wrong message.
AWS: Smithy: BatchAddClusterNodes input ClusterName (required) + NodesToAdd (@length 1..5), output Successful/Failed both required, modeled errors ResourceLimitExceeded and ResourceNotFound (both 400). A cluster being deleted is refused with ValidationException whose Message is the sentence "Cluster <cluster-arn> is in a termination state that cannot be modified." — fixed text "Cluster " ... " is in a termination state that cannot be modified." with the cluster ARN (unquoted) taken from the request's cluster; the instance group's target count is left unchanged.
Emulator: refuses with the right error class but an emulator-authored sentence that quotes the ARN and names the status: 'Cluster "<cluster-arn>" cannot be changed while it is in Deleting status.' The positive control (the same call while InService) succeeds, so only the refusal wording diverges.
Why: a client that matches the termination-state refusal (to stop retrying node additions against a cluster that is going away, rather than treating it as a transient capacity error) does not match and retries until its own timeout; the quoting of the ARN also breaks naive ARN extraction from the message.
Fix: the BatchAddClusterNodes handler's cluster-state guard message template.
Traces: sagemaker.batch-add-cluster-nodes-wrong-state-rejected.

Cause 5 of 5: BatchDescribeModelPackage's per-ARN error entry uses a top-level exception's code and message instead of the batch error vocabulary.
AWS: Smithy: BatchDescribeModelPackage models NO top-level errors; unresolvable ARNs are reported per entry in BatchDescribeModelPackageErrorMap, keyed by the model-package ARN, whose value BatchDescribeModelPackageError has required members ErrorCode and ErrorResponse. For an ARN that names no model package the entry is ErrorCode "EntityNotFoundError" and ErrorResponse "The ARN is not found." — both are fixed text, with no request values interpolated.
Emulator: returns an entry of ErrorCode "ValidationException" and ErrorResponse "ModelPackage <model-package-arn> does not exist." — it reuses the single-describe exception code and interpolates the requested ARN into the message, so neither required member carries the value AWS defines.
Why: a client that batches describes and switches on ErrorCode == "EntityNotFoundError" to tell "this package is gone" from "my request was malformed" mis-classifies every missing package as a client-side validation fault, and typically aborts the whole batch instead of skipping the one ARN. The related open issue 20261109T020211Z-sagemaker-2-net-new-causes-batchdescribemodelpackage-and-describemodelpackage.md covers this operation's ModelPackageStatus disagreement and the CreateModelPackage token message, not the error-map vocabulary, and the replay reports this check as unowned.
Fix: the BatchDescribeModelPackage handler's error-map construction (the per-ARN not-found branch) must emit the fixed ErrorCode/ErrorResponse pair rather than forwarding the single-describe ResourceNotFound/ValidationException text.
Traces: sagemaker.batch-describe-model-package-matches-individual-creates.

## Evidence
Cause 1 — sagemaker.presigned-notebook-url-expires-after-ttl [mismatch]:
  assert not (p := ctx.published_url_mismatch(url, labels=("notebook", ctx.region, "sagemaker"))), p
  AssertionError: the emulator published https://<nb-name>.notebook.us-west-2.sagemaker.aws/auth?authToken=<hex>: its host is not one of the emulator's own hosts; a customer of the emulator needs http(s)://127.0.0.1:<port>/<resource path>, or http(s)://<own host>:<port>/<path> on a per-resource listener
and sagemaker.presigned-notebook-instance-url-expires-after-ttl [mismatch]:
  simparity.published.PublishedUrlRejected: endpoint_url was given https://<nb-name>.notebook.us-west-2.sagemaker.aws/auth?authToken=<hex> (published by the emulator or built by the check): its host is not one of the emulator's own hosts ...
The notebook name, token and port are per-run values; the requirement is that AuthorizedUrl's host/port is one the emulator serves and that fetching it yields an authenticated session which stops authenticating after SessionExpirationDurationInSeconds.

Cause 2 — sagemaker.get-lineage-group-policy-missing-not-found [mismatch]:
  assert _lineage_group_exists(sm), ...
  AssertionError: expected creating context '<name>' to provision the default lineage group 'sagemaker-default-lineage-group', but it still does not exist
(the check then wanted DescribeLineageGroup(LineageGroupName='sagemaker-default-lineage-group') to succeed and GetLineageGroupPolicy on it to return ResourcePolicy '{}'). The context name is a per-run value; 'sagemaker-default-lineage-group' is fixed text.

Cause 3 — sagemaker.get-lineage-group-policy-not-found and sagemaker.get-lineage-group-policy-missing [both mismatch]:
  expected Message: "LineageGroup 'arn:aws:sagemaker:<region>:<account>:lineage-group/<name>' does not exist." with code ResourceNotFound
  got:              "Lineage group 'arn:aws:sagemaker:us-west-2:000000000000:lineage-group/<name>' does not exist."
The code matched; only the leading noun diverges. The region, account and group name are request/environment values, not part of the template.

Cause 4 — sagemaker.batch-add-cluster-nodes-wrong-state-rejected [mismatch]:
  assert message == expected_message
  AssertionError: expected message 'Cluster arn:aws:sagemaker:<region>:<account>:cluster/<id> is in a termination state that cannot be modified.', got 'Cluster "arn:aws:sagemaker:<region>:<account>:cluster/<id>" cannot be changed while it is in Deleting status.'
The cluster ARN is a per-run value; the sentence around it is fixed text.

Cause 5 — sagemaker.batch-describe-model-package-matches-individual-creates [mismatch]:
  assert (bogus_error["ErrorCode"], bogus_error["ErrorResponse"]) == expected
  AssertionError: expected ErrorCode and ErrorResponse ('EntityNotFoundError', 'The ARN is not found.') for a non-existent ARN, got {'ErrorCode': 'ValidationException', 'ErrorResponse': 'ModelPackage arn:aws:sagemaker:<region>:<account>:model-package/<group>/9999 does not exist.'}
Both expected values are fixed text with nothing interpolated.

Smithy consulted (services/service-com-amazonaws-sagemaker-sagemaker/smithy-model/com-amazonaws-sagemaker-sagemaker.json): CreatePresignedNotebookInstanceUrl (output AuthorizedUrl -> NotebookInstanceUrl; SessionExpirationDurationInSeconds @range 1800..43200; no modeled errors); GetLineageGroupPolicy (errors: ResourceNotFound, client, 400; output LineageGroupArn @pattern ^arn:aws[a-z\-]*:sagemaker:[a-z0-9\-]*:[0-9]{12}:lineage-group/ + ResourcePolicy @length 0..20480); BatchAddClusterNodes (NodesToAdd @length 1..5; Successful/Failed required; errors ResourceLimitExceeded, ResourceNotFound); BatchDescribeModelPackage (no top-level errors; BatchDescribeModelPackageErrorMap keyed by ModelPackageArn, value BatchDescribeModelPackageError with required ErrorCode and ErrorResponse).

## Steps to Reproduce
Replay the behavioral suite (run_behavioral_suite) and re-run the seven checks listed in trace_ids. Manually, per cause:

Cause 1: CreateNotebookInstance, wait for InService, then CreatePresignedNotebookInstanceUrl(NotebookInstanceName=<nb>, SessionExpirationDurationInSeconds=1800) and inspect AuthorizedUrl's host. It must be an address the emulator serves (e.g. the endpoint host/port the request arrived on, or a per-resource listener it owns); the emulator returns <nb>.notebook.<region>.sagemaker.aws, which does not resolve to the emulator. Fetch the URL: it must return an authenticated session, and the same URL fetched after its expiry window must no longer authenticate while DescribeNotebookInstance still reports InService.

Cause 2: CreateContext(ContextName=<name>, Source=..., ContextType=...), then ListLineageGroups and DescribeLineageGroup(LineageGroupName='sagemaker-default-lineage-group'), then GetLineageGroupPolicy on that group. AWS lists/describes the default group and returns ResourcePolicy '{}'; the emulator has no such group.

Cause 3: GetLineageGroupPolicy(LineageGroupName=<a name that does not exist>) and read Error.Code/Error.Message. Expect ResourceNotFound and "LineageGroup '<arn>' does not exist."; the emulator says "Lineage group '<arn>' does not exist."

Cause 4: CreateCluster (Slurm, a controller group and a zero-count worker group), wait for InService, confirm BatchAddClusterNodes succeeds, then DeleteCluster and immediately call BatchAddClusterNodes(ClusterName=<name>, NodesToAdd=[{InstanceGroupName:'workers', IncrementTargetCountBy:1}]). Read the ValidationException Message: expect "Cluster <arn> is in a termination state that cannot be modified."; the emulator returns the 'cannot be changed while it is in Deleting status.' wording.

Cause 5: CreateModelPackageGroup + CreateModelPackage to get a real ARN, then BatchDescribeModelPackage(ModelPackageArnList=[<real arn>, <same group, version 9999>]) and read BatchDescribeModelPackageErrorMap[<bogus arn>]: expect ErrorCode 'EntityNotFoundError' and ErrorResponse 'The ARN is not found.'; the emulator returns 'ValidationException' / 'ModelPackage <arn> does not exist.'
