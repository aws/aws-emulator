// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-transfer-transferservice service.
//!
//! Everything AWS Transfer Family's server + user + web app + AS2 profile + connector
//! + host-key lifecycle needs to answer a read from stored state lives here: the
//! resource maps (servers, their users and host keys, the web apps `ListWebApps`
//! pages, the AS2 profiles, the AS2 agreements a server's profiles trade under, the
//! connectors, and the
//! workflows a server's
//! `WorkflowDetails` attach), the
//! id-minting counter, the
//! documented create-time defaults and the clock-due server state transitions (the
//! `ServerTransition` rule `StartServer`/`StopServer` share), the ARN shapes, and the
//! opaque pagination token codec the `List*` operations share.
//!
//! The service-managed security policy catalog ([`SECURITY_POLICIES`]) lives here too.
//! No operation creates or changes a security policy — a server or connector only
//! attaches one by name — so the catalog is published data rather than per-account
//! state, and `DescribeSecurityPolicy` answers out of it.
//!
//! A tag set is NOT a map of its own: it belongs to the resource that carries it, so
//! the tag plane ([`ComAmazonawsTransferTransferserviceState::tags_of`],
//! [`ComAmazonawsTransferTransferserviceState::tags_mut`]) resolves a resource by its
//! stored ARN and reads or rewrites that resource's own `tags` — which is why a tag
//! written through `TagResource` is at once what the resource's own reader reports.
//!
//! The verdict `TestConnection` reports on a stored SFTP connector STARTS here
//! ([`TEST_CONNECTION_STATUS_ERROR`], [`sftp_configuration_defect`]) — the defects that
//! connector's stored configuration carries on its own — and is decided in
//! [`sftp_target`], which resolves the endpoint the connector's `Url` names against
//! this same store and answers for the connection to it. [`connector_delivery`] is the
//! other half of that endpoint: it carries an accepted transfer out against it.
//!
//! `UserCount` and `SshPublicKeyCount` are DERIVED at read time ([`
//! ComAmazonawsTransferTransferserviceState::user_count`],
//! [`User::ssh_public_key_count`]) rather than stored, so they cannot go stale when a
//! user or a key is added or removed. Every other member a reader reports is RESOLVED
//! once, when the resource is created or updated, so no reader has to re-apply a
//! default.
//!
//! The [`authorization`] submodule is the one gate — the server, user, SSH-key and
//! access-creating operations consult it before they act, and the other implemented
//! families do not yet (its own header records that scope): it reads the calling
//! principal's — and a passed role's — IAM policies out of the account's IAM store.
//!
//! The `Message` of a not-found, an already-exists or a documented state refusal is
//! built here, as a FIXED text per condition that carries no request data: the
//! identifier a caller needs to see travels in the error's own `Resource`/
//! `ResourceType` members, so an id can never reach a message a client logs or asserts
//! on, and a text can never be derived from an internal name. The ONE exception is a
//! condition whose recorded wording itself names the resource in the text
//! ([`no_web_app_customization`]): the service's own words are the requirement, so that
//! text is reproduced as a template whose variable part is read off the REQUEST, never
//! off an internal name. A handler still words
//! its own refusals for conditions this module holds no text for (a store-capacity
//! refusal, say); the one message built from the request is the framework's
//! `ValidationException`, whose template (member path + modelled constraint, no value)
//! lives in `crate::service`.

pub mod as2_connector_requirements;
pub mod authorization;
pub mod connector_delivery;
pub mod home_directory_mappings;
pub mod identity_provider;
pub mod vended_log_group;
pub mod connector_egress_addresses;
pub mod connector_egress_type;
pub mod connector_endpoint_url;
pub mod connector_plane;
pub mod connector_protocol;
pub mod identity_center;
pub mod remote_storage;
pub mod sftp_target;
pub mod ssh_public_key_body;
pub mod tagged_resource;
pub mod web_app_customization;
pub mod web_app_write_claim;
pub mod workflow_runs;
mod security_policies;
mod host_key_material;
pub use connector_egress_addresses::service_managed_egress_ip_addresses;

/// Whether the endpoint a connector's `Url` names can exist, and WHICH of the two fixed
/// texts the service refuses one that cannot with: their own concern, in
/// [`connector_endpoint_url`], re-exported here beside the other connector rules the
/// write path reads.
pub use connector_endpoint_url::{endpoint_url_defect, URL_CARRIES_PATH_INFORMATION, URL_NOT_VALID};

/// Which `As2Config` members an AS2 connector cannot be written without: their own
/// concern, in [`as2_connector_requirements`], re-exported here beside the other
/// connector rules the write path reads.
pub use as2_connector_requirements::as2_configuration_violation;
pub use host_key_material::*;
pub use security_policies::*;

/// Which kind of resource an ARN names: the tag plane's own concern, in
/// [`tagged_resource`], re-exported here so every reader of this module's not-found
/// payloads names them the way it always has.
pub use tagged_resource::unknown_tagged_resource;

/// One run of a workflow, and the store the runs live in: they are their own concern,
/// in [`workflow_runs`], and re-exported here so every reader of this module's state
/// names them the way it always has.
pub use workflow_runs::WorkflowExecution;

use std::collections::BTreeMap;
use std::time::{Duration, Instant, SystemTime};

use crate::ops::types::{
    AccessDeniedException, As2AsyncMdnConnectorConfig, As2ConnectorConfig, ConflictException,
    ConnectorEgressConfig,
    ConnectorFileTransferResult, ConnectorVpcLatticeEgressConfig, CustomDirectoriesType,
    DescribedWebAppEndpointDetails,
    DescribedWebAppIdentityProviderDetails, EndpointDetails, HomeDirectoryMapEntry,
    IdentityProviderDetails, InvalidNextTokenException, InvalidRequestException, PosixProfile,
    ProtocolDetails, ProxyConfig,
    ResourceExistsException, ResourceNotFoundException, S3StorageOptions, SftpConnectorConfig,
    SftpPortWithOptions, Tag, UpdateConnectorEgressConfig, WebAppUnits, WorkflowDetails,
    WorkflowStep,
};

/// The region used for an ARN when the request carries no SigV4 credential scope to
/// read one from. Matches the region the emulator's generated SDK profile uses.
pub const DEFAULT_REGION: &str = "us-east-1";

/// `arn:aws:transfer:…` — this service's ARN namespace, per `OPERATIONS.md`.
const ARN_SERVICE: &str = "transfer";

/// The security policy a server gets when `CreateServer` names none.
///
/// AWS Transfer Family's current default security policy for a new server
/// ("TransferSecurityPolicy-2024-01 ... is the default security policy", AWS Transfer
/// Family User Guide, *Security policies for servers*), confirmed by the recorded
/// response of a server created without `SecurityPolicyName`. It satisfies
/// `SecurityPolicyName`'s modelled pattern
/// `^Transfer[A-Za-z0-9]*SecurityPolicy-[A-Za-z0-9-]+$`.
pub const DEFAULT_SECURITY_POLICY_NAME: &str = "TransferSecurityPolicy-2024-01";

/// `ProtocolDetails.PassiveIp`'s default.
///
/// Modelled doc: "The value `PassiveIp=AUTO` is assigned by default"; the recorded
/// response reports it for every server, whatever its protocols.
pub const DEFAULT_PASSIVE_IP: &str = "AUTO";

/// `ProtocolDetails.TlsSessionResumptionMode`'s documented default ("If a
/// TlsSessionResumptionMode value is not specified during CreateServer, it is set to
/// ENFORCED by default").
pub const DEFAULT_TLS_SESSION_RESUMPTION_MODE: &str = "ENFORCED";

/// `ProtocolDetails.SetStatOption`'s default: the `SetStatOption` enum's `DEFAULT`
/// member, i.e. `SETSTAT` is not ignored.
pub const DEFAULT_SET_STAT_OPTION: &str = "DEFAULT";

/// `ProtocolDetails.SftpPorts`' documented default port ("If you don't specify any
/// configurations during CreateServer, the service uses port 22 with
/// SERVER_TALK_FIRST by default").
pub const DEFAULT_SFTP_PORT: i32 = 22;

/// `SftpPortWithOptions.CommunicationMode`'s documented default — the other half of
/// the `SftpPorts` default sentence quoted on [`DEFAULT_SFTP_PORT`].
pub const DEFAULT_COMMUNICATION_MODE: &str = "SERVER_TALK_FIRST";

/// `ProtocolDetails.ProxyConfig.SftpMode`'s documented default ("If you don't specify
/// a value, the default is NONE").
pub const DEFAULT_PROXY_SFTP_MODE: &str = "NONE";

/// `S3StorageOptions.DirectoryListingOptimization`'s documented default ("If using the
/// API or CLI, this is disabled by default").
pub const DEFAULT_DIRECTORY_LISTING_OPTIMIZATION: &str = "DISABLED";

/// `Domain`'s `S3` value (the `Domain` enum is `[S3, EFS]`).
pub const DOMAIN_S3: &str = "S3";

/// `Domain`'s documented default ("The default value is S3").
pub const DEFAULT_DOMAIN: &str = DOMAIN_S3;

/// The `IdentityProviderType` enum's values (`[SERVICE_MANAGED, API_GATEWAY,
/// AWS_DIRECTORY_SERVICE, AWS_LAMBDA]`). The three below `SERVICE_MANAGED` are the
/// ones `IdentityProviderDetails` documents itself as required for.
pub const IDENTITY_PROVIDER_SERVICE_MANAGED: &str = "SERVICE_MANAGED";
pub const IDENTITY_PROVIDER_API_GATEWAY: &str = "API_GATEWAY";
pub const IDENTITY_PROVIDER_AWS_DIRECTORY_SERVICE: &str = "AWS_DIRECTORY_SERVICE";
pub const IDENTITY_PROVIDER_AWS_LAMBDA: &str = "AWS_LAMBDA";

/// `IdentityProviderType`'s documented default ("The default value is SERVICE_MANAGED").
pub const DEFAULT_IDENTITY_PROVIDER_TYPE: &str = IDENTITY_PROVIDER_SERVICE_MANAGED;

/// The `EndpointType` enum's two VPC-hosted values (`[PUBLIC, VPC, VPC_ENDPOINT]`).
/// A server with either one hosts its endpoint inside a VPC, so it is configured by
/// `EndpointDetails`.
pub const ENDPOINT_TYPE_VPC: &str = "VPC";
pub const ENDPOINT_TYPE_VPC_ENDPOINT: &str = "VPC_ENDPOINT";

/// `EndpointType`'s default: a server with no VPC endpoint settings is reachable over
/// the public internet.
pub const DEFAULT_ENDPOINT_TYPE: &str = "PUBLIC";

/// `IpAddressType`'s documented default ("The default value is IPV4").
pub const DEFAULT_IP_ADDRESS_TYPE: &str = "IPV4";

/// `IpAddressType`'s dual-stack value (the `IpAddressType` enum is
/// `[IPV4, DUALSTACK]`).
pub const IP_ADDRESS_TYPE_DUALSTACK: &str = "DUALSTACK";

/// `WebAppEndpointPolicy`'s documented default ("Setting for the type of endpoint
/// policy for the web app. The default value is STANDARD"). The enum is
/// `[FIPS, STANDARD]`.
pub const DEFAULT_WEB_APP_ENDPOINT_POLICY: &str = "STANDARD";

/// `WebAppEndpointType`'s publicly-accessible value (the enum is `[PUBLIC, VPC]`),
/// which is what a web app created without `EndpointDetails` is hosted on: the member
/// is documented as "PUBLIC for publicly accessible endpoints and VPC for VPC-hosted
/// endpoints that provide network isolation", and the only `WebAppEndpointDetails`
/// member is `Vpc`.
pub const WEB_APP_ENDPOINT_TYPE_PUBLIC: &str = "PUBLIC";

/// `WebAppEndpointType`'s VPC-hosted value — see [`WEB_APP_ENDPOINT_TYPE_PUBLIC`].
pub const WEB_APP_ENDPOINT_TYPE_VPC: &str = "VPC";

/// The `Protocol` enum values this service names (the enum is
/// `[SFTP, FTP, FTPS, AS2]`): `SFTP`, the one a connector speaks and the one
/// `Protocols` defaults to, and the three whose documentation states a requirement on
/// another member of the server.
pub const PROTOCOL_SFTP: &str = "SFTP";
pub const PROTOCOL_FTP: &str = "FTP";
pub const PROTOCOL_FTPS: &str = "FTPS";
pub const PROTOCOL_AS2: &str = "AS2";

/// `Protocols`' default: SFTP alone, the only protocol a `PUBLIC`,
/// `SERVICE_MANAGED` server can serve.
pub const DEFAULT_PROTOCOL: &str = PROTOCOL_SFTP;

/// `HomeDirectoryType`'s value that makes `HomeDirectoryMappings` the user's home
/// directory (the enum is `[PATH, LOGICAL]`).
pub const HOME_DIRECTORY_TYPE_LOGICAL: &str = "LOGICAL";

/// `HomeDirectoryType` for a user whose write named none.
///
/// `HomeDirectoryType`'s documentation pairs the two values with the member each one
/// is configured through: "If HomeDirectoryType is LOGICAL, you must provide
/// mappings, using the HomeDirectoryMappings parameter. If, on the other hand,
/// HomeDirectoryType is PATH, you provide an absolute path using the HomeDirectory
/// parameter." [`home_directory_violation`] refuses every write that breaks that
/// pairing, so a write that names NO type configured a path rather than mappings —
/// a `PATH` user, which is what the recorded Transfer wire reports for a `CreateUser`
/// that named no type.
pub const DEFAULT_HOME_DIRECTORY_TYPE: &str = "PATH";

/// The `HomeDirectoryType` a user write leaves on the stored record.
///
/// The caller's value whenever it sent one, else [`DEFAULT_HOME_DIRECTORY_TYPE`].
/// Every user write resolves the type here, so the stored record carries a type for
/// every user and the readers project it without defaulting a second time.
pub fn resolved_home_directory_type(requested: Option<String>) -> String {
    requested.unwrap_or_else(|| DEFAULT_HOME_DIRECTORY_TYPE.to_string())
}

/// The page bound a `List*` call uses when the caller sends no `MaxResults`.
///
/// Equal to `MaxResults`' modelled maximum (`range 1..1000`), so an unbounded
/// request still cannot answer more than one modelled page.
pub const MAX_PAGE_SIZE: usize = 1000;

/// How many servers the store holds before `CreateServer` refuses with the modelled
/// `ServiceUnavailableException` instead of growing.
///
/// AWS Transfer Family publishes an account- and Region-level quota on servers that an
/// account can have raised, and this emulator models neither Service Quotas nor any
/// account's adjustments — so this is the emulator's OWN bound on the store, not a
/// claim about any account's limit. It is far above any workflow an emulator serves,
/// and it is what stops `servers` growing without end: every record it holds is
/// released by `DeleteServer`, which also drops that server's users and their keys, so
/// the user map cannot outlive the bound either.
pub const MAX_STORED_SERVERS: usize = 200;

/// How many users ONE server holds before `CreateUser` refuses with the modelled
/// `ServiceUnavailableException` instead of growing.
///
/// Chosen on the same basis as [`MAX_STORED_SERVERS`] — the emulator's own bound, not
/// an account's quota. Together the two cap the whole store at
/// `MAX_STORED_SERVERS * MAX_USERS_PER_SERVER` users, and each stored record is itself
/// bounded by the modelled lengths of the members it holds (`HomeDirectoryMappings`
/// `len 1..50000`, `Policy` `len 0..2048`, `SshPublicKeyBody` `len 0..2048`, `Tags`
/// `len 1..50`), which the framework's boundary validator enforces before a handler
/// runs. `DeleteUser` releases one record; `DeleteServer` releases all of a server's.
pub const MAX_USERS_PER_SERVER: usize = 1_000;

/// How many web apps the store holds before `CreateWebApp` refuses instead of growing.
///
/// Chosen on the same basis as [`MAX_STORED_SERVERS`] — the emulator's OWN bound on
/// the store, not a claim about any account's quota (this emulator models no Service
/// Quotas). It is what stops `web_apps` growing without end; `DeleteWebApp` releases a
/// record. Each stored record is itself bounded by the modelled lengths of the members
/// it holds (`WebAppAccessEndpoint` `len 1..1024`, `Tags` `len 1..50`), which the
/// framework's boundary validator enforces before a handler runs.
pub const MAX_STORED_WEB_APPS: usize = 200;

/// How many connectors the store holds before a connector create refuses with the
/// modelled `ServiceUnavailableException` instead of growing.
///
/// Chosen on the same basis as [`MAX_STORED_SERVERS`] — the emulator's OWN bound on
/// the store, not a claim about any account's quota. It is what stops `connectors`
/// growing without end: every write consults
/// [`ComAmazonawsTransferTransferserviceState::has_connector_capacity`] before it
/// inserts, and `DeleteConnector` releases a record again. Each stored record is
/// itself bounded by the modelled lengths of the members it holds (`Url`
/// `len 0..255`), which the framework's boundary validator enforces before a handler
/// runs, so the bound caps the map in bytes as well as in count.
pub const MAX_STORED_CONNECTORS: usize = 200;

/// How many AS2 profiles the store holds before `CreateProfile` refuses with the
/// modelled `ServiceUnavailableException` instead of growing.
///
/// Chosen on the same basis as [`MAX_STORED_SERVERS`] — the emulator's OWN bound on
/// the store, not a claim about any account's quota, since this emulator models
/// neither Service Quotas nor an account's adjustments. Each stored record is itself
/// bounded by the modelled lengths of the members it holds (`As2Id` `len 1..128`,
/// `CertificateIds` entries `len 22..22`, `Tags` `len 1..50`), which the framework's
/// boundary validator enforces before a handler runs, and `DeleteProfile` releases a
/// record.
pub const MAX_STORED_PROFILES: usize = 200;

/// How long after `CreateServer` or `StartServer` a server leaves `STARTING` for
/// `ONLINE`.
///
/// AWS documents the transition ("it can take a couple of minutes for the server to
/// be completely operational") but not a span the caller can rely on, so this span is
/// CHOSEN: the span this emulator gives every transition the clock owes. It has to
/// outlast a caller that acts straight after the call AND keeps calling — the several
/// requests a client makes before it looks at `State` again, each of which takes
/// unbounded wall-clock time on a loaded host — or the transient state, and the state
/// errors that depend on it, would be gone before that caller could observe either.
/// The due instant is stored and materialised on the next read; nothing sleeps for it.
pub const SERVER_START_SPAN: Duration = Duration::from_secs(3);

/// How long after `StopServer` a server leaves `STOPPING` for `OFFLINE`.
///
/// AWS documents the intermediate `STOPPING` state but no span, so this is CHOSEN on
/// the same basis, and at the same length, as [`SERVER_START_SPAN`]: a caller polling
/// straight after `StopServer` still observes `STOPPING`, and the `OFFLINE` it waits
/// for comes due after the span without anything sleeping.
pub const SERVER_STOP_SPAN: Duration = Duration::from_secs(3);

/// A server's `State` right after `CreateServer`: instantiated, not yet able to serve
/// file operations.
pub const SERVER_STATE_STARTING: &str = "STARTING";

/// A server's `State` once the start the create began has completed.
pub const SERVER_STATE_ONLINE: &str = "ONLINE";

/// A server's `State` right after `StopServer`: the intermediate state the model
/// documents as "not fully able to respond, or not fully offline".
pub const SERVER_STATE_STOPPING: &str = "STOPPING";

/// A server's `State` once the stop has completed: the server exists but cannot
/// perform file transfer operations.
pub const SERVER_STATE_OFFLINE: &str = "OFFLINE";

/// A server's `State` when a start failed: the model documents `START_FAILED` as "an
/// error condition", which a caller answers by starting the server again. Nothing in
/// this emulator writes it — a start it begins always completes — but the shared
/// transition rule names it so a retry from it would begin a start rather than be
/// refused.
pub const SERVER_STATE_START_FAILED: &str = "START_FAILED";

/// A server's `State` when a stop failed. The counterpart of
/// [`SERVER_STATE_START_FAILED`], on the same basis.
pub const SERVER_STATE_STOP_FAILED: &str = "STOP_FAILED";

/// The HTTP status AWS Transfer Family answers `ResourceNotFoundException` with for a
/// RESOURCE REFERENCE it cannot resolve.
///
/// The model puts `@httpError(404)` on the shape, but the recorded Transfer wire
/// answers a reference it cannot resolve with HTTP **400** and `__type`
/// `ResourceNotFoundException` — so the status on the wire is 400 and the shape's
/// `httpError` does not describe the service. Handlers return it through
/// [`not_found`], which keeps the body the model's own and changes only the status
/// line.
///
/// The status belongs to the SERVICE, not to one operation: every operation of this
/// service that reports an unresolvable reference — including
/// `SendWorkflowStepState`'s callback, whose unknown execution and already-consumed
/// token are the same condition — answers it here, so a client that branches on the
/// status line before it reads `__type` classifies all of them alike.
pub const RESOURCE_NOT_FOUND_HTTP_STATUS: u16 = 400;

/// `ResourceType` for the server resource: the resource name capitalized, as the
/// recorded Transfer error bodies carry it.
pub const RESOURCE_TYPE_SERVER: &str = "Server";

/// `ResourceType` for the user resource. Same convention as [`RESOURCE_TYPE_SERVER`].
pub const RESOURCE_TYPE_USER: &str = "User";

/// `ResourceType` for one of a user's SSH public keys, addressed by its
/// `SshPublicKeyId`. Same convention as [`RESOURCE_TYPE_SERVER`].
pub const RESOURCE_TYPE_SSH_PUBLIC_KEY: &str = "SshPublicKey";

/// `ResourceType` for a directory access grant, addressed by its `ExternalId`. Same
/// convention as [`RESOURCE_TYPE_SERVER`].
pub const RESOURCE_TYPE_ACCESS: &str = "Access";

/// An `ExternalId` the resolved server holds no access grant for. Its own entry in the
/// not-found table, like every other kind.
pub const NOT_FOUND_ACCESS: NotFoundResource = NotFoundResource {
    resource_type: RESOURCE_TYPE_ACCESS,
    message: "Unknown access",
};

/// `ResourceType` for the connector resource, addressed by its `ConnectorId`. Same
/// convention as [`RESOURCE_TYPE_SERVER`].
pub const RESOURCE_TYPE_CONNECTOR: &str = "Connector";

/// A `ConnectorId` nothing in the store answers to. Its own entry in the not-found
/// table, like every other kind.
pub const NOT_FOUND_CONNECTOR: NotFoundResource = NotFoundResource {
    resource_type: RESOURCE_TYPE_CONNECTOR,
    message: "Unknown connector",
};

/// `ResourceType` for the web app resource. Same convention as
/// [`RESOURCE_TYPE_SERVER`].
pub const RESOURCE_TYPE_WEB_APP: &str = "WebApp";

/// A `WebAppId` nothing in the store answers to. Its own entry in the not-found
/// table, like every other kind.
///
/// The text names the web app resource as the service's own recorded wording does —
/// the camel-case resource name, as [`NOT_FOUND_SECURITY_POLICY`] also spells it —
/// rather than a prose rendering of it.
pub const NOT_FOUND_WEB_APP: NotFoundResource = NotFoundResource {
    resource_type: RESOURCE_TYPE_WEB_APP,
    message: "Unknown WebApp",
};

/// `ResourceType` for the AS2 profile resource, addressed by its `ProfileId`. Same
/// convention as [`RESOURCE_TYPE_SERVER`].
pub const RESOURCE_TYPE_PROFILE: &str = "Profile";

/// `ResourceType` for the workflow resource, addressed by its `WorkflowId`. Same
/// convention as [`RESOURCE_TYPE_SERVER`].
pub const RESOURCE_TYPE_WORKFLOW: &str = "Workflow";

/// `ResourceType` for one execution of a workflow, addressed by its `ExecutionId`.
/// Same convention as [`RESOURCE_TYPE_SERVER`].
pub const RESOURCE_TYPE_EXECUTION: &str = "Execution";

/// An `ExecutionId` the resolved workflow is not running. Its own entry in the
/// not-found table, like every other kind.
pub const NOT_FOUND_EXECUTION: NotFoundResource = NotFoundResource {
    resource_type: RESOURCE_TYPE_EXECUTION,
    message: "Unknown execution",
};

/// `ResourceType` as the duplicate-`UserName` `ResourceExistsException` carries it:
/// the resource name in lower case, which is the value this service has answered that
/// one condition with since its duplicate check was written. Nothing recorded
/// contradicts either spelling, so both are left exactly as they were; only the
/// `Message` of that error is this module's concern.
pub const RESOURCE_TYPE_USER_LOWERCASE: &str = "user";

/// One kind of Transfer resource as a not-found error REPORTS it: the `ResourceType`
/// the error body carries, and the fixed `Message` the service answers for a
/// reference to that kind it cannot resolve.
///
/// The two travel together, in a table of named constants, because the `Message` is
/// the only thing in the body that says WHICH reference was bad: deriving it from the
/// `ResourceType` token (or from any other internal name) would make the
/// caller-visible wording follow this crate's internals. A resource kind is added by
/// adding a constant below, with the text the service answers for it.
#[derive(Debug, Clone, Copy)]
pub struct NotFoundResource {
    /// The `ResourceType` member of the error body.
    pub resource_type: &'static str,
    /// The `Message` member: fixed for this kind, carrying no request data.
    pub message: &'static str,
}

/// A `ServerId` nothing in the store answers to.
pub const NOT_FOUND_SERVER: NotFoundResource = NotFoundResource {
    resource_type: RESOURCE_TYPE_SERVER,
    message: "Unknown server",
};

/// A `UserName` the resolved server does not hold.
pub const NOT_FOUND_USER: NotFoundResource = NotFoundResource {
    resource_type: RESOURCE_TYPE_USER,
    message: "Unknown user",
};

/// An `SshPublicKeyId` the resolved user does not hold. The service names this
/// resource by its ID in the text, which is what distinguishes it from a user that
/// could not be resolved.
pub const NOT_FOUND_SSH_PUBLIC_KEY: NotFoundResource = NotFoundResource {
    resource_type: RESOURCE_TYPE_SSH_PUBLIC_KEY,
    message: "Unknown public key ID",
};

/// A `ProfileId` nothing in the store answers to.
pub const NOT_FOUND_PROFILE: NotFoundResource = NotFoundResource {
    resource_type: RESOURCE_TYPE_PROFILE,
    message: "Unknown profile",
};

