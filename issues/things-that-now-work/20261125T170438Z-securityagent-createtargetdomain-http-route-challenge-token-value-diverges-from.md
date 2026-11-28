Issue-Source: trace-sweep-divergence
Issue-Trace: securityagent-target-domain-tagging-and-errors
Issue-Step: setup#1
Issue-Severity: LOW
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: securityagent-target-domain-tagging-and-errors
Issue-Shape-Id: com.amazonaws.securityagent#SecurityAgent
Issue-Key: securityagent|CreateTargetDomain|2xx->2xx|$.verificationDetails.httpRoute.token
Issue-Commit: d070972b18118943755a5fa07935407d1da45bd2

# CreateTargetDomain HTTP-route challenge token value diverges from the domain's bound challenge

## Resolution (cause declined: a per-run minted ownership-verification secret)

The emulator's behaviour is correct and the one cause is DECLINED, not implemented. The
ownership-verification challenge is the proof of control of the domain: the service DRAWS
it from its own entropy and holds it against the domain, so it cannot be computed from
anything the request or a third party can see. `src/state.rs` draws it once per domain
(`IdentifierMint::next_secret`), binds it to the name in its normalised form so one domain
keeps one challenge across its registrations, its spellings and the deletion of them, and
`TargetDomainRecord::verification_details` serves that one stored value as both
`dnsTxt.token` and `httpRoute.token`. `HttpVerification.token` is an unconstrained `String`
in the model (no `@pattern`, no `@length`), so nothing pins the value, and the step's
expectation is the secret ONE recorded run drew; the replay key
`securityagent|CreateTargetDomain|2xx->2xx|$.verificationDetails.httpRoute.token`
therefore stays failing by construction, as it did for the three earlier behaviours.

