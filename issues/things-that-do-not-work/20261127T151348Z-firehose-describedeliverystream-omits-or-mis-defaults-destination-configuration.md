Issue-Source: trace-sweep-divergence
Issue-Trace: firehose-create-delivery-stream-dynamic-partitioning-buffer-size-floor
Issue-Step: task#6
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: firehose-create-delivery-stream-dynamic-partitioning-buffer-size-floor firehose-create-delivery-stream-dynamic-partitioning-zero-buffering-interval firehose-data-format-conversion-buffering-default firehose-describe-delivery-stream-dynamic-partitioning-default-buffering firehose-http-endpoint-destination-describe-reports-every-default
Issue-Shape-Id: com.amazonaws.firehose#Firehose_20150804
Issue-Key: firehose|DescribeDeliveryStream|2xx->2xx|$.DeliveryStreamDescription.Destinations[*].ExtendedS3DestinationDescription.DynamicPartitioningConfiguration.RetryOptions
Issue-Key: firehose|DescribeDeliveryStream|2xx->2xx|$.DeliveryStreamDescription.Destinations[*].ExtendedS3DestinationDescription.BufferingHints.SizeInMBs
Issue-Key: firehose|DescribeDeliveryStream|2xx->2xx|$.DeliveryStreamDescription.Destinations[*].S3DestinationDescription.BufferingHints.SizeInMBs
Issue-Key: firehose|DescribeDeliveryStream|2xx->2xx|$.DeliveryStreamDescription.Destinations[*].HttpEndpointDestinationDescription.RequestConfiguration
Issue-Key: firehose|DescribeDeliveryStream|2xx->2xx|$.DeliveryStreamDescription.Destinations[*].HttpEndpointDestinationDescription.RetryOptions
Issue-Key: firehose|DescribeDeliveryStream|2xx->2xx|$.DeliveryStreamDescription.Destinations[*].HttpEndpointDestinationDescription.S3BackupMode
Issue-Commit: f9b7321d6a22920f7cbed43b33d059dedecbc1ee

# firehose DescribeDeliveryStream omits or mis-defaults destination configuration AWS materializes

> _Note: this issue was identified against the code base at commit `f9b7321d6a22920f7cbed43b33d059dedecbc1ee`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeDeliveryStream

## Description
Four net-new root causes with one shared fault: AWS Firehose MATERIALIZES the optional sub-structures and buffering defaults of a destination configuration at CreateDeliveryStream time and then reports the materialized values back on DescribeDeliveryStream, whereas this emulator stores only what the request literally carried, so the description it serves is missing whole sub-structures and carries the plain 5 MiB buffering default where AWS applies 128. firehose is bootstrapped (get_bootstrap_status reports service-com-amazonaws-firehose-firehose-20150804), so these are fidelity findings, not bootstrap gaps. All four causes are DescribeDeliveryStream 200 vs 200 body divergences — status and error classification are not involved. None is covered by an open issue: 20261124T080946Z-firehose-iceberg-kfi-create-and-update-validation-diverges.md owns create/update VALIDATION of Iceberg/KFI destinations, not the description of an accepted stream.

Cause 1 of 4: ExtendedS3DestinationDescription.DynamicPartitioningConfiguration is reported without its RetryOptions sub-structure.
AWS: Smithy: ExtendedS3DestinationDescription.DynamicPartitioningConfiguration targets com.amazonaws.firehose#DynamicPartitioningConfiguration, whose members are RetryOptions (-> com.amazonaws.firehose#RetryOptions, doc "The retry behavior in case Firehose is unable to deliver data to an Amazon S3 prefix") and Enabled; RetryOptions.DurationInSeconds is an integer with @range min 0 max 7200. The model states no default, so the default is recorded behavior only: when a stream is created with DynamicPartitioningConfiguration and the request omits RetryOptions, AWS's DescribeDeliveryStream still reports DynamicPartitioningConfiguration.RetryOptions with DurationInSeconds set to the service default of 300.
Emulator: DescribeDeliveryStream returns DynamicPartitioningConfiguration with Enabled present but RetryOptions entirely absent (null/omitted).
Why: a client that reads the retry window off a description — to display it, to copy a description into an UpdateDestination request, or to assert a round trip — sees a missing key where AWS always supplies a structure, and any code dereferencing RetryOptions.DurationInSeconds faults instead of reading 300.
Fix: the ExtendedS3 destination's state/description layer in the firehose service — the CreateDeliveryStream handler's normalization of DynamicPartitioningConfiguration (or the DescribeDeliveryStream serializer that projects it) must materialize RetryOptions with the default DurationInSeconds when the request omits it.
Traces: firehose-create-delivery-stream-dynamic-partitioning-buffer-size-floor#task#6 and firehose-create-delivery-stream-dynamic-partitioning-zero-buffering-interval#task#6 (same cluster), and the same omission inside Cause 2's and Cause 3's steps.

