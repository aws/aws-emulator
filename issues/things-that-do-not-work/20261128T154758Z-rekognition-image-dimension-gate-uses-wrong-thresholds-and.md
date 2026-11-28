Issue-Source: trace-sweep-divergence
Issue-Trace: rekognition-detect-faces-image-dimension-limit
Issue-Step: task#1
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: rekognition-detect-faces-image-dimension-limit rekognition-detect-labels-image-dimension-limit rekognition-recognize-celebrities-image-dimension-limit rekognition-detect-protective-equipment-dimension-limits rekognition-protective-equipment-image-dimension-bounds
Issue-Shape-Id: com.amazonaws.rekognition#RekognitionService
Issue-Key: rekognition|DetectFaces|2xx->4xx|$.FaceDetails
Issue-Key: rekognition|DetectFaces|2xx->4xx|status
Issue-Key: rekognition|DetectFaces|4xx->4xx|$.Code
Issue-Key: rekognition|DetectFaces|4xx->4xx|$.Message
Issue-Key: rekognition|DetectFaces|4xx->4xx|$.__type
Issue-Key: rekognition|DetectLabels|2xx->4xx|$.LabelModelVersion
Issue-Key: rekognition|DetectLabels|2xx->4xx|$.Labels
Issue-Key: rekognition|DetectLabels|2xx->4xx|status
Issue-Key: rekognition|DetectLabels|4xx->4xx|$.Message
Issue-Key: rekognition|RecognizeCelebrities|2xx->4xx|$.CelebrityFaces
Issue-Key: rekognition|RecognizeCelebrities|2xx->4xx|$.UnrecognizedFaces
Issue-Key: rekognition|RecognizeCelebrities|2xx->4xx|status
Issue-Key: rekognition|RecognizeCelebrities|4xx->4xx|$.Code
Issue-Key: rekognition|RecognizeCelebrities|4xx->4xx|$.Message
Issue-Key: rekognition|RecognizeCelebrities|4xx->4xx|$.__type
Issue-Key: rekognition|DetectProtectiveEquipment|4xx->2xx|$.Code
Issue-Key: rekognition|DetectProtectiveEquipment|4xx->2xx|$.Message
Issue-Key: rekognition|DetectProtectiveEquipment|4xx->2xx|$.__type
Issue-Key: rekognition|DetectProtectiveEquipment|4xx->2xx|status
Issue-Commit: 2ad04cd39860f70eb0b7d33f623aafe0280db657

# Rekognition image dimension gate uses wrong thresholds and wrong refusal errors across the Detect/Recognize image operations

> _Note: this issue was identified against the code base at commit `2ad04cd39860f70eb0b7d33f623aafe0280db657`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DetectFaces

## Description
Seven net-new root causes, all of them the SAME emulator fault seen from seven sides: the shared image-source gate that the synchronous image operations call applies ITS OWN pixel thresholds (at most 8192 pixels on a side and 4,000,000 pixels in all) and ITS OWN refusal error/message, instead of the per-operation dimension limits, error classes and messages that Amazon Rekognition applies. The result is refusals where AWS answers 200, acceptances where AWS refuses, and a hand-written message in place of every modelled one.

No open issue covers this. 20261127T144756Z-rekognition-ppe-4-mib-image-limit-not-enforced.md covers the PPE 4 MiB BYTE ceiling (and three unrelated video-job causes); the causes below are the DIMENSION/resolution gate, a different check on a different input property. 20261127T000000Z-rekognition-detectlabels-names-no-real-world-entity-of-a-photograph.md and 20261127T065852Z-...-rekognition-detectlabels-answers-a-decodable-jpeg-... cover WHICH labels DetectLabels names for a decodable image, not whether an in-range image is accepted at all.

