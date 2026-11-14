Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: iot.fleet-metric-invalid-aggregation-rejected iot.negative-fleet-metric-index-not-ready-blocked iot.telemetry-fleet-metric-invalid-aggregation-rejected iot.negative-domain-configuration-invalid-certificate-blocked iot.negative-dynamic-thing-group-invalid-query-blocked iot.op-add-thing-to-missing-group-rejected iot.update-package-unknown-default-version-rejected iot.concurrent-policy-version-creation-respects-limit iot.concurrent-thing-group-update-optimistic-lock iot.certificate-provider-singleton-lifecycle iot.provisioning-template-version-default-delete-guard iot.stream-lifecycle-delete-allowed-while-referenced
Issue-Shape-Id: com.amazonaws.iot#AWSIotService
Issue-Key: iot.fleet-metric-invalid-aggregation-rejected|run#5|AssertionError|assert code == "InvalidRequestException", f"unexpected code: {code!r}"
Issue-Key: iot.negative-fleet-metric-index-not-ready-blocked|run#9|AssertionError|assert create_code == "ResourceNotFoundException", create_code
Issue-Key: iot.telemetry-fleet-metric-invalid-aggregation-rejected|run#11|AssertionError|assert code == "InvalidRequestException", f"expected InvalidRequestException, got {code!r}"
Issue-Key: iot.negative-domain-configuration-invalid-certificate-blocked|run#9|AssertionError|assert code == "InvalidRequestException", code
Issue-Key: iot.negative-dynamic-thing-group-invalid-query-blocked|run#9|AssertionError|assert code == "InvalidQueryException", f"expected InvalidQueryException, got {code}"
Issue-Key: iot.op-add-thing-to-missing-group-rejected|run#7|AssertionError|assert code == "ResourceNotFoundException", f"expected ResourceNotFoundException, got {code}"
Issue-Key: iot.update-package-unknown-default-version-rejected|run#20|AssertionError|assert code == "ResourceNotFoundException", f"expected ResourceNotFoundException, got {code}"
Issue-Key: iot.concurrent-policy-version-creation-respects-limit|run#10|AssertionError|assert 1 <= successes <= 4, f"expected 1..4 successful version creations, got {successes}"
Issue-Key: iot.concurrent-thing-group-update-optimistic-lock|run#12|AssertionError|assert successes == 1, f"expected exactly one winner of the optimistic-lock race, got {successes}"
Issue-Key: iot.certificate-provider-singleton-lifecycle|_poll_until#23|Timeout|time.sleep(interval_s)
Issue-Key: iot.provisioning-template-version-default-delete-guard|_poll_until#11|Timeout|time.sleep(interval_s)
Issue-Key: iot.stream-lifecycle-delete-allowed-while-referenced|_poll_until#23|Timeout|time.sleep(interval_s)
Issue-Commit: 9ad1a007c88406c445bb3f2d55ab006d23610bba

# iot: 10 further behavioral checks are blocked by scaffold stubs that refuse their setup or the operation under test (CreateFleetMetric, CreateDomainConfiguration, CreateDynamicThingGroup, AddThingToThingGroup, UpdatePackage, CreatePolicyVersion, UpdateThingGroup, CreateCertificateProvider, CreateProvisioningTemplate, CreateStream)

> _Note: this issue was identified against the code base at commit `9ad1a007c88406c445bb3f2d55ab006d23610bba`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateFleetMetric

## Description
Ten net-new causes, each a distinct iot operation that answers `NotImplemented` ("operation <Op> is scaffolded but not implemented in this emulator") and thereby blocks the listed behavioral checks. These checks were NOT marked ALREADY FILED/OWNED in the replay report, so the stub issues filed for other checks do not carry their replay keys; this issue records them. Each cause is a separate operation to implement.

Cause 1 of 10: CreateFleetMetric is a stub. AWS: Smithy CreateFleetMetric models InvalidRequestException (400) and IndexNotReadyException; an unsupported aggregation type/field is refused InvalidRequestException and nothing is created, and while the AWS_Things index is not ACTIVE the call is refused IndexNotReadyException. Emulator: code NotImplemented for every call. Why: fleet metrics cannot be created, so neither the invalid-aggregation nor the index-not-ready refusal is observable. Fix: services/service-com-amazonaws-iot-awsiotservice/src/ops/create_fleet_metric.rs (+ describe/delete siblings). Traces: iot.fleet-metric-invalid-aggregation-rejected, iot.negative-fleet-metric-index-not-ready-blocked, iot.telemetry-fleet-metric-invalid-aggregation-rejected.

Cause 2 of 10: CreateDomainConfiguration is a stub. AWS: a serverCertificateArn that does not exist is refused InvalidRequestException with the fixed template `Certificate <certificateArn> does not exist.` and no domain configuration is created (DescribeDomainConfiguration then refuses ResourceNotFoundException). Emulator: code NotImplemented. Why: the certificate-existence precondition is unobservable. Fix: create_domain_configuration.rs. Traces: iot.negative-domain-configuration-invalid-certificate-blocked.

Cause 3 of 10: CreateDynamicThingGroup is a stub. AWS: an unparsable queryString is refused InvalidQueryException and no group is created. Emulator: code NotImplemented (AssertionError: expected InvalidQueryException, got NotImplemented). Why: dynamic thing groups and their query validation are unavailable. Fix: create_dynamic_thing_group.rs. Traces: iot.negative-dynamic-thing-group-invalid-query-blocked.

