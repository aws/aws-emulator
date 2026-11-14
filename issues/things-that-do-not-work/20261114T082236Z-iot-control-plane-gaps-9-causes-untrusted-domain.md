Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: iot.create-domain-configuration-invalid-certificate-rejected iot.domain-configuration-lifecycle iot.domain-configuration-lifecycle-transitions iot.telemetry-fleet-metric-lifecycle-and-aggregation iot.add-remove-thing-from-group iot.role-alias-lifecycle-and-delete-conflict iot.cancel-completed-audit-task-rejected iot.delete-default-policy-version-fails iot.job-targets-limit-exceeded-rejected iot.negative-fleet-metric-index-not-ready-blocked iot.provisioning-claim-unknown-template-rejected
Issue-Shape-Id: com.amazonaws.iot#AWSIotService
Issue-Key: iot.create-domain-configuration-invalid-certificate-rejected|_error#11|AssertionError|raise AssertionError(f"expected a ClientError, but the call SUCCEEDED and returned {result!r}")
Issue-Key: iot.domain-configuration-lifecycle|_error_code_and_message#17|AssertionError|raise AssertionError(f"expected a rejection, but the call SUCCEEDED and returned {result!r}")
Issue-Key: iot.domain-configuration-lifecycle-transitions|_error_code_and_message#8|AssertionError|raise AssertionError(f"expected a rejection, but the call SUCCEEDED and returned {result!r}")
Issue-Key: iot|CreateFleetMetric|InvalidRequestException
Issue-Key: iot.add-remove-thing-from-group|_eventually#11|Timeout|time.sleep(interval_s)
Issue-Key: iot.role-alias-lifecycle-and-delete-conflict|_fetch_role_alias_credentials#31|Timeout|time.sleep(3)
Issue-Key: iot.cancel-completed-audit-task-rejected|run#7|AssertionError|assert code == "InvalidRequestException", f"expected InvalidRequestException, got {code!r}"
Issue-Key: iot.delete-default-policy-version-fails|run#9|AssertionError|assert code == "DeleteConflictException", f"expected DeleteConflictException, got {code}"
Issue-Key: iot.job-targets-limit-exceeded-rejected|run#16|AssertionError|assert code == "LimitExceededException", f"expected LimitExceededException, got {code}"
Issue-Key: iot.negative-fleet-metric-index-not-ready-blocked|run#12|AssertionError|assert bucket_code == "ResourceNotFoundException", bucket_code
Issue-Key: iot.provisioning-claim-unknown-template-rejected|run#12|AssertionError|assert code == "ResourceNotFoundException", f"expected ResourceNotFoundException, got {code}"
Issue-Commit: dafb6a92f3304d60364f819845fa972c02211566

# iot control-plane gaps: 9 causes — untrusted domain-configuration certificates accepted, built-in registry aggregation fields rejected, group membership and the credentials-provider endpoint unobservable, and five refusal paths answer 501 NotImplemented

> _Note: this issue was identified against the code base at commit `dafb6a92f3304d60364f819845fa972c02211566`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateDomainConfiguration

## Description
Nine net-new causes in the iot control plane, all grounded in failing behavioral checks that the report marked neither ALREADY FILED nor OWNED. Causes 1-2 are wrong accept/refuse decisions, causes 3-4 are paths that never reach a verdict, and causes 5-9 are refusal paths that answer 501 NotImplemented where AWS answers a modeled error.

