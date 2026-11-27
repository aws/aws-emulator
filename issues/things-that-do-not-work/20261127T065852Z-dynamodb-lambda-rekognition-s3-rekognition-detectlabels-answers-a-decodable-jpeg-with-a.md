Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: dynamodb__lambda__rekognition__s3.detect-labels-on-an-s3-object-narrows-by-maxlabels-and-minconfidence
Issue-Key: dynamodb__lambda__rekognition__s3.detect-labels-on-an-s3-object-narrows-by-maxlabels-and-minconfidence|run#8|AssertionError|assert len(labels) >= 2, (
Issue-Commit: 228c26f129cf4b4c1a06828a63287b3a2085e11b

# Rekognition DetectLabels answers a decodable JPEG with a single composition concept at 100% confidence, so MaxLabels capping and MinConfidence narrowing are unobservable

> _Note: this issue was identified against the code base at commit `228c26f129cf4b4c1a06828a63287b3a2085e11b`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DetectLabels

## Description
One net-new root cause (Cause 1 of 1), grounded in the single failing check the suite marked neither ALREADY FILED nor OWNED. Two related open issues exist but carry neither this replay key nor this symptom, and neither covers it: 20261121T045409Z-dynamodb-lambda-rekognition-s3-rekognition-detectlabels-answers-an-empty-labels-list-for.md reports ZERO labels for a different seeded JPEG (failing closed), and 20261127T000000Z-rekognition-detectlabels-names-no-real-world-entity-of-a-photograph.md reports that no reported name is a real-world ENTITY of a photograph. This cause is about CARDINALITY and CONFIDENCE SPREAD of the answer for a decodable JPEG: the emulator returns exactly ONE label, at a saturated 100.0 confidence, so none of DetectLabels' documented narrowing relations can be exercised at all.

Cause 1 of 1: DetectLabels on a decodable JPEG read out of S3 answers HTTP 200 with a single-element Labels array whose one entry sits at Confidence 100.0, instead of the several labels at differing confidences AWS reports for the same bytes. Failing check id: dynamodb__lambda__rekognition__s3.detect-labels-on-an-s3-object-narrows-by-maxlabels-and-minconfidence.

AWS: DetectLabels "detects instances of real-world entities within an image (JPEG or PNG)" and "For each object, scene, and concept the API returns one or more labels". Smithy: DetectLabels, input Image (required), MaxLabels target UInteger @range min 0, MinConfidence target Percent @range 0..100; output Labels (list of Label), OrientationCorrection, LabelModelVersion, ImageProperties; modeled errors AccessDeniedException, ImageTooLargeException, InvalidImageFormatException, InvalidParameterException, InvalidS3ObjectException, ProvisionedThroughputExceededException (client kind, HTTP 400), InternalServerError/ThrottlingException (server, 500). MinConfidence's own documentation: "Amazon Rekognition doesn't return any labels with confidence lower than this specified value". No error is involved in the diverging call: both sides answer HTTP 200, so this is a response-CONTENT divergence. For a decodable JPEG held in S3, DetectLabels(Image={S3Object:{Bucket,Name}}, MaxLabels=10, MinConfidence=70) answers 200 with at least two labels, carries LabelModelVersion, honours MaxLabels as a hard cap (MaxLabels=1 returns exactly the highest-confidence label of the wide answer), and applies MinConfidence as a monotone filter over the SAME scores (raising it to 95 can only remove labels from the MaxLabels=10/MinConfidence=70 answer, never add or re-score any).

Emulator: for the same decodable baseline JPEG PUT into the emulator's own S3 and read back through S3Object, DetectLabels(MaxLabels=10, MinConfidence=70) answers 200 with exactly ONE label — the frame's composition concept 'Texture' — at Confidence 100.0. The check fails its cardinality precondition (AssertionError: "the seeded image must yield at least two labels at MinConfidence 70 for the narrowing relations to be observable; DetectLabels returned [('Texture', 100.0)]"). Because the one label is reported at the top of the scale, every subsequent relation is also unexercisable: MaxLabels=1 can only ever return that same label, and raising MinConfidence to 95 removes nothing, so the cap and the filter are indistinguishable from no-ops. The single name and the value 100.0 are this run's values; the requirement is the class of answer (two or more labels, with confidences actually spread across the requested threshold).

