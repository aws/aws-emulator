Issue-Source: trace-sweep-divergence
Issue-Trace: imagebuilder-infrastructure-configuration-round-trips-profile-subnet-and-security-group
Issue-Step: task#8
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: imagebuilder-infrastructure-configuration-round-trips-profile-subnet-and-security-group imagebuilder-infrastructure-configuration-update-accepts-an-unknown-instance-type-subnet-and-group
Issue-Shape-Id: com.amazonaws.imagebuilder#imagebuilder
Issue-Key: imagebuilder|GetInfrastructureConfiguration|2xx->2xx|$.infrastructureConfiguration.dateUpdated
Issue-Key: imagebuilder|GetInfrastructureConfiguration|2xx->2xx|$.infrastructureConfiguration.terminateInstanceOnCompletion
Issue-Key: imagebuilder|GetInfrastructureConfiguration|2xx->2xx|$.infrastructureConfiguration.instanceTypes[*]
Issue-Key: imagebuilder|GetInfrastructureConfiguration|2xx->2xx|$.infrastructureConfiguration.subnetId
Issue-Key: imagebuilder|GetInfrastructureConfiguration|2xx->2xx|$.infrastructureConfiguration.securityGroupIds[*]
Issue-Commit: 74dcf08ec50b24aa2701d0c20dd3470a3fa969ed

# imagebuilder: GetInfrastructureConfiguration body diverges - UpdateInfrastructureConfiguration changes not persisted and unset members emitted

> _Note: this issue was identified against the code base at commit `74dcf08ec50b24aa2701d0c20dd3470a3fa969ed`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetInfrastructureConfiguration

## Description
The GetInfrastructureConfiguration read model diverges from AWS in two ways: it synthesizes members AWS leaves absent, and it serves pre-update state after an accepted UpdateInfrastructureConfiguration. FOUR distinct causes are bundled here; all four must be closed. No open issue covers them - the nearest imagebuilder infrastructure-configuration issues are 20261103T091826Z-imagebuilder-2-net-new-delete-path-divergences-deleteinfrastructureconfiguration.md (delete path answering 501) and 20261108T022729Z-imagebuilder-listinfrastructureconfigurations-stub.md (unimplemented List), neither of which touches the Get read model or update persistence.

