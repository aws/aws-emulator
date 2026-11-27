Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: transcribe.start-call-analytics-job-reused-name-rejected transcribe.transcription-job-output-key-slash-decides-object-name
Issue-Shape-Id: com.amazonaws.transcribe#Transcribe
Issue-Key: transcribe.start-call-analytics-job-reused-name-rejected|run#22|AssertionError|assert expected_key in held, f"no {expected_key!r} in the output folder; it holds {held!r}"
Issue-Key: transcribe.transcription-job-output-key-slash-decides-object-name|run#15|AssertionError|assert named == key, f"TranscriptFileUri names key {named!r}, expected {key!r}"
Issue-Commit: 7f2f9e8fbacb5ccb6dc94ed688d25f78f8760a08

# transcribe: finished jobs write their transcript at the wrong S3 object key — the Call Analytics 'analytics/' folder is missing, and a slash-less OutputKey is treated as a folder

> _Note: this issue was identified against the code base at commit `7f2f9e8fbacb5ccb6dc94ed688d25f78f8760a08`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
StartTranscriptionJob, StartCallAnalyticsJob

## Description
Two independent root causes in how the Amazon Transcribe emulator resolves the S3 object key a finished job's output document is written to and reported at. Both leave a client that fetches the transcript at the location AWS puts it with a 404, and both are in the transcribe service only.

Cause 1 of 2: a completed Call Analytics job omits the 'analytics/' folder AWS writes its output under.
  Failing check: transcribe.start-call-analytics-job-reused-name-rejected
  AWS: Smithy StartCallAnalyticsJobRequest$OutputLocation documents the accepted forms 's3://BUCKET', 's3://BUCKET/my-output-folder/' and 's3://BUCKET/my-output-folder/my-call-analytics-job.json', and states "Unless you specify a file name (option 3), the name of your output file has a default value that matches the name you specified for your transcription job using the CallAnalyticsJobName parameter." Recorded behavior (real AWS) for the folder form: when OutputLocation names a folder (a URI ending in '/'), the Call Analytics output document is written to '<folder>/analytics/<CallAnalyticsJobName>.json' — the family inserts its own 'analytics/' path segment under the caller's folder — and Transcript.TranscriptFileUri names that object. The template is '<folder>/analytics/<CallAnalyticsJobName>.json': the fixed parts are the literal 'analytics/' segment and the '.json' suffix; the folder comes from the request's OutputLocation and the file name from the request's CallAnalyticsJobName.
  Emulator: the document is written one level up, directly in the caller's folder as '<folder>/<CallAnalyticsJobName>.json' — the 'analytics/' segment is never produced. The replay listed the output folder and found only {'analytics-a/<job>.json'} where 'analytics-a/analytics/<job>.json' was expected.
  Why: a client that follows AWS's documented Call Analytics layout — or that lists '<folder>/analytics/' to collect per-job results — finds nothing there; GetObject on the real key 404s, and any consumer keyed on the 'analytics/' prefix (a second job family writing to the same folder, a prefix-triggered notification, a lifecycle rule) silently processes zero objects.
  Fix: the Call Analytics job family's output-location resolution in the transcribe service's state — the helper that builds the completed record's Transcript.TranscriptFileUri and the key the output document is stored under (store_job_outputs). The reported URI and the stored object must both carry the 'analytics/' segment, so they cannot drift.

Cause 2 of 2: StartTranscriptionJob treats an OutputKey without a trailing slash as a folder instead of as the exact object key.
  Failing check: transcribe.transcription-job-output-key-slash-decides-object-name
  AWS: recorded behavior (real AWS) — the trailing slash on OutputKey decides the object name. An OutputKey with NO trailing slash is the transcript's exact object key (it is used verbatim, with no '<job name>.json' appended and no such object created); an OutputKey WITH a trailing slash is a folder, and the object is '<OutputKey><TranscriptionJobName>.json'. Transcript.TranscriptFileUri names the object that was actually written, and HeadObject on the other candidate answers 404. Smithy StartTranscriptionJobRequest$OutputKey documents the same pairing with OutputBucketName ("Use in combination with OutputBucketName to specify the output location of your transcript and, optionally, a unique name for your output file", with 'test-files/' given as the folder form); the recording is what decides which of a key's two readings applies.
  Emulator: '<job name>.json' is appended unconditionally, so a slash-less OutputKey is turned into a folder. For OutputKey 'reports/daily' the job reported TranscriptFileUri naming key 'reports/daily/<job>.json' and wrote the document there; the exact key 'reports/daily' holds nothing.
  Why: a caller that asks for one named output object (the whole point of OutputKey) gets no object at the key it chose, and a GetObject on that key 404s. Pipelines that pre-register the output key, or that overwrite a fixed key per run, break; the transcript appears at an unexpected, job-name-dependent key instead.
  Fix: the OutputKey → object-key resolution in StartTranscriptionJob's output-location handling / the transcription-job family's state, used both for the reported Transcript.TranscriptFileUri and for the key the output document is written to: append '<TranscriptionJobName>.json' only when OutputKey ends in '/', and use OutputKey verbatim otherwise.

