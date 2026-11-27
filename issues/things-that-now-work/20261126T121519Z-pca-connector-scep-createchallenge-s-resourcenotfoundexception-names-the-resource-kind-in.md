Issue-Source: trace-sweep-divergence
Issue-Trace: pca-connector-scep-error-not-found-resources
Issue-Step: task#3
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: pca-connector-scep-error-not-found-resources
Issue-Shape-Id: com.amazonaws.pcaconnectorscep#PcaConnectorScep
Issue-Key: pca-connector-scep|CreateChallenge|4xx->4xx|$.Message
Issue-Key: pca-connector-scep|CreateChallenge|4xx->4xx|$.ResourceType
Issue-Commit: 7d5f5571d94544e1e0d6279d2f57c51740cc17a4

# CreateChallenge's ResourceNotFoundException names the resource KIND in both Message and ResourceType where AWS names the input member ConnectorArn

> _Note: this issue was identified against the code base at commit `7d5f5571d94544e1e0d6279d2f57c51740cc17a4`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateChallenge

## Description
Cause 1 of 1: CreateChallenge's not-found refusal is worded and classified after the resource KIND of the ARN it could not resolve, where AWS words and classifies it after the INPUT MEMBER that carried the unresolvable reference. Both the Message sentence and the machine-readable ResourceType member diverge, in the same step.

AWS: CreateChallenge (Smithy: POST /challenges, success 202) takes a required ConnectorArn (@required, @length 5..200, pattern ^arn:aws(-[a-z]+)*:pca-connector-scep:<region>:<12-digit account>:connector/<uuid>$) and models ResourceNotFoundException with httpError 404; that shape's Message, ResourceId and ResourceType are all @required, and ResourceType's doc says only "The resource type, which can be either Connector or Challenge." The model does NOT state the Message wording, and the recorded response contradicts the ResourceType doc's prose, so for both members this is recorded behavior only. The recording shows CreateChallenge, given a syntactically valid ConnectorArn that resolves to no connector, answering HTTP 404 with error code ResourceNotFoundException and:
  - Message = "ConnectorArn was not found: <ConnectorArn>." — fixed text "ConnectorArn was not found: " and the terminating ".", with the ARN taken verbatim from the request's ConnectorArn member;
  - ResourceType = "ConnectorArn" — the fixed name of the input member that carried the reference, not a value from any one run.
That is, AWS reports the unresolvable REQUEST MEMBER on this operation. The same pattern is already recorded for CreateConnector, whose refusal for a missing private CA reports ResourceType "CertificateAuthorityArn" (see open issue 20261123T212057Z-pca-connector-scep-createconnector-error-response--r2.md). Hypothesis: AWS populates these two members from the input member it failed to resolve rather than from the ARN's resource segment; the ResourceType doc's two-value prose is simply incomplete.

Emulator: the status (404) and the error code (ResourceNotFoundException) are right, but the body is built from the ARN's resource kind, so the step diverges on two JSON members at once:
  - $.Message: expected "ConnectorArn was not found: <ConnectorArn>.", got "Connector was not found: <ConnectorArn>."
  - $.ResourceType: expected "ConnectorArn", got "Connector"
The two are coupled by construction: the single shared builder crate::state::resource_not_found(resource_type, resource_id) derives the sentence from the ResourceType word (message = "{resource_type} was not found: {resource_id}."), and CreateChallenge's not_found helper passes crate::state::resource_type_of(connector_arn), which reads the kind off the ARN and yields "Connector". So one wrong input produces both wrong members, and neither can be corrected in isolation through that builder.

Why: a client that branches on ResourceType to decide which of its inputs to re-resolve or re-prompt for reads "Connector" — a resource kind — where AWS hands it the member name "ConnectorArn", so the branch that AWS steers to "fix your ConnectorArn" never fires and the error is mis-routed as a missing resource of an unspecified field. A client, log scrape or assertion that reads the refusal sentence likewise sees a different sentence than AWS emits, and sees CreateChallenge as indistinguishable from the sibling read ListChallengeMetadata, which legitimately words this class after the kind ("Connector was not found: <arn>.") and which the emulator already matches and must not change.

Note: open issue 20261125T223402Z-pca-connector-scep-createchallenge-s-resourcenotfoundexception-message-names-the-resource-kind.md covers the Message half from the behavioral corpus, but it carries no wire replay key for this fixture step, and it explicitly scopes ResourceType OUT — it argues ResourceType must stay "Connector" and declines to reopen it. This wire recording shows ResourceType must be "ConnectorArn" as well, so that issue as written cannot close this step.

