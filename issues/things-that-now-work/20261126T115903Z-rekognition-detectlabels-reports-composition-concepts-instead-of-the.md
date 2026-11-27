Issue-Source: trace-sweep-divergence
Issue-Trace: rekognition-image-analysis-detect-operations
Issue-Step: task#3
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: rekognition-image-analysis-detect-operations rekognition-refarch-rekreup-detect-labels-s3object-mistakes
Issue-Shape-Id: com.amazonaws.rekognition#RekognitionService
Issue-Key: rekognition|DetectLabels|2xx->2xx|$.Labels[*]
Issue-Commit: 43586243266d19b209b15f2de40e433673053310

# Rekognition DetectLabels reports composition concepts instead of the real-world entities in the image

## Resolution of this report

Cause 2 of 2 is FIXED: `DetectLabels` no longer appends composition concepts a frame carries
nothing of, and no reading is reported at a confidence its own measurement did not establish, so
the request's `MinConfidence` narrows them like any other label. Replay of
`rekognition-refarch-rekreup-detect-labels-s3object-mistakes` (task#5 and task#6) passes.

Cause 1 of 2 is NOT fixed and is NOT claimed fixed by this report's move. Naming the entities of
the general-label taxonomy that an arbitrary photograph carries is a trained classifier over that
taxonomy; its weights can neither be shipped in this repository nor fetched at run time, and the
only ways to produce the recorded names without one — keying them to a file name, an object key, a
byte digest or the recorded answer — are fabrication. That cause is carried on as its own open
report, `issues/things-that-do-not-work/20261127T000000Z-rekognition-detectlabels-names-no-real-world-entity-of-a-photograph.md`,
which records the blocker and the fixture step that still diverges on it.

> _Note: this issue was identified against the code base at commit `43586243266d19b209b15f2de40e433673053310`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DetectLabels

## Description
This issue bundles 2 causes, both in the rekognition service, both on DetectLabels and both rooted in the same defect: the emulator has no general-label classifier, so the Labels list it answers with is built from the three colour-and-composition readings of the frame (TEXTURE / PATTERN / MONOCHROME, category "Colors and Visual Composition") plus, where the geometric face detector fires, FACE/HEAD/PERSON — never from the real-world entities of the general-label taxonomy that the submitted image carries. Both causes must be closed before submitting.

Cause 1 of 2 — DetectLabels answers an image's entity labels with colour/composition concepts only (fixture rekognition-image-analysis-detect-operations, step task#3).
AWS: DetectLabels "detects instances of real-world entities within an image (JPEG or PNG)… objects like flower, tree, and table; events like wedding, graduation, and birthday party; and concepts like landscape, evening, and nature" and returns Labels, "an array of labels for the real-world objects detected" (Smithy: DetectLabels, input Image required / MaxLabels UInteger range min 0 / MinConfidence Percent range 0..100 / Features DetectLabelsFeatureList len 0..2 / Settings DetectLabelsSettings; output Labels (Labels), OrientationCorrection, LabelModelVersion, ImageProperties; modeled errors AccessDeniedException, ImageTooLargeException, InvalidImageFormatException, InvalidParameterException, InvalidS3ObjectException, ProvisionedThroughputExceededException (400), InternalServerError, ThrottlingException (500)). Each Label carries Name, Confidence, Categories (the published taxonomy's category for that entity), Parents ("the response includes the all ancestors for a label, where every ancestor is a unique label") and Instances ("bounding boxes are described for all instances of detected common object labels"). For the fixture's image the recorded 200 answer is a list of entity labels drawn from across the taxonomy — person-description entities and their ancestors, an apparel/accessory entity carrying an Instance with a BoundingBox and that instance's own Confidence, and nature/animal/hobby concepts — i.e. several distinct categories, with ancestors expanded and at least one bounded instance. The recording's particular names, confidences and box coordinates are example values of that class, not the requirement.
Emulator: 200, but Labels holds three entries only — Monochrome, Texture and Pattern — every one of them Categories=[{Name: "Colors and Visual Composition"}], Parents=[], Instances=[], Aliases=[]. Not one entity of the recorded answer is present, no ancestor chain is reported, no Instance/BoundingBox is reported, and the face-derived labels do not appear either, so for this image the answer carries no real-world entity at all. The emulator's own documentation states the cause plainly: "Recognising a NAMED thing (a flower, a wedding, a table) needs a trained classifier over that whole taxonomy, and no such classifier exists in this process", so the three composition readings are all that can ever be reported for a frame no face was located in.
Why: a client that submits an image and routes, tags or filters on Labels[].Name gets a fixed three-element vocabulary that describes how the picture is composed rather than what is in it. Every content-driven branch (does this image contain a hardhat / a person / an animal?) is dead, Instances/BoundingBox consumers get nothing to draw, Parents-based rollups collapse, and Settings.GeneralLabels category/label filters can only ever match the one colour-and-composition category.
Fix: the label-production state behind the DetectLabels handler — services/service-com-amazonaws-rekognition-rekognitionservice/src/state/label_detection.rs (the set of Measured readings that become Labels) together with the taxonomy it expands through, src/state/label_taxonomy.rs. The entity labels the corpus records for a submitted image must be produced for that image, with their taxonomy Categories, their expanded Parents and an Instance (BoundingBox + Confidence) for located common objects; the colour-and-composition concepts must not stand in for them.
Traces: rekognition-image-analysis-detect-operations#task#3.

Cause 2 of 2 — the composition concepts are appended as EXTRA labels past the recorded set, at Confidence 100.0 so no MinConfidence floor removes them (fixture rekognition-refarch-rekreup-detect-labels-s3object-mistakes, step task#5; same divergence on step task#6).
AWS: the same operation and model as above. The Labels list AWS returns for an image read through Image.S3Object is exactly the entities it detected, narrowed by the request: MinConfidence "specifies the minimum confidence level for the labels to return… if MinConfidence is not specified, the operation returns labels with a confidence values greater than or equal to 55 percent", and MaxLabels is the "maximum number of labels you want the service to return… the service returns the specified number of highest confidence labels". The recorded 200 answer for this step carries ONE label; nothing beyond the entities detected is added, and a concept AWS is not confident about is dropped by the floor rather than reported at full confidence.
Emulator: 200 with the recorded label present but TWO further entries appended — Labels[1] = {Name: "Pattern", Confidence: 100.0, Categories: [{Name: "Colors and Visual Composition"}], Parents: [], Instances: [], Aliases: []} and Labels[2] = {Name: "Monochrome", Confidence: 100.0, …}. Hypothesis: these are the frame-composition readings of src/state/label_detection.rs, which by design are "not silenced by a floor of this service's own"; for a flat or single-tone frame the pattern and monochrome shares measure as complete agreement, i.e. 100.0, so they survive any MinConfidence the request can send and are also the highest-confidence entries MaxLabels would keep — crowding out genuine entity labels.
Why: a client gets labels for things that are not in the image, at the maximum possible confidence. Confidence thresholds cannot filter them (100.0 passes every floor), MaxLabels=N returns these synthetic concepts in preference to real detections because they rank highest, and counts/assertions over Labels ("exactly one label was detected") never reproduce AWS.
Fix: same component — src/state/label_detection.rs: the composition readings must not be emitted as Labels entries alongside the detected entities (the frame's colour/composition statistics belong under ImageProperties, where the model puts a property of the picture), and no reading may be reported at a confidence that makes it immune to the request's MinConfidence narrowing in label_detection::narrowed.
Traces: rekognition-refarch-rekreup-detect-labels-s3object-mistakes#task#5 and #task#6.

Neither cause is covered by an already-open issue. The replay marked both unowned. The open behavioral issue 20261121T045409Z-dynamodb-lambda-rekognition-s3-rekognition-detectlabels-answers-an-empty-labels-list-for.md reports the same component failing CLOSED (Labels = [] for a decodable S3-sourced JPEG) and carries behavioral check ids, not these fixture steps; the other open rekognition issues are unimplemented-operation stubs (DetectModerationLabels, ListProjectPolicies, GetLabelDetection) and the celebrity-catalogue issue 20261125T185721Z, none of which touch label production.

## Evidence
Replay of trace set simparity-wire-aws-v1 (emulator up; /__admin/health answered 200). Both causes are verdict=mismatch with expected_status 200 and actual_status 200 — the divergence is in response CONTENT.

Cause 1 — divergence key service=rekognition|DetectLabels|status=200->200|keys=$.Labels[0],$.Labels[1],$.Labels[2],$.Labels[3],$.Labels[4],$.Labels[5],$.Labels[6],$.Labels[7],$.Labels[8],$.Labels[9]; fixture rekognition-image-analysis-detect-operations, step task#3.
  Expected Labels[0..9] (recorded AWS): entity labels with taxonomy categories and expanded ancestors, e.g.
    {Name: 'Face', Confidence: 83.1154…, Categories: [{Name: 'Person Description'}], Parents: [{Name: 'Head'}, {Name: 'Person'}], Instances: [], Aliases: []}
    {Name: 'Head', Categories: [{Name: 'Person Description'}], Parents: [{Name: 'Person'}]}
    {Name: 'Hardhat', Categories: [{Name: 'Apparel and Accessories'}], Parents: [{Name: 'Clothing'}, {Name: 'Helmet'}], Instances: [{BoundingBox: {Height, Left, Top, Width}, Confidence}]}
    {Name: 'Snow', Categories: [{Name: 'Nature and Outdoors'}], Parents: [{Name: 'Nature'}, {Name: 'Outdoors'}]}
    {Name: 'Stencil', Categories: [{Name: 'Hobbies and Interests'}]}
    {Name: 'Animal', Categories: [{Name: 'Animals and Pets'}]}  … (ten labels in total)
  Actual Labels (emulator): three entries only —
    {Name: 'Monochrome', Confidence: 97.502…, Categories: [{Name: 'Colors and Visual Composition'}], Parents: [], Instances: [], Aliases: []}
    {Name: 'Texture',    Confidence: 96.700…, Categories: [{Name: 'Colors and Visual Composition'}], Parents: [], Instances: [], Aliases: []}
    {Name: 'Pattern',    Confidence: 84.949…, Categories: [{Name: 'Colors and Visual Composition'}], Parents: [], Instances: [], Aliases: []}
  Replay reported every expected index as `missing` (indices 3..9 got None, i.e. the emulator's list is shorter) and the emulator's own entries as `extra`. No category other than 'Colors and Visual Composition' appears, no Parents chain and no Instance/BoundingBox is reported.

Cause 2 — divergence key service=rekognition|DetectLabels|status=200->200|keys=$.Labels[1],$.Labels[2]; fixture rekognition-refarch-rekreup-detect-labels-s3object-mistakes, step task#5 (affected_count 2: also step task#6).
  $.Labels[1]: expected None, got {'Name': 'Pattern', 'Confidence': 100.0, 'Instances': [], 'Parents': [], 'Aliases': [], 'Categories': [{'Name': 'Colors and Visual Composition'}]}
  $.Labels[2]: expected None, got {'Name': 'Monochrome', 'Confidence': 100.0, 'Instances': [], 'Parents': [], 'Aliases': [], 'Categories': [{'Name': 'Colors and Visual Composition'}]}
  Labels[0] matched, so the divergence is purely the two appended composition concepts, each at the maximum Confidence 100.0.

Requirement vs example: no error code or HTTP status is involved (both sides 200). The requirement is on response content — Labels must name the real-world entities detected in the submitted image, each with its published-taxonomy Categories, its expanded Parents and an Instance (BoundingBox + Confidence) for located common objects, narrowed only by the request's MinConfidence (default 55) and MaxLabels; concepts the service is not actually confident about must be subject to that floor, and the frame's colour/composition statistics belong under ImageProperties. The specific names, confidence numbers and bounding-box coordinates quoted above are the recordings' example values filling that template, not literals to hard-code.

## Steps to Reproduce
1. Boot the emulator from this checkout and replay the wire corpus: replay_wire_corpus() (trace set simparity-wire-aws-v1).
2. Cause 1: replay fixture rekognition-image-analysis-detect-operations step task#3 — DetectLabels on the fixture's image. Observed: 200 with Labels = [Monochrome, Texture, Pattern], all in category 'Colors and Visual Composition', no Parents, no Instances. Expected: 200 with the ten recorded entity labels across several taxonomy categories, ancestors expanded as unique labels, and an Instance with a BoundingBox for the located common object.
3. Cause 2: replay fixture rekognition-refarch-rekreup-detect-labels-s3object-mistakes step task#5 (and step task#6, the same divergence) — DetectLabels with Image.S3Object pointing at the object the fixture stored. Observed: 200 with the recorded label at Labels[0] plus appended Labels[1]='Pattern' and Labels[2]='Monochrome', both Confidence 100.0. Expected: 200 with only the recorded label — no appended composition concepts.
4. Confirm the floor is bypassed: repeat step 3 with a high MinConfidence (e.g. 99) and/or MaxLabels=1. The 100.0-confidence composition concepts still pass the floor and rank highest, so they are returned in preference to the detected entity.
5. Re-replay all three steps after the fix: rekognition-image-analysis-detect-operations#task#3, rekognition-refarch-rekreup-detect-labels-s3object-mistakes#task#5 and #task#6.
