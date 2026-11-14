Issue-Source: trace-sweep-divergence
Issue-Trace: acm-certificate-domain-validations-email-method
Issue-Step: task#2
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: acm-certificate-domain-validations-email-method
Issue-Shape-Id: com.amazonaws.acm#CertificateManager
Issue-Key: acm|ListCertificateDomainValidations|2xx->2xx|$.DomainValidationSummaryList[*].RequestedValidationConfiguration.ValidationStatus
Issue-Related: 20261029T192604Z-acm-certificate-read-projections-omit-and-mis-name.md
Issue-Commit: fb003e0a3fdd2a31392ea928ed9540deb9fe4c34

# ACM ListCertificateDomainValidations omits RequestedValidationConfiguration for EMAIL-validated certificates

> _Note: this issue was identified against the code base at commit `fb003e0a3fdd2a31392ea928ed9540deb9fe4c34`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListCertificateDomainValidations

## Description
One root cause, on the EMAIL-validation leg of ListCertificateDomainValidations. The step answered HTTP 200 with the right envelope; only the body member under which the per-domain validation configuration is reported diverges, and because the fixture polls that member the step never converges.

Cause 1 of 1: for a certificate requested with ValidationMethod EMAIL, ListCertificateDomainValidations reports each per-domain configuration under ActiveValidationConfiguration instead of RequestedValidationConfiguration, so the member AWS populates is absent and a poll on its ValidationStatus never resolves.
AWS: Smithy (services/service-com-amazonaws-acm-certificatemanager/smithy-model/com-amazonaws-acm-certificatemanager.json) DomainValidationSummary{DomainName required, ActiveValidationConfiguration: ValidationConfiguration, RequestedValidationConfiguration: ValidationConfiguration}; ValidationConfiguration{ValidationMethod: ValidationMethod enum (DNS|EMAIL|HTTP), ValidationChallenge: ValidationChallenge, ValidationStatus: DomainStatus enum (PENDING_VALIDATION|SUCCESS|FAILED)}; ValidationChallenge is a union{EmailValidationChallenge, DnsValidationChallenge}; EmailValidationChallenge{ValidationEmails: ValidationEmailList, ValidationDomain}; the operation is paginated on DomainValidationSummaryList. Recorded behavior: for an EMAIL-validated certificate, every element of DomainValidationSummaryList carries exactly DomainName plus RequestedValidationConfiguration{ValidationMethod: 'EMAIL', ValidationStatus: <the domain's current DomainStatus, 'FAILED' in the recording>, ValidationChallenge: {EmailValidationChallenge: {ValidationDomain: <the certificate's validation domain>, ValidationEmails: []}}}; ActiveValidationConfiguration is absent, and ValidationEmails is an EMPTY list because ACM has not mailed anything for this request. The recording is the requirement here: the shape docs describe ActiveValidationConfiguration as the configuration 'currently in effect' and RequestedValidationConfiguration as a pending migration, but what AWS actually serves on this operation for a freshly requested certificate is the Requested member, so the model's prose does not license the emulator's choice.
Emulator: each summary carries ActiveValidationConfiguration with the same ValidationMethod 'EMAIL', the same ValidationStatus and the same EmailValidationChallenge{ValidationEmails: [], ValidationDomain}; RequestedValidationConfiguration is absent entirely, so $.DomainValidationSummaryList[0].RequestedValidationConfiguration.ValidationStatus reads None.
Why: a client that reads summary['RequestedValidationConfiguration'] — the member AWS populates for a requested certificate — gets None/KeyError, so the per-domain validation method, the email challenge (validation domain and dispatched addresses) and the validation status are all invisible through this operation. Worse, a client that polls that path for a terminal DomainStatus (SUCCESS or FAILED) never observes one: the member stays absent forever, so email-validation automation keyed on this operation hangs instead of reporting the outcome, which the replay reproduces as 60 poll attempts all reading None while the emulator's own state already says FAILED.
Fix: the ListCertificateDomainValidations response projection in the ACM service crate — the DomainValidationSummary serializer must emit the per-domain configuration under RequestedValidationConfiguration (and leave ActiveValidationConfiguration unset) for a requested certificate, on the EMAIL path as well as the DNS path.
Traces: acm-certificate-domain-validations-email-method#task#2 (single-member cluster; no other fixture step is affected).

