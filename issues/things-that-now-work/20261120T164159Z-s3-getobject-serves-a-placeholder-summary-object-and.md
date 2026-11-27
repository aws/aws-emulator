Issue-Source: trace-sweep-divergence
Issue-Trace: connecthealth-patient-insights-job-completes-with-summary
Issue-Step: task#13
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: connecthealth-patient-insights-job-completes-with-summary
Issue-Shape-Id: com.amazonaws.connecthealth#ConnectHealth
Issue-Key: s3|GetObject|2xx->2xx|$.PatientSummaries[*].PatientSummary.Sections[*]
Issue-Key: s3|GetObject|2xx->2xx|$.PatientSummaries[*].PatientSummary.Sections[*].ClinicalNarrative[*]
Issue-Key: s3|GetObject|2xx->2xx|$.PatientSummaries[*].PatientSummary.Sections[*].SectionName
Issue-Key: s3|GetObject|2xx->2xx|$headers.x-amz-checksum-crc32
Issue-Commit: b96dec13d92b321d340976e9dfed40403f6506f9

# s3 GetObject serves a placeholder summary object and omits the stored CRC32 checksum header

> _Note: this issue was identified against the code base at commit `b96dec13d92b321d340976e9dfed40403f6506f9`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetObject

## Description
One cause, with two coupled symptoms observed at the same step: the object body s3 serves back for a generated patient-insights summary is a placeholder stand-in rather than the summary document, and the response carries no object checksum header.

