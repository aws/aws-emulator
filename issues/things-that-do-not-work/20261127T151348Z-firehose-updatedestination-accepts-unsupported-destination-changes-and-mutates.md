Issue-Source: trace-sweep-divergence
Issue-Trace: firehose-update-destination-disable-dynamic-partitioning
Issue-Step: task#6
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: firehose-update-destination-disable-dynamic-partitioning firehose-update-destination-enable-dynamic-partitioning-on-existing-stream firehose-update-destination-rejects-disabling-enabled-s3-backup
Issue-Shape-Id: com.amazonaws.firehose#Firehose_20150804
Issue-Key: firehose|UpdateDestination|4xx->2xx|$
Issue-Key: firehose|UpdateDestination|4xx->2xx|status
Issue-Key: firehose|DescribeDeliveryStream|2xx->2xx|$.DeliveryStreamDescription.Destinations[*].ExtendedS3DestinationDescription.DynamicPartitioningConfiguration.Enabled
Issue-Key: firehose|DescribeDeliveryStream|2xx->2xx|$.DeliveryStreamDescription.VersionId
Issue-Key: firehose|DescribeDeliveryStream|2xx->2xx|$.DeliveryStreamDescription.Destinations[*].ExtendedS3DestinationDescription.S3BackupMode
Issue-Related: 20261127T151348Z-firehose-describedeliverystream-omits-or-mis-defaults-destination-configuration.md
Issue-Commit: f9b7321d6a22920f7cbed43b33d059dedecbc1ee

# firehose UpdateDestination accepts unsupported destination changes and mutates the stream instead of refusing

> _Note: this issue was identified against the code base at commit `f9b7321d6a22920f7cbed43b33d059dedecbc1ee`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
UpdateDestination

## Description
Four net-new root causes with one shared fault: AWS Firehose REFUSES the destination updates that would toggle Dynamic Partitioning or turn an already-enabled S3 backup off, and such a refused UpdateDestination leaves the stream's configuration and its VersionId untouched. This emulator accepts those updates with 200 and an empty body, applies them to its state, and bumps DeliveryStreamDescription.VersionId, so the subsequent DescribeDeliveryStream diverges too. firehose is bootstrapped (get_bootstrap_status reports service-com-amazonaws-firehose-firehose-20150804), so these are fidelity findings, not bootstrap gaps. Not covered by an open issue: 20261124T080946Z-firehose-iceberg-kfi-create-and-update-validation-diverges.md owns UpdateDestination validation of ICEBERG/KFI destination tables (and the replay report marks those steps OWNED); these four concern ExtendedS3 dynamic partitioning and S3 backup and were reported unowned.

Cause 1 of 4: UpdateDestination accepts an ExtendedS3DestinationUpdate that enables or disables Dynamic Partitioning.
AWS: Smithy: UpdateDestination models InvalidArgumentException (errorKind client, HTTP 400, documented "The specified input parameter has a value that is not valid") alongside ConcurrentModificationException, ResourceInUseException and ResourceNotFoundException; its input requires DeliveryStreamName, CurrentDeliveryStreamVersionId and DestinationId. The message text is recorded behavior only: an UpdateDestination whose ExtendedS3DestinationUpdate changes DynamicPartitioningConfiguration.Enabled relative to the live configuration — in either direction — is refused with HTTP 400, __type InvalidArgumentException and the ENTIRELY FIXED message "Enabling or disabling Dynamic Partitioning is not supported at the moment" (no variable part: no stream name, version id or member name is interpolated).
Emulator: the same call returns HTTP 200 with an empty JSON body ({}), i.e. UpdateDestination's modeled empty output, and the toggle is applied.
Why: dynamic partitioning determines how delivered objects are keyed, so a client that AWS stops at the API boundary gets a silently reconfigured stream against the emulator; a caller written to catch InvalidArgumentException and fall back to recreating the stream never takes that branch, and a test asserting the refusal passes against AWS and fails only in production.
Fix: the UpdateDestination handler's ExtendedS3 validation in the firehose service must compare the requested DynamicPartitioningConfiguration.Enabled against the stored destination and raise InvalidArgumentException with that fixed message before any state mutation.
Traces: firehose-update-destination-disable-dynamic-partitioning#task#6, firehose-update-destination-enable-dynamic-partitioning-on-existing-stream#task#6 and firehose-update-destination-rejects-disabling-enabled-s3-backup#task#7 (one cluster, affected_count 3; the quoted body is the representative's — see Cause 4 for the S3-backup member).