Cause 1 of 4 - GetInfrastructureConfiguration emits dateUpdated and terminateInstanceOnCompletion for a configuration that was never updated and never supplied them (fixture imagebuilder-infrastructure-configuration-round-trips-profile-subnet-and-security-group, step task#8).
AWS: Smithy: InfrastructureConfiguration.dateUpdated targets DateTime, doc "The date on which the infrastructure configuration was last updated"; InfrastructureConfiguration.terminateInstanceOnCompletion targets NullableBoolean (nullable, NO @default) - as does the CreateInfrastructureConfiguration input member of the same name. Neither is @required and neither has a modeled default, so absence is meaningful: a configuration created and not yet updated has no last-updated date, and a boolean the caller never supplied has no server-side value to report. The recording confirms it: the AWS response for this configuration carries neither member.
Emulator: returns HTTP 200 with both members present - $.infrastructureConfiguration.dateUpdated set to a timestamp (the create time) and $.infrastructureConfiguration.terminateInstanceOnCompletion set to true - where AWS omits both ("extra (expected None, got ...)").
Why: a client cannot distinguish "never updated" from "updated" because dateUpdated is always populated, so change-detection and drift-reconciliation logic (CloudFormation/Terraform-style refresh, "has this config changed since I wrote it" polls) always sees an update timestamp. And a caller reading back terminateInstanceOnCompletion sees an asserted true it never set, so it believes Image Builder will terminate build/test instances on success when AWS has recorded no such setting - the opposite inference from an unset nullable boolean.
Fix: the GetInfrastructureConfiguration serializer / the InfrastructureConfiguration response projection in the imagebuilder service. Carry dateUpdated and terminateInstanceOnCompletion as optional stored values and skip them during serialization when unset, instead of defaulting dateUpdated to dateCreated and terminateInstanceOnCompletion to true at create time.
Traces: imagebuilder-infrastructure-configuration-round-trips-profile-subnet-and-security-group#task#8.

Cause 2 of 4 - an accepted UpdateInfrastructureConfiguration does not persist instanceTypes (fixture imagebuilder-infrastructure-configuration-update-accepts-an-unknown-instance-type-subnet-and-group, step task#12).
AWS: Smithy: UpdateInfrastructureConfiguration.instanceTypes targets InstanceTypeList with no enum or existence constraint, and the operation's modeled errors are CallRateLimitExceeded/Client/Forbidden/IdempotentParameterMismatch/InvalidRequest/ResourceInUse/Service/ServiceUnavailable - none of which expresses "instance type not found". Recorded behavior: the Update call is accepted with HTTP 200 (that step replays clean) and the following GetInfrastructureConfiguration echoes back the instance type exactly as submitted, including a syntactically valid but non-existent type. Image Builder stores the instance-type list declaratively and resolves it only when an image build launches an instance; it does not validate membership at update time.
Emulator: the Update returns 200 like AWS, but the subsequent GetInfrastructureConfiguration returns the pre-update value - $.infrastructureConfiguration.instanceTypes[0] still carries the originally created type instead of the one the update submitted ("value_mismatch (expected 't3.nonesuch', got 't3.medium')"). The submitted value is silently discarded; the request-level verdict is a success the state never reflects.
Why: a success-plus-stale-read is the worst possible combination for a client. The caller gets 200, so it records the update as applied, then reads back the old instance type and either loops trying to re-apply, reports spurious configuration drift, or launches builds against instance types it believes it replaced. A refusal would at least be actionable; a silent no-op is not.
Fix: the UpdateInfrastructureConfiguration handler and the infrastructure-configuration state store in the imagebuilder service. Write the submitted instanceTypes into stored state unconditionally rather than filtering or rejecting values that are absent from local EC2 state and keeping the previous list.
Traces: imagebuilder-infrastructure-configuration-update-accepts-an-unknown-instance-type-subnet-and-group#task#12.

Cause 3 of 4 - the same accepted update does not persist subnetId (same fixture, step task#14).
AWS: Smithy: UpdateInfrastructureConfiguration.subnetId targets NonEmptyString with @length min 1 max 1024 and no pattern or existence constraint; no modeled error expresses "subnet not found" at update time. Recorded behavior: Update is accepted with 200 and GetInfrastructureConfiguration echoes the submitted subnet id verbatim, even one that resolves to nothing in the account.
Emulator: Update answers 200, then GetInfrastructureConfiguration returns the subnet the configuration was created with rather than the submitted one - $.infrastructureConfiguration.subnetId "value_mismatch (expected 'subnet-0123456789abcdef0', got 'subnet-0000000000000003a')". The second value is an emulator-generated subnet id from its own state, which shows the stored field was never overwritten.
Why: networking placement for build instances silently reverts to whatever the configuration was created with. A client that moves a build pipeline onto a different subnet gets a 200, then every read and every build keeps using the old subnet, with no error anywhere to explain it.
Fix: the UpdateInfrastructureConfiguration handler / infrastructure-configuration state in the imagebuilder service - persist the submitted subnetId instead of retaining the created value when the id is unknown locally.
Traces: imagebuilder-infrastructure-configuration-update-accepts-an-unknown-instance-type-subnet-and-group#task#14.

Cause 4 of 4 - the same accepted update does not persist securityGroupIds (same fixture, step task#16).
AWS: Smithy: UpdateInfrastructureConfiguration.securityGroupIds targets SecurityGroupIds (a list) with no existence constraint, and the operation models no "security group not found" error. Recorded behavior: Update is accepted with 200 and GetInfrastructureConfiguration returns the submitted group ids verbatim, including ids that do not exist in the account.
Emulator: Update answers 200, then GetInfrastructureConfiguration returns the created group id instead of the submitted one - $.infrastructureConfiguration.securityGroupIds[0] "value_mismatch (expected 'sg-0123456789abcdef0', got 'sg-00000000000000043')", again an emulator-generated id, so the submitted list was dropped.
Why: the security posture a client configures for build and test instances silently does not take effect, while the API reports success - a client believes it has tightened or changed build-instance network access and read-back appears to confirm the old value, which it will read as drift rather than as its own update being discarded.
Fix: the UpdateInfrastructureConfiguration handler / infrastructure-configuration state in the imagebuilder service - persist the submitted securityGroupIds list as given.
Traces: imagebuilder-infrastructure-configuration-update-accepts-an-unknown-instance-type-subnet-and-group#task#16.

Causes 2-4 share a mechanism (update input validated against local EC2-like state and discarded when it does not resolve) but are three independently asserted members on three separate steps; cause 1 is in the Get response projection. All four are listed in trace_ids / Issue-Trace so a fixer can re-replay every one.

Note on values: the ARNs, generated subnet/security-group/instance ids and timestamps quoted above are the recorded run's fillers, not requirements. The requirements are: members AWS leaves unset are ABSENT from the Get body, and the value the Get body reports for instanceTypes, subnetId and securityGroupIds is the value the last accepted Update submitted.

## Evidence
Replay of trace set simparity-wire-aws-v1 against the emulator (emulator_up=true, score 61.5%). Four net-new mismatch clusters, all service=imagebuilder, operation=GetInfrastructureConfiguration, expected_status 200 / actual_status 200 - the divergences are entirely in the response body:

1) fixture imagebuilder-infrastructure-configuration-round-trips-profile-subnet-and-security-group, step task#8:
   $.infrastructureConfiguration.dateUpdated: extra (expected None, got '2026-11-08T12:54:55.238591Z')
   $.infrastructureConfiguration.terminateInstanceOnCompletion: extra (expected None, got True)
   Expected: both members absent (AWS omits them for a configuration never updated and never supplied the flag). Actual: both present.

2) fixture imagebuilder-infrastructure-configuration-update-accepts-an-unknown-instance-type-subnet-and-group, step task#12:
   $.infrastructureConfiguration.instanceTypes[0]: value_mismatch (expected 't3.nonesuch', got 't3.medium')
   Expected: the instance type the preceding UpdateInfrastructureConfiguration submitted. Actual: the value from create time.

3) same fixture, step task#14:
   $.infrastructureConfiguration.subnetId: value_mismatch (expected 'subnet-0123456789abcdef0', got 'subnet-0000000000000003a')
   Expected: the submitted subnet id. Actual: an emulator-generated id held from create time.

4) same fixture, step task#16:
   $.infrastructureConfiguration.securityGroupIds[0]: value_mismatch (expected 'sg-0123456789abcdef0', got 'sg-00000000000000043')
   Expected: the submitted security group id. Actual: an emulator-generated id held from create time.