Related open filing, which does NOT carry this fixture step as a replay key: 20261029T192604Z-acm-certificate-read-projections-omit-and-mis-name.md. Its cause 1 covers the same mis-named member on the DNS path (and additionally the spurious DnsValidationChallenge.ResourceRecord), and its Issue-Trace-Ids mention this fixture id but bind no step of it, so its replay never exercises the EMAIL leg or the poll. The EMAIL leg has its own serializer branch (EmailValidationChallenge rather than DnsValidationChallenge) and must be re-replayed on its own binding.

## Evidence
Replay of trace set simparity-wire-aws-v1 against the booted emulator (79 fixtures, 393 steps, 358 passed, 34 mismatched). This cluster: fixture acm-certificate-domain-validations-email-method, step task#2, operation ListCertificateDomainValidations, verdict=mismatch, expected_status 200, actual_status 200, affected_count 1, stub_path empty, owned_by empty. divergence_key: service=acm|ListCertificateDomainValidations|status=200->200|keys=$.DomainValidationSummaryList[0],$.DomainValidationSummaryList[0].RequestedValidationConfiguration.ValidationStatus.

Detail as reported:
- $.DomainValidationSummaryList[0]: missing — expected {'DomainName': '<cert domain>', 'RequestedValidationConfiguration': {'ValidationChallenge': {'EmailValidationChallenge': {'ValidationDomain': '<cert domain>', 'ValidationEmails': []}}, 'ValidationMethod': 'EMAIL', 'ValidationStatus': 'FAILED'}}, got {'DomainName': '<cert domain>', 'ActiveValidationConfiguration': {'ValidationMethod': 'EMAIL', 'ValidationChallenge': {'EmailValidationChallenge': {'ValidationEmails': [], 'ValidationDomain': '<cert domain>'}}, 'ValidationStatus': 'FAILED'}}.
- $.DomainValidationSummaryList[0].RequestedValidationConfiguration.ValidationStatus: poll: not in ['FAILED'] after 60 attempt(s) (expected 'FAILED', got None).

The generated domain name in the recording (sim2real-probe-acm-emailcert-<hex>.example.com) and the certificate ARN are per-run values and are not part of the requirement; the requirement is the member name RequestedValidationConfiguration, the absence of ActiveValidationConfiguration, the EmailValidationChallenge union arm carrying ValidationDomain plus an empty ValidationEmails list, and that the ValidationStatus reachable at that path reaches the domain's terminal DomainStatus.

Smithy citations: ListCertificateDomainValidations input {CertificateArn required, NextToken, MaxItems}, output {DomainValidationSummaryList, NextToken}, errors AccessDeniedException/InvalidArgsException/ResourceNotFoundException/ThrottlingException; DomainValidationSummary{DomainName required, ActiveValidationConfiguration, RequestedValidationConfiguration}; ValidationConfiguration{ValidationMethod, ValidationChallenge, ValidationStatus}; ValidationChallenge union{EmailValidationChallenge, DnsValidationChallenge}; EmailValidationChallenge{ValidationEmails, ValidationDomain}.

## Steps to Reproduce
1. Boot the emulator and replay trace set simparity-wire-aws-v1; the cluster reproduces as verdict=mismatch at HTTP 200.
2. acm.request_certificate(DomainName='<d>', ValidationMethod='EMAIL') to get <arn>.
3. acm.list_certificate_domain_validations(CertificateArn=<arn>) and read DomainValidationSummaryList[0]: the emulator returns the configuration under 'ActiveValidationConfiguration' and no 'RequestedValidationConfiguration'; AWS returns it under 'RequestedValidationConfiguration' with ValidationMethod 'EMAIL', an EmailValidationChallenge carrying ValidationDomain and an empty ValidationEmails list, and no 'ActiveValidationConfiguration'.
4. Poll DomainValidationSummaryList[0]['RequestedValidationConfiguration']['ValidationStatus'] until it reaches a terminal DomainStatus: against the emulator it is None on every attempt (60 attempts in the fixture) even once the certificate's domain validation has reached FAILED.
5. Re-replay acm-certificate-domain-validations-email-method#task#2; it must pass before closing.