/// A `WorkflowId` nothing in the store answers to.
pub const NOT_FOUND_WORKFLOW: NotFoundResource = NotFoundResource {
    resource_type: RESOURCE_TYPE_WORKFLOW,
    message: "Unknown workflow",
};

/// The fixed `Message` for a `CreateUser` whose `UserName` the server already holds.
pub const USER_ALREADY_EXISTS: &str = "User already exists";

/// The fixed `Message` for an `ImportSshPublicKey` whose body the user already holds.
pub const SSH_PUBLIC_KEY_ALREADY_EXISTS: &str = "Public key already exists";

/// The fixed `Message` for an `ImportHostKey` whose material the server already holds.
pub const HOST_KEY_ALREADY_EXISTS: &str = "Host key already exists";

/// The fixed `Message` for an `UpdateServer` that would change `IpAddressType` on a
/// server that is not [`SERVER_STATE_OFFLINE`].
///
/// The model states the rule on `IpAddressType` itself: "It cannot be changed while
/// the server is online. You must stop the server before modifying this parameter."
/// The text states that requirement, so it is the same whichever non-`OFFLINE` state
/// the server is in.
pub const IP_ADDRESS_TYPE_NEEDS_OFFLINE_SERVER: &str =
    "Server must be OFFLINE to change IpAddressType";

/// The `WorkflowStepType` enum's values (`[COPY, CUSTOM, TAG, DELETE, DECRYPT]`).
/// Each one names the ONE `WorkflowStep.*StepDetails` member that configures it, which
/// is what [`workflow_step_violation`] enforces.
pub const WORKFLOW_STEP_TYPE_COPY: &str = "COPY";
pub const WORKFLOW_STEP_TYPE_CUSTOM: &str = "CUSTOM";
pub const WORKFLOW_STEP_TYPE_TAG: &str = "TAG";
pub const WORKFLOW_STEP_TYPE_DELETE: &str = "DELETE";
pub const WORKFLOW_STEP_TYPE_DECRYPT: &str = "DECRYPT";

/// The fixed `Message` the service answers for a [`WORKFLOW_STEP_TYPE_CUSTOM`] step
/// that names no Lambda function to invoke — the one thing such a step acts through
/// (`CustomStepDetails` "consists of the Lambda function's name, target, and timeout",
/// and `Target` is "The ARN for the Lambda function that is being called").
///
/// FIXED text in its entirety: the refusal reports the defect, not the request, so it
/// carries no step index, no member path and no step name — the same sentence whichever
/// step list the step sat in and wherever in it.
pub const CUSTOM_STEP_FUNCTION_INVALID: &str = "Invalid function provided";

/// `SourceFileLocation`'s documented default, on every step-details shape that carries
/// the member ("To use the previous file as the input, enter `${previous.file}`. In
/// this case, this workflow step uses the output file from the previous workflow step
/// as input. This is the default value").
pub const DEFAULT_SOURCE_FILE_LOCATION: &str = "${previous.file}";

/// `OverwriteExisting`'s documented default, on every step-details shape that carries
/// the member ("A flag that indicates whether to overwrite an existing file of the same
/// name. The default is `FALSE`"). It is the `OverwriteExisting` enum's `FALSE` value
/// (the enum is `[FALSE, TRUE]`).
pub const DEFAULT_OVERWRITE_EXISTING: &str = "FALSE";

/// The `ExecutionStatus` enum's values (`[IN_PROGRESS, COMPLETED, EXCEPTION,
/// HANDLING_EXCEPTION]`), documented on `ListedExecution.Status` as "in progress,
/// completed, exception encountered, or handling the exception".
///
/// `HANDLING_EXCEPTION` has no constant: it is the window in which a workflow is
/// running its `OnExceptionSteps`, and this emulator runs no step, so no execution it
/// holds is ever inside that window.
pub const EXECUTION_STATUS_IN_PROGRESS: &str = "IN_PROGRESS";
pub const EXECUTION_STATUS_COMPLETED: &str = "COMPLETED";
pub const EXECUTION_STATUS_EXCEPTION: &str = "EXCEPTION";

/// The `CustomStepStatus` enum's values (`[SUCCESS, FAILURE]`) — the outcome a
/// `SendWorkflowStepState` callback reports for the custom step its `Token` addresses,
/// and so the two outcomes [`WorkflowExecution::reported_status`] reads back off a
/// step.
pub const CUSTOM_STEP_STATUS_SUCCESS: &str = "SUCCESS";
pub const CUSTOM_STEP_STATUS_FAILURE: &str = "FAILURE";

/// How many executions ONE workflow holds before an upload starts no further one.
///
/// The emulator's OWN bound on the store, chosen on the same basis as
/// [`MAX_TRANSFERS_PER_CONNECTOR`] — not a claim about any account's quota. Each record
/// is itself bounded by what it holds: one file location, one set of session details,
/// and one pending-step entry per `CUSTOM` step of a workflow whose `Steps` the model
/// bounds. See [`ComAmazonawsTransferTransferserviceState::take_execution_capacity`]
/// for how room is made, and `DeleteWorkflow` for what releases a workflow's records.
pub const MAX_EXECUTIONS_PER_WORKFLOW: usize = 100;

/// How many executions the whole store holds, across every workflow, before an upload
/// starts no further one.
///
/// The per-workflow bound alone would let a caller that keeps creating workflows (or
/// recycling identifiers) grow the store without end, so the store is bounded in total
/// as well — the same two-level bound [`MAX_STORED_WORKFLOWS`] and
/// [`MAX_USERS_PER_SERVER`] give the workflow and user maps.
pub const MAX_STORED_EXECUTIONS: usize = 1_000;

/// How many workflows the store holds before `CreateWorkflow` refuses with the
/// modelled `ServiceUnavailableException` instead of growing.
///
/// Chosen on the same basis as [`MAX_STORED_SERVERS`] — the emulator's own bound on the
/// store, not a claim about any account's Service Quota, which this emulator does not
/// model. Each stored workflow is itself bounded by the modelled lengths of what it
/// holds (`Steps` and `OnExceptionSteps` `len 0..8`, `Description` `len 0..256`, `Tags`
/// `len 1..50`, `StructuredLogDestinations` `len 0..1`), which the framework's boundary
/// validator enforces before a handler runs, and `DeleteWorkflow` releases one.
pub const MAX_STORED_WORKFLOWS: usize = 200;

/// `ResourceType` for one of a server's host keys, addressed by its `HostKeyId`.
/// Same convention as [`RESOURCE_TYPE_SERVER`].
pub const RESOURCE_TYPE_HOST_KEY: &str = "HostKey";

/// A `HostKeyId` the resolved server does not hold — never imported, or already
/// deleted. Its own entry in the not-found table, like every other kind.
pub const NOT_FOUND_HOST_KEY: NotFoundResource = NotFoundResource {
    resource_type: RESOURCE_TYPE_HOST_KEY,
    message: "Unknown host key",
};

/// `HostKeyStorageType`'s value for a key the service itself stores (the enum is
/// `[SERVICE_MANAGED, SECRETS_MANAGER]`).
pub const HOST_KEY_STORAGE_SERVICE_MANAGED: &str = "SERVICE_MANAGED";

/// `HostKeyStorageType`'s value for a key whose private material lives in a Secrets
/// Manager secret.
pub const HOST_KEY_STORAGE_SECRETS_MANAGER: &str = "SECRETS_MANAGER";

/// `StorageType` for an import that named none.
///
/// `StorageType` documents Secrets Manager storage as the OPT-IN half of the pair
/// ("When you specify SECRETS_MANAGER, Transfer Family stores the private host key
/// material in an Secrets Manager secret"), so a request that specifies nothing leaves
/// the key stored by Transfer Family itself.
pub const DEFAULT_HOST_KEY_STORAGE_TYPE: &str = HOST_KEY_STORAGE_SERVICE_MANAGED;

/// How many host keys ONE server holds before `ImportHostKey` refuses instead of
/// growing.
///
/// Chosen on the same basis as [`MAX_STORED_SERVERS`] — the emulator's own bound on
/// the store, not a claim about any account's quota. Each stored record is itself
/// bounded by the modelled lengths of what a caller may send (`HostKeyBody`
/// `len 0..4096`, `Description` `len 0..200`, `Tags` `len 1..50`), which the
/// framework's boundary validator enforces before a handler runs, so this bounds a
/// server's key set in bytes as well as in count. `DeleteHostKey` releases one record,
/// `DeleteServer` all of a server's.
pub const MAX_HOST_KEYS_PER_SERVER: usize = 10;

/// How many SSH public keys ONE user holds before `ImportSshPublicKey` refuses
/// instead of growing.
///
/// AWS Transfer Family's published service quota for a Transfer Family user is 10 SSH
/// public keys, so that is the bound here; this emulator models no Service Quotas
/// adjustments, so it is a fixed one. Each stored key is itself bounded by
/// `SshPublicKeyBody`'s modelled `len 0..2048`, which the framework's boundary
/// validator enforces before a handler runs, so this bounds a user's key set in bytes
/// as well as in count. `DeleteSshPublicKey` releases one, `DeleteUser` all of them.
pub const MAX_SSH_KEYS_PER_USER: usize = 10;

// -----------------------------------------------------------------------------
// Connectors
// -----------------------------------------------------------------------------

/// The `ConnectorEgressType` enum's values (`[SERVICE_MANAGED, VPC_LATTICE]`):
/// `SERVICE_MANAGED` "uses Transfer Family managed NAT gateways", `VPC_LATTICE`
/// "routes traffic through customer VPCs using VPC Lattice".
pub const CONNECTOR_EGRESS_TYPE_SERVICE_MANAGED: &str = "SERVICE_MANAGED";
pub const CONNECTOR_EGRESS_TYPE_VPC_LATTICE: &str = "VPC_LATTICE";

/// `DescribedConnector.EgressType`'s modelled default (`@default("SERVICE_MANAGED")`):
/// a connector created with no `EgressConfig` is "a connector without egress
/// configuration", i.e. a service-managed one.
pub const DEFAULT_CONNECTOR_EGRESS_TYPE: &str = CONNECTOR_EGRESS_TYPE_SERVICE_MANAGED;

/// `DescribedConnector.Status`'s modelled default (`@default("ACTIVE")`), documented as
/// "ready for operations".
///
/// This is the status a created connector carries: `CreateConnector` records the
/// parameters of a connection and the connector is usable as soon as it answers, and
/// the `ConnectorStatus` enum's other values describe a create or update still in
/// flight (`PENDING`) or one that failed (`ERRORED`) — neither of which this emulator
/// can enter, because a configuration it cannot hold is refused at the create call
/// itself rather than recorded and then failed.
pub const CONNECTOR_STATUS_ACTIVE: &str = "ACTIVE";

/// `ConnectorVpcLatticeEgressConfig.PortNumber`'s documented default ("Defaults to 22
/// if not specified"), which is also the SFTP port.
pub const DEFAULT_VPC_LATTICE_PORT: i32 = 22;

/// `SftpConnectorConfig.MaxConcurrentConnections`' documented default ("The default
/// value is `1`"), which the model also carries as `@default(1)`.
pub const DEFAULT_MAX_CONCURRENT_CONNECTIONS: i32 = 1;

/// The Secrets Manager version stage a connector reads the SFTP user's credentials
/// from when `OrderedUserSecretVersionStages` names none.
///
/// `OrderedUserSecretVersionStages` is "an ordered list of Amazon Web Services Secrets
/// Manager version stages (staging labels, such as `AWSCURRENT` and `AWSPREVIOUS`) for
/// the secret identified by `UserSecretId`", and `AWSCURRENT` is the label Secrets
/// Manager keeps on a secret's current version — so a connector that names no order
/// reads the current version alone, which is the one-stage order a connector created
/// without the member reports.
pub const DEFAULT_SECRET_VERSION_STAGE: &str = "AWSCURRENT";

/// The security policy an SFTP connector gets when no `SecurityPolicyName` is named.
///
/// `ConnectorSecurityPolicyName`'s modelled pattern
/// `^TransferSFTPConnectorSecurityPolicy-[A-Za-z0-9-]+$` admits only the SFTP-connector
/// generations of [`SECURITY_POLICIES`], and this is the current non-FIPS one of them,
/// as the recorded response of a connector created without the member reports. The
/// policy is published by this service's own catalog, so the policy a connector reports
/// is one a caller can describe.
pub const DEFAULT_CONNECTOR_SECURITY_POLICY_NAME: &str =
    "TransferSFTPConnectorSecurityPolicy-2024-03";

/// `SftpConnectorConfig.MaxConcurrentConnections`' documented maximum ("The maximum
/// values is `5`").
///
/// The member's modelled `@range` carries only `min: 1`, so the boundary validator
/// refuses 0 and below but not 6 and above: the upper bound lives in the member's own
/// documentation, so it is checked by the create handler.
pub const MAX_CONCURRENT_CONNECTIONS: i32 = 5;

/// The not-found payload for a reference of `kind` the store could not resolve.
///
/// The identifier travels in `Resource` ALONE: `Message` is the fixed text the
/// service answers for that KIND of reference ([`NotFoundResource::message`]), so one
/// caller's id never reaches the message a client logs or asserts on, and the text
/// does not change when an internal name does. Every operation in this service that
/// cannot find a resource builds its error here, so the construction cannot drift
/// between them.
pub fn resource_not_found(kind: &NotFoundResource, resource: &str) -> ResourceNotFoundException {
    ResourceNotFoundException {
        message: kind.message.to_string(),
        resource: resource.to_string(),
        resource_type: kind.resource_type.to_string(),
    }
}

/// The not-found payload for a `ServerId` the store does not hold — never created, or
/// already deleted.
pub fn unknown_server(server_id: &str) -> ResourceNotFoundException {
    resource_not_found(&NOT_FOUND_SERVER, server_id)
}

/// The not-found payload for a `UserName` the named server does not hold.
pub fn unknown_user(user_name: &str) -> ResourceNotFoundException {
    resource_not_found(&NOT_FOUND_USER, user_name)
}

/// The not-found payload for an `SshPublicKeyId` the named user does not hold —
/// never imported, or already deleted.
pub fn unknown_ssh_public_key(ssh_public_key_id: &str) -> ResourceNotFoundException {
    resource_not_found(&NOT_FOUND_SSH_PUBLIC_KEY, ssh_public_key_id)
}

/// The not-found payload for an `ExternalId` the named server holds no access grant
/// for — never granted, or already deleted.
pub fn unknown_access(external_id: &str) -> ResourceNotFoundException {
    resource_not_found(&NOT_FOUND_ACCESS, external_id)
}

/// The not-found payload for a `ConnectorId` the store does not hold — never created,
/// or already deleted.
pub fn unknown_connector(connector_id: &str) -> ResourceNotFoundException {
    resource_not_found(&NOT_FOUND_CONNECTOR, connector_id)
}

/// The not-found payload for an `ExecutionId` the named workflow is not running — the
/// execution never started, or has already finished.
pub fn unknown_execution(execution_id: &str) -> ResourceNotFoundException {
    resource_not_found(&NOT_FOUND_EXECUTION, execution_id)
}

/// The message the modelled `AccessDeniedException` carries.
///
/// It is the shape's own documented sentence
/// (`smithy_query(query="shape",shape="AccessDeniedException")`: "You do not have
/// sufficient access to perform this action."). AWS's own text names the caller's ARN
/// and the action, which is account- and principal-specific; the requirement is the
/// 403 classification and the code, so the modelled sentence is what this states, and
/// it deliberately says nothing about whether the named resource exists.
pub const INSUFFICIENT_ACCESS_MESSAGE: &str =
    "You do not have sufficient access to perform this action.";

/// The authorization refusal for a request this emulator cannot authorize.
///
/// Carries [`INSUFFICIENT_ACCESS_MESSAGE`] and nothing else, so the refusal is the
/// same for every request and discloses nothing about the resource it named. Returned
/// through the operation's own modelled `AccessDeniedException` variant, so it keeps
/// the model's HTTP 403.
///
/// WHICH identifier is judged here, and which is reported not found, is settled by the
/// resource a permission for the action NAMES: `SendWorkflowStepState` is addressed at
/// a workflow (its `ExecutionId` and `Token` address a run and a step INSIDE that
/// workflow), so an unresolvable `WorkflowId` is refused here and an execution the
/// resolved workflow is not running is reported not found. An operation whose input
/// names ONE resource and nothing inside it resolves that resource through the
/// modelled not-found instead (`DescribeWebApp`), so this refusal never makes an
/// operation's own `ResourceNotFoundException` unreachable.
pub fn access_denied() -> AccessDeniedException {
    AccessDeniedException {
        message: Some(INSUFFICIENT_ACCESS_MESSAGE.to_string()),
    }
}

/// Whether `identity_provider_type` names a CUSTOMER-run identity provider — one the
/// server delegates authentication to — rather than Transfer's own user store.
///
/// The `IdentityProviderType` enum is `[SERVICE_MANAGED, API_GATEWAY,
/// AWS_DIRECTORY_SERVICE, AWS_LAMBDA]`, and the three non-`SERVICE_MANAGED` values are
/// exactly the ones `IdentityProviderDetails` documents itself as required for: each
/// one points at something outside the service (a REST endpoint, a directory, a
/// function). `SERVICE_MANAGED` points at nothing: its users are the ones `CreateUser`
/// stores.
pub fn is_external_identity_provider(identity_provider_type: &str) -> bool {
    matches!(
        identity_provider_type,
        IDENTITY_PROVIDER_API_GATEWAY
            | IDENTITY_PROVIDER_AWS_DIRECTORY_SERVICE
            | IDENTITY_PROVIDER_AWS_LAMBDA
    )
}

/// `TestIdentityProvider`'s refusal for a server that delegates authentication to
/// nothing, so there is no identity provider for the call to test.
///
/// The wording is the model's own: "If you provide a server ID for a server that uses
/// service-managed users, you get an error: An error occurred
/// (InvalidRequestException) when calling the TestIdentityProvider operation:
/// s-server-ID not configured for external auth" — the one documented message of this
/// refusal, built from the `ServerId` the request named.
pub fn not_configured_for_external_auth(server_id: &str) -> InvalidRequestException {
    InvalidRequestException {
        message: format!("{server_id} not configured for external auth"),
    }
}

/// `TestIdentityProvider`'s `StatusCode` for a test that got no HTTP response back.
///
/// `StatusCode` is "the HTTP status code that is the response from your API Gateway or
/// your Lambda function", and the model gives the member `@default(0)`: the value an
/// answer carries when there is no such status to report. That is the answer for a
/// provider endpoint this emulator serves nothing at, and for one it serves that never
/// answered the authentication request — with [`identity_provider_test_message`] or
/// [`IDENTITY_PROVIDER_NO_ANSWER`] saying which, and no `Response`, which is what the
/// operation documents for a test that did not get one ("If you provide any incorrect
/// values for any parameters, the `Response` field is empty").
pub const IDENTITY_PROVIDER_NO_RESPONSE_STATUS: i32 = 0;

/// `Message` for a test of a server whose `IdentityProviderDetails` names no endpoint
/// for the provider type it delegates authentication to.
///
/// Each of the three external types is configured through ONE member of
/// `IdentityProviderDetails` — `Url` for `API_GATEWAY`, `Function` for `AWS_LAMBDA`,
/// `DirectoryId` for `AWS_DIRECTORY_SERVICE` — so without it the server names nothing
/// to authenticate against, whatever else the request carries.
const IDENTITY_PROVIDER_NAMES_NO_ENDPOINT: &str = "The server's IdentityProviderDetails \
     names no endpoint for its IdentityProviderType, so there is no identity provider \
     for this test to authenticate against.";

/// `Message` for a test of a server whose identity provider IS named, and is an endpoint
/// this emulator serves nothing at: a REST endpoint of an API no emulation here holds, or
/// the directory an `AWS_DIRECTORY_SERVICE` server names, which this emulator does not run.
/// The test got as far as the authentication request itself, which is what reached no
/// provider.
const IDENTITY_PROVIDER_NOT_INVOKED: &str = "The stored identity provider \
     configuration is complete, but the authentication request could not be made: this \
     emulator opens no outbound connection and runs no customer identity provider, so \
     the provider returned no response.";

/// `Message` for a test whose authentication request WAS made against a provider this
/// emulator serves and got no answer back: the function or the endpoint the server names
/// could not be invoked, or never answered.
///
/// Fixed for that condition, like every other text here, and it claims no outcome: a
/// request that was not answered has no status and no body, so the answer carries
/// [`IDENTITY_PROVIDER_NO_RESPONSE_STATUS`] and no `Response` beside it.
pub const IDENTITY_PROVIDER_NO_ANSWER: &str = "The authentication request was made \
     against the identity provider the server names, and it returned no response: the \
     function could not be invoked, or did not answer.";

/// The endpoint of the service `server` authenticates its users against, as
/// `TestIdentityProvider`'s required `Url` reports it ("the endpoint of the service used
/// to authenticate a user").
///
/// Read off the ONE `IdentityProviderDetails` member the server's own
/// `IdentityProviderType` is configured through. The EMPTY string — which `Url`'s
/// `len 0..255` admits — for a server that names none, because there is then no endpoint
/// to report and none may be invented.
pub fn identity_provider_endpoint(server: &Server) -> String {
    let Some(details) = server.identity_provider_details.as_ref() else {
        return String::new();
    };
    let named = match server.identity_provider_type.as_str() {
        IDENTITY_PROVIDER_API_GATEWAY => details.url.as_deref(),
        IDENTITY_PROVIDER_AWS_LAMBDA => details.function.as_deref(),
        IDENTITY_PROVIDER_AWS_DIRECTORY_SERVICE => details.directory_id.as_deref(),
        _ => None,
    };
    named
        .map(|endpoint| endpoint.trim().to_string())
        .unwrap_or_default()
}

/// The `Message` a test of `server`'s identity provider reports: "a message that
/// indicates whether the test was successful or not".
///
/// Decided from the server's STORED configuration alone, like the configuration
/// verdict [`sftp_configuration_defect`] gives a connector: each text is fixed for the
/// condition it states and carries no request data, and none of them claims an outcome
/// for an authentication request nothing made.
pub fn identity_provider_test_message(server: &Server) -> &'static str {
    if identity_provider_endpoint(server).is_empty() {
        return IDENTITY_PROVIDER_NAMES_NO_ENDPOINT;
    }
    IDENTITY_PROVIDER_NOT_INVOKED
}

/// What an access operation DOES to the grant it addresses, as that operation's
/// directory-provider refusal names the action: one word per operation, taken from the
/// operation's own name, so the refusal text is fixed for the operation rather than
/// built from anything the caller sent.
pub const ACCESS_ACTION_CREATE: &str = "create";
pub const ACCESS_ACTION_DESCRIBE: &str = "describe";
pub const ACCESS_ACTION_LIST: &str = "list";
pub const ACCESS_ACTION_UPDATE: &str = "update";
pub const ACCESS_ACTION_DELETE: &str = "delete";

/// The refusal for an access-grant request addressed at a server that has no
/// directory to hold groups.
///
/// An access grant IS a directory group: `ExternalId` is documented as "A unique
/// identifier that is required to identify specific groups within your directory",
/// read out of Active Directory with `Get-ADGroup`. Only a server whose
/// `IdentityProviderType` is `AWS_DIRECTORY_SERVICE` authenticates against a
/// directory, so on any other server there is no group an `ExternalId` could name —
/// which is a malformed request, not a missing grant.
///
/// The text is fixed for the operation and the server's configuration: `action` is the
/// operation's own verb (one of [`ACCESS_ACTION_CREATE`], [`ACCESS_ACTION_DESCRIBE`],
/// [`ACCESS_ACTION_UPDATE`], [`ACCESS_ACTION_DELETE`]) and `identity_provider_type` is
/// a value of the
/// `IdentityProviderType` enum's closed set, read off the STORED server. Nothing the
/// caller sent — no `ServerId`, no `ExternalId` — reaches it, so two requests refused
/// for the same condition on equally configured servers read identically.
pub fn accesses_require_directory_provider(
    action: &str,
    identity_provider_type: &str,
) -> InvalidRequestException {
    InvalidRequestException {
        message: format!("Cannot {action} access on server with IdP type: {identity_provider_type}"),
    }
}

/// The not-found payload for a `WebAppId` the store does not hold — never created, or
/// already deleted.
pub fn unknown_web_app(web_app_id: &str) -> ResourceNotFoundException {
    resource_not_found(&NOT_FOUND_WEB_APP, web_app_id)
}

/// The not-found payload for a web app that HOLDS no customization — none was ever
/// assigned to it, or `DeleteWebAppCustomization` removed the one that was.
///
/// The web app itself resolved, so this reports the missing sub-resource rather than a
/// bad `WebAppId`: it is the one condition in this service whose recorded wording names
/// the resource inside the text, so the text is a template over the `WebAppId` the
/// REQUEST carried (see this module's header) while `Resource`/`ResourceType` stay what
/// every other web-app refusal carries.
pub fn no_web_app_customization(web_app_id: &str) -> ResourceNotFoundException {
    ResourceNotFoundException {
        message: format!("The WebApp {web_app_id} doesn't have customization"),
        resource: web_app_id.to_string(),
        resource_type: RESOURCE_TYPE_WEB_APP.to_string(),
    }
}

/// The not-found payload for a `ProfileId` the store does not hold — never created,
/// or already deleted.
pub fn unknown_profile(profile_id: &str) -> ResourceNotFoundException {
    resource_not_found(&NOT_FOUND_PROFILE, profile_id)
}

/// The already-exists payload for a `UserName` the named server already holds.
///
/// `Message` is the service's fixed text for the condition; the name travels in
/// `Resource`, which is where a caller reads which reference collided.
pub fn user_already_exists(user_name: &str) -> ResourceExistsException {
    ResourceExistsException {
        message: USER_ALREADY_EXISTS.to_string(),
        resource: user_name.to_string(),
        resource_type: RESOURCE_TYPE_USER_LOWERCASE.to_string(),
    }
}

/// The already-exists payload for an SSH public key body the named user already
/// holds. Same construction as [`user_already_exists`]: fixed text, identifier in
/// `Resource`.
pub fn ssh_public_key_already_exists(user_name: &str) -> ResourceExistsException {
    ResourceExistsException {
        message: SSH_PUBLIC_KEY_ALREADY_EXISTS.to_string(),
        resource: user_name.to_string(),
        resource_type: RESOURCE_TYPE_USER.to_string(),
    }
}

