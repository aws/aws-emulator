Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: pca-connector-scep.delete-challenge-removes-password pca-connector-scep.delete-connector-cascades-challenges pca-connector-scep.delete-connector-twice-rejected
Issue-Shape-Id: com.amazonaws.pcaconnectorscep#PcaConnectorScep
Issue-Key: pca-connector-scep|DeleteChallenge|ResourceNotFoundException
Issue-Key: pca-connector-scep|DeleteConnector|ResourceNotFoundException
Issue-Commit: cd1ff1c075c03af4b96f1d8fe21bfc329bc355d4

# pca-connector-scep: 2 net-new causes — DeleteChallenge and DeleteConnector are not idempotent, refusing an already-deleted resource with 404 ResourceNotFoundException instead of succeeding

> _Note: this issue was identified against the code base at commit `cd1ff1c075c03af4b96f1d8fe21bfc329bc355d4`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DeleteChallenge

## Description
REOPENS 20261024T020831Z-pca-connector-scep-deletechallenge-error-response.md: its fix left the call refused. REOPENS 20261024T020831Z-pca-connector-scep-deleteconnector-error-response.md: its fix left the call refused.

Behavioral replay of suite simparity-behavioral-aws-v1 against the pca-connector-scep emulator failed 9 of 21 checks. Six of them are already owned by open issue 20261029T213350Z-pca-connector-scep-7-net-new-causes-createconnector.md and are excluded here. The remaining three now fail PAST the divergences that issue records (the ResourceNotFoundException message template it describes in its cause 4 now matches AWS: the emulator emits "Challenge was not found: <challengeArn>." / "Connector was not found: <connectorArn>."), on a new divergence: the two delete operations refuse a repeat delete. These three failures cluster into 2 distinct root causes, both in pca-connector-scep. A fixer must close both before submitting.

Cause 1 of 2: DeleteChallenge is not idempotent — a repeat delete of a challenge that is already gone is refused instead of succeeding.
  AWS: DeleteChallenge is declared @idempotent in the service model and its success is HTTP 202 with an empty body. Repeating the call on a ChallengeArn whose challenge has already been removed — either by an earlier DeleteChallenge or by the cascading teardown of its connector — answers that same empty 202/200 success, and the challenge stays unreadable afterwards (GetChallengeMetadata / GetChallengePassword keep answering 404 ResourceNotFoundException with Message "Challenge was not found: <challengeArn>.": fixed text "Challenge was not found: " plus a terminating ".", the ARN taken verbatim from the request). Smithy (com.amazonaws.pcaconnectorscep): DeleteChallenge DELETE /challenges/{ChallengeArn}, success code 202, trait idempotent=true, output Unit; ResourceNotFoundException is modeled (errorKind=client, httpError=404) but, per the recording, is not what a well-formed ARN with no live record yields on a DELETE. That the already-deleted case is a success rather than the modeled 404 is recorded behavior only (the suite states it was measured against the real service).
  Emulator: the repeat DeleteChallenge answers 404 ResourceNotFoundException, Message "Challenge was not found: <challengeArn>." — the exception escapes the check and aborts it.
  Why: a client cannot safely retry a delete. Any at-least-once caller — an SDK retry after a timed-out first attempt, a cleanup/teardown routine, a Terraform/CloudFormation-style converge loop, a reconciler that deletes whatever it finds — sees a hard 404 ClientError on the second attempt and reports a failure for an operation AWS reports as done. It also makes "delete then confirm gone, then delete again" (the shape both failing checks use) impossible to express.
  Fix: the DeleteChallenge handler's existence check — a well-formed ChallengeArn that resolves to no live challenge must return the empty 202 success instead of raising ResourceNotFoundException, and must not resurrect or re-create any state. The read paths (GetChallengeMetadata, GetChallengePassword, ListChallengeMetadata) keep their 404/absent behavior unchanged.
  Traces: pca-connector-scep.delete-challenge-removes-password, pca-connector-scep.delete-connector-cascades-challenges.

