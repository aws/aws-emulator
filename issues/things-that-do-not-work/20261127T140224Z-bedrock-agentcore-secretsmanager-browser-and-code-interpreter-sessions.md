Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: bedrock-agentcore__secretsmanager.browser-session-expired-certificate-secret-rejected bedrock-agentcore__secretsmanager.browser-session-non-pem-second-certificate-secret-rejected bedrock-agentcore__secretsmanager.browser-session-proxy-secret-without-password-rejected bedrock-agentcore__secretsmanager.code-interpreter-session-not-yet-valid-certificate-rejected bedrock-agentcore__secretsmanager.code-interpreter-session-trusts-every-secret-held-root-ca
Issue-Key: bedrock-agentcore__secretsmanager.browser-session-expired-certificate-secret-rejected|_rejected#11|AssertionError|raise AssertionError(f"a refusal was expected, yet a session started: {answer.get('sessionId')!r}")
Issue-Key: bedrock-agentcore__secretsmanager.browser-session-non-pem-second-certificate-secret-rejected|_refusal#11|AssertionError|raise AssertionError(f"the start was expected to be refused but answered {sorted(got)!r}")
Issue-Key: bedrock-agentcore__secretsmanager.browser-session-proxy-secret-without-password-rejected|_refused_with#11|AssertionError|raise AssertionError(f"the proxied session was expected to be refused, got session {out.get('sessionId')!r}")
Issue-Key: bedrock-agentcore__secretsmanager.code-interpreter-session-not-yet-valid-certificate-rejected|_expect_refusal#13|AssertionError|raise AssertionError(f"StartCodeInterpreterSession was expected to refuse, but returned {res!r}")
Issue-Key: bedrock-agentcore__secretsmanager.code-interpreter-session-trusts-every-secret-held-root-ca|run#14|AssertionError|assert installed == sorted(prints), (
Issue-Commit: 461e190cfbed2a60f922688bafee74a0fd88eec8

# bedrock-agentcore__secretsmanager: browser and code-interpreter sessions never validate the certificate/proxy secrets they reference, and never trust the root CAs those secrets hold

> _Note: this issue was identified against the code base at commit `461e190cfbed2a60f922688bafee74a0fd88eec8`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
StartBrowserSession

## Description
Two net-new root causes in the AgentCore session surfaces that read material out of Secrets Manager.

Cause 1 of 2: the secret material a session references is never validated, so a session starts where AWS refuses one.
AWS: recorded behavior only (the session models carry the secret references but state no material rules). The recording shows StartBrowserSession and StartCodeInterpreterSession refusing the start when the referenced secret's material is unusable: a certificate whose validity has expired, a certificate whose notBefore is in the future, a second certificate that is not PEM at all, and a proxy credential secret that carries a username but no password — in each case no session is created.
Emulator: every one of those starts succeeds — StartBrowserSession returns a sessionId/streams and StartCodeInterpreterSession answers HTTP 200 with a sessionId and createdAt.
Why: a client that mis-populates a secret is told its session is healthy; the session then runs with no (or broken) TLS material instead of being refused at start, so the refusal AWS uses to surface the misconfiguration never happens.
Fix: secret-material validation (PEM parsing, notBefore/notAfter window, required proxy fields) in the StartBrowserSession / StartCodeInterpreterSession handlers, before the session is created.
Traces: bedrock-agentcore__secretsmanager.browser-session-expired-certificate-secret-rejected, bedrock-agentcore__secretsmanager.browser-session-non-pem-second-certificate-secret-rejected, bedrock-agentcore__secretsmanager.browser-session-proxy-secret-without-password-rejected, bedrock-agentcore__secretsmanager.code-interpreter-session-not-yet-valid-certificate-rejected.

Cause 2 of 2: root CAs held in the referenced secrets are not installed in the session's trust store.
AWS: recorded behavior only: a code-interpreter session started with two secrets each holding a root CA trusts both — code running in the session sees both CA fingerprints in its trust store.
Emulator: the session trusts 0 of the 2 root CAs (both fingerprints missing).
Why: code in the session cannot verify TLS against the CAs the caller provisioned, so every outbound HTTPS call it makes to a host signed by them fails, with no indication the configuration was ignored.
Fix: install the secret-held CA bundle into the session's trust store in the StartCodeInterpreterSession session-provisioning path.
Traces: bedrock-agentcore__secretsmanager.code-interpreter-session-trusts-every-secret-held-root-ca.

## Evidence
Cause 1 — expected a refusal; actual `a refusal was expected, yet a session started: '1a21d434c400000000000000001'`, `the start was expected to be refused but answered ['ResponseMetadata', 'browserIdentifier', 'createdAt', 'sessionId', 'streams']`, `the proxied session was expected to be refused, got session '1a21d362b6a0000000000000002'`, and `StartCodeInterpreterSession was expected to refuse, but returned {... 'sessionId': '…', 'createdAt': …}` (HTTP 200).
Cause 2 — actual `the session started with both secrets trusts 0 of the 2 root CAs (missing ['9d3345f4…', 'd207ab82…'])`; expected both CA fingerprints present in the session trust store.

## Steps to Reproduce
Cause 1: CreateSecret holding (a) an expired certificate, (b) a certificate whose notBefore is in the future, (c) a non-PEM blob as the second certificate, (d) proxy credentials with a username and no password. Call StartBrowserSession / StartCodeInterpreterSession referencing each secret and observe a created session instead of a refusal.
Cause 2: CreateSecret twice, each holding a distinct root CA; StartCodeInterpreterSession referencing both; run code in the session that lists the trust store fingerprints — neither CA is present.