Cause 1 of 9: CreateDomainConfiguration accepts a server certificate that is not signed by a trusted CA.
  AWS: Smithy: CreateDomainConfiguration -> POST /domainConfigurations/{domainConfigurationName}, input serverCertificateArns (list, max 1), validationCertificateArn (ACM ARN pattern), serviceType; modeled errors include CertificateValidationException (400) and InvalidRequestException (400). Recorded behavior: when the ACM certificate named in serverCertificateArns is self-signed (or otherwise not chained to a trusted CA) and no usable validationCertificateArn is supplied, AWS REFUSES with InvalidRequestException whose message begins "Must provide validation certificate when using server certificates not signed by trusted CA or not within" — and no domain configuration is created, so a following DescribeDomainConfiguration answers ResourceNotFoundException. This holds whether or not the certificate's SAN covers the domain name.
  Emulator: HTTP 200 with the full CreateDomainConfigurationResponse (domainConfigurationName, domainConfigurationArn) — the configuration is created. The emulator validates only that the ACM ARN resolves to something (the non-existent-ARN leg is refused correctly); it never inspects the certificate's trust chain or requires a validation certificate.
  Why: an operator wiring a custom IoT domain gets a configuration that reads back ACTIVE in the emulator and is rejected outright by AWS — the deployment passes in test and fails in production, and the negative control a caller relies on to catch a mis-issued certificate is silently inverted. This is the overly-permissive direction, the worst kind for a pre-production emulator.
  Fix: the CreateDomainConfiguration handler's validation step — for a CUSTOMER_MANAGED/custom-domain configuration, require a validationCertificateArn (or a trusted chain) for the supplied server certificate and refuse with InvalidRequestException otherwise, creating nothing.
  Traces: iot.create-domain-configuration-invalid-certificate-rejected, iot.domain-configuration-lifecycle, iot.domain-configuration-lifecycle-transitions.

Cause 2 of 9: CreateFleetMetric refuses a built-in registry aggregation field as "not indexed".
  AWS: Smithy: CreateFleetMetric input aggregationField (required, min length 1), indexName, aggregationType; errors include InvalidRequestException (400) and IndexNotReadyException. Recorded behavior: with thingIndexingConfiguration.thingIndexingMode = REGISTRY and the AWS_Things index ACTIVE, a Cardinality aggregation over the built-in registry field "registry.thingTypeName" is ACCEPTED (HTTP 200) — registry.* fields are part of REGISTRY indexing and do not have to be declared as custom fields. Only user attribute fields (attributes.<name>) must be declared via UpdateIndexingConfiguration customFields.
  Emulator: 400 InvalidRequestException "Aggregation field registry.thingTypeName is not indexed in index AWS_Things; add it as a custom field by calling UpdateIndexingConfiguration". The handler treats every aggregationField as requiring a customFields declaration, so the whole fleet-metric lifecycle (create -> describe -> update aggregation -> delete) is unreachable.
  Why: no caller can create a fleet metric over the registry fields AWS documents as aggregatable; the refusal also tells the operator to do something AWS rejects (declaring a reserved registry.* name as a custom field).
  Fix: the CreateFleetMetric/UpdateFleetMetric validation step — treat the built-in registry.* and connectivity.* fields as indexed whenever the corresponding indexing mode is on, and require a customFields declaration only for attributes.* fields.
  Traces: iot.telemetry-fleet-metric-lifecycle-and-aggregation.

Cause 3 of 9: thing-group membership is never observable after AddThingToThingGroup.
  AWS: Smithy: AddThingToThingGroup -> PUT /thing-groups/addThingToThingGroup (200, empty body); ListThingsInThingGroup and ListThingGroupsForThing read the membership back. Recorded behavior: membership propagates within seconds — ListThingsInThingGroup(thingGroupName=G) contains the thing name and ListThingGroupsForThing(thingName=T) contains the group, and after RemoveThingFromThingGroup the thing is gone from the listing.
  Emulator: the check polls both listings (interval 3 s) and neither ever satisfies the predicate; the check is killed at the harness's 60 s cap with no verdict, so the add/remove round trip cannot be observed at all.
  Why: thing-group membership is the foundation of job targeting, policy attachment by group and fleet indexing; a client that writes a membership and then reads it back blocks forever.
  Fix: the membership store shared by AddThingToThingGroup / RemoveThingFromThingGroup and the two listing handlers — the write must be visible to the reads. Note: ListThingGroupsForThing is reported as an unimplemented scaffold stub by a separate auto-filed issue (it blocks iot.thing-group-listings-for-thing); if that stub is the only reason this check hangs, this cause closes with it, but the ListThingsInThingGroup leg must be confirmed too, since the check polls it FIRST.
  Traces: iot.add-remove-thing-from-group.

