Issue-Source: trace-sweep-divergence
Issue-Trace: medical-imaging-dicom-import-job-yields-searchable-image-set
Issue-Step: task#9
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: medical-imaging-dicom-import-job-yields-searchable-image-set
Issue-Shape-Id: com.amazonaws.medicalimaging#AHIGatewayService
Issue-Key: s3|ListObjectsV2|2xx->2xx|$.ListBucketResult.Contents[*]
Issue-Related: 20261111T153432Z-s3-listobjectsv2-reports-keycount-0-and-no-contents.md
Issue-Commit: c035be45a7182c86e523045b713949a5a5fe5fd5

# s3 ListObjectsV2 of the DICOM import job output prefix is missing three of the five objects AWS writes

> _Note: this issue was identified against the code base at commit `c035be45a7182c86e523045b713949a5a5fe5fd5`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListObjectsV2

## Description
Replaying the DICOM-import-to-searchable-image-set fixture, the s3 ListObjectsV2 listing of the import job's output prefix diverges: the prefix holds only the two objects the emulator chose to write, not the full result set AWS leaves behind. ONE cause.

Cause 1 of 1 — the objects a completed HealthImaging DICOM import job leaves under its output prefix are incomplete: the prefix-root write-access probe file and the empty per-category result files are never written.
  AWS: Smithy: ListObjectsV2 (GET /{Bucket}?list-type=2, output shape ListObjectsV2Output serialized as <ListBucketResult>) reports KeyCount as the number of keys returned and one <Contents> entry per object, each carrying Key, LastModified, ETag, Size, StorageClass and (for a CRC64NVME full-object checksum) ChecksumAlgorithm/ChecksumType. ListObjectsV2 itself is a faithful mirror of what is stored, so the requirement here is the SET OF OBJECTS the producer left in the bucket. Smithy (medical-imaging): DICOMImportJobProperties.outputS3Uri — "The output prefix of the S3 bucket to upload the results of the DICOM import job." Recorded behavior only for the layout: after the job reaches COMPLETED, AWS has written FIVE objects under the caller's output prefix — (a) a zero-byte write-access probe object directly at the prefix root, named `.write_access_check_file.temp` (zero-byte, so ETag is the empty-content MD5, Size 0, CRC64NVME/FULL_OBJECT, STANDARD); and, inside the job's own `<datastoreId>-DicomImport-<jobId>/` sub-prefix, (b) `FAILURE/failure.ndjson`, (c) `SUCCESS/success.ndjson`, (d) `WARNING/warning.ndjson` and (e) `job-output-manifest.json`. The FAILURE and WARNING ndjson files exist as ZERO-byte objects when the job produced no failures or warnings — their presence, not their content, is the invariant. The bucket name, the `out/` prefix, the datastore id and the job id in the recording are values taken from the request and from the job's identity; only the file names, the per-category directory layout and the zero-byte-when-empty rule are the requirement.
  Emulator: the same listing reports KeyCount 2 with only the job sub-prefix's `SUCCESS/success.ndjson` and `job-output-manifest.json` present. No `.write_access_check_file.temp` exists at the prefix root, and neither `FAILURE/failure.ndjson` nor `WARNING/warning.ndjson` is created for the categories that came back empty. Status is 200 on both sides, so nothing signals the shortfall: the listing is simply short three objects (expected KeyCount 5, got 2; Contents entries for the probe file and the FAILURE/WARNING ndjson files reported missing).
  Why: a client that drives HealthImaging imports reads the per-category ndjson files unconditionally — the standard pattern is to fetch `<outputPrefix>/<job>/FAILURE/failure.ndjson` and `WARNING/warning.ndjson` after the job settles and treat an empty file as "no failures/warnings". Against the emulator those GETs answer NoSuchKey, so the client cannot distinguish "nothing went wrong" from "results missing" and either faults or silently reports an unvalidated import. Tooling that probes whether the service can write to the destination bucket by looking for the `.write_access_check_file.temp` marker likewise concludes the service never touched the prefix, and any listing-based assertion on the output prefix (object count, per-category presence) disagrees with AWS.
  Fix: the medical-imaging DICOM import job's S3 output writer — the component that materializes a finished job's result objects under outputS3Uri must emit the prefix-root write-access probe object and one ndjson object per result category (SUCCESS/FAILURE/WARNING), zero-byte when that category is empty, alongside the job-output manifest. No change is needed in the s3 ListObjectsV2 handler itself: it reports faithfully what was stored, and this divergence is observed through it.
  Traces: medical-imaging-dicom-import-job-yields-searchable-image-set#task#9.

