Issue-Severity: CRITICAL
Issue-Gap-Class: missing
Issue-Commit: 946e8f5307921972a6976130c41f5f7a7b319937

# Bootstrap the ELB Proxy emulator

_Filed: 2026-11-27T18:24:57Z_
_Operation: service-com-amazonaws-elbproxy-elbproxy | Class: missing_

> _Note: this issue was identified against the code base at commit `946e8f5307921972a6976130c41f5f7a7b319937`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Every one of the 18 operations in this crate is an unmodified scaffold that answers HTTP 501 `NotImplemented`, and `src/state.rs` holds no state at all (`// TODO: add service-specific state fields`). No client workflow can run.

Implement a FIRST, SMALL happy-path slice around the primary resource, the **proxy**, so a realistic client script works end-to-end. Scope (in order):

1. **CreateProxy** — input `name` (1..32, pattern `(^[a-zA-Z0-9]+$|^[a-zA-Z0-9][a-zA-Z0-9-]*[a-zA-Z0-9]$)`, required), `type` (`ProxyType`, only `envoy`, required), `proxyVersion` (pattern `^\d+\.\d+$`, required), `roleArn` (required), `configuration` (`ProxyConfiguration{configurationFile, configurationArchive}`, required), `networkConfiguration` (`ProxyNetworkUserConfiguration{ipConfiguration, vpcConfiguration{subnets, securityGroups}}`, required), optional `attributes`, `customerKmsKeyArn`, `tags`, `clientToken` (`@idempotencyToken`). Must mint and PERSIST a new proxy: generate `arn` matching the identifier pattern the other operations require, `arn:aws:elasticloadbalancing:<region>:<account>:proxy/<name>:<generated-id>`; store the submitted configuration bytes, the network configuration, the role ARN, tags, attributes and a `createdAt` timestamp; derive `dnsName`; compute `configurationMetadata` (`configFileHash`/`configArchiveHash` as SHA-256 of the stored bytes, plus `configFileLastUpdatedAt`/`configArchiveLastUpdatedAt`); return `status` as a real `ProxyStatus{code, reasons}` using the modeled `ProxyStatusCode` values. Reuse of the same `clientToken` with the same input must return the SAME stored proxy rather than creating a second one; a second CreateProxy with an existing `name` must raise `ConflictException (HTTP 409)`.
2. **GetProxy** — input `proxyIdentifier` (the ARN, required). Must look the proxy up in the store and return the SAME `arn`, `name`, `type`, `proxyVersion`, `dnsName`, `roleArn`, `networkConfiguration` (as `ProxyNetworkConfiguration{ipConfiguration, vpcConfiguration{vpcId, availabilityZones, securityGroups}}`, derived from what CreateProxy stored), `status`, `configurationMetadata`, `createdAt`, `customerKmsKeyArn` that CreateProxy produced. An unknown-but-well-formed ARN must raise `ResourceNotFoundException (HTTP 404)`.
3. **GetProxyConfigurationContents** — input `proxyIdentifier` (required), `includeConfigFile`, `includeConfigArchive`. Must return the configuration bytes actually stored by CreateProxy: `configurationFileContents` only when `includeConfigFile` is true, `configurationArchiveContents` only when `includeConfigArchive` is true, plus the stored `configurationMetadata` and `customerKmsKeyArn`. Unknown proxy → `ResourceNotFoundException`.
4. **ListProxies** — input `maxResults`, `nextToken` (1..1024). Must enumerate the stored proxies as `items` (`list<ProxySummary>` with `arn`, `name`, `type`, `dnsName`, `vpcId`, `status`, `createdAt`), honour `maxResults` by truncating and emitting a `nextToken` that resumes exactly where the previous page stopped; the last page must omit `nextToken`. A created proxy must appear here and a deleted one must not.
5. **DeleteProxy** — input `proxyIdentifier` (required), `@idempotent`. Must remove the proxy (and its certificate associations and tags) from the store and return the required `arn` and `name` of the proxy it deleted, plus `status`. After it, GetProxy must answer `ResourceNotFoundException` and ListProxies must no longer include the proxy. Because it is `@idempotent`, a repeat delete of the same identifier must not panic.