=== Cause 1 of 7: DetectFaces refuses an image that is within AWS's dimension limits ===
AWS: DetectFaces analyses any decodable image whose dimensions are within the service limit (Smithy: DetectFaces.Image required; errors ImageTooLargeException, InvalidImageFormatException, InvalidParameterException, all httpError 400; ImageTooLargeException docs: "The input image size exceeds the allowed limit"). Recorded behavior: an image whose larger side is at the limit and whose total pixel count far exceeds 4,000,000 is ANALYSED — HTTP 200 with a FaceDetails list (empty when the image contains no face), never an error.
Emulator: HTTP 400 with no body members at all ($.FaceDetails missing, got None) — the shared gate refuses the image because it is wider than 8192 px and/or larger than 4,000,000 px in total, thresholds that are the emulator's own and not Rekognition's.
Why: a client that sizes its images against Rekognition's documented limits has a perfectly valid request refused, so a working code path becomes an error path; worse, the error is a 400 client error, so retry/backoff logic will not recover and the caller concludes its own image is invalid.
Fix: DetectFaces' validation, specifically the per-side/total-pixel thresholds in the shared image-source dimension check it calls — they must be the operation's AWS limits, not a local 8192/4MP pair.
Traces: rekognition-detect-faces-image-dimension-limit#task#1 (also task#3).

=== Cause 2 of 7: DetectFaces words an over-dimension refusal as ImageTooLargeException with a hand-written message instead of InvalidImageFormatException ===
AWS: when the image's dimensions are past what DetectFaces will decode, the refusal is HTTP 400 with error code InvalidImageFormatException and the message "Request has invalid image format" (Smithy: InvalidImageFormatException is bound to DetectFaces, httpError 400, docs "The provided image format is not supported"). That message is entirely fixed text — it interpolates nothing from the request, so no width, height or limit appears in it.
Emulator: HTTP 400 with $.__type and $.Code both "ImageTooLargeException" and $.Message "Image size is too large: the image is 10001x80 pixels; this emulator analyses at most 8192 pixels on a side and 4000000 pixels in all" — the wrong error class, plus a message that is not AWS wording at all and that advertises the emulator and its private thresholds to the caller.
Why: the error code is what a client branches on. A caller that catches InvalidImageFormatException to re-encode the image never sees it, while an ImageTooLargeException handler fires and a size-reduction path runs for a condition AWS classifies as a format problem. The message naming "this emulator" also leaks the emulator into client logs and user-facing text.
Fix: DetectFaces' error mapping/serializer — the over-dimension condition must raise InvalidImageFormatException with AWS's fixed sentence, not the local too-large error.
Traces: rekognition-detect-faces-image-dimension-limit#task#2 (also task#4).

=== Cause 3 of 7: DetectLabels refuses an image that is within AWS's dimension limits ===
AWS: DetectLabels analyses any decodable image within the service's dimension limit and answers HTTP 200 with Labels (each entry carrying Name, Confidence, Instances, Parents, Aliases, Categories) and LabelModelVersion (Smithy: DetectLabels output Labels: Labels, LabelModelVersion: String; errors ImageTooLargeException/InvalidImageFormatException/InvalidParameterException at httpError 400). Recorded behavior: the same wide image is analysed and returns a populated Labels list plus LabelModelVersion.
Emulator: HTTP 400; $.Labels and $.LabelModelVersion are absent entirely (got None), because the shared 8192-px/4MP gate rejects the image before any analysis.
Why: identical to cause 1 but on the labelling path — an in-range image the client sized correctly produces a non-retryable 400 instead of labels, so label-dependent logic (filtering, thresholding, routing on a label) never runs.
Fix: DetectLabels' validation — the same shared dimension check, called with the operation's AWS limits.
Traces: rekognition-detect-labels-image-dimension-limit#task#1 (also task#3).

=== Cause 4 of 7: DetectLabels' over-dimension message is the emulator's own sentence instead of AWS's dimension sentence ===
AWS: for DetectLabels the over-dimension refusal keeps error code ImageTooLargeException (the emulator already matches on code and status here — only the message diverges) and words it as "Input image dimensions <width> x <height> exceed maximum dimension size of <max> pixels". Fixed text: "Input image dimensions ", " x ", " exceed maximum dimension size of ", " pixels". Variable parts: <width> and <height> are the submitted image's measured pixel dimensions, and <max> is Rekognition's documented maximum dimension for the operation. The recorded instance of that template is "Input image dimensions 10001 x 80 exceed maximum dimension size of 10000 pixels"; 10001, 80 and 10000 are the fixture's values filling the template, not part of it.
Emulator: $.Message is "Image size is too large: the image is 10001x80 pixels; this emulator analyses at most 8192 pixels on a side and 4000000 pixels in all" — different fixed text, a different dimension format (10001x80 rather than "10001 x 80"), and a limit that is the emulator's internal threshold rather than the service maximum.
Why: the message is a requirement of the error, and this one both reads differently and states a WRONG limit, so a client (or a human reading a log) that parses or displays it is told to resize to 8192 when AWS's ceiling is the documented maximum; it also exposes the emulator's identity in a production-shaped error.
Fix: DetectLabels' error serializer / the shared too-large message builder — it must emit AWS's dimension template with the measured width/height and the modelled maximum interpolated.
Traces: rekognition-detect-labels-image-dimension-limit#task#2 (also task#4).

