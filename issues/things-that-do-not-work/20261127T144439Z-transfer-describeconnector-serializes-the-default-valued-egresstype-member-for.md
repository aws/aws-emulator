Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: transfer.connector-config-exclusivity-and-described-shape
Issue-Shape-Id: com.amazonaws.transfer#TransferService
Issue-Key: transfer.connector-config-exclusivity-and-described-shape|run#34|AssertionError|assert 'EgressType' not in conn, (
Issue-Commit: 9f25bb510dd848135c623da12e090eb624e6ceaa

# DescribeConnector serializes the default-valued EgressType member for a SERVICE_MANAGED connector instead of eliding it

> _Note: this issue was identified against the code base at commit `9f25bb510dd848135c623da12e090eb624e6ceaa`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeConnector

## Description
One net-new cause in the AWS Transfer Family emulator: a response-shape (member presence) divergence on a successfully routed 200 call.

Cause 1 of 1: DescribeConnector emits DescribedConnector.EgressType on the wire for a service-managed connector, where AWS omits it.

  AWS:      Smithy (services/service-com-amazonaws-transfer-transferservice/smithy-model/com-amazonaws-transfer-transferservice.json) models DescribedConnector.EgressType -> ConnectorEgressType with @required AND @default("SERVICE_MANAGED") (doc: "Type of egress configuration for the connector. SERVICE_MANAGED uses Transfer Family managed NAT gateways, while VPC_LATTICE routes traffic through customer VPCs using VPC Lattice."). A member carrying a @default is elided from the serialized payload when its value equals that default, so for a connector created WITHOUT any egress configuration (the plain AS2 connector case: CreateConnector with Url + AccessRole + As2Config only), the recorded AWS response is HTTP 200 whose $.Connector object contains NO EgressType key at all. Recorded behavior confirms the same for the two correlated members in that state: no EgressConfig member (populated only for VPC_LATTICE egress) and a non-empty ServiceManagedEgressIpAddresses list of IPv4 dotted quads. Only the EgressType presence diverged in this replay; the other two assertions were never reached.
  Emulator: the same call returns HTTP 200 but $.Connector.EgressType is present with the string value 'SERVICE_MANAGED'. Replay detail: "AssertionError: a SERVICE_MANAGED connector elides the defaulted EgressType member, got 'SERVICE_MANAGED'". The emulator stores/serializes the default eagerly instead of treating the default as "absent on the wire".
  Why:      a client that distinguishes egress modes by member presence — the documented way to tell a default service-managed connector from one that was explicitly configured, since EgressType is the discriminator for whether EgressConfig/VPC Lattice settings are meaningful — concludes the connector carries an explicit egress configuration. Round-tripping a described connector back into UpdateConnector then sends an egress type the original create never set, and SDK/assertion code written against AWS's payload (key absent) fails against the emulator. More generally, any @default member the emulator materializes eagerly will show the same class of divergence.
  Fix:      the DescribeConnector response serializer / DescribedConnector projection in the transfer service crate (the connector state -> DescribedConnector mapping): omit members whose value equals their modeled Smithy @default — specifically EgressType when it is SERVICE_MANAGED — rather than always writing the stored value.
  Traces:   behavioral check transfer.connector-config-exclusivity-and-described-shape (the check's earlier legs — As2Config/SftpConfig exclusivity, the invalid-Url rejection, and the per-member 'Compression/EncryptionAlgorithm/SigningAlgorithm is required in As2Config' rejections — all passed; the failure is exactly this member-presence assertion).

No open Transfer issue covers this cause: the closest open items are "Transfer response-field fidelity: ListAccesses IdP-type error message wording and DescribeCertificate Status derivation diverge" (20261127T085914Z, ListAccesses message text + DescribeCertificate Status derivation), "Transfer input validation, authorization and reference data diverge" (20261022T202037Z, connector Url/home-directory validation) and "Transfer host-key and web-app planes still refuse valid calls" (20261026T022407Z) — none touches DescribeConnector's default-member elision.

## Evidence
Behavioral suite simparity-behavioral-aws-v1, 125/168 passed; failing check transfer.connector-config-exclusivity-and-described-shape, verdict mismatch.

Failure site (check_transfer_connector_config_exclusivity_and_described_shape.py line 161):
  assert 'EgressType' not in conn, (
      f"a SERVICE_MANAGED connector elides the defaulted EgressType member, got {conn.get('EgressType')!r}")
  AssertionError: a SERVICE_MANAGED connector elides the defaulted EgressType member, got 'SERVICE_MANAGED'

Expected (AWS): CreateConnector(Url=<https url>, AccessRole=<role arn>, As2Config={LocalProfileId, PartnerProfileId, MdnResponse, Compression, EncryptionAlgorithm, SigningAlgorithm}) then DescribeConnector(ConnectorId=<returned id>) -> HTTP 200, $.Connector has NO 'EgressType' key (member equals its Smithy @default "SERVICE_MANAGED" and is therefore elided), NO 'EgressConfig' key, and a non-empty $.Connector.ServiceManagedEgressIpAddresses list of IPv4 dotted quads.

Actual (emulator): HTTP 200, $.Connector.EgressType == 'SERVICE_MANAGED' (key present).

Model citation: DescribedConnector.EgressType -> ConnectorEgressType, traits @required + @default("SERVICE_MANAGED"); DescribedConnector.EgressConfig -> DescribedConnectorEgressConfig (VPC_LATTICE only); DescribedConnector.ServiceManagedEgressIpAddresses -> list ("assigned automatically when you create the connector").

No status-code or error-code divergence is involved: the call succeeds in both, only the serialized member set differs. All identifiers, URLs and IP values in the recording are per-run and are not part of the requirement.

## Steps to Reproduce
1. Clone the repo and boot the emulator built from it.
2. transfer: CreateProfile(ProfileType='LOCAL') and CreateProfile(ProfileType='PARTNER'); iam: CreateRole with a transfer.amazonaws.com trust policy.
3. CreateConnector(Url='https://<host>.example.com/as2', AccessRole=<role arn>, As2Config={LocalProfileId=<local>, PartnerProfileId=<partner>, MdnResponse='NONE', Compression='DISABLED', EncryptionAlgorithm='NONE', SigningAlgorithm='NONE'}) — no egress configuration of any kind.
4. DescribeConnector(ConnectorId=<returned ConnectorId>) and inspect the raw response body's Connector object.
5. Observe 'EgressType': 'SERVICE_MANAGED' is present; AWS omits the key entirely because the value equals the member's modeled @default. Also confirm 'EgressConfig' is absent and ServiceManagedEgressIpAddresses is a non-empty IPv4 list.
6. Re-replay behavioral check transfer.connector-config-exclusivity-and-described-shape after the fix; it must pass to the end (delete + ResourceNotFoundException 'Unknown connector').