Cause 4 of 10: AddThingToThingGroup is a stub. AWS: adding a thing to a thing group that does not exist is refused ResourceNotFoundException. Emulator: code NotImplemented (AssertionError: expected ResourceNotFoundException, got NotImplemented). Why: group membership cannot be established at all, so membership-dependent behavior (including group-targeted jobs) is unreachable. Fix: add_thing_to_thing_group.rs. Traces: iot.op-add-thing-to-missing-group-rejected.

Cause 5 of 10: UpdatePackage is a stub. AWS: UpdatePackage(defaultVersionName=<unknown>) is refused ResourceNotFoundException with the fixed template `PackageVersion <versionName> for package <packageName> not found`, and the package's defaultVersionName stays unset; a valid Published version is accepted and reported by GetPackage. Emulator: code NotImplemented. Why: a package's default version can never be set or validated. Fix: update_package.rs. Traces: iot.update-package-unknown-default-version-rejected.

Cause 6 of 10: CreatePolicyVersion is a stub. AWS: a policy holds at most 5 versions; concurrent CreatePolicyVersion calls resolve to successes plus VersionsLimitExceededException/InvalidRequestException losers, and ListPolicyVersions never exceeds the cap. Emulator: code NotImplemented, so zero version creations succeed (AssertionError: expected 1..4 successful version creations, got 0). Why: policy versioning — and the 5-version cap it is graded on — does not exist. Fix: create_policy_version.rs. Traces: iot.concurrent-policy-version-creation-respects-limit.

Cause 7 of 10: UpdateThingGroup is a stub. AWS: UpdateThingGroup takes expectedVersion and resolves a concurrent race to exactly one winner, the losers refused VersionConflictException, with DescribeThingGroup's version advanced by exactly 1. Emulator: code NotImplemented, so no racer wins (AssertionError: expected exactly one winner of the optimistic-lock race, got 0). Why: thing-group properties can never be updated and the optimistic-lock contract is unobservable. Fix: update_thing_group.rs. Traces: iot.concurrent-thing-group-update-optimistic-lock.

Cause 8 of 10: CreateCertificateProvider is a stub. AWS: only one certificate provider may hold the CreateCertificateFromCsr account default; a second claim is refused InvalidRequestException `Cannot set provider [<name>] as default for operation CreateCertificateFromCsr because another provider was set to be default for the operation`, and the slot is reclaimable after DeleteCertificateProvider. Emulator: code NotImplemented; the check's create-retry poll therefore never succeeds and the check times out after 60s. Why: certificate providers cannot be created, so the default-exclusivity rule is unobservable. Fix: create_certificate_provider.rs. Traces: iot.certificate-provider-singleton-lifecycle.

Cause 9 of 10: CreateProvisioningTemplate is a stub. AWS: a provisioning template holds versions whose default version cannot be deleted (DeleteProvisioningTemplateVersion refuses) until another version is made default. Emulator: code NotImplemented; the check's setup never completes and it times out after 60s. Why: fleet provisioning templates and their version/default guards are unavailable. Fix: create_provisioning_template.rs. Traces: iot.provisioning-template-version-default-delete-guard.

Cause 10 of 10: CreateStream is a stub. AWS: DeleteStream succeeds even while an OTA update references the stream (the OTA update keeps the now-dangling streamId), and a second delete refuses ResourceNotFoundException `Requested stream <streamId> does not exist in given account <account>`. Emulator: code NotImplemented; the check's create-retry poll never succeeds and it times out after 60s. Why: streams cannot be created, so the OTA/stream reference semantics are unobservable. Fix: create_stream.rs. Traces: iot.stream-lifecycle-delete-allowed-while-referenced.

## Evidence
All ten causes present the same wire shape — the operation answers error code `NotImplemented` with message "operation <Op> is scaffolded but not implemented in this emulator" where AWS answers either a 200 or the operation's modelled client error:
- CreateFleetMetric: expected InvalidRequestException / IndexNotReadyException; actual 'NotImplemented' (AssertionError: unexpected code: 'NotImplemented').
- CreateDomainConfiguration: expected InvalidRequestException 'Certificate <arn> does not exist.'; actual NotImplemented.
- CreateDynamicThingGroup: expected InvalidQueryException; actual NotImplemented.
- AddThingToThingGroup: expected ResourceNotFoundException; actual NotImplemented.
- UpdatePackage: expected ResourceNotFoundException 'PackageVersion <ver> for package <pkg> not found'; actual NotImplemented.
- CreatePolicyVersion: expected 1..4 successful version creations under contention; actual 0.
- UpdateThingGroup: expected exactly one optimistic-lock winner; actual 0.
- CreateCertificateProvider / CreateProvisioningTemplate / CreateStream: the setup call never succeeds, so the check ends 'Check timed out after 60.0s'.

## Steps to Reproduce
Replay the listed checks (suite simparity-behavioral-aws-v1) and observe the NotImplemented refusal (or the resulting setup timeout) for each operation:
CreateFleetMetric — iot.fleet-metric-invalid-aggregation-rejected, iot.negative-fleet-metric-index-not-ready-blocked, iot.telemetry-fleet-metric-invalid-aggregation-rejected; CreateDomainConfiguration — iot.negative-domain-configuration-invalid-certificate-blocked; CreateDynamicThingGroup — iot.negative-dynamic-thing-group-invalid-query-blocked; AddThingToThingGroup — iot.op-add-thing-to-missing-group-rejected; UpdatePackage — iot.update-package-unknown-default-version-rejected; CreatePolicyVersion — iot.concurrent-policy-version-creation-respects-limit; UpdateThingGroup — iot.concurrent-thing-group-update-optimistic-lock; CreateCertificateProvider — iot.certificate-provider-singleton-lifecycle; CreateProvisioningTemplate — iot.provisioning-template-version-default-delete-guard; CreateStream — iot.stream-lifecycle-delete-allowed-while-referenced.