=== Cause 5 of 7: RecognizeCelebrities refuses an image that is within AWS's dimension limits ===
AWS: RecognizeCelebrities answers HTTP 200 with CelebrityFaces and UnrecognizedFaces for any decodable in-range image, both lists present and empty when nothing is recognised (Smithy: RecognizeCelebrities output CelebrityFaces: CelebrityList, UnrecognizedFaces: ComparedFaceList; errors at httpError 400 include ImageTooLargeException and InvalidImageFormatException). Recorded behavior: the wide in-range image yields 200 with CelebrityFaces [] and UnrecognizedFaces [].
Emulator: HTTP 400; both $.CelebrityFaces and $.UnrecognizedFaces are absent (got None) — same shared gate, same wrong thresholds.
Why: a caller that always reads both lists off a 200 gets a 400 instead, and the two always-present output lists are missing, so even lenient clients that tolerate empties break.
Fix: RecognizeCelebrities' validation — the shared dimension check with the operation's AWS limits.
Traces: rekognition-recognize-celebrities-image-dimension-limit#task#1 (also task#3).

=== Cause 6 of 7: RecognizeCelebrities words an over-dimension refusal as ImageTooLargeException instead of InvalidImageFormatException ===
AWS: past the decodable dimension the refusal is HTTP 400, error code InvalidImageFormatException, message "Request has invalid image format" — entirely fixed text, interpolating nothing from the request (Smithy: InvalidImageFormatException bound to RecognizeCelebrities, httpError 400).
Emulator: $.__type and $.Code are "ImageTooLargeException" and $.Message is the emulator's "Image size is too large: the image is 10001x80 pixels; this emulator analyses at most 8192 pixels on a side and 4000000 pixels in all".
Why: as in cause 2 — the wrong exception class steers a client into a resize-and-retry path for a condition AWS reports as an unsupported format, and the message is not AWS's.
Fix: RecognizeCelebrities' error mapping/serializer.
Traces: rekognition-recognize-celebrities-image-dimension-limit#task#2 (also task#4).

=== Cause 7 of 7: DetectProtectiveEquipment ACCEPTS an image whose resolution exceeds the PPE limit ===
AWS: DetectProtectiveEquipment has tighter image limits than the generic image surface and refuses an over-resolution image with HTTP 400, error code InvalidParameterException and the message "Request has invalid parameters" — fixed text interpolating nothing from the request (Smithy: DetectProtectiveEquipment.Image required; errors include InvalidParameterException and ImageTooLargeException at httpError 400; ImageTooLargeException docs: "If you are calling DetectProtectiveEquipment, the image size or resolution exceeds the allowed limit"). Recorded behavior: the over-dimension image is refused with that code and message.
Emulator: HTTP 200 with no error members at all ($.__type, $.Code, $.Message all missing) — the PPE handler never applies a resolution ceiling, so it analyses and returns a success body for input AWS rejects. Note this is the resolution gate, NOT the 4 MiB byte ceiling already filed as 20261127T144756Z-rekognition-ppe-4-mib-image-limit-not-enforced.md.
Why: the direction is the dangerous one — a client's too-large/invalid-parameter handling for PPE is never exercised against the emulator and ships untested, and the emulator reports a PPE analysis (no persons / empty summary) for an image real Rekognition refuses outright, so the caller silently trusts a result AWS would never have produced.
Fix: DetectProtectiveEquipment's validation — it needs the operation's own resolution ceiling wired into the shared image-source dimension check, mapped to InvalidParameterException with AWS's fixed sentence.
Traces: rekognition-detect-protective-equipment-dimension-limits#task#2 (also task#3 and rekognition-protective-equipment-image-dimension-bounds#task#2, #task#3, #task#4).

