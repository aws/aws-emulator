Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: apigatewayv2.api-mapping-key-non-ascii-rejected apigatewayv2.create-api-mapping-malformed-path-rejected apigatewayv2.create-api-mapping-missing-stage-rejected apigatewayv2.concurrent-api-mapping-same-path-conflict apigatewayv2.create-routing-rule-missing-condition-rejected apigatewayv2.create-domain-name-duplicate-bad-request apigatewayv2.domain-name-security-policy-server-enforced apigatewayv2.domain-name-mtls-ownership-verification-enforced apigatewayv2.e2e-custom-domain-mapping-invoke-flow
Issue-Shape-Id: com.amazonaws.apigatewayv2#ApiGatewayV2
Issue-Key: apigatewayv2.api-mapping-key-non-ascii-rejected|run#16|AssertionError|assert code == "BadRequestException", code
Issue-Key: apigatewayv2.create-api-mapping-malformed-path-rejected|run#6|AssertionError|assert code == "BadRequestException", (
Issue-Key: apigatewayv2.create-api-mapping-missing-stage-rejected|run#11|AssertionError|assert code == "BadRequestException", f"expected BadRequestException, got {code!r}"
Issue-Key: apigatewayv2.concurrent-api-mapping-same-path-conflict|run#12|AssertionError|assert len(successes) == 1, f"expected exactly one winning CreateApiMapping, got {len(successes)}"
Issue-Key: apigatewayv2.create-routing-rule-missing-condition-rejected|run#6|AssertionError|assert code == "BadRequestException", f"expected BadRequestException, got {code!r}"
Issue-Key: apigatewayv2.create-domain-name-duplicate-bad-request|run#11|AssertionError|assert code == "BadRequestException", f"expected BadRequestException, got {code!r}: {message!r}"
Issue-Key: apigatewayv2.domain-name-security-policy-server-enforced|_error#11|AssertionError|raise AssertionError(f"expected the call to be rejected, but it succeeded with {result!r}")
Issue-Key: apigatewayv2.domain-name-mtls-ownership-verification-enforced|_error_code#21|AssertionError|raise AssertionError(f"expected BadRequestException, but the call SUCCEEDED and returned {result!r}")
Issue-Key: apigatewayv2.e2e-custom-domain-mapping-invoke-flow|run#9|KeyError|gw_zone = cfg["HostedZoneId"]
Issue-Commit: 1030924aed157d95218fdb79c11a6b06e677df02

# apigatewayv2: custom-domain and API-mapping control plane — 9 fidelity defects (CreateApiMapping/CreateRoutingRule answer NotImplemented, duplicate-domain uses the wrong error code, UpdateDomainName accepts what AWS refuses, GetDomainName omits HostedZoneId)

> _Note: this issue was identified against the code base at commit `1030924aed157d95218fdb79c11a6b06e677df02`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateApiMapping, CreateRoutingRule, CreateDomainName, UpdateDomainName, GetDomainName

## Description
Nine distinct root causes in the apigatewayv2 custom-domain / API-mapping control plane, each grounded in a failing behavioral check of this suite. A fixer must close ALL NINE before submitting; they are numbered and each carries its own failing check ids.

Related open issues that do NOT cover these causes and do not carry these checks' replay keys: 20261024T120951Z-apigatewayv2-createapimapping-stub.md and 20261024T120951Z-apigatewayv2-createroutingrule-stub.md (they name the stub handlers but carry none of the validation rules or the replay keys below), 20261024T120951Z-apigatewayv2-the-execute-api-data-plane-never-serves.md, 20261025T041541Z-apigatewayv2-refusal-messages-do-not-name-the-blocking.md.