Cause 2 of 4: with DataFormatConversionConfiguration enabled, BufferingHints.SizeInMBs is reported as 5 instead of the 128 AWS applies.
AWS: Smithy: BufferingHints.SizeInMBs is an integer with @range min 1 max 128, documented "Buffer incoming data to the specified size, in MiBs ... The default value is 5"; ExtendedS3DestinationDescription.BufferingHints is @required. The 5 MiB figure is the PLAIN-destination default; for a stream whose ExtendedS3 destination enables DataFormatConversionConfiguration (JSON -> Parquet/ORC), the recorded behavior is that AWS applies a buffering size of 128 MiB when the request specifies none, and reports it on BOTH DeliveryStreamDescription.Destinations[0].ExtendedS3DestinationDescription.BufferingHints.SizeInMBs and the companion S3DestinationDescription.BufferingHints.SizeInMBs.
Emulator: both paths report SizeInMBs 5 — the generic default is applied with no regard for the conversion configuration.
Why: the buffering size is the contract for how large delivered objects get and how long records are held; a client that configures format conversion and then reads back a 5 MiB buffer sizes its downstream expectations (object count, latency, Glue partition volume) off a value AWS never serves, and a configuration round trip silently rewrites the buffer 25x smaller.
Fix: the CreateDeliveryStream handler's ExtendedS3 buffering-hint defaulting must select the format-conversion default rather than the plain default when DataFormatConversionConfiguration is enabled, and the description must project the same value to the mirrored S3DestinationDescription.
Traces: firehose-data-format-conversion-buffering-default#task#7.

Cause 3 of 4: with Dynamic Partitioning enabled, BufferingHints.SizeInMBs is likewise reported as 5 instead of 128 (and RetryOptions is missing as in Cause 1).
AWS: Smithy: as Cause 2 for BufferingHints (@range 1..128, plain default 5) and as Cause 1 for DynamicPartitioningConfiguration.RetryOptions. Recorded behavior: a stream created with DynamicPartitioningConfiguration enabled and no explicit BufferingHints is described with ExtendedS3DestinationDescription.BufferingHints.SizeInMBs 128 and S3DestinationDescription.BufferingHints.SizeInMBs 128, plus DynamicPartitioningConfiguration.RetryOptions.DurationInSeconds 300.
Emulator: SizeInMBs is 5 on both descriptions and DynamicPartitioningConfiguration.RetryOptions is absent.
Why: the same wrong-buffer consequence as Cause 2, and it is the case AWS itself constrains hardest (dynamic partitioning carries a buffering-size floor), so a client reading 5 back believes it holds a configuration AWS would refuse.
Fix: same component as Cause 2 — the buffering default selection must account for dynamic partitioning as well as format conversion; RetryOptions materialization is Cause 1's fix.
Traces: firehose-describe-delivery-stream-dynamic-partitioning-default-buffering#task#5.

Cause 4 of 4: HttpEndpointDestinationDescription omits every default AWS materializes — RequestConfiguration, RetryOptions and S3BackupMode.
AWS: Smithy: com.amazonaws.firehose#HttpEndpointDestinationDescription has members RequestConfiguration (-> HttpEndpointRequestConfiguration, whose members are ContentEncoding, an enum, and CommonAttributes, a list with @length min 0 max 50), RetryOptions (-> HttpEndpointRetryOptions) and S3BackupMode (-> HttpEndpointS3BackupMode, documented "You can back up all documents (AllData) or only the documents that Firehose could not deliver to the specified HTTP endpoint destination (FailedDataOnly)"). The model fixes no defaults, so these are recorded behavior only: when an HttpEndpointDestinationConfiguration omits them, AWS's description still reports RequestConfiguration with ContentEncoding NONE and an empty CommonAttributes list, RetryOptions with DurationInSeconds 300, and S3BackupMode FailedDataOnly.
Emulator: all three members are absent from HttpEndpointDestinationDescription.
Why: the three omitted members are exactly the ones a client inspects to learn how the endpoint will be called (encoding, headers), how failures are retried, and what is backed up to S3; against the emulator each reads as unset, so a client cannot distinguish "defaulted" from "unsupported", and feeding the description back into UpdateDestination drops the defaults AWS had applied.
Fix: the HTTP-endpoint destination's description projection in the firehose service (the CreateDeliveryStream handler's normalization of HttpEndpointDestinationConfiguration, or the DescribeDeliveryStream serializer for that destination variant) must materialize these defaults.
Traces: firehose-http-endpoint-destination-describe-reports-every-default#task#10.