Cause 2 of 4: after the update AWS refuses, the stream's DynamicPartitioningConfiguration.Enabled has flipped to false and VersionId has advanced.
AWS: Smithy: DescribeDeliveryStream returns DeliveryStreamDescription (@required), which carries VersionId and the Destinations list. Recorded behavior: because the disable-dynamic-partitioning UpdateDestination is refused (Cause 1), the following DescribeDeliveryStream still reports ExtendedS3DestinationDescription.DynamicPartitioningConfiguration.Enabled true, its RetryOptions {DurationInSeconds: 300}, and DeliveryStreamDescription.VersionId "1" — a refused update neither applies nor consumes a version.
Emulator: the description reports Enabled false, omits DynamicPartitioningConfiguration.RetryOptions, and reports VersionId "2".
Why: the VersionId is the optimistic-concurrency token a client must pass as CurrentDeliveryStreamVersionId on the NEXT UpdateDestination. Bumping it on a call AWS rejects desynchronizes every client that cached the version from a prior describe: its next legitimate update fails the version check, and its view of whether dynamic partitioning is on is simply wrong.
Fix: the UpdateDestination handler must reject before mutating state, so neither the destination configuration nor the version counter in the delivery-stream state is touched on a refusal. (The missing RetryOptions member in the same response body is the materialization defect filed separately under the DescribeDeliveryStream defaults issue.)
Traces: firehose-update-destination-disable-dynamic-partitioning#task#7.

Cause 3 of 4: a refused enable-Dynamic-Partitioning update likewise advances VersionId.
AWS: Smithy: as Cause 2 (DeliveryStreamDescription.VersionId). Recorded behavior: after the refused UpdateDestination that tries to turn dynamic partitioning ON for an existing ExtendedS3 stream, DescribeDeliveryStream reports VersionId "1" — the version a successful update would have produced is never issued.
Emulator: VersionId reads "2".
Why: same concurrency-token corruption as Cause 2, in the opposite toggle direction, which shows the version bump is unconditional rather than tied to one code path.
Fix: same component as Cause 2 — version increment must be the consequence of an APPLIED update only.
Traces: firehose-update-destination-enable-dynamic-partitioning-on-existing-stream#task#7.

Cause 4 of 4: UpdateDestination accepts disabling an already-enabled S3 backup, and the stream's S3BackupMode and VersionId change.
AWS: Smithy: ExtendedS3DestinationDescription.S3BackupMode targets the S3BackupMode enum, and UpdateDestination models InvalidArgumentException (client, HTTP 400). Recorded behavior: an UpdateDestination that sets S3BackupMode to Disabled on a stream whose S3 backup is already Enabled is refused with HTTP 400 (the step records the refusal status; the body is not asserted on that step, and InvalidArgumentException is the modeled input-validity error of the operation), and the following DescribeDeliveryStream still reports ExtendedS3DestinationDescription.S3BackupMode "Enabled" with DeliveryStreamDescription.VersionId "1".
Emulator: the update returns 200 with an empty body and the description then reports S3BackupMode "Disabled" and VersionId "2".
Why: S3 backup is the durability guarantee for records that cannot be delivered; AWS makes it one-way precisely so it cannot be turned off under a running stream. An emulator that honours the request lets a client believe a configuration AWS would never let it reach, and the same stale-version problem as Causes 2-3 follows.
Fix: the UpdateDestination handler's ExtendedS3 validation must refuse a transition of S3BackupMode from Enabled to Disabled with InvalidArgumentException, before mutating state or the version counter.
Traces: firehose-update-destination-rejects-disabling-enabled-s3-backup#task#8 (the describe) and its UpdateDestination step #task#7, which is the third member of Cause 1's cluster.

