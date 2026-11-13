Issue-Source: trace-sweep-divergence
Issue-Trace: gameliftstreams-session-and-url-error-paths
Issue-Step: task#4
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: gameliftstreams-session-and-url-error-paths
Issue-Shape-Id: com.amazonaws.gameliftstreams#GameLiftStreams
Issue-Key: gameliftstreams|GetStreamUrl|4xx->4xx|$.Message
Issue-Key: gameliftstreams|RevokeStreamUrl|4xx->4xx|$.Message
Issue-Commit: 6c2a412138b2fb0170715c3d0a682813a233dd77

# gameliftstreams stream-URL not-found message uses generic resource template instead of "Stream URL not found"

> _Note: this issue was identified against the code base at commit `6c2a412138b2fb0170715c3d0a682813a233dd77`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetStreamUrl

## Description
Two causes. Both are the SAME fault seen on the two stream-URL operations: when the stream URL named by the {StreamUrlIdentifier} path label does not exist, the emulator serves the correct 404 ResourceNotFoundException class but fills its Message with the service's generic "Resource '<arn>' does not exist" template instead of the stream-URL-specific text AWS uses. Both causes must be closed.

Cause 1 of 2 — GetStreamUrl (fixture gameliftstreams-session-and-url-error-paths, step task#4).
AWS: Smithy model (services/service-com-amazonaws-gameliftstreams-gameliftstreams/smithy-model/com-amazonaws-gameliftstreams-gameliftstreams.json) — GetStreamUrl is GET /streamgroups/{Identifier}/streamurls/{StreamUrlIdentifier} (http code 200, readonly); both path labels are @required Identifier (length 1..128, pattern '^(^[a-zA-Z0-9-]+$)|(^arn:aws:gameliftstreams:([^:]*):([0-9]{12}):([^:]*)$)$'). The operation models ResourceNotFoundException (errorKind client, httpError 404), whose required member Message is documented "Description of the error" and whose shape doc is "The resource specified in the request was not found. Correct the request before you try again." The model does not dictate the text, so the message is recorded behavior only: for a missing stream URL AWS answers 404 ResourceNotFoundException with Message of the form
  Stream URL not found (Internal ID: <internal-id>)
Fixed text, exactly: "Stream URL not found (Internal ID: " then ")". It quotes NO ARN and NO identifier. The only variable part is the internal id, a per-run server-generated correlation id that is NOT part of the contract.
Emulator: answers 404 with the right error code but Message of the form "Resource '<arn:aws:gameliftstreams:<region>:<account>:streamurl/<groupId>/<streamUrlId>>' does not exist (Internal ID: <internal-id>)" — i.e. the generic resource-not-found template, plus a streamurl ARN the emulator composed from the two path labels. $.Message was the only diverging field (expected status 404, actual 404).

Cause 2 of 2 — RevokeStreamUrl (fixture gameliftstreams-session-and-url-error-paths, step task#5).
AWS: same model file — RevokeStreamUrl is POST /streamgroups/{Identifier}/streamurls/{StreamUrlIdentifier}/revoke (http code 204, idempotent, empty output), errors include ResourceNotFoundException (client, httpError 404). Again recorded behavior only: a revoke against a stream URL that does not exist answers 404 ResourceNotFoundException with Message "Stream URL not found (Internal ID: <internal-id>)" — same fixed text as GetStreamUrl, same single per-run variable.
Emulator: answers 404 with Message "Resource '<arn:aws:gameliftstreams:<region>:<account>:streamurl/<groupId>/<streamUrlId>>' does not exist (Internal ID: <internal-id>)", the same generic template. $.Message was the only diverging field (expected 404, actual 404).

Why: the Message is the only place these two operations tell a client WHAT was missing, and the stream-URL paths deliberately use a short, resource-type-specific sentence that does not echo an identifier. A client or test that branches on AWS's "Stream URL not found" wording — retry/refresh the stream URL rather than re-resolve the stream group — never matches against the emulator, and instead receives a generic "Resource '<arn>' does not exist" naming a streamurl ARN the emulator synthesized from the request's path labels (so it also leaks a resource identity AWS never returns here, and will read as authoritative). Logs and error-classification code keyed on the AWS sentence silently fall through to a default branch.

Fix: the not-found path of the GetStreamUrl and RevokeStreamUrl handlers, and the service's shared ResourceNotFoundException message builder it delegates to. Those two operations must emit the stream-URL message template ("Stream URL not found (Internal ID: <id>)", no ARN) rather than the generic "Resource '<arn>' does not exist" template; the generic template remains correct for the operations that do report a resource ARN. Note the complementary open issue 20261030T062718Z-gameliftstreams-createstreamsessionadminshell-resourcenotfoundexception-message-names-a-synthesized-stream.md, which reports the OPPOSITE substitution on CreateStreamSessionAdminShell (a resource-specific sentence where AWS uses the generic one); it does not cover the two stream-URL operations filed here, and whichever is fixed first should not be assumed to fix the other — per-operation message selection is the shared root.

Traces: gameliftstreams-session-and-url-error-paths#task#4 (GetStreamUrl), gameliftstreams-session-and-url-error-paths#task#5 (RevokeStreamUrl).

## Evidence
Replay of trace set simparity-wire-aws-v1, fixture gameliftstreams-session-and-url-error-paths. Emulator up (/__admin/health 200), bootstrap_signal false, so these are fidelity divergences, not a missing bootstrap.

Cause 1 — step task#4, operation GetStreamUrl, verdict mismatch:
- Status: expected 404, actual 404 (status and error class agree).
- $.Message: value_mismatch.
  expected: "Stream URL not found (Internal ID: dea62e98-8bd5-4698-8a1b-137f78f1091f)"
  actual:   "Resource 'arn:aws:gameliftstreams:us-west-2:000000000000:streamurl/sg-1AB2C3De9/su-1AB2C3De9' does not exist (Internal ID: dd199890-7201-053a-735e-8ee34466f959)"
- divergence_key: service=gameliftstreams|GetStreamUrl|status=404->404|keys=$.Message

Cause 2 — step task#5, operation RevokeStreamUrl, verdict mismatch:
- Status: expected 404, actual 404.
- $.Message: value_mismatch.
  expected: "Stream URL not found (Internal ID: adf4d020-1e68-4877-8859-d7c8f17e705e)"
  actual:   "Resource 'arn:aws:gameliftstreams:us-west-2:000000000000:streamurl/sg-1AB2C3De9/su-1AB2C3De9' does not exist (Internal ID: 0fa59dc3-d2cc-2dd6-9fbf-ca7c4b271aa5)"
- divergence_key: service=gameliftstreams|RevokeStreamUrl|status=404->404|keys=$.Message

Which parts are requirements: the fixed text "Stream URL not found (Internal ID: " ... ")" is required verbatim, and the absence of any quoted ARN/identifier is part of it. The Internal ID is a per-run correlation id and is explicitly NOT the divergence being filed (the two sides differ there in every run). The region us-west-2, account 000000000000, group id sg-1AB2C3De9 and stream-url id su-1AB2C3De9 above are this recording's request/state values shown only to identify the emulator's synthesized ARN — they are not part of the requirement.

Model evidence (smithy_query on services/service-com-amazonaws-gameliftstreams-gameliftstreams/smithy-model/com-amazonaws-gameliftstreams-gameliftstreams.json): GetStreamUrl GET /streamgroups/{Identifier}/streamurls/{StreamUrlIdentifier}, 200, readonly; RevokeStreamUrl POST /streamgroups/{Identifier}/streamurls/{StreamUrlIdentifier}/revoke, 204, idempotent, output Unit; both list ResourceNotFoundException (errorKind client, httpError 404). ResourceNotFoundException = structure with @required Message (String, doc "Description of the error"), shape doc "The resource specified in the request was not found. Correct the request before you try again." — no message template is modeled, hence "recorded behavior only" for the text.

## Steps to Reproduce
1. git clone the repo and boot the emulator built from that checkout.
2. Replay fixture gameliftstreams-session-and-url-error-paths from trace set simparity-wire-aws-v1.
3. Cause 1 — step task#4 issues GET /streamgroups/{Identifier}/streamurls/{StreamUrlIdentifier} for a stream URL that does not exist. Observe the 404 body: $.Message reads "Resource '<streamurl ARN composed from both path labels>' does not exist (Internal ID: ...)" where AWS answers "Stream URL not found (Internal ID: ...)".
4. Cause 2 — step task#5 issues POST /streamgroups/{Identifier}/streamurls/{StreamUrlIdentifier}/revoke for the same non-existent stream URL. Observe the same generic "Resource '<streamurl ARN>' does not exist (Internal ID: ...)" message where AWS answers "Stream URL not found (Internal ID: ...)".
5. Both steps return the correct 404 / ResourceNotFoundException class, so only $.Message diverges; ignore the differing Internal ID values, which are per-run.
