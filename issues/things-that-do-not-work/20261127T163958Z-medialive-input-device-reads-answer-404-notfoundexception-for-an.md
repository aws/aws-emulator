Issue-Source: trace-sweep-divergence
Issue-Trace: medialive-input-device-thumbnail-refusals
Issue-Step: task#1
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: medialive-input-device-thumbnail-refusals
Issue-Shape-Id: com.amazonaws.medialive#MediaLive
Issue-Key: medialive|DescribeInputDeviceThumbnail|4xx->4xx|$.message
Issue-Key: medialive|DescribeInputDeviceThumbnail|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: medialive|DescribeInputDeviceThumbnail|4xx->4xx|status
Issue-Key: medialive|DescribeInputDevice|4xx->4xx|$.message
Issue-Key: medialive|DescribeInputDevice|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: medialive|DescribeInputDevice|4xx->4xx|status
Issue-Commit: 5bf95535a88c836b855c8cd193d84d7babf276ba

# Input-device reads answer 404 NotFoundException for an unusable InputDeviceId instead of 422 invalid id

> _Note: this issue was identified against the code base at commit `5bf95535a88c836b855c8cd193d84d7babf276ba`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeInputDevice

## Description
Two divergence clusters, one shared fault: both read paths on an input device classify an InputDeviceId that the service cannot accept as a missing resource (404 NotFoundException) instead of refusing the request as unprocessable (422, message 'invalid id'), so the status class, the error type and the message all diverge. 2 causes must be closed.

Cause 1 of 2: DescribeInputDeviceThumbnail answers 404 NotFoundException instead of 422 'invalid id'
AWS: Smithy: DescribeInputDeviceThumbnail is GET /prod/inputDevices/{InputDeviceId}/thumbnailData with a required InputDeviceId httpLabel and a required accept header; com.amazonaws.medialive#UnprocessableEntityException is modelled with @httpError 422 and members message/validationErrors, and NotFoundException with @httpError 404. The operation's bound error list does not include the 422, so for this refusal the requirement is recorded behavior: AWS answers HTTP 422 with body {"message": "invalid id"} when the InputDeviceId in the path is one the service will not process, rather than reporting a missing device. The message is entirely fixed text - 'invalid id' - with no variable part; the device id in the recording's URI is a request value, not part of the message.
Emulator: answers HTTP 404 with x-amzn-errortype 'NotFoundException' and message 'InputDevice <id> not found' (the recording's run shows 'InputDevice hd-02c4199b7d7cc49 not found', i.e. the message interpolates the requested id).
Why: 422 and 404 mean opposite things to a client. A 404 says the device id is well-formed but absent, so a caller retries after a provisioning wait, polls for the device to appear, or treats the device as deleted; a 422 says the id itself will never work and the caller must fix its input. Code that branches on UnprocessableEntityException vs NotFoundException takes the wrong branch, and any error-message assertion sees a not-found sentence instead of 'invalid id'.
Fix: the InputDeviceId handling in the DescribeInputDeviceThumbnail handler of the medialive crate (services/service-com-amazonaws-medialive-medialive/src/ops) - validate the id before the state lookup and map an unusable id to the 422 UnprocessableEntityException with message 'invalid id', reserving 404 NotFoundException for an acceptable id that is simply absent.
Traces: medialive-input-device-thumbnail-refusals#task#1.

Cause 2 of 2: DescribeInputDevice answers 404 NotFoundException instead of 422 'invalid id'
AWS: Smithy: DescribeInputDevice is GET /prod/inputDevices/{InputDeviceId} with the same required httpLabel id; same modelled error classes (UnprocessableEntityException @httpError 422 with a message member, NotFoundException @httpError 404). Recorded behavior: the same unusable id yields HTTP 422 with body {"message": "invalid id"} on the plain describe path too - the refusal is a property of the id, not of the thumbnail sub-resource, so both paths must classify it identically.
Emulator: answers HTTP 404 with x-amzn-errortype 'NotFoundException' and message 'InputDevice <id> not found'.
Why: same consequence as cause 1 - a validation refusal is served as a missing-resource answer, so a client retries or treats the device as deleted instead of correcting its request; and the two paths cannot be made consistent by fixing only one of them.
Fix: the InputDeviceId handling in the DescribeInputDevice handler (same crate's src/ops) - share the id-acceptance check with the thumbnail path so both refuse with 422 UnprocessableEntityException / 'invalid id'.
Traces: medialive-input-device-thumbnail-refusals#task#2.

The requirement is the status (422), the error class (UnprocessableEntityException, the 422 the model carries) and the fixed message text 'invalid id'. The id in the recorded URI and in the emulator's current message is a per-run request value and is not part of the requirement.

## Evidence
Replay of simparity-wire-aws-v1 against the emulator built from this checkout; 2 mismatch clusters on fixture medialive-input-device-thumbnail-refusals:
- step task#1, DescribeInputDeviceThumbnail: 'status: expected 422, got 404'; '$.message: value_mismatch (expected "invalid id", got "InputDevice hd-02c4199b7d7cc49 not found")'; '$headers.x-amzn-errortype: shape_mismatch (expected <shaped-not-asserted>, got "NotFoundException")'.
- step task#2, DescribeInputDevice: 'status: expected 422, got 404'; '$.message: value_mismatch (expected "invalid id", got "InputDevice hd-02c4199b7d7cc49 not found")'; '$headers.x-amzn-errortype: shape_mismatch (expected <shaped-not-asserted>, got "NotFoundException")'.
Model support for the expected class: com.amazonaws.medialive#UnprocessableEntityException has errorKind client, @httpError 422 and a message member (jsonName 'message'); NotFoundException has @httpError 404. 'hd-02c4199b7d7cc49' is the id the recording sent in the path - evidence of the input, not a value to hardcode.

## Steps to Reproduce
1. Replay fixture medialive-input-device-thumbnail-refusals, step task#1: GET /prod/inputDevices/<unusable-input-device-id>/thumbnailData with the accept header. Expected HTTP 422 with {"message": "invalid id"}; the emulator returns HTTP 404, x-amzn-errortype NotFoundException and a message that interpolates the requested id.
2. Replay step task#2 of the same fixture: GET /prod/inputDevices/<the same unusable id>. Expected HTTP 422 with {"message": "invalid id"}; the emulator again returns 404 NotFoundException.
3. Both steps pass only once both handlers refuse an unusable InputDeviceId with the 422 UnprocessableEntityException and the message 'invalid id', keeping 404 for an acceptable-but-absent device.