/// The already-exists payload for host key material the resolved server already holds,
/// addressed by the `HostKeyId` of the key that holds it.
///
/// Same construction as [`user_already_exists`]: the `Message` is the service's fixed
/// text for the condition and interpolates nothing — not the server the request named,
/// not the colliding key's id, not a fingerprint of the submitted material — while the
/// identifier a caller needs travels in `Resource`.
pub fn host_key_already_exists(host_key_id: &str) -> ResourceExistsException {
    ResourceExistsException {
        message: HOST_KEY_ALREADY_EXISTS.to_string(),
        resource: host_key_id.to_string(),
        resource_type: RESOURCE_TYPE_HOST_KEY.to_string(),
    }
}

/// The not-found payload for a `WorkflowId` the store does not hold — never created,
/// or already deleted.
pub fn unknown_workflow(workflow_id: &str) -> ResourceNotFoundException {
    resource_not_found(&NOT_FOUND_WORKFLOW, workflow_id)
}

/// `InvalidRequestException`'s `Message` for a `DeleteWorkflow` of a workflow a server
/// still attaches through its `WorkflowDetails`, as the service states it: the fixed
/// sentence followed by the one variable part, the id of the referencing server — read
/// from this service's own state
/// ([`ComAmazonawsTransferTransferserviceState::servers_using_workflow`]), since the
/// request names only the workflow.
pub fn workflow_in_use_by_server(server_id: &str) -> InvalidRequestException {
    InvalidRequestException {
        message: format!("Workflow is still in use by server {server_id}"),
    }
}

/// Lift an operation's modelled not-found variant into its error channel, answered
/// with [`RESOURCE_NOT_FOUND_HTTP_STATUS`] rather than the model's `@httpError`.
///
/// Generic over the operation's own `*ErrorModeled` enum, so every handler reaches
/// the one status decision through this function.
pub fn not_found<E>(error: E) -> crate::scaffolded::Scaffolded<E> {
    crate::scaffolded::Scaffolded::modeled_with_status(error, RESOURCE_NOT_FOUND_HTTP_STATUS)
}

/// The HTTP status this service answers a refusal it RELAYS from a call it made to
/// another service on the caller's behalf with.
///
/// The decision such a refusal reports was not taken by this service: it is the answer
/// the dependent call itself got, and the recorded Transfer wire answers it with the
/// status that call carried — HTTP **400** — rather than the `@httpError` of the shape
/// this service relays it on. So the body is the model's own and only the status line
/// follows the dependent call, exactly as [`RESOURCE_NOT_FOUND_HTTP_STATUS`] does for a
/// not-found. A refusal this service decides ITSELF keeps its shape's modelled status
/// (an identity-policy deny is answered 403), so the two are not confused.
pub const DEPENDENT_REFUSAL_HTTP_STATUS: u16 = 400;

/// Lift an operation's modelled variant for a RELAYED refusal into its error channel,
/// answered with [`DEPENDENT_REFUSAL_HTTP_STATUS`].
pub fn dependent_refusal<E>(error: E) -> crate::scaffolded::Scaffolded<E> {
    crate::scaffolded::Scaffolded::modeled_with_status(error, DEPENDENT_REFUSAL_HTTP_STATUS)
}

/// The refusal for an `API_GATEWAY` identity provider whose `InvocationRole` could
/// not be shown able to invoke the API the provider names.
///
/// The message names the API the request gave, so the caller knows which one could
/// not be verified; a request that gave an invocation role but no `Url` names no API
/// to quote, and is told exactly that rather than an empty value.
pub fn unverifiable_identity_provider(url: Option<&str>) -> String {
    match url {
        Some(url) => format!("Unable to verify access to API {url}"),
        None => {
            "Unable to verify access to the API named by IdentityProviderDetails.Url".to_string()
        }
    }
}

/// The `SshPublicKeyBody` the service STORES for a submitted one, and so the body
/// every reader of the key reports back.
///
/// Trailing whitespace is removed and nothing else is touched: the key material and
/// the comment after it are kept byte-identical, so a client that fingerprints what
/// it reads back fingerprints what it sent. The modelled pattern ends
/// `(\s*|[ \t]+[\S \t]*\s*)$`, so the trailing-whitespace form is accepted on the
/// wire and normalised here rather than refused — the recorded service behaviour.
/// Every write that takes a key body (`CreateUser`, `ImportSshPublicKey`) stores the
/// result of this function, so two writes of one key cannot differ by whitespace.
pub fn stored_ssh_public_key_body(submitted: &str) -> String {
    submitted.trim_end().to_string()
}

/// The refusal for a session policy that is not a JSON document — `None` when the
/// request names a policy this service can parse, or names none at all.
///
/// `Policy` is modelled `@length 0..2048` with NO pattern, so a document's syntax is
/// checked by the service and not at the boundary: a scope-down policy is a JSON
/// document, and one that does not parse is refused before it can be stored and
/// silently become a user's effective permissions. The empty string the member's
/// `len 0..2048` admits names no policy rather than a malformed one.
pub fn policy_document_violation(member: &str, policy: Option<&str>) -> Option<String> {
    let policy = policy?;
    if policy.trim().is_empty() {
        return None;
    }
    ::serde_json::from_str::<::serde_json::Value>(policy)
        .is_err()
        .then(|| format!("{member} is not a valid JSON document"))
}

/// One stored SSH public key of a user.
#[derive(Debug, Clone)]
pub struct SshKey {
    /// The minted `key-` + 17 hex-digit identifier (`^key-[0-9a-f]{17}$`).
    pub ssh_public_key_id: String,
    /// The key material as [`stored_ssh_public_key_body`] normalises it.
    pub ssh_public_key_body: String,
    /// When this key was added to the user.
    pub date_imported: SystemTime,
}

/// One host key a caller IMPORTED into a server, keyed by `(ServerId, HostKeyId)`.
///
/// A server's host KEYS are the keys `ImportHostKey` added to it: the key the service
/// manages for the server itself is not one of them — it has no `HostKeyId` to address
/// and no import date — and surfaces only as the server's own
/// `HostKeyFingerprint` ([`Server::host_key_fingerprint`]).
///
/// The PRIVATE material the import carried is NOT kept: nothing in this emulator can
/// use it (it serves no SSH transport) and no modelled member returns it. What the
/// readers report is derived from that material once, at import time — the genuine
/// OpenSSH fingerprint and algorithm name [`read_host_key`] parses out of it — so a
/// read reports a real property of a real key rather than an echo of the request. The
/// PUBLIC half is kept, because it is what a client that connects to the server is
/// shown: it is the key the server presents on the host-key exchange, which
/// [`sftp_target`] reports as the host key observed on a connection to it.
#[derive(Debug, Clone)]
pub struct HostKey {
    /// The minted `hostkey-` + 17 hex-digit identifier (`^hostkey-[0-9a-f]{17}$`).
    pub host_key_id: String,
    /// `arn:aws:transfer:<region>:<account>:host-key/<ServerId>/<HostKeyId>`.
    pub arn: String,
    /// The Base64 SHA256 fingerprint of the imported key, in the `SHA256:<base64>`
    /// form every member that reports a fingerprint carries ([`fingerprint_of`]).
    pub fingerprint: String,
    /// `DescribedHostKey.Type`: the imported key's SSH algorithm name, one of the
    /// values that member documents (`ssh-rsa`, `ssh-ed25519`,
    /// `ecdsa-sha2-nistp256/384/521`).
    pub key_type: String,
    /// The caller's `Description`, absent when the import named none.
    pub description: Option<String>,
    /// `DateImported`: when this key was added to the server.
    pub date_imported: SystemTime,
    /// The resolved `HostKeyStorageType` (see [`DEFAULT_HOST_KEY_STORAGE_TYPE`]).
    pub storage_type: String,
    /// The public half of this key, in the `key-type key-body` form `ssh-keygen` writes
    /// — the key a client connecting to the server is presented with, computed from the
    /// key's own material by [`read_host_key`]/[`mint_host_key`] and never invented.
    pub public_key: String,
    pub tags: Vec<Tag>,
}

/// One stored Transfer Family workflow, keyed by `WorkflowId`.
///
/// The steps are held in the caller's own order — a workflow runs its steps in the
/// order they were submitted — with every documented step-detail default already
/// applied by [`resolve_workflow_steps`], so a reader projects them without
/// defaulting a second time.
#[derive(Debug, Clone)]
pub struct Workflow {
    pub workflow_id: String,
    pub arn: String,
    pub description: Option<String>,
    pub steps: Vec<WorkflowStep>,
    pub on_exception_steps: Vec<WorkflowStep>,
    pub structured_log_destinations: Vec<String>,
    pub tags: Vec<Tag>,
}

/// One stored Transfer Family user, keyed by `(ServerId, UserName)`.
#[derive(Debug, Clone)]
pub struct User {
    pub server_id: String,
    pub user_name: String,
    pub arn: String,
    pub role: String,
    pub home_directory: Option<String>,
    pub home_directory_type: Option<String>,
    pub home_directory_mappings: Vec<HomeDirectoryMapEntry>,
    pub policy: Option<String>,
    pub posix_profile: Option<PosixProfile>,
    pub ssh_public_keys: Vec<SshKey>,
    pub tags: Vec<Tag>,
}

impl User {
    /// `SshPublicKeyCount`: the live number of keys stored for this user.
    pub fn ssh_public_key_count(&self) -> i32 {
        i32::try_from(self.ssh_public_keys.len()).unwrap_or(i32::MAX)
    }
}

/// One stored AS2 profile, keyed by `ProfileId`.
///
/// Everything `DescribeProfile` and `ListProfiles` report is RESOLVED here, by the
/// create and by every later write, so a reader projects the stored record without
/// re-deriving anything: the minted id, its ARN, the AS2-name, the profile type, the
/// tags the create carried and the certificate ids the last profile write established.
#[derive(Debug, Clone)]
pub struct Profile {
    /// The minted `p-` + 17 hex-digit identifier (`^p-([0-9a-f]{17})$`).
    pub profile_id: String,
    pub arn: String,
    /// The AS2-name (RFC 4130) exactly as the caller supplied it.
    pub as2_id: String,
    /// `LOCAL` or `PARTNER` — the `ProfileType` enum, enforced before a handler runs.
    pub profile_type: String,
    /// The certificate identifiers a profile write named, stored as given: a profile
    /// REFERENCES certificates by id, and no operation of this service resolves that
    /// reference against the certificate map, so nothing here rewrites or drops an id.
    pub certificate_ids: Vec<String>,
    pub tags: Vec<Tag>,
}

impl Profile {
    /// Apply one profile write's `CertificateIds` to this stored profile.
    ///
    /// A write that NAMES the member replaces the whole attachment list with the ids it
    /// carries — an empty list attaches none — and a write that leaves the member out
    /// names no new value at all, so the profile keeps the attachments its last write
    /// established. One place for that rule, so every read of the profile reports the
    /// ids the last write left.
    pub fn apply_certificate_ids(&mut self, certificate_ids: Option<Vec<String>>) {
        if let Some(certificate_ids) = certificate_ids {
            self.certificate_ids = certificate_ids;
        }
    }
}

/// One stored Transfer Family server, keyed by `ServerId`.
#[derive(Debug, Clone)]
pub struct Server {
    pub server_id: String,
    pub arn: String,
    pub domain: String,
    pub identity_provider_type: String,
    pub identity_provider_details: Option<IdentityProviderDetails>,
    pub endpoint_type: String,
    pub endpoint_details: Option<EndpointDetails>,
    pub protocols: Vec<String>,
    /// The protocol settings, with every member the model documents a default for
    /// already resolved at create time — so a reader reports what the server is
    /// configured with rather than "unconfigured".
    pub protocol_details: ProtocolDetails,
    pub logging_role: Option<String>,
    pub security_policy_name: String,
    pub pre_authentication_login_banner: Option<String>,
    pub post_authentication_login_banner: Option<String>,
    /// The Base64 SHA256 fingerprint of the server's host key, in the
    /// `SHA256:<base64>` form every member that reports a fingerprint carries
    /// ([`fingerprint_of`]).
    ///
    /// Computed at create time from a host key the emulator GENERATES for the server
    /// (as the service does for a server whose host key it manages) — see
    /// [`mint_host_key`]. Never invented: it is the real fingerprint of
    /// real key material.
    pub host_key_fingerprint: String,
    /// The PUBLIC half of the host key this server serves with, in the `key-type
    /// key-body` form `ssh-keygen` writes — the key a client connecting to the server
    /// is presented with on the host-key exchange.
    ///
    /// Computed from the same material as [`Self::host_key_fingerprint`], so the two
    /// always describe ONE key, and empty only for material that would not encode.
    pub host_key_public: String,
    /// The log group structured logging is configured to publish to, or `None` when
    /// the server has NO structured-logging configuration.
    ///
    /// "Not configured" and "configured with nothing" are not the same server: a
    /// reader reports the first by OMITTING `StructuredLogDestinations` and the second
    /// cannot exist, because the model documents an empty value as what CLEARS the
    /// setting ("you can clear it, and in effect turn off structured logging, by
    /// providing an empty value for this parameter"). See
    /// [`configured_log_destinations`].
    pub structured_log_destinations: Option<Vec<String>>,
    /// The S3 storage settings, defaulted at create time like
    /// [`Self::protocol_details`].
    pub s3_storage_options: S3StorageOptions,
    pub ip_address_type: String,
    pub workflow_details: Option<WorkflowDetails>,
    pub certificate: Option<String>,
    pub state: String,
    /// The state this server owes the clock, and when it comes due.
    ///
    /// Set by `CreateServer`/`StartServer` to `(now + SERVER_START_SPAN, "ONLINE")` and
    /// by `StopServer` to `(now + SERVER_STOP_SPAN, "OFFLINE")`; materialised by
    /// [`Server::settle`] on the next read, never by sleeping.
    pub state_due: Option<(Instant, String)>,
    pub tags: Vec<Tag>,
}

impl Server {
    /// Apply the state transition the clock has already made due, if any.
    ///
    /// Called by the reporting operations that render `State`. It writes only a change
    /// that was owed at `now` whether or not anybody called, so two reads at the same
    /// instant answer the same thing.
    pub fn settle(&mut self, now: Instant) {
        if let Some((due, next)) = self.state_due.clone() {
            if now >= due {
                self.state = next;
                self.state_due = None;
            }
        }
    }

    /// Begin the stop the clock will complete: the intermediate `STOPPING` now,
    /// `OFFLINE` when [`SERVER_STOP_SPAN`] has elapsed.
    pub fn begin_stop(&mut self, now: Instant) {
        self.state = SERVER_STATE_STOPPING.to_string();
        self.state_due = Some((now + SERVER_STOP_SPAN, SERVER_STATE_OFFLINE.to_string()));
    }

    /// Begin the start the clock will complete: the intermediate `STARTING` now,
    /// `ONLINE` when [`SERVER_START_SPAN`] has elapsed.
    pub fn begin_start(&mut self, now: Instant) {
        self.state = SERVER_STATE_STARTING.to_string();
        self.state_due = Some((now + SERVER_START_SPAN, SERVER_STATE_ONLINE.to_string()));
    }

    /// What `StopServer` does to this server, by its current `State`.
    pub fn stop_decision(&self) -> ServerTransition {
        ServerTransition::decide(
            &self.state,
            SERVER_STATE_ONLINE,
            SERVER_STATE_OFFLINE,
            SERVER_STATE_STOPPING,
            SERVER_STATE_STOP_FAILED,
        )
    }

    /// What `StartServer` does to this server, by its current `State`.
    pub fn start_decision(&self) -> ServerTransition {
        ServerTransition::decide(
            &self.state,
            SERVER_STATE_OFFLINE,
            SERVER_STATE_ONLINE,
            SERVER_STATE_STARTING,
            SERVER_STATE_START_FAILED,
        )
    }

    /// Whether this server's `WorkflowDetails` attach `workflow_id` to either of the
    /// two triggers the shape carries (`OnUpload`, `OnPartialUpload`).
    ///
    /// A workflow attached to a server is IN USE by it, which is what `DeleteWorkflow`
    /// refuses on.
    pub fn references_workflow(&self, workflow_id: &str) -> bool {
        let Some(details) = self.workflow_details.as_ref() else {
            return false;
        };
        [&details.on_upload, &details.on_partial_upload]
            .into_iter()
            .flatten()
            .flatten()
            .any(|detail| detail.workflow_id == workflow_id)
    }
}

/// One stored Transfer Family connector, keyed by `ConnectorId`.
///
/// A connector is a RECORD of the parameters of a connection to a partner's AS2 or
/// SFTP endpoint. The emulator speaks neither protocol, so nothing here is a live
/// connection; every member a reader reports is resolved once, when the connector is
/// created, so no reader has to re-apply a default. Every connector operation reads
/// this ONE record — the members a listing reports (`ListedConnector` is
/// `{Arn, ConnectorId, Url}`) and the ones only `DescribeConnector` reports live
/// together here rather than in a second map beside it.
#[derive(Debug, Clone)]
pub struct Connector {
    /// The minted `"c-" + 17 hex` identifier (`ConnectorId` is `^c-([0-9a-f]{17})$`).
    pub connector_id: String,
    /// `arn:aws:transfer:<region>:<account>:connector/<ConnectorId>`, as
    /// [`connector_arn`] builds it.
    pub arn: String,
    /// The partner endpoint, as supplied, when it names one.
    ///
    /// A blank `Url` names no endpoint (the member's `len 0..255` admits the empty
    /// string), which is the reading [`ConnectorConfig::cross_member_violation`] gives
    /// it too — so it is stored as absent, never as `""`. Absent exactly for a
    /// `VPC_LATTICE` connector, whose `Url` the model documents as "must be null":
    /// every other connector is refused without one.
    pub url: Option<String>,
    pub as2_config: Option<As2ConnectorConfig>,
    /// The SFTP settings, with every member the service itself settles already resolved
    /// at create time by [`resolve_sftp_connector_config`].
    pub sftp_config: Option<SftpConnectorConfig>,
    pub access_role: String,
    pub logging_role: Option<String>,
    /// The security policy governing the connector's SSH algorithms, resolved at create
    /// time to [`DEFAULT_CONNECTOR_SECURITY_POLICY_NAME`] for a connector that
    /// configures an SFTP connection and named none.
    ///
    /// `None` exactly for a connector that configures no such connection: the member's
    /// own modelled pattern admits only the `TransferSFTPConnectorSecurityPolicy-*`
    /// policies this service publishes for SFTP connectors, so an AS2 connector carries
    /// no policy of that kind to report.
    pub security_policy_name: Option<String>,
    /// `ServiceManagedEgressIpAddresses`: the egress addresses the service assigned this
    /// connector when it was created, and empty for a connector it assigned none — see
    /// [`service_managed_egress_ip_addresses`].
    pub service_managed_egress_ip_addresses: Vec<String>,
    /// The VPC Lattice egress settings, with `PortNumber` resolved at create time by
    /// [`resolve_connector_egress_config`]. `ConnectorEgressConfig` is a union with one
    /// member, so the variant is carried by [`Self::egress_type`] and this holds the
    /// settings themselves.
    pub egress_config: Option<ConnectorVpcLatticeEgressConfig>,
    pub egress_type: String,
    /// `IpAddressType`, resolved at create time: the value the caller named, else
    /// [`DEFAULT_IP_ADDRESS_TYPE`]. Every connector's network connections have an
    /// address type, so this is materialised for every stored connector rather than
    /// defaulted by each reader.
    pub ip_address_type: String,
    pub status: String,
    pub tags: Vec<Tag>,
}

/// The `EgressType` a connector's configuration selects: its own concern, in
/// [`connector_egress_type`], re-exported here beside the other connector rules the
/// write path reads.
pub use connector_egress_type::connector_egress_type;

/// The endpoint a connector's `Url` names, when it names one.
///
/// A blank value names no endpoint: `Url` is modelled `len 0..255`, so the empty
/// string passes the boundary validator, and a connector that may not carry a URL at
/// all must not report one either. Only the emptiness test ignores surrounding
/// whitespace; a value that names an endpoint is kept exactly as the caller sent it.
/// Both the cross-member check and the stored record read the member through here, so
/// the two cannot disagree.
pub fn connector_url(url: Option<&str>) -> Option<String> {
    url.filter(|url| !url.trim().is_empty())
        .map(str::to_string)
}

/// Resolve the `SftpConnectorConfig` a connector is stored with.
///
/// Every member the service itself settles is materialised HERE, once, at create time,
/// so no reader has to re-apply a default and a configuration read back reports the
/// connector as the service configured it: `MaxConcurrentConnections` at its documented
/// default, `TrustedHostKeys` as the (possibly empty) set of keys the connector trusts,
/// and `OrderedUserSecretVersionStages` at the order the credentials are read in.
///
/// `MaxConcurrentConnections` is modelled non-optional with `@default(1)`, so a request
/// that omits it arrives as the schema's default rather than as "absent", and its
/// `@range{min: 1}` is refused at the boundary before a handler runs: the floor below is
/// therefore belt-and-braces, for a value that reaches the store unset, and never a
/// validation rule of its own.
pub fn resolve_sftp_connector_config(requested: SftpConnectorConfig) -> SftpConnectorConfig {
    // Destructured rather than updated through `..requested`, so every member of the
    // shape is accounted for here: a member the model gains is a compile error until it
    // is resolved too.
    let SftpConnectorConfig {
        user_secret_id,
        trusted_host_keys,
        max_concurrent_connections,
        ordered_user_secret_version_stages,
    } = requested;
    SftpConnectorConfig {
        user_secret_id,
        max_concurrent_connections: if max_concurrent_connections
            >= DEFAULT_MAX_CONCURRENT_CONNECTIONS
        {
            max_concurrent_connections
        } else {
            DEFAULT_MAX_CONCURRENT_CONNECTIONS
        },
        // The host keys the connector trusts are a SET the connector carries, so a
        // request that named none leaves it EMPTY rather than unknown: `TrustedHostKeys`
        // is modelled `len 0..10`, and "optional for `CreateConnector`. If not provided,
        // you can use `TestConnection` to retrieve the server host key during the initial
        // connection attempt, and subsequently update the connector with the observed
        // host key" — the sequence a caller walks by reading the set back, pinning a key
        // and reading it again.
        trusted_host_keys: Some(trusted_host_keys.unwrap_or_default()),
        // The order the credentials are read in: the one the request named, else the one
        // stage a connector that named no order reads
        // ([`DEFAULT_SECRET_VERSION_STAGE`]). The member is modelled `len 1..2`, so an
        // empty order is refused at the boundary and the emptiness test below is
        // belt-and-braces for a value that reaches the store unset.
        ordered_user_secret_version_stages: Some(
            ordered_user_secret_version_stages
                .filter(|stages| !stages.is_empty())
                .unwrap_or_else(|| vec![DEFAULT_SECRET_VERSION_STAGE.to_string()]),
        ),
    }
}

/// Apply the `SftpConfig` an update NAMES onto the SFTP settings the connector already
/// has.
///
/// An update MODIFIES the members it names and leaves the others as they stand, which
/// is the rule the documented provisioning sequence requires: `TrustedHostKeys` is
/// "optional for `CreateConnector`. If not provided, you can use `TestConnection` to
/// retrieve the server host key during the initial connection attempt, and subsequently
/// update the connector with the observed host key" — so an update that names only that
/// key must leave the connector the credential it already had, otherwise the very
/// sequence the model documents leaves the connector unusable. The same rule
/// [`merge_connector_egress_config`] and [`merge_protocol_details`] follow for the other
/// nested configuration this service holds.
///
/// `MaxConcurrentConnections` is the one member the model gives a default (`@default(1)`),
/// so an omitted one arrives AS that default and cannot be told from a value the caller
/// chose: an update that names nothing above it therefore leaves the setting as it
/// stands. The create-time resolution runs over the result, so the stored settings stay
/// fully materialised.
pub fn merge_sftp_connector_config(
    stored: Option<&SftpConnectorConfig>,
    requested: SftpConnectorConfig,
) -> SftpConnectorConfig {
    let Some(stored) = stored else {
        return resolve_sftp_connector_config(requested);
    };
    resolve_sftp_connector_config(SftpConnectorConfig {
        user_secret_id: requested
            .user_secret_id
            .or_else(|| stored.user_secret_id.clone()),
        trusted_host_keys: requested
            .trusted_host_keys
            .or_else(|| stored.trusted_host_keys.clone()),
        ordered_user_secret_version_stages: requested
            .ordered_user_secret_version_stages
            .or_else(|| stored.ordered_user_secret_version_stages.clone()),
        max_concurrent_connections: if requested.max_concurrent_connections
            > DEFAULT_MAX_CONCURRENT_CONNECTIONS
        {
            requested.max_concurrent_connections
        } else {
            stored.max_concurrent_connections
        },
    })
}

/// Resolve the `As2ConnectorConfig` a connector is stored with.
///
/// `BasicAuthSecretId` is the one member of the structure that is either SET or NOT SET:
/// it "provides Basic authentication support to the AS2 Connectors API", its "default
/// value ... is `null`, which indicates that Basic authentication is not enabled for the
/// connector", and it is CLEARED by an empty value ("If you have previously enabled
/// Basic authentication for a connector, you can disable it by using the
/// `UpdateConnector` API call ... `--as2-config 'BasicAuthSecretId=\"\"'`"). Its target
/// `As2ConnectorSecretId` is modelled `len 0..2048`, so the empty string reaches a
/// handler, and it is stored as not configured — which a reader reports by omitting the
/// member — on the same rule [`configured_logging_role`] follows for the server member
/// that is cleared the same way.
pub fn resolve_as2_connector_config(requested: As2ConnectorConfig) -> As2ConnectorConfig {
    // Destructured rather than updated through `..requested`, so every member of the
    // shape is accounted for here: a member the model gains is a compile error until it
    // is resolved too.
    let As2ConnectorConfig {
        local_profile_id,
        partner_profile_id,
        message_subject,
        compression,
        encryption_algorithm,
        signing_algorithm,
        mdn_signing_algorithm,
        mdn_response,
        basic_auth_secret_id,
        preserve_content_type,
        async_mdn_config,
    } = requested;
    As2ConnectorConfig {
        local_profile_id,
        partner_profile_id,
        message_subject,
        compression,
        encryption_algorithm,
        signing_algorithm,
        mdn_signing_algorithm,
        mdn_response,
        basic_auth_secret_id: basic_auth_secret_id.filter(|secret_id| !secret_id.is_empty()),
        preserve_content_type,
        async_mdn_config,
    }
}

