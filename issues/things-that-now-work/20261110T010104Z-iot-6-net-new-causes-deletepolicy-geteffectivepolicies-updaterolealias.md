Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: iot.iot-attached-policy-delete-blocked-until-detached-rejected iot.iot-get-effective-policies-rejected-with-conflicting-identity iot.role-alias-lifecycle iot.certificate-provider-singleton-lifecycle iot.provisioning-template-version-default-delete-guard iot.stream-lifecycle-delete-allowed-while-referenced
Issue-Shape-Id: com.amazonaws.iot#AWSIotService
Issue-Key: iot.iot-attached-policy-delete-blocked-until-detached-rejected|run#17|AssertionError|assert code == "DeleteConflictException", (
Issue-Key: iot.iot-get-effective-policies-rejected-with-conflicting-identity|run#12|AssertionError|assert code == "InvalidRequestException", (
Issue-Key: iot.role-alias-lifecycle|run#17|AssertionError|assert update_code == "ResourceNotFoundException", (
Issue-Key: iot.certificate-provider-singleton-lifecycle|_poll_until#23|AssertionError|raise AssertionError(f"condition not met within {timeout_s}s (last={last!r})")
Issue-Key: iot.provisioning-template-version-default-delete-guard|run#12|AssertionError|assert created is not None, "CreateProvisioningTemplate never succeeded (role propagation timeout)"
Issue-Key: iot.stream-lifecycle-delete-allowed-while-referenced|run#24|AssertionError|assert result is not None, "CreateStream never succeeded (role propagation timeout)"
Issue-Commit: 255f0ed37c2f4cb6b491d33a1606f5f049f58c1d

# iot: 6 net-new causes — DeletePolicy/GetEffectivePolicies/UpdateRoleAlias answer 501 NotImplemented instead of their modeled refusals, and CreateCertificateProvider/CreateProvisioningTemplate/CreateStream never create, so three lifecycle guards are unreachable

> _Note: this issue was identified against the code base at commit `255f0ed37c2f4cb6b491d33a1606f5f049f58c1d`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DeletePolicy

## Description
Six behavioral checks under `### iot` survive the report's ALREADY-FILED / OWNED markers and are filed here. Each is a distinct operation whose handler is still a scaffold that answers HTTP 501 with error code `NotImplemented` and the body message `operation <Name> is scaffolded but not implemented in this emulator`. Three of them surface as the wrong error CODE on the operation under test; three surface as a setup call that never succeeds, so the guard under test is never reached at all. Open stub issues name some of these operations (20261106T201842Z-iot-deletepolicy-stub.md, 20261108T182336Z-iot-geteffectivepolicies-stub.md, 20261108T182336Z-iot-updaterolealias-stub.md, 20261106T201842Z-iot-createcertificateprovider-stub.md, 20261106T201842Z-iot-createprovisioningtemplate-stub.md, 20261106T201842Z-iot-createstream-stub.md) but none of them carries any of the six replay keys below, so those checks are not covered by them and are recorded here. SIX causes must be closed before submitting.

Cause 1 of 6: DeletePolicy does not refuse a policy that still has attached principals.
AWS: Smithy — `DeletePolicy` (DELETE /policies/{policyName}) models `DeleteConflictException` @httpError 409, `ResourceNotFoundException` @httpError 404, `InvalidRequestException` @httpError 400. AWS refuses the delete with 409 `DeleteConflictException` for as long as ANY principal remains attached, and the refusal is idempotent: detaching one of several attached targets does not unblock it. Recorded behavior shows the message is fixed text with one variable part taken from the request: `The policy cannot be deleted as the policy is attached to one or more principals (name=<policyName>)` — the fixed text is everything outside `<policyName>`, which is the request's `policyName`. After the last target is detached the delete succeeds and a subsequent `GetPolicy` answers 404 `ResourceNotFoundException` with the fixed message `Policy default version not found` (no variable part).
Emulator: `DeletePolicy` answers 501 with error code `NotImplemented`; the replay asserts on the code and reports `deleting '<policyName>' with two certificates attached raised 'NotImplemented', expected DeleteConflictException`.
Why: a client that relies on the delete-conflict guard to detect "still attached" sees a server-fault-class 501 instead of a 409 client error. Retrying SDKs treat 501 as non-retryable fatal, teardown code that catches `DeleteConflictException` to drive a detach-then-retry loop never runs, and the policy is never removed.
Fix: the DeletePolicy handler and its delete-gating against the policy-target state.
Traces: iot.iot-attached-policy-delete-blocked-until-detached-rejected.