=== Cause 1 of 9: CreateApiMapping answers NotImplemented, so the ApiMappingKey character-class rule is never enforced ===
AWS: Smithy (services/service-com-amazonaws-apigatewayv2-apigatewayv2/smithy-model): CreateApiMapping POST /v2/domainnames/{DomainName}/apimappings, success code 201, input ApiMappingKey -> SelectionKey, modeled errors BadRequestException (client, httpError 400), ConflictException (409), NotFoundException (404), TooManyRequestsException (429). Recorded behavior for the text: an ApiMappingKey containing a non-ASCII character is refused with BadRequestException and the constant message "An API mapping key may contain only letters, numbers and one of $-_.+!*'(), characters." — fixed text for the whole illegal-character class, naming nothing from the request — and no mapping is created, while an ASCII key (e.g. letters/digits/'.') is accepted and round-trips byte-identical through GetApiMapping.
Emulator: the call is refused with error code NotImplemented, i.e. the operation is an unimplemented handler, so neither the refusal nor the positive control can happen.
Why: a client cannot attach any custom domain to a stage at all, and when the handler is written the character-class rule must come with it, otherwise illegal keys are persisted and the same template fails on AWS.
Fix: implement the CreateApiMapping handler (ops/create_api_mapping.rs) and its ApiMappingKey validation.
Traces: apigatewayv2.api-mapping-key-non-ascii-rejected.

=== Cause 2 of 9: the ApiMappingKey path-shape rule (no leading '/', no consecutive '/') is not enforced ===
AWS: Smithy as above (BadRequestException 400 on CreateApiMapping). Recorded behavior for the text: an ApiMappingKey that begins with '/' is refused with BadRequestException and the constant message "API mapping key should not start with a '/' or have consecutive '/'s.", nothing is persisted, and a well-formed key still maps successfully.
Emulator: error code NotImplemented instead of BadRequestException (same unimplemented handler as cause 1), so the rule is unreachable.
Why: callers that build base-path mappings from user input rely on this refusal to catch malformed base paths; an emulator that accepts them (or refuses everything with NotImplemented) cannot reproduce either outcome.
Fix: the leading/consecutive-slash validation in the CreateApiMapping handler.
Traces: apigatewayv2.create-api-mapping-malformed-path-rejected.