Related issues, none of which cover this: open 20261109T113806Z-medical-imaging-dicom-import-job-and-image-set.md owns the six medical-imaging clusters in this same fixture and its Cause 2 concerns the VALUE GetDICOMImportJob reports in jobProperties.outputS3Uri, not the objects written under it. Open 20261111T153432Z-s3-listobjectsv2-reports-keycount-0-and-no-contents.md is the same symptom shape for a DMS full-load target prefix (a different producer, and there nothing at all is written). The issue that previously owned this cluster, 20261029T192819Z-s3-listobjectsv2-echoes-the-percent-encoded-prefix-instead.md, is resolved and concerned the echoed Prefix member, not the object set.

## Evidence
Fixture medical-imaging-dicom-import-job-yields-searchable-image-set, 38 steps, 31 passed / 7 diverged (score 81.6%). Six of the seven clusters are medical-imaging and already owned; the task#9 s3 cluster has no owner.

Step task#9, s3 ListObjectsV2 on the job's output prefix, 200 -> 200:
  $.ListBucketResult.KeyCount: expected '5', got '2'.
  $.ListBucketResult.Contents[0]: missing — expected {'Key': 'out/.write_access_check_file.temp', 'LastModified': <shaped-not-asserted>, 'ETag': '"d41d8cd98f00b204e9800998ecf8427e"' (empty-content MD5), 'ChecksumAlgorithm': 'CRC64NVME', 'ChecksumType': 'FULL_OBJECT', 'Size': '0', 'StorageClass': 'STANDARD'}; got the job sub-prefix's SUCCESS/success.ndjson entry instead (Size '80').
  $.ListBucketResult.Contents[1]: missing — expected {'Key': 'out/<datastoreId>-DicomImport-<jobId>/FAILURE/failure.ndjson', 'ETag': '"d41d8cd98f00b204e9800998ecf8427e"', 'Size': '0', 'ChecksumAlgorithm': 'CRC64NVME', 'ChecksumType': 'FULL_OBJECT', 'StorageClass': 'STANDARD'}; got the job-output-manifest.json entry (Size '411').
  $.ListBucketResult.Contents[3]: missing — expected {'Key': 'out/<datastoreId>-DicomImport-<jobId>/WARNING/warning.ndjson', 'ETag': '"d41d8cd98f00b204e9800998ecf8427e"', 'Size': '0', 'ChecksumAlgorithm': 'CRC64NVME', 'ChecksumType': 'FULL_OBJECT', 'StorageClass': 'STANDARD'}; got None.

Net effect: the emulator's output prefix contains SUCCESS/success.ndjson and job-output-manifest.json only; AWS's contains those plus the prefix-root .write_access_check_file.temp and the zero-byte FAILURE/failure.ndjson and WARNING/warning.ndjson. The ids, bucket name, sizes, ETags and timestamps above are this run's values; the requirement is the presence and naming of the three missing objects and the zero-byte-when-empty rule for the per-category ndjson files.

## Steps to Reproduce
1. Boot the emulator from this checkout and replay the wire fixture medical-imaging-dicom-import-job-yields-searchable-image-set (trace set simparity-wire-aws-v1).
2. Create the datastore and submit StartDICOMImportJob with inputS3Uri and outputS3Uri pointing at the `out/` prefix of the probe bucket; wait for GetDICOMImportJob to report the job COMPLETED (fixture steps task#7/task#8).
3. Step task#9: call s3 ListObjectsV2 on that bucket with Prefix=`out/` and read KeyCount and the Contents keys.
4. Observe KeyCount 2, with only `<job>/SUCCESS/success.ndjson` and `<job>/job-output-manifest.json`. AWS reports KeyCount 5, additionally listing `out/.write_access_check_file.temp` (zero-byte, at the prefix root) and the zero-byte `<job>/FAILURE/failure.ndjson` and `<job>/WARNING/warning.ndjson`.
5. Confirm the consequence directly: GetObject on `<outputPrefix>/<datastoreId>-DicomImport-<jobId>/FAILURE/failure.ndjson` and on `.../WARNING/warning.ndjson` answers NoSuchKey against the emulator, where AWS serves a zero-byte object.