Cause 4 of 9: the published iot:CredentialProvider endpoint serves no mTLS credential exchange.
  AWS: recorded behavior only (the credentials provider is not in the Smithy model). DescribeEndpoint(endpointType='iot:CredentialProvider') returns an address that serves GET /role-aliases/{roleAlias}/credentials over mutual TLS, authenticated by the device certificate, and answers 200 with a JSON body whose credentials object carries accessKeyId, secretAccessKey and sessionToken.
  Emulator: the request never succeeds — the check retries the mTLS GET every 3 s for up to 90 s and is killed at the harness's 60 s cap, so the role-alias lifecycle (describe/update round trip of credentialDurationSeconds, the attached-policy DeleteConflictException, the ACTIVE-certificate CertificateStateException, DeleteRoleAlias and the post-delete refusals) never reaches a verdict.
  Why: the IoT credentials provider is how a device exchanges its certificate for AWS credentials; nothing that uses a role alias can be exercised against the emulator.
  Fix: the published iot:CredentialProvider endpoint must be an address the emulator serves, and it must handle GET /role-aliases/{roleAlias}/credentials with client-certificate authentication. Related open issues: 20261111T093215Z-iot-3-net-new-causes-the-published-endpoint.md and 20261110T222008Z-iot-the-published-data-plane-address-serves-no.md describe the same endpoint family but do not carry this check's replay key.
  Traces: iot.role-alias-lifecycle-and-delete-conflict.

Cause 5 of 9: CancelAuditTask answers 501 NotImplemented.
  AWS: Smithy: CancelAuditTask -> PUT /audit/tasks/{taskId}/cancel, output empty, modeled errors InvalidRequestException (400), ResourceNotFoundException (404), InternalFailureException, ThrottlingException. Recorded behavior: cancelling a task that has already reached COMPLETED is refused with InvalidRequestException and the task's status stays COMPLETED; cancelling a freshly started task is accepted (or refused naming the terminal state it already reached).
  Emulator: the error code is NotImplemented — "operation ... is scaffolded but not implemented" — which is not a modeled error of this operation, so the task-state guard cannot be exercised at all.
  Fix: implement the CancelAuditTask handler including the terminal-state guard.
  Traces: iot.cancel-completed-audit-task-rejected.

Cause 6 of 9: DeletePolicyVersion answers 501 NotImplemented on the default-version guard.
  AWS: Smithy: DeletePolicyVersion -> DELETE /policies/{policyName}/version/{policyVersionId}, modeled errors include DeleteConflictException (409) and ResourceNotFoundException (404). Recorded behavior: deleting the version that is currently the policy's DEFAULT version is refused with DeleteConflictException; the version remains.
  Emulator: NotImplemented. A separate auto-filed issue reports DeletePolicyVersion as an unimplemented scaffold (it blocks iot.policy-version-limit-exceeded); this cause carries the default-version guard's own check, which that issue does not name.
  Fix: implement the DeletePolicyVersion handler, refusing a delete of the default version with DeleteConflictException.
  Traces: iot.delete-default-policy-version-fails.

Cause 7 of 9: AssociateTargetsWithJob answers 501 NotImplemented on the target-limit guard.
  AWS: Smithy: AssociateTargetsWithJob -> POST /jobs/{jobId}/targets, modeled errors include LimitExceededException (httpError 410), InvalidRequestException and ResourceNotFoundException. Recorded behavior: pushing a CONTINUOUS IN_PROGRESS job past 100 targets is refused with LimitExceededException and the job's existing target list is left unchanged rather than truncated.
  Emulator: NotImplemented. A separate auto-filed issue reports AssociateTargetsWithJob as an unimplemented scaffold (it blocks iot.associate-targets-exceeds-limit-rejected); this cause carries the limit-guard check it does not name.
  Fix: implement the AssociateTargetsWithJob handler including the 100-target limit.
  Traces: iot.job-targets-limit-exceeded-rejected.

Cause 8 of 9: GetBucketsAggregation answers 501 NotImplemented when the index is absent.
  AWS: Smithy: GetBucketsAggregation -> POST /indices/buckets, modeled errors include ResourceNotFoundException (404), IndexNotReadyException (400), InvalidAggregationException and InvalidQueryException. Recorded behavior: with thingIndexingMode OFF the AWS_Things index does not exist, so both CreateFleetMetric and GetBucketsAggregation are refused with ResourceNotFoundException naming the missing index, and the same calls succeed once the index is ACTIVE.
  Emulator: NotImplemented, so the index-not-ready contrast cannot be observed. Open issue 20261106T201842Z-iot-fleet-indexing-reads-are-unimplemented-stubs-getbucketsaggregation.md owns iot.get-buckets-aggregation-index-not-ready-rejected but not this check.
  Fix: implement GetBucketsAggregation, including the absent-index refusal.
  Traces: iot.negative-fleet-metric-index-not-ready-blocked.

