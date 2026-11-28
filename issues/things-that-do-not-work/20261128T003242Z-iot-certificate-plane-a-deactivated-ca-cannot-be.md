Issue-Source: trace-sweep-divergence
Issue-Trace: iot-ca-certificate-crud
Issue-Step: teardown#7
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: iot-ca-certificate-crud iot-ca-certificate-lifecycle iot-ca-certificate-lifecycle-with-device-cert iot-certificate-registration-methods iot-iot-ca-certificate-lifecycle iot-register-certificate-with-ca iot-register-certificate-with-ca-chain iot-iot-registration-code-lifecycle iot-registration-code-lifecycle
Issue-Shape-Id: com.amazonaws.iot#AWSIotService
Issue-Key: iot|DeleteCACertificate|2xx->4xx|status
Issue-Key: iot|DeleteCACertificate|4xx->4xx|$.message
Issue-Key: iot|DeleteRegistrationCode|2xx->4xx|$
Issue-Key: iot|DeleteRegistrationCode|2xx->4xx|status
Issue-Key: iot|RegisterCertificate|4xx->4xx|$.message
Issue-Commit: 244eae870d957920c579d40eef12d136da6f5bab

# iot certificate plane: a deactivated CA cannot be deleted, DeleteRegistrationCode 404s, and two refusal messages are emulator-authored

> _Note: this issue was identified against the code base at commit `244eae870d957920c579d40eef12d136da6f5bab`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DeleteCACertificate

## Description
Four independent net-new root causes in the iot certificate/registration plane. Each cause must be closed before submitting. Related but NOT covering these: open issue 20261118T071347Z-iot-ca-certificate-plane-diverges-updatecacertificate-never-applies.md owns the UpdateCACertificate no-op, the delete-leaves-the-CA-readable cause and the not-found wording (the replay reports those clusters as owned by it), and open issue 20261126T195127Z-iot-deletecacertificate-s-certificatestateexception-message-drops-the-ca.md is a BEHAVIORAL issue naming the same wording as Cause 2 here but carrying no wire replay key — the replay reports all four clusters below as unowned.

Cause 1 of 4: DeleteCACertificate refuses to delete a CA certificate that the caller already deactivated.
AWS: Smithy DeleteCACertificate (DELETE /cacertificate/{certificateId}, certificateId @httpLabel @required @length 64..64 @pattern ^(0x)?[a-fA-F0-9]+$) returns an empty body with HTTP 200 and models CertificateStateException (406) only for a CA whose status is still ACTIVE. Recorded behavior: after UpdateCACertificate sets newStatus=INACTIVE, DeleteCACertificate on that id answers HTTP 200 with an empty body.
Emulator: the delete is refused with HTTP 406 CertificateStateException, so the CA can never be removed once registered.
Why: every CA-certificate teardown is impossible — a client that follows AWS's documented deactivate-then-delete sequence is blocked, leaves the CA registered, and then collides on the next registration of the same CA. 7 recorded teardown steps across 7 fixtures hit it.
Hypothesis: the refusal is downstream of the owned UpdateCACertificate no-op (issue 20261118T071347Z): because the deactivation is never applied, the stored CA is still ACTIVE when the delete is evaluated. The delete path must honour the stored status after an applied update, and the fix must be verified by re-replaying this step, which that issue's pointer does not cover.
Fix: DeleteCACertificate's state precondition and the CA record's status it reads (services/service-com-amazonaws-iot-awsiotservice/src/ops/delete_ca_certificate.rs plus the CA-certificate state record written by update_ca_certificate.rs).
Traces: iot-ca-certificate-crud#teardown#7 (cluster of 7: also iot-ca-certificate-lifecycle#teardown#5, iot-ca-certificate-lifecycle-with-device-cert#teardown#11, iot-certificate-registration-methods#teardown#10, iot-iot-ca-certificate-lifecycle#teardown#10, iot-register-certificate-with-ca#teardown#8, iot-register-certificate-with-ca-chain#teardown#7).