Cause 2 of 2: DeleteConnector is not idempotent — a repeat delete of a connector that is already gone is refused instead of succeeding.
  AWS: DeleteConnector is declared @idempotent in the service model and its success is HTTP 202 with an empty body. Once the connector has been deleted (GetConnector on that ARN answering 404 ResourceNotFoundException with Message "Connector was not found: <connectorArn>."), calling DeleteConnector again on the same ARN answers the empty success again rather than an error. The idempotency is scoped to a legitimately shaped Connector ARN: a wrongly shaped one (a Connector ARN with a trailing /challenge/<uuid> segment) is still refused 400 ValidationException with Message "1 validation error detected: Value '<value>' at 'connectorArn' failed to satisfy constraint: Member must satisfy regular expression pattern: <pattern>" (fixed sentence scaffold; variable parts are the rejected value from the request, the lowerCamel member name, and the member's @pattern with its leading ^ and trailing $ removed). Smithy: DeleteConnector DELETE /connectors/{ConnectorArn}, success code 202, trait idempotent=true, output Unit, ConnectorArn @httpLabel required @length 5..200 @pattern ^arn:aws(-[a-z]+)*:pca-connector-scep:[a-z]+(-[a-z]+)+-[1-9]\d*:\d{12}:connector\/[0-9a-f]{8}(-[0-9a-f]{4}){3}-[0-9a-f]{12}$; ResourceNotFoundException is modeled (client, 404) but is not what the already-deleted case yields — recorded behavior only.
  Emulator: the second DeleteConnector answers 404 ResourceNotFoundException, Message "Connector was not found: <connectorArn>." — the exception escapes the check and aborts it before the ValidationException assertion on the wrongly shaped ARN is ever reached, so that branch is unverified.
  Why: the same retry hazard as cause 1, on the resource that owns everything else in this service. A cleanup path that deletes a connector twice (common when a creation retry produced one handle and teardown runs per-handle), or an SDK retry of a delete whose response was lost, turns a completed deletion into a client error, so callers either crash or must special-case a 404 the real service never sends.
  Fix: the DeleteConnector handler's existence check — a well-formed ConnectorArn with no live connector (including one still tearing down or already torn down) returns the empty 202 success; ARN-shape validation stays ahead of it so a mis-shaped ARN is still a 400 ValidationException. GetConnector/ListConnectors behavior is unchanged.
  Traces: pca-connector-scep.delete-connector-twice-rejected.

## Evidence
Behavioral suite simparity-behavioral-aws-v1: 12/21 passed; pca-connector-scep 12/21 (57%), 9 failed, of which 6 are marked OWNED by 20261029T213350Z-pca-connector-scep-7-net-new-causes-createconnector.md and skipped. The three remaining failures all have verdict [error]: the emulator's refusal surfaced as an uncaught botocore exception out of the check body.

Cause 1:
- pca-connector-scep.delete-challenge-removes-password [error]: the check creates and activates an acm-pca ROOT CA, shares it with pca-connector-scep.amazonaws.com via RAM, creates a connector, waits for ACTIVE, CreateChallenge, DeleteChallenge, then confirms GetChallengePassword/GetChallengeMetadata answer "Challenge was not found: <challengeArn>." and that the challenge is absent from ListChallengeMetadata — all of which now pass — and finally repeats the delete (comment in the check: "Measured, not assumed: DeleteChallenge on an already-deleted challenge is idempotent on real AWS -- it returns 200, not ResourceNotFoundException. Any ClientError fails the check."). That repeat raised: botocore.errorfactory.ResourceNotFoundException: An error occurred (ResourceNotFoundException) when calling the DeleteChallenge operation: Challenge was not found: arn:aws:pca-connector-scep:us-west-2:000000000000:connector/11e0b6a9-2596-4e15-a507-2d94e36ee78e/challenge/fa042742-c731-409d-8d2b-0f7bdd7e7d33.
- pca-connector-scep.delete-connector-cascades-challenges [error]: two challenges are created, the connector is deleted, the cascade is confirmed (GetConnector -> ("ResourceNotFoundException", "Connector was not found: <connectorArn>."); GetChallengeMetadata and GetChallengePassword on each challenge -> ("ResourceNotFoundException", "Challenge was not found: <challengeArn>.")), and the check then asserts DeleteChallenge on a cascade-removed challenge is an idempotent success. It raised: botocore.errorfactory.ResourceNotFoundException: An error occurred (ResourceNotFoundException) when calling the DeleteChallenge operation: Challenge was not found: arn:aws:pca-connector-scep:us-west-2:000000000000:connector/0e59d6d3-7bf8-490d-84dd-b86bb71c83b0/challenge/f102f13d-c555-49b5-ae13-b4942408ce23.

Cause 2:
- pca-connector-scep.delete-connector-twice-rejected [error]: CA created and activated, connector created and polled to a terminal status, first DeleteConnector succeeds (positive control), GetConnector then correctly answers ResourceNotFoundException with "Connector was not found: <connectorArn>.". The second DeleteConnector on the same ARN raised: botocore.errorfactory.ResourceNotFoundException: An error occurred (ResourceNotFoundException) when calling the DeleteConnector operation: Connector was not found: arn:aws:pca-connector-scep:us-west-2:000000000000:connector/9a132ef9-03ac-4335-8af8-5bdfb44372fb.
  The check's remaining assertion — DeleteConnector with connector_arn + "/challenge/<uuid>" must still be refused 400 ValidationException naming 'connectorArn' and the anchor-stripped pattern — was never executed.

Expected in all three places: an empty success response (HTTP 202 per the model's success code; the check accepts any non-error), no ClientError. Observed: HTTP 404 with code ResourceNotFoundException. The ARNs, UUIDs, account id and region above are this run's values filling the message templates; only the status, the error code's absence, and the fixed sentence text are requirements.

Note on provenance: fixed issues 20261024T020831Z-pca-connector-scep-deletechallenge-error-response.md and 20261024T020831Z-pca-connector-scep-deleteconnector-error-response.md asked for exactly this 404 on a delete of an ARN that holds no record. That fix is what these checks now trip over: for a well-formed ARN the delete operations are @idempotent and succeed, so the not-found refusal belongs on the read paths (where it is now correct) and not on DELETE.

## Steps to Reproduce
Boot the emulator and replay the pca-connector-scep checks of suite simparity-behavioral-aws-v1 listed in trace_ids.

Cause 1 (DeleteChallenge idempotency):
1. In acm-pca create a ROOT CA, issue and import its certificate so it reaches ACTIVE, and share it with the pca-connector-scep.amazonaws.com service principal via a RAM resource share (sources = the calling account, permission AWSRAMBlankEndEntityCertificateAPICSRPassthroughIssuanceCertificateAuthority); wait for the share to be ACTIVE.
2. CreateConnector with that CertificateAuthorityArn; poll GetConnector until Status is ACTIVE.
3. CreateChallenge on the connector; keep Challenge.Arn.
4. DeleteChallenge(ChallengeArn=that arn) — succeeds.
5. Confirm GetChallengePassword and GetChallengeMetadata on that ARN answer 404 ResourceNotFoundException, Message "Challenge was not found: <challengeArn>.", and that the ARN is absent from ListChallengeMetadata(ConnectorArn=...). (These pass today.)
6. Call DeleteChallenge(ChallengeArn=that same arn) AGAIN. Expect an empty success (202, no error body). Observe 404 ResourceNotFoundException "Challenge was not found: <challengeArn>."
7. Same expectation for a challenge removed by cascade: repeat steps 1-3 creating two challenges, call DeleteConnector, wait for the connector to be gone, then DeleteChallenge on each challenge ARN — each must be an empty success, and GetChallengeMetadata on it must still answer the 404 afterwards.

Cause 2 (DeleteConnector idempotency):
8. Repeat steps 1-2 to get an ACTIVE connector.
9. DeleteConnector(ConnectorArn=...) — succeeds. Poll GetConnector until it answers 404 ResourceNotFoundException, Message "Connector was not found: <connectorArn>."
10. Call DeleteConnector(ConnectorArn=that same arn) AGAIN. Expect an empty success (202). Observe 404 ResourceNotFoundException "Connector was not found: <connectorArn>."
11. Then call DeleteConnector(ConnectorArn=<connectorArn> + "/challenge/<random-uuid>"). This must still be refused 400 ValidationException with Message "1 validation error detected: Value '<value>' at 'connectorArn' failed to satisfy constraint: Member must satisfy regular expression pattern: <ConnectorArn @pattern with its leading ^ and trailing $ removed>", proving the idempotency is scoped to well-shaped Connector ARNs and not a blanket never-fails.
