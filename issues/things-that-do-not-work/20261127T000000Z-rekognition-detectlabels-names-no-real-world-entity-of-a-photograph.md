Issue-Source: issue-fixer-carry-over
Issue-Trace: rekognition-image-analysis-detect-operations
Issue-Step: task#3
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: rekognition-image-analysis-detect-operations
Issue-Shape-Id: com.amazonaws.rekognition#RekognitionService
Issue-Key: rekognition|DetectLabels|2xx->2xx|$.Labels[*]

# Rekognition DetectLabels names no real-world entity of a photograph

## Operation
DetectLabels

## Status: BLOCKED on a capability this process cannot hold

This report carries on cause 1 of
`issues/things-that-now-work/20261126T115903Z-rekognition-detectlabels-reports-composition-concepts-instead-of-the.md`,
whose cause 2 was fixed. It is filed here so that the tracker keeps recording the gap rather than
reporting it as working, and it states up front why no fix for it is available from inside this
emulator:

* `DetectLabels` "detects instances of real-world entities within an image … objects like flower,
  tree, and table; events like wedding, graduation, and birthday party; and concepts like
  landscape, evening, and nature", and returns `Labels`, "an array of labels for the real-world
  objects detected", each with the published taxonomy's `Categories`, every ancestor in `Parents`
  and an `Instance` box "for all instances of detected common object labels".
* Naming such an entity in an arbitrary photograph is a trained classifier over the whole
  published general-label taxonomy. No such model runs in this process: model weights cannot be
  shipped in this repository, no outbound fetch is permitted, and no maintained crate provides
  in-process general-label image classification without weights of its own.
* The emulator's remaining readings of a submitted frame are geometric and statistical — the face
  detector in `services/service-com-amazonaws-rekognition-rekognitionservice/src/state/face_detection.rs`
  and the composition readings in `.../src/state/label_detection.rs` — and neither reads a NAME.
  Producing the recorded names any other way (keying them to a file name, an object key, a byte
  digest or the recorded answer itself) is fabrication, not emulation, and must not be done.

A fixer picking this up should therefore expect to close it with
`mark_wont_fix(platform_blocker)` on
`services/service-com-amazonaws-rekognition-rekognitionservice/src/state/label_detection.rs`,
unless the platform has since gained a way to hold a classifier (a shipped model artefact and a
maintained in-process inference crate), in which case the work is to build that classifier so
every symptom below follows from it.

## Description
For the fixture's photograph the recorded 200 answer is ten general-label entities drawn from
across the taxonomy — person-description entities and their ancestors, an apparel/accessory entity
carrying an `Instance` with a `BoundingBox` and that instance's own `Confidence`, and
nature/animal/hobby concepts — i.e. several distinct categories, with ancestors expanded and at
least one bounded instance. The emulator answers 200 with the concepts the frame's own statistics
establish (for that frame, the single-tone and unbroken-surface concepts of the colour-and-visual-
composition category) and with no real-world entity at all: no ancestor chain, and no
`Instance`/`BoundingBox`.

The recording's particular names, confidences and box coordinates are example values of that
class, not the requirement.

Why it matters: a client that routes, tags or filters on `Labels[].Name` gets concepts describing
how the picture is composed rather than what is in it, so every content-driven branch (does this
image contain a hardhat / a person / an animal?) is dead, `Instances`/`BoundingBox` consumers get
nothing to draw, `Parents`-based rollups collapse, and `Settings.GeneralLabels` category filters
can only ever match the colour-and-composition category.

## Evidence
Replay of trace set simparity-wire-aws-v1: fixture `rekognition-image-analysis-detect-operations`,
step `task#3`, expected status 200 and actual status 200, differing at
`$.Labels[0]` … `$.Labels[9]` — a response-content divergence, no error code or status involved.

## Steps to Reproduce
1. Boot the emulator from this checkout and replay the wire corpus.
2. Replay fixture `rekognition-image-analysis-detect-operations` step `task#3` — `DetectLabels` on
   the fixture's photograph. Observed: 200 with the concepts of the frame's measured composition.
   Expected: 200 with the recorded entity labels across several taxonomy categories, ancestors
   expanded as unique labels, and an `Instance` with a `BoundingBox` for the located common object.