Cause 2 of 4: the still-ACTIVE refusal carries the device-certificate sentence, dropping the "CA " qualifier.
AWS: deleting a CA certificate whose status is ACTIVE is refused HTTP 406, error code CertificateStateException, message "CA Certificate must be deactivated (not ACTIVE) before deletion. Id: <certificateId>". Fixed text: "CA Certificate must be deactivated (not ACTIVE) before deletion. Id: "; the only variable part is the certificateId from the request path. The CA-plane wording is deliberately distinct from the device-certificate plane's DeleteCertificate refusal, which has no "CA " qualifier.
Emulator: status 406 and code CertificateStateException are right, but the message is "Certificate must be deactivated (not ACTIVE) before deletion. Id: <certificateId>" — the device-certificate sentence.
Why: a client that classifies refusals by message text cannot tell which plane refused it, and the two are remediated differently (UpdateCACertificate vs UpdateCertificate).
Fix: the CA-plane refusal construction in DeleteCACertificate — do not reuse the device-certificate message builder shared with DeleteCertificate (services/service-com-amazonaws-iot-awsiotservice/src/ops/delete_ca_certificate.rs).
Traces: iot-ca-certificate-lifecycle-with-device-cert#task#4 (cluster of 2: also iot-iot-ca-certificate-lifecycle#task#5).

Cause 3 of 4: DeleteRegistrationCode refuses with 404 where AWS answers 200.
AWS: Smithy DeleteRegistrationCode (DELETE /registrationcode) takes no input and returns an empty body; ResourceNotFoundException (404) is modeled. Recorded behavior: after the account's registration code has been obtained with GetRegistrationCode, DeleteRegistrationCode answers HTTP 200 with an empty body {}.
Emulator: HTTP 404 with body {"__type": "ResourceNotFoundException", "message": "The registration code does not exist."} — the account's registration code is reported absent immediately after it was served.
Why: the CA-registration workflow (get a registration code, sign it into a verification certificate, then clear the code) cannot be completed or cleaned up; a client sees the delete fail for a code it just read.
Hypothesis: GetRegistrationCode mints the code without recording it in the account's state (or records it under a different key), so the delete path finds nothing to remove.
Fix: DeleteRegistrationCode's lookup and the account registration-code state GetRegistrationCode writes (services/service-com-amazonaws-iot-awsiotservice/src/ops/delete_registration_code.rs plus get_registration_code.rs and the shared account state).
Traces: iot-iot-registration-code-lifecycle#task#4 (cluster of 2: also iot-registration-code-lifecycle#task#3).

Cause 4 of 4: RegisterCertificate's duplicate refusal message is emulator-authored.
AWS: Smithy RegisterCertificate (POST /certificate/register) models ResourceAlreadyExistsException (httpError 409). Recorded behavior: registering a certificate PEM that is already provisioned or registered is refused 409 with the message "The certificate is already provisioned or registered" — the whole sentence is fixed text, with NO variable part; in particular the certificateId is not interpolated into the message (ResourceAlreadyExistsException carries resourceId/resourceArn members for that).
Emulator: status 409 is right, but the message is "Certificate already exists (certificateId: <64-hex id>)" — different fixed text plus a parenthetical the real service never emits.
Why: exact-match clients, SDK test suites and log scrapers keyed to AWS's sentence never match, and the emulator's message implies the id is part of the contract when AWS's is not.
Fix: RegisterCertificate's duplicate-detection refusal construction (services/service-com-amazonaws-iot-awsiotservice/src/ops/register_certificate.rs).
Traces: iot-register-certificate-with-ca#task#4 (cluster of 2: also iot-register-certificate-with-ca-chain#task#3).

## Evidence
Wire corpus simparity-wire-aws-v1 replayed against the emulator built from this checkout. The 64-hex ids and probe names below are one run's values filling the templates described per cause.

