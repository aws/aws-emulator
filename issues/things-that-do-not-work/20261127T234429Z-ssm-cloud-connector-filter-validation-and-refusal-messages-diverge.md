Issue-Source: trace-sweep-divergence
Issue-Trace: ssm-cloud-connector-filter-value-length-bounds
Issue-Step: task#3
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: ssm-cloud-connector-filter-value-length-bounds ssm-cloud-connector-azure-tenant-display-name-length ssm-delete-cloud-connector-id-length-bounds ssm-delete-cloud-connector-id-pattern-and-legal-neighbour
Issue-Shape-Id: com.amazonaws.ssm#AmazonSSM
Issue-Key: ssm|ListCloudConnectors|4xx->4xx|$.message
Issue-Key: ssm|ListCloudConnectors|4xx->2xx|$.__type
Issue-Key: ssm|ListCloudConnectors|4xx->2xx|$.message
Issue-Key: ssm|ListCloudConnectors|4xx->2xx|status
Issue-Key: ssm|CreateCloudConnector|4xx->4xx|$.message
Issue-Key: ssm|DeleteCloudConnector|4xx->4xx|$.message
Issue-Commit: 255279eeed7e5207bd6ac2895e89724e049cf5ef

# Cloud Connector filter validation and refusal messages diverge from AWS

> _Note: this issue was identified against the code base at commit `255279eeed7e5207bd6ac2895e89724e049cf5ef`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListCloudConnectors

## Description
Four net-new causes on the SSM Cloud Connector operations. Shared fault: the connector filter/identifier validation checks the wrong thing and renders the refusal differently from AWS.

Cause 1 of 4: ListCloudConnectors validates the filter KEY instead of the filter VALUE.
AWS: for a filter whose key is legal (ProjectId) but whose value is outside that key's grammar, 400 with message "Value '<value>' at 'ProjectId' failed to satisfy constraint: Member must satisfy regular expression pattern: ^([a-z][a-z0-9-]{4,28}[a-z0-9]|NONE)$" — the refusal names the filter key as the member path and quotes the submitted value and that key's own grammar.
Emulator: refuses the key itself — "1 validation error detected: Value 'ProjectId' at 'filters.1.member.filterKey' failed to satisfy constraint: Member must satisfy enum value set: [TenantId, OrganizationId, ProjectId, SubscriptionId]" — naming a key its own quoted set contains.
Why: the caller is told a legal filter key is illegal and never learns the value is malformed, so the request cannot be corrected from the error.
Fix: the ListCloudConnectors filter validation (validate each filter's value against the key's grammar).
Traces: ssm-cloud-connector-filter-value-length-bounds#task#3.

Cause 2 of 4: ListCloudConnectors accepts a TenantId filter value AWS refuses.
AWS: 400, __type ValidationException, message "Value '<value>' at 'TenantId' failed to satisfy constraint: Member must satisfy regular expression pattern: ^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$".
Emulator: 200.
Why: a malformed tenant filter silently returns an unfiltered or empty listing instead of a refusal.
Fix: as cause 1 (the same missing per-key value check).
Traces: ssm-cloud-connector-filter-value-length-bounds#task#2.

Cause 3 of 4: CreateCloudConnector truncates the value it echoes.
AWS: "1 validation error detected: Value '<full submitted value>' at 'configuration.azureConfiguration.tenantDisplayName' failed to satisfy constraint: Member must have length less than or equal to 256" — the whole value is quoted.
Emulator: quotes a prefix plus '... [257 bytes]'.
Fix: the validation message renderer (no truncation marker).
Traces: ssm-cloud-connector-azure-tenant-display-name-length#task#2.

Cause 4 of 4: DeleteCloudConnector quotes the cloudConnectorId pattern un-anchored.
AWS: "2 validation errors detected: Value '<value>' at 'cloudConnectorId' failed to satisfy constraint: Member must have length greater than or equal to 36; Value '<value>' at 'cloudConnectorId' failed to satisfy constraint: Member must satisfy regular expression pattern: ^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$".
Emulator: identical except the pattern is emitted without its ^...$ anchors.
Why: the quoted grammar admits ids AWS refuses.
Fix: the validation message renderer (emit the pattern as modeled).
Traces: ssm-delete-cloud-connector-id-length-bounds#task#2.

## Evidence
Replay of simparity-wire-aws-v1.
Cause 1 ssm-cloud-connector-filter-value-length-bounds#task#3 (400->400): $.message expected "Value '<256-char value>' at 'ProjectId' ... pattern: ^([a-z][a-z0-9-]{4,28}[a-z0-9]|NONE)$", got "1 validation error detected: Value 'ProjectId' at 'filters.1.member.filterKey' ... enum value set: [TenantId, OrganizationId, ProjectId, SubscriptionId]".
Cause 2 #task#2: expected 400 'ValidationException' with the TenantId pattern message, got 200.
Cause 3 ssm-cloud-connector-azure-tenant-display-name-length#task#2: echo truncated with '... [257 bytes]'.
Cause 4 ssm-delete-cloud-connector-id-length-bounds#task#2: pattern quoted without ^...$ anchors.

## Steps to Reproduce
1. Boot the emulator and replay the corpus.
2. Causes 1-2: fixture ssm-cloud-connector-filter-value-length-bounds steps task#3 (and task#4, task#5) and task#2.
3. Cause 3: fixture ssm-cloud-connector-azure-tenant-display-name-length step task#2.
4. Cause 4: fixture ssm-delete-cloud-connector-id-length-bounds step task#2 (and ssm-delete-cloud-connector-id-pattern-and-legal-neighbour step task#1).
