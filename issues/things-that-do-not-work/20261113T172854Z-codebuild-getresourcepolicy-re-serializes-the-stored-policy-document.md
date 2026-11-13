Issue-Source: trace-sweep-divergence
Issue-Trace: codebuild-report-group-resource-policy-lifecycle
Issue-Step: task#4
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: codebuild-report-group-resource-policy-lifecycle
Issue-Shape-Id: com.amazonaws.codebuild#CodeBuild_20161006
Issue-Key: codebuild|GetResourcePolicy|2xx->2xx|$.policy
Issue-Commit: 9ca58d7cf30a6ec744c8cbd01b69502e157676e2

# CodeBuild GetResourcePolicy re-serializes the stored policy document instead of returning it verbatim

> _Note: this issue was identified against the code base at commit `9ca58d7cf30a6ec744c8cbd01b69502e157676e2`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetResourcePolicy

## Description
One cause.

Cause 1 of 1: CodeBuild GetResourcePolicy returns a policy document the emulator re-generated from its own parsed representation, rather than the exact string that was stored by PutResourcePolicy.

AWS: In the CodeBuild model the resource policy is an opaque string on both sides of the round trip — Smithy: PutResourcePolicy.policy targets NonEmptyString (@required, @length min 1) and GetResourcePolicy.policy targets NonEmptyString (@length min 1). There is no structured policy shape, so the service persists the submitted document as a string and GetResourcePolicy hands back that same string byte-for-byte: the same JSON member ordering, the same separator whitespace (CodeBuild's recorded documents are compact, no space after ':' or ','), and the same scalar-vs-array form for each policy element (a single-value `Action` stays a string, `Principal.AWS` keeps the exact principal spelling that was submitted, e.g. a root-account ARN of the form arn:aws:iam::<account-id>:root). AWS does not canonicalize, re-order, re-indent, or rewrite principals on read; whatever a caller PUT is what a caller GETs. Recorded behavior confirms this: the fixture PUTs a compact single-statement document and the recorded GetResourcePolicy response echoes it character-for-character.

Emulator: GetResourcePolicy answers HTTP 200 but with a policy string the emulator rebuilt, so three things differ from the stored document: (1) it is pretty-separated — `{"Version": "2012-10-17", "Statement": [...` with a space after every ':' and ',' where the stored document has none; (2) a single-element `Action` is wrapped into an array (`"Action": ["codebuild:BatchGetReportGroups"]` where the stored document has the bare string `"Action":"codebuild:BatchGetReportGroups"`); (3) the principal is both wrapped into an array and semantically rewritten from the submitted root ARN to a bare account id (`"Principal": {"AWS": ["000000000000"]}` where the stored document has `"Principal":{"AWS":"arn:aws:iam::<account-id>:root"}`). The $.policy string therefore never equals what was PUT. Status, error-free path, and the Resource member are correct; only the document text diverges.

Why: Clients treat this field as an opaque document and routinely compare it to what they submitted. Infrastructure tools (CloudFormation/CDK drift detection, Terraform's codebuild_resource_policy state comparison, idempotent re-apply loops) diff the GET result against their desired document and see a permanent, unfixable drift: re-applying the same policy never converges, every plan reports a change. A client that reads the policy, edits one statement and PUTs it back silently loses the original principal spelling, because the root ARN has already been replaced by an account id. Any caller parsing the document for an exact principal ARN, or hashing/signing the document text, gets the wrong answer.

Fix: the GetResourcePolicy handler and the resource-policy state for CodeBuild. The policy must be stored as the opaque string submitted to PutResourcePolicy and returned unmodified on read — not parsed into a structure and re-serialized on the way out. If a parsed form is needed for authorization decisions, keep it alongside the verbatim text and serve the text from GetResourcePolicy. The same re-serialization fault is already filed for another service in 20261029T233239Z-lambda-resource-policy-documents-are-re-serialized-with.md; that issue does not cover CodeBuild, and this cluster additionally shows a principal rewrite, not only whitespace.

Traces: codebuild-report-group-resource-policy-lifecycle#task#4 (representative) and codebuild-report-group-resource-policy-lifecycle#task#6 (the same GetResourcePolicy divergence after the policy is re-put in the lifecycle), both reported under divergence key service=codebuild|GetResourcePolicy|status=200->200|keys=$.policy.

## Evidence
Fixture codebuild-report-group-resource-policy-lifecycle, step task#4 (operation GetResourcePolicy), verdict mismatch. Status matches (expected 200, got 200); the body member $.policy diverges:

Expected (recorded AWS response — the document exactly as PUT):
  {"Version":"2012-10-17","Statement":[{"Effect":"Allow","Principal":{"AWS":"arn:aws:iam::000000000000:root"},"Action":"codebuild:BatchGetReportGroups","Resource":"arn:aws:codebuild:us-west-2:000000000000:report-group/sim2real-probe-codebuild-cb-policy-reportgroup-5e96c195"}]}

Actual (emulator):
  {"Version": "2012-10-17", "Statement": [{"Effect": "Allow", "Principal": {"AWS": ["000000000000"]}, "Action": ["codebuild:BatchGetReportGroups"], "Resource": "arn:aws:codebuild:us-west-2:000000000000:report-group/sim2real-probe-codebuild-cb-policy-reportgroup-5e96c195"}]}

Three classes of difference, all emulator-introduced normalization:
  - separator whitespace added (': ' / ', ' vs ':' / ',')
  - single-value Action promoted to a one-element array
  - Principal.AWS promoted to a one-element array AND rewritten from the root-account ARN form to a bare account id

The account id (000000000000), region (us-west-2) and report-group name in the strings above are this recording's values filling the template; the requirement is that the returned string is identical to the one submitted to PutResourcePolicy, whatever those values are. The same mismatch recurs at step task#6 of the same fixture (affected_count 2).

## Steps to Reproduce
1. Replay fixture codebuild-report-group-resource-policy-lifecycle against the emulator (replay_wire_corpus reproduces it; the divergence is reported at step task#4, and again at task#6).
2. Manually: create a CodeBuild report group, then call PutResourcePolicy with resourceArn = the report group ARN and policy = a compact single-statement document whose Principal.AWS is the root-account ARN (arn:aws:iam::<account-id>:root) and whose Action is the bare string "codebuild:BatchGetReportGroups", with no whitespace between tokens.
3. Call GetResourcePolicy with the same resourceArn.
4. Compare the returned $.policy string to the exact string submitted in step 2. Expected: identical strings. Actual: the emulator returns a re-serialized document with added separator whitespace, Action and Principal.AWS wrapped in arrays, and the root ARN replaced by the bare account id.