Cause 1 — iot-ca-certificate-crud#teardown#7, DeleteCACertificate: expected status 200; actual status 406. Cluster of 7 teardown steps (iot-ca-certificate-lifecycle#teardown#5, iot-ca-certificate-lifecycle-with-device-cert#teardown#11, iot-certificate-registration-methods#teardown#10, iot-iot-ca-certificate-lifecycle#teardown#10, iot-register-certificate-with-ca#teardown#8, iot-register-certificate-with-ca-chain#teardown#7). The same fixtures' earlier DescribeCACertificate steps show the deactivation was not applied (status ACTIVE, customerVersion 1 where INACTIVE/2 was expected) — that read is owned by issue 20261118T071347Z; the delete refusal is not.

Cause 2 — iot-ca-certificate-lifecycle-with-device-cert#task#4, DeleteCACertificate, status 406 both sides:
  expected $.message "CA Certificate must be deactivated (not ACTIVE) before deletion. Id: ae1a79a31677aa5f8d37e79e66a39d680d56367c3cd3b5ed667c0f3012824f0e"
  actual   $.message "Certificate must be deactivated (not ACTIVE) before deletion. Id: ae1a79a31677aa5f8d37e79e66a39d680d56367c3cd3b5ed667c0f3012824f0e"
  Fixed text differs by the missing leading "CA "; the id is the request's certificateId.

Cause 3 — iot-iot-registration-code-lifecycle#task#4, DeleteRegistrationCode: expected status 200 with body {}; actual status 404 with body {"__type": "ResourceNotFoundException", "message": "The registration code does not exist."} (cluster of 2: iot-registration-code-lifecycle#task#3).

Cause 4 — iot-register-certificate-with-ca#task#4, RegisterCertificate, status 409 both sides:
  expected $.message "The certificate is already provisioned or registered"
  actual   $.message "Certificate already exists (certificateId: 17d718ccf823efd831413d0d98f19d3aa607c4c3803fe9597ee54a9dd1928ee0)"
  AWS's sentence has no variable part; the emulator appends one run's certificateId.

Model citations: DeleteCACertificate DELETE /cacertificate/{certificateId} -> empty output; errors CertificateStateException(406), ResourceNotFoundException(404), InvalidRequestException(400), UnauthorizedException(401), ThrottlingException(400), InternalFailureException(500), ServiceUnavailableException(503). DeleteRegistrationCode DELETE /registrationcode, no input, empty output; errors ResourceNotFoundException(404), UnauthorizedException(401), ThrottlingException(400), InternalFailureException(500), ServiceUnavailableException(503). RegisterCertificate POST /certificate/register, required certificatePem; errors ResourceAlreadyExistsException(409), CertificateConflictException(409), CertificateStateException(406), CertificateValidationException(400), InvalidRequestException(400).

## Steps to Reproduce
1. git clone the repo and boot the emulator built from this checkout.
2. RegisterCACertificate with a self-signed CA PEM (setAsActive=true); keep the certificateId.
3. UpdateCACertificate(certificateId, newStatus='INACTIVE'), confirm DescribeCACertificate reports INACTIVE, then DELETE /cacertificate/<certificateId> — AWS answers 200 with an empty body; the emulator answers 406 CertificateStateException (Cause 1).
4. With the CA still ACTIVE, DELETE /cacertificate/<certificateId> — the 406 message must read "CA Certificate must be deactivated (not ACTIVE) before deletion. Id: <certificateId>"; the emulator drops the "CA " qualifier (Cause 2).
5. GET /registrationcode, then DELETE /registrationcode — AWS answers 200 {}; the emulator answers 404 ResourceNotFoundException "The registration code does not exist." (Cause 3).
6. RegisterCertificate with a leaf PEM, then RegisterCertificate with the same PEM again — the 409 message must read exactly "The certificate is already provisioned or registered" (Cause 4).
7. Re-replay iot-ca-certificate-crud#teardown#7, iot-ca-certificate-lifecycle-with-device-cert#task#4, iot-iot-registration-code-lifecycle#task#4 and iot-register-certificate-with-ca#task#4 to confirm the fix.