Cause 9 of 9: CreateProvisioningClaim answers 501 NotImplemented for an unknown template.
  AWS: Smithy: CreateProvisioningClaim -> POST /provisioning-templates/{templateName}/provisioning-claim, modeled errors include ResourceNotFoundException (404) and InvalidRequestException. Recorded behavior: an unknown templateName is refused with ResourceNotFoundException whose message is "Unable to find template named " + <templateName>; a real template returns an Amazon-issued certificate whose NotAfter matches the reported expiration.
  Emulator: NotImplemented. A separate auto-filed issue reports CreateProvisioningClaim as an unimplemented scaffold (it blocks iot.e2e-fleet-provisioning-claim-to-thing); this cause carries the unknown-template refusal check it does not name.
  Fix: implement CreateProvisioningClaim including the unknown-template refusal and its message.
  Traces: iot.provisioning-claim-unknown-template-rejected.

## Evidence
Behavioral suite simparity-behavioral-aws-v1 replayed against the emulator built from this checkout (service heading '### iot').

Cause 1 — iot.create-domain-configuration-invalid-certificate-rejected [mismatch]:
  AssertionError: expected a ClientError, but the call SUCCEEDED and returned {'ResponseMetadata': {'HTTPStatusCode': 200, ...}, 'domainConfigurationName': '<name>', 'domainConfigurationArn': 'arn:aws:iot:<region>:<account>:domainconfiguration/<name>'}
  Expected: 400 InvalidRequestException, message beginning "Must provide validation certificate when using server certificates not signed by trusted CA or not within", and nothing created.
  iot.domain-configuration-lifecycle and iot.domain-configuration-lifecycle-transitions [mismatch] fail identically: "expected a rejection, but the call SUCCEEDED and returned {... 'HTTPStatusCode': 200 ... 'domainConfigurationArn': 'arn:aws:iot:<region>:<account>:domainconfiguration/<name>'}" for a CUSTOMER_MANAGED configuration whose server certificate is self-signed and which supplies no validationCertificateArn.

Cause 2 — iot.telemetry-fleet-metric-lifecycle-and-aggregation [error]:
  botocore.errorfactory.InvalidRequestException: An error occurred (InvalidRequestException) when calling the CreateFleetMetric operation: Aggregation field registry.thingTypeName is not indexed in index AWS_Things; add it as a custom field by calling UpdateIndexingConfiguration
  Request: thingIndexingConfiguration={'thingIndexingMode': 'REGISTRY'} applied and the index polled to ACTIVE first, then CreateFleetMetric(queryString='thingName:*', aggregationType={'name': 'Cardinality'}, period=300, aggregationField='registry.thingTypeName'). Expected: HTTP 200.

Cause 3 — iot.add-remove-thing-from-group [mismatch]: "Check timed out after 60.0s". The check creates a thing group and a thing, calls AddThingToThingGroup, then polls ListThingsInThingGroup(thingGroupName=G) for the thing and ListThingGroupsForThing(thingName=T) for the group every 3 s; neither predicate is ever satisfied, so the 60 s harness cap is hit before the remove leg is reached.

Cause 4 — iot.role-alias-lifecycle-and-delete-conflict [mismatch]: "Check timed out after 60.0s". The only long-poll in the check is the mTLS GET to DescribeEndpoint(endpointType='iot:CredentialProvider') + "/role-aliases/<roleAlias>/credentials", retried every 3 s and expecting status 200 with body.credentials containing accessKeyId/secretAccessKey/sessionToken.

