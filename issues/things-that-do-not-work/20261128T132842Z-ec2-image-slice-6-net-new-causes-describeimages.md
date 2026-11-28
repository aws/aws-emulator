Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ec2.op-contract-deregister-image ec2.snapshot-in-use-by-registered-image-delete-rejected ec2.image-register-describe-deregister-lifecycle ec2.op-contract-delete-image-usage-report ec2.op-contract-describe-image-usage-reports ec2.op-contract-describe-image-attribute ec2.op-contract-modify-image-attribute
Issue-Shape-Id: com.amazonaws.ec2#AmazonEC2
Issue-Key: ec2|DescribeImages|InvalidAMIID.NotFound
Issue-Key: ec2.image-register-describe-deregister-lifecycle|run#26|AssertionError|assert mutate_code == "InvalidAMIID.Unavailable", (
Issue-Key: ec2|DescribeImageUsageReports|InvalidImageUsageReportId.NotFound
Issue-Key: ec2.op-contract-describe-image-usage-reports|run#14|AssertionError|assert code == "InvalidFilter", f"expected InvalidFilter, got {code}"
Issue-Key: ec2.op-contract-describe-image-attribute|run#10|AssertionError|assert code == "AuthFailure", f"expected AuthFailure for the deprecated attribute, got {code}"
Issue-Key: ec2.op-contract-modify-image-attribute|run#20|AssertionError|assert msg == expected, f"expected message {expected!r}, got {msg!r}"
Issue-Commit: c3e5fb924e64874a3c7124e31a5bf7fe6194984f

# ec2 image slice: 6 net-new causes — DescribeImages/DeregisterImage refuse a deregistered AMI id, DescribeImageUsageReports refuses a deleted report id and mis-classifies an unknown filter, and Describe/ModifyImageAttribute answer the wrong AuthFailure code and message

