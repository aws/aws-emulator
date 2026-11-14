Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__iam__iot__s3.cfn-iot-stream-create-and-delete-with-stack cloudformation__iam__iot__s3.cfn-iot-stream-nonexistent-s3-object-rejected cloudformation__iam__iot__s3.cfn-iot-ota-update-create-and-delete-with-stack
Issue-Key: cloudformation__iam__iot__s3|CreateStack|ValidationError
Issue-Commit: 3e55f33fd01bce8af894192328b0955056ba342a

# cloudformation: CreateStack refuses AWS::IoT::Stream and AWS::IoT::OTAUpdate as unrecognized resource types

> _Note: this issue was identified against the code base at commit `3e55f33fd01bce8af894192328b0955056ba342a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateStack

## Description
Two net-new root causes in the CloudFormation resource-type registry, both reached from the cloudformation__iam__iot__s3 behavioral section. Both must be closed before this issue is submitted.

Cause 1 of 2: CloudFormation does not recognize the AWS::IoT::Stream resource type.
AWS: CreateStack validates a template's resource types against the registry of supported types and refuses only genuinely unknown ones with HTTP 400 ValidationError and the message `Template format error: Unrecognized resource types: [<Type>, ...]` (fixed text, with the bracketed list taken from the template's unknown types). AWS::IoT::Stream IS a registered type: the stack creates an IoT stream from an S3 file (IoT CreateStream), the physical id is the stream id, GetAtt serves its Arn/StreamId, an update changes the stream's files/role, and stack deletion deletes the stream. A stream whose S3 file does not exist is refused at provisioning time and the stack rolls back, not refused at template validation. Smithy: cloudformation#CreateStack declares no resource-type vocabulary (the registry is a service fact) — recorded behavior only for the type list; the refusal's status/code/message template above is what the recording shows AWS using for a truly unknown type.
Emulator: every CreateStack whose template contains AWS::IoT::Stream is refused at validation with ValidationError `Template format error: Unrecognized resource types: [AWS::IoT::Stream]`, so no stack is ever created and neither the happy path nor the negative (nonexistent S3 object) path can be reached.
Why: a client deploying IoT streaming/OTA infrastructure through CloudFormation cannot create the stack at all; the failure arrives as a template-format ValidationError, which a client reads as "my template is wrong" rather than "this provider is missing", and the documented rollback behavior for a bad S3 object can never be observed.
Fix: the CloudFormation resource-type registry plus a provisioner/handler for AWS::IoT::Stream (create/update/delete mapped onto the IoT control plane, GetAtt attributes, and provisioning-time refusal for a missing S3 object).
Traces: cloudformation__iam__iot__s3.cfn-iot-stream-create-and-delete-with-stack, cloudformation__iam__iot__s3.cfn-iot-stream-nonexistent-s3-object-rejected.

Cause 2 of 2: CloudFormation does not recognize the AWS::IoT::OTAUpdate resource type.
AWS: AWS::IoT::OTAUpdate is a registered type; the stack creates an OTA update job (IoT CreateOTAUpdate) from a target thing/group and a signed file, exposes its id and Arn through Ref/GetAtt, and deletes it with the stack. An unknown type is the only case that yields `Template format error: Unrecognized resource types: [...]`. Recorded behavior only (the registry is a service fact, not modelled in Smithy).
Emulator: CreateStack answers HTTP 400 ValidationError `Template format error: Unrecognized resource types: [AWS::IoT::OTAUpdate]`.
Why: the whole OTA-update deployment path is unavailable through CloudFormation, and the client is told its template is malformed when it is valid against AWS.
Fix: the CloudFormation resource-type registry plus a provisioner/handler for AWS::IoT::OTAUpdate.
Traces: cloudformation__iam__iot__s3.cfn-iot-ota-update-create-and-delete-with-stack.

## Evidence
Behavioral suite replay (simparity-behavioral-aws-v1) against the emulator built from this checkout; section cloudformation__iam__iot__s3 0/3 passed.

Cause 1:
[error] cloudformation__iam__iot__s3.cfn-iot-stream-create-and-delete-with-stack — botocore.exceptions.ClientError: An error occurred (ValidationError) when calling the CreateStack operation: Template format error: Unrecognized resource types: [AWS::IoT::Stream]
[error] cloudformation__iam__iot__s3.cfn-iot-stream-nonexistent-s3-object-rejected — identical ValidationError / "Template format error: Unrecognized resource types: [AWS::IoT::Stream]" (the check never reaches the expected provisioning-time refusal and rollback).

Cause 2:
[error] cloudformation__iam__iot__s3.cfn-iot-ota-update-create-and-delete-with-stack — botocore.exceptions.ClientError: An error occurred (ValidationError) when calling the CreateStack operation: Template format error: Unrecognized resource types: [AWS::IoT::OTAUpdate]

Expected in all three: CreateStack accepted (stack id returned), the stack reaching CREATE_COMPLETE with the IoT resource provisioned (or, for the negative check, CREATE_FAILED/ROLLBACK_COMPLETE with a provisioning-time reason naming the missing S3 object), and the resource removed on stack delete.

## Steps to Reproduce
1. git_clone the repo; build and boot the emulator from the checkout.
2. Cause 1: call cloudformation CreateStack with a template containing a single AWS::IoT::Stream resource (Files pointing at an S3 object plus a RoleArn). Observe HTTP 400 ValidationError "Template format error: Unrecognized resource types: [AWS::IoT::Stream]". Repeat with an S3 key that does not exist — AWS refuses at provisioning and rolls the stack back instead. Replay cloudformation__iam__iot__s3.cfn-iot-stream-create-and-delete-with-stack and cloudformation__iam__iot__s3.cfn-iot-stream-nonexistent-s3-object-rejected.
3. Cause 2: call CreateStack with a template containing an AWS::IoT::OTAUpdate resource. Observe the same ValidationError naming [AWS::IoT::OTAUpdate]. Replay cloudformation__iam__iot__s3.cfn-iot-ota-update-create-and-delete-with-stack.