Every one of these five MUST compute its response from persisted state. No canned bodies, no echoing the input back, and no minting an ARN or an ID that is never stored — a create followed by a get must return what was created, and a delete must be observable by the later get and list.

## State Model

In `ComAmazonawsElbproxyElbproxyState` (currently empty):

- `proxies: HashMap<String /* proxy ARN */, Proxy>`
- `proxies_by_name: HashMap<String /* name */, String /* ARN */>` (or an equivalent scan) to detect the `ConflictException` on duplicate `name`
- `idempotency: HashMap<String /* clientToken */, String /* ARN */>` so a replayed CreateProxy returns the first proxy
- `Proxy { arn: String, name: String, proxy_type: ProxyType, proxy_version: String, dns_name: String, role_arn: String, network_configuration: ProxyNetworkConfiguration, status: ProxyStatus /* code + reasons */, created_at: SystemTime, customer_kms_key_arn: Option<String>, config_file: Option<Vec<u8>>, config_archive: Option<Vec<u8>>, configuration_metadata: ConfigurationMetadata { config_file_hash, config_archive_hash, config_file_last_updated_at, config_archive_last_updated_at }, attributes: BTreeMap<String /* key */, String /* value */>, tags: BTreeMap<String, String>, certificate_associations: BTreeMap<String /* certificate ARN */, CertificateAssociation> }`
- a shared pagination helper that encodes/decodes `nextToken` for `ListProxies` so the later list operations (`ListCertificateAssociations`, `ListProxyUpdates`, `ListProxyVersions`) can reuse it.

The remaining 13 operations (AssociateCertificate, DisassociateCertificate, ListCertificateAssociations, GetProxyAttributes, UpdateProxyAttributes, UpdateProxyConfiguration, UpdateProxyNetworkConfiguration, GetProxyUpdate, ListProxyUpdates, ListProxyVersions, TagResource, UntagResource, ListTagsForResource) stay scaffolded at 501 and are filed as individual gap issues later — the state model above is laid out so they can be added on top without reshaping it.

## Evidence

All 18 handlers are unmodified scaffolds — e.g. `services/service-com-amazonaws-elbproxy-elbproxy/src/ops/create_proxy.rs:4`: `//! SCAFFOLDED, NOT IMPLEMENTED: this operation answers HTTP 501 `NotImplemented` and`; the identical marker is at line 4 of get_proxy.rs, get_proxy_configuration_contents.rs, list_proxies.rs, delete_proxy.rs and the other 13 files.

No state exists: `services/service-com-amazonaws-elbproxy-elbproxy/src/state.rs:7-9`:
```
pub struct ComAmazonawsElbproxyElbproxyState {
    // TODO: add service-specific state fields
}
```

Contract quoted from `services/service-com-amazonaws-elbproxy-elbproxy/OPERATIONS.md`:
- :53-54 `## CreateProxy` — `**Input:** `name` (string, len 1..32, ... required), `type` (ProxyType, one of: envoy, required), `proxyVersion` (... required), `roleArn` (... required), `configuration` (ProxyConfiguration, required), `networkConfiguration` (ProxyNetworkUserConfiguration, required), `attributes` (list<Attribute>), `customerKmsKeyArn` ..., `tags` ..., `clientToken` (... @idempotencyToken)`` and `**Output:** `arn` ..., `name` ..., `type` ..., `proxyVersion` ..., `dnsName` (string), `roleArn` ..., `networkConfiguration` (ProxyNetworkConfiguration), `status` (ProxyStatus), `configurationMetadata` (ConfigurationMetadata), `createdAt` (timestamp), `customerKmsKeyArn` ...`; errors include ``ConflictException (HTTP 409)``.
- :60-62 `## GetProxy` — `**Traits:** @readonly`, `**Input:** `proxyIdentifier` (string, len 1..2048, pattern ^arn:[a-z-]+:elasticloadbalancing:[a-z0-9-]+:[0-9]{12}:proxy/[a-zA-Z0-9-]+:[a-zA-Z0-9]+$, required)`, `**Errors:** `ResourceNotFoundException (HTTP 404)`, `ValidationException (HTTP 400)``.
- :102-103 `## GetProxyConfigurationContents` — `**Input:** `proxyIdentifier` ... required), `includeConfigFile` (boolean), `includeConfigArchive` (boolean)`, `**Output:** `configurationFileContents` (blob, len ..1048576), `configurationArchiveContents` (blob, len ..52428800), `configurationMetadata` (ConfigurationMetadata), `customerKmsKeyArn` ...`.
- :73-75 `## ListProxies` — `**Traits:** @paginated(inputToken=nextToken, outputToken=nextToken, items=items), @readonly`, `**Input:** `maxResults` (integer), `nextToken` (string, len 1..1024)`, `**Output:** `items` (list<ProxySummary>, required), `nextToken` (string, len 1..1024)`.
- :66-68 `## DeleteProxy` — `**Traits:** @idempotent`, `**Output:** `arn` (string, len 1..2048, required), `name` (... required), `status` (ProxyStatus)`.

