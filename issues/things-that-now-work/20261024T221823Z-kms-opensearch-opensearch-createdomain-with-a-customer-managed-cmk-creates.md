Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: kms__opensearch.xsvc-opensearch-3-cmk-domain-echoes-key-grants-and-serves-search
Issue-Key: kms__opensearch.xsvc-opensearch-3-cmk-domain-echoes-key-grants-and-serves-search|run#21|Timeout|time.sleep(10)
Issue-Commit: 564feb857476f3247a0693cb65aa2d44e3322d3a

# OpenSearch CreateDomain with a customer-managed CMK creates no KMS grant for the OpenSearch service principal, so the CMK-domain check hangs to the 60 s cap (1 cause)

> _Note: this issue was identified against the code base at commit `564feb857476f3247a0693cb65aa2d44e3322d3a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateDomain

## Description
ONE root cause, grounded in one failing behavioral check the report marked neither ALREADY FILED nor OWNED.

Related open issue that does NOT cover this cause (named for context only): 20261024T055421Z-kms-opensearch-createdomain-does-not-validate-encryptionatrestoptions-kmskeyid-disabled-and.md covers CreateDomain not VALIDATING EncryptionAtRestOptions.KmsKeyId (disabled / cross-Region CMKs accepted). This finding is the separate cross-service EFFECT: even for a perfectly valid, enabled, same-Region CMK, CreateDomain never touches KMS at all.

Cause 1 of 1 — CreateDomain with EncryptionAtRestOptions={Enabled:true, KmsKeyId:<customer CMK>} creates no KMS grant on that key.
AWS: creating an encrypted-at-rest domain causes OpenSearch to take a grant on the customer's CMK, visible through KMS ListGrants on that key. The grant's GranteePrincipal and RetiringPrincipal are the regional service principal es.<region>.amazonaws.com, its Operations include at least Decrypt and GenerateDataKey, its IssuingAccount is arn:aws:iam::<account>:root, and it is constrained by Constraints.EncryptionContextSubset.domainARN equal to the new domain's ARN. Smithy (opensearch): CreateDomain takes EncryptionAtRestOptions and answers DomainStatus; Smithy (kms): CreateGrant takes GranteePrincipal/RetiringPrincipal (PrincipalIdType) plus Constraints (GrantConstraints) and ListGrants returns the GrantList those members are read back from — the grant is the only observable binding the domain's storage to the key. Recorded behavior: the check polls ListGrants for up to 600 s for exactly that grant.
Emulator: no grant ever appears. services/service-com-amazonaws-opensearch-amazonopensearchservice/src/ops/create_domain.rs stores input.encryption_at_rest_options verbatim on the Domain record and marks the option touched; it makes no KMS call and holds no reference to the KMS service at all. The check therefore spins in the ListGrants poll (interval 10 s, budget 600 s) and is killed at the suite's 60 s cap, so it never reaches its later legs (DescribeDomain KmsKeyId after activation, and the signed _search over the domain endpoint); those remain unverified rather than known-good.
Why: the grant is the auditable, revocable link between an encrypted domain and its key. Without it, a client cannot see which keys its domains depend on, cannot rehearse revoking a key from a domain, and gets the opposite answer from the emulator for any least-privilege or key-inventory review — the domain looks encrypted while nothing in KMS records the dependency. This is the same class of gap already filed for ECR (20261024T182018Z-ecr-kms-ecr-createrepository-with-encryptiontype-kms-creates-no-kms.md), in a different service.
Fix: the CreateDomain handler in services/service-com-amazonaws-opensearch-amazonopensearchservice/src/ops/create_domain.rs — when EncryptionAtRestOptions.Enabled is true with a customer CMK, create the domain-scoped KMS grant (grantee/retiring principal es.<region>.amazonaws.com, Operations covering Decrypt and GenerateDataKey, EncryptionContextSubset {domainARN: <domain ARN>}) in KMS state, and retire it on DeleteDomain.
Traces: kms__opensearch.xsvc-opensearch-3-cmk-domain-echoes-key-grants-and-serves-search (60 s timeout).

## Evidence
kms__opensearch.xsvc-opensearch-3-cmk-domain-echoes-key-grants-and-serves-search — "Error: Check timed out after 60.0s".

The check's earlier assertions (CreateKey KeyManager=='CUSTOMER'; CreateDomain echoing EncryptionAtRestOptions=={'Enabled': True, 'KmsKeyId': <key ARN>}; DescribeDomain echoing the same) are passed before the blocking loop, so the timeout is inside the grant poll:

    deadline = time.monotonic() + 600
    while time.monotonic() < deadline:
        grants = _all_grants(kms, key['KeyId'])
        ours = [g for g in grants
                if g.get('GranteePrincipal') == f'es.{ctx.region}.amazonaws.com'
                and ((g.get('Constraints') or {}).get('EncryptionContextSubset') or {}).get('domainARN') == domain_arn]
        if ours: break
        time.sleep(10)
    assert ours, "no KMS grant to 'es.<region>.amazonaws.com' constrained to domainARN=... appeared on <key ARN> within 600s"

Expected: KMS ListGrants on the CMK returns a grant with GranteePrincipal and RetiringPrincipal 'es.<region>.amazonaws.com', Operations ⊇ {Decrypt, GenerateDataKey}, IssuingAccount 'arn:aws:iam::<account>:root', Constraints.EncryptionContextSubset.domainARN == 'arn:aws:es:<region>:<account>:domain/<domain>'.
Actual: ListGrants on the key stays empty — the handler at services/service-com-amazonaws-opensearch-amazonopensearchservice/src/ops/create_domain.rs:228-434 only persists `encryption_at_rest_options: input.encryption_at_rest_options` (line 301) and touches OPT_ENCRYPTION_AT_REST_OPTIONS (lines 365-368); there is no KMS interaction anywhere in the operation.

## Steps to Reproduce
1. kms CreateKey (symmetric, customer-managed) and note its Arn and KeyId.
2. opensearch CreateDomain(DomainName=<name>, EngineVersion='OpenSearch_2.11', ClusterConfig={'InstanceType':'t3.small.search','InstanceCount':1}, EBSOptions={'EBSEnabled':True,'VolumeType':'gp3','VolumeSize':10}, EncryptionAtRestOptions={'Enabled':True,'KmsKeyId':<key ARN>}, AccessPolicies=<account-root es:* policy>).
3. kms ListGrants(KeyId=<key id>) — observe an empty Grants list, where AWS shows a grant to es.<region>.amazonaws.com constrained by EncryptionContextSubset.domainARN to the new domain's ARN with Operations including Decrypt and GenerateDataKey.
4. Re-run check kms__opensearch.xsvc-opensearch-3-cmk-domain-echoes-key-grants-and-serves-search to confirm it no longer hits the 60 s cap.