Fix: CreateChallenge's error construction — its not_found helper in src/ops/create_challenge.rs — must report the input member that carried the unresolved reference for BOTH members of the exception, i.e. ResourceType = the member name ConnectorArn and a Message whose fixed text is "<that member name> was not found: " plus the request's ConnectorArn and ".". Because the shared builder in src/state.rs derives the sentence from the ResourceType word, either give that builder a member-name input (defaulting to the resource kind so ListChallengeMetadata, GetConnector, DeleteChallenge and the rest keep the kind-named sentence and kind-valued ResourceType they already match), or let CreateChallenge construct its own ResourceNotFoundException. Do not change ListChallengeMetadata's wording.

Traces: fixture pca-connector-scep-error-not-found-resources, step task#3 (expected vs actual on $.Message and $.ResourceType). Code citations: src/ops/create_challenge.rs:364-379 (not_found, and the doc comment that pins ResourceType to the kind); src/state.rs:859-865 (resource_type_of) and src/state.rs:867-894 (resource_not_found, message template at :890).

## Evidence
Fixture pca-connector-scep-error-not-found-resources, step task#3, operation CreateChallenge, verdict mismatch.

Expected status 404, actual status 404 (status and error code ResourceNotFoundException are correct). Diverging members:

  $.Message   value_mismatch
    expected: 'ConnectorArn was not found: arn:aws:pca-connector-scep:us-west-2:000000000000:connector/fa9ce23c-aedb-47ef-94d6-505cf953adba.'
    actual:   'Connector was not found: arn:aws:pca-connector-scep:us-west-2:000000000000:connector/fa9ce23c-aedb-47ef-94d6-505cf953adba.'
  $.ResourceType  value_mismatch
    expected: 'ConnectorArn'
    actual:   'Connector'

What is fixed text vs variable in the expected Message: the fixed text is "ConnectorArn was not found: " and the trailing "."; the only variable part is the ARN, which is the request's own ConnectorArn value. The region (us-west-2), account (000000000000) and connector UUID (fa9ce23c-...) above are this run's values and are NOT requirements. The expected ResourceType 'ConnectorArn' is entirely fixed text — the name of the input member.

Model (smithy_query on services/service-com-amazonaws-pcaconnectorscep-pcaconnectorscep/smithy-model/com-amazonaws-pcaconnectorscep-pcaconnectorscep.json):
  CreateChallenge: POST /challenges, http code 202; input ConnectorArn @required @length 5..200 pattern ^arn:aws(-[a-z]+)*:pca-connector-scep:...:connector/<uuid>$; errors include ResourceNotFoundException (client, httpError 404).
  ResourceNotFoundException.ResourceType doc: "The resource type, which can be either Connector or Challenge." — constrains nothing about Message, and the recording shows the member itself carrying the input-member name.

Emulator source producing both wrong members:
  services/service-com-amazonaws-pcaconnectorscep-pcaconnectorscep/src/state.rs:888-894
    pub fn resource_not_found(resource_type: &str, resource_id: &str) -> ResourceNotFoundException {
        ResourceNotFoundException {
            message: format!("{resource_type} was not found: {resource_id}."),
            resource_id: resource_id.to_string(),
            resource_type: resource_type.to_string(),
        }
    }
  services/service-com-amazonaws-pcaconnectorscep-pcaconnectorscep/src/ops/create_challenge.rs:373-379
    fn not_found(connector_arn: &str) -> CreateChallengeError {
        CreateChallengeErrorModeled::ResourceNotFoundException(crate::state::resource_not_found(
            crate::state::resource_type_of(connector_arn),
            connector_arn,
        ))
        .into()
    }
  src/state.rs:859-865 resource_type_of returns RESOURCE_TYPE_CONNECTOR ("Connector") for a connector ARN.

Contrast that must be preserved: ListChallengeMetadata (GET /challengeMetadata, same required ConnectorArn) answers this class as "Connector was not found: <arn>." with ResourceType "Connector", and the emulator already matches it.

## Steps to Reproduce
1. Boot the emulator and stand up the pca-connector-scep plane.
2. Construct a well-formed connector ARN that was never created: arn:aws:pca-connector-scep:<region>:<account>:connector/<fresh uuid4>.
3. POST /challenges with body {"ConnectorArn": "<that ARN>", "ClientToken": "<uuid4>"} signed for that region.
4. Observe HTTP 404 ResourceNotFoundException with Message "Connector was not found: <that ARN>." and ResourceType "Connector". AWS answers Message "ConnectorArn was not found: <that ARN>." and ResourceType "ConnectorArn".
5. For contrast, GET /challengeMetadata?ConnectorArn=<the same ARN>: both AWS and the emulator answer 404 with Message "Connector was not found: <that ARN>." and ResourceType "Connector" — that one is correct and must not change.
6. Re-replay fixture pca-connector-scep-error-not-found-resources, step task#3, to confirm the fix.
