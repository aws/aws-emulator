Issue-Severity: LOW
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: codeconnections.connection-invalid-provider-type-rejected
Issue-Shape-Id: com.amazonaws.codeconnections#CodeConnections_20231201
Issue-Key: codeconnections.connection-invalid-provider-type-rejected|run#6|AssertionError|assert message == expected, f"expected message {expected!r}, got {message!r}"
Issue-Commit: ed3ed06301643834f5ccb593614cfd988c817634

# codeconnections CreateConnection ValidationException message lists a 7th providerType value AWS does not enumerate

> _Note: this issue was identified against the code base at commit `ed3ed06301643834f5ccb593614cfd988c817634`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateConnection

## Description
One cause.

Cause 1 of 1: the constraint-violation message CreateConnection returns for a `providerType` outside the accepted set enumerates one more value than the service does.

AWS: CreateConnection with a ProviderType string that is not an accepted provider is refused with error code `ValidationException` and the standard single-violation message `1 validation error detected: Value '<providerType>' at 'providerType' failed to satisfy constraint: Member must be one of: [GitLab, GitHub, GitHubEnterpriseServer, AzureDevOps, GitLabSelfManaged, Bitbucket]`. The fixed text is `1 validation error detected: Value '` + `' at 'providerType' failed to satisfy constraint: Member must be one of: [` + `]`; the only variable part is the rejected value, taken verbatim from the request. The member-list inside the brackets is fixed text too — AWS enumerates exactly those six names, in that order. (Smithy: CreateConnection$ProviderType targets com.amazonaws.codeconnections#ProviderType, whose modeled values are Bitbucket, GitHub, GitHubEnterpriseServer, GitLab, GitLabSelfManaged, AzureDevOps, GitHubEnterpriseCloudDataResidency; the validation-message text and the list AWS actually prints in it are recorded behavior only, and the recording shows the ghe.com-tenant provider `GitHubEnterpriseCloudDataResidency` is NOT named in this message. Error code recorded on the refusal: ValidationException. Hypothesis: the framework-level constraint message for this member is generated from an older provider enumeration than the Smithy model carries.)

Emulator: same error code and same sentence, but the bracketed member list carries a seventh entry appended at the end — `Member must be one of: [GitLab, GitHub, GitHubEnterpriseServer, AzureDevOps, GitLabSelfManaged, Bitbucket, GitHubEnterpriseCloudDataResidency]`. The six AWS names and their order are already correct, so the divergence is exactly the extra trailing value. The refusal itself (code, and no connection created) matches; only the message text diverges.

Why: the message is part of the wire contract. A client, test or golden-file comparison that asserts on the service's refusal sentence — the usual way callers verify that a bad provider was rejected for the right reason, and the way SDK-level test suites pin this error class — does not match the emulator's string, so an otherwise correct rejection reads as an unexpected message. A caller that parses the bracketed list to present the user the set of selectable providers also offers a provider this message does not advertise.

Fix: the allowed-value list rendered for the `providerType` enum-constraint violation on CreateConnection — the service's ProviderType value list used by the input-validation message builder (see `PROVIDER_TYPE_VALUES` referenced from the codeconnections state module) must render the six providers this message names, in AWS's order, and must not append the ghe.com-tenant provider. This is a validation/message-rendering change, not a change to which values CreateConnection accepts: do not start refusing a provider the model admits.

Traces: behavioral check codeconnections.connection-invalid-provider-type-rejected.

Related (does NOT cover this): open issue 20261029T175842Z-codeconnections-refusal-messages-are-emulator-authored-instead-of.md covers emulator-authored refusal text for DeleteHost, CreateRepositoryLink, GetRepositoryLink, GetRepositorySyncStatus and UpdateSyncBlocker; it names neither CreateConnection nor this constraint message.

## Evidence
Replay of behavioral check codeconnections.connection-invalid-provider-type-rejected against the emulator:

CreateConnection(ConnectionName='sim2real-conn-<run>', ProviderType='NotARealProvider')
- Error code: ValidationException — MATCHES.
- No connection created (name absent from ListConnections) — MATCHES.
- Positive control CreateConnection(ProviderType='GitHub') succeeds and GetConnection reports ProviderType 'GitHub' — MATCHES.
- Message MISMATCH:
  expected "1 validation error detected: Value 'NotARealProvider' at 'providerType' failed to satisfy constraint: Member must be one of: [GitLab, GitHub, GitHubEnterpriseServer, AzureDevOps, GitLabSelfManaged, Bitbucket]"
  got      "1 validation error detected: Value 'NotARealProvider' at 'providerType' failed to satisfy constraint: Member must be one of: [GitLab, GitHub, GitHubEnterpriseServer, AzureDevOps, GitLabSelfManaged, Bitbucket, GitHubEnterpriseCloudDataResidency]"

The only differing substring is the trailing ", GitHubEnterpriseCloudDataResidency" before the closing bracket. 'NotARealProvider' in the message is the value from the request, not part of the requirement; everything else in the sentence, including the six-name bracketed list and its order, is fixed text.

## Steps to Reproduce
1. Boot the emulator from this checkout.
2. Call codeconnections CreateConnection with a unique ConnectionName and ProviderType set to a string outside the provider set, e.g. 'NotARealProvider'.
3. Read the ClientError's Error.Code and Error.Message: the code is ValidationException (correct), the message's bracketed member list contains a seventh entry 'GitHubEnterpriseCloudDataResidency' that AWS does not include.
4. Confirm the refusal is otherwise faithful: ListConnections does not contain the name, and CreateConnection with ProviderType='GitHub' succeeds and GetConnection reports ProviderType 'GitHub'.
5. Equivalently, replay behavioral check codeconnections.connection-invalid-provider-type-rejected.