Cause 2 of 6: GetEffectivePolicies does not refuse mutually exclusive identities.
AWS: Smithy — `GetEffectivePolicies` (POST /effective-policies) takes `principal` and `cognitoIdentityPoolId` as separate optional members and models `InvalidRequestException` @httpError 400. AWS rejects a request that supplies BOTH with 400 `InvalidRequestException`; recorded behavior shows the message is entirely fixed text with no variable part: `Principal must be null if CognitoIdentityPoolId is provided.` With `principal` alone the call succeeds and `effectivePolicies` lists exactly the policies attached to that principal, each with its `policyName` and the attached `policyDocument`.
Emulator: `GetEffectivePolicies` answers 501 `NotImplemented`; the replay reports `GetEffectivePolicies with both principal='arn:aws:iot:<region>:<account>:cert/<certId>' and cognitoIdentityPoolId='<region>:<uuid>' raised 'NotImplemented', expected InvalidRequestException`.
Why: the mutual-exclusion contract is invisible. A client that passes both by mistake gets a 501 server fault rather than the 400 that tells it which member to drop, and the positive path (resolving a certificate principal's effective policies) is unavailable, so no authorization-introspection flow works.
Fix: the GetEffectivePolicies handler, its input validation (principal/cognitoIdentityPoolId exclusivity) and its resolution against the policy-attachment state.
Traces: iot.iot-get-effective-policies-rejected-with-conflicting-identity.

Cause 3 of 6: UpdateRoleAlias does not answer not-found for a deleted alias.
AWS: Smithy — `UpdateRoleAlias` (PUT /role-aliases/{roleAlias}) models `ResourceNotFoundException` @httpError 404 and `InvalidRequestException` @httpError 400, with `credentialDurationSeconds` @range min 900 max 43200. After `DeleteRoleAlias`, both the read (`DescribeRoleAlias`) and the mutation (`UpdateRoleAlias`) on the dead alias answer 404 `ResourceNotFoundException`. Recorded behavior shows one message template for both, fixed text with one variable part from the request: `Role alias <roleAlias> not found`.
Emulator: `UpdateRoleAlias` answers 501 `NotImplemented`; the replay reports `expected UpdateRoleAlias on a deleted alias to report ResourceNotFoundException, got 'NotImplemented'`.
Why: a mutation on a deleted id is indistinguishable from a transient server fault, so a client cannot tell "this alias is gone, stop" from "retry later". Nothing can ever update a role alias, and delete is unverifiable on the write path.
Fix: the UpdateRoleAlias handler and the role-alias state lookup it must perform before applying the update.
Traces: iot.role-alias-lifecycle.

Cause 4 of 6: CreateCertificateProvider never creates, so the account default-for-operation exclusivity guard is unreachable.
AWS: Smithy — `CreateCertificateProvider` (POST /certificate-providers/{certificateProviderName}) requires `lambdaFunctionArn` and `accountDefaultForOperations` (@length min 1 max 1) and returns `certificateProviderName` + `certificateProviderArn`; it models `InvalidRequestException` @httpError 400 and `ResourceAlreadyExistsException` @httpError 409. AWS creates the provider and echoes the requested name; `DescribeCertificateProvider` then reports the same `lambdaFunctionArn` and `accountDefaultForOperations`. Only the per-operation DEFAULT is exclusive: a SECOND provider claiming the same default is refused with 400 `InvalidRequestException` (not `LimitExceededException`), message fixed text with one variable part from the request and one from the requested operation: `Cannot set provider [<certificateProviderName>] as default for operation CreateCertificateFromCsr because another provider was set to be default for the operation`. The refused create leaves nothing behind — `DescribeCertificateProvider` on that name answers `ResourceNotFoundException` with `Certificate provider with name [<certificateProviderName>] not found` — and deleting the first provider frees the slot so a fresh provider claims it.
Emulator: `CreateCertificateProvider` answers 501 `NotImplemented` on every attempt. The check retries the create for 60 s past Lambda-permission propagation and reports `AssertionError: condition not met within 60.0s (last=None)`; no provider is ever created, so neither the exclusivity refusal nor the slot-reclaim is observable.
Why: the whole certificate-provider surface is unavailable and the account-singleton semantics around `CreateCertificateFromCsr` cannot be exercised. A client provisioning certificates through a provider Lambda cannot register one at all, and a 501 gives it no signal that the resource type is simply absent.
Fix: the CreateCertificateProvider handler (services/service-com-amazonaws-iot-awsiotservice/src/ops/create_certificate_provider.rs) plus the certificate-provider state that must hold the per-operation account default and the Describe/Delete reads over it.
Traces: iot.certificate-provider-singleton-lifecycle.

Cause 5 of 6: CreateProvisioningTemplate never creates, so the default-template-version delete guard is unreachable.
AWS: Smithy — `CreateProvisioningTemplate` (POST /provisioning-templates) requires `templateName`, `templateBody` and `provisioningRoleArn` and returns `templateArn`, `templateName`, `defaultVersionId`; it models `InvalidRequestException` @httpError 400 and `ResourceAlreadyExistsException` @httpError 409. With the template created, `DeleteProvisioningTemplateVersion` (which models `InvalidRequestException` @httpError 400 and `DeleteConflictException` @httpError 409) refuses the CURRENT default version with 400 `InvalidRequestException` — recorded behavior, fixed text with the request's `versionId` as the only variable part: `Unable to delete template version <versionId>, it is the default version`. The refused delete leaves that version present and still default. Promoting another version via `UpdateProvisioningTemplate(defaultVersionId=...)` then lets the old version delete cleanly, after which `DescribeProvisioningTemplateVersion` answers `ResourceNotFoundException` with `Unable to find a template version with ID <versionId> for template <templateName>`, and deleting the template makes `DescribeProvisioningTemplate` answer `ResourceNotFoundException` with `Unable to find template named <templateName>`.
Emulator: `CreateProvisioningTemplate` answers 501 `NotImplemented` on every attempt; the check's bounded retry gives up with `AssertionError: CreateProvisioningTemplate never succeeded (role propagation timeout)`.
Why: no fleet-provisioning template can be created, so no version can be added, promoted or guarded. A client sees a 501 server fault where AWS either succeeds or returns a 400/409 client error, and the default-version protection — which is what stops a fleet from losing its active provisioning body — is entirely absent.
Fix: the CreateProvisioningTemplate handler (services/service-com-amazonaws-iot-awsiotservice/src/ops/create_provisioning_template.rs) and the provisioning-template/version state that the version create, default promotion and delete guards read.
Traces: iot.provisioning-template-version-default-delete-guard.

Cause 6 of 6: CreateStream never creates, so DeleteStream-while-referenced and the stream not-found refusal are unreachable.
AWS: Smithy — `CreateStream` (POST /streams/{streamId}) requires `files` (@length min 1 max 50) and `roleArn` and returns `streamId`, `streamArn`, `description`, `streamVersion`; it models `InvalidRequestException` @httpError 400, `ResourceAlreadyExistsException` @httpError 409 and `ResourceNotFoundException` @httpError 404. AWS does NOT treat an OTA update's reference to a stream as a delete conflict: `DeleteStream` succeeds on a stream a CREATE_COMPLETE OTA update still points at, the OTA update keeps the now-dangling `streamId` in its `otaUpdateFiles[].fileLocation.stream.streamId`, and both `DescribeStream` and a second `DeleteStream` then answer `ResourceNotFoundException` — recorded behavior, fixed text with two variable parts, the request's `streamId` and the caller's account: `Requested stream <streamId> does not exist in given account <accountId>`.
Emulator: `CreateStream` answers 501 `NotImplemented` on every attempt; the check's bounded retry gives up with `AssertionError: CreateStream never succeeded (role propagation timeout)`.
Why: the IoT stream surface is unavailable, so OTA firmware distribution cannot be modelled at all. A client gets a 501 server fault instead of a created stream or a 400/409 client error, and the deliberately permissive delete semantics (delete allowed while referenced, dangling reference preserved) cannot be relied on. Note 20261109T163411Z-cloudformation-iam-iot-s3-iot-createstream-answers-notimplemented-so-aws-iot-stream.md records the same operation refusing a CloudFormation path; it does not carry this check's replay key.
Fix: the CreateStream handler (services/service-com-amazonaws-iot-awsiotservice/src/ops/create_stream.rs) plus the stream state that DescribeStream/DeleteStream read and that the OTA-update file reference must be allowed to dangle against.
Traces: iot.stream-lifecycle-delete-allowed-while-referenced.

## Evidence
All six are grounded in failing checks from the behavioral suite report (Total: 17/301 passed), each listed under the `### iot` heading and marked neither ALREADY FILED nor OWNED.

Cause 1 — iot.iot-attached-policy-delete-blocked-until-detached-rejected [mismatch]
  Expected: DeletePolicy on a policy with two attached certificates -> HTTP 409, code `DeleteConflictException`, message `The policy cannot be deleted as the policy is attached to one or more principals (name=<policyName>)`; after both detaches the delete succeeds and GetPolicy -> 404 `ResourceNotFoundException` / `Policy default version not found`.
  Actual: `AssertionError: deleting 'sim2real-scn-iotpol-<id>' with two certificates attached raised 'NotImplemented', expected DeleteConflictException` (HTTP 501, body `operation DeletePolicy is scaffolded but not implemented in this emulator`).

Cause 2 — iot.iot-get-effective-policies-rejected-with-conflicting-identity [mismatch]
  Expected: GetEffectivePolicies(principal=<certArn>, cognitoIdentityPoolId=<poolId>) -> HTTP 400, code `InvalidRequestException`, message `Principal must be null if CognitoIdentityPoolId is provided.`; GetEffectivePolicies(principal=<certArn>) alone -> one effectivePolicies entry whose policyName and policyDocument are the attached policy's.
  Actual: `AssertionError: GetEffectivePolicies with both principal='arn:aws:iot:<region>:<account>:cert/<certId>' and cognitoIdentityPoolId='<region>:<uuid>' raised 'NotImplemented', expected InvalidRequestException — the two identities are mutually exclusive`.

Cause 3 — iot.role-alias-lifecycle [mismatch]
  Expected: after DeleteRoleAlias, UpdateRoleAlias(roleAlias=<alias>, credentialDurationSeconds=1800) -> HTTP 404, code `ResourceNotFoundException`, message `Role alias <alias> not found`.
  Actual: `AssertionError: expected UpdateRoleAlias on a deleted alias to report ResourceNotFoundException, got 'NotImplemented'`.

Cause 4 — iot.certificate-provider-singleton-lifecycle [mismatch]
  Expected: CreateCertificateProvider(certificateProviderName, lambdaFunctionArn, accountDefaultForOperations=["CreateCertificateFromCsr"]) -> 200 echoing the name; a second provider claiming the same default -> HTTP 400 `InvalidRequestException` / `Cannot set provider [<name2>] as default for operation CreateCertificateFromCsr because another provider was set to be default for the operation`; DescribeCertificateProvider(<name2>) -> `ResourceNotFoundException` / `Certificate provider with name [<name2>] not found`.
  Actual: every CreateCertificateProvider attempt over 60 s is refused (501 `NotImplemented`), so the retry loop ends `AssertionError: condition not met within 60.0s (last=None)` at check line 116 and nothing downstream is exercised.

Cause 5 — iot.provisioning-template-version-default-delete-guard [mismatch]
  Expected: CreateProvisioningTemplate(templateName, templateBody, provisioningRoleArn) -> 200 with `defaultVersionId`; DeleteProvisioningTemplateVersion on that default -> HTTP 400 `InvalidRequestException` / `Unable to delete template version <versionId>, it is the default version`, with v1 still `isDefaultVersion: true`.
  Actual: `AssertionError: CreateProvisioningTemplate never succeeded (role propagation timeout)` at check line 111 — every create answered 501 `NotImplemented`.

Cause 6 — iot.stream-lifecycle-delete-allowed-while-referenced [mismatch]
  Expected: CreateStream(streamId, files=[{fileId:0,s3Location:{bucket,key}}], roleArn) -> 200; DeleteStream succeeds while a CREATE_COMPLETE OTA update references it; DescribeStream and a re-DeleteStream -> `ResourceNotFoundException` / `Requested stream <streamId> does not exist in given account <accountId>`; the OTA update still reports the dangling streamId.
  Actual: `AssertionError: CreateStream never succeeded (role propagation timeout)` at check line 118 — every create answered 501 `NotImplemented`.

Common emulator signature for all six: HTTP 501, `x-amzn-errortype`/Error.Code `NotImplemented`, message `operation <OperationName> is scaffolded but not implemented in this emulator`. AWS never answers 501 for any of these operations; the modeled outcomes are 200 or a 4xx client error as quoted above.

## Steps to Reproduce
1. git clone the repo and build/boot the emulator (the behavioral suite does this).
2. Replay the iot behavioral suite and re-run exactly these six checks:
   - iot.iot-attached-policy-delete-blocked-until-detached-rejected
   - iot.iot-get-effective-policies-rejected-with-conflicting-identity
   - iot.role-alias-lifecycle
   - iot.certificate-provider-singleton-lifecycle
   - iot.provisioning-template-version-default-delete-guard
   - iot.stream-lifecycle-delete-allowed-while-referenced

Minimal per-cause repro against the running emulator (botocore `iot` client, any region):
- Cause 1: create_policy(policyName=P, policyDocument=<any valid doc>); create_keys_and_certificate(setAsActive=True) twice; attach_policy(policyName=P, target=<each certArn>); then delete_policy(policyName=P). Expect 409 DeleteConflictException naming P; observe 501 NotImplemented. Detach one target and repeat — AWS still answers 409. Detach the last, delete_policy succeeds, get_policy(P) -> 404 ResourceNotFoundException / 'Policy default version not found'.
- Cause 2: create_policy + create_keys_and_certificate + attach_policy as above, then get_effective_policies(principal=<certArn>, cognitoIdentityPoolId='<region>:00000000-0000-0000-0000-000000000000'). Expect 400 InvalidRequestException / 'Principal must be null if CognitoIdentityPoolId is provided.'; observe 501 NotImplemented. Then get_effective_policies(principal=<certArn>) alone and expect the one attached policy.
- Cause 3: create_role_alias(roleAlias=A, roleArn=<role trusted by credentials.iot.amazonaws.com>, credentialDurationSeconds=3600); describe_role_alias(A); delete_role_alias(A); then update_role_alias(roleAlias=A, credentialDurationSeconds=1800). Expect 404 ResourceNotFoundException / 'Role alias A not found'; observe 501 NotImplemented.
- Cause 4: create a Lambda function and add_permission for principal iot.amazonaws.com, then create_certificate_provider(certificateProviderName=N1, lambdaFunctionArn=<fnArn>, accountDefaultForOperations=['CreateCertificateFromCsr']). Expect 200; observe 501 NotImplemented. With it created, a second create_certificate_provider(N2, same default) must answer 400 InvalidRequestException with the quoted message, describe_certificate_provider(N2) must answer ResourceNotFoundException, and after delete_certificate_provider(N1) the N2 create must succeed and describe must report accountDefaultForOperations=['CreateCertificateFromCsr'].
- Cause 5: create_provisioning_template(templateName=T, templateBody=<fleet-provisioning body>, provisioningRoleArn=<role trusted by iot.amazonaws.com>). Expect 200 with defaultVersionId; observe 501 NotImplemented. Then create_provisioning_template_version(T, body2, setAsDefault=False) and delete_provisioning_template_version(T, versionId=<defaultVersionId>) -> 400 InvalidRequestException / 'Unable to delete template version <id>, it is the default version'; update_provisioning_template(T, defaultVersionId=<v2>) then the same delete succeeds.
- Cause 6: put an S3 object, create an IAM role trusted by iot.amazonaws.com, then create_stream(streamId=S, files=[{'fileId':0,'s3Location':{'bucket':B,'key':K}}], roleArn=<roleArn>). Expect 200 with streamArn/streamVersion; observe 501 NotImplemented. Then create_ota_update referencing {'stream':{'streamId':S,'fileId':0}}, wait for CREATE_COMPLETE, delete_stream(S) -> succeeds, describe_stream(S) and delete_stream(S) -> ResourceNotFoundException / 'Requested stream S does not exist in given account <accountId>', and get_ota_update still reports streamId S.
