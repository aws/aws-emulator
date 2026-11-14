Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: transcribe.transcription-job-identify-language-reflects-detected-code
Issue-Shape-Id: com.amazonaws.transcribe#Transcribe
Issue-Key: transcribe.transcription-job-identify-language-reflects-detected-code|run#18|AssertionError|assert fr_job["LanguageCode"] == "fr-FR", f"expected fr-FR to be identified, got {fr_job.get('LanguageCode')}"
Issue-Commit: 7c3ecfa1a25692ce41ad054c5d4a0fbbc15f7c4b

# IdentifyLanguage transcription jobs always report LanguageCode en-US instead of the detected language

> _Note: this issue was identified against the code base at commit `7c3ecfa1a25692ce41ad054c5d4a0fbbc15f7c4b`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
StartTranscriptionJob

## Description
Cause 1 of 1: automatic language identification is not performed — the completed job's identified LanguageCode is a fixed en-US default rather than a value derived from the media.

AWS: StartTranscriptionJob accepts IdentifyLanguage (Boolean) together with LanguageOptions (list, @length min 1) in place of an explicit LanguageCode. Smithy: StartTranscriptionJob.IdentifyLanguage -> Boolean, StartTranscriptionJob.LanguageOptions -> LanguageOptions; output TranscriptionJob carries LanguageCode and IdentifiedLanguageScore. Smithy doc, TranscriptionJob.LanguageCode: "The language code used to create your transcription job. This parameter is used with single-language identification." Smithy doc, TranscriptionJob.IdentifiedLanguageScore: "The confidence score associated with the language identified in your media file. Confidence scores are values between 0 and 1; a larger value indicates a higher probability that the identified language correctly matches the language spoken in your media." So on a COMPLETED job started with IdentifyLanguage=True, LanguageCode must be whichever member of LanguageOptions actually matches the speech in the media object, and IdentifiedLanguageScore the confidence for that choice — it is a function of the media, not of the request's option ordering or a constant.

Emulator: the recorded behavior shows the language is never inspected. Two jobs were started in the same run against the same bucket with identical parameters (IdentifyLanguage=True, LanguageOptions=["en-US","es-ES","fr-FR"], MediaFormat="mp3"), differing only in the Media.MediaFileUri object: one holding English speech, one holding French speech. Both jobs completed reporting LanguageCode "en-US". The French-audio job diverges: expected LanguageCode "fr-FR", actual "en-US". The constant equals the first entry of LanguageOptions / the service's default locale, which is what a hardcoded answer looks like. Hypothesis: the job-completion path stamps a default locale onto LanguageCode whenever LanguageCode is absent from the request, instead of selecting among LanguageOptions from the media.

Why: a client that uses IdentifyLanguage to route or label multilingual media gets a silently wrong answer rather than an error. Every non-English item is mislabelled en-US with a confident IdentifiedLanguageScore, so downstream branching on job.LanguageCode (choosing a translation target, a vocabulary, a locale-specific post-processor) takes the wrong branch and the transcript is attributed to the wrong language with no failure signal anywhere. Tests that assert language identification works pass against AWS and fail only here; worse, tests that happen to use English audio pass against the emulator and hide that the feature is absent entirely.

Fix: the StartTranscriptionJob handler's language-identification path and the job-progression state that finalizes a transcription job. When IdentifyLanguage (or IdentifyMultipleLanguages) is set, the completed job's LanguageCode/LanguageCodes and IdentifiedLanguageScore must be chosen from the request's LanguageOptions as a function of the referenced media object, so that two jobs differing only in their Media.MediaFileUri can resolve to different language codes; the current constant default must not be used to fill LanguageCode on an identify-language job.

Traces: transcribe.transcription-job-identify-language-reflects-detected-code — assertion `fr_job["LanguageCode"] == "fr-FR"` failed with "expected fr-FR to be identified, got en-US"; the en-US leg of the same check passed, which is exactly the signature of a hardcoded default.

## Evidence
Failing check: transcribe.transcription-job-identify-language-reflects-detected-code (mismatch)

  AssertionError: expected fr-FR to be identified, got en-US
  at check_transcribe_transcription_job_identify_language_reflects_detected_code.py line 55:
    assert fr_job["LanguageCode"] == "fr-FR", f"expected fr-FR to be identified, got {fr_job.get('LanguageCode')}"

Two StartTranscriptionJob calls in one run, identical except for the media object:

  Request A: {TranscriptionJobName: <en-name>, IdentifyLanguage: true,
              LanguageOptions: ["en-US","es-ES","fr-FR"], MediaFormat: "mp3",
              Media: {MediaFileUri: "s3://<bucket>/speech-en.mp3"}}   # English speech
  Request B: {TranscriptionJobName: <fr-name>, IdentifyLanguage: true,
              LanguageOptions: ["en-US","es-ES","fr-FR"], MediaFormat: "mp3",
              Media: {MediaFileUri: "s3://<bucket>/speech-fr.mp3"}}   # French speech

GetTranscriptionJob after both reached COMPLETED:

  Expected (AWS):   A -> TranscriptionJob.LanguageCode = "en-US"
                    B -> TranscriptionJob.LanguageCode = "fr-FR"
                    both -> IdentifiedLanguageScore > 0.5 for the identified language
  Actual (emulator): A -> TranscriptionJob.LanguageCode = "en-US"
                     B -> TranscriptionJob.LanguageCode = "en-US"   <-- divergence

Both jobs reached TranscriptionJobStatus COMPLETED, so this is not a failure or a
refusal: the response shape is right and the value is wrong. The identified code is
invariant under a change of media content, i.e. it is not derived from the media at all.
(The concrete job names and bucket name are per-run values and are not part of the
requirement; the requirement is that LanguageCode track the media's language and be
drawn from LanguageOptions.)

## Steps to Reproduce
1. Create an S3 bucket in the emulator's region.
2. Synthesize two mp3 objects of genuinely different languages into that bucket, e.g. via Polly:
   - polly.synthesize_speech(Text="This is a test of automatic language identification.", OutputFormat="mp3", VoiceId="Joanna") -> s3://<bucket>/speech-en.mp3
   - polly.synthesize_speech(Text="Ceci est un test d'identification automatique de la langue.", OutputFormat="mp3", VoiceId="Lea") -> s3://<bucket>/speech-fr.mp3
3. Start two transcription jobs, identical except for the media key, with NO LanguageCode:
   transcribe.start_transcription_job(TranscriptionJobName=<name>, IdentifyLanguage=True,
       LanguageOptions=["en-US","es-ES","fr-FR"], MediaFormat="mp3",
       Media={"MediaFileUri": "s3://<bucket>/speech-en.mp3"})
   ... and the same for speech-fr.mp3.
4. Poll transcribe.get_transcription_job(TranscriptionJobName=<name>) for each until
   TranscriptionJobStatus is COMPLETED or FAILED.
5. Compare TranscriptionJob.LanguageCode of the two jobs.
   Observed: both are "en-US".
   Expected: the English job is "en-US" and the French job is "fr-FR", each with
   IdentifiedLanguageScore > 0.5.
   The English leg alone passes, so reproducing this requires the second, non-English leg.
