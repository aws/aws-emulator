Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: sesv2.delete-email-identity-blocked-while-certificate-associated
Issue-Shape-Id: com.amazonaws.sesv2#SimpleEmailService_v2
Issue-Key: sesv2.delete-email-identity-blocked-while-certificate-associated|run#13|AssertionError|assert blocked != "DELETED", (
Issue-Commit: 33932bf6f81e8cc72c4071b6c64638fb9bd78d71

# sesv2 DeleteEmailIdentity is not refused while the identity holds a certificate association whose preparation FAILED (or is INACTIVE), so the identity is destroyed instead of guarded

> _Note: this issue was identified against the code base at commit `33932bf6f81e8cc72c4071b6c64638fb9bd78d71`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DeleteEmailIdentity

## Description
One net-new root cause in the SESv2 identity/certificate surface. This issue bundles 1 cause; it must be closed before submitting.

Cause 1 of 1 — the DeleteEmailIdentity certificate guard consults only associations the emulator believes can SIGN, so an association whose certificate could not be resolved/prepared does not hold the identity back, and the delete succeeds where AWS refuses it.

AWS: Smithy declares DeleteEmailIdentity input {EmailIdentity (httpLabel, required, @length min 1)} with errors BadRequestException (400), ConcurrentModificationException (500), NotFoundException (404), TooManyRequestsException (429); AssociateEmailIdentityCertificate takes {EmailIdentity (required), FromAddress, CertificateArn (required, @length 20..2048, pattern ^arn:[\w+=/,.@-]+:[\w+=/,.@-]+:[\w+=/,.@-]*:[0-9]+:certificate/[\w+=,.@-]+$)} and models AlreadyExistsException/BadRequestException/NotFoundException/TooManyRequestsException; ListEmailIdentityCertificates returns Certificates (IdentityCertificateList). Beyond the model this is recorded behavior only: AWS ACCEPTS an association whose CertificateArn is a well-formed ARN for a certificate the account does not hold (the recording associates an all-zeros certificate UUID ARN and the call succeeds), and from then on the identity is HELD by that association record. DeleteEmailIdentity is then refused HTTP 400, error code BadRequestException, message `Cannot delete <ARN> because it has certificates associated with it. Disassociate all certificates and try again.` — fixed text `Cannot delete <` + `> because it has certificates associated with it. Disassociate all certificates and try again.`, whose single variable part is the identity's own ARN, i.e. `arn:aws:ses:<region>:<account>:identity/<EmailIdentity>` built from the signed Region, the caller's account and the identity named in the request (never a literal region/account/domain from one run). The refusal has no effect: a following GetEmailIdentity still reports the identity (IdentityType DOMAIN). After DisassociateEmailIdentityCertificate the association RECORD REMAINS, listed by ListEmailIdentityCertificates with Status INACTIVE, disassociation is idempotent (a second call does not raise), and the same delete guard keeps firing on the INACTIVE record for tens of seconds to minutes before DeleteEmailIdentity eventually succeeds; after that success GetEmailIdentity answers 404 NotFoundException `Email identity <IDENTITY> does not exist.` In short, the gate AWS applies is "the identity holds a certificate association record at all", not "the identity holds an association that can sign".

Emulator: AssociateEmailIdentityCertificate resolves the CertificateArn against the emulator's own Certificate Manager store (crate::state::certificate_can_sign); an ARN that store holds nothing under yields certificate_is_usable = false, so the association record is minted directly in the terminal FAILED status. The delete guard then tests `record.certificates.values().any(IdentityCertificateRecord::signs)`, and `signs()` is true only for PROVISIONING | ACTIVE — FAILED and INACTIVE are excluded by construction. Consequently the immediately following DeleteEmailIdentity is ACCEPTED with HTTP 200 and an empty body, and the identity is removed from both this service's table and the shared account identity registry. The check dies on its first assertion: `AssertionError: DeleteEmailIdentity succeeded while a certificate was associated with the identity; real AWS must refuse this`, so the BadRequestException wording, the "refusal has no effect" read-back, the INACTIVE listing after disassociation and the eventual-success polling are all left unexercised. The same narrowing means a disassociated (INACTIVE) association never holds the identity either, so the eventual-success behavior cannot be distinguished from immediate success.

Why: a client relying on AWS's guard loses data silently — it deletes an identity that still carries S/MIME certificate associations, where AWS would have refused the call and required DisassociateEmailIdentityCertificate first. Teardown/cleanup code that treats the BadRequestException as "disassociate, then retry" never takes that branch, and any client that probes the guard to discover whether certificates are still attached is told there are none. Because the emulator's FAILED status is derived from whether another service's store happens to hold the ARN, the identity's deletability depends on cross-service state AWS does not consult at this point.