Cause 1 of 1: GetObject of the patient-insights summary object (fixture connecthealth-patient-insights-job-completes-with-summary, step task#13).

AWS: The object body is not a modelled Smithy shape (s3 GetObject's Body is a StreamingBlob @httpPayload, so s3 returns verbatim whatever bytes were stored), so the document's composition is recorded behavior only. The recording shows the object stored at output/<jobId>/<patientId>/summary.json by a SUCCEEDED patient-insights job is a JSON document whose PatientSummaries[0].PatientSummary.Sections is a LIST OF ONE SECTION PER INSIGHT SECTION, each with a SectionName that is a markdown heading (fixed text of the form "# <SECTION_TOKEN>\n", e.g. the overview, since-last-visit and trends sections) and a ClinicalNarrative list whose every element has BOTH members: Text (the narrative prose, generated from the patient's documents) and Evidence (a list, empty when no document evidence is cited). The section tokens and their ordering are the generator's fixed vocabulary; the narrative Text and the Evidence entries are values derived from the request's patient/document state. For the response HEADER: Smithy GetObject output member ChecksumCRC32 binds to httpHeader x-amz-checksum-crc32, and the operation carries httpChecksum with responseAlgorithms including CRC32; the recorded 200 response for this object carries x-amz-checksum-crc32, i.e. s3 returns the checksum it stored for the object alongside the body.

Emulator: It answers 200 but serves a one-section placeholder document. $.PatientSummaries[0].PatientSummary.Sections has a single element whose SectionName is the literal token 'InsightsNotGenerated' instead of a markdown section heading, and whose ClinicalNarrative[0] is {"Text": "No clinical narrative was generated: this emulator runs no clinical summarisation model, so a patient insights job stores the identity of its summary and no insights content."} — a self-describing emulator notice in place of narrative prose, and with the Evidence member omitted entirely rather than present and empty. Sections[1] (the since-last-visit section) and Sections[2] (the trends section) are absent. The 200 response also omits the x-amz-checksum-crc32 header (expected present, got None).

Why: a client that reads the summary object the job points it at cannot consume it. Every consumer walks Sections by heading and reads ClinicalNarrative[].Text/Evidence pairs; against the emulator it finds one unknown section token, no per-section narratives, and a missing Evidence member, so section-driven rendering, evidence linking and any iteration over the modelled section set either produce nothing or fail on the absent key. Separately, a client that enables response checksum validation (or merely records the object's checksum for later integrity comparison) gets no x-amz-checksum-crc32 header, so it cannot verify the bytes it just downloaded and either skips integrity checking silently or errors on the missing header.

Fix: two components. (a) The object-content producer: the patient-insights job's terminal-success transition writes this object, so the summary document it stores must have the per-section structure above (one entry per insight section, each ClinicalNarrative element carrying both Text and Evidence) instead of a single 'InsightsNotGenerated' placeholder section — s3's GetObject handler faithfully returns whatever was stored, so the stored bytes are what diverge. (b) The s3 GetObject response serializer / object state: it must retain each stored object's checksum and emit the corresponding x-amz-checksum-* header (CRC32 here) on the GetObject response.

Related but not covering this: open issue 20261109T171442Z-connecthealth-patient-insights-job-responses-diverge-epoch-number.md owns the connecthealth StartPatientInsightsJob/GetPatientInsightsJob API response divergences (epoch timestamps, job-id form, insightsOutput.uri prefix, absent statusDetails) at steps task#11 and task#12. It does not cover the s3 object this step reads, nor the missing s3 checksum header, and it carries no replay key for step task#13.

Traces: connecthealth-patient-insights-job-completes-with-summary#task#13.

## Evidence
Replay of trace set simparity-wire-aws-v1 against the booted emulator (score 95.2%, 3 mismatched steps of 62). Divergence cluster service=s3|GetObject|status=200->200, fixture connecthealth-patient-insights-job-completes-with-summary, step task#13, expected status 200 and 200 received, body and headers diverge:

  $.PatientSummaries[0].PatientSummary.Sections[0].SectionName: expected a markdown section heading of the form '# <SECTION_TOKEN>\n' (recorded: '# PATIENT_AND_ENCOUNTER_OVERVIEW\n'), got 'InsightsNotGenerated'
  $.PatientSummaries[0].PatientSummary.Sections[0].ClinicalNarrative[0]: expected an element carrying BOTH members, {'Text': <narrative prose>, 'Evidence': <list, empty when nothing is cited>} (recorded: {'Text': '## Overview\n', 'Evidence': []}), got {'Text': 'No clinical narrative was generated: this emulator runs no clinical summarisation model, so a patient insights job stores the identity of its summary and no insights content.'} — an emulator notice in place of narrative text, with no Evidence member at all
  $.PatientSummaries[0].PatientSummary.Sections[1]: expected the since-last-visit section (recorded: {'SectionName': '# SINCE_LAST_VISIT\n', 'ClinicalNarrative': [{'Text': "This is the patient's first visit with this provider (no previous visit found).\n", 'Evidence': []}]}), got None — the section is absent
  $.PatientSummaries[0].PatientSummary.Sections[2]: expected the trends section (recorded: {'SectionName': '# TRENDS\n', 'ClinicalNarrative': [{'Text': 'No active trends requiring monitoring.\n', 'Evidence': []}]}), got None — the section is absent
  $headers.x-amz-checksum-crc32: expected present (the object's stored CRC32), got None — the header was not emitted

Fixed vs variable: the section tokens and the SectionName/ClinicalNarrative/Text/Evidence member names are fixed structure; the narrative prose and Evidence contents are values derived from the patient's documents and prior-visit state; the bucket name, job id and patient id in the object key, and the checksum value itself, are per-run values and are not requirements.

## Steps to Reproduce
1. Boot the emulator and replay fixture connecthealth-patient-insights-job-completes-with-summary from trace set simparity-wire-aws-v1.
2. The fixture starts a patient-insights job and waits for it to reach its terminal success status (steps task#11/task#12), then at step task#13 issues GET /<outputBucket>/output/<jobId>/<patientId>/summary.json (s3 GetObject) for the summary object the job produced.
3. Inspect the 200 response body: PatientSummaries[0].PatientSummary.Sections holds a single element with SectionName 'InsightsNotGenerated' and a ClinicalNarrative[0] containing only a Text member whose value is an emulator notice; the since-last-visit and trends sections are missing and no ClinicalNarrative element carries Evidence.
4. Inspect the 200 response headers: x-amz-checksum-crc32 is absent, although the object was stored through this emulator's s3 and the recorded response carries it.