## Evidence
Replayed against the emulator booted from this checkout; all four are verdict=mismatch with expected_status 200 and actual_status 200 (body-only divergence).

Cause 1 — fixture firehose-create-delivery-stream-dynamic-partitioning-buffer-size-floor, step task#6, operation DescribeDeliveryStream: "$.DeliveryStreamDescription.Destinations[0].ExtendedS3DestinationDescription.DynamicPartitioningConfiguration.RetryOptions: missing (expected {'DurationInSeconds': 300}, got None)". Cluster also contains firehose-create-delivery-stream-dynamic-partitioning-zero-buffering-interval#task#6 (affected_count 2).

Cause 2 — fixture firehose-data-format-conversion-buffering-default, step task#7, operation DescribeDeliveryStream: "$.DeliveryStreamDescription.Destinations[0].ExtendedS3DestinationDescription.BufferingHints.SizeInMBs: value_mismatch (expected 128, got 5)" and "$.DeliveryStreamDescription.Destinations[0].S3DestinationDescription.BufferingHints.SizeInMBs: value_mismatch (expected 128, got 5)".

Cause 3 — fixture firehose-describe-delivery-stream-dynamic-partitioning-default-buffering, step task#5, operation DescribeDeliveryStream: SizeInMBs value_mismatch (expected 128, got 5) on both ExtendedS3DestinationDescription and S3DestinationDescription, plus "DynamicPartitioningConfiguration.RetryOptions: missing (expected {'DurationInSeconds': 300}, got None)".

Cause 4 — fixture firehose-http-endpoint-destination-describe-reports-every-default, step task#10, operation DescribeDeliveryStream: "HttpEndpointDestinationDescription.RequestConfiguration: missing (expected {'CommonAttributes': [], 'ContentEncoding': 'NONE'}, got None)", "HttpEndpointDestinationDescription.RetryOptions: missing (expected {'DurationInSeconds': 300}, got None)", "HttpEndpointDestinationDescription.S3BackupMode: missing (expected 'FailedDataOnly', got None)".

The delivery-stream names, ARNs, role ARNs and timestamps in these fixtures are per-run values and are not part of the requirement; the requirement is that the named members are present and carry the service defaults (300 s retry duration, NONE/empty request configuration, FailedDataOnly backup mode, and the 128 MiB buffering size in the format-conversion and dynamic-partitioning cases).

## Steps to Reproduce
1. Build and boot the emulator from this checkout and point a firehose client at its data listener.
2. Cause 1: CreateDeliveryStream with DeliveryStreamType=DirectPut and an ExtendedS3DestinationConfiguration that sets DynamicPartitioningConfiguration={"Enabled": true} (with the required processing configuration) and NO RetryOptions, then DescribeDeliveryStream — ExtendedS3DestinationDescription.DynamicPartitioningConfiguration has no RetryOptions member, where AWS reports {"DurationInSeconds": 300}.
3. Cause 2: CreateDeliveryStream with an ExtendedS3DestinationConfiguration carrying an enabled DataFormatConversionConfiguration and NO BufferingHints, then DescribeDeliveryStream — BufferingHints.SizeInMBs reads 5 on both ExtendedS3DestinationDescription and S3DestinationDescription, where AWS reports 128.
4. Cause 3: CreateDeliveryStream with DynamicPartitioningConfiguration enabled and NO BufferingHints, then DescribeDeliveryStream — SizeInMBs reads 5 instead of 128 and RetryOptions is missing.
5. Cause 4: CreateDeliveryStream with an HttpEndpointDestinationConfiguration that omits RequestConfiguration, RetryOptions and S3BackupMode, then DescribeDeliveryStream — all three members are absent from HttpEndpointDestinationDescription instead of being reported as ContentEncoding NONE / empty CommonAttributes, DurationInSeconds 300 and FailedDataOnly.
6. Or replay the wire corpus and re-run the fixture steps listed in Issue-Trace/Issue-Step and Issue-Trace-Ids.