=== Cause 3 of 9: the Stage-reference rule on CreateApiMapping is not enforced ===
AWS: Smithy: CreateApiMapping input Stage -> StringWithLengthBetween1And128, required. Recorded behavior: Stage="" (present but not a usable stage reference) is refused with BadRequestException and the constant message "Invalid stage identifier specified"; GetApiMappings on the domain then returns an empty Items list, and the same call with a real stage reference ($default) succeeds and is visible through GetApiMappings.
Emulator: error code NotImplemented, so the empty-stage input is neither refused with AWS's code/message nor laundered into a mapping — nothing about the rule is observable.
Why: a client that mistypes or templates an empty stage gets an off-model error code (NotImplemented is not in CreateApiMapping's modeled error set at all), so botocore's typed BadRequestException never matches and error handling written against AWS breaks.
Fix: the Stage-reference validation in the CreateApiMapping handler.
Traces: apigatewayv2.create-api-mapping-missing-stage-rejected.

=== Cause 4 of 9: two concurrent CreateApiMapping calls for the same DomainName+ApiMappingKey do not resolve to exactly one winner ===
AWS: Smithy: CreateApiMapping models ConflictException (client, httpError 409). Recorded behavior: when two CreateApiMapping calls race for the same domain and the same mapping key (different stages), exactly ONE returns 201 and the other is refused with ConflictException; GetApiMappings afterwards holds exactly one mapping for that key, the winner's.
Emulator: BOTH calls are refused (0 successes, assertion 'expected exactly one winning CreateApiMapping, got 0') because the handler answers NotImplemented, so there is no uniqueness decision and no mapping state at all.
Why: the domain+key uniqueness invariant is what makes base-path routing deterministic; a client that treats a 409 as "someone else won" and reads the mapping back finds nothing here.
Fix: the CreateApiMapping handler's uniqueness/conflict path — key the mapping on (DomainName, ApiMappingKey) in state and refuse the loser with ConflictException.
Traces: apigatewayv2.concurrent-api-mapping-same-path-conflict.

=== Cause 5 of 9: CreateRoutingRule answers NotImplemented, so a condition with no match clause is not refused ===
AWS: Smithy: CreateRoutingRule POST /v2/domainnames/{DomainName}/routingrules, success 201, Conditions -> list of RoutingRuleCondition (members MatchBasePaths, MatchHeaders — both optional on the structure), Priority range 1..1000000, modeled errors BadRequestException (400), ConflictException (409), NotFoundException (404), TooManyRequestsException (429). Recorded behavior: a Conditions entry that carries neither MatchBasePaths nor MatchHeaders is refused with BadRequestException, no rule is created (ListRoutingRules shows no rule at that Priority), and a rule with a real condition (MatchBasePaths.AnyOf) is created and readable through GetRoutingRule.
Emulator: error code NotImplemented, so the empty-condition refusal and the positive control are both unreachable.
Why: RoutingRuleCondition's members are optional in the model, so this cross-member "at least one match clause" rule is the service's only guard against a rule that matches nothing; without it (and without the operation at all) no routing-rule configuration can be validated or exercised.
Fix: implement the CreateRoutingRule handler (ops/create_routing_rule.rs) and its Conditions validation.
Traces: apigatewayv2.create-routing-rule-missing-condition-rejected.

=== Cause 6 of 9: a duplicate CreateDomainName is refused with ConflictException instead of BadRequestException, with a request-derived message ===
AWS: Smithy: CreateDomainName POST /v2/domainnames, success 201, models BOTH BadRequestException (400) and ConflictException (409). Recorded behavior (verified against real AWS, and the reason the check asserts it): a domain-name collision is refused with BadRequestException — not the ConflictException the API-reference table implies — and the message is the constant "The domain name you provided already exists.", fixed text that does NOT interpolate the requested domain name; the originally created domain is left untouched.
Emulator: ConflictException (409) with a message built from the request, 'The domain name <requested domain> already exists.' So both the error code and the message template diverge; only the "original resource untouched" half matches.
Why: botocore maps modeled error shapes to typed exceptions — client.exceptions.BadRequestException never matches, so idempotent create-or-adopt logic written against AWS takes the wrong branch, and a caller matching the constant text does not recognise the refusal.
Fix: the duplicate-domain path of the CreateDomainName handler (ops/create_domain_name.rs) — raise BadRequestException with AWS's constant text.
Traces: apigatewayv2.create-domain-name-duplicate-bad-request.

=== Cause 7 of 9: UpdateDomainName accepts a SecurityPolicy downgrade to TLS_1_0 ===
AWS: Smithy: UpdateDomainName PATCH /v2/domainnames/{DomainName}, success 200, DomainNameConfiguration.SecurityPolicy -> SecurityPolicy enum with values [TLS_1_0, TLS_1_2]; modeled errors include BadRequestException (400). Recorded behavior: although TLS_1_0 is a modeled enum value, API Gateway v2 enforces TLS_1_2 as the floor for domain names and refuses the downgrade with BadRequestException and the constant message "TLS_1_0 is not supported for APIGatewayV2 domainName."; GetDomainName afterwards still reports the untouched TLS_1_2 policy.
Emulator: the update SUCCEEDS with HTTP 200 and returns DomainNameConfigurations[0].SecurityPolicy = 'TLS_1_0' (with DomainNameStatus 'UPDATING'), i.e. the floor is not enforced and the weakened policy is persisted.
Why: over-permissiveness — a template or SDK caller can configure a TLS 1.0 custom domain that real AWS refuses outright, so a security-relevant misconfiguration passes locally and fails (or, worse, is assumed applied) in production.
Fix: SecurityPolicy validation in the UpdateDomainName handler (ops/update_domain_name.rs), shared with CreateDomainName's configuration validation.
Traces: apigatewayv2.domain-name-security-policy-server-enforced.

=== Cause 8 of 9: UpdateDomainName accepts a mutual-TLS configuration on an ACM-imported certificate with no OwnershipVerificationCertificateArn ===
AWS: Smithy: UpdateDomainName input MutualTlsAuthentication -> MutualTlsAuthenticationInput (TruststoreUri, TruststoreVersion); DomainNameConfiguration.OwnershipVerificationCertificateArn is documented as "The ARN of the public certificate issued by ACM to validate ownership of your custom domain. Only required when configuring mutual TLS and using an ACM imported or private CA certificate ARN"; modeled errors include BadRequestException (400). Recorded behavior: adding mTLS on top of an ACM-IMPORTED CertificateArn without supplying OwnershipVerificationCertificateArn is refused with BadRequestException and the constant message "Missing ownershipVerificationCertificate. To use an imported or private certificate for the domain name, ownershipVerificationCertificate is required.", and no partial mTLS state is left behind (GetDomainName still reports no MutualTlsAuthentication).
Emulator: the UpdateDomainName call SUCCEEDS (the check's wrapper reports 'the call SUCCEEDED' where a BadRequestException was required), so the documented precondition is not enforced and an mTLS configuration AWS never accepts is applied.
Why: a client gets a domain the emulator reports as mTLS-enabled which cannot exist on AWS; the deploy that "worked" locally fails at the real control plane, and no local test can catch the missing ownership-verification certificate.
Fix: the MutualTlsAuthentication precondition check in the UpdateDomainName handler (and the same rule on CreateDomainName) — when the certificate is ACM-imported/private-CA, require OwnershipVerificationCertificateArn.
Traces: apigatewayv2.domain-name-mtls-ownership-verification-enforced.

=== Cause 9 of 9: GetDomainName's DomainNameConfigurations omit HostedZoneId ===
AWS: Smithy: DomainNameConfiguration member HostedZoneId -> __string, jsonName hostedZoneId, documented "The Amazon Route 53 Hosted Zone ID of the endpoint." Recorded behavior: for a REGIONAL custom domain that has reached DomainNameStatus=AVAILABLE, the configuration carries BOTH ApiGatewayDomainName and HostedZoneId — the hosted-zone id is the tell that the regional target is a Route 53 alias target, and it is the value a client must put in an alias record.
Emulator: the configuration returned once DomainNameStatus is AVAILABLE contains no HostedZoneId member at all (the check dies with KeyError: 'HostedZoneId' reading cfg['HostedZoneId'] straight off the polled configuration; ApiGatewayDomainName is present).
Why: aliasing a custom domain is impossible without it — Route 53 ChangeResourceRecordSets requires the alias target's HostedZoneId, so every client/IaC path that wires a custom domain to DNS (and every check that invokes through one) is blocked at this field.
Fix: the DomainNameConfiguration serializer / domain-name state for CreateDomainName, UpdateDomainName and GetDomainName — populate HostedZoneId for the endpoint type alongside ApiGatewayDomainName.
Traces: apigatewayv2.e2e-custom-domain-mapping-invoke-flow.

## Evidence
Cause 1 — CreateApiMapping(ApiId, DomainName, Stage='prod', ApiMappingKey='caf\u00e9'): expected BadRequestException with the constant message "An API mapping key may contain only letters, numbers and one of $-_.+!*'(), characters."; actual error code 'NotImplemented' (AssertionError: NotImplemented at assert code == "BadRequestException").
Cause 2 — CreateApiMapping(..., ApiMappingKey='/bad-leading-slash'): expected BadRequestException "API mapping key should not start with a '/' or have consecutive '/'s."; actual 'NotImplemented' (AssertionError: a leading-slash ApiMappingKey should be rejected with BadRequestException, got 'NotImplemented').
Cause 3 — CreateApiMapping(ApiId, DomainName, Stage=''): expected BadRequestException "Invalid stage identifier specified"; actual 'NotImplemented' (AssertionError: expected BadRequestException, got 'NotImplemented').
Cause 4 — two concurrent CreateApiMapping calls with the same DomainName and ApiMappingKey='orders' (stages 'dev' and 'prod'): expected exactly 1 success (201) and exactly 1 ConflictException; actual 0 successes (AssertionError: expected exactly one winning CreateApiMapping, got 0), both calls refused by the unimplemented handler.
Cause 5 — CreateRoutingRule(DomainName, Priority=100, Conditions=[{}], Actions=[{InvokeApi:{ApiId,Stage}}]): expected BadRequestException; actual 'NotImplemented' (AssertionError: expected BadRequestException, got 'NotImplemented').
Cause 6 — CreateDomainName for a DomainName that already exists (fresh certificate, so only the name collides): expected BadRequestException with "The domain name you provided already exists."; actual ConflictException with 'The domain name <domain> already exists.' (AssertionError: expected BadRequestException, got 'ConflictException': 'The domain name sim2real-behav-dup....example.com already exists.').
Cause 7 — UpdateDomainName(DomainName, DomainNameConfigurations=[{CertificateArn, EndpointType='REGIONAL', SecurityPolicy='TLS_1_0'}]) on a domain created with TLS_1_2: expected BadRequestException "TLS_1_0 is not supported for APIGatewayV2 domainName."; actual HTTP 200 with DomainNameConfigurations=[{... 'DomainNameStatus': 'UPDATING', 'SecurityPolicy': 'TLS_1_0' ...}] (AssertionError: expected the call to be rejected, but it succeeded).
Cause 8 — UpdateDomainName(DomainName, DomainNameConfigurations=[{CertificateArn=<ACM-imported>, EndpointType='REGIONAL'}], MutualTlsAuthentication={TruststoreUri, TruststoreVersion}) with no OwnershipVerificationCertificateArn: expected BadRequestException "Missing ownershipVerificationCertificate. To use an imported or private certificate for the domain name, ownershipVerificationCertificate is required."; actual the call succeeded (AssertionError: expected BadRequestException, but the call SUCCEEDED). The positive-control legs (CreateDomainName/GetDomainName round-trip of CertificateArn and EndpointType, and no MutualTlsAuthentication on the plain domain) passed.
Cause 9 — GetDomainName(DomainName) polled until DomainNameConfigurations[0].DomainNameStatus == 'AVAILABLE', then reading HostedZoneId: KeyError: 'HostedZoneId' — the member is absent from the configuration (ApiGatewayDomainName is present).

## Steps to Reproduce
1. Boot the emulator from this checkout and point an SDK/CLI at it (region/account per the environment).
2. Causes 1-4: ACM ImportCertificate a self-signed cert for '<name>.example.com'; CreateDomainName(DomainName=<name>.example.com, DomainNameConfigurations=[{CertificateArn, EndpointType='REGIONAL'}]); CreateApi(ProtocolType='HTTP'); CreateStage(StageName='prod') and CreateStage(StageName='$default'). Then:
   - CreateApiMapping(..., ApiMappingKey='caf\u00e9') -> expect BadRequestException with the illegal-character message; then GetApiMappings -> expect Items == [] and an ASCII key such as 'v1.2' to be accepted and round-trip.
   - CreateApiMapping(..., ApiMappingKey='/bad-leading-slash') -> expect BadRequestException with the leading/consecutive-slash message.
   - CreateApiMapping(ApiId, DomainName, Stage='') -> expect BadRequestException 'Invalid stage identifier specified'; then the same call with Stage='$default' -> expect 201 and the mapping visible in GetApiMappings.
   - Fire two CreateApiMapping calls in parallel with the same DomainName and ApiMappingKey='orders' but different stages -> expect exactly one 201 and one ConflictException, and exactly one persisted mapping for that key.
3. Cause 5: on a domain name as above, CreateRoutingRule(DomainName, Priority=100, Conditions=[{}], Actions=[{'InvokeApi': {'ApiId': <id>, 'Stage': 'dev'}}]) -> expect BadRequestException and no rule at Priority 100 in ListRoutingRules; then CreateRoutingRule(Priority=200, Conditions=[{'MatchBasePaths': {'AnyOf': ['positive']}}]) -> expect 201 and GetRoutingRule to report Priority 200.
4. Cause 6: CreateDomainName for a domain, then CreateDomainName for the SAME domain with a different (fresh) CertificateArn -> expect BadRequestException 'The domain name you provided already exists.'
5. Cause 7: CreateDomainName with SecurityPolicy='TLS_1_2', then UpdateDomainName with SecurityPolicy='TLS_1_0' -> expect BadRequestException 'TLS_1_0 is not supported for APIGatewayV2 domainName.' and GetDomainName still reporting TLS_1_2.
6. Cause 8: with an ACM-IMPORTED certificate, CreateDomainName (no mTLS), upload a truststore to S3, then UpdateDomainName adding MutualTlsAuthentication={TruststoreUri, TruststoreVersion} and no OwnershipVerificationCertificateArn -> expect BadRequestException with the missing-ownershipVerificationCertificate message and no MutualTlsAuthentication on the subsequent GetDomainName.
7. Cause 9: CreateDomainName(EndpointType='REGIONAL'), poll GetDomainName until DomainNameStatus=='AVAILABLE', and read DomainNameConfigurations[0]['HostedZoneId'] -> it must be present.
8. Replay the check ids in trace_ids to confirm all nine causes.
