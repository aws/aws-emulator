Issue-Source: trace-sweep-divergence
Issue-Trace: api-image-scanning-and-findings
Issue-Step: task#9
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: api-image-scanning-and-findings
Issue-Shape-Id: com.amazonaws.ecr#AmazonEC2ContainerRegistry_V20150921
Issue-Key: ecr|StartImageScan|2xx->2xx|$.imageScanStatus.status
Issue-Key: ecr|DescribeImageScanFindings|2xx->2xx|$.imageScanStatus.description
Issue-Commit: 99a6bf4e8a9dd64f73b61f622317c845b92ac2b7

# ECR image-scan status surface reports the wrong ScanStatus and an emulator-authored status description

> _Note: this issue was identified against the code base at commit `99a6bf4e8a9dd64f73b61f622317c845b92ac2b7`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
StartImageScan

## Description
Two net-new root causes in the ECR image-scanning surface: the scan status a freshly started scan reports, and the status description ECR serves for an image basic scanning cannot assess. Both are values in the ImageScanStatus structure the scan surface returns, and both are emulator-authored rather than what ECR serves.

Cause 1 of 2: StartImageScan reports a newly admitted scan as IN_PROGRESS instead of PENDING.
AWS: Smithy: StartImageScan output member imageScanStatus targets ImageScanStatus{status: ScanStatus, description: ScanStatusDescription}; ScanStatus is an enum whose values include PENDING and IN_PROGRESS (ecr#ScanStatus). The recorded response is HTTP 200 with $.imageScanStatus.status == "PENDING": ECR admits the scan request and reports the scan as queued in the StartImageScan response itself, and only later reports IN_PROGRESS (through DescribeImageScanFindings) once the scan is actually running. The enum carries both states precisely because the queued state is observable.
Emulator: answers HTTP 200 with $.imageScanStatus.status == "IN_PROGRESS" directly out of StartImageScan, collapsing the queued state so PENDING is never observable for any image.
Why: a client that starts a scan and then polls until the status leaves PENDING (the documented way to wait for a queued scan to begin) sees a scan that was never queued, so queue-aware logic — back-off before the first findings poll, distinguishing "accepted but not yet started" from "running" — never triggers against the emulator and behaves differently against AWS.
Fix: the StartImageScan handler's response projection / the scan record's initial status in the ECR image-scan state: the record must be created in the queued state and reported as PENDING by StartImageScan, with the transition to IN_PROGRESS owned by the scan's progression rather than by the start call.
Traces: api-image-scanning-and-findings#task#9 (expected $.imageScanStatus.status 'PENDING', got 'IN_PROGRESS'; both sides HTTP 200).

Cause 2 of 2: the UNSUPPORTED_IMAGE status description is emulator-worded.
AWS: Smithy: ImageScanStatus.description targets ScanStatusDescription ("The description of the image scan status"); the model does not fix the text, so the wording is recorded behavior only. The recording shows ECR serving, for an image basic scanning cannot assess, HTTP 200 with $.imageScanStatus.description == "UnsupportedImageError: The image is not supported." That whole sentence is fixed text — it has no variable parts taken from the request or from state — and the "UnsupportedImageError: " prefix is the machine-readable part clients key on.
Emulator: answers HTTP 200 with $.imageScanStatus.description == "UnsupportedImageError: The operating system and/or package manager are not supported.", i.e. the correct prefix followed by a different, emulator-authored sentence.
Why: the scan-status description is the only place the reason for an unscannable image is reported (DescribeImageScanFindings still answers 200, not an error), so tooling that surfaces or matches that sentence reports text no AWS account ever produces.
Fix: the ECR scan-status description constants used by the DescribeImageScanFindings response projection (the unsupported-image status description in particular).
Traces: api-image-scanning-and-findings#task#10 (DescribeImageScanFindings, expected description 'UnsupportedImageError: The image is not supported.').

## Evidence
Replay of trace set simparity-wire-aws-v1 against the emulator built from mainline (66 fixtures, 456 steps, 10 mismatched):

1) fixture api-image-scanning-and-findings step task#9, StartImageScan — expected HTTP 200, got HTTP 200; $.imageScanStatus.status: value_mismatch (expected 'PENDING', got 'IN_PROGRESS').
2) fixture api-image-scanning-and-findings step task#10, DescribeImageScanFindings — expected HTTP 200, got HTTP 200; $.imageScanStatus.description: value_mismatch (expected 'UnsupportedImageError: The image is not supported.', got 'UnsupportedImageError: The operating system and/or package manager are not supported.').

Model facts used: ecr#ScanStatus values [IN_PROGRESS, COMPLETE, FAILED, UNSUPPORTED_IMAGE, ACTIVE, PENDING, SCAN_ELIGIBILITY_EXPIRED, FINDINGS_UNAVAILABLE, LIMIT_EXCEEDED, IMAGE_ARCHIVED]; StartImageScan/DescribeImageScanFindings both return imageScanStatus: ImageScanStatus{status, description}.

## Steps to Reproduce
1. Replay fixture api-image-scanning-and-findings against the emulator.
2. Step task#9 calls StartImageScan for a repository image; compare $.imageScanStatus.status in the 200 response — AWS reports PENDING, the emulator reports IN_PROGRESS.
3. Step task#10 calls DescribeImageScanFindings for an image basic scanning cannot assess; compare $.imageScanStatus.description in the 200 response — AWS reports 'UnsupportedImageError: The image is not supported.', the emulator reports 'UnsupportedImageError: The operating system and/or package manager are not supported.'.