Why: a client relying on AWS's behavior cannot drive any label-count or label-ranking logic off this emulator. Code that asks for a top-N answer (MaxLabels) always gets one candidate, so re-ranking, "best label" selection and "did anything else match?" branches are dead; code that tightens MinConfidence to trade recall for precision sees no change, so confidence thresholds cannot be tuned or regression-tested; and a saturated 100.0 confidence makes "high confidence" indistinguishable from "the only reading we had", so downstream gating on Confidence is meaningless. Any tag/route/moderation pipeline that expects an answer with several labels and a usable confidence spread silently collapses to one constant.

Fix: the label-detection state behind the rekognition DetectLabels handler — the component that turns the decoded frame into the Labels array and the confidence it reports for each entry, together with the MaxLabels/MinConfidence narrowing applied on top of it. For a decodable JPEG it must produce more than one label with confidences that actually differ, so that MaxLabels caps a ranked list and MinConfidence filters it monotonically over the same scores. This is the same component the two related open issues named above address from the "empty list" and "no real-world entity" sides; the single-saturated-concept answer is a third, separate symptom of it and is the one this replay key pins. File and line citations are under Traces.

Traces: dynamodb__lambda__rekognition__s3.detect-labels-on-an-s3-object-narrows-by-maxlabels-and-minconfidence (AssertionError at check line 51/the len(labels) >= 2 assertion). Component in the checkout: services/service-com-amazonaws-rekognition-rekognitionservice/src/state/label_detection.rs (its module documentation, lines 44-89, states that a frame with no located object is reported only by the composition concepts TEXTURE / PATTERN / MONOCHROME, and that PATTERN and MONOCHROME establish nothing for a frame with no boundary and no tonal span — which is exactly why a flat decodable JPEG yields the single TEXTURE label at full agreement).

## Evidence
Expected (real AWS, same request on the same decodable JPEG object):
  rekognition.detect_labels(Image={'S3Object': {'Bucket': <bucket>, 'Name': 'alpha.jpg'}}, MaxLabels=10, MinConfidence=70)
    -> HTTP 200, len(Labels) >= 2, every Confidence >= 70 with the values actually spread (not all at the top of the scale), LabelModelVersion present.
  rekognition.detect_labels(..., MaxLabels=1, MinConfidence=70)
    -> HTTP 200 with exactly one label, equal to the highest-confidence entry of the wide answer, same LabelModelVersion.
  rekognition.detect_labels(..., MaxLabels=10, MinConfidence=95)
    -> HTTP 200 whose Labels names are exactly the subset of the wide answer with Confidence >= 95 (monotone filter over the SAME scores).

Actual (emulator):
  rekognition.detect_labels(Image={'S3Object': {'Bucket': <bucket>, 'Name': 'alpha.jpg'}}, MaxLabels=10, MinConfidence=70)
    -> HTTP 200 with Labels = [('Texture', 100.0)] — one label only, at the top of the confidence scale.
  Check failure: AssertionError: the seeded image must yield at least two labels at MinConfidence 70 for the narrowing relations to be observable; DetectLabels returned [('Texture', 100.0)]

Fixed text vs variable parts: no HTTP status, error code or error message is involved — both sides answer 200 and nothing is refused. The requirement is on response content: two or more labels for a decodable JPEG, each at or above the requested MinConfidence, with confidences that differ, plus LabelModelVersion; MaxLabels a hard cap selecting the highest-confidence entries; MinConfidence a monotone filter over the same scores. The label name 'Texture' and the value 100.0 are this run's observed values, not part of the requirement.

## Steps to Reproduce
1. Boot the emulator from this checkout.
2. s3.create_bucket(Bucket=<name>) (with CreateBucketConfiguration.LocationConstraint=<region> when the region is not us-east-1).
3. s3.put_object(Bucket=<name>, Key='alpha.jpg', Body=<a small decodable baseline JPEG, e.g. a 64x64 solid RGB JPEG at quality 90>, ContentType='image/jpeg').
4. rekognition.detect_labels(Image={'S3Object': {'Bucket': <name>, 'Name': 'alpha.jpg'}}, MaxLabels=10, MinConfidence=70).
   Observed: HTTP 200 with a single label at Confidence 100.0. Expected: HTTP 200 with at least two labels, confidences >= 70 and actually spread, plus LabelModelVersion.
5. Repeat with MaxLabels=1, MinConfidence=70 and with MaxLabels=10, MinConfidence=95 — on the emulator both answers are the same single label, so the cap and the filter are no-ops.
6. Equivalent: re-replay check dynamodb__lambda__rekognition__s3.detect-labels-on-an-s3-object-narrows-by-maxlabels-and-minconfidence.
