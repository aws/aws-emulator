Issue-Source: trace-sweep-divergence
Issue-Trace: codeartifact-repository-error-paths
Issue-Step: task#7
Issue-Severity: LOW
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: codeartifact-repository-error-paths
Issue-Shape-Id: com.amazonaws.codeartifact#CodeArtifactControlPlaneService
Issue-Key: codeartifact|AssociateExternalConnection|4xx->4xx|$.message
Issue-Commit: 09fb1164602e6ebdd2bd6e78b61b67ab44fe1e77

# CodeArtifact AssociateExternalConnection renders its own external-connection-not-found sentence

> _Note: this issue was identified against the code base at commit `09fb1164602e6ebdd2bd6e78b61b67ab44fe1e77`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
AssociateExternalConnection

## Description
ONE root cause, found by replaying the simparity-wire-aws-v1 wire corpus against the emulator booted from this checkout. A fixer must close this single cause before submitting. (The domain, repository, external-connection and account values quoted below are one run's values filling a template - only the fixed text and the rule for the variable parts are the requirement.)

=== Cause 1 of 1: AssociateExternalConnection's not-found refusal message is re-authored, naming repository, domain and owner account where the service's sentence does not ===
AWS: Smithy AssociateExternalConnection (POST /v1/repository/external-connection, success 200, httpQuery members domain [required], domainOwner, repository [required], externalConnection [required, ExternalConnectionName @length 2..100 @pattern '^[A-Za-z0-9][A-Za-z0-9._\\-:]{1,99}$']) models ResourceNotFoundException with httpError 404, whose message member is @required and whose resourceId/resourceType members are optional. The model does not document the refusal text, so the sentence is recorded behavior only - and in this corpus the recorded message is MASKED ('<shaped-not-asserted>'), i.e. the recording pins the message's shape rather than its literal characters. What the recording therefore establishes is that the service's sentence for an external connection that does not exist is NOT the sentence the emulator emits: the recorded shape does not admit it. The requirement is the service's own fixed text for this error class with only the identifier(s) the service includes - the external connection name as the request sent it - and nothing drawn from emulator state.
Emulator: the error class and status are right - ResourceNotFoundException / HTTP 404, which match - but the message is the emulator's own four-clause sentence, "External connection not found. External connection '<externalConnection>' in repository '<repository>' in domain '<domain>' owned by account '<domainOwner>' does not exist." (in this run: "External connection not found. External connection 'public:does-not-exist' in repository 'sim2real-probe-codeartifact-caarterrepo-f3d17e83' in domain 'sim2real-probe-codeartifact-caarterdom-f3d17e83' owned by account '000000000000' does not exist."). Of its variable parts, the external connection, repository and domain come from the request; the account component comes from emulator state (the resolved domain owner), not from the request. Hypothesis: a generic "not found" formatter enumerates the whole resolved resource path for every CodeArtifact lookup miss, rather than this operation's own sentence.
Why: the refusal message is part of the wire contract. A client, SDK retry/classification layer, or test that matches or surfaces the external-connection-not-found sentence - e.g. to distinguish "this external connection name is not a real one" from "this repository or domain is missing" - reads text the service never produces, and the emulator additionally discloses the owner account in a message AWS's sentence does not scope that way. Anything asserting on the message passes against the emulator and fails against AWS.
Fix: the not-found refusal message construction in the AssociateExternalConnection handler (and the shared CodeArtifact not-found message helper it calls) in the codeartifact service crate - this branch must emit the service's own fixed sentence for an unknown external connection, parameterised only by the request's external-connection name, and must not append repository/domain/owner-account clauses. The status (404) and error code (ResourceNotFoundException) are already correct and must not change.
Traces: codeartifact-repository-error-paths#task#7 (the only member of its cluster). The replay output reports owned_by empty and stub_path empty for it, so no open issue carries this replay key. The open issues 20261103T055241Z-codeartifact-describepackage-not-found-message-includes-a-namespace.md and 20261103T103905Z-codeartifact-describepackage-package-not-found-sentence-renders-a.md are the same FAMILY of defect (a re-authored CodeArtifact refusal sentence) but a different operation, branch and template, and do not cover this step; the two open external-connection issues (20261027T083906Z..., 20261029T095746Z...) are about data-plane ingestion through an ASSOCIATED connection, not about this refusal.

## Evidence
Replay of simparity-wire-aws-v1 against the emulator booted from this checkout: 34 fixtures, 344 steps, 305 passed, 39 mismatched, 0 errored, 0 blocked, score 88.7%, 14 divergence clusters - all on service "codeartifact". Verdict verbatim from the replay output for this cluster (neither stub_path nor owned_by set):

[mismatch] codeartifact-repository-error-paths#task#7 AssociateExternalConnection, status 404 -> 404, affected_count 1:
  $.message: value_mismatch (expected '<shaped-not-asserted>', got "External connection not found. External connection 'public:does-not-exist' in repository 'sim2real-probe-codeartifact-caarterrepo-f3d17e83' in domain 'sim2real-probe-codeartifact-caarterdom-f3d17e83' owned by account '000000000000' does not exist.")

Expected: the recorded message, pinned by shape ('<shaped-not-asserted>' is the corpus's mask for a message asserted on shape rather than literal text) - the service's own fixed sentence for an unknown external connection, carrying only the external-connection name the request sent. Actual: the emulator's four-clause sentence above. HTTP status (404) and the error code / x-amzn-errortype (ResourceNotFoundException) match AWS; only the message diverges, and the divergence is structural, not a per-run value: the emulator's sentence adds whole clauses (" in repository '...'", " in domain '...'", " owned by account '...'") and the trailing " does not exist.", and the account clause is filled from emulator state rather than from the request. The names and the account digits quoted are this run's values filling the template and are not themselves the requirement.

## Steps to Reproduce
Boot the emulator from this checkout and replay simparity-wire-aws-v1; the failing step is codeartifact-repository-error-paths#task#7.

By hand, with a codeartifact client pointed at the emulator:
1. create_domain(domain=D).
2. create_repository(domain=D, repository=R).
3. associate_external_connection(domain=D, repository=R, externalConnection='public:does-not-exist')
   -> expect ResourceNotFoundException / HTTP 404 whose message is the service's fixed external-connection-not-found sentence, parameterised only by the external-connection name sent in the request.
   -> observe ResourceNotFoundException / HTTP 404 with "External connection not found. External connection 'public:does-not-exist' in repository 'R' in domain 'D' owned by account '<ownerAccount>' does not exist." - three extra clauses, one of them filled from emulator state (the resolved domain owner).
4. Control (shows the clauses are unconditional, not a per-run artifact): repeat step 3 with a different domain/repository pair and a different bogus connection name; the same three extra clauses appear with the new values substituted, so the divergence is in the template, not in any one run's values.