Traces: see Evidence; the two causes are grounded in the two failing checks named above (both under the suite's 'transcribe' heading). Neither cause is covered by an open issue: the open transcribe/s3 transcript issues (20261118T015953Z-transcribe-a-completed-transcription-job-serves-a-transcript-document.md, 20261112T121939Z-s3-serves-the-transcribe-transcript-document-with-an.md, 20261125T014745Z-s3-getobject-serves-the-transcribe-transcript-document-with.md) are about the CONTENT of the transcript document, not the object key it is stored at and reported under.

## Evidence
Cause 1 (transcribe.start-call-analytics-job-reused-name-rejected), after the job reached COMPLETED:
  AssertionError: no 'analytics-a/analytics/sim2real-caname-54dbc5beef27.json' in the output folder; it holds {'analytics-a/sim2real-caname-54dbc5beef27.json'}
  Expected (AWS): ListObjectsV2(Prefix='<folder>/') contains '<folder>/analytics/<CallAnalyticsJobName>.json'.
  Actual (emulator): only '<folder>/<CallAnalyticsJobName>.json' exists; nothing under '<folder>/analytics/'.
  (The names/ids in the quoted message are per-run values from the request; the requirement is the key template '<folder>/analytics/<CallAnalyticsJobName>.json'.)

Cause 2 (transcribe.transcription-job-output-key-slash-decides-object-name), StartTranscriptionJob with OutputBucketName=<bucket>, OutputKey='reports/daily' (no trailing slash), polled to COMPLETED:
  AssertionError: TranscriptFileUri names key 'reports/daily/sim2real-tjkey-dbe372b601e4.json', expected 'reports/daily'
  Expected (AWS): Transcript.TranscriptFileUri names key '<OutputKey>' exactly ('reports/daily'); GetObject on it returns the transcript document; HeadObject on '<OutputKey>/<TranscriptionJobName>.json' answers 404.
  Actual (emulator): TranscriptFileUri names '<OutputKey>/<TranscriptionJobName>.json' and the document is written there; the exact key '<OutputKey>' does not exist.
  (The job name in the quoted message is a per-run value; the requirement is that the trailing slash, not the job name, decides the key.)

## Steps to Reproduce
Cause 1:
1. CreateBucket <bucket>; PutObject <bucket>/calls/call.wav (2-channel WAV).
2. StartCallAnalyticsJob(CallAnalyticsJobName=<job>, Media={MediaFileUri: s3://<bucket>/calls/call.wav}, OutputLocation='s3://<bucket>/analytics-a/', ChannelDefinitions=[{0,AGENT},{1,CUSTOMER}]).
3. Poll GetCallAnalyticsJob until COMPLETED.
4. ListObjectsV2(Bucket=<bucket>, Prefix='analytics-a/'). AWS holds 'analytics-a/analytics/<job>.json'; the emulator holds 'analytics-a/<job>.json'.

Cause 2:
1. CreateBucket <bucket>; PutObject <bucket>/media/clicks.wav (mono WAV).
2. StartTranscriptionJob(TranscriptionJobName=<job>, LanguageCode='en-GB', MediaFormat='wav', Media={MediaFileUri: s3://<bucket>/media/clicks.wav}, OutputBucketName=<bucket>, OutputKey='reports/daily').
3. Poll GetTranscriptionJob until COMPLETED and read Transcript.TranscriptFileUri.
4. AWS: the URI names key 'reports/daily', GetObject on 'reports/daily' returns the document and HeadObject on 'reports/daily/<job>.json' is 404. Emulator: the URI names 'reports/daily/<job>.json' and 'reports/daily' does not exist.
5. Repeat with OutputKey='archive/weekly/' (trailing slash): both agree on 'archive/weekly/<job>.json'.
