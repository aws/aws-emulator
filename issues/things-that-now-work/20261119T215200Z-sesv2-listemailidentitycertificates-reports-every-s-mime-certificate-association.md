Issue-Source: trace-sweep-divergence
Issue-Trace: sesv2-identity-certificate-association-lifecycle
Issue-Step: task#5
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: sesv2-identity-certificate-association-lifecycle
Issue-Shape-Id: com.amazonaws.sesv2#SimpleEmailService_v2
Issue-Key: sesv2|ListEmailIdentityCertificates|2xx->2xx|$.Certificates[*].Status
Issue-Commit: 93174c59dc6eaf545661028161c23f9dd16ed610

# sesv2 ListEmailIdentityCertificates reports every S/MIME certificate association ACTIVE and never settles one to FAILED

> _Note: this issue was identified against the code base at commit `93174c59dc6eaf545661028161c23f9dd16ed610`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListEmailIdentityCertificates

## Description
Cause 1 of 1: the S/MIME certificate-association status SESv2 projects on an email identity is hard-wired to ACTIVE, so an association whose certificate cannot be prepared for use never reaches the terminal FAILED state.

AWS: Smithy (services/service-com-amazonaws-sesv2-simpleemailservice-v2) — AssociateEmailIdentityCertificate (POST /v2/email/identity/certificates; input EmailIdentity @required, FromAddress, CertificateArn @required with @pattern ^arn:[\w+=/,.@-]+:[\w+=/,.@-]+:[\w+=/,.@-]*:[0-9]+:certificate/[\w+=,.@-]+$, @length 20..2048) creates the association, and ListEmailIdentityCertificates (POST /v2/email/identity/certificates/list, 200) returns Certificates: IdentityCertificateList of IdentityCertificate{FromAddress, Status, CertificateArn, CertificateExpiryTime}. Status targets the enum IdentityCertificateStatus, whose modeled values and documented meanings are: PROVISIONING ("the certificate association was created and the certificate is being prepared for use"), ACTIVE ("the certificate is ready to use for signing"), INACTIVE ("the certificate is no longer used for signing"), DEPROVISIONING ("the certificate association is being cleaned up") and FAILED ("the certificate couldn't be prepared for use, or the certificate has expired"). The IdentityCertificate.Status member doc states the same contract: "A status of ACTIVE indicates that the certificate is ready to use for signing." So Status is a real lifecycle projection, not a constant: an association that AWS could not prepare for use settles at FAILED, and only one it did prepare reads ACTIVE. The recording confirms this: in this fixture the association is made and the subsequent list reports Status FAILED (status 200 on both sides — this is purely the projected enum value, not an error path).

Emulator: ListEmailIdentityCertificates answers 200 with $.Certificates[0].Status = 'ACTIVE' where the recording has 'FAILED'. Every other member of the entry matches, so the association itself is stored; only its status is wrong. The emulator appears to stamp ACTIVE at association time and never recompute or settle it, meaning FAILED (and equally PROVISIONING and DEPROVISIONING) are values it can never emit.

Why: a client that polls ListEmailIdentityCertificates to learn whether an S/MIME certificate is usable for signing is told the certificate is "ready to use for signing" when AWS reports the certificate could not be prepared for use (or has expired). Code that waits for a terminal state sees ACTIVE immediately and proceeds to sign/send with a certificate that does not work, and error-handling branches keyed on FAILED are dead code against the emulator — the failure surfaces later, at send time, instead of at the documented checkpoint. Conversely nothing can be tested against the PROVISIONING -> FAILED path at all.

Fix: the email-identity certificate-association state in the sesv2 crate's state/handler for AssociateEmailIdentityCertificate, plus the projection that ListEmailIdentityCertificates builds its IdentityCertificate entries from. The stored association needs a real status field driven by whether the referenced ACM certificate could be prepared for use (and whether it has expired), and the list projection must read that field rather than emitting a constant ACTIVE. No existing open sesv2 issue covers it: '20261118T045426Z-sesv2-non-send-reads-skip-modeled-validation-and.md' covers omitted/invalid members on other non-send reads and '20261029T171854Z-sesv2-email-identity-dkim-and-mail-from-attributes-are.md' covers DKIM/MAIL FROM attributes, neither the certificate-association status enum.

Traces: sesv2-identity-certificate-association-lifecycle#task#5 (ListEmailIdentityCertificates, 200 vs 200, $.Certificates[0].Status).

## Evidence
Fixture sesv2-identity-certificate-association-lifecycle, step task#5, operation ListEmailIdentityCertificates, verdict mismatch.

Expected (recorded AWS): HTTP 200, $.Certificates[0].Status = "FAILED".
Actual (emulator):       HTTP 200, $.Certificates[0].Status = "ACTIVE".

Replay detail verbatim: "$.Certificates[0].Status: value_mismatch (expected 'FAILED', got 'ACTIVE')".

Status code and every other member of the certificate entry agree; the single diverging field is the modeled enum IdentityCertificateStatus. Both 'FAILED' and 'ACTIVE' are fixed modeled enum tokens, not per-run values — the requirement is the enum token itself, not any identity name, certificate ARN or timestamp from this recording (those are request/state-derived and vary per environment). Modeled domain: PROVISIONING | INACTIVE | DEPROVISIONING | ACTIVE | FAILED; FAILED is documented as "the certificate couldn't be prepared for use, or the certificate has expired", ACTIVE as "the certificate is ready to use for signing". Divergence cluster key: service=sesv2|ListEmailIdentityCertificates|status=200->200|keys=$.Certificates[0].Status, affected_count 1, owned_by [] (net-new, no stub).

## Steps to Reproduce
1. Clone the repo and boot the emulator built from it.
2. Replay wire fixture sesv2-identity-certificate-association-lifecycle from trace set simparity-wire-aws-v1.
3. The fixture creates an email identity (setup#1), associates an S/MIME certificate with it via AssociateEmailIdentityCertificate (POST /v2/email/identity/certificates) using a certificate that AWS cannot prepare for signing use, then calls ListEmailIdentityCertificates (POST /v2/email/identity/certificates/list) at step task#5.
4. Inspect $.Certificates[0].Status in the task#5 response body.
5. The emulator answers 200 with Status "ACTIVE"; the recorded AWS response is 200 with Status "FAILED". The association never leaves the constant ACTIVE value, so no FAILED (or PROVISIONING/DEPROVISIONING) status is ever observable.