/// Apply the `As2Config` an update NAMES onto the AS2 settings the connector already
/// has.
///
/// The same rule [`merge_sftp_connector_config`], [`merge_connector_egress_config`] and
/// [`merge_protocol_details`] follow for the other nested configuration this service
/// holds: the members the request names are the members the update changes, and the rest
/// of the structure stays as it stands. The model states that rule for this structure on
/// `BasicAuthSecretId`, whose documented way to turn Basic authentication off is an
/// UPDATE CARRYING AN EMPTY VALUE for that one member — a member that a request omitting
/// it already cleared would need no such call — so an omitted member keeps its stored
/// value and only a named empty one clears it ([`resolve_as2_connector_config`]).
///
/// `AsyncMdnConfig` is a nested structure of its own, so the same rule applies inside
/// it: an update that names it changes the members it names there and leaves the rest.
pub fn merge_as2_connector_config(
    stored: Option<&As2ConnectorConfig>,
    requested: As2ConnectorConfig,
) -> As2ConnectorConfig {
    let Some(stored) = stored else {
        return resolve_as2_connector_config(requested);
    };
    let As2ConnectorConfig {
        local_profile_id,
        partner_profile_id,
        message_subject,
        compression,
        encryption_algorithm,
        signing_algorithm,
        mdn_signing_algorithm,
        mdn_response,
        basic_auth_secret_id,
        preserve_content_type,
        async_mdn_config,
    } = requested;
    resolve_as2_connector_config(As2ConnectorConfig {
        local_profile_id: local_profile_id.or_else(|| stored.local_profile_id.clone()),
        partner_profile_id: partner_profile_id.or_else(|| stored.partner_profile_id.clone()),
        message_subject: message_subject.or_else(|| stored.message_subject.clone()),
        compression: compression.or_else(|| stored.compression.clone()),
        encryption_algorithm: encryption_algorithm.or_else(|| stored.encryption_algorithm.clone()),
        signing_algorithm: signing_algorithm.or_else(|| stored.signing_algorithm.clone()),
        mdn_signing_algorithm: mdn_signing_algorithm
            .or_else(|| stored.mdn_signing_algorithm.clone()),
        mdn_response: mdn_response.or_else(|| stored.mdn_response.clone()),
        basic_auth_secret_id: basic_auth_secret_id
            .or_else(|| stored.basic_auth_secret_id.clone()),
        preserve_content_type: preserve_content_type
            .or_else(|| stored.preserve_content_type.clone()),
        async_mdn_config: match async_mdn_config {
            Some(requested) => Some(merge_as2_async_mdn_config(
                stored.async_mdn_config.as_ref(),
                requested,
            )),
            None => stored.async_mdn_config.clone(),
        },
    })
}

/// Apply the `AsyncMdnConfig` an update names onto the asynchronous-MDN settings the
/// connector already has, on the same rule as [`merge_as2_connector_config`].
fn merge_as2_async_mdn_config(
    stored: Option<&As2AsyncMdnConnectorConfig>,
    requested: As2AsyncMdnConnectorConfig,
) -> As2AsyncMdnConnectorConfig {
    let Some(stored) = stored else {
        return requested;
    };
    let As2AsyncMdnConnectorConfig { url, server_ids } = requested;
    As2AsyncMdnConnectorConfig {
        url: url.or_else(|| stored.url.clone()),
        server_ids: server_ids.or_else(|| stored.server_ids.clone()),
    }
}

// -----------------------------------------------------------------------------
// Testing an SFTP connector's connection
// -----------------------------------------------------------------------------
//
// `TestConnection`'s whole answer is a verdict on ONE connector: `Status` is "OK for
// successful test, or ERROR if the test fails", `StatusMessage` is "Connection
// succeeded ... Or, ... a descriptive error message if the test fails", and
// `SftpConnectionDetails.HostKey` is "the SSH public key of the remote SFTP server ...
// returned during the initial connection attempt". The three texts below each state
// one defect the STORED configuration carries, decided from that configuration alone,
// before any connection is attempted; what the attempt itself answers is decided in
// [`sftp_target`]. Each is a FIXED text per condition carrying no request data, for the
// reason this module's docs give.
//
// Each one is a VERDICT: one line saying what stopped the connection, as the recorded
// answer for a connector that trusts no host key states it. The member's documentation
// then lists what to check for each verdict a caller receives, which is guidance to the
// reader of that documentation and no part of the answer, so none of it is appended to a
// message here.

/// `TestConnection`'s `Status` for a test that succeeded.
///
/// `Status` is modelled as a plain string, so the two values the operation documents
/// ("Returns OK for successful test, or ERROR if the test fails") live here as
/// constants. Reported for a connection to an SFTP endpoint this emulator SERVES that
/// was established with the connector's own stored credentials and trusted host key —
/// see [`sftp_target`], which is where a test's attempt is made.
pub const TEST_CONNECTION_STATUS_OK: &str = "OK";

/// `TestConnection`'s `Status` for a test that did not succeed.
///
/// The operation's own documented failure state: the stored configuration carries a
/// defect ([`sftp_configuration_defect`]), or the attempt on the endpoint the `Url`
/// names did not establish a session ([`sftp_target`]). The `StatusMessage` that
/// travels with it says which.
pub const TEST_CONNECTION_STATUS_ERROR: &str = "ERROR";

/// `StatusMessage` for a test that succeeded: the model's own words for that outcome
/// ("Returns Connection succeeded if the test is successful"), and the whole of them.
pub const TEST_CONNECTION_SUCCESS_MESSAGE: &str = "Connection succeeded";

/// `StatusMessage` for an SFTP configuration that names no secret to authenticate
/// with. `UserSecretId` is "the identifier for the secret ... that contains the SFTP
/// user's private key, password, or both", so without it the connector has no
/// credentials to offer — and the verdict names the member the configuration provides
/// none of, in the form the recorded [`SFTP_NO_TRUSTED_HOST_KEY`] verdict states it.
const SFTP_NO_USER_SECRET: &str = "No UserSecretId provided.";

/// `StatusMessage` for an SFTP configuration that trusts no host key.
///
/// `TrustedHostKeys` is "the public portion of the host key, or keys, that are used to
/// identify the external server to which you are connecting", and is "optional for
/// `CreateConnector` [because] you can use `TestConnection` to retrieve the server host
/// key during the initial connection attempt" — so a connector that trusts none has
/// nothing to identify the remote server by, and the test that says so is the very call
/// that reports the host key to pin.
///
/// The service's recorded verdict for this condition, word for word, and the form the
/// verdicts beside it state their own condition in.
const SFTP_NO_TRUSTED_HOST_KEY: &str = "No TrustedHostKeys provided.";

/// `StatusMessage` for an SFTP configuration whose trusted host key is not a key.
///
/// `TrustedHostKeys`' documented form is "`key-type key-body`", the "`<key type>` and
/// `<body base64>`" elements of an SSH public key; a value [`ssh_key`] cannot read as
/// one is not a host key at all, so it can never match what the remote server presents
/// — whatever that server turns out to be.
const SFTP_UNREADABLE_TRUSTED_HOST_KEY: &str = "A TrustedHostKeys entry is not an SSH \
     public key in 'key-type key-body' form.";

/// Whether `material` is an SSH PUBLIC key this service can read.
///
/// Asks [`ssh_key`] — the crate that implements the OpenSSH key formats — rather than
/// matching on the text, so what counts as a key is the format itself: the
/// `key-type key-body [comment]` form `ssh-keygen` writes, and not, say, the
/// `host key-type key-body` line `ssh-keyscan` prints, whose host-name element the
/// model documents as one a caller must NOT paste into `TrustedHostKeys`.
///
/// Crate-private: the rule a handler needs is the whole verdict
/// ([`sftp_target::probe`]), not this one predicate.
pub(crate) fn is_readable_ssh_public_key(material: &str) -> bool {
    ::ssh_key::PublicKey::from_openssh(material).is_ok()
}

/// The `StatusMessage` for the FIRST defect the SFTP configuration `config` carries
/// that makes an SFTP session impossible — `None` when it carries none, which is when
/// the connection itself decides the verdict ([`sftp_target::probe`]).
///
/// Every branch is decided from the stored configuration alone — no request data
/// reaches the text, and nothing here claims an outcome for a session nothing opened.
pub fn sftp_configuration_defect(config: &SftpConnectorConfig) -> Option<&'static str> {
    let names_secret = config
        .user_secret_id
        .as_deref()
        .is_some_and(|secret| !secret.trim().is_empty());
    if !names_secret {
        return Some(SFTP_NO_USER_SECRET);
    }

    let trusted_host_keys = config.trusted_host_keys.as_deref().unwrap_or_default();
    if trusted_host_keys.is_empty() {
        return Some(SFTP_NO_TRUSTED_HOST_KEY);
    }
    if trusted_host_keys
        .iter()
        .any(|key| !is_readable_ssh_public_key(key))
    {
        return Some(SFTP_UNREADABLE_TRUSTED_HOST_KEY);
    }

    None
}

/// `TestConnection`'s refusal for a connector that configures no SFTP connection.
///
/// The operation "tests whether your SFTP connector is set up successfully", and its
/// whole output — the `Status`, the `StatusMessage`, the remote server's host key — is
/// about an SFTP session. An AS2 connector configures none (`CreateConnector` takes
/// "exactly one configuration object: either for AS2 (`As2Config`) or SFTP
/// (`SftpConfig`)"), so there is no connection for this call to test: a malformed
/// request, on the same reading [`not_configured_for_external_auth`] gives
/// `TestIdentityProvider` on a server that delegates authentication to nothing.
pub fn connector_configures_no_sftp_connection() -> InvalidRequestException {
    InvalidRequestException {
        message: required_when("SftpConfig", "a connector's connection is tested"),
    }
}

/// Resolve the egress settings a `VPC_LATTICE` connector is stored with: the Resource
/// Configuration as supplied, and `PortNumber` at its documented default when the
/// request named none.
pub fn resolve_connector_egress_config(
    requested: ConnectorEgressConfig,
) -> ConnectorVpcLatticeEgressConfig {
    match requested {
        ConnectorEgressConfig::VpcLattice(config) => ConnectorVpcLatticeEgressConfig {
            port_number: Some(config.port_number.unwrap_or(DEFAULT_VPC_LATTICE_PORT)),
            ..config
        },
    }
}

/// Apply the `UpdateConnectorEgressConfig` an update NAMES onto the egress settings the
/// connector already has — `Err(message)` when the connector has none.
///
/// `UpdateConnectorVpcLatticeEgressConfig` makes EVERY member optional where
/// `ConnectorVpcLatticeEgressConfig` requires the Resource Configuration ARN, so an
/// update MODIFIES an existing egress configuration member by member ("Updates the
/// egress configuration for the connector, allowing you to modify how traffic is
/// routed") — the same rule [`merge_protocol_details`] follows for a server, and the
/// create-time resolution runs over the result so the stored settings stay fully
/// materialised.
///
/// A connector with no egress configuration is a `SERVICE_MANAGED` one, and nothing in
/// the model names `EgressType` as an updatable member, so such a request has no egress
/// configuration to modify: it is refused with the condition the member is settable
/// under, the same reading [`ConnectorConfig::cross_member_violation`] gives
/// `EgressConfig` at create time.
pub fn merge_connector_egress_config(
    stored: Option<&ConnectorVpcLatticeEgressConfig>,
    requested: UpdateConnectorEgressConfig,
) -> Result<ConnectorVpcLatticeEgressConfig, String> {
    let UpdateConnectorEgressConfig::VpcLattice(requested) = requested;
    let Some(stored) = stored else {
        return Err(settable_only_when(
            "EgressConfig",
            &format!("EgressType is {CONNECTOR_EGRESS_TYPE_VPC_LATTICE}"),
        ));
    };
    Ok(resolve_connector_egress_config(
        ConnectorEgressConfig::VpcLattice(ConnectorVpcLatticeEgressConfig {
            resource_configuration_arn: requested
                .resource_configuration_arn
                .unwrap_or_else(|| stored.resource_configuration_arn.clone()),
            port_number: requested.port_number.or(stored.port_number),
        }),
    ))
}

// -----------------------------------------------------------------------------
// Connector operations: the transfers, moves and deletes a connector is asked to
// perform against the partner's REMOTE endpoint
// -----------------------------------------------------------------------------
//
// `StartFileTransfer` and `StartRemoteMove` are ASYNCHRONOUS: each one accepts the
// request and answers an identifier for it, and the work itself then happens against
// the remote AS2 or SFTP endpoint the connector's `Url` names. So each accepted request
// is RECORDED here, under the identifier it was answered with, and
// `ListFileTransferResults` reports the per-file status of the record.
//
// An endpoint this emulator SERVES is one it can carry the work out against:
// [`connector_delivery`] performs such a transfer and records what the attempt did to
// each file, which is what the record then reports once the transfer's span has
// elapsed. An endpoint outside the emulator is one nothing can connect to, so no
// attempt is made and its files stay `QUEUED` — a status that claims nothing about
// bytes nobody moved.

/// `TransferTableStatus`' value for a file whose transfer has been accepted and has
/// not run (the enum is `[QUEUED, IN_PROGRESS, COMPLETED, FAILED]`).
pub const TRANSFER_STATUS_QUEUED: &str = "QUEUED";

/// `TransferTableStatus`' value for a file the attempt transferred.
///
/// Reported only for a file whose bytes really were written to the destination the
/// request named ([`connector_delivery`]), never for one nothing attempted.
pub const TRANSFER_STATUS_COMPLETED: &str = "COMPLETED";

/// `TransferTableStatus`' value for a file the attempt could not transfer.
///
/// Reported only for an attempt that was MADE and failed, with the `FailureCode` and
/// `FailureMessage` the storage the attempt addressed answered — never a code this
/// service invented for work it did not try.
pub const TRANSFER_STATUS_FAILED: &str = "FAILED";

/// How long an accepted transfer takes before its per-file outcome is reported.
///
/// A transfer is asynchronous: "begins a file transfer" is what the call does, and the
/// result of each file is read back afterwards, so a caller reading results straight
/// after the call sees the accepted-and-not-yet-reported state (`QUEUED`) rather than a
/// terminal one. Nothing SLEEPS for this: the record carries the instant its outcome
/// becomes due and [`ConnectorTransfer::results`] compares the clock against it.
pub const CONNECTOR_TRANSFER_SPAN: Duration = Duration::from_secs(3);

/// `ResourceType` for one accepted connector operation, addressed by its
/// `TransferId`. Same convention as [`RESOURCE_TYPE_SERVER`].
pub const RESOURCE_TYPE_TRANSFER: &str = "Transfer";

/// A `TransferId` the resolved connector has no accepted operation under. Its own
/// entry in the not-found table, like every other kind.
pub const NOT_FOUND_TRANSFER: NotFoundResource = NotFoundResource {
    resource_type: RESOURCE_TYPE_TRANSFER,
    message: "Unknown transfer",
};

/// The not-found payload for a `TransferId` the named connector has no record of.
pub fn unknown_transfer(transfer_id: &str) -> ResourceNotFoundException {
    resource_not_found(&NOT_FOUND_TRANSFER, transfer_id)
}

/// How many accepted operations ONE connector holds before a `Start*` call refuses
/// with the modelled `ServiceUnavailableException` instead of growing.
///
/// The emulator's OWN bound on the store, chosen on the same basis as
/// [`MAX_STORED_SERVERS`] — not a claim about any account's quota. Each record is
/// itself bounded by the modelled lengths of what a caller may send (`SendFilePaths`
/// and `RetrieveFilePaths` `len 1..10` of `FilePath` `len 1..1024`), which the
/// framework's boundary validator enforces before a handler runs, so this bounds a
/// connector's records in bytes as well as in count. `DeleteConnector` releases all of
/// a connector's.
pub const MAX_TRANSFERS_PER_CONNECTOR: usize = 100;

/// What the attempt on ONE file of a transfer did to it.
///
/// Recorded by [`connector_delivery`] after the attempt, so a status this carries is
/// always a statement about work that ran: `FailureCode`/`FailureMessage` are "for
/// transfers that fail" and carry what the storage the attempt addressed answered.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConnectorFileOutcome {
    /// The `TransferTableStatus` the attempt reached: [`TRANSFER_STATUS_COMPLETED`] or
    /// [`TRANSFER_STATUS_FAILED`].
    pub status_code: String,
    /// The error code the failed attempt was answered with, as the service that
    /// answered it named the error.
    pub failure_code: Option<String>,
    /// That answer's own message.
    pub failure_message: Option<String>,
}

impl ConnectorFileOutcome {
    /// The outcome of a file the attempt transferred.
    pub fn completed() -> Self {
        Self {
            status_code: TRANSFER_STATUS_COMPLETED.to_string(),
            failure_code: None,
            failure_message: None,
        }
    }

    /// The outcome of a file the attempt could not transfer, with the error the
    /// storage it addressed answered.
    pub fn failed(code: &str, message: &str) -> Self {
        Self {
            status_code: TRANSFER_STATUS_FAILED.to_string(),
            failure_code: Some(code.to_string()),
            failure_message: Some(message.to_string()),
        }
    }
}

/// One accepted connector operation, keyed by `(ConnectorId, TransferId)`.
///
/// The paths are held in the caller's OWN order — the order the request named the files
/// in is the order their results are reported in, which is the only order the request
/// states — so this is a `Vec`, not a set, and the outcomes are a `Vec` positioned
/// against it.
#[derive(Debug, Clone)]
pub struct ConnectorTransfer {
    /// The minted identifier this operation was answered with (`TransferId`/`MoveId`
    /// are both `len 1..512`, pattern `^[0-9a-zA-Z./-]+$`).
    pub transfer_id: String,
    /// The files the request named, in the order it named them.
    pub file_paths: Vec<String>,
    /// When the request was accepted.
    pub requested_at: SystemTime,
    /// What the attempt did to each named file, by that file's position in
    /// [`Self::file_paths`] — `None` for a file no attempt has reported on.
    pub outcomes: Vec<Option<ConnectorFileOutcome>>,
    /// When this transfer's reported outcome comes due: accepted + the span the
    /// transfer takes ([`CONNECTOR_TRANSFER_SPAN`]).
    pub reported_at: Instant,
}

impl ConnectorTransfer {
    /// The per-file results of this operation, in submission order: one result per file
    /// the request named.
    ///
    /// A file is `QUEUED` until this transfer's span has elapsed and the attempt has
    /// reported on it; after that it carries the outcome the attempt recorded.
    pub fn results(&self, now: Instant) -> Vec<ConnectorFileTransferResult> {
        self.file_paths
            .iter()
            .enumerate()
            .map(|(index, file_path)| {
                let reported = (now >= self.reported_at)
                    .then(|| self.outcomes.get(index).cloned().flatten())
                    .flatten();
                ConnectorFileTransferResult {
                    file_path: file_path.clone(),
                    status_code: reported.as_ref().map_or_else(
                        || TRANSFER_STATUS_QUEUED.to_string(),
                        |outcome| outcome.status_code.clone(),
                    ),
                    failure_code: reported.as_ref().and_then(|o| o.failure_code.clone()),
                    failure_message: reported.and_then(|o| o.failure_message),
                }
            })
            .collect()
    }
}

/// The refusal for a `Start*` call that names no file for the connector to act on.
///
/// `StartFileTransfer` takes the files from whichever direction the request names
/// ("you specify one or more `RetrieveFilePaths` to identify the files you want to
/// transfer" inbound, "one or more `SendFilePaths`" outbound), so a request that names
/// neither identifies nothing to transfer.
///
/// The service's own FIXED text for that condition, in its entirety: it reports that
/// the list of files to transfer is empty and names no member, so the same sentence
/// answers a request that named no list and one that named an empty one.
pub const TRANSFER_NAMES_NO_FILE: &str = "Empty send file list";

/// [`TRANSFER_NAMES_NO_FILE`] on the modelled `InvalidRequestException` channel.
pub fn transfer_names_no_file() -> InvalidRequestException {
    InvalidRequestException {
        message: TRANSFER_NAMES_NO_FILE.to_string(),
    }
}

/// The refusal for an INBOUND transfer asked of an AS2 connector.
///
/// The operation documents the directions per connector kind: "For an AS2 connector,
/// you specify the `ConnectorId` and one or more `SendFilePaths`" — outbound only —
/// while "for an SFTP connector, the file transfer can be either outbound or inbound".
/// So retrieving files is something only an SFTP connector can be asked to do.
pub fn retrieval_requires_sftp_connector() -> InvalidRequestException {
    InvalidRequestException {
        message: settable_only_when("RetrieveFilePaths", "the connector configures SftpConfig"),
    }
}

/// The refusal for a remote path that is not absolute.
///
/// `StartRemoteMove` "moves or renames a file or directory on the remote SFTP server",
/// and a path on that server is resolved from its root: a relative path names no file
/// there, so it is refused as the malformed request it is rather than acted on against
/// some assumed directory. `member` is the modelled member name (`SourcePath` or
/// `TargetPath`), and nothing the caller sent reaches the text.
pub fn remote_path_must_be_absolute(member: &str) -> InvalidRequestException {
    InvalidRequestException {
        message: format!("{member} must be an absolute path on the remote server"),
    }
}

/// Whether `path` is an absolute path on the remote server, i.e. is resolved from its
/// root rather than from an assumed working directory.
pub fn is_absolute_remote_path(path: &str) -> bool {
    path.starts_with('/')
}

impl ComAmazonawsTransferTransferserviceState {
    /// The operations accepted for `connector_id`, in `TransferId` order.
    ///
    /// A RANGE scan over that connector's contiguous run of the map, on the same rule
    /// as [`Self::users_of`].
    pub fn transfers_of<'a>(
        &'a self,
        connector_id: &'a str,
    ) -> impl Iterator<Item = &'a ConnectorTransfer> + 'a {
        use ::std::ops::Bound;
        let lower = (connector_id.to_string(), String::new());
        self.connector_transfers
            .range((Bound::Included(lower), Bound::Unbounded))
            .take_while(move |((cid, _), _)| cid == connector_id)
            .map(|(_, transfer)| transfer)
    }

    /// Whether `connector_id` can take another accepted operation, i.e. is below
    /// [`MAX_TRANSFERS_PER_CONNECTOR`].
    pub fn connector_has_transfer_capacity(&self, connector_id: &str) -> bool {
        self.transfers_of(connector_id).count() < MAX_TRANSFERS_PER_CONNECTOR
    }

    /// Record one accepted operation against `connector_id` and answer the identifier
    /// it is held under — the `TransferId`/`MoveId` the call answers with.
    ///
    /// Minted and stored under the SAME lock, so the answer never names an operation
    /// the store does not hold.
    pub fn accept_connector_transfer(
        &mut self,
        connector_id: &str,
        file_paths: Vec<String>,
        now: Instant,
    ) -> String {
        let transfer_id = self.mint_transfer_id();
        let transfer = ConnectorTransfer {
            outcomes: vec![None; file_paths.len()],
            file_paths,
            requested_at: SystemTime::now(),
            reported_at: now + CONNECTOR_TRANSFER_SPAN,
            transfer_id: transfer_id.clone(),
        };
        self.connector_transfers
            .insert((connector_id.to_string(), transfer_id.clone()), transfer);
        transfer_id
    }

    /// Record what the attempt did to the file at `index` of one accepted transfer.
    ///
    /// Written by [`connector_delivery`] once the attempt on that file has finished;
    /// the record reports it when its span has elapsed
    /// ([`ConnectorTransfer::results`]). A transfer the store no longer holds — its
    /// connector was deleted while the attempt ran — takes no outcome, which is why
    /// this resolves the record rather than holding a handle to it.
    pub fn record_transfer_outcome(
        &mut self,
        connector_id: &str,
        transfer_id: &str,
        index: usize,
        outcome: ConnectorFileOutcome,
    ) {
        if let Some(slot) = self
            .connector_transfers
            .get_mut(&(connector_id.to_string(), transfer_id.to_string()))
            .and_then(|transfer| transfer.outcomes.get_mut(index))
        {
            *slot = Some(outcome);
        }
    }
}

/// What a state-changing server call does, given the server's current `State`.
///
/// `StartServer` and `StopServer` share ONE rule, so the two handlers cannot drift:
/// the call begins its transition from the state it is documented to move the server
/// out of, and from its own failure state, which is what a caller retries; it has "no
/// impact" on a server that is already in, or already on its way to, the state it
/// targets; and it is refused when the server is in flight the other way, because
/// there is then no such transition to make.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ServerTransition {
    /// Write the intermediate state now and owe the terminal one to the clock.
    Begin,
    /// Nothing to do: answer success and leave the server, and any pending
    /// transition, exactly as they are.
    NoImpact,
    /// Refuse with the modelled `InvalidRequestException`.
    Refuse,
}

impl ServerTransition {
    /// Classify `state` for a call that moves a server out of `from`, towards `target`
    /// through the intermediate `in_flight`, and that is retried from `failed` — its
    /// own `*_FAILED` state, which the model documents as "an error condition" of this
    /// very transition.
    fn decide(state: &str, from: &str, target: &str, in_flight: &str, failed: &str) -> Self {
        if state == from || state == failed {
            Self::Begin
        } else if state == target || state == in_flight {
            Self::NoImpact
        } else {
            Self::Refuse
        }
    }
}

