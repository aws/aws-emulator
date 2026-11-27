Issue-Severity: MEDIUM
Issue-Gap-Class: error-response
Issue-Commit: 55a0d20c7e4924f79fde4d90746ab5257e5583db

# Create operations put an input member name in ResourceNotFoundException.ResourceType, outside the domain the model states

_Filed: 2026-11-27T18:51:50Z_
_Operation: CreateChallenge | Class: error-response_

> _Note: this issue was identified against the code base at commit `55a0d20c7e4924f79fde4d90746ab5257e5583db`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Both creates answer their 404 with `crate::state::reference_not_found(<input member name>, arn)`, which sets the error's required `ResourceType` member to the NAME OF AN INPUT MEMBER ("ConnectorArn" for `CreateChallenge`, "CertificateAuthorityArn" for `CreateConnector`) and opens the required `Message` with that same word. The Smithy model states the domain of that member in its own documentation sentence: it "can be either `Connector` or `Challenge`". Neither "ConnectorArn" nor "CertificateAuthorityArn" is in that domain, so a client branching on `ResourceType` (or parsing the sentence, which is built from it) sees a value the model says cannot occur.

The same crate contradicts itself here twice over:

1. `create_challenge.rs` itself sets `resource_type` to the modelled vocabulary for its two OTHER errors — `ConflictException` (line 258) and `ServiceQuotaExceededException` (line 314) — whose `ResourceType` members carry the IDENTICAL model sentence ("The resource type, which can be either `Connector` or `Challenge`"). Only the 404 deviates.
2. `state.rs` quotes the model sentence in the very comment that then documents breaking it.

The fix is for `reference_not_found` to report the resource KIND the unresolved ARN names (which `resource_type_of` already computes, and which `resource_not_found` already reports for every read and delete) rather than an input member name. The input member can still be named in the `Message` if desired; `ResourceType` must stay inside its documented domain.

## Evidence

Smithy model, `ResourceNotFoundException$ResourceType` (smithy_query query=shape shape=ResourceNotFoundException):
  "ResourceType": {"target": "String", "required": true, "doc": "<p>The resource type, which can be either <code>Connector</code> or <code>Challenge</code>.</p>"}

Handler side — services/service-com-amazonaws-pcaconnectorscep-pcaconnectorscep/src/ops/create_challenge.rs:372:
  `const MISSING_MEMBER_CONNECTOR_ARN: &str = "ConnectorArn";`
and :383-389:
  `fn not_found(connector_arn: &str) -> CreateChallengeError {`
  `    CreateChallengeErrorModeled::ResourceNotFoundException(crate::state::reference_not_found(`
  `        MISSING_MEMBER_CONNECTOR_ARN,`
  `        connector_arn,`
  `    ))`

Same defect in the sibling create — src/ops/create_connector.rs:522 and :527-532:
  `const MISSING_MEMBER_CERTIFICATE_AUTHORITY_ARN: &str = "CertificateAuthorityArn";`
  `CreateConnectorErrorModeled::ResourceNotFoundException(crate::state::reference_not_found(`
  `    MISSING_MEMBER_CERTIFICATE_AUTHORITY_ARN,`
  `    certificate_authority_arn,`
  `))`

The helper puts that string straight into the member and into the sentence — src/state.rs:915-917 and :893-899:
  `pub fn reference_not_found(input_member: &str, resource_arn: &str) -> ResourceNotFoundException {`
  `    resource_not_found(input_member, resource_arn)`
  `}`
  `        message: format!("{resource_type} was not found: {resource_id}."),`
  `        resource_type: resource_type.to_string(),`

Internal inconsistency inside this same handler — src/ops/create_challenge.rs:252-258 (ConflictException) and :308-314 (ServiceQuotaExceededException), whose `ResourceType` members carry the identical model sentence:
  `                    resource_id: arn,`
  `                    resource_type: crate::state::RESOURCE_TYPE_CHALLENGE.to_string(),`
  `                    resource_type: crate::state::RESOURCE_TYPE_CHALLENGE.to_string(),`

And the crate's own comment quotes the rule it then breaks — src/state.rs:72-78:
  `/// `ResourceId`/`ResourceType` vocabulary of this service's errors: the `ResourceType``
  `/// member's doc says it "can be either `Connector` or `Challenge`", which are the two kinds`
  `/// of resource this service owns. ... A create reports the input member that carried the reference`
  `/// it could not resolve instead ([`reference_not_found`]), because the resource that call is`
  `/// about does not exist yet.`

Test coverage: tests/all/resource_not_found_message.rs pins the current (contradicting) value — it asserts the create must NOT report a resource kind at lines 687-697 (`assert!(!RESOURCE_TYPE_DOMAIN.contains(&answered), ...)`) while quoting the very model sentence at lines 49-53 as "The WHOLE domain of `ResourceType` ... No 404 such a call answers may carry anything else." The stated reason for the carve-out (lines 19-22) is design rationale ("a CREATE ... reports the NAME of the input member that carried the reference"), not a model quote, and the model sentence contradicts it. That test's create rows must be updated with the fix.

## Steps to Reproduce

1. Create no connector, then POST /challenges with a well-formed but unknown `ConnectorArn` (e.g. arn:aws:pca-connector-scep:eu-west-2:000000000000:connector/11111111-1111-1111-1111-111111111111).
2. Observe HTTP 404 with body `{"Message":"ConnectorArn was not found: arn:...","ResourceId":"arn:...","ResourceType":"ConnectorArn"}`.
3. Compare with the model: `ResourceNotFoundException$ResourceType` is documented as "can be either Connector or Challenge" — "ConnectorArn" is outside that domain.
4. Compare with the same handler's other errors: reuse one `ClientToken` across two different `ConnectorArn` values to get `ConflictException`, whose `ResourceType` is "Challenge" — the modelled vocabulary.
5. Repeat with POST /connectors and an unknown `CertificateAuthorityArn`: `ResourceType` is "CertificateAuthorityArn".