Fix: the DeleteEmailIdentity handler's certificate gate (services/service-com-amazonaws-sesv2-simpleemailservice-v2/src/ops/delete_email_identity.rs) must refuse while the identity holds ANY certificate association record — including one whose preparation ended FAILED and one left INACTIVE by a disassociation — rather than only ones the record's `signs()` predicate admits; correspondingly the association record's lifecycle in src/state/email_identities.rs (IdentityCertificateRecord::signs / in_force and the status minted by IdentityCertificateRecord::new from crate::state::certificate_can_sign) is where "does this association hold the identity" has to stop being "can this association sign".

Traces: sesv2.delete-email-identity-blocked-while-certificate-associated.

## Evidence
Behavioral suite simparity-behavioral-aws-v1: 212/252 passed; sesv2 129/150. Failing check sesv2.delete-email-identity-blocked-while-certificate-associated (verdict: mismatch).

Sequence replayed: CreateEmailIdentity(EmailIdentity=<generated>.example.com) -> AssociateEmailIdentityCertificate(EmailIdentity=<domain>, FromAddress=probe@<domain>, CertificateArn=arn:aws:acm:<region>:<account>:certificate/00000000-0000-0000-0000-000000000000) [accepted] -> DeleteEmailIdentity(EmailIdentity=<domain>).

Expected (AWS): HTTP 400, Error.Code BadRequestException, Error.Message 'Cannot delete <arn:aws:ses:<region>:<account>:identity/<domain>> because it has certificates associated with it. Disassociate all certificates and try again.' and the identity still readable by GetEmailIdentity with IdentityType DOMAIN.
Actual (emulator): HTTP 200 with an empty body; the identity is deleted.
Failure: `AssertionError: DeleteEmailIdentity succeeded while a certificate was associated with the identity; real AWS must refuse this` (check_sesv2_delete_email_identity_blocked_while_certificate_associated.py line 74).

Code path confirming the cause: src/ops/delete_email_identity.rs gates on `record.certificates.values().any(crate::state::IdentityCertificateRecord::signs)`; src/state/email_identities.rs defines `signs()` as `matches!(self.status, CertificateStatus::Provisioning | CertificateStatus::Active)` and `IdentityCertificateRecord::new` mints CertificateStatus::Failed whenever `crate::state::certificate_can_sign(...)` is false, which src/state/identity_certificates.rs returns for any ARN the Certificate Manager store holds no certificate under. The region/account/domain values above are this run's; the requirement is the ARN template and the message text, not those values.

Not exercised because the check stops at the first assertion (same cause): the refusal wording, the post-refusal GetEmailIdentity read-back, ListEmailIdentityCertificates reporting the disassociated record with Status INACTIVE, idempotent DisassociateEmailIdentityCertificate, and DeleteEmailIdentity succeeding only after the INACTIVE record is cleaned up.

## Steps to Reproduce
1. Build and boot the emulator and replay the behavioral suite; re-run exactly the check in trace_ids (sesv2.delete-email-identity-blocked-while-certificate-associated).
2. CreateEmailIdentity with a fresh domain identity.
3. AssociateEmailIdentityCertificate(EmailIdentity=<domain>, FromAddress=probe@<domain>, CertificateArn=<well-formed ACM certificate ARN for a certificate the account does not hold>) — expect it to be accepted.
4. DeleteEmailIdentity(EmailIdentity=<domain>) — expect HTTP 400 BadRequestException with 'Cannot delete <arn:aws:ses:<region>:<account>:identity/<domain>> because it has certificates associated with it. Disassociate all certificates and try again.'; today it returns HTTP 200 and deletes the identity.
5. GetEmailIdentity(EmailIdentity=<domain>) — expect the identity to still be present with IdentityType DOMAIN after the refused delete.
6. DisassociateEmailIdentityCertificate(EmailIdentity=<domain>, FromAddress=probe@<domain>) twice — both must succeed (idempotent); ListEmailIdentityCertificates(EmailIdentity=<domain>) must still list the record with Status INACTIVE.
7. Poll DeleteEmailIdentity: it must keep being refused with the same BadRequestException while the INACTIVE record is being cleaned up, then succeed; afterwards GetEmailIdentity must answer 404 NotFoundException 'Email identity <<domain>> does not exist.'