## Evidence
Replayed against the emulator booted from this checkout; all four are verdict=mismatch.

Cause 1 — fixture firehose-update-destination-disable-dynamic-partitioning, step task#6, operation UpdateDestination: "status: expected 400, got 200" and "$: empty_body (expected {'__type': 'InvalidArgumentException', 'message': 'Enabling or disabling Dynamic Partitioning is not supported at the moment'}, got {})". divergence_key service=firehose|UpdateDestination|status=400->200|keys=$,status; affected_count 3: firehose-update-destination-disable-dynamic-partitioning#task#6, firehose-update-destination-enable-dynamic-partitioning-on-existing-stream#task#6, firehose-update-destination-rejects-disabling-enabled-s3-backup#task#7. Required wire shape: HTTP 400 with __type "InvalidArgumentException" and the message text above verbatim — it has no variable part.

Cause 2 — fixture firehose-update-destination-disable-dynamic-partitioning, step task#7, operation DescribeDeliveryStream, expected 200 / actual 200: "...ExtendedS3DestinationDescription.DynamicPartitioningConfiguration.Enabled: value_mismatch (expected True, got False)"; "...DynamicPartitioningConfiguration.RetryOptions: missing (expected {'DurationInSeconds': 300}, got None)"; "$.DeliveryStreamDescription.VersionId: value_mismatch (expected '1', got '2')".

Cause 3 — fixture firehose-update-destination-enable-dynamic-partitioning-on-existing-stream, step task#7, operation DescribeDeliveryStream, expected 200 / actual 200: "$.DeliveryStreamDescription.VersionId: value_mismatch (expected '1', got '2')".

Cause 4 — fixture firehose-update-destination-rejects-disabling-enabled-s3-backup, step task#8, operation DescribeDeliveryStream, expected 200 / actual 200: "...ExtendedS3DestinationDescription.S3BackupMode: value_mismatch (expected 'Enabled', got 'Disabled')" and "$.DeliveryStreamDescription.VersionId: value_mismatch (expected '1', got '2')"; its UpdateDestination step task#7 is the 400->200 member of Cause 1's cluster.

The version strings "1"/"2" are not themselves the requirement: the requirement is that a REFUSED UpdateDestination does not advance the stream's VersionId and does not alter the destination configuration. Stream names, ARNs and timestamps in these fixtures are per-run values.

## Steps to Reproduce
1. Build and boot the emulator from this checkout and point a firehose client at its data listener.
2. CreateDeliveryStream with DeliveryStreamType=DirectPut and an ExtendedS3DestinationConfiguration whose DynamicPartitioningConfiguration is {"Enabled": true} (with the required processing configuration); DescribeDeliveryStream and note VersionId "1" and the DestinationId.
3. Cause 1 + Cause 2: call UpdateDestination with that DeliveryStreamName, CurrentDeliveryStreamVersionId="1", the DestinationId, and an ExtendedS3DestinationUpdate setting DynamicPartitioningConfiguration={"Enabled": false} — the emulator answers 200 with {} instead of 400 InvalidArgumentException "Enabling or disabling Dynamic Partitioning is not supported at the moment". Re-describe: DynamicPartitioningConfiguration.Enabled is now false and VersionId is "2".
4. Cause 3: create an ExtendedS3 stream with dynamic partitioning OFF and call UpdateDestination with an ExtendedS3DestinationUpdate setting DynamicPartitioningConfiguration={"Enabled": true} — again accepted with 200; the following describe shows VersionId "2" instead of "1".
5. Cause 4: create an ExtendedS3 stream with S3BackupMode=Enabled (and an S3BackupConfiguration), then call UpdateDestination with an ExtendedS3DestinationUpdate setting S3BackupMode=Disabled — accepted with 200 instead of refused with 400; the following describe reports S3BackupMode "Disabled" and VersionId "2" instead of "Enabled" and "1".
6. Or replay the wire corpus and re-run the fixture steps listed in Issue-Trace/Issue-Step and Issue-Trace-Ids.