/// Resolve the `ProtocolDetails` a server is created with.
///
/// Every member the model documents a default for is materialised HERE, once, at create
/// time, so a reader rebuilds the server's real protocol settings instead of reporting
/// nothing configured. A member the caller supplied is kept exactly as given; only the
/// absent ones take their default. `As2Transports` has no documented default (it is
/// meaningful only for AS2 servers), so an absent one stays absent.
pub fn resolve_protocol_details(requested: Option<ProtocolDetails>) -> ProtocolDetails {
    let requested = requested.unwrap_or_default();
    ProtocolDetails {
        passive_ip: Some(
            requested
                .passive_ip
                .unwrap_or_else(|| DEFAULT_PASSIVE_IP.to_string()),
        ),
        tls_session_resumption_mode: Some(
            requested
                .tls_session_resumption_mode
                .unwrap_or_else(|| DEFAULT_TLS_SESSION_RESUMPTION_MODE.to_string()),
        ),
        set_stat_option: Some(
            requested
                .set_stat_option
                .unwrap_or_else(|| DEFAULT_SET_STAT_OPTION.to_string()),
        ),
        sftp_ports: Some(
            requested
                .sftp_ports
                .filter(|ports| !ports.is_empty())
                .map(|ports| {
                    ports
                        .into_iter()
                        .map(|port| SftpPortWithOptions {
                            sftp_port: port.sftp_port,
                            // `CommunicationMode` is optional per entry and defaults
                            // the same way the whole-list default does.
                            communication_mode: Some(port.communication_mode.unwrap_or_else(
                                || DEFAULT_COMMUNICATION_MODE.to_string(),
                            )),
                        })
                        .collect()
                })
                .unwrap_or_else(|| {
                    vec![SftpPortWithOptions {
                        sftp_port: DEFAULT_SFTP_PORT,
                        communication_mode: Some(DEFAULT_COMMUNICATION_MODE.to_string()),
                    }]
                }),
        ),
        as2_transports: requested.as2_transports,
        proxy_config: Some(ProxyConfig {
            sftp_mode: Some(
                requested
                    .proxy_config
                    .and_then(|proxy| proxy.sftp_mode)
                    .unwrap_or_else(|| DEFAULT_PROXY_SFTP_MODE.to_string()),
            ),
        }),
    }
}

/// The STORED form of the structured-logging destination a request supplied.
///
/// `StructuredLogDestinations` is a configuration setting that is either SET or NOT
/// SET: the model documents an EMPTY value as what turns it off ("If you have
/// previously specified a log group for a server, you can clear it, and in effect turn
/// off structured logging, by providing an empty value for this parameter"), so an
/// empty list is stored as `None` — not configured — rather than as a configured empty
/// list, and a reader omits the member for it.
pub fn configured_log_destinations(requested: Option<Vec<String>>) -> Option<Vec<String>> {
    requested.filter(|destinations| !destinations.is_empty())
}

/// The STORED form of the logging role a request supplied, on the same rule as
/// [`configured_log_destinations`].
///
/// `LoggingRole` targets `NullableRole`, whose pattern `^(|arn:.*role/\S+)$` admits the
/// EMPTY STRING for exactly one purpose — clearing the role — so an empty value is
/// stored as not configured and reported absent, like any other logging member the
/// server does not have.
pub fn configured_logging_role(requested: Option<String>) -> Option<String> {
    requested.filter(|role| !role.is_empty())
}

/// Resolve the `S3StorageOptions` a server is created with, on the same rule as
/// [`resolve_protocol_details`].
pub fn resolve_s3_storage_options(requested: Option<S3StorageOptions>) -> S3StorageOptions {
    S3StorageOptions {
        directory_listing_optimization: Some(
            requested
                .and_then(|options| options.directory_listing_optimization)
                .unwrap_or_else(|| DEFAULT_DIRECTORY_LISTING_OPTIMIZATION.to_string()),
        ),
    }
}

/// Apply the `ProtocolDetails` an update NAMES onto the ones the server already has.
///
/// An update changes the members it names and leaves the rest of the server's protocol
/// settings where they were — then the same create-time resolution runs over the
/// result, so the stored settings stay fully materialised and no member can fall back
/// to "unset".
pub fn merge_protocol_details(
    stored: &ProtocolDetails,
    requested: ProtocolDetails,
) -> ProtocolDetails {
    resolve_protocol_details(Some(ProtocolDetails {
        passive_ip: requested.passive_ip.or_else(|| stored.passive_ip.clone()),
        tls_session_resumption_mode: requested
            .tls_session_resumption_mode
            .or_else(|| stored.tls_session_resumption_mode.clone()),
        set_stat_option: requested
            .set_stat_option
            .or_else(|| stored.set_stat_option.clone()),
        sftp_ports: requested.sftp_ports.or_else(|| stored.sftp_ports.clone()),
        as2_transports: requested
            .as2_transports
            .or_else(|| stored.as2_transports.clone()),
        proxy_config: requested
            .proxy_config
            .or_else(|| stored.proxy_config.clone()),
    }))
}

/// Apply the `S3StorageOptions` an update names onto the stored ones, on the same rule
/// as [`merge_protocol_details`].
pub fn merge_s3_storage_options(
    stored: &S3StorageOptions,
    requested: S3StorageOptions,
) -> S3StorageOptions {
    resolve_s3_storage_options(Some(S3StorageOptions {
        directory_listing_optimization: requested
            .directory_listing_optimization
            .or_else(|| stored.directory_listing_optimization.clone()),
    }))
}

// =============================================================================
// Cross-member requirements
//
// Some of this service's members are only meaningful in combination: the model's own
// documentation states, for a member, which value of ANOTHER member makes it
// required, or makes it settable at all. The framework's boundary validator enforces
// the single-member constraints (`@required`, `@length`, `@pattern`, enum value sets)
// before a handler runs, but it cannot see across members — so these requirements are
// checked here, by the create/update handler, BEFORE anything is persisted, and
// refused with the modelled `InvalidRequestException` (HTTP 400), which the model
// documents as "thrown when the client submits a malformed request".
//
// The wording below names the member and, where the violated requirement is one the
// MODEL states about that member alone (a list's non-empty minimum, say), that
// requirement; otherwise the condition the member was submitted under. Both are built
// from the model's own names and its own closed value sets — never from a value the
// caller chose.
// =============================================================================

/// The fixed `Message` the service answers a server configuration that hosts its
/// endpoint in a VPC and names no `EndpointDetails` with.
///
/// `EndpointDetails` is "the virtual private cloud (VPC) endpoint settings that are
/// configured for your server", whose `SubnetIds` are "required to host your server
/// endpoint in your VPC", so the settings are what VPC hosting needs. The text is the
/// service's own, FIXED for the condition in its entirety: it states the requirement
/// VPC hosting carries and has no slot for the member, the `EndpointType` value or
/// anything else the caller sent, so every endpoint type that hosts the endpoint in a
/// VPC is refused with the same sentence.
pub const ENDPOINT_DETAILS_REQUIRED_FOR_VPC: &str = "EndpointDetails required for VPC";

/// "`<member>` must not be empty" — a collection member the model gives a non-empty
/// minimum (`@length` min 1), required by a documented condition and sent empty or
/// absent. The requirement stated is the LIST's own minimum, which is the modelled
/// violation, rather than the other member whose value made the list required.
fn must_not_be_empty(member: &str) -> String {
    format!("{member} must not be empty")
}

/// "`<member>` must be specified when `<condition>`" — a member the model documents as
/// required under a condition the request meets, and did not send.
fn required_when(member: &str, condition: &str) -> String {
    format!("{member} must be specified when {condition}")
}

/// "`<member>` required for `<deciding member>`: `<value>`" — a member the model
/// documents as required for particular VALUES of another member, and did not send.
///
/// This service states that class of requirement as what the missing member is needed
/// FOR, naming the deciding member and the value the request chose — the same
/// `<member> required for <condition>` form [`ENDPOINT_DETAILS_REQUIRED_FOR_VPC`] takes —
/// rather than as the condition the request was submitted under ([`required_when`], which
/// words a requirement a member's own value or a list's membership carries).
fn required_for_value(member: &str, deciding_member: &str, value: &str) -> String {
    format!("{member} required for {deciding_member}: {value}")
}

/// "`<member>` must be specified" — a member the model documents as part of what the
/// request names, with no condition attached, and did not send.
fn required_member(member: &str) -> String {
    format!("{member} must be specified")
}

/// "`<member>` cannot be specified when `<condition>`" — a member the model documents
/// as unusable under a condition the request meets, and sent anyway.
fn forbidden_when(member: &str, condition: &str) -> String {
    format!("{member} cannot be specified when {condition}")
}

/// "`<member>` can only be set when `<condition>`" — a member the model documents as
/// settable under ONE condition, sent under another.
fn settable_only_when(member: &str, condition: &str) -> String {
    format!("{member} can only be set when {condition}")
}

/// "`<member>` must be `<requirement>` when `<condition>`" — a member whose VALUE the
/// model constrains to another member's value.
fn must_be_when(member: &str, requirement: &str, condition: &str) -> String {
    format!("{member} must be {requirement} when {condition}")
}

/// "FTP and FTPS protocols unsupported for IdentityProviderType `<type>`" — the
/// identity-provider half of the requirement `Protocols` documents, stated as the
/// incompatibility it is: the authentication mode the server was asked for cannot
/// serve those protocols at all.
fn unsupported_for_identity_provider(identity_provider_type: &str) -> String {
    format!(
        "{PROTOCOL_FTP} and {PROTOCOL_FTPS} protocols unsupported for \
         IdentityProviderType {identity_provider_type}"
    )
}

/// "One of `<first>` or `<second>` must be specified" — two members the model documents
/// as alternatives, exactly one of which configures the resource, and the request named
/// NEITHER.
///
/// The text states the requirement the request actually violated — that one of the two
/// is needed — rather than the whole either/or rule: the condition being reported is
/// that nothing configures the resource, not that both were named.
fn one_of_required(first: &str, second: &str) -> String {
    format!("One of {first} or {second} must be specified")
}

/// "Only one of `<first>` or `<second>` must be specified" — the OTHER half of the same
/// either/or: two alternative members of which the request named both, so which
/// configuration to build is ambiguous. A different condition from [`one_of_required`]'s,
/// so a different text — and the two halves are deliberately PARALLEL sentences over the
/// same two member names, this one adding the "Only" that says which half was broken.
fn not_both_of(first: &str, second: &str) -> String {
    format!("Only one of {first} or {second} must be specified")
}

/// The violation an either/or member pair reports for the way it was violated, or
/// `None` when exactly one of the two is named — the ONE reading of a documented
/// "specify either A or B, but not both" requirement, so every pair in this service
/// words both halves the same way.
fn either_or_violation(
    first: (&str, bool),
    second: (&str, bool),
) -> Option<String> {
    let ((first_member, first_named), (second_member, second_named)) = (first, second);
    match (first_named, second_named) {
        (false, false) => Some(one_of_required(first_member, second_member)),
        (true, true) => Some(not_both_of(first_member, second_member)),
        _ => None,
    }
}

/// "`<member>` must be at most `<maximum>`" — a member whose documented upper bound the
/// model's own constraints do not carry.
fn must_be_at_most(member: &str, maximum: i32) -> String {
    format!("{member} must be at most {maximum}")
}

/// The server configuration a `CreateServer` (or `UpdateServer`) request resolves to:
/// every member a documented cross-member requirement mentions, with the model's
/// documented defaults ALREADY applied.
///
/// Defaults are applied first because the requirements are about the configuration the
/// server will actually have: a request that omits `EndpointType` selects `PUBLIC`,
/// which needs no VPC endpoint settings at all.
#[derive(Debug)]
pub struct ServerConfig<'a> {
    pub domain: &'a str,
    pub endpoint_type: &'a str,
    pub endpoint_details: Option<&'a EndpointDetails>,
    pub identity_provider_type: &'a str,
    pub identity_provider_details: Option<&'a IdentityProviderDetails>,
    pub ip_address_type: &'a str,
    pub protocols: &'a [String],
    pub certificate: Option<&'a str>,
}

impl ServerConfig<'_> {
    /// Whether this configuration's `IdentityProviderType` is one of the three the
    /// model documents `IdentityProviderDetails` as required for.
    ///
    /// Those three are exactly the types that delegate authentication to something
    /// outside the service, so the enum membership is listed once, in
    /// [`is_external_identity_provider`], and read from here.
    fn needs_identity_provider_details(&self) -> bool {
        is_external_identity_provider(self.identity_provider_type)
    }

    /// Whether `Protocols` names `protocol`.
    fn speaks(&self, protocol: &str) -> bool {
        self.protocols.iter().any(|named| named == protocol)
    }

    /// The first documented cross-member requirement this configuration violates,
    /// worded as the refusal states it — `None` when it violates none.
    ///
    /// Every rule below is a sentence in this service's own model:
    ///
    /// * `EndpointDetails` is "the virtual private cloud (VPC) endpoint settings that
    ///   are configured for your server", whose `SubnetIds` are "required to host your
    ///   server endpoint in your VPC" — so a server whose `EndpointType` hosts the
    ///   endpoint in a VPC cannot be configured without them.
    /// * each `EndpointDetails` member states the one `EndpointType` it "can only be
    ///   set when" (`VpcEndpointId` -> `VPC_ENDPOINT`, the rest -> `VPC`), and
    ///   `AddressAllocationIds` adds "You can't set address allocation IDs for servers
    ///   that have an `IpAddressType` set to `DUALSTACK`".
    /// * `IdentityProviderDetails` is "Required when `IdentityProviderType` is set to
    ///   `AWS_DIRECTORY_SERVICE`, `AWS_LAMBDA` or `API_GATEWAY`" and "Cannot be
    ///   specified when `IdentityProviderType` is set to `SERVICE_MANAGED`".
    /// * `Certificate` is "Required when `Protocols` is set to `FTPS`".
    /// * `Protocols` states "If `Protocol` includes either `FTP` or `FTPS`, then the
    ///   `EndpointType` must be `VPC` and the `IdentityProviderType` must be either
    ///   `AWS_DIRECTORY_SERVICE`, `AWS_LAMBDA`, or `API_GATEWAY`" and "If `Protocol`
    ///   includes `AS2`, then the `EndpointType` must be `VPC`, and domain must be
    ///   Amazon S3".
    ///
    /// Where one member's value makes a whole protocol FAMILY unservable, that gate is
    /// evaluated before any rule about how a protocol of that family would be
    /// configured, so every protocol of the family answers the one refusal the
    /// condition they share earns rather than its own configuration rule.
    pub fn cross_member_violation(&self) -> Option<String> {
        let endpoint_type = self.endpoint_type;
        let hosted_in_vpc =
            matches!(endpoint_type, ENDPOINT_TYPE_VPC | ENDPOINT_TYPE_VPC_ENDPOINT);

        if hosted_in_vpc && self.endpoint_details.is_none() {
            return Some(ENDPOINT_DETAILS_REQUIRED_FOR_VPC.to_string());
        }

        if let Some(details) = self.endpoint_details {
            for (member, submitted, only_with) in [
                (
                    "EndpointDetails.VpcId",
                    details.vpc_id.is_some(),
                    ENDPOINT_TYPE_VPC,
                ),
                (
                    "EndpointDetails.SubnetIds",
                    details.subnet_ids.is_some(),
                    ENDPOINT_TYPE_VPC,
                ),
                (
                    "EndpointDetails.SecurityGroupIds",
                    details.security_group_ids.is_some(),
                    ENDPOINT_TYPE_VPC,
                ),
                (
                    "EndpointDetails.AddressAllocationIds",
                    details.address_allocation_ids.is_some(),
                    ENDPOINT_TYPE_VPC,
                ),
                (
                    "EndpointDetails.VpcEndpointId",
                    details.vpc_endpoint_id.is_some(),
                    ENDPOINT_TYPE_VPC_ENDPOINT,
                ),
            ] {
                if submitted && endpoint_type != only_with {
                    return Some(settable_only_when(
                        member,
                        &format!("EndpointType is {only_with}"),
                    ));
                }
            }
            if details.address_allocation_ids.is_some()
                && self.ip_address_type == IP_ADDRESS_TYPE_DUALSTACK
            {
                return Some(forbidden_when(
                    "EndpointDetails.AddressAllocationIds",
                    &format!("IpAddressType is {IP_ADDRESS_TYPE_DUALSTACK}"),
                ));
            }
        }

        let provider_condition = format!("IdentityProviderType is {}", self.identity_provider_type);
        match (
            self.needs_identity_provider_details(),
            self.identity_provider_details.is_some(),
        ) {
            (true, false) => {
                return Some(required_for_value(
                    "IdentityProviderDetails",
                    "IdentityProviderType",
                    self.identity_provider_type,
                ))
            }
            (false, true) => {
                return Some(forbidden_when(
                    "IdentityProviderDetails",
                    &provider_condition,
                ))
            }
            _ => {}
        }

        // The FTP/FTPS FAMILY gate, evaluated before ANY rule about how one of those
        // protocols would be configured (the FTPS certificate below, the endpoint-type
        // half further down): which protocols a server can serve at all follows from
        // its authentication mode, and no certificate and no endpoint type makes a
        // provider that cannot serve the family able to serve it. Reporting a
        // configuration rule first would send the caller down a path that stays illegal
        // once taken — and the two protocols of the one family would answer two
        // different refusals for the one condition they share.
        if (self.speaks(PROTOCOL_FTP) || self.speaks(PROTOCOL_FTPS))
            && !self.needs_identity_provider_details()
        {
            return Some(unsupported_for_identity_provider(
                self.identity_provider_type,
            ));
        }

        // An empty `Certificate` names no certificate: the member's `len 0..1600`
        // admits the empty string, so presence alone is not enough.
        if self.speaks(PROTOCOL_FTPS)
            && self
                .certificate
                .is_none_or(|certificate| certificate.trim().is_empty())
        {
            return Some(required_when(
                "Certificate",
                &format!("Protocols includes {PROTOCOL_FTPS}"),
            ));
        }

        for protocol in [PROTOCOL_FTP, PROTOCOL_FTPS] {
            if !self.speaks(protocol) {
                continue;
            }
            if endpoint_type != ENDPOINT_TYPE_VPC {
                return Some(must_be_when(
                    "EndpointType",
                    ENDPOINT_TYPE_VPC,
                    &format!("Protocols includes {protocol}"),
                ));
            }
        }

        if self.speaks(PROTOCOL_AS2) {
            if endpoint_type != ENDPOINT_TYPE_VPC {
                return Some(must_be_when(
                    "EndpointType",
                    ENDPOINT_TYPE_VPC,
                    &format!("Protocols includes {PROTOCOL_AS2}"),
                ));
            }
            if self.domain != DOMAIN_S3 {
                return Some(must_be_when(
                    "Domain",
                    DOMAIN_S3,
                    &format!("Protocols includes {PROTOCOL_AS2}"),
                ));
            }
        }

        None
    }
}

/// The connector configuration a `CreateConnector` request resolves to: every member a
/// documented cross-member requirement mentions, with `EgressType` — which the rest are
/// stated against — already resolved from `EgressConfig`.
#[derive(Debug)]
pub struct ConnectorConfig<'a> {
    pub url: Option<&'a str>,
    pub as2_config: Option<&'a As2ConnectorConfig>,
    pub sftp_config: Option<&'a SftpConnectorConfig>,
    pub egress_type: &'a str,
}

impl ConnectorConfig<'_> {
    /// The first documented cross-member requirement this configuration violates,
    /// worded as the refusal states it — `None` when it violates none.
    ///
    /// Every rule below is a sentence in this service's own model:
    ///
    /// * `CreateConnector` states "You must specify exactly one configuration object:
    ///   either for AS2 (`As2Config`) or SFTP (`SftpConfig`)".
    /// * `EgressConfig` "determines how traffic is routed from the connector to the
    ///   SFTP server", and `ConnectorEgressType`'s `VPC_LATTICE` "routes traffic through
    ///   customer VPCs" to that same SFTP server — so only an SFTP connector has one.
    /// * `Url` states "When creating AS2 connectors or service-managed SFTP connectors
    ///   (connectors without egress configuration), you must provide a URL to specify
    ///   the remote server endpoint. For VPC Lattice type connectors, the URL must be
    ///   null."
    /// * `Url` is also "the URL of the partner's AS2 or SFTP endpoint" — the address a
    ///   transfer on this connector connects out to — so a value that names no endpoint
    ///   the service could reach is refused with the service's own fixed text for the
    ///   condition the value fails on ([`endpoint_url_defect`]).
    /// * `As2Config` is "a structure that contains the parameters for an AS2 connector
    ///   object", and the members that settle how an AS2 message is built carry no
    ///   documented default, so each is required on its own
    ///   ([`as2_configuration_violation`]).
    /// * `MaxConcurrentConnections` states "The maximum values is `5`" — a bound the
    ///   member's `@range` does not carry (see [`MAX_CONCURRENT_CONNECTIONS`]).
    pub fn cross_member_violation(&self) -> Option<String> {
        if let Some(violation) = either_or_violation(
            ("As2Config", self.as2_config.is_some()),
            ("SftpConfig", self.sftp_config.is_some()),
        ) {
            return Some(violation);
        }

        let vpc_lattice = self.egress_type == CONNECTOR_EGRESS_TYPE_VPC_LATTICE;
        if vpc_lattice && self.sftp_config.is_none() {
            return Some(settable_only_when("EgressConfig", "SftpConfig is specified"));
        }

        // An empty `Url` names no endpoint — the same reading [`connector_url`] gives
        // it when the connector is stored, and the one `Certificate` already gets in
        // `ServerConfig::cross_member_violation`.
        let endpoint_named = connector_url(self.url).is_some();
        match (vpc_lattice, endpoint_named) {
            (true, true) => {
                return Some(forbidden_when(
                    "Url",
                    &format!("EgressType is {CONNECTOR_EGRESS_TYPE_VPC_LATTICE}"),
                ))
            }
            (false, false) => {
                return Some(required_when(
                    "Url",
                    &format!("EgressType is {CONNECTOR_EGRESS_TYPE_SERVICE_MANAGED}"),
                ))
            }
            _ => {}
        }

        // A `Url` that names an endpoint has to name one the service could reach: the
        // member is the address a transfer on this connector connects OUT to, so a
        // value that is no absolute URL, or whose host can never be resolved, is
        // refused here rather than stored on a connector no transfer can complete. See
        // [`connector_endpoint_url::endpoint_url_defect`], whose text is the service's
        // own fixed sentence for the condition.
        if let Some(url) = connector_url(self.url) {
            if let Some(defect) = endpoint_url_defect(&url) {
                return Some(defect.to_string());
            }
        }

        // The AS2 configuration then has to settle how the messages this connector sends
        // are BUILT: the members that choose the compression, the encryption and the
        // signing are each required on their own, and the refusal names the one that is
        // missing. See [`as2_connector_requirements`], and note the position — after the
        // either/or above and after the `Url`, so a request that breaks one of those is
        // answered for it rather than for a member of a configuration the connector may
        // not even be allowed to carry.
        if let Some(violation) = self.as2_config.and_then(as2_configuration_violation) {
            return Some(violation);
        }

        if self
            .sftp_config
            .is_some_and(|config| config.max_concurrent_connections > MAX_CONCURRENT_CONNECTIONS)
        {
            return Some(must_be_at_most(
                "SftpConfig.MaxConcurrentConnections",
                MAX_CONCURRENT_CONNECTIONS,
            ));
        }

        None
    }
}

/// The documented cross-member requirement a user's home-directory configuration
/// violates, worded as the refusal states it — `None` when it violates none.
///
/// `HomeDirectoryType` states "If `HomeDirectoryType` is `LOGICAL`, you must provide
/// mappings, using the `HomeDirectoryMappings` parameter", and
/// `HomeDirectoryMappings` states "This value can be set only when
/// `HomeDirectoryType` is set to `LOGICAL`" — one requirement in each direction.
///
/// The missing-mappings direction states `HomeDirectoryMappings`' OWN modelled
/// minimum (`@length` min 1, max 50000), because that is the constraint an absent or
/// empty list violates; the opposite direction has no modelled constraint of its own
/// to state, so it names the condition the value was sent under.
///
/// The mappings a request DOES send are then held to the requirement they carry among
/// themselves — no two entries may name one path ([`home_directory_mappings`]) — so
/// every write of these members goes through one validator, whichever resource it
/// configures.
pub fn home_directory_violation(
    home_directory_type: Option<&str>,
    home_directory_mappings: Option<&[HomeDirectoryMapEntry]>,
) -> Option<String> {
    let logical = home_directory_type == Some(HOME_DIRECTORY_TYPE_LOGICAL);
    let mapped = home_directory_mappings.is_some_and(|mappings| !mappings.is_empty());
    match (logical, mapped) {
        (true, false) => return Some(must_not_be_empty("HomeDirectoryMappings")),
        (false, true) => {
            return Some(settable_only_when(
                "HomeDirectoryMappings",
                &format!("HomeDirectoryType is {HOME_DIRECTORY_TYPE_LOGICAL}"),
            ))
        }
        _ => {}
    }

    // The mappings also have to be consistent WITH EACH OTHER: each `Entry` is the path
    // one `Target` is made visible at, so two entries that name one path leave the
    // logical directory undefined. See [`home_directory_mappings`], which states the
    // rule and words the refusal.
    home_directory_mappings.and_then(home_directory_mappings::overlapping_entry)
}

/// One stored directory access grant, keyed by `(ServerId, ExternalId)`.
///
/// An access grant is a GROUP in the directory the server authenticates against (see
/// [`accesses_require_directory_provider`]), so a grant exists only on a server whose
/// `IdentityProviderType` is `AWS_DIRECTORY_SERVICE`, and the server id is part of its
/// key: the same directory group may be granted access on several servers
/// independently. `BTreeMap` on that key so one server's grants are a contiguous,
/// ordered range for `ListAccesses`.
///
/// `DeleteAccess` reads only the grant's identity; the members below are the ones
/// `DescribedAccess` reports, which `CreateAccess` resolves when it writes a grant and
/// `UpdateAccess` replaces one at a time.
#[derive(Debug, Clone, Default)]
pub struct Access {
    pub server_id: String,
    pub external_id: String,
    pub home_directory: Option<String>,
    pub home_directory_type: Option<String>,
    pub home_directory_mappings: Vec<HomeDirectoryMapEntry>,
    pub policy: Option<String>,
    pub posix_profile: Option<PosixProfile>,
    pub role: Option<String>,
}