## Evidence
Cause 1 — DetectFaces, rekognition-detect-faces-image-dimension-limit#task#1: expected HTTP 200 with $.FaceDetails == []; actual HTTP 400, $.FaceDetails missing (got None). Same cluster: task#3.

Cause 2 — DetectFaces, same fixture#task#2: expected 400 with $.__type == $.Code == 'InvalidImageFormatException' and $.Message == 'Request has invalid image format'; actual 400 with $.__type == $.Code == 'ImageTooLargeException' and $.Message == 'Image size is too large: the image is 10001x80 pixels; this emulator analyses at most 8192 pixels on a side and 4000000 pixels in all'. Same cluster: task#4.

Cause 3 — DetectLabels, rekognition-detect-labels-image-dimension-limit#task#1: expected HTTP 200 with a populated $.Labels list (Name/Confidence/Instances/Parents/Aliases/Categories per entry) and $.LabelModelVersion present; actual HTTP 400, $.Labels and $.LabelModelVersion missing (got None). Same cluster: task#3.

Cause 4 — DetectLabels, same fixture#task#2: status and error code match; expected $.Message == 'Input image dimensions 10001 x 80 exceed maximum dimension size of 10000 pixels'; actual $.Message == 'Image size is too large: the image is 10001x80 pixels; this emulator analyses at most 8192 pixels on a side and 4000000 pixels in all'. Same cluster: task#4.

Cause 5 — RecognizeCelebrities, rekognition-recognize-celebrities-image-dimension-limit#task#1: expected HTTP 200 with $.CelebrityFaces == [] and $.UnrecognizedFaces == []; actual HTTP 400, both missing (got None). Same cluster: task#3.

Cause 6 — RecognizeCelebrities, same fixture#task#2: expected 400 with $.__type == $.Code == 'InvalidImageFormatException', $.Message == 'Request has invalid image format'; actual 400 with 'ImageTooLargeException' and the emulator's size sentence. Same cluster: task#4.

Cause 7 — DetectProtectiveEquipment, rekognition-detect-protective-equipment-dimension-limits#task#2: expected HTTP 400 with $.__type == $.Code == 'InvalidParameterException' and $.Message == 'Request has invalid parameters'; actual HTTP 200 with none of those members present. Same cluster: rekognition-detect-protective-equipment-dimension-limits#task#3 and rekognition-protective-equipment-image-dimension-bounds#task#2, #task#3, #task#4 (5 steps).

Common signature: every refusal the emulator invents cites "at most 8192 pixels on a side and 4000000 pixels in all", which is neither a Rekognition limit nor mentioned by the model — it is the shared gate's own threshold, and it is simultaneously too strict for DetectFaces/DetectLabels/RecognizeCelebrities and absent for DetectProtectiveEquipment.

## Steps to Reproduce
1. Point an AWS SDK/botocore Rekognition client at the emulator's data endpoint.
2. Cause 1/3/5: submit a decodable image whose larger side is at Rekognition's documented maximum dimension and whose total pixel count exceeds 4,000,000 (the fixtures use a 10000-pixel-wide strip) to DetectFaces(Image={'Bytes': ...}), DetectLabels(Image=...) and RecognizeCelebrities(Image=...). AWS answers 200 (FaceDetails / Labels+LabelModelVersion / CelebrityFaces+UnrecognizedFaces); the emulator answers 400 with no body members.
3. Cause 2/6: submit an image one pixel past that maximum dimension (fixtures use 10001x80) to DetectFaces and RecognizeCelebrities and read (Code, Message): AWS gives ('InvalidImageFormatException', 'Request has invalid image format'); the emulator gives ('ImageTooLargeException', 'Image size is too large: ... this emulator analyses at most 8192 pixels on a side and 4000000 pixels in all').
4. Cause 4: submit the same over-dimension image to DetectLabels and read Message: AWS gives 'Input image dimensions <w> x <h> exceed maximum dimension size of <max> pixels'; the emulator gives its own too-large sentence with the 8192/4000000 thresholds.
5. Cause 7: submit an image whose resolution exceeds the PPE limit to DetectProtectiveEquipment(Image=...): AWS answers 400 ('InvalidParameterException', 'Request has invalid parameters'); the emulator answers 200 with a PPE result body.
6. Or re-replay the fixture steps named in Issue-Trace/Issue-Step and trace_ids with the wire corpus.
