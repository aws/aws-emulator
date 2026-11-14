Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: transfer.agreement-update-conflicting-directories-rejected
Issue-Shape-Id: com.amazonaws.transfer#TransferService
Issue-Key: transfer.agreement-update-conflicting-directories-rejected|run#17|AssertionError|assert error == ("InvalidRequestException", expected_message), error
Issue-Commit: cbcb3257918cb52483b21ac09d54481cbc170336

# UpdateAgreement's both-directories refusal carries emulator-authored prose instead of AWS's "Both BaseDirectory and CustomDirectories are configured..." message

> _Note: this issue was identified against the code base at commit `cbcb3257918cb52483b21ac09d54481cbc170336`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
UpdateAgreement

## Description
ONE root cause in the transfer emulator, grounded in one failing behavioral check of this suite.

=== Cause 1 of 1: the InvalidRequestException message for an UpdateAgreement that names BOTH BaseDirectory and CustomDirectories is the emulator's own sentence, and it drops the request values AWS echoes ===

AWS: Smithy (com.amazonaws.transfer#TransferService) UpdateAgreement input AgreementId (@required, len 19..19, ^a-([0-9a-f]{17})$), ServerId (@required, len 19..19, ^s-([0-9a-f]{17})$), BaseDirectory (HomeDirectory, len 0..1024, ^(|/.*)$), CustomDirectories (CustomDirectoriesType), output AgreementId (@required); modeled errors InternalServiceError 503, InvalidRequestException 400, ResourceExistsException 409, ResourceNotFoundException 404, ServiceUnavailableException 503, ThrottlingException 429. The operation's own modeled documentation states the rule: "Specify either BaseDirectory or CustomDirectories, but not both. Specifying both causes the command to fail." The MESSAGE shape of that refusal is recorded behavior only: AWS answers HTTP 400 InvalidRequestException whose Message is built from a fixed template that echoes the conflicting request values, in this exact form —

  Both BaseDirectory and CustomDirectories are configured. BaseDirectory: <BaseDirectory>, CustomDirectories: CustomDirectoriesType(failedFilesDirectory=<FailedFilesDirectory>, mdnFilesDirectory=<MdnFilesDirectory>, payloadFilesDirectory=<PayloadFilesDirectory>, statusFilesDirectory=<StatusFilesDirectory>, temporaryFilesDirectory=<TemporaryFilesDirectory>)

Fixed text: "Both BaseDirectory and CustomDirectories are configured. BaseDirectory: ", the literal ", CustomDirectories: CustomDirectoriesType(", the five lowerCamelCase member labels in that order (failedFilesDirectory, mdnFilesDirectory, payloadFilesDirectory, statusFilesDirectory, temporaryFilesDirectory), the ", " separators, the "=" between label and value, and the closing ")". Variable parts, all taken from the REQUEST: the BaseDirectory string and the five CustomDirectories member values. (The bucket/prefix paths in the recorded run are one run's values filling that template, not part of it.)

Emulator: the code and status are right — HTTP 400 InvalidRequestException — but the Message is emulator-authored prose with no request values at all: "BaseDirectory and CustomDirectories cannot both be specified". It is produced by crate::state::agreement_directory_update_violation(...) and raised in services/service-com-amazonaws-transfer-transferservice/src/ops/update_agreement.rs (the pre-read guard at lines 213-221).

Why: the error message is part of the requirement for this error class. A client, assertion, or operator runbook that matches AWS's sentence for a both-directories UpdateAgreement never matches, and because the emulator's text echoes neither BaseDirectory nor the CustomDirectories members, a caller that parses the message to learn WHICH conflicting values the service saw (common when an update is assembled from merged config) gets nothing to report and cannot tell which side to drop.

Fix: the refusal text for the both-directories violation in the transfer crate's agreement state helper (crate::state::agreement_directory_update_violation, used by the UpdateAgreement handler's validation) must be rendered from AWS's template, interpolating the request's BaseDirectory and the five CustomDirectories members in AWS's fixed order and lowerCamelCase labels. Note the same helper also guards the create path, so correcting the text must not regress whatever CreateAgreement checks currently pass.

Traces: transfer.agreement-update-conflicting-directories-rejected.

## Evidence
Check transfer.agreement-update-conflicting-directories-rejected, assertion at check line 93:
  assert error == ("InvalidRequestException", expected_message), error
  AssertionError: ('InvalidRequestException', 'BaseDirectory and CustomDirectories cannot both be specified')

Expected (code, message): ("InvalidRequestException", "Both BaseDirectory and CustomDirectories are configured. BaseDirectory: <request BaseDirectory>, CustomDirectories: CustomDirectoriesType(failedFilesDirectory=<request FailedFilesDirectory>, mdnFilesDirectory=<request MdnFilesDirectory>, payloadFilesDirectory=<request PayloadFilesDirectory>, statusFilesDirectory=<request StatusFilesDirectory>, temporaryFilesDirectory=<request TemporaryFilesDirectory>)") with HTTP 400.
Actual: HTTP 400, code InvalidRequestException (correct), message "BaseDirectory and CustomDirectories cannot both be specified" — fixed prose that is not AWS's text and omits every echoed request value.

Source: services/service-com-amazonaws-transfer-transferservice/src/ops/update_agreement.rs lines 207-221 returns InvalidRequestException with the message returned by crate::state::agreement_directory_update_violation(input.base_directory, input.custom_directories).

The rest of the check's expectations are satisfied once the message matches: the update is correctly not applied (DescribeAgreement still reports the original BaseDirectory and no CustomDirectories), and the positive control (CustomDirectories alone) succeeds.

## Steps to Reproduce
1. Boot the emulator.
2. Create an AS2 server: transfer.create_server(Protocols=['AS2'], ProtocolDetails={'As2Transports': ['HTTP']}, EndpointType='VPC', EndpointDetails={'VpcId': <vpc>, 'SubnetIds': [<subnet>]}).
3. iam.create_role trusted by transfer.amazonaws.com; transfer.create_profile(ProfileType='LOCAL') and transfer.create_profile(ProfileType='PARTNER').
4. transfer.create_agreement(ServerId=<server>, LocalProfileId=<local>, PartnerProfileId=<partner>, AccessRole=<role arn>, BaseDirectory='/<bucket>/as2', Status='INACTIVE').
5. transfer.update_agreement(AgreementId=<agreement>, ServerId=<server>, BaseDirectory='/<bucket>/as2', CustomDirectories={'FailedFilesDirectory': '/<bucket>/failed', 'MdnFilesDirectory': '/<bucket>/mdn', 'PayloadFilesDirectory': '/<bucket>/payload', 'StatusFilesDirectory': '/<bucket>/status', 'TemporaryFilesDirectory': '/<bucket>/temp'}).
6. Observe HTTP 400 InvalidRequestException with Message 'BaseDirectory and CustomDirectories cannot both be specified' instead of 'Both BaseDirectory and CustomDirectories are configured. BaseDirectory: /<bucket>/as2, CustomDirectories: CustomDirectoriesType(failedFilesDirectory=/<bucket>/failed, mdnFilesDirectory=/<bucket>/mdn, payloadFilesDirectory=/<bucket>/payload, statusFilesDirectory=/<bucket>/status, temporaryFilesDirectory=/<bucket>/temp)'.