/// One stored Transfer Family web app, keyed by `WebAppId`.
///
/// Every member a reader reports is RESOLVED here, at create time — the documented
/// `WebAppEndpointPolicy` default, the `EndpointType` the endpoint configuration
/// selects, the web app's own endpoint host name, the `AccessEndpoint` (the caller's
/// custom URL or the default one), and the IAM Identity Center application the create
/// registered. So `DescribeWebApp` projects the stored record without re-applying a
/// default, and the identity-provider and endpoint details are kept in the `Described*`
/// shapes the readers answer with. `ListWebApps` pages this map in `WebAppId` order and
/// projects `ListedWebApp` from the same record.
#[derive(Debug, Clone)]
pub struct WebApp {
    pub web_app_id: String,
    pub arn: String,
    /// The identity provider as the readers report it, with the `ApplicationArn` the
    /// create minted already in place.
    pub identity_provider_details: Option<DescribedWebAppIdentityProviderDetails>,
    /// The URL users are given for this web app: the caller's custom
    /// `AccessEndpoint`, or [`default_access_endpoint`] when the create named none.
    pub access_endpoint: String,
    /// The web app's own unique endpoint host name — see [`web_app_endpoint`].
    pub web_app_endpoint: String,
    /// `WebAppUnits` exactly as the caller set it. The model documents no default, so
    /// a create that named none leaves this `None` rather than inventing a unit count.
    pub web_app_units: Option<WebAppUnits>,
    pub web_app_endpoint_policy: String,
    /// `PUBLIC` or `VPC`, selected by whether the create configured a VPC endpoint.
    pub endpoint_type: String,
    /// The endpoint configuration as the readers report it, with the `VpcEndpointId`
    /// the create minted for a VPC-hosted web app. `None` for a `PUBLIC` web app,
    /// which has no VPC endpoint to report.
    pub endpoint_details: Option<DescribedWebAppEndpointDetails>,
    pub tags: Vec<Tag>,
    /// The branding `UpdateWebAppCustomization` assigns to this web app and
    /// `DescribeWebAppCustomization` reports — see [`WebAppCustomization`]. It lives on
    /// the web app rather than in a map of its own because a customization has no
    /// identifier of its own: it is addressed by `WebAppId`, and
    /// `DescribedWebAppCustomization.Arn` is documented as "the ARN for the web app",
    /// i.e. [`Self::arn`].
    ///
    /// `None` is the web app HAVING no customization: a customization is a sub-resource
    /// that comes into existence when an update assigns it ("deletes the
    /// WebAppCustomization object that corresponds to the web app ID specified" is what
    /// `DeleteWebAppCustomization` does to it) and ceases to exist when that delete
    /// removes it, so a reader of a web app that holds none has no object to project —
    /// see [`no_web_app_customization`]. No `CreateWebApp` member can set one, so a new
    /// web app starts with `None`.
    pub customization: Option<WebAppCustomization>,
    /// Whether a write of this web app's own CONFIGURATION is being applied right now,
    /// and so whether a second one that arrives is a racing writer — see
    /// [`web_app_write_claim`], which is the one place this is taken and released.
    pub configuration_write_in_flight: bool,
    /// Whether a write of this web app's CUSTOMIZATION is being applied right now, and
    /// so whether a second one that arrives is a racing writer — see
    /// [`web_app_customization`], which is the one place this is taken and released.
    pub customization_write_in_flight: bool,
}

/// `ConflictException`'s `Message` for a write of a web app's own configuration that
/// could not be carried out because another write of it was still being applied.
///
/// The service's own sentence for the condition: it states that the write could not be
/// done and names the resource by the service's own name for it, in the same
/// `Unable to <verb> WebApp …` form the two writes of a web app's CUSTOMIZATION are
/// refused with ([`web_app_customization::UPDATE_CONFLICT`] and its sibling, which add
/// the sub-resource they address). Fixed for the condition in its entirety: it has no
/// slot for the web app id or anything else from the request or the store — the web app
/// a caller addressed is the one it named — so every loser of every race reads the same.
pub const WEB_APP_UPDATE_CONFLICT: &str = "Unable to update WebApp";

/// The conflict payload for an update of a web app that is still applying another.
pub fn web_app_update_conflict() -> ConflictException {
    ConflictException {
        message: WEB_APP_UPDATE_CONFLICT.to_string(),
    }
}

/// The refusal for a VPC-endpoint update of a web app that has no VPC endpoint.
///
/// `EndpointType` is "`PUBLIC` for publicly accessible endpoints and `VPC` for
/// VPC-hosted endpoints", and it is selected by the endpoint configuration the create
/// carried — so a publicly accessible web app has no VPC endpoint whose subnets an
/// update could change, which makes the request a malformed one.
pub fn web_app_has_no_vpc_endpoint() -> InvalidRequestException {
    InvalidRequestException {
        message: settable_only_when(
            "EndpointDetails.Vpc",
            &format!("EndpointType is {WEB_APP_ENDPOINT_TYPE_VPC}"),
        ),
    }
}

/// One web app's customization: the page title, logo and icon a caller assigned to it.
///
/// `UpdateWebAppCustomization` is documented as "assigns new customization properties
/// to a web app. You can modify the icon file, logo file, and title", and models each
/// of those three members OPTIONAL beside the `@required` `WebAppId` — so an update
/// assigns the members it carries and leaves the others as they stand
/// ([`Self::assign`]); removing a customization is what the service's own
/// `DeleteWebAppCustomization` is for, not an omitted member — and that delete removes
/// the WHOLE object ([`WebApp::customization`] goes back to `None`), which is what an
/// omitted member never does. Each member is `None`
/// until an update assigns it, and `DescribeWebAppCustomization` reports exactly the
/// ones that were: `Title` is modelled `len 0..100`, so `Some(String::new())` is a
/// title a caller really did assign (the empty one) and is distinct from never having
/// assigned one, while `LogoFile`/`FaviconFile` are modelled `len 1..`, so no empty
/// blob can be stored.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct WebAppCustomization {
    pub title: Option<String>,
    pub logo_file: Option<Vec<u8>>,
    pub favicon_file: Option<Vec<u8>>,
}

impl WebAppCustomization {
    /// Assign the customization members an update CARRIES, leaving the ones it omits
    /// exactly as they stand.
    ///
    /// The one place this service applies the partial-update rule stated on
    /// [`WebAppCustomization`], so an update and any later reader cannot disagree about
    /// what an omitted member means.
    pub fn assign(
        &mut self,
        title: Option<String>,
        logo_file: Option<Vec<u8>>,
        favicon_file: Option<Vec<u8>>,
    ) {
        if let Some(title) = title {
            self.title = Some(title);
        }
        if let Some(logo_file) = logo_file {
            self.logo_file = Some(logo_file);
        }
        if let Some(favicon_file) = favicon_file {
            self.favicon_file = Some(favicon_file);
        }
    }
}

/// The first documented step requirement a submitted step LIST violates, worded as the
/// refusal states it — `None` when every step satisfies them.
///
/// A requirement the service reports as a missing or misplaced member names the step by
/// its zero-based index in `member` — the way a JSON request indexes the list — so one
/// message locates the step a caller has to fix; `member` is the request member the list
/// came from (`Steps` or `OnExceptionSteps`). A requirement the service states as a
/// FIXED sentence about the defect itself (see [`CUSTOM_STEP_FUNCTION_INVALID`]) names
/// neither, whichever list and index the step sat at.
pub fn workflow_steps_violation(member: &str, steps: &[WorkflowStep]) -> Option<String> {
    steps
        .iter()
        .enumerate()
        .find_map(|(index, step)| workflow_step_violation(&format!("{member}[{index}]"), step))
}

/// The documented requirement ONE workflow step violates, worded as the refusal states
/// it — `None` when it violates none.
///
/// Both rules below are sentences in this service's own model:
///
/// * `WorkflowStep.Type` "specifies which of the following actions is being taken for
///   this step", and each `*StepDetails` member is documented as the details "for a
///   step that" performs exactly that one action — so a step carries the details
///   structure its `Type` names, and no other. A step that names no `Type` names no
///   action at all.
/// * each details shape documents what the step acts ON as part of what the step
///   "consists of": a `CUSTOM` step is "a custom step with an Lambda function target"
///   whose `Target` is "The ARN for the Lambda function that is being called", a `COPY`
///   step consists of "An Amazon S3 location for the destination of the file copy", and
///   a `TAG` step's `Tags` are the "1 to 10 key/value pairs" it adds. `DECRYPT`'s own
///   two (`Type`, `DestinationFileLocation`) are `@required` members of
///   `DecryptStepDetails`, so the boundary validator refuses those before a handler
///   runs, and `DELETE` acts on the source file alone.
///
/// A `CUSTOM` step is the one case the service words as a fixed sentence of its own
/// rather than as a member requirement: whether it sent no details or details naming no
/// Lambda function, the step provides no function to invoke and the refusal says so
/// ([`CUSTOM_STEP_FUNCTION_INVALID`]).
fn workflow_step_violation(path: &str, step: &WorkflowStep) -> Option<String> {
    let Some(step_type) = step.r#type.as_deref().filter(|named| !named.is_empty()) else {
        return Some(required_member(&format!("{path}.Type")));
    };
    let condition = format!("Type is {step_type}");

    // The details member each step type selects: present for its own type, absent for
    // every other one.
    for (member, submitted, selected_by) in [
        (
            "CopyStepDetails",
            step.copy_step_details.is_some(),
            WORKFLOW_STEP_TYPE_COPY,
        ),
        (
            "CustomStepDetails",
            step.custom_step_details.is_some(),
            WORKFLOW_STEP_TYPE_CUSTOM,
        ),
        (
            "TagStepDetails",
            step.tag_step_details.is_some(),
            WORKFLOW_STEP_TYPE_TAG,
        ),
        (
            "DeleteStepDetails",
            step.delete_step_details.is_some(),
            WORKFLOW_STEP_TYPE_DELETE,
        ),
        (
            "DecryptStepDetails",
            step.decrypt_step_details.is_some(),
            WORKFLOW_STEP_TYPE_DECRYPT,
        ),
    ] {
        let member = format!("{path}.{member}");
        match (submitted, step_type == selected_by) {
            // A `CUSTOM` step that sent no details named no function either, which the
            // function requirement below reports — in the service's own wording for
            // that defect, rather than as a missing member.
            (false, true) if selected_by == WORKFLOW_STEP_TYPE_CUSTOM => {}
            (false, true) => return Some(required_when(&member, &condition)),
            (true, false) => {
                return Some(settable_only_when(
                    &member,
                    &format!("Type is {selected_by}"),
                ))
            }
            _ => {}
        }
    }

    // What the step acts on. An empty string names nothing: `CustomStepTarget` is
    // `len 0..170`, so the modelled length admits one, and its pattern
    // `^arn:[a-z-]+:lambda:.*$` is what refuses it at the boundary — this is the
    // belt-and-braces check that an absent target is refused here either way.
    match step_type {
        // A `CUSTOM` step is "a custom step with an Lambda function target", so the
        // function it calls is what the step acts through: a step that names none —
        // because it sent no details at all, or details with no `Target`, or a blank
        // one — is refused for the function, in the service's own fixed wording for
        // that defect, which reports nothing from the request.
        WORKFLOW_STEP_TYPE_CUSTOM
            if step.custom_step_details.as_ref().is_none_or(|details| {
                details
                    .target
                    .as_deref()
                    .is_none_or(|target| target.trim().is_empty())
            }) =>
        {
            Some(CUSTOM_STEP_FUNCTION_INVALID.to_string())
        }
        WORKFLOW_STEP_TYPE_COPY
            if step
                .copy_step_details
                .as_ref()
                .is_some_and(|details| details.destination_file_location.is_none()) =>
        {
            Some(required_when(
                &format!("{path}.CopyStepDetails.DestinationFileLocation"),
                &condition,
            ))
        }
        WORKFLOW_STEP_TYPE_TAG
            if step.tag_step_details.as_ref().is_some_and(|details| {
                details.tags.as_ref().is_none_or(|tags| tags.is_empty())
            }) =>
        {
            Some(required_when(
                &format!("{path}.TagStepDetails.Tags"),
                &condition,
            ))
        }
        _ => None,
    }
}

/// The steps as they will be STORED: the caller's order, with every documented
/// step-detail default resolved once, here, so no reader has to re-apply one.
pub fn resolve_workflow_steps(steps: Vec<WorkflowStep>) -> Vec<WorkflowStep> {
    steps.into_iter().map(resolve_workflow_step).collect()
}

/// `current` when the caller sent one, else `fallback` — the one place a step-detail
/// default is applied.
fn defaulted(current: Option<String>, fallback: &str) -> Option<String> {
    Some(current.unwrap_or_else(|| fallback.to_string()))
}

/// One step with its documented defaults applied: every details shape that carries
/// `SourceFileLocation` defaults it to [`DEFAULT_SOURCE_FILE_LOCATION`], and every one
/// that carries `OverwriteExisting` defaults it to [`DEFAULT_OVERWRITE_EXISTING`].
fn resolve_workflow_step(mut step: WorkflowStep) -> WorkflowStep {
    if let Some(details) = step.copy_step_details.as_mut() {
        details.source_file_location =
            defaulted(details.source_file_location.take(), DEFAULT_SOURCE_FILE_LOCATION);
        details.overwrite_existing =
            defaulted(details.overwrite_existing.take(), DEFAULT_OVERWRITE_EXISTING);
    }
    if let Some(details) = step.custom_step_details.as_mut() {
        details.source_file_location =
            defaulted(details.source_file_location.take(), DEFAULT_SOURCE_FILE_LOCATION);
    }
    if let Some(details) = step.tag_step_details.as_mut() {
        details.source_file_location =
            defaulted(details.source_file_location.take(), DEFAULT_SOURCE_FILE_LOCATION);
    }
    if let Some(details) = step.delete_step_details.as_mut() {
        details.source_file_location =
            defaulted(details.source_file_location.take(), DEFAULT_SOURCE_FILE_LOCATION);
    }
    if let Some(details) = step.decrypt_step_details.as_mut() {
        details.source_file_location =
            defaulted(details.source_file_location.take(), DEFAULT_SOURCE_FILE_LOCATION);
        details.overwrite_existing =
            defaulted(details.overwrite_existing.take(), DEFAULT_OVERWRITE_EXISTING);
    }
    step
}

/// Per-service state for com-amazonaws-transfer-transferservice. A handler reaches it through [`service_state`].
#[derive(Debug, Default)]
pub struct ComAmazonawsTransferTransferserviceState {
    /// Servers by `ServerId` (`"s-" + 17 hex`). `BTreeMap` so `ListServers` pages in a
    /// stable, resumable order.
    pub servers: BTreeMap<String, Server>,
    /// Users by `(ServerId, UserName)`, so one server's users are a contiguous,
    /// ordered range for `ListUsers`.
    pub users: BTreeMap<(String, String), User>,
    /// Directory access grants by `(ServerId, ExternalId)`, so one server's grants are
    /// a contiguous, ordered range for `ListAccesses`.
    pub accesses: BTreeMap<(String, String), Access>,
    /// Workflow executions by `(WorkflowId, ExecutionId)`, so one workflow's
    /// executions are a contiguous, ordered range for `ListExecutions`.
    ///
    /// BOUNDED by [`MAX_EXECUTIONS_PER_WORKFLOW`] per workflow and by
    /// [`MAX_STORED_EXECUTIONS`] in total, which
    /// [`Self::take_execution_capacity`] is the predicate for: an upload that finds no
    /// room starts no execution. `DeleteWorkflow` releases a workflow's records, and a
    /// run whose callbacks have FINISHED is reclaimable at the bound, because nothing
    /// addresses a finished run any more.
    pub workflow_executions: BTreeMap<(String, String), WorkflowExecution>,
    /// Web apps by `WebAppId` (`"webapp-" + 17 hex`). `BTreeMap` so `ListWebApps` pages
    /// in a stable, resumable order.
    pub web_apps: BTreeMap<String, WebApp>,
    /// Host keys by `(ServerId, HostKeyId)`, so one server's keys are a contiguous,
    /// ordered range for `ListHostKeys`.
    pub host_keys: BTreeMap<(String, String), HostKey>,
    /// Connectors by `ConnectorId` (`"c-" + 17 hex`). `BTreeMap` for the same reason
    /// as [`Self::servers`]: `ListConnectors` pages it in a stable, resumable order.
    /// BOUNDED by [`MAX_STORED_CONNECTORS`], which
    /// [`Self::has_connector_capacity`] is the predicate for.
    pub connectors: BTreeMap<String, Connector>,
    /// AS2 profiles by `ProfileId` (`"p-" + 17 hex`). `BTreeMap` so `ListProfiles`
    /// pages in a stable, resumable order, like [`Self::servers`].
    pub profiles: BTreeMap<String, Profile>,
    /// AS2 agreements by `(ServerId, AgreementId)` — the pair the model makes the
    /// `AgreementResource`'s identifiers — so one server's agreements are a contiguous,
    /// ordered range for `ListAgreements`, exactly as [`Self::users`] is keyed.
    pub agreements: BTreeMap<(String, String), Agreement>,
    /// Imported certificates by `CertificateId` (`"cert-" + 17 hex`). `BTreeMap` so
    /// `ListCertificates` pages in a stable, resumable order, like [`Self::profiles`].
    pub certificates: BTreeMap<String, Certificate>,
    /// Workflows by `WorkflowId` (`"w-" + 17 hex`). `BTreeMap` so `ListWorkflows` pages
    /// in a stable, resumable order.
    pub workflows: BTreeMap<String, Workflow>,
    /// The operations a connector has ACCEPTED, by `(ConnectorId, TransferId)`, so one
    /// connector's are a contiguous, ordered range — exactly as [`Self::users`] is
    /// keyed. BOUNDED per connector by [`MAX_TRANSFERS_PER_CONNECTOR`], which
    /// [`Self::connector_has_transfer_capacity`] is the predicate for.
    pub connector_transfers: BTreeMap<(String, String), ConnectorTransfer>,
    /// Monotonic counter behind every minted id, so ids are unique and reproducible.
    pub next_id_seq: u64,
}

impl ComAmazonawsTransferTransferserviceState {
    /// Mint the next `<prefix>-<17 hex digits>` identifier.
    ///
    /// `ServerId` is `^s-([0-9a-f]{17})$`, `SshPublicKeyId` is `^key-[0-9a-f]{17}$`,
    /// `HostKeyId` is `^hostkey-[0-9a-f]{17}$`,
    /// `ConnectorId` is `^c-([0-9a-f]{17})$`, `ProfileId` is `^p-([0-9a-f]{17})$` and
    /// `WebAppId` is `^webapp-[0-9a-f]{17}$`,
    /// so all of them are this shape;
    /// `{:017x}` is exactly 17 lowercase hex digits for every
    /// counter value below 16^17, which a counter incremented once per created
    /// resource cannot reach.
    pub fn mint_id(&mut self, prefix: &str) -> String {
        self.mint_hex_id(prefix, 17)
    }

    /// The ONE minting rule: bump the counter and render it as `<prefix>-<digits
    /// lowercase hex digits>`, zero-padded. Every minted identifier in this service is
    /// this shape and differs only in its prefix and width, so the counter bump and the
    /// formatting live here once.
    fn mint_hex_id(&mut self, prefix: &str, digits: usize) -> String {
        self.next_id_seq += 1;
        format!("{prefix}-{:0width$x}", self.next_id_seq, width = digits)
    }

    /// Mint the identifier of one accepted connector operation.
    ///
    /// `TransferId` and `MoveId` are `len 1..512`, pattern `^[0-9a-zA-Z./-]+$`, and
    /// carry no documented prefix — unlike every resource id this service mints — so
    /// this is the bare counter in 32 lowercase hex digits, which that pattern admits
    /// and which no resource identifier can be mistaken for.
    pub fn mint_transfer_id(&mut self) -> String {
        self.next_id_seq += 1;
        format!("{:032x}", self.next_id_seq)
    }

    /// Mint the identifier of one workflow execution.
    ///
    /// `ExecutionId` is `len 36..36` with the UUID pattern
    /// `^[0-9a-fA-F]{8}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{12}$`,
    /// so the counter is rendered into that shape: the version and variant nibbles are
    /// the fixed middle groups and the counter fills the first and last, which keeps
    /// ascending runs ascending ids — the order `ListExecutions` pages in.
    pub fn mint_execution_id(&mut self) -> String {
        self.next_id_seq += 1;
        format!(
            "{:08x}-0000-4000-8000-{:012x}",
            self.next_id_seq, self.next_id_seq
        )
    }

    /// Mint the callback token ONE pending custom step of an execution is handed.
    ///
    /// `CallbackToken` is `len 1..64`, pattern `^\w+$`, and "used to distinguish
    /// between multiple callbacks for multiple Lambda steps within the same execution",
    /// so it is the bare counter in 32 lowercase hex digits — unique across this store,
    /// like every other minted value, and distinct per step.
    pub fn mint_callback_token(&mut self) -> String {
        self.next_id_seq += 1;
        format!("{:032x}", self.next_id_seq)
    }

    /// The users stored for `server_id`, in `ListUsers` paging order.
    ///
    /// A RANGE scan, not a filter over the whole map: the key is
    /// `(ServerId, UserName)`, so one server's users are a contiguous run, and this
    /// walks only that run. One allocation (the range's lower bound) for the whole
    /// walk.
    pub fn users_of<'a>(&'a self, server_id: &'a str) -> impl Iterator<Item = &'a User> + 'a {
        use ::std::ops::Bound;
        let lower = (server_id.to_string(), String::new());
        self.users
            .range((Bound::Included(lower), Bound::Unbounded))
            .take_while(move |((sid, _), _)| sid == server_id)
            .map(|(_, user)| user)
    }

    /// The host keys stored for `server_id`, in `ListHostKeys` paging order.
    ///
    /// A RANGE scan over that server's contiguous run of the key map, on the same rule
    /// as [`Self::users_of`].
    pub fn host_keys_of<'a>(&'a self, server_id: &'a str) -> impl Iterator<Item = &'a HostKey> + 'a {
        use ::std::ops::Bound;
        let lower = (server_id.to_string(), String::new());
        self.host_keys
            .range((Bound::Included(lower), Bound::Unbounded))
            .take_while(move |((sid, _), _)| sid == server_id)
            .map(|(_, host_key)| host_key)
    }

    /// Whether `server_id` can take another host key, i.e. is below
    /// [`MAX_HOST_KEYS_PER_SERVER`].
    pub fn server_has_host_key_capacity(&self, server_id: &str) -> bool {
        self.host_keys_of(server_id).count() < MAX_HOST_KEYS_PER_SERVER
    }

    /// Materialise the host key a server HOLDS as one of that server's host-key
    /// resources, and answer the `HostKeyId` it is held under.
    ///
    /// The key whose fingerprint a server reports as its own is one of its host keys:
    /// the service manages it, so `ListHostKeys` enumerates it and `DescribeHostKey`
    /// reports it, exactly as it does a key a caller imported — which is what lets a
    /// client that audits or rotates a server's keys find the one the server is
    /// actually serving with. Every write that SETS a server's host key calls this with
    /// the fingerprint and algorithm read out of that key's material
    /// ([`read_host_key`]) or generated with it ([`mint_host_key`]), so no server can
    /// report a fingerprint none of its host keys has.
    ///
    /// A server that already holds a key with this fingerprint keeps it — the material
    /// IS the key's identity within a server, the rule `ImportHostKey` refuses a
    /// re-import on — so setting the same key twice stores one record, not two.
    ///
    /// The per-server BOUND ([`MAX_HOST_KEYS_PER_SERVER`],
    /// [`Self::server_has_host_key_capacity`]) belongs to the caller: `CreateServer`
    /// materialises the first key of a server that holds none, and `UpdateServer`
    /// refuses a rotation the server has no room for before it writes anything.
    pub fn materialise_host_key(
        &mut self,
        region: &str,
        server_id: &str,
        key: &HostKeyMaterial,
    ) -> String {
        let held = self
            .host_keys_of(server_id)
            .find(|host_key| host_key.fingerprint == key.fingerprint)
            .map(|host_key| host_key.host_key_id.clone());
        if let Some(host_key_id) = held {
            return host_key_id;
        }
        let host_key_id = self.mint_id("hostkey");
        let host_key = HostKey {
            arn: host_key_arn(region, server_id, &host_key_id),
            fingerprint: key.fingerprint.clone(),
            key_type: key.key_type.clone(),
            public_key: key.public_key.clone(),
            // The service generated or adopted this key for the server itself: no
            // caller described it and no caller tagged it, so neither member is
            // invented for it.
            description: None,
            date_imported: SystemTime::now(),
            storage_type: DEFAULT_HOST_KEY_STORAGE_TYPE.to_string(),
            tags: Vec::new(),
            host_key_id: host_key_id.clone(),
        };
        self.host_keys
            .insert((server_id.to_string(), host_key_id.clone()), host_key);
        host_key_id
    }

    /// The PUBLIC half of the host key `server_id` is serving with — the key a client
    /// that connects to that server is presented with on the host-key exchange.
    ///
    /// Read off the SERVER's own record, which the write that set the server's host key
    /// filled from the same material as the fingerprint it reports, so this is the key
    /// the server serves with rather than whichever one was imported last. `None` for a
    /// `ServerId` the store does not hold, and for a key that did not encode.
    pub fn active_host_key_public(&self, server_id: &str) -> Option<String> {
        self.servers
            .get(server_id)
            .map(|server| server.host_key_public.clone())
            .filter(|public_key| !public_key.is_empty())
    }

    /// `UserCount`: the live number of users assigned to `server_id`.
    ///
    /// Walks only that server's run of the user map. A caller that needs the count for
    /// SEVERAL servers (`ListServers`) must use [`Self::user_counts`] instead, so the
    /// work stays one pass in total rather than one walk per listed server.
    pub fn user_count(&self, server_id: &str) -> i32 {
        i32::try_from(self.users_of(server_id).count()).unwrap_or(i32::MAX)
    }

    /// Whether `server_id` can take another user, i.e. is below
    /// [`MAX_USERS_PER_SERVER`].
    pub fn server_has_user_capacity(&self, server_id: &str) -> bool {
        self.users_of(server_id).count() < MAX_USERS_PER_SERVER
    }

    /// Whether the store can take another server, i.e. is below
    /// [`MAX_STORED_SERVERS`].
    pub fn has_server_capacity(&self) -> bool {
        self.servers.len() < MAX_STORED_SERVERS
    }

    /// Whether the store can take another web app, i.e. is below
    /// [`MAX_STORED_WEB_APPS`].
    pub fn has_web_app_capacity(&self) -> bool {
        self.web_apps.len() < MAX_STORED_WEB_APPS
    }

    /// Mint the `apl-<16 hex digits>` identifier of the IAM Identity Center
    /// application a `CreateWebApp` registers for the new web app.
    ///
    /// `IdentityCenterApplicationArn`'s modelled pattern ends in `apl-[a-zA-Z0-9]{16}`
    /// — 16 characters, one fewer than [`Self::mint_id`] emits — so this is
    /// [`Self::mint_hex_id`] at that width. It shares the one counter, so an
    /// application id is unique across this store like every other minted id.
    pub fn mint_identity_center_application_id(&mut self) -> String {
        self.mint_hex_id("apl", 16)
    }

    /// Whether the store can take another connector, i.e. is below
    /// [`MAX_STORED_CONNECTORS`].
    ///
    /// The connector counterpart of [`Self::has_server_capacity`]: a connector write
    /// consults this BEFORE inserting and refuses with the modelled
    /// `ServiceUnavailableException` when it answers `false`, so no caller can grow the
    /// map without end.
    pub fn has_connector_capacity(&self) -> bool {
        self.connectors.len() < MAX_STORED_CONNECTORS
    }

    /// Whether the store can take another AS2 profile, i.e. is below
    /// [`MAX_STORED_PROFILES`].
    pub fn has_profile_capacity(&self) -> bool {
        self.profiles.len() < MAX_STORED_PROFILES
    }

    /// Whether the store can take another workflow, i.e. is below
    /// [`MAX_STORED_WORKFLOWS`].
    pub fn has_workflow_capacity(&self) -> bool {
        self.workflows.len() < MAX_STORED_WORKFLOWS
    }

    /// Whether the store can take another imported certificate, i.e. is below
    /// [`MAX_STORED_CERTIFICATES`].
    pub fn has_certificate_capacity(&self) -> bool {
        self.certificates.len() < MAX_STORED_CERTIFICATES
    }

    /// The ids of the servers that currently attach `workflow_id`, in `ServerId` order.
    ///
    /// A workflow attached to a server is in use by it; `DeleteWorkflow` refuses while
    /// this is non-empty, so the servers' `WorkflowDetails` cannot come to name a
    /// workflow that no longer exists.
    pub fn servers_using_workflow(&self, workflow_id: &str) -> Vec<&str> {
        self.servers
            .values()
            .filter(|server| server.references_workflow(workflow_id))
            .map(|server| server.server_id.as_str())
            .collect()
    }

    /// `UserCount` for every server that has users, in ONE pass over the user map.
    ///
    /// `ListServers` renders up to [`MAX_PAGE_SIZE`] servers per call; counting each
    /// one separately would cost `page x users`, a product no single bound covers.
    /// This makes the whole page cost `users + page`.
    pub fn user_counts(&self) -> BTreeMap<&str, i32> {
        let mut counts: BTreeMap<&str, i32> = BTreeMap::new();
        for (server_id, _) in self.users.keys() {
            let count = counts.entry(server_id.as_str()).or_insert(0);
            *count = count.saturating_add(1);
        }
        counts
    }

    /// Materialise every server state transition the clock has already made due.
    pub fn settle_servers(&mut self, now: Instant) {
        for server in self.servers.values_mut() {
            server.settle(now);
        }
    }

    /// The due instant for a server created at `now`, and the state it owes.
    pub fn server_start_due(now: Instant) -> (Instant, String) {
        (now + SERVER_START_SPAN, SERVER_STATE_ONLINE.to_string())
    }
}

