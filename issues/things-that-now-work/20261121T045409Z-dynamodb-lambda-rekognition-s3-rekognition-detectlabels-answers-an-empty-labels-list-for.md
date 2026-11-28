Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: dynamodb__lambda__rekognition__s3.detect-labels-on-a-jpg-named-object-holding-text-rejected-invalid-image-format dynamodb__lambda__rekognition__s3.detect-labels-on-a-missing-s3-object-rejected-invalid-s3-object
Issue-Key: dynamodb__lambda__rekognition__s3.detect-labels-on-a-jpg-named-object-holding-text-rejected-invalid-image-format|run#16|AssertionError|assert len(ok["Labels"]) >= 1, (
Issue-Key: dynamodb__lambda__rekognition__s3.detect-labels-on-a-missing-s3-object-rejected-invalid-s3-object|run#13|AssertionError|assert len(ok["Labels"]) >= 1, (
Issue-Commit: 2f215cd8a92428a081069d88561cad2cfdd06f78

# Rekognition DetectLabels answers an empty Labels list for a decodable JPEG read from S3, so the positive control of both S3-rejection checks fails

> _Note: this issue was identified against the code base at commit `2f215cd8a92428a081069d88561cad2cfdd06f78`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DetectLabels

## Description
One net-new root cause (Cause 1 of 1), grounded in two failing checks the suite marked neither ALREADY FILED nor OWNED. A related open issue exists but does not carry these replay keys and reports a different symptom: 20261120T143926Z-dynamodb-lambda-rekognition-s3-detectlabels-on-an-s3-sourced-image-returns-a.md reports DetectLabels answering a DIFFERENT seeded image with exactly ONE colour-family label ('Red'); the two checks here get ZERO labels for their image, which is what makes the positive control of each rejection check fail.

Cause 1 of 1: DetectLabels on a decodable JPEG stored in S3 answers 200 with an EMPTY Labels array. Failing check ids: dynamodb__lambda__rekognition__s3.detect-labels-on-a-jpg-named-object-holding-text-rejected-invalid-image-format and dynamodb__lambda__rekognition__s3.detect-labels-on-a-missing-s3-object-rejected-invalid-s3-object.
AWS: DetectLabels "detects instances of real-world entities within an image (JPEG or PNG)" and returns, per the model, Labels — "An array of labels for the real-world objects detected" — plus LabelModelVersion (Smithy: DetectLabels; input Image required, MaxLabels target UInteger range min 0, MinConfidence target Percent range 0..100; output Labels (Labels), OrientationCorrection, LabelModelVersion, ImageProperties; modeled errors include InvalidImageFormatException and InvalidS3ObjectException, both client kind, HTTP 400). For a decodable JPEG held in S3, DetectLabels(Image={S3Object:{Bucket,Name}}, MaxLabels=10, MinConfidence=70) answers HTTP 200 with a NON-EMPTY Labels array (each entry at or above the requested MinConfidence). Both checks use exactly that call as the positive control that proves the bucket, the bytes and the permissions are fine, so that only the one thing each check broke (non-image bytes under a .jpg key; a key that does not exist) explains the refusal. No error code is involved in the diverging call — both sides answer 200; this is a response-content divergence.
Emulator: for the same decodable JPEG bytes PUT into the emulator's own S3 and read back through S3Object, DetectLabels(MaxLabels=10, MinConfidence=70) answers 200 with Labels = [] — no label at all. Both checks reach their control only AFTER their negative leg has already passed (the non-image bytes are correctly refused InvalidImageFormatException with the message "Request has invalid image format", and the missing key is correctly refused InvalidS3ObjectException), so the refusal contract is intact and the divergence is isolated to the analysis answer.
Why: a client that submits an image AWS labels gets nothing back, so every branch keyed on label presence is dead and "the image has no recognisable content" becomes indistinguishable from "the call was refused" or "the object was unreadable". A negative test that proves a refusal by contrasting it with a working control can never be written against this emulator, and tag/route pipelines driven by Labels[].Name silently stop producing work.
Fix: the label vocabulary and detection state behind the rekognition DetectLabels handler (the component that produces the Labels array from the decoded frame, and the ancestor/category expansion it feeds) — for a decodable JPEG it must return at least one real-world-entity label at the requested MinConfidence, with LabelModelVersion, instead of an empty array. The already-open issue named above addresses the same component from the "one colour-family label" side; an empty array for this image is the same component failing closed.
Traces: dynamodb__lambda__rekognition__s3.detect-labels-on-a-jpg-named-object-holding-text-rejected-invalid-image-format (AssertionError: "the control call on real JPEG bytes must return labels; got []"), dynamodb__lambda__rekognition__s3.detect-labels-on-a-missing-s3-object-rejected-invalid-s3-object (AssertionError: "the control call must return at least one label, proving the bucket, the bytes and the permissions are all fine and only the key was wrong; got []").

## Evidence
Expected (recorded AWS, same request on the same decodable JPEG object):
  rekognition.detect_labels(Image={'S3Object': {'Bucket': <bucket>, 'Name': 'alpha.jpg'}}, MaxLabels=10, MinConfidence=70) -> HTTP 200 with len(Labels) >= 1, each entry a real-world-entity label with Confidence >= 70, plus LabelModelVersion.

Actual (emulator), both checks:
  same call -> HTTP 200 with Labels = [] (empty array).
  check detect-labels-on-a-jpg-named-object-holding-text-rejected-invalid-image-format:
    AssertionError: the control call on real JPEG bytes must return labels; got []
  check detect-labels-on-a-missing-s3-object-rejected-invalid-s3-object:
    AssertionError: the control call must return at least one label, proving the bucket, the bytes and the permissions are all fine and only the key was wrong; got []

In the same runs the refusal legs were CORRECT and are not part of this issue: non-image bytes stored under a .jpg key with ContentType image/jpeg were refused ('InvalidImageFormatException', 'Request has invalid image format') and a missing key was refused InvalidS3ObjectException, and S3 HeadObject/GetObject on the lying object still reported image/jpeg with the stored bytes unchanged.

Fixed text vs variable parts: no error message is involved (both sides are HTTP 200). The requirement is on response CONTENT — at least one real-world-entity label at or above the requested MinConfidence for a decodable JPEG. Specific label names and confidence values from any one run are run values, not part of the requirement.

## Steps to Reproduce
1. CreateBucket (with CreateBucketConfiguration.LocationConstraint when the region is not us-east-1).
2. s3.put_object(Bucket=<bucket>, Key='alpha.jpg', Body=<a small decodable baseline JPEG>, ContentType='image/jpeg').
3. rekognition.detect_labels(Image={'S3Object': {'Bucket': <bucket>, 'Name': 'alpha.jpg'}}, MaxLabels=10, MinConfidence=70).
Observed: HTTP 200 with Labels = []. Expected: HTTP 200 with at least one label (Name/Confidence >= 70) and LabelModelVersion.
4. For contrast (these already behave correctly): PUT non-image bytes under 'beta.jpg' with ContentType image/jpeg and call detect_labels on it -> InvalidImageFormatException "Request has invalid image format"; call detect_labels on a key that does not exist -> InvalidS3ObjectException.
5. Equivalent: re-replay checks dynamodb__lambda__rekognition__s3.detect-labels-on-a-jpg-named-object-holding-text-rejected-invalid-image-format and dynamodb__lambda__rekognition__s3.detect-labels-on-a-missing-s3-object-rejected-invalid-s3-object.