Cause 5 — iot.cancel-completed-audit-task-rejected [mismatch]: AssertionError: expected InvalidRequestException, got 'NotImplemented' (CancelAuditTask on a COMPLETED audit task).
Cause 6 — iot.delete-default-policy-version-fails [mismatch]: AssertionError: expected DeleteConflictException, got NotImplemented (DeletePolicyVersion on the policy's default version).
Cause 7 — iot.job-targets-limit-exceeded-rejected [mismatch]: AssertionError: expected LimitExceededException, got NotImplemented (AssociateTargetsWithJob past 100 targets).
Cause 8 — iot.negative-fleet-metric-index-not-ready-blocked [mismatch]: AssertionError: NotImplemented at `assert bucket_code == "ResourceNotFoundException"` (GetBucketsAggregation with thingIndexingMode OFF).
Cause 9 — iot.provisioning-claim-unknown-template-rejected [mismatch]: AssertionError: expected ResourceNotFoundException, got NotImplemented (CreateProvisioningClaim for a template that does not exist).

Names, ARNs, account ids, regions and timestamps in the output above are per-run values from each check's own setup; the requirements are the accept/refuse decision, the HTTP status, the error code and the message's fixed text with its request/state-derived parts.

Smithy consulted: services/service-com-amazonaws-iot-awsiotservice/smithy-model/com-amazonaws-iot-awsiotservice.json — CreateDomainConfiguration (POST /domainConfigurations/{domainConfigurationName}; CertificateValidationException 400, InvalidRequestException 400), CancelAuditTask (PUT /audit/tasks/{taskId}/cancel; InvalidRequestException 400, ResourceNotFoundException 404), DeletePolicyVersion (DELETE /policies/{policyName}/version/{policyVersionId}; DeleteConflictException 409), AssociateTargetsWithJob (POST /jobs/{jobId}/targets; LimitExceededException 410), GetBucketsAggregation (POST /indices/buckets; ResourceNotFoundException 404, IndexNotReadyException 400), CreateProvisioningClaim (POST /provisioning-templates/{templateName}/provisioning-claim; ResourceNotFoundException 404), DescribeFleetMetric / AddThingToThingGroup as cited.

## Steps to Reproduce
1. Boot the emulator built from this checkout and point iot/acm clients at it.
2. Cause 1: generate a self-signed certificate for <name>.example.com, ImportCertificate it into ACM, then CreateDomainConfiguration(domainConfigurationName=<name>, domainName=<name>.example.com, serverCertificateArns=[<that ARN>], serviceType='DATA') with no validationCertificateArn. The emulator answers 200 and creates the configuration; AWS refuses with InvalidRequestException and creates nothing.
3. Cause 2: UpdateIndexingConfiguration(thingIndexingConfiguration={'thingIndexingMode': 'REGISTRY'}), wait for DescribeIndex(indexName='AWS_Things').indexStatus == 'ACTIVE', then CreateFleetMetric(metricName=M, queryString='thingName:*', aggregationType={'name':'Cardinality'}, period=300, aggregationField='registry.thingTypeName'). The emulator refuses with InvalidRequestException; AWS answers 200.
4. Cause 3: CreateThingGroup(G), CreateThing(T), AddThingToThingGroup(thingGroupName=G, thingName=T), then poll ListThingsInThingGroup(thingGroupName=G) and ListThingGroupsForThing(thingName=T): neither ever reports the membership.
5. Cause 4: create an IAM role trusted by credentials.iot.amazonaws.com, CreateRoleAlias(roleAlias=A, roleArn=..., credentialDurationSeconds=900), CreateKeysAndCertificate(setAsActive=true), attach a policy allowing iot:AssumeRoleWithCertificate on the alias ARN, then GET https://<DescribeEndpoint(iot:CredentialProvider)>/role-aliases/A/credentials with that certificate as the TLS client cert: no 200 is ever served.
6. Cause 5: enable an audit check, StartOnDemandAuditTask, wait for COMPLETED, then CancelAuditTask(taskId=...): the emulator answers NotImplemented.
7. Cause 6: CreatePolicy, CreatePolicyVersion(setAsDefault=true), then DeletePolicyVersion on the default version: NotImplemented.
8. Cause 7: CreateJob(targetSelection='CONTINUOUS') and AssociateTargetsWithJob past 100 targets: NotImplemented.
9. Cause 8: UpdateIndexingConfiguration(thingIndexingConfiguration={'thingIndexingMode': 'OFF'}), then GetBucketsAggregation(queryString='thingName:*', aggregationField='registry.version', bucketsAggregationType=...): NotImplemented.
10. Cause 9: CreateProvisioningClaim(templateName=<never created>): NotImplemented.
11. Re-replay the eleven checks in trace_ids to confirm each cause is closed.