/// `arn:aws:transfer:<region>:<account>:server/<ServerId>`.
pub fn server_arn(region: &str, server_id: &str) -> String {
    ::engine::emulator_arn(ARN_SERVICE, region, &format!("server/{server_id}"))
}

/// `arn:aws:transfer:<region>:<account>:server/*` — the resource an action on the
/// SERVER resource type is evaluated against when the request names no server instance.
///
/// `CreateServer` is such an action: the identifier is the service's to mint, so the
/// only server resource the request can be judged on is every server of the account in
/// the Region it was signed for. The wildcard is IAM's own, so a policy scoped to a
/// single server does not cover it while one scoped to the type does.
pub fn any_server_arn(region: &str) -> String {
    server_arn(region, "*")
}

/// `arn:aws:transfer:<region>:<account>:profile/<ProfileId>`.
pub fn profile_arn(region: &str, profile_id: &str) -> String {
    ::engine::emulator_arn(ARN_SERVICE, region, &format!("profile/{profile_id}"))
}

/// `arn:aws:transfer:<region>:<account>:user/<ServerId>/<UserName>`.
pub fn user_arn(region: &str, server_id: &str, user_name: &str) -> String {
    ::engine::emulator_arn(
        ARN_SERVICE,
        region,
        &format!("user/{server_id}/{user_name}"),
    )
}

/// `arn:aws:transfer:<region>:<account>:webapp/<WebAppId>`.
pub fn web_app_arn(region: &str, web_app_id: &str) -> String {
    ::engine::emulator_arn(ARN_SERVICE, region, &format!("webapp/{web_app_id}"))
}

/// The DNS label suffix of a web app's own endpoint host name.
///
/// `WebAppEndpoint` is documented as "the unique URL for your Transfer Family web app
/// ... the value that you use when you configure Origins on CloudFront", i.e. a host
/// name unique to the web app in its Region. The service publishes no host-name
/// grammar, so this suffix is the emulator's OWN: what matters, and what
/// [`web_app_endpoint`] guarantees, is that the name is derived from the web app's id
/// and Region and so is unique, stable across reads, and different for two web apps.
/// The emulator serves no web app UI, so nothing resolves this name.
const WEB_APP_ENDPOINT_SUFFIX: &str = "transfer-webapp";

/// `<WebAppId>.transfer-webapp.<region>.on.aws` — the web app's own endpoint host
/// name. Derived from the id and the Region, never stored input; see
/// [`WEB_APP_ENDPOINT_SUFFIX`].
pub fn web_app_endpoint(region: &str, web_app_id: &str) -> String {
    format!("{web_app_id}.{WEB_APP_ENDPOINT_SUFFIX}.{region}.on.aws")
}

/// The `AccessEndpoint` of a web app whose create named none.
///
/// `AccessEndpoint` is documented as "the URL that you provide to your users for them
/// to interact with the Transfer Family web app. You can specify a custom URL or use
/// the default value" — so when no custom URL is given the default is the `https://`
/// URL of the web app's own endpoint, the only URL the service has for the app.
pub fn default_access_endpoint(web_app_endpoint: &str) -> String {
    format!("https://{web_app_endpoint}")
}

/// The ARN of the IAM Identity Center application a `CreateWebApp` registers in the
/// Identity Center instance the caller named, or `None` when the caller named no
/// instance.
///
/// `DescribedIdentityCenterConfig.ApplicationArn` is documented as "the ARN for the
/// IAM Identity Center application: this value is set automatically when you create
/// your web app" — an application inside the given INSTANCE, which is why the
/// instance's own id is carried over from `InstanceArn` and only the `apl-` half is
/// minted. `None` when `InstanceArn` names no instance: there is then no instance to
/// register an application in, and nothing is invented. `InstanceArn`'s modelled
/// pattern (`^arn:[\w-]+:sso:::instance/(sso)?ins-[a-zA-Z0-9-.]{16}$`) is enforced
/// before a handler runs, so a present one always carries the `:instance/` segment
/// this reads.
pub fn identity_center_application_arn(
    instance_arn: &str,
    application_id: &str,
) -> Option<String> {
    let (_, instance) = instance_arn.split_once(":instance/")?;
    Some(::engine::emulator_arn(
        "sso",
        "",
        &format!("application/{instance}/{application_id}"),
    ))
}

/// `arn:aws:transfer:<region>:<account>:host-key/<ServerId>/<HostKeyId>`.
///
/// The model declares no `aws.api#arn` template for a host key, so this follows the
/// shape its templates give every OTHER resource a server owns —
/// `user/{ServerId}/{UserName}`, `agreement/{ServerId}/{AgreementId}` — with the
/// resource name the API uses for this one.
pub fn host_key_arn(region: &str, server_id: &str, host_key_id: &str) -> String {
    ::engine::emulator_arn(
        ARN_SERVICE,
        region,
        &format!("host-key/{server_id}/{host_key_id}"),
    )
}

/// `arn:aws:transfer:<region>:<account>:connector/<ConnectorId>`.
pub fn connector_arn(region: &str, connector_id: &str) -> String {
    ::engine::emulator_arn(ARN_SERVICE, region, &format!("connector/{connector_id}"))
}

/// `arn:aws:transfer:<region>:<account>:workflow/<WorkflowId>`.
pub fn workflow_arn(region: &str, workflow_id: &str) -> String {
    ::engine::emulator_arn(ARN_SERVICE, region, &format!("workflow/{workflow_id}"))
}

/// The region the caller signed this request for.
///
/// Read from the SigV4 credential scope
/// (`Credential=<akid>/<date>/<region>/<service>/aws4_request`), which is the only
/// place the region appears in a request to an emulated endpoint. Parsed with
/// `emulator_server::sigv4_credential_scope`, the framework's single source of
/// SigV4-scope parsing, so this agrees with how routing and telemetry read the same
/// header — including on rejecting a header that is not a well-formed
/// `AWS4-HMAC-SHA256` scope. Falls back to [`DEFAULT_REGION`] when there is no such
/// scope, so an unsigned request still gets a well-formed ARN.
pub fn request_region(parts: &::http::request::Parts) -> String {
    ::emulator_server::sigv4_credential_scope(&parts.headers)
        .map(|scope| scope.region)
        .unwrap_or_else(|| DEFAULT_REGION.to_string())
}

/// Encode the exclusive start key of the next page as an opaque `NextToken`.
///
/// Hex rather than the raw key so the token reads as opaque to a caller and carries
/// no character a JSON string or a URL has to escape; it decodes back to exactly one
/// key, so paging resumes deterministically.
pub fn encode_page_token(next_key: &str) -> String {
    ::hex::encode(next_key.as_bytes())
}

/// Decode a `NextToken` back to the exclusive start key.
///
/// `None` when the token was not produced by [`encode_page_token`] — the caller gets
/// `InvalidNextTokenException` rather than a silently restarted listing.
pub fn decode_page_token(token: &str) -> Option<String> {
    String::from_utf8(::hex::decode(token).ok()?).ok()
}

/// The fixed `Message` for a `NextToken` this service did not issue — the one
/// diagnostic every `@paginated` operation of this service answers for that condition,
/// whatever it was listing.
pub const INVALID_NEXT_TOKEN: &str = "Invalid nextToken";

/// The refusal payload for a `NextToken` [`decode_page_token`] could not read back.
///
/// Every `List*` handler builds its own modelled `InvalidNextTokenException` variant
/// from this ONE payload, so the token diagnostic cannot differ between two listings:
/// the text is fixed for the condition and carries neither the rejected token nor
/// anything else the request sent.
pub fn invalid_next_token() -> InvalidNextTokenException {
    InvalidNextTokenException {
        message: INVALID_NEXT_TOKEN.to_string(),
    }
}

/// The index range of one page of an ordered key list, and the next page's token key.
///
/// `all` must be sorted (it comes straight out of a `BTreeMap`). `after` is the
/// exclusive start key decoded from the caller's `NextToken`; `limit` is the page
/// bound. Returns the half-open index range to render and, only when more remain, the
/// exclusive start key of the next page. Indices rather than cloned keys, so a caller
/// can slice the records it already holds instead of looking each one up again.
pub fn page_bounds(
    all: &[&str],
    after: Option<&str>,
    limit: usize,
) -> (::std::ops::Range<usize>, Option<String>) {
    let start = match after {
        // A token naming a key that has since been deleted still resumes right after
        // where it pointed, because this is a position, not a lookup.
        Some(key) => all.partition_point(|candidate| *candidate <= key),
        None => 0,
    };
    let end = start.saturating_add(limit).min(all.len());
    // The token is the LAST key of this page, which is exactly the exclusive start of
    // the next one — and only when a next one exists.
    let next = if end < all.len() {
        all.get(start..end)
            .and_then(<[&str]>::last)
            .map(|key| (*key).to_string())
    } else {
        None
    };
    (start..end, next)
}

/// The index range of one page of a list whose order is the order its items were
/// SUBMITTED in, and the next page's token key.
///
/// [`page_bounds`] resumes by KEY, which needs the list sorted; a listing that reports
/// its items in submission order is not sorted, so its token carries the POSITION the
/// next page starts at instead. `None` for a token this codec did not issue — one that
/// is not a position, or one past the end of the list — so a caller is refused rather
/// than silently given a restarted listing.
pub fn page_bounds_by_position(
    count: usize,
    after: Option<&str>,
    limit: usize,
) -> Option<(::std::ops::Range<usize>, Option<String>)> {
    let start = match after {
        Some(position) => {
            let start = position.parse::<usize>().ok()?;
            if start > count {
                return None;
            }
            start
        }
        None => 0,
    };
    let end = start.saturating_add(limit).min(count);
    let next = if end < count {
        Some(end.to_string())
    } else {
        None
    };
    Some((start..end, next))
}

/// Turn a caller's `MaxResults` into a page bound.
///
/// The modelled `range 1..1000` is already enforced before a handler runs, so an
/// out-of-range value never reaches here; an absent one means "no caller-imposed
/// bound", which is [`MAX_PAGE_SIZE`].
pub fn page_limit(max_results: Option<i32>) -> usize {
    max_results
        .and_then(|n| usize::try_from(n).ok())
        .filter(|n| *n > 0)
        .unwrap_or(MAX_PAGE_SIZE)
        .min(MAX_PAGE_SIZE)
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsTransferTransferserviceState>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsTransferTransferserviceState>)
}

// ---------------------------------------------------------------------------
// What writing an access grant adds to the resource
// ---------------------------------------------------------------------------
//
// The access grant itself — [`Access`], the `accesses` map, `RESOURCE_TYPE_ACCESS`,
// `NOT_FOUND_ACCESS`/[`unknown_access`] and the directory-provider rule
// [`accesses_require_directory_provider`] every access operation refuses a
// non-directory server with — is declared above, beside the resources it sits with.
// `CreateAccess` is the first operation that WRITES a grant, so what a write needs and
// a delete did not is added here: the store bound, the already-exists payload for a
// grant the server already has, and the two lookups over one server's run of the map.

/// How many access grants ONE server holds before `CreateAccess` refuses with the
/// modelled `ServiceUnavailableException` instead of growing.
///
/// Chosen on the same basis as [`MAX_USERS_PER_SERVER`] — the emulator's own bound on
/// the store, not an account's quota. `DeleteAccess` releases one grant and
/// `DeleteServer` all of a server's.
pub const MAX_ACCESSES_PER_SERVER: usize = 1_000;

/// The fixed `Message` for a `CreateAccess` whose `ExternalId` the server already has
/// a grant for.
pub const ACCESS_ALREADY_EXISTS: &str = "Access already exists";

/// The already-exists payload for an `ExternalId` the named server already has a grant
/// for. Same construction as [`user_already_exists`]: the service's fixed text for the
/// condition, with the colliding identifier in `Resource` alone.
pub fn access_already_exists(external_id: &str) -> ResourceExistsException {
    ResourceExistsException {
        message: ACCESS_ALREADY_EXISTS.to_string(),
        resource: external_id.to_string(),
        resource_type: RESOURCE_TYPE_ACCESS.to_string(),
    }
}

impl ComAmazonawsTransferTransferserviceState {
    /// The access grants stored for `server_id`, in `ListAccesses` paging order.
    ///
    /// A RANGE scan over the contiguous run of the `(ServerId, ExternalId)` key, on
    /// the same basis as [`Self::users_of`].
    pub fn accesses_of<'a>(&'a self, server_id: &'a str) -> impl Iterator<Item = &'a Access> + 'a {
        use ::std::ops::Bound;
        let lower = (server_id.to_string(), String::new());
        self.accesses
            .range((Bound::Included(lower), Bound::Unbounded))
            .take_while(move |((sid, _), _)| sid == server_id)
            .map(|(_, access)| access)
    }

    /// Whether `server_id` can take another access grant, i.e. is below
    /// [`MAX_ACCESSES_PER_SERVER`].
    pub fn server_has_access_capacity(&self, server_id: &str) -> bool {
        self.accesses_of(server_id).count() < MAX_ACCESSES_PER_SERVER
    }
}

// ---------------------------------------------------------------------------
// The AS2 agreement resource
// ---------------------------------------------------------------------------
//
// An agreement is "a bilateral trading partner agreement, or partnership, between an
// Transfer Family server and an AS2 process", which "Transfer Family combines" out of
// "a server, local profile, partner profile, certificate, and other attributes" — so
// it is a RECORD that references a server and two profiles, all three of which must
// resolve before one can be written. The emulator speaks no AS2, so nothing here
// transfers a file; every member a reader reports is resolved once, at create time, on
// the same rule as every other resource in this module.
//
// The model's `AgreementResource` identifies an agreement by `(ServerId, AgreementId)`
// and gives it the ARN template `agreement/{ServerId}/{AgreementId}`, so the store is
// keyed by that pair and [`agreement_arn`] renders that template.

/// `ResourceType` for the AS2 agreement resource, addressed by its `AgreementId`.
/// Same convention as [`RESOURCE_TYPE_SERVER`]; the `Message` that travels with it is
/// [`NOT_FOUND_AGREEMENT`]'s.
pub const RESOURCE_TYPE_AGREEMENT: &str = "Agreement";

/// An `AgreementId` the addressed server holds no agreement for — never created, or
/// already deleted.
pub const NOT_FOUND_AGREEMENT: NotFoundResource = NotFoundResource {
    resource_type: RESOURCE_TYPE_AGREEMENT,
    message: "Unknown agreement",
};

/// The not-found payload for an `AgreementId` the addressed server holds no agreement
/// for. Same construction as [`unknown_profile`]: the service's fixed text for the
/// condition, with the unresolved identifier in `Resource` alone.
pub fn unknown_agreement(agreement_id: &str) -> ResourceNotFoundException {
    resource_not_found(&NOT_FOUND_AGREEMENT, agreement_id)
}

/// How many agreements ONE server holds before `CreateAgreement` refuses with the
/// modelled `ServiceUnavailableException` instead of growing.
///
/// Chosen on the same basis as [`MAX_ACCESSES_PER_SERVER`] — the emulator's own bound
/// on the store, not a claim about any account's Service Quota. `DeleteAgreement`
/// releases one.
pub const MAX_AGREEMENTS_PER_SERVER: usize = 1_000;

/// How many agreements the WHOLE store holds before `CreateAgreement` refuses, whatever
/// server each one is on.
///
/// The per-server bound above cannot bound the store on its own, because a caller that
/// keeps creating servers keeps a fresh per-server allowance with each one. This is the
/// total bound every other resource map gets from
/// [`MAX_STORED_SERVERS`]/[`MAX_STORED_PROFILES`]/[`MAX_STORED_CONNECTORS`], stated here
/// for the map addressed by a PAIR: past it a create is refused rather than growing the
/// store, and `DeleteAgreement` releases one, as does the `DeleteServer` that takes the
/// server's agreements with it.
pub const MAX_STORED_AGREEMENTS: usize = MAX_STORED_SERVERS * 10;

/// `Status`'s value for an agreement whose create named none.
///
/// `AgreementStatusType` is `[ACTIVE, INACTIVE]` and `DescribedAgreement.Status` is
/// "the current status of the agreement, either `ACTIVE` or `INACTIVE`" — so every
/// stored agreement has one of the two, and a create that names neither makes the
/// agreement the server can use straight away. A create that DOES name one keeps it.
pub const DEFAULT_AGREEMENT_STATUS: &str = "ACTIVE";

/// `PreserveFilename`'s documented default ("`DISABLED` (default value): when Transfer
/// Family saves the file, the filename is adjusted").
pub const DEFAULT_PRESERVE_FILENAME: &str = "DISABLED";

/// `EnforceMessageSigning`'s documented default ("`DISABLED` (default value): Transfer
/// Family accepts unsigned messages from your trading partner").
pub const DEFAULT_ENFORCE_MESSAGE_SIGNING: &str = "DISABLED";

/// One stored AS2 agreement, keyed by `(ServerId, AgreementId)`.
///
/// Every member a reader reports is RESOLVED here, at create time — the documented
/// `PreserveFilename` and `EnforceMessageSigning` defaults, the agreement's `Status`
/// and its own ARN — so no reader has to re-apply a default. The members
/// `ListedAgreement` reports (`{Arn, AgreementId, Description, Status, ServerId,
/// LocalProfileId, PartnerProfileId}`) are a subset of these, so both readers answer
/// out of this ONE record rather than a second map beside it.
#[derive(Debug, Clone, Default)]
pub struct Agreement {
    /// The minted `"a-" + 17 hex` identifier (`AgreementId` is `^a-([0-9a-f]{17})$`).
    pub agreement_id: String,
    /// The server the agreement is written on, the first half of its key.
    pub server_id: String,
    /// `arn:aws:transfer:<region>:<account>:agreement/<ServerId>/<AgreementId>`, as
    /// [`agreement_arn`] builds it from the model's own ARN template.
    pub arn: String,
    pub description: Option<String>,
    /// One of `AgreementStatusType`'s values, resolved at create time (see
    /// [`DEFAULT_AGREEMENT_STATUS`]).
    pub status: String,
    pub local_profile_id: String,
    pub partner_profile_id: String,
    /// The landing directory, when the agreement has one.
    ///
    /// A blank `BaseDirectory` names no directory — the member's pattern `^(|/.*)$`
    /// admits the empty string, the same reading [`agreement_base_directory`] gives it
    /// and `connector_url` gives a blank `Url` — so it is stored as absent, never as
    /// `""`. Absent exactly for an agreement configured with `CustomDirectories`
    /// instead, which the model documents as the alternative to it.
    pub base_directory: Option<String>,
    pub custom_directories: Option<CustomDirectoriesType>,
    pub access_role: String,
    /// One of `PreserveFilenameType`'s values, resolved at create time.
    pub preserve_filename: String,
    /// One of `EnforceMessageSigningType`'s values, resolved at create time.
    pub enforce_message_signing: String,
    pub tags: Vec<Tag>,
}

/// `arn:aws:transfer:<region>:<account>:agreement/<ServerId>/<AgreementId>` — the
/// model's own `aws.api#arn` template for `AgreementResource`.
pub fn agreement_arn(region: &str, server_id: &str, agreement_id: &str) -> String {
    ::engine::emulator_arn(
        ARN_SERVICE,
        region,
        &format!("agreement/{server_id}/{agreement_id}"),
    )
}

/// The landing directory a request supplied, when it names one.
///
/// A blank value names no directory: `BaseDirectory` targets `HomeDirectory`, whose
/// pattern `^(|/.*)$` admits the empty string — and admits NOTHING else that is not
/// rooted at `/`, so the empty string is the only value reaching a handler that names
/// no directory. It is therefore stored as not configured and reported absent. Both
/// the cross-member check and the stored record read the member through here, so the
/// two cannot disagree — the rule [`connector_url`] follows for a blank `Url`.
pub fn agreement_base_directory(base_directory: Option<&str>) -> Option<String> {
    base_directory
        .filter(|directory| !directory.is_empty())
        .map(str::to_string)
}

/// `CustomDirectoriesType` as a refusal that reports a request's custom directories
/// renders it: the shape's own name, then its five `@required` members
/// (`smithy_query(query="shape",shape="CustomDirectoriesType")`) in model order, each
/// under its lowerCamelCase label and carrying the value the request sent.
fn rendered_custom_directories(custom_directories: &CustomDirectoriesType) -> String {
    format!(
        "CustomDirectoriesType(failedFilesDirectory={}, mdnFilesDirectory={}, \
         payloadFilesDirectory={}, statusFilesDirectory={}, temporaryFilesDirectory={})",
        custom_directories.failed_files_directory,
        custom_directories.mdn_files_directory,
        custom_directories.payload_files_directory,
        custom_directories.status_files_directory,
        custom_directories.temporary_files_directory,
    )
}

/// The refusal text for a request that configures BOTH of an agreement's alternative
/// directory configurations: the condition reported is that two configurations are
/// configured at once, and the text reports WHICH — the `BaseDirectory` the request
/// carried and the whole `CustomDirectories` structure beside it — so a caller that
/// assembled the request out of merged configuration can see both sides and tell which
/// one to drop. Every value in it comes from the REQUEST; nothing is read from the
/// store.
fn both_directories_configured(
    base_directory: &str,
    custom_directories: &CustomDirectoriesType,
) -> String {
    format!(
        "Both BaseDirectory and CustomDirectories are configured. \
         BaseDirectory: {base_directory}, CustomDirectories: {}",
        rendered_custom_directories(custom_directories)
    )
}

/// The documented cross-member requirement an agreement's directory configuration
/// violates, worded as the refusal states it — `None` when it violates none.
///
/// `CreateAgreement` and `UpdateAgreement` both state it: "Specify either
/// `BaseDirectory` or `CustomDirectories`, but not both. Specifying both causes the
/// command to fail." Read as the sentence states it, that is an EITHER/OR over the two
/// members — one of them says where an agreement's AS2 files land, and the second half
/// of the sentence rules out naming both — so an agreement is written with exactly one
/// of them and a request naming both, or neither, is refused with the modelled
/// `InvalidRequestException`.
///
/// Each half is reported as the condition it is. Naming NEITHER member is worded as
/// [`one_of_required`] words it, the one reading this crate gives a documented either/or
/// that leaves a resource unconfigured — the same condition
/// [`ConnectorConfig::cross_member_violation`] reports on `As2Config` and `SftpConfig`.
/// Naming BOTH names them back ([`both_directories_configured`]): which two
/// configurations the service saw is what the caller has to act on, and both are in the
/// request, so this half reads the same wherever the check runs —
/// [`agreement_directory_update_violation`] reports it identically.
///
/// What counts as "specified" for `BaseDirectory` is EMPTINESS, not presence: the
/// member's pattern `^(|/.*)$` admits the empty string, which names no directory (see
/// [`agreement_base_directory`]), so a blank value cannot be the one configuration an
/// agreement is written with and is read here exactly as the stored record reads it —
/// the same test this crate applies to a blank `Url` on a connector and a blank
/// `Certificate` on a server. A blank `BaseDirectory` therefore leaves
/// `CustomDirectories` as the only configuration named, rather than making the request
/// name two.
pub fn agreement_directory_violation(
    base_directory: Option<&str>,
    custom_directories: Option<&CustomDirectoriesType>,
) -> Option<String> {
    match (
        agreement_base_directory(base_directory),
        custom_directories,
    ) {
        (Some(base_directory), Some(custom_directories)) => Some(both_directories_configured(
            &base_directory,
            custom_directories,
        )),
        (None, None) => Some(one_of_required("BaseDirectory", "CustomDirectories")),
        _ => None,
    }
}