> _Note: this issue was identified against the code base at commit `c3e5fb924e64874a3c7124e31a5bf7fe6194984f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeImages

## Description
Six net-new root causes in the ec2 image slice (AMI reads and refusals, the image-attribute APIs and the image-usage-report APIs). All six must be closed before this issue is submitted. Related open issue: 20261125T150300Z-ec2-2-net-new-causes-describeimages-refuses-a.md records DescribeImages refusing a deregistered AMI id with InvalidAMIID.Malformed; the classification has since changed to InvalidAMIID.NotFound but the call is still refused, so Cause 1's checks still fail, and Cause 2 is a different operation (DeregisterImage) and a different refusal.

=== Cause 1 of 6: DescribeImages on a deregistered, well-formed AMI id is refused InvalidAMIID.NotFound instead of answering 200 with an empty Images list ===
AWS: Smithy (services/service-com-amazonaws-ec2-amazonec2/smithy-model/com-amazonaws-ec2-amazonec2.json) declares no modelled errors for DescribeImages or DeregisterImage — this is the ec2Query protocol, so error codes are recorded behavior only; DescribeImagesResult$Images is a list (xmlName imagesSet) and the ImageState enum is [pending, available, invalid, deregistered, transient, failed, error, disabled]. Recorded behavior: once DeregisterImage succeeds, DescribeImages naming that id answers HTTP 200 with an EMPTY Images list, immediately and durably, and the id also drops out of DescribeImages(Owners=["self"]); InvalidAMIID.NotFound is reserved for a well-formed id that never existed, and InvalidAMIID.Malformed (message: fixed text `Invalid id: ` plus the caller's value in double quotes) for a value that is not shaped like an AMI id at all.
Emulator: after DeregisterImage returns, DescribeImages(ImageIds=[<that id>]) is refused HTTP 400 InvalidAMIID.NotFound with message `The image id '[<the requested id>]' does not exist` — fixed text `The image id '[`, the requested image id from the request, then `]' does not exist` (one recording shows ami-000000000000000c3 / ami-000000000000000c4; the id itself is a per-run value, not part of the requirement). The deregistered image is purged from state rather than retained as a selectable-but-empty id.
Why: a client that deregisters an AMI and then polls DescribeImages to confirm it is gone gets a hard 400 that botocore raises instead of a page, so AMI cleanup loops, deletion waiters and "is it deregistered yet" polls abort. The empty-page / NotFound / Malformed distinction is exactly what callers key their retry logic on.
Fix: the DescribeImages handler and its id resolution in services/service-com-amazonaws-ec2-amazonec2/src/ops/describe_images.rs, plus the deregistered-image bookkeeping in crate::state it reads: an AMI this account deregistered must select no image and return 200 with an empty Images list.
Traces: ec2.op-contract-deregister-image, ec2.snapshot-in-use-by-registered-image-delete-rejected.

=== Cause 2 of 6: a second DeregisterImage on an already-deregistered id is refused InvalidAMIID.Malformed instead of InvalidAMIID.Unavailable with AWS's "no longer available" wording ===
AWS: no modelled errors on DeregisterImage (recorded behavior only). Recorded behavior: repeating DeregisterImage on an id this account has just deregistered is refused InvalidAMIID.Unavailable with the message built as fixed text `The image ID '` + the request's image id + `' is no longer available` (once the record is fully purged AWS may answer InvalidAMIID.NotFound instead; Malformed is not one of the outcomes for a well-formed id).
Emulator: the second DeregisterImage on the emulator's own freshly minted, well-formed id (`ami-` + 17 lowercase hex characters) answers InvalidAMIID.Malformed — the id is reclassified as syntactically invalid once its row is gone.
Why: a caller retrying a deregistration, or a cleanup path that tolerates InvalidAMIID.NotFound/InvalidAMIID.Unavailable as "already done", sees a Malformed error instead and re-raises; the code also tells the client its id is syntactically wrong, which is false and unactionable.
Fix: the DeregisterImage handler services/service-com-amazonaws-ec2-amazonec2/src/ops/deregister_image.rs — classify a well-formed id whose image was deregistered as Unavailable (with AWS's wording) or NotFound, and keep Malformed for values that are not AMI ids.
Traces: ec2.image-register-describe-deregister-lifecycle.

=== Cause 3 of 6: DescribeImageUsageReports refuses a report id it no longer holds instead of answering an empty ImageUsageReports list ===
AWS: Smithy models DescribeImageUsageReports with ReportIds (list, length 0-200), Filters, MaxResults/NextToken and output ImageUsageReports (xmlName imageUsageReportSet); no modelled errors (ec2Query — recorded behavior only). Recorded behavior: after DeleteImageUsageReport(ReportId=X) returns Return=true, DescribeImageUsageReports(ReportIds=[X]) answers 200 with an empty ImageUsageReports list (the read is the way a client confirms the delete), while DeleteImageUsageReport on the same id is the call that is then refused InvalidImageUsageReportId.NotFound with fixed text `The image-usage-report ID '` + the request's report id + `' does not exist`.
Emulator: DescribeImageUsageReports(ReportIds=[<deleted id>]) is refused HTTP 400 InvalidImageUsageReportId.NotFound with message `The image usage report id '<report id>' does not exist` — a refusal where AWS serves a page, and with "image usage report id" where AWS writes "image-usage-report ID".
Why: the delete can never be confirmed: the read a client uses to see the report is gone raises, so the whole report lifecycle (create, delete, verify) aborts and the subsequent repeat-delete boundary is never reached.
Fix: the DescribeImageUsageReports handler services/service-com-amazonaws-ec2-amazonec2/src/ops/describe_image_usage_reports.rs — an unknown/removed report id in ReportIds must select nothing and return an empty list, not refuse.
Traces: ec2.op-contract-delete-image-usage-report.

=== Cause 4 of 6: DescribeImageUsageReports rejects an unknown filter name with InvalidParameterValue instead of EC2's InvalidFilter ===
AWS: Filters on DescribeImageUsageReports is the shared FilterList; errors are recorded behavior only. Recorded behavior: a filter whose Name is not a supported filter is refused InvalidFilter with the fixed message `An invalid filter name was provided. Remove the filter and try again.` (no variable parts).
Emulator: the same request is refused with code InvalidParameterValue, so the error class a client dispatches on is wrong.
Why: EC2 clients distinguish "you named a filter that does not exist" (InvalidFilter — fix the request shape) from "a parameter value is out of range" (InvalidParameterValue — fix the value). Collapsing them makes the failure unactionable and breaks error-code-based branching.
Fix: the filter-name validation on the DescribeImageUsageReports path in services/service-com-amazonaws-ec2-amazonec2/src/ops/describe_image_usage_reports.rs (and the shared EC2 filter validation helper it calls), which must raise the InvalidFilter class with AWS's fixed text.
Traces: ec2.op-contract-describe-image-usage-reports.

=== Cause 5 of 6: DescribeImageAttribute's refusal of the deprecated blockDeviceMapping attribute carries the error code Client.AuthFailure instead of AuthFailure ===
AWS: ImageAttributeName includes blockDeviceMapping; DescribeImageAttribute has no modelled errors (ec2Query — recorded behavior only). Recorded behavior: DescribeImageAttribute(Attribute="blockDeviceMapping") is refused with error code exactly `AuthFailure`, even on a caller-owned AMI (the attribute is documented to always fail that way).
Emulator: the refusal is serialized with code `Client.AuthFailure` — the EC2 query error Code element carries a `Client.` prefix EC2 does not use.
Why: every client matches EC2 error codes literally (botocore's error_code, retry/condition handling, `except` branches on "AuthFailure"). A prefixed code matches nothing, so a documented permission refusal reads as an unknown error.
Fix: the error serialization on the DescribeImageAttribute path — services/service-com-amazonaws-ec2-amazonec2/src/ops/describe_image_attribute.rs and the ec2Query error writer it uses — must emit the bare code `AuthFailure`.
Traces: ec2.op-contract-describe-image-attribute.

=== Cause 6 of 6: ModifyImageAttribute's AuthFailure message for an AMI the caller does not own re-words AWS's fixed text ===
AWS: no modelled errors on ModifyImageAttribute (recorded behavior only). Recorded behavior: modifying LaunchPermission on an AMI the caller does not own (e.g. an amazon-published AMI) is refused AuthFailure with the message built as fixed text `Not authorized for image:` immediately followed by the request's image id — no space, no brackets, singular "image".
Emulator: the code is right (AuthFailure) but the message is `Not authorized for images: [<image id>]` — plural, a space, and the id wrapped in brackets.
Why: the message is part of the refusal contract; tooling and operators that surface or match on it (and any test or runbook quoting AWS's wording) see different text for the same refusal, and the bracketed plural form suggests a list where AWS names exactly one image.
Fix: the not-authorized refusal in the ModifyImageAttribute handler services/service-com-amazonaws-ec2-amazonec2/src/ops/modify_image_attribute.rs must render AWS's singular `Not authorized for image:<image id>` template.
Traces: ec2.op-contract-modify-image-attribute.

## Evidence
Cause 1 — Expected: DeregisterImage(ImageId=ami-X) succeeds, then DescribeImages(ImageIds=[ami-X]) answers HTTP 200 with Images: [] (and ami-X is absent from DescribeImages(Owners=["self"])). Actual (both checks error out on that read): `botocore.exceptions.ClientError: An error occurred (InvalidAMIID.NotFound) when calling the DescribeImages operation: The image id '[ami-000000000000000c3]' does not exist` and `... The image id '[ami-000000000000000c4]' does not exist`. The ami- ids are per-run values; the divergence is the 400 refusal where the recording has a 200 empty page.

Cause 2 — Expected: a second DeregisterImage on the deregistered id is refused InvalidAMIID.Unavailable, message `The image ID '<that id>' is no longer available`. Actual: `AssertionError: expected a second DeregisterImage on the deregistered id to be refused, got 'InvalidAMIID.Malformed'` (ec2.image-register-describe-deregister-lifecycle, assertion `mutate_code == "InvalidAMIID.Unavailable"`).

Cause 3 — Expected: after DeleteImageUsageReport(ReportId=X) returns Return=true, DescribeImageUsageReports(ReportIds=[X]) answers 200 with an empty ImageUsageReports list. Actual: `botocore.exceptions.ClientError: An error occurred (InvalidImageUsageReportId.NotFound) when calling the DescribeImageUsageReports operation: The image usage report id 'imgusgrpt-000000000000000ea' does not exist` (report id is a per-run value).

Cause 4 — Expected: DescribeImageUsageReports(Filters=[{Name: "<unknown filter>", Values: ["x"]}]) is refused InvalidFilter, message `An invalid filter name was provided. Remove the filter and try again.` Actual: `AssertionError: expected InvalidFilter, got InvalidParameterValue`.

Cause 5 — Expected: DescribeImageAttribute(ImageId=<owned ami>, Attribute="blockDeviceMapping") is refused with error code `AuthFailure`. Actual: `AssertionError: expected AuthFailure for the deprecated attribute, got Client.AuthFailure`.

Cause 6 — Expected message: `Not authorized for image:<ami id>`. Actual message: `Not authorized for images: [<ami id>]` — recorded as `AssertionError: expected message 'Not authorized for image:ami-0000000000000009d', got 'Not authorized for images: [ami-0000000000000009d]'` (the ami id is a per-run value filling the template).

## Steps to Reproduce
Cause 1: (1) CreateVolume(Size=1) in an available AZ and wait for `available`; CreateSnapshot and wait for `completed`. (2) RegisterImage(Architecture=x86_64, VirtualizationType=hvm, RootDeviceName=/dev/xvda, BlockDeviceMappings=[{DeviceName:/dev/xvda, Ebs:{SnapshotId:<snap>}}]) and wait for the AMI to be `available`. (3) DeregisterImage on it. (4) Call DescribeImages(ImageIds=[<that id>]) — observe HTTP 400 InvalidAMIID.NotFound instead of 200 with an empty Images list.

Cause 2: continue from Cause 1 step 4 and call DeregisterImage(ImageId=<that id>) a second time — observe InvalidAMIID.Malformed instead of InvalidAMIID.Unavailable `The image ID '<that id>' is no longer available`.

Cause 3: (1) Pick or create a self-owned AMI. (2) CreateImageUsageReport(ImageId=<ami>, ResourceTypes=[{ResourceType:"ec2:Instance"}]) and note ReportId. (3) DeleteImageUsageReport(ReportId=<id>) — Return=true. (4) Call DescribeImageUsageReports(ReportIds=[<id>]) — observe InvalidImageUsageReportId.NotFound instead of 200 with an empty ImageUsageReports list.

Cause 4: call DescribeImageUsageReports(Filters=[{"Name": "sim2real-bogus-filter", "Values": ["x"]}]) — observe InvalidParameterValue instead of InvalidFilter with `An invalid filter name was provided. Remove the filter and try again.`

Cause 5: create/own an AMI, then call DescribeImageAttribute(ImageId=<it>, Attribute="blockDeviceMapping") — observe the error Code `Client.AuthFailure` instead of `AuthFailure`.

Cause 6: resolve an amazon-published AMI id (DescribeImages(Owners=["amazon"])) and call ModifyImageAttribute(ImageId=<that id>, LaunchPermission={"Add":[{"UserId":"111122223333"}]}) — observe AuthFailure with message `Not authorized for images: [<id>]` instead of `Not authorized for image:<id>`.
