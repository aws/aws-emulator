Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: mq.create-configuration-duplicate-name-conflict-rejected
Issue-Shape-Id: com.amazonaws.mq#mq
Issue-Key: mq.create-configuration-duplicate-name-conflict-rejected|run#16|AssertionError|assert message == expected, message
Issue-Commit: e47284ab37991d11b5f4fdcc5d2b5ad1491b8a6a

# mq UpdateConfiguration's NotFoundException reuses DescribeConfiguration's message wording for an unknown ConfigurationId

> _Note: this issue was identified against the code base at commit `e47284ab37991d11b5f4fdcc5d2b5ad1491b8a6a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
UpdateConfiguration

## Description
Cause 1 of 1: UpdateConfiguration on a syntactically valid but never-issued ConfigurationId is correctly refused 404 NotFoundException, but the emulator serves DescribeConfiguration's not-found wording instead of UpdateConfiguration's.

AWS: Smithy (services/service-com-amazonaws-mq-mq/smithy-model/com-amazonaws-mq-mq.json) models UpdateConfiguration as PUT /v1/configurations/{ConfigurationId} with ConfigurationId an httpLabel and NotFoundException among its errors (client, httpError 404); DescribeConfiguration is the separate GET on the same URI with its own NotFoundException. Recorded behavior: Amazon MQ gives the two operations DIFFERENT not-found texts. UpdateConfiguration answers 404 NotFoundException with the fixed text `Configuration ID [` + `<the ConfigurationId path segment from the request>` + `] could not be found. Check the ID and try again.`; DescribeConfiguration answers 404 NotFoundException with the fixed text `Configuration ID [` + `<the same request value>` + `] can't be found. Make sure it exists.` Only the bracketed id varies — it is the value the caller sent, not part of the template.

Emulator: UpdateConfiguration returns the right status and error code (404 NotFoundException) but with DescribeConfiguration's message: `Configuration ID [<id>] can't be found. Make sure it exists.` where AWS says `... could not be found. Check the ID and try again.` The DescribeConfiguration leg of the same check passes, so the emulator is routing both operations' not-found refusal through one shared message rather than per-operation wording.

Why: the error message is part of the contract. A client, test or operator runbook that matches or surfaces Amazon MQ's UpdateConfiguration failure text (or that distinguishes the write-path refusal from the read-path one by its wording) reads text AWS never produces for that operation, so it mis-classifies the failure or its assertion passes only against the emulator.

Fix: UpdateConfiguration's handler/not-found error construction in the mq service — emit UpdateConfiguration's own NotFoundException message template rather than reusing the DescribeConfiguration one, keeping the status 404, the error code NotFoundException and the x-amzn-errortype header as they already are.

Traces: the failing behavioral check listed under `### mq` (see trace_ids). The open issue 20261103T045546Z-mq-refuses-invalid-requests-with-the-wrong-status.md touches UpdateConfiguration, but only its missing engine-level Data parse validation (Cause 4) and revision bounds (Cause 5) — not the unknown-ConfigurationId message, which is a distinct defect; 20261029T174752Z-mq-createconfiguration-stub.md and the other open mq issues do not cover it either.

## Evidence
Check mq.create-configuration-duplicate-name-conflict-rejected (its duplicate-name positive control and the DescribeConfiguration leg both pass; the UpdateConfiguration refusal is where it diverges):
  request: UpdateConfiguration(ConfigurationId='c-00000000-0000-0000-0000-000000000000', Data=<base64 of a minimal broker document>)
  expected: 404 NotFoundException, message "Configuration ID [c-00000000-0000-0000-0000-000000000000] could not be found. Check the ID and try again."
  actual:   404 NotFoundException, message "Configuration ID [c-00000000-0000-0000-0000-000000000000] can't be found. Make sure it exists."
  harness: AssertionError: Configuration ID [c-00000000-0000-0000-0000-000000000000] can't be found. Make sure it exists.
            (raised by `assert message == expected` after `code == "NotFoundException"` passed)
  Fixed text expected of UpdateConfiguration: 'Configuration ID [' ... '] could not be found. Check the ID and try again.'
  Fixed text expected of DescribeConfiguration (and what the emulator wrongly serves for both): 'Configuration ID [' ... "] can't be found. Make sure it exists."
  The bracketed id is whatever ConfigurationId the request carried; the all-zero uuid above is just this check's never-issued probe value.

## Steps to Reproduce
1. Clone the repo and boot the emulator built from that checkout; replay the behavioral suite (simparity-behavioral-aws-v1).
2. Call mq.UpdateConfiguration with a well-formed but never-issued ConfigurationId (e.g. 'c-00000000-0000-0000-0000-000000000000') and any base64 Data: PUT /v1/configurations/c-00000000-0000-0000-0000-000000000000 with body {"data": "<base64>"}.
3. Observe 404 NotFoundException (correct) whose message is "Configuration ID [<id>] can't be found. Make sure it exists." — AWS answers "Configuration ID [<id>] could not be found. Check the ID and try again."
4. For contrast, call mq.DescribeConfiguration with the same id: its 404 message is the "can't be found. Make sure it exists." form, which is what the emulator is also emitting for the update path.