In every one of causes 2-4 the UpdateInfrastructureConfiguration step itself replayed CLEAN (HTTP 200, matching AWS) - it is only the read-back that diverges, which localizes the defect to persistence rather than to request rejection.

Model evidence: Smithy com.amazonaws.imagebuilder - InfrastructureConfiguration.dateUpdated: DateTime, not required, no default; InfrastructureConfiguration.terminateInstanceOnCompletion: NullableBoolean, not required, no default; UpdateInfrastructureConfiguration input members instanceTypes (InstanceTypeList), subnetId (NonEmptyString @length 1..1024), securityGroupIds (SecurityGroupIds) carry no existence constraint, and the operation's modeled error set (CallRateLimitExceededException, ClientException, ForbiddenException, IdempotentParameterMismatchException, InvalidRequestException, ResourceInUseException, ServiceException, ServiceUnavailableException) contains no 'resource not found' variant for these members.

## Steps to Reproduce
Cause 1:
1. PUT /CreateInfrastructureConfiguration with a name, an existing instanceProfileName and a clientToken, and do NOT supply terminateInstanceOnCompletion.
2. GET /GetInfrastructureConfiguration?infrastructureConfigurationArn=<arn> immediately, performing no update.
3. Observe the 200 body: the emulator includes $.infrastructureConfiguration.dateUpdated and $.infrastructureConfiguration.terminateInstanceOnCompletion. AWS omits both members entirely.
   Replay: fixture imagebuilder-infrastructure-configuration-round-trips-profile-subnet-and-security-group, step task#8.

Causes 2-4:
1. PUT /CreateInfrastructureConfiguration with a real instance type, subnet and security group; note the returned infrastructureConfigurationArn.
2. PUT /UpdateInfrastructureConfiguration for that ARN with the required instanceProfileName and clientToken plus instanceTypes, subnetId and securityGroupIds set to syntactically valid identifiers that do not exist in the account (e.g. an unknown instance type, subnet-0123456789abcdef0, sg-0123456789abcdef0). It answers 200, as AWS does.
3. GET /GetInfrastructureConfiguration?infrastructureConfigurationArn=<arn>.
4. Observe that instanceTypes[0] (cause 2), subnetId (cause 3) and securityGroupIds[0] (cause 4) still hold the values from step 1; AWS returns the values submitted in step 2.
   Replay: fixture imagebuilder-infrastructure-configuration-update-accepts-an-unknown-instance-type-subnet-and-group, steps task#12, task#14 and task#16.
