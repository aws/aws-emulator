Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: 14fef70bd15ca29a08f279a21ebf0b06fea39359

# encryptionConfiguration is stored without checking that the type and kmsKeyId agree or that the KMS key exists

_Filed: 2026-10-24T00:32:02Z_
_Operation: UpdateInvestigationGroup | Class: validation_

> _Note: this issue was identified against the code base at commit `14fef70bd15ca29a08f279a21ebf0b06fea39359`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Expected: EncryptionConfiguration describes which key encrypts investigation data, and the two members must agree — `type: CUSTOMER_MANAGED_KMS_KEY` requires a `kmsKeyId` naming a usable customer managed KMS key, and `type: AWS_OWNED_KEY` names no key. AWS validates the key at write time (the key must exist, be in the group's Region/account and be usable by the service) and refuses an incoherent or unusable configuration with the modelled ValidationException (400), leaving the stored configuration untouched.

Actual: the only check applied is that the `type` string is one of the modelled enum values (crate::state::encryption_configuration_rejection). So UpdateInvestigationGroup with {"encryptionConfiguration":{"type":"CUSTOMER_MANAGED_KMS_KEY"}} (no kmsKeyId) returns 200 and the group is left claiming customer-managed encryption with no key; {"type":"AWS_OWNED_KEY","kmsKeyId":"arn:aws:kms:...:key/abcd"} is likewise stored verbatim; and a kmsKeyId naming a key that was never created (any `^arn:.*$` string) is accepted. GetInvestigationGroup then reports these impossible configurations back as fact. The same unchecked path is used by CreateInvestigationGroup (the open issue 'CreateInvestigationGroup accepts an unassumable roleArn' covers roleArn assumability only, not encryption).

## Evidence

Spec: EncryptionConfiguration{type: EncryptionConfigurationType enum [AWS_OWNED_KEY, CUSTOMER_MANAGED_KMS_KEY], kmsKeyId: length 1..256, pattern ^arn:.*$}; kmsKeyId is documented as 'If the investigation group uses a customer managed key for encryption, this field displays the ID of that key' and the structure is documented as 'Use this structure to specify a customer managed KMS key to use to encrypt investigation data' — so selecting CUSTOMER_MANAGED_KMS_KEY without naming a key is not a configuration the service can honour. UpdateInvestigationGroup models ValidationException (400). Handler: services/service-com-amazonaws-aiops-aiops/src/ops/update_investigation_group.rs lines ~183-207 call only encryption_configuration_rejection, which (src/state.rs lines ~497-512) returns None as soon as `type` is in ENCRYPTION_CONFIGURATION_TYPE_VALUES and never looks at kmsKeyId; line ~225 then stores the structure as given. Test coverage: tests/all/investigation_group_singleton.rs (an_omitted_encryption_configuration_reads_back_as_the_aws_owned_key) and tests/all/investigation_groups.rs only use well-formed CUSTOMER_MANAGED_KMS_KEY + kmsKeyId pairs; no test exercises a type/kmsKeyId mismatch or a nonexistent key.

## Steps to Reproduce

1. Create an investigation group with no encryptionConfiguration and note its ARN
2. PATCH /investigationGroups/<url-encoded ARN> with {"encryptionConfiguration":{"type":"CUSTOMER_MANAGED_KMS_KEY"}} -> emulator answers 200; AWS refuses with 400 ValidationException because no kmsKeyId was supplied
3. GET /investigationGroups/<url-encoded ARN> -> encryptionConfiguration reports CUSTOMER_MANAGED_KMS_KEY with no key
4. PATCH again with {"encryptionConfiguration":{"type":"AWS_OWNED_KEY","kmsKeyId":"arn:aws:kms:us-west-2:000000000000:key/never-created"}} -> emulator answers 200 and stores the contradictory pair