Shape detail from `smithy_query`: `ConfigurationMetadata.configFileHash` — "The SHA-256 hash of the configuration file."; `ConfigurationMetadata.configArchiveHash` — "The SHA-256 hash of the configuration archive. This value is absent when the proxy does not have a configuration archive."; `ProxyStatus` — members `code` (`ProxyStatusCode`, required) and `reasons` (required), with `ProxyStatusCode` values `action-required, active, create-failed, create-in-progress, deleted, delete-in-progress, update-in-progress, cancel-update-in-progress`; `ProxySummary` members `arn, name, type, dnsName, vpcId, status, createdAt`.

The crate README states the stubs' contract: `services/service-com-amazonaws-elbproxy-elbproxy/README.md:38-42` — "Every generated stub starts out **scaffolded, not implemented**: it is fully typed and registered, and it answers **HTTP 501 `NotImplemented`** until you write it. It deliberately does not return an empty success — a hollow `200` with a zeroed body is indistinguishable from a real implementation to a client". The only existing test is the scaffold example `tests/all/state_roundtrip.rs`; no operation has behavioral coverage yet.

## Steps to Reproduce

1. Start the emulator and point an `elb-proxy` client (sigv4 signing name `elasticloadbalancing`) at it.
2. CreateProxy with name=`demo-proxy`, type=`envoy`, proxyVersion=`1.0`, roleArn=`arn:aws:iam::123456789012:role/elb-proxy`, configuration.configurationFile.configurationBytes=`<envoy yaml bytes>`, networkConfiguration with one subnet and one security group, tags={`env`:`dev`}, clientToken=`tok-1`. Expect 200 with a stored `arn` matching `arn:aws:elasticloadbalancing:<region>:123456789012:proxy/demo-proxy:<id>`, a non-empty `dnsName`, `status.code` set, `configurationMetadata.configFileHash` = SHA-256 of the submitted bytes, and `createdAt`. Today: HTTP 501 NotImplemented.
3. Repeat the identical CreateProxy call with clientToken=`tok-1`. Expect the SAME `arn` back (no second proxy).
4. CreateProxy again with name=`demo-proxy` and a fresh clientToken. Expect `ConflictException` (HTTP 409).
5. GetProxy with proxyIdentifier=<arn from step 2>. Expect every field to match what step 2 returned, read back from state.
6. GetProxyConfigurationContents with proxyIdentifier=<arn>, includeConfigFile=true, includeConfigArchive=false. Expect `configurationFileContents` byte-identical to the bytes submitted in step 2, `configurationArchiveContents` absent, and the same `configurationMetadata`.
7. ListProxies with maxResults=1 after creating a second proxy `demo-proxy-2`. Expect one `ProxySummary` item plus a `nextToken`; follow the token and expect the other proxy and no `nextToken`.
8. DeleteProxy with proxyIdentifier=<arn>. Expect 200 with `arn` and `name` of the deleted proxy.
9. GetProxy with the same ARN. Expect `ResourceNotFoundException` (HTTP 404). ListProxies must no longer include it.
10. Call DeleteProxy once more with the same ARN and confirm the server does not panic (`@idempotent`).
