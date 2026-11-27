Issue-Source: trace-sweep-divergence
Issue-Trace: backup-gateway-metadata-sync-hypervisor-arn-refusals
Issue-Step: task#6
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: backup-gateway-metadata-sync-hypervisor-arn-refusals
Issue-Shape-Id: com.amazonaws.backupgateway#BackupOnPremises_v20210101
Issue-Key: backup-gateway|StartVirtualMachinesMetadataSync|4xx->4xx|$.ErrorCode
Issue-Key: backup-gateway|StartVirtualMachinesMetadataSync|4xx->4xx|$.Message
Issue-Key: backup-gateway|StartVirtualMachinesMetadataSync|4xx->4xx|$.__type
Issue-Commit: d1c103039c63f2296b4493ce7c42775d3faa3fcc

# Out-of-region HypervisorArn answered AccessDeniedException/MappingsNotConfigured instead of ValidationException/InvalidResourceARN

> _Note: this issue was identified against the code base at commit `d1c103039c63f2296b4493ce7c42775d3faa3fcc`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
StartVirtualMachinesMetadataSync

## Description
Single cause (1 of 1): StartVirtualMachinesMetadataSync refuses a syntactically valid HypervisorArn that belongs to a region OTHER than the one the request was addressed to with the wrong modelled error shape, error code and message.

AWS: StartVirtualMachinesMetadataSync models both ValidationException (httpError 400) and AccessDeniedException (httpError 403) — Smithy: StartVirtualMachinesMetadataSync.errors = [AccessDeniedException(403), ResourceNotFoundException(404), InternalServerException(500), ThrottlingException(429), ValidationException(400)]; StartVirtualMachinesMetadataSync.HypervisorArn -> ServerArn, @required, @length min 50 max 500, @pattern ^arn:(aws|aws-cn|aws-us-gov):backup-gateway(:[a-zA-Z-0-9]+){3}\/[a-zA-Z-0-9]+$; ValidationException = {ErrorCode: "A description of what caused the validation error.", Message} and is documented "The operation did not succeed because a validation error occurred."; AccessDeniedException = {ErrorCode (required): "A description of why you have insufficient permissions.", Message} and is documented "The operation cannot proceed because you have insufficient permissions." The model does not enumerate the ErrorCode values, so for the region-scope rule this is recorded behavior only: the recording shows an ARN whose region segment is not the endpoint's region refused at HTTP 400 with wire __type com.amazonaws.backuponpremises#ValidationException, ErrorCode InvalidResourceARN and Message "The resource does not belong to the region.". That message is entirely FIXED text for this error class — it carries no request- or state-derived value; the variable part of the answer is only the ErrorCode/exception selection, driven by the ARN's region segment versus the endpoint's region. AWS treats a cross-region ARN as a malformed/invalid resource identifier (a validation fault about the ARN), not as a permissions fault and not as a property-mappings precondition.

Emulator: the same step is answered HTTP 400 with __type AccessDeniedException, ErrorCode MappingsNotConfigured and Message "HypervisorPropertyMappings is not configured for the Hypervisor". So all three of the exception type, the error code and the message diverge. The emulator folds the out-of-region test into its property-mappings precondition (one combined gate) and therefore reports an unrelated precondition failure. As a secondary inconsistency, it serves AccessDeniedException at status 400 although the model pins that shape to httpError 403.

Why: a client that switches on the modelled exception shape sees a permissions/authorization failure where AWS reports a request-validation failure. SDK error-type dispatch (catch ValidationException), retry and remediation logic diverge: the caller is told to fix IAM permissions or to configure HypervisorPropertyMappings, when the actual fault is that it addressed the wrong regional endpoint for that hypervisor ARN. The ErrorCode InvalidResourceARN, which is the only machine-readable signal distinguishing the two, is never emitted, and the diagnostic sentence sends the operator down the wrong path.

Fix: the operation's handler/validation for StartVirtualMachinesMetadataSync must separate the region-scope check from the property-mappings precondition, and check the ARN's region FIRST: when the HypervisorArn's region segment is not the request's region, return the modelled ValidationException with ErrorCode InvalidResourceARN and the fixed message "The resource does not belong to the region.", serialized with the wire __type the service uses (com.amazonaws.backuponpremises#ValidationException) at HTTP 400. The MappingsNotConfigured/AccessDeniedException answer must remain reserved for an in-region hypervisor whose property mappings are not configured, and AccessDeniedException should be serialized at its modelled 403. Note the existing OPEN issue 20261126T102247Z-backup-gateway-startvirtualmachinesmetadatasync-error-response--r2 ('The store's region-scope rule says every single-ARN operation refuses an out-of-region ARN ... while this handler answers AccessDeniedException/MappingsNotConfigured by design'): it does NOT cover this divergence — it argues the opposite direction, treating the combined gate as the pinned behavior and the crate's region-scope module doc as the stale side. The replayed fixture shows the combined gate is the defect, so that issue's conclusion (and the test that pins the combined answer) must be revisited rather than used as justification.

Traces: backup-gateway-metadata-sync-hypervisor-arn-refusals#task#6 and #task#7 (one cluster, 2 affected steps).

## Evidence
Fixture backup-gateway-metadata-sync-hypervisor-arn-refusals, step task#6 (cluster also covers task#7), operation StartVirtualMachinesMetadataSync, verdict mismatch.

Expected status 400, actual status 400, but the body diverges on three fields:
  $.__type:     expected 'com.amazonaws.backuponpremises#ValidationException', got 'AccessDeniedException'
  $.ErrorCode:  expected 'InvalidResourceARN', got 'MappingsNotConfigured'
  $.Message:    expected 'The resource does not belong to the region.', got 'HypervisorPropertyMappings is not configured for the Hypervisor'

Message anatomy: the expected message 'The resource does not belong to the region.' is wholly fixed text for the InvalidResourceARN class (no request or state values interpolated). The emulator's 'HypervisorPropertyMappings is not configured for the Hypervisor' is the fixed text of a DIFFERENT error class (the property-mappings precondition) and is simply the wrong answer here.

Model evidence (smithy_query, services/service-com-amazonaws-backupgateway-backuponpremises-v20210101/smithy-model/com-amazonaws-backupgateway-backuponpremises-v20210101.json): ValidationException errorKind=client httpError=400 with members ErrorCode/Message; AccessDeniedException errorKind=client httpError=403 with required ErrorCode/Message; both are bound to StartVirtualMachinesMetadataSync.

## Steps to Reproduce
1. git_clone the repo and boot the emulator built from it.
2. Replay the wire corpus (trace set simparity-wire-aws-v1): replay_wire_corpus().
3. Observe fixture 'backup-gateway-metadata-sync-hypervisor-arn-refusals', step 'task#6' (and 'task#7') report verdict=mismatch for StartVirtualMachinesMetadataSync.
4. Equivalently: call StartVirtualMachinesMetadataSync against one regional endpoint with a well-formed HypervisorArn whose region segment names a DIFFERENT region.
5. Expected (AWS/recorded): HTTP 400 with __type 'com.amazonaws.backuponpremises#ValidationException', ErrorCode 'InvalidResourceARN', Message 'The resource does not belong to the region.'
6. Actual (emulator): HTTP 400 with __type 'AccessDeniedException', ErrorCode 'MappingsNotConfigured', Message 'HypervisorPropertyMappings is not configured for the Hypervisor'.