Do NOT implement the remedy this report proposes (derive the challenge from the account and
the normalised domain name): a challenge computable from public information is no proof of
control at all — anyone could publish it for a domain they do not own — and it contradicts
the rule the workspace already applies to the other service that issues a domain-ownership
challenge (`services/common/src/ses_identities.rs` `mint_verification_token`, drawn from the
registry's counter, system entropy and the minting instant).

What the resolution added instead is the test that pins the rule:
`tests/all/target_domain_challenge_secret.rs` asserts, over a table of domain-name SHAPES and
both challenge-publishing verification methods, that one account's challenge for a domain is
fixed by the domain name alone — the member it is published through and the draws the mint
made for other resources in between leave it as it was — that no two names share one, and
that independent mints driven through byte-identical calls in the same order are told to
publish different challenges, which a value derived from public input could not be.

> _Note: this issue was identified against the code base at commit `d070972b18118943755a5fa07935407d1da45bd2`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateTargetDomain

## Description
ONE net-new cause. The replay reports it as the corpus's only unowned mismatch cluster (owned_by empty, stub_path empty; the other four clusters of this run are owned by open issues). Status, members and body shape of the CreateTargetDomain response are all correct — the single diverging member is the HTTP-route ownership-verification challenge value.

Cause 1 of 1 (fixture securityagent-target-domain-tagging-and-errors, step setup#1, CreateTargetDomain): $.verificationDetails.httpRoute.token is not the challenge value the corpus holds for the domain this step registers. The emulator answers with a value of its own minting whose form is a 64-character lowercase hex SHA-256 digest, and the step's expectation ({{http_route_token_2}}) is not satisfied by it.

AWS: Smithy (services/service-com-amazonaws-securityagent-securityagent/smithy-model/com-amazonaws-securityagent-securityagent.json, restJson1): CreateTargetDomain POST /CreateTargetDomain, success 200, no modeled errors; input targetDomainName (@required String) and verificationMethod (@required DomainVerificationMethod DNS_TXT|HTTP_ROUTE|PRIVATE_VPC) plus tags; output targetDomainId (@required TargetDomainId), domainName (@required), verificationStatus (@required TargetDomainStatus), verificationStatusReason, verificationDetails (VerificationDetails{method, dnsTxt: DnsVerification, httpRoute: HttpVerification}), createdAt, verifiedAt. HttpVerification.token is an unconstrained String documented 'The verification token to serve at the specified route path'; it carries no @pattern and no @length, so the model pins NO format — recorded behavior only for the token itself. What the recording pins is that this registration of this domain reports the challenge value the corpus has bound for that domain, i.e. the proof-of-control challenge is a property of the (account, domainName) pair and a challenge its owner has already published at /.well-known/aws/securityagent-domain-verification.json keeps matching. Every other member of this same response — targetDomainId (itself a corpus placeholder, satisfied by the emulator's td-<uuid>), domainName, verificationStatus, verificationStatusReason, verificationDetails.method and verificationDetails.httpRoute.routePath — matches, so the recording constrains nothing else here.

Emulator: HTTP 200 with every other member correct, and $.verificationDetails.httpRoute.token carrying a freshly drawn 64-character lowercase hex digest in its place (this run drew '89ad44827ed90d0da236364efbf5040daf7a446e8b7a76ede6dfb1e2f01af417'; the quoted literal is this run's value, NOT the requirement). The value is minted by services/service-com-amazonaws-securityagent-securityagent/src/state.rs IdentifierMint::next_secret, which hex-encodes the SHA-256 of a challenge context, the mint's system-entropy namespace, a never-reused counter and the draw instant, and is then stored against the normalised domain name in ComAmazonawsSecurityagentSecurityagentState::domain_verification_tokens and projected by TargetDomainRecord::verification_details into httpRoute.token (and dnsTxt.token). Note what has already been ruled out by two now-RESOLVED issues against this exact divergence_key (service=securityagent|CreateTargetDomain|status=200->200|keys=$.verificationDetails.httpRoute.token), neither of which covers this failure any more and neither of which the replay reports as owning it: 20261030T004326Z-securityagent-created-resource-identifiers-and-verification-tokens-are-fixed.md made the token unique instead of a fixed constant, and 20261114T185730Z-securityagent-createtargetdomain-mints-a-fresh-http-route-verification-token.md made it stable per registered domain name (src/ops/create_target_domain.rs lines 236-244 and the domain_verification_tokens registry show that stability landed, with normalized_domain_name handling case and the rooted trailing dot). The step still mismatches after both, through three different value-identity behaviours (constant digest, per-call digest, per-domain digest) that share ONE unchanged property: the token's own form and derivation, a drawn 64-hex secret carrying nothing about the domain it is the challenge for. Hypothesis: what remains unsatisfied is the token's form/derivation rather than its stability — the recorded service's challenge value for a domain is not a 64-character lowercase hex digest of service-side entropy, so a drawn secret of that shape cannot be the value the corpus binds for this domain however stably it is stored.

Why: the token is the proof-of-control challenge a client must publish on the domain (as the /.well-known/aws/securityagent-domain-verification.json document for HTTP_ROUTE, as the TXT record value for DNS_TXT) before VerifyTargetDomain can succeed. A client that holds the challenge AWS issued for a domain — published once, cached per domain name, or compared against the value a fresh registration reports — gets a value from the emulator that does not agree with it, so its published document never matches the challenge the emulator believes in and ownership verification for that domain cannot be made to pass. Any client that asserts on the shape of the challenge it is told to publish (length, alphabet, or that it is derived from the domain) also diverges on every call.

Fix: the CreateTargetDomain handler's identity minting and the target-domain state behind it — services/service-com-amazonaws-securityagent-securityagent/src/ops/create_target_domain.rs (state.mint_target_domain_identity) and src/state.rs (mint_target_domain_identity, IdentifierMint::next_secret, the domain_verification_tokens registry, TargetDomainRecord::verification_token and TargetDomainRecord::verification_details, which projects the stored token into both httpRoute.token and dnsTxt.token). The challenge a domain's registration reports must be the challenge value that domain has in this corpus, so it must be derived from the identity of the domain (the account and the normalised domain name) rather than drawn as service-side entropy and merely cached for the life of the in-memory registry; the same stored value must feed httpRoute.token and dnsTxt.token and the pending status reason that quotes it, and it must be reported again on a re-registration of the same domain name.

Traces: securityagent-target-domain-tagging-and-errors#setup#1.

## Evidence
Replay of wire trace set simparity-wire-aws-v1 against the emulator built from this checkout (emulator_up=true, '/__admin/health answered 200 and the :46579 data listener accepts'; 16 fixtures replayed, 105 steps: 82 passed, 23 mismatched, 0 errored, 0 blocked, 18 not_implemented, score 78.1%, 5 divergence clusters). Four clusters carry owned_by (three ListSecurityRequirements description clusters owned by 20261123T171023Z-securityagent-aws-managed-security-requirement-catalog-descriptions-are-paraphrased.md, and VerifyTargetDomain#task#5 owned by 20261108T133019Z-securityagent-verifytargetdomain-stays-pending-and-omits-arn-ownerid-verificationstatus.md). This is the only cluster with stub_path empty AND owned_by empty.

Fixture securityagent-target-domain-tagging-and-errors, step setup#1, operation CreateTargetDomain, verdict mismatch, expected status 200 / actual status 200, affected_count 1:
  $.verificationDetails.httpRoute.token: value_mismatch (expected '{{http_route_token_2}}', got '89ad44827ed90d0da236364efbf5040daf7a446e8b7a76ede6dfb1e2f01af417')

divergence_key: service=securityagent|CreateTargetDomain|status=200->200|keys=$.verificationDetails.httpRoute.token

No other member of the response is reported: $.targetDomainId (a corpus placeholder in its own right, satisfied by the emulator's td-<uuid> value), $.domainName, $.verificationStatus, $.verificationStatusReason, $.verificationDetails.method and $.verificationDetails.httpRoute.routePath all match, so the divergence is confined to the challenge token.

The quoted hex literal is this run's drawn value and is NOT the requirement; the requirement is that this step reports the challenge value bound for this domain in the corpus. Corroboration that the step is not merely tolerating a per-run value: the sibling placeholder $.targetDomainId at this SAME step is satisfied by an emulator-minted value, while this key has now mismatched across three distinct emulator behaviours for it — the constant '0778a98ec3c7bfd032e1266778cc3d244a3db4cc51b7638568e2a01afaffd780' (issue 20261030T004326Z, resolved), a per-call draw '9b1e8b7bd29a7f51cbff83e388fc95862994c29746215218a2937d9f4c63dd08' (issue 20261114T185730Z, resolved), and now a per-domain-stable draw — all three of them 64-character lowercase hex digests of service-side entropy.

Model basis (smithy_query, model_path services/service-com-amazonaws-securityagent-securityagent/smithy-model/com-amazonaws-securityagent-securityagent.json): CreateTargetDomain -> CreateTargetDomainOutput.verificationDetails -> VerificationDetails.httpRoute -> HttpVerification{token: String, routePath: String}; token has no @pattern/@length and the operation has no modeled errors, so the token's value is recorded behavior only.

Code citations (current checkout): services/service-com-amazonaws-securityagent-securityagent/src/ops/create_target_domain.rs:236-251 (the comment states the intended contract — 'The ownership-verification token ... is the proof of control of the DOMAIN, so it is the one the account already holds for that domain name where it holds one' — and calls state.mint_target_domain_identity(&input.target_domain_name)); src/state.rs:1805-1816 (domain_verification_tokens, 'The ownership-verification challenge this account holds for each domain name it has registered'); src/state.rs:1298-1303 and 1322-1349 (TargetDomainRecord::verification_token, ::pending); src/state.rs:1702-1709 (IdentifierMint::next_secret: 64 lowercase hex, SHA-256 of context + system-entropy namespace + never-reused counter + draw instant); src/state.rs:2355-2368 (normalized_domain_name).

## Steps to Reproduce
1. git clone the repo and boot the emulator built from that checkout.
2. Replay the wire corpus (replay_wire_corpus, trace set simparity-wire-aws-v1) and inspect fixture securityagent-target-domain-tagging-and-errors, step setup#1 (CreateTargetDomain): $.verificationDetails.httpRoute.token is the only reported key, while every other member of the 200 response — including the sibling $.targetDomainId placeholder — matches.
3. Equivalently by hand against the emulator's data listener (restJson1, POST with sigv4): POST /CreateTargetDomain {"targetDomainName":"probe-one.example.com","verificationMethod":"HTTP_ROUTE"} and read verificationDetails.httpRoute.token: a 64-character lowercase hex digest that carries nothing of the domain it is the challenge for. Repeat with the same name (and with the rooted/mixed-case spellings) to confirm the per-domain stability already in place, and with DNS_TXT to see the same value served as dnsTxt.token.
4. Expected: the registration reports the ownership-verification challenge that belongs to that domain in this account — the value the corpus binds for the domain at this step, derived from the domain's identity so it is the same challenge whenever that domain is registered. Observed: a drawn 64-hex secret that does not satisfy the bound challenge.
5. Re-replay securityagent-target-domain-tagging-and-errors#setup#1 to confirm the fix (this is the single binding to re-run for this issue).
