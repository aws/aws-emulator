Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: firehose__iam__opensearch__s3.xsvc-opensearch-1-firehose-fgac-strict-mapping-indexes-only-conforming-documents
Issue-Key: firehose__iam__opensearch__s3.xsvc-opensearch-1-firehose-fgac-strict-mapping-indexes-only-conforming-documents|_read_to_eof#18|Timeout|return b"".join(parts)
Issue-Commit: e939f3162fad9baaa59a21c0fa7179928b05c50a

# firehose__iam__opensearch__s3: the Firehose→FGAC-OpenSearch strict-mapping delivery scenario still makes no observable progress and is cut off at the 60 s harness cap

> _Note: this issue was identified against the code base at commit `e939f3162fad9baaa59a21c0fa7179928b05c50a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutRecordBatch

## Description
Cause 1 of 1: the Firehose-to-OpenSearch delivery path makes no observable progress — the check reaches no assertion at all and is cut off by the suite's cap, so neither half of the strict-mapping delivery contract (conforming documents indexed on the domain, violating documents diverted to the S3 backup prefix) can be graded.

Note on existing issues: this check id is ALREADY named in the Issue-Trace-Ids of open issue 20261111T000041Z-firehose-iam-opensearch-s3-the-firehose-into-fgac.md, which describes exactly this hang, but the replay no longer matches that issue's recorded replay key (it stored `_wait_admin#18|Timeout|...`, while this replay produces a bare `Check timed out after 60.0s` with no assertion frame at all), so the suite did not report the check as owned. Treat this as a re-report of that cause against the current commit rather than a second defect; fixing it closes both. Sibling checks in the same section are owned by 20261111T103332Z-firehose-iam-opensearch-s3-the-opensearch-domain-endpoint.md and 20261111T232646Z-firehose-iam-opensearch-s3-the-opensearch-domain-endpoint-2ec57a.md, which report the unrouted `es` domain endpoint — a plausible upstream of this hang, but neither carries this check id.

AWS: recorded behavior only for the end-to-end timing; the behaviour asserted is the documented Firehose OpenSearch-destination contract. A delivery stream with an AmazonopensearchserviceDestinationConfiguration (zero-second BufferingHints, no RetryOptions, S3BackupMode=FailedDocumentsOnly) reaches DeliveryStreamStatus=ACTIVE; PutRecordBatch of a batch of documents answers FailedPutCount 0; documents conforming to a `dynamic: strict` index template appear in the date-rotated index on the domain (visible through `GET /_cat/indices?format=json` and `/<index>/_count`); documents violating that mapping are NOT indexed and are written as objects under the S3 backup bucket's error prefix; and DescribeDeliveryStream echoes the configured buffering, retry and backup settings. Each of those steps answers within seconds once the stream is ACTIVE.

Emulator: the check never reaches any assertion — the replay reports only `Error: Check timed out after 60.0s`. Its only unbounded waits are the domain publishing an endpoint and admitting the assumed FGAC admin role on `/_cluster/health`, DescribeDeliveryStream reporting ACTIVE, and the delivered documents becoming visible on the domain; one of those conditions never becomes true, so every poll loop runs to the cap. Hypothesis: the OpenSearch destination has no delivery attempt behind it (the same inert-destination class already filed for the Splunk, HTTP-endpoint and Iceberg destinations), and/or the domain's data-plane host is unrouted, so the index never appears and the poll cannot terminate.

Why: a client building the standard ingestion pipeline (producer → Firehose → OpenSearch index, mapping failures diverted to S3) sees an empty domain and an empty backup prefix with no error anywhere to explain it. Both halves of the contract are unobservable: the client cannot tell 'Firehose delivered nothing' from 'the domain is not answering', and a mapping-rejected record is lost rather than backed up.

Fix: the Firehose delivery engine in the firehose service crate — the AmazonopensearchserviceDestinationConfiguration destination needs a real delivery attempt against the target domain's data plane, with mapping-rejected documents routed to the configured S3 backup prefix; it depends on the OpenSearch domain endpoint being routed for signing name `es` so that delivery and the check's own data-plane polling can both resolve.

Traces: firehose__iam__opensearch__s3.xsvc-opensearch-1-firehose-fgac-strict-mapping-indexes-only-conforming-documents.

## Evidence
Behavioral suite replay (simparity-behavioral-aws-v1), `### firehose__iam__opensearch__s3` section (0/3 passed).

[mismatch] firehose__iam__opensearch__s3.xsvc-opensearch-1-firehose-fgac-strict-mapping-indexes-only-conforming-documents
  Error: Check timed out after 60.0s

No assertion frame is reported at all, so no step of the scenario completed. Expected: after PutRecordBatch of the batch (conforming + template-violating documents) with FailedPutCount 0, the domain holds exactly one date-suffixed index named <prefix>-<date> whose _count equals the number of conforming documents, the violating documents appear as objects under the S3 backup error prefix, and DescribeDeliveryStream echoes BufferingHints 0 / RetryOptions 0 / S3BackupMode=FailedDocumentsOnly. Actual: nothing is indexed, nothing is backed up, and the check is cut off at the cap. The two sibling checks in this section fail with the router's 404 {"__type":"UnknownService","message":"no emulated service is registered for 'es'"} on the domain endpoint, which is consistent with this check's unbounded data-plane poll never terminating.

## Steps to Reproduce
1. git_clone the repo; build and boot the emulator from the checkout.
2. Create an IAM admin role, an FGAC OpenSearch domain that maps that role as master, an S3 backup bucket and a Firehose delivery role.
3. CreateDeliveryStream with an AmazonopensearchserviceDestinationConfiguration (IndexName <prefix>, IndexRotationPeriod OneDay, BufferingHints 0, RetryOptions 0, S3BackupMode=FailedDocumentsOnly) and poll DescribeDeliveryStream for DeliveryStreamStatus=ACTIVE.
4. As the admin role, PUT a `dynamic: strict` index template on the domain over its published endpoint (SigV4, signing name `es`).
5. PutRecordBatch a mix of conforming and template-violating documents; FailedPutCount must be 0.
6. Poll GET /_cat/indices?format=json and /<index>/_count on the domain, and ListObjectsV2 on the S3 backup error prefix. AWS shows the conforming documents indexed and the violating ones backed up; the emulator shows neither and the scenario never terminates.
7. Replay firehose__iam__opensearch__s3.xsvc-opensearch-1-firehose-fgac-strict-mapping-indexes-only-conforming-documents; it times out at the 60 s harness cap.