/// The documented cross-member requirement an agreement UPDATE's directory
/// configuration violates, worded as the refusal states it — `None` when it violates
/// none.
///
/// `UpdateAgreement` states the same sentence the create does ("Specify either
/// `BaseDirectory` or `CustomDirectories`, but not both. Specifying both causes the
/// command to fail."), and then states what naming ONE of them does to the other: "If
/// you update an agreement from using base directory to custom directories, the base
/// directory is no longer used. Similarly, if you change from custom directories to a
/// base directory, the custom directories are no longer used." So an update can only
/// violate the "not both" half: a request naming NEITHER replaces neither, and leaves
/// the agreement with the one configuration it was written with — which is why
/// [`agreement_directory_violation`]'s other half, the one that reports that nothing
/// configures the resource, cannot apply to an update. The wording of the half that
/// does apply is [`both_directories_configured`]'s, so the create and the update report
/// the same condition the same way: the two conflicting configurations the request
/// carried, named back to the caller.
///
/// What counts as "specified" is EMPTINESS, exactly as at create time
/// ([`agreement_base_directory`]): a blank `BaseDirectory` names no directory, so it
/// neither replaces the agreement's configuration nor makes the request name two.
pub fn agreement_directory_update_violation(
    base_directory: Option<&str>,
    custom_directories: Option<&CustomDirectoriesType>,
) -> Option<String> {
    let base_directory = agreement_base_directory(base_directory)?;
    custom_directories
        .map(|custom_directories| both_directories_configured(&base_directory, custom_directories))
}

impl ComAmazonawsTransferTransferserviceState {
    /// The agreements stored for `server_id`, in `ListAgreements` paging order.
    ///
    /// A RANGE scan over the contiguous run of the `(ServerId, AgreementId)` key, on
    /// the same basis as [`Self::users_of`].
    pub fn agreements_of<'a>(
        &'a self,
        server_id: &'a str,
    ) -> impl Iterator<Item = &'a Agreement> + 'a {
        use ::std::ops::Bound;
        let lower = (server_id.to_string(), String::new());
        self.agreements
            .range((Bound::Included(lower), Bound::Unbounded))
            .take_while(move |((sid, _), _)| sid == server_id)
            .map(|(_, agreement)| agreement)
    }

    /// Whether `server_id` can take another agreement, i.e. is below
    /// [`MAX_AGREEMENTS_PER_SERVER`].
    pub fn server_has_agreement_capacity(&self, server_id: &str) -> bool {
        self.agreements_of(server_id).count() < MAX_AGREEMENTS_PER_SERVER
    }

    /// Whether the store can take another agreement at all, i.e. is below
    /// [`MAX_STORED_AGREEMENTS`].
    ///
    /// The TOTAL bound, which [`Self::server_has_agreement_capacity`] cannot stand in
    /// for: a per-server allowance is renewed by every new server, so the per-server
    /// bound alone leaves the store unbounded.
    pub fn has_agreement_capacity(&self) -> bool {
        self.agreements.len() < MAX_STORED_AGREEMENTS
    }
}

// ---------------------------------------------------------------------------
// The imported-certificate plane
// ---------------------------------------------------------------------------
//
// A certificate is an ACCOUNT-level resource (its id is the only identifier any
// certificate operation takes), so the records live in one map keyed by
// `CertificateId`, like [`ComAmazonawsTransferTransferserviceState::profiles`] — the
// AS2 profiles whose `CertificateIds` name them.
//
// `Status` is DERIVED at read time ([`Certificate::status`]) rather than stored,
// because it is a function of the CLOCK and the certificate's validity window: a
// certificate whose `InactiveDate` has passed must not still read as `ACTIVE`, and no
// caller has to be made to touch the record for the clock's own effect to show.

/// `CertificateStatusType`'s `ACTIVE`: the clock is inside the certificate's validity
/// window. (`smithy_query(query="shape",shape="CertificateStatusType")` reports the
/// whole value set `[ACTIVE, PENDING_ROTATION, INACTIVE]`.)
pub const CERTIFICATE_STATUS_ACTIVE: &str = "ACTIVE";

/// `CertificateStatusType`'s `PENDING_ROTATION`: the certificate is not active yet —
/// its modelled doc is "PENDING_ROTATION means that this certificate will replace the
/// current certificate when it expires".
pub const CERTIFICATE_STATUS_PENDING_ROTATION: &str = "PENDING_ROTATION";

/// `CertificateStatusType`'s `INACTIVE`: the certificate's validity window has closed.
pub const CERTIFICATE_STATUS_INACTIVE: &str = "INACTIVE";

/// `CertificateType`'s `CERTIFICATE` — the type of a certificate imported WITHOUT a
/// private key. (`smithy_query(query="shape",shape="CertificateType")` reports
/// `[CERTIFICATE, CERTIFICATE_WITH_PRIVATE_KEY]`.)
pub const CERTIFICATE_TYPE_CERTIFICATE: &str = "CERTIFICATE";

/// `CertificateType`'s `CERTIFICATE_WITH_PRIVATE_KEY` — "If a private key has been
/// specified for the certificate, its type is `CERTIFICATE_WITH_PRIVATE_KEY`".
pub const CERTIFICATE_TYPE_WITH_PRIVATE_KEY: &str = "CERTIFICATE_WITH_PRIVATE_KEY";

/// `arn:aws:transfer:<region>:<account>:certificate/<CertificateId>`.
///
/// The same shape the model's `aws.api#arn` templates give every other account-level
/// resource of this service (`profile/{ProfileId}`, `connector/{ConnectorId}`), with
/// the resource name the API uses for this one.
pub fn certificate_arn(region: &str, certificate_id: &str) -> String {
    ::engine::emulator_arn(ARN_SERVICE, region, &format!("certificate/{certificate_id}"))
}

/// `ResourceType` for the imported-certificate resource, addressed by its
/// `CertificateId`. Same convention as [`RESOURCE_TYPE_SERVER`].
pub const RESOURCE_TYPE_CERTIFICATE: &str = "Certificate";

/// A `CertificateId` nothing in the store answers to — never imported, or already
/// deleted. Its own entry in the not-found table, like every other kind.
pub const NOT_FOUND_CERTIFICATE: NotFoundResource = NotFoundResource {
    resource_type: RESOURCE_TYPE_CERTIFICATE,
    message: "Unknown certificate",
};

/// The not-found payload for a `CertificateId` the store does not hold. Same
/// construction as [`unknown_profile`]: the service's fixed text for the condition,
/// with the unresolved identifier in `Resource` alone.
pub fn unknown_certificate(certificate_id: &str) -> ResourceNotFoundException {
    resource_not_found(&NOT_FOUND_CERTIFICATE, certificate_id)
}

/// How many certificates the store holds before `ImportCertificate` refuses with the
/// modelled `ServiceUnavailableException` instead of growing.
///
/// Chosen on the same basis as [`MAX_STORED_PROFILES`] — the emulator's own bound on
/// the store, not a claim about any account's Service Quota — and `DeleteCertificate`
/// releases one. Each stored record is itself bounded by the modelled lengths of the
/// members it holds (`Certificate` `len 1..16384`, `CertificateChain`
/// `len 1..2097152`, `Description` `len 1..200`, `Tags` `len 1..50`), which the
/// framework's boundary validator enforces before a handler runs.
pub const MAX_STORED_CERTIFICATES: usize = 200;

/// What an imported certificate itself states: the values a handler must read OUT of
/// the certificate rather than invent for it.
#[derive(Debug, Clone)]
pub struct ParsedCertificate {
    /// The X.509 `serialNumber`, rendered by [`certificate_serial`].
    pub serial: String,
    /// The `notBefore` bound of the certificate's own validity period.
    pub not_before: SystemTime,
    /// Its `notAfter` bound.
    pub not_after: SystemTime,
}

/// A certificate's serial number as the model lets this service report it: the bytes
/// of the X.509 serial the issuing CA assigned, each as exactly two lowercase hex
/// digits, with no separator.
///
/// `CertSerial` is `len 0..48`, pattern `^[0-9a-fA-F{}:?]*$`
/// (`smithy_query(query="shape",shape="CertSerial")`). RFC 5280 section 4.1.2.2 bounds
/// a serial at 20 octets, which is 40 digits unseparated and 59 colon-separated — so
/// the unseparated rendering is the only one that fits the modelled length for every
/// serial a conforming CA can assign.
pub fn certificate_serial(bytes: &[u8]) -> String {
    ::hex::encode(bytes)
}

/// The wall-clock instant a modelled `CertDate` names, or `None` when the value is
/// outside the range this host can hold as an instant.
///
/// `CertDate` is a plain `timestamp` with no modelled range, so a caller can send a
/// value no `SystemTime` can represent; the conversion is fallible for exactly that
/// case and a write refuses it rather than storing a different date
/// ([`unholdable_cert_date`]).
pub fn cert_date_instant(at: ::aws_smithy_types::DateTime) -> Option<SystemTime> {
    SystemTime::try_from(at).ok()
}

/// The refusal for a `CertDate` member whose value this service cannot hold as an
/// instant: the modelled `InvalidRequestException` ("thrown when the client submits a
/// malformed request"), under a fixed text naming the MEMBER and no value from the
/// request.
pub fn unholdable_cert_date(member: &str) -> InvalidRequestException {
    InvalidRequestException {
        message: format!("{member} is not a date this service can represent"),
    }
}

/// The refusal for a `Certificate` body that is not a PEM-encoded X.509 certificate.
///
/// The import reports the certificate's own serial and validity period
/// ([`parse_certificate`]), so material it cannot read has none to report and may not
/// be stored with invented ones. The modelled `InvalidRequestException` is the refusal
/// for a request the service will not act on, and the text names the member and the
/// encoding it must carry, exactly as this service's other material refusal does.
pub fn unreadable_certificate_body() -> InvalidRequestException {
    InvalidRequestException {
        message: "Certificate must be a PEM-encoded X.509 certificate".to_string(),
    }
}

/// The pre- and post-encapsulation boundaries of a PEM-framed X.509 certificate
/// (RFC 7468 section 5.1).
const PEM_CERTIFICATE_BEGIN: &str = "-----BEGIN CERTIFICATE-----";
const PEM_CERTIFICATE_END: &str = "-----END CERTIFICATE-----";

/// The bounds of the PEM-framed certificate message(s) `text` carries — from the
/// first pre-encapsulation boundary to the end of the `last` post-encapsulation
/// boundary when `last` is true, or of the FIRST one when it is false — or `None` when
/// `text` frames no certificate at all.
///
/// RFC 7468 section 5.2 allows explanatory text around an encapsulated message, and
/// `Certificate`'s doc allows a body carrying the certificate AND its chain ("you can
/// provide both the certificate and its chain in this parameter"), so what a
/// certificate member CARRIES is the message span and what surrounds it — a line
/// ending, a subject line a tool printed — is framing rather than material. Only the
/// BOUNDARIES are located here, by the labels the RFC defines; every byte between them
/// is decoded by the X.509 parser ([`parse_certificate`]), never here.
fn certificate_message_bounds(text: &str, last: bool) -> Option<(usize, usize)> {
    let start = text.find(PEM_CERTIFICATE_BEGIN)?;
    let message = text.get(start..)?;
    let closed = if last {
        message.rfind(PEM_CERTIFICATE_END)?
    } else {
        message.find(PEM_CERTIFICATE_END)?
    } + PEM_CERTIFICATE_END.len();
    Some((start, start + closed))
}

/// The material a `Certificate` member carries, as `(the certificate, the chain it
/// also carried)`.
///
/// A certificate record holds the certificate and its chain SEPARATELY — the model
/// reports them as two members — while the `Certificate` member may carry both
/// ("without needing to use the `CertificateChain` parameter"). So the certificate is
/// the LEADING message of that member and the messages after it are the chain the body
/// carried. `None` when the body frames no certificate at all.
pub fn split_certificate_material(body: &str) -> Option<(&str, Option<&str>)> {
    let (start, end) = certificate_message_bounds(body, false)?;
    let certificate = body.get(start..end)?;
    let after = body.get(end..)?;
    let chain = certificate_message_bounds(after, true)
        .and_then(|(start, end)| after.get(start..end));
    Some((certificate, chain))
}

/// The chain a `CertificateChain` member carries, as the record holds it: the message
/// span it frames, on the same rule as the certificate's own material, and the text as
/// given when it frames no certificate message at all — nothing this service reports
/// is read out of a chain, so a chain it cannot recognise is kept rather than refused.
pub fn certificate_chain_material(text: &str) -> &str {
    certificate_message_bounds(text, true)
        .and_then(|(start, end)| text.get(start..end))
        .unwrap_or(text)
}

/// What the PEM certificate message `message` carries, or `None` when it does not
/// decode as an X.509 certificate.
///
/// `message` is the certificate message [`split_certificate_material`] separated out of
/// a `Certificate` member. The PEM framing and the DER inside it are decoded by the
/// `x509-cert` parser rather than by anything written here, so the serial and validity
/// window a reader reports are the certificate's OWN values, and material this emulator
/// cannot read is refused by the import instead of stored with invented ones.
pub fn parse_certificate(message: &str) -> Option<ParsedCertificate> {
    use ::x509_cert::der::DecodePem;

    let certificate = ::x509_cert::Certificate::from_pem(message.as_bytes()).ok()?;
    let validity = &certificate.tbs_certificate.validity;
    Some(ParsedCertificate {
        serial: certificate_serial(certificate.tbs_certificate.serial_number.as_bytes()),
        not_before: validity.not_before.to_system_time(),
        not_after: validity.not_after.to_system_time(),
    })
}

/// One imported certificate, keyed by `CertificateId`.
///
/// Holds what a certificate READER reports about the record: the minted id, its ARN,
/// how the certificate is used, its type, the material the import carried, the serial
/// and validity period read OUT of that material, the window its status follows, the
/// caller's description and its tag set.
///
/// The `PrivateKey` an import may carry is NOT held: no operation of this service
/// reports it and none uses it, and it is the one member of the import that is a
/// secret — so what it decides, [`Self::certificate_type`], is resolved at import time
/// and the key itself is not persisted.
#[derive(Debug, Clone, Default)]
pub struct Certificate {
    /// The minted `cert-` + 17 hex-digit identifier (`^cert-([0-9a-f]{17})$`).
    pub certificate_id: String,
    /// `arn:aws:transfer:<region>:<account>:certificate/<CertificateId>`.
    pub arn: String,
    /// `CertificateUsageType`: `SIGNING`, `ENCRYPTION` or `TLS`.
    pub usage: String,
    /// `CertificateType`: [`CERTIFICATE_TYPE_CERTIFICATE`] or
    /// [`CERTIFICATE_TYPE_WITH_PRIVATE_KEY`].
    pub certificate_type: String,
    /// The PEM certificate message the import carried, boundaries included: the
    /// material itself, without the line ending or explanatory text the request framed
    /// it with. `DescribeCertificate` serves it back as stored, never a re-encoding of
    /// it.
    pub body: String,
    /// The chain the import carried, kept the same way — in `CertificateChain`, or
    /// after the certificate in the `Certificate` member; absent when it carried none.
    pub chain: Option<String>,
    /// The certificate's own `serialNumber`, as [`certificate_serial`] renders it.
    pub serial: Option<String>,
    /// `NotBeforeDate`: "the earliest date that the certificate is valid" — the
    /// certificate's own `notBefore`, read out of the material at import time.
    pub not_before: Option<SystemTime>,
    /// `NotAfterDate`: "the final date that the certificate is valid" — its own
    /// `notAfter`, read the same way.
    pub not_after: Option<SystemTime>,
    /// `ActiveDate`: when the certificate becomes active, resolved at import time from
    /// the caller's value or the certificate's own `NotBeforeDate`. `None` leaves the
    /// window open at the start, so the certificate is active from the first read.
    pub active_date: Option<SystemTime>,
    /// `InactiveDate`: when the certificate becomes inactive, resolved the same way
    /// from `NotAfterDate`. `None` leaves the window open at the end.
    pub inactive_date: Option<SystemTime>,
    /// The caller's `Description`, absent when the import named none.
    pub description: Option<String>,
    /// The tag set the import carried, which `TagResource`/`UntagResource` then
    /// rewrite in place like every other taggable resource's.
    pub tags: Vec<Tag>,
}

impl Certificate {
    /// `Status` at instant `now`: DERIVED from the validity window, never stored.
    ///
    /// `Status`' modelled doc is "If you set values for these parameters
    /// [`ActiveDate`/`InactiveDate`], those values are used to determine whether the
    /// certificate has a status of ACTIVE or INACTIVE", and `PENDING_ROTATION` "means
    /// that this certificate will replace the current certificate when it expires" —
    /// so the certificate is `ACTIVE` exactly while the clock is inside the half-open
    /// window `[ActiveDate, InactiveDate)`, `PENDING_ROTATION` before it opens (the
    /// certificate is imported but not in use yet) and `INACTIVE` once it has closed.
    /// An absent bound leaves that end of the window open.
    pub fn status(&self, now: SystemTime) -> &'static str {
        if self.active_date.is_some_and(|active_from| now < active_from) {
            CERTIFICATE_STATUS_PENDING_ROTATION
        } else if self.inactive_date.is_some_and(|inactive_from| now >= inactive_from) {
            CERTIFICATE_STATUS_INACTIVE
        } else {
            CERTIFICATE_STATUS_ACTIVE
        }
    }

    /// Apply the validity window a write states, under the ONE documented resolution
    /// rule both writes of this resource follow.
    ///
    /// `ActiveDate` is "an optional date that specifies when the certificate becomes
    /// active. If you do not specify a value, `ActiveDate` takes the same value as
    /// `NotBeforeDate`, which is specified by the CA", and `InactiveDate` the same from
    /// `NotAfterDate` — the model states that for `ImportCertificate`'s members AND for
    /// `UpdateCertificate`'s, so a write that names a bound sets it and a write that
    /// names none leaves that end of the window at the certificate's own date. A
    /// certificate whose material states no date leaves that end open, which
    /// [`Self::status`] reads as unbounded.
    pub fn apply_window(&mut self, active_date: Option<SystemTime>, inactive_date: Option<SystemTime>) {
        self.active_date = active_date.or(self.not_before);
        self.inactive_date = inactive_date.or(self.not_after);
    }
}

// ---------------------------------------------------------------------------
// The tag plane
// ---------------------------------------------------------------------------
//
// A tag set is not a resource of its own: every taggable resource already stores its
// own `tags`, written by the create (or import) that made it and reported by that
// resource's own reader. So `TagResource`/`UntagResource`/`ListTagsForResource` add no
// map here — they RESOLVE a resource by the ARN the caller sends and then read or
// rewrite that resource's existing set, which is why a tag written through
// `TagResource` is immediately visible to `DescribeServer`, `DescribeUser`,
// `DescribeHostKey`, `DescribeWebApp`, `DescribeProfile`, `DescribeConnector` and
// `DescribeWorkflow` without any of them changing.

/// How many tags ONE resource carries before a tag write is refused instead of
/// growing the set.
///
/// The modelled `Tags` list is `len 1..50`
/// (`smithy_query(query="shape",shape="Tags")`), and `Tags` is the shape EVERY reader
/// reports a resource's tag set through — `ListTagsForResource`'s output member and
/// each `Describe*`'s — so a set past 50 could not be reported without violating the
/// model. That makes 50 the bound here, and it is also the AWS-wide limit of 50 tags
/// per resource. `UntagResource` releases capacity again.
pub const MAX_TAGS_PER_RESOURCE: usize = 50;

/// `ResourceType` for a resource addressed by an ARN whose KIND this service does not
/// hold.
///
/// A tag write resolves its target by ARN, and the kind is decoded from the ARN itself
/// ([`tagged_resource`]); when the ARN names no kind this service holds — another
/// service's ARN, or a Transfer resource kind nothing here stores — there is no kind to
/// name for it. This is the emulator's own token for that case, on the same capitalized
/// convention as [`RESOURCE_TYPE_SERVER`]; the ARN the caller sent travels in
/// `Resource`, never in the message.
pub const RESOURCE_TYPE_RESOURCE: &str = "Resource";

/// An ARN no stored resource answers to, and whose resource KIND this service does not
/// hold either. Its own entry in the not-found table, like every other kind; which kind
/// an ARN names, and the entry that kind is reported with, is
/// [`tagged_resource::unknown_tagged_resource`].
pub const NOT_FOUND_TAGGED_RESOURCE: NotFoundResource = NotFoundResource {
    resource_type: RESOURCE_TYPE_RESOURCE,
    message: "Unknown resource",
};

/// The refusal for a tag write that would push a resource past
/// [`MAX_TAGS_PER_RESOURCE`]: a FIXED text for the condition, with the bound it
/// states and no request data.
pub fn too_many_tags() -> InvalidRequestException {
    InvalidRequestException {
        message: format!("A resource cannot carry more than {MAX_TAGS_PER_RESOURCE} tags"),
    }
}

/// The tag set the merge of `incoming` into `tags` would have, as a COUNT, without
/// touching either.
///
/// A tag key identifies one tag on a resource, so a key already present is replaced
/// rather than added: only the keys that are new to `tags` grow the set, and a key
/// repeated inside `incoming` grows it once. A write consults this BEFORE mutating,
/// so a refused write leaves the set exactly as it was.
pub fn merged_tag_count(tags: &[Tag], incoming: &[Tag]) -> usize {
    let present: ::std::collections::BTreeSet<&str> =
        tags.iter().map(|tag| tag.key.as_str()).collect();
    let added: ::std::collections::BTreeSet<&str> = incoming
        .iter()
        .map(|tag| tag.key.as_str())
        .filter(|key| !present.contains(key))
        .collect();
    present.len() + added.len()
}

/// Merge `incoming` into `tags`: a key the resource already carries takes the
/// incoming value IN PLACE, a key it does not carry is appended.
///
/// A tag key is unique per resource (tags are a key-value map, not a multiset), so
/// tagging with a key that is already there is an update and not a second tag — which
/// is also why a key repeated within one request leaves the LAST value sent.
pub fn merge_tags(tags: &mut Vec<Tag>, incoming: Vec<Tag>) {
    for tag in incoming {
        match tags.iter_mut().find(|present| present.key == tag.key) {
            Some(present) => present.value = tag.value,
            None => tags.push(tag),
        }
    }
}

/// Detach every tag whose key is in `keys`, keeping the rest in their stored order.
///
/// A key the resource does not carry removes nothing and is not an error: the request
/// asks for those keys to be absent, and after it they are.
pub fn remove_tags(tags: &mut Vec<Tag>, keys: &[String]) {
    tags.retain(|tag| !keys.contains(&tag.key));
}

impl ComAmazonawsTransferTransferserviceState {
    /// Every stored resource that carries a tag set, as `(ARN, tags)` pairs.
    ///
    /// The ONE list of what is taggable in this service, and the order
    /// [`Self::tags_mut`] resolves in. A resource kind becomes taggable by storing an
    /// `arn` and a `tags` of its own and being added to both.
    fn taggable(&self) -> impl Iterator<Item = (&str, &[Tag])> {
        let servers = self
            .servers
            .values()
            .map(|r| (r.arn.as_str(), r.tags.as_slice()));
        let users = self
            .users
            .values()
            .map(|r| (r.arn.as_str(), r.tags.as_slice()));
        let host_keys = self
            .host_keys
            .values()
            .map(|r| (r.arn.as_str(), r.tags.as_slice()));
        let web_apps = self
            .web_apps
            .values()
            .map(|r| (r.arn.as_str(), r.tags.as_slice()));
        let profiles = self
            .profiles
            .values()
            .map(|r| (r.arn.as_str(), r.tags.as_slice()));
        let connectors = self
            .connectors
            .values()
            .map(|r| (r.arn.as_str(), r.tags.as_slice()));
        let workflows = self
            .workflows
            .values()
            .map(|r| (r.arn.as_str(), r.tags.as_slice()));
        // `AgreementResource` carries `aws.api#taggable{property: Tags}` in the model,
        // and a stored agreement has an `arn` and a `tags` of its own, so it is
        // taggable on the same terms as every kind above.
        let agreements = self
            .agreements
            .values()
            .map(|r| (r.arn.as_str(), r.tags.as_slice()));
        // An imported certificate takes `Tags` at import, reports them through
        // `DescribeCertificate` and has an `arn` of its own, so it is taggable on the
        // same terms as every kind above.
        let certificates = self
            .certificates
            .values()
            .map(|r| (r.arn.as_str(), r.tags.as_slice()));
        servers
            .chain(users)
            .chain(host_keys)
            .chain(web_apps)
            .chain(profiles)
            .chain(connectors)
            .chain(workflows)
            .chain(agreements)
            .chain(certificates)
    }

    /// The tag set of the resource `arn` identifies, or `None` when no stored resource
    /// answers to that ARN.
    ///
    /// Matched against each resource's OWN stored ARN, so the region and account id in
    /// the ARN are part of the identity: an ARN that differs from the one the create
    /// returned names no resource, rather than resolving by id alone.
    pub fn tags_of(&self, arn: &str) -> Option<&[Tag]> {
        self.taggable()
            .find(|(stored, _)| *stored == arn)
            .map(|(_, tags)| tags)
    }

    /// The tag set of the resource `arn` identifies, for a write.
    ///
    /// The same resolution as [`Self::tags_of`] over the same maps in the same order;
    /// an ARN is unique across the store, so at most one record answers to it.
    pub fn tags_mut(&mut self, arn: &str) -> Option<&mut Vec<Tag>> {
        if let Some(found) = self.servers.values_mut().find(|r| r.arn == arn) {
            return Some(&mut found.tags);
        }
        if let Some(found) = self.users.values_mut().find(|r| r.arn == arn) {
            return Some(&mut found.tags);
        }
        if let Some(found) = self.host_keys.values_mut().find(|r| r.arn == arn) {
            return Some(&mut found.tags);
        }
        if let Some(found) = self.web_apps.values_mut().find(|r| r.arn == arn) {
            return Some(&mut found.tags);
        }
        if let Some(found) = self.profiles.values_mut().find(|r| r.arn == arn) {
            return Some(&mut found.tags);
        }
        if let Some(found) = self.connectors.values_mut().find(|r| r.arn == arn) {
            return Some(&mut found.tags);
        }
        if let Some(found) = self.workflows.values_mut().find(|r| r.arn == arn) {
            return Some(&mut found.tags);
        }
        if let Some(found) = self.agreements.values_mut().find(|r| r.arn == arn) {
            return Some(&mut found.tags);
        }
        if let Some(found) = self.certificates.values_mut().find(|r| r.arn == arn) {
            return Some(&mut found.tags);
        }
        None
    }
}
