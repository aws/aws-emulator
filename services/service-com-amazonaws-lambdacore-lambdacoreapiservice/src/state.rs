// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-lambdacore-lambdacoreapiservice service.
//!
//! The network connector is this service's only resource:
//! `CreateNetworkConnector` mints one, `GetNetworkConnector` /
//! `ListNetworkConnectors` / `ListNetworkConnectorVersions` report it,
//! `UpdateNetworkConnector` appends a version to it and
//! `DeleteNetworkConnector` removes it (or one of its historical versions).
//! The records below are the ONE copy of a connector every one of those
//! handlers reads and writes, plus the two projections the model asks for
//! (`NetworkConnectorSummary` and `NetworkConnectorVersionSummary`).

use std::collections::{BTreeMap, HashMap};
use std::time::{Duration, SystemTime};

pub mod identifier;
pub mod network;

use crate::ops::types::{
    InvalidParameterValueException, NetworkConnectorConfiguration, NetworkConnectorSummary,
    NetworkConnectorVersionSummary, ResourceConflictException, ResourceNotFoundException,
    ServiceException,
};

/// The region an ARN is built for when the request carries no SigV4 credential
/// scope to read one from.
pub const DEFAULT_REGION: &str = "us-east-1";

/// The only value `NetworkConnectorType` declares: "`VPC_EGRESS` provides VPC
/// egress connectivity", which is what a `VpcEgressConfiguration` configures.
pub const CONNECTOR_TYPE_VPC_EGRESS: &str = "VPC_EGRESS";

/// The `NetworkConnectorType` values whose connector provisions elastic network
/// interfaces in the caller's VPC, and so cannot be created without the
/// `OperatorRole` Lambda assumes to manage them.
///
/// `NetworkConnectorType` declares exactly one value
/// ([`CONNECTOR_TYPE_VPC_EGRESS`], "provides VPC egress connectivity"), and a
/// connector of that type is the one that owns interfaces in a caller's VPC.
pub const CONNECTOR_TYPES_REQUIRING_OPERATOR_ROLE: &[&str] = &[CONNECTOR_TYPE_VPC_EGRESS];

/// Whether a connector of this type needs an `OperatorRole` to be created.
pub fn operator_role_is_required(connector_type: &str) -> bool {
    CONNECTOR_TYPES_REQUIRING_OPERATOR_ROLE.contains(&connector_type)
}

/// `NetworkConnectorState$PENDING` — "Being created, resources provisioning".
pub const STATE_PENDING: &str = "PENDING";
/// `NetworkConnectorState$ACTIVE` — "Active and ready for use".
pub const STATE_ACTIVE: &str = "ACTIVE";
/// `NetworkConnectorState$DELETING` — "Being deleted, cleaning up resources".
pub const STATE_DELETING: &str = "DELETING";
/// `NetworkConnectorState$FAILED` — "Creation or update failed".
pub const STATE_FAILED: &str = "FAILED";

/// The `StateReason` a connector and its first version carry while their initial
/// creation is the event that put them in their current state.
///
/// `StateReason` is "a human-readable explanation of the current state", and the recorded
/// AWS answer for a freshly created connector carries one, so a reported `State` comes
/// with the reason the resource reached it rather than being reserved for a failure. No
/// documented rule computes the text, so the emulator uses the short noun phrase naming
/// the event, and keeps it until another event sets a new state: a connector is `ACTIVE`
/// because the creation that started it finished.
pub const STATE_REASON_INITIAL_CREATION: &str = "Initial creation";

/// The `StateReason` a version appended by `UpdateNetworkConnector` carries: "each
/// configuration update creates a new immutable version", so that update is the event
/// that put the new version in its state.
pub const STATE_REASON_CONFIGURATION_UPDATE: &str = "Configuration update";

/// The `StateReason` a connector carries once `DeleteNetworkConnector` moved it to
/// `DELETING`: the deletion is the event that put it there.
pub const STATE_REASON_DELETION_REQUESTED: &str = "Deletion requested";

/// `NetworkConnectorLastUpdateStatus$InProgress`.
pub const LAST_UPDATE_IN_PROGRESS: &str = "InProgress";
/// `NetworkConnectorLastUpdateStatus$Successful`.
pub const LAST_UPDATE_SUCCESSFUL: &str = "Successful";

/// The largest page `ListNetworkConnectors` serves, and the page it serves when
/// the request names no `MaxItems`.
///
/// `MaxItems` is modelled `@range(min: 1, max: 100)`, but the answer it fills —
/// `NetworkConnectorsList` — is modelled `@length(min: 0, max: 50)`, so 50 is
/// the most summaries one response can carry.
pub const CONNECTOR_PAGE_LIMIT: i32 = 50;

/// The largest page `ListNetworkConnectorVersions` serves, and the page it
/// serves when the request names no `MaxItems`: `NetworkConnectorVersionsList`
/// is modelled `@length(min: 0, max: 100)`.
pub const VERSION_PAGE_LIMIT: i32 = 100;

/// The ceiling `MaxHundredListItems` declares (`@range(min: 1, max: 100)`).
pub const MAX_ITEMS_CEILING: i32 = 100;

/// How many connectors the store holds before `CreateNetworkConnector` refuses
/// with the modelled `NetworkConnectorLimitExceededException` ("the account has
/// reached the maximum number of network connectors allowed").
///
/// AWS publishes an adjustable per-account quota and this emulator models
/// neither Service Quotas nor an account's adjustments, so this is the
/// emulator's OWN bound on the store rather than a claim about any account's
/// limit. It is far above any workflow an emulator serves, and it is what stops
/// the state growing without end.
pub const CONNECTOR_QUOTA: usize = 1_000;

/// How many versions one connector holds before `UpdateNetworkConnector`
/// refuses with the modelled `NetworkConnectorLimitExceededException` ("for
/// UpdateNetworkConnector, the connector has reached the maximum number of
/// versions allowed", which `UpdateNetworkConnector`'s own documentation says
/// holds "until you delete older versions").
///
/// Grounded in the model: `NetworkConnectorVersionsList` is
/// `@length(min: 0, max: 100)`, so 100 versions is the most the versions list
/// can describe.
pub const MAX_VERSIONS_PER_CONNECTOR: usize = 100;

/// How many `ClientToken`s the store remembers at once, across every connector.
///
/// A client token is caller-supplied input, so the map that remembers one needs
/// a bound of its own: nothing else caps how many distinct tokens a caller may
/// present, and a token is released early only when the connector it named is
/// deleted. Past this count the oldest remembered token is evicted (see
/// [`ComAmazonawsLambdacoreLambdacoreapiserviceState::remember_token`]). The
/// value is the emulator's own choice, set well above the retry depth any
/// workflow needs.
pub const MAX_IDEMPOTENCY_TOKENS: usize = 1_000;

/// How long the asynchronous work a call started is still in flight before the
/// clock owes its outcome: provisioning a new connector (`PENDING` -> `ACTIVE`),
/// an update (`InProgress` -> `Successful`) and a deletion (`DELETING` -> the
/// record is forgotten).
///
/// `CreateNetworkConnector` documents that provisioning "typically takes up to
/// 10 minutes"; the emulator chooses a span instead, and it is deliberately NOT
/// zero so a caller acting straight after the call still observes the transient
/// state. The instant it falls due is stored on the record and materialised by
/// whichever later call reads it — nothing sleeps.
pub const LIFECYCLE_TRANSITION: Duration = Duration::from_millis(500);

/// What the clock owes a connector once its transition falls due.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PendingChange {
    /// Provisioning finishes and the connector carries this state from then on.
    State(String),
    /// Deletion finishes: the record is forgotten, so a later read answers
    /// `ResourceNotFoundException`.
    Removal,
}

/// A change the clock owes: the instant it falls due, and what it does.
#[derive(Debug, Clone)]
pub struct PendingTransition {
    pub due: SystemTime,
    pub change: PendingChange,
}

/// One immutable version of a connector: "each configuration update creates a
/// new immutable version".
#[derive(Debug, Clone)]
pub struct ConnectorVersion {
    pub version: i64,
    /// `<unqualified Arn>:<version>`.
    pub version_arn: String,
    pub configuration: NetworkConnectorConfiguration,
    pub operator_role: Option<String>,
    /// One of `NetworkConnectorState`'s values.
    pub state: String,
    /// The human-readable explanation of [`ConnectorVersion::state`]: the event that set
    /// it (see [`STATE_REASON_INITIAL_CREATION`]). Set by whatever sets `state`, so the
    /// two are always reported together.
    pub state_reason: Option<String>,
    /// The state this version takes once its provisioning falls due.
    pub pending: Option<PendingTransition>,
    pub last_modified: SystemTime,
}

impl ConnectorVersion {
    /// Materialise the state change the clock owes this version.
    fn settle(&mut self, now: SystemTime) {
        let Some(pending) = self.pending.as_ref() else {
            return;
        };
        if now < pending.due {
            return;
        }
        if let PendingChange::State(state) = &pending.change {
            self.state = state.clone();
        }
        self.pending = None;
    }
}

/// One network connector, as this emulator persists it.
#[derive(Debug, Clone)]
pub struct NetworkConnector {
    pub name: String,
    /// Minted by [`ComAmazonawsLambdacoreLambdacoreapiserviceState::mint_connector_id`].
    pub id: String,
    /// The unqualified ARN: `arn:aws:lambda:<region>:<account>:network-connector:<Id>`.
    pub arn: String,
    /// One of `NetworkConnectorType`'s values.
    pub connector_type: String,
    /// One of `NetworkConnectorState`'s values.
    pub state: String,
    /// The change the clock owes this connector, if any.
    pub pending: Option<PendingTransition>,
    /// The human-readable explanation of [`NetworkConnector::state`]: the event that set
    /// it (see [`STATE_REASON_INITIAL_CREATION`]). Set by whatever sets `state`, so the
    /// two are always reported together.
    pub state_reason: Option<String>,
    pub state_reason_code: Option<String>,
    /// One of `NetworkConnectorLastUpdateStatus`'s values; unset until the
    /// connector has been updated at all.
    pub last_update_status: Option<String>,
    pub last_update_status_reason: Option<String>,
    pub last_update_status_reason_code: Option<String>,
    /// The `LastUpdateStatus` the clock owes once the update in flight finishes.
    pub pending_update: Option<PendingTransition>,
    pub last_modified: SystemTime,
    pub tags: BTreeMap<String, String>,
    pub latest_version: i64,
    /// Every persisted version, ordered by version number.
    pub versions: BTreeMap<i64, ConnectorVersion>,
}

impl NetworkConnector {
    /// Materialise every change the clock owes this connector and its versions.
    ///
    /// Answers `true` when the record is due to be forgotten, which only the
    /// owner of the map can do.
    pub fn settle(&mut self, now: SystemTime) -> bool {
        for version in self.versions.values_mut() {
            version.settle(now);
        }

        if let Some(pending) = self.pending.clone() {
            if now >= pending.due {
                match pending.change {
                    PendingChange::State(state) => {
                        self.state = state;
                        self.pending = None;
                    }
                    PendingChange::Removal => return true,
                }
            }
        }

        if let Some(pending) = self.pending_update.clone() {
            if now >= pending.due {
                if let PendingChange::State(status) = pending.change {
                    self.last_update_status = Some(status);
                }
                self.pending_update = None;
            }
        }

        false
    }

    /// The qualified ARN of version `version` of this connector.
    pub fn version_arn(&self, version: i64) -> String {
        format!("{}:{version}", self.arn)
    }

    /// The version `version` names, or the latest one when `version` is `None`.
    pub fn version(&self, version: Option<i64>) -> Option<&ConnectorVersion> {
        self.versions.get(&version.unwrap_or(self.latest_version))
    }

    /// The qualified ARN of the connector's latest version.
    pub fn latest_version_arn(&self) -> String {
        self.version_arn(self.latest_version)
    }

    /// The `NetworkConnectorSummary` projection `ListNetworkConnectors`
    /// returns.
    pub fn to_summary(&self) -> NetworkConnectorSummary {
        NetworkConnectorSummary {
            arn: self.arn.clone(),
            name: self.name.clone(),
            id: self.id.clone(),
            r#type: self.connector_type.clone(),
            state: Some(self.state.clone()),
            last_modified: Some(::aws_smithy_types::DateTime::from(self.last_modified)),
            latest_version_arn: Some(self.latest_version_arn()),
            latest_version: Some(self.latest_version),
        }
    }

    /// The `NetworkConnectorVersionSummary` projection
    /// `ListNetworkConnectorVersions` returns for one of this connector's
    /// versions.
    pub fn to_version_summary(&self, version: &ConnectorVersion) -> NetworkConnectorVersionSummary {
        NetworkConnectorVersionSummary {
            arn: self.arn.clone(),
            version_arn: version.version_arn.clone(),
            name: self.name.clone(),
            id: self.id.clone(),
            version: version.version,
            state: version.state.clone(),
            // `StateReason` explains the state this answer reports, so it is
            // the reason the record holds for it.
            state_reason: version.state_reason.clone(),
            // `StateReasonCode` is one of `NetworkConnectorStateReasonCode`'s
            // values, every one of which names a provisioning FAILURE
            // (`InvalidSubnet`, `InternalError`, …). No version this emulator
            // provisions fails, so none of those codes describes its state and
            // the member stays unset.
            state_reason_code: None,
            last_modified: Some(::aws_smithy_types::DateTime::from(version.last_modified)),
        }
    }
}

/// What an `Identifier` names: the connector key (its name or id, or the name
/// carried by an ARN) and the version an ARN qualified, if any.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedIdentifier {
    pub key: String,
    pub version: Option<i64>,
}

/// Split an `Identifier` into the connector it names and the version it
/// qualifies.
///
/// `NetworkConnectorIdentifier` is "a flexible identifier that accepts a
/// network connector ID, name, or ARN", and `GetNetworkConnectorRequest$Identifier`
/// documents the two ARN forms: an unqualified one naming the connector, and a
/// version-qualified one "that includes the immutable version suffix". A name is
/// `[a-zA-Z0-9_-]{1,64}` and an id carries no colon either, so the last
/// colon-separated segment is a version suffix exactly when it is a positive
/// integer.
pub fn parse_identifier(identifier: &str) -> ParsedIdentifier {
    // An ARN's resource part is `network-connector:<resource>[:<version>]`; the
    // segments before it (partition, region, account) say nothing about which
    // connector is meant in a single-account emulator.
    let tail = match identifier.split_once(ARN_RESOURCE_INFIX) {
        Some((_, tail)) => tail,
        None => identifier,
    };

    match tail.rsplit_once(':') {
        Some((key, suffix)) => match suffix.parse::<i64>() {
            Ok(version) if version >= 1 && !key.is_empty() => ParsedIdentifier {
                key: key.to_string(),
                version: Some(version),
            },
            _ => ParsedIdentifier {
                key: tail.to_string(),
                version: None,
            },
        },
        None => ParsedIdentifier {
            key: tail.to_string(),
            version: None,
        },
    }
}

/// What a connector's ARN carries before the resource it names, which is also what
/// tells an identifier that IS an ARN from one that is a bare name or id.
pub const ARN_RESOURCE_INFIX: &str = ":network-connector:";

/// The prefix of every `NetworkConnectorId` this service assigns
/// ([`ComAmazonawsLambdacoreLambdacoreapiserviceState::mint_connector_id`]).
pub const CONNECTOR_ID_PREFIX: &str = "nc-";

/// How many hex digits that prefix is followed by.
const CONNECTOR_ID_DIGITS: usize = 16;

/// Whether `value` has the form of a `NetworkConnectorId` this service ASSIGNS, rather
/// than of a `NetworkConnectorName` a caller chose.
///
/// The two identifier forms overlap as grammars — `NetworkConnectorName` is
/// `[a-zA-Z0-9_-]{1,64}`, which admits an id's spelling too — so what tells them apart is
/// the form the SERVICE mints
/// ([`ComAmazonawsLambdacoreLambdacoreapiserviceState::mint_connector_id`]): `nc-` and
/// sixteen hex digits. It matters because an id determines the connector's ARN (the ARN's
/// resource part IS the id) while a name determines nothing of it.
pub fn names_a_connector_id(value: &str) -> bool {
    value
        .strip_prefix(CONNECTOR_ID_PREFIX)
        .is_some_and(|digits| {
            digits.len() == CONNECTOR_ID_DIGITS
                && digits.bytes().all(|digit| digit.is_ascii_hexdigit())
        })
}

/// Per-service state for com-amazonaws-lambdacore-lambdacoreapiservice. A handler reaches it through [`service_state`].
#[derive(Debug, Default)]
pub struct ComAmazonawsLambdacoreLambdacoreapiserviceState {
    /// Every connector, keyed by name — the name is unique "within your account
    /// and Region". A `BTreeMap` is what gives `ListNetworkConnectors` the
    /// stable total order its `Marker` indexes into.
    pub connectors: BTreeMap<String, NetworkConnector>,
    /// A `ClientToken` to the connector and version the call carrying it
    /// produced, so a replayed `@idempotencyToken` request answers with that
    /// resource instead of making a second one.
    ///
    /// BOUNDED: at most [`MAX_IDEMPOTENCY_TOKENS`] entries, written only
    /// through [`ComAmazonawsLambdacoreLambdacoreapiserviceState::remember_token`],
    /// which evicts the oldest entry rather than letting caller-supplied tokens
    /// accumulate without end.
    pub idempotency: HashMap<String, TokenOutcome>,
    /// The counter connector ids are minted from.
    pub connector_id_seq: u64,
    /// The counter that orders remembered client tokens, so the oldest can be
    /// identified for eviction.
    pub token_seq: u64,
}

/// What an already-seen `ClientToken` produced: the connector it named and the
/// version that call created, plus the parameters the call carried.
#[derive(Debug, Clone)]
pub struct TokenOutcome {
    pub name: String,
    pub version: i64,
    /// The parameter set the call that first presented this token carried, so a
    /// later call presenting it can be told apart from a retry of that one.
    pub fingerprint: TokenFingerprint,
    /// The value of [`ComAmazonawsLambdacoreLambdacoreapiserviceState::token_seq`]
    /// when this token was first remembered: the smallest one is the oldest.
    pub remembered: u64,
}

/// The parameters a call presenting a `ClientToken` asked for.
///
/// `@idempotencyToken` holds for the retry of "a request that completed
/// successfully using the same client token" — the token stands for THAT
/// request, so the service can only replay it for a call asking for the same
/// thing. Every member a caller can vary is recorded, so a call that changed
/// any of them is not a retry; the token itself is the key, not part of the set.
#[derive(Debug, Clone, PartialEq)]
pub struct TokenFingerprint {
    /// The connector the request named: the `Name` a create asked for, the
    /// `Identifier` an update addressed.
    pub target: String,
    /// The `Configuration` the request carried, absent when it carried none.
    pub configuration: Option<NetworkConnectorConfiguration>,
    /// The `OperatorRole` the request carried, absent when it carried none.
    pub operator_role: Option<String>,
    /// The `Tags` the request carried, ordered so two equal tag sets compare
    /// equal whatever order they arrived in.
    pub tags: BTreeMap<String, String>,
}

impl ComAmazonawsLambdacoreLambdacoreapiserviceState {
    /// Mint an unused connector id.
    ///
    /// `NetworkConnectorId` is "assigned by the service at creation time" and
    /// carries no pattern, so the shape is the emulator's: `nc-` and 16 hex
    /// digits. Sixteen hex digits hold the whole counter, and the loop skips an
    /// id already taken.
    pub fn mint_connector_id(&mut self) -> String {
        loop {
            self.connector_id_seq = self.connector_id_seq.wrapping_add(1);
            let id = format!("nc-{:016x}", self.connector_id_seq);
            if self.connectors.values().all(|c| c.id != id) {
                return id;
            }
        }
    }

    /// Materialise every change the clock owes, for a caller about to report on
    /// all of them: a connector whose deletion has completed is forgotten,
    /// along with the client tokens that named it.
    pub fn settle_all(&mut self, now: SystemTime) {
        let mut forgotten = Vec::new();
        for (name, connector) in &mut self.connectors {
            if connector.settle(now) {
                forgotten.push(name.clone());
            }
        }
        for name in forgotten {
            self.forget(&name);
        }
    }

    /// The connector `identifier` names, with every change the clock owes it
    /// already materialised, plus the version the identifier qualified.
    ///
    /// Answers `None` when the store holds no live connector for it, which
    /// includes one whose deletion completed before this call: the identifier
    /// names no connector of this account and Region from then on, which every
    /// operation answers `ResourceNotFoundException` for.
    pub fn settled_mut(
        &mut self,
        identifier: &str,
        now: SystemTime,
    ) -> Option<(&mut NetworkConnector, Option<i64>)> {
        let parsed = parse_identifier(identifier);
        let name = self.name_of(&parsed.key)?;
        let forget = self.connectors.get_mut(&name)?.settle(now);
        if forget {
            self.forget(&name);
            return None;
        }
        self.connectors
            .get_mut(&name)
            .map(|connector| (connector, parsed.version))
    }

    /// The name of the connector `key` names, where `key` is a name or an id.
    pub fn name_of(&self, key: &str) -> Option<String> {
        if self.connectors.contains_key(key) {
            return Some(key.to_string());
        }
        self.connectors
            .values()
            .find(|connector| connector.id == key)
            .map(|connector| connector.name.clone())
    }

    /// Remember what a `ClientToken` produced, within the store's bound.
    ///
    /// A token already known keeps its place in the eviction order and only has
    /// its outcome refreshed. A new token past [`MAX_IDEMPOTENCY_TOKENS`] evicts
    /// the oldest remembered token first, so a caller replaying requests with a
    /// fresh token each time cannot grow this map without end — a request whose
    /// token was evicted is simply treated as a request the service has not seen
    /// before.
    pub fn remember_token(
        &mut self,
        token: String,
        name: String,
        version: i64,
        fingerprint: TokenFingerprint,
    ) {
        if let Some(existing) = self.idempotency.get_mut(&token) {
            existing.name = name;
            existing.version = version;
            existing.fingerprint = fingerprint;
            return;
        }
        while self.idempotency.len() >= MAX_IDEMPOTENCY_TOKENS {
            let oldest = self
                .idempotency
                .iter()
                .min_by_key(|(_, outcome)| outcome.remembered)
                .map(|(token, _)| token.clone());
            match oldest {
                Some(token) => {
                    self.idempotency.remove(&token);
                }
                None => break,
            }
        }
        self.token_seq = self.token_seq.saturating_add(1);
        self.idempotency.insert(
            token,
            TokenOutcome {
                name,
                version,
                fingerprint,
                remembered: self.token_seq,
            },
        );
    }

    /// Complete a connector's deletion: the record and every client token that
    /// named it go, so the token map can never outlive the resources it points
    /// at.
    ///
    /// The name is free for a new connector from here on, and the identifiers
    /// that named this one name nothing.
    pub fn forget(&mut self, name: &str) {
        self.connectors.remove(name);
        self.idempotency.retain(|_, outcome| outcome.name != *name);
    }
}

/// The unqualified ARN of the connector whose `Id` is `id`, in `region`.
///
/// `Id` is "the unique identifier for a network connector, assigned by the service at
/// creation time", and `NetworkConnectorArn`'s resource part is
/// `network-connector:<resource>`: the resource segment is that Id, so the `Arn` and the
/// `Id` of one answer always name the same resource and a client can build either from
/// the other. The Id is therefore minted BEFORE the ARN; a caller that addresses the
/// connector by name still resolves through [`parse_identifier`] and
/// [`ComAmazonawsLambdacoreLambdacoreapiserviceState::name_of`].
pub fn connector_arn(region: &str, id: &str) -> String {
    ::engine::emulator_arn("lambda", region, &format!("network-connector:{id}"))
}

/// The region this request was signed for — what a connector ARN names.
pub fn request_region(parts: &::http::request::Parts) -> String {
    ::emulator_server::sigv4_credential_scope(&parts.headers)
        .map(|scope| scope.region)
        .unwrap_or_else(|| DEFAULT_REGION.to_string())
}

/// The `ResourceNotFoundException` payload for an identifier that names no
/// connector (or no such version of one), for the operations that CHANGE a
/// connector.
///
/// The message echoes the identifier the request SUPPLIED, byte for byte, and
/// nothing else: `NetworkConnectorIdentifier` is "a flexible identifier that
/// accepts a network connector ID, name, or ARN", the three forms are
/// interchangeable in the request, and what a write could not change is the thing
/// the caller addressed — so a request by id is refused naming that id, one by
/// name that name, and one by ARN that ARN, each with whatever version suffix it
/// carried.
///
/// The operations that REPORT a connector resolve the identifier instead
/// ([`connector_not_found_resolved`]), which is the one place the two refusals
/// differ.
///
/// The service states it as `Network connector not found: <what was addressed>`
/// ([`NOT_FOUND_PREFIX`]).
pub fn connector_not_found(identifier: &str) -> ResourceNotFoundException {
    not_found(identifier)
}

/// The `ResourceNotFoundException` payload for the same identifier from an
/// operation that REPORTS a connector (`GetNetworkConnector`,
/// `ListNetworkConnectorVersions`).
///
/// A read resolves the identifier to the RESOURCE it determines and names that,
/// which is the resource it would have reported: an ARN is one already, and an
/// `Id` IS an ARN's resource part ([`connector_arn`]), so a read addressed by id
/// is refused naming the ARN that id determines. A NAME determines no ARN (the
/// resource part being an id a name does not carry), so it is stated exactly as
/// supplied. A version-qualified identifier keeps its suffix: the version is part
/// of what was not found.
pub fn connector_not_found_resolved(region: &str, identifier: &str) -> ResourceNotFoundException {
    let parsed = parse_identifier(identifier);
    if identifier.contains(ARN_RESOURCE_INFIX) || !names_a_connector_id(&parsed.key) {
        return not_found(identifier);
    }
    let arn = connector_arn(region, &parsed.key);
    match parsed.version {
        Some(version) => not_found(&format!("{arn}:{version}")),
        None => not_found(&arn),
    }
}

/// The refusal naming `addressed` exactly, which is what the two helpers above
/// resolved the request's identifier to.
fn not_found(addressed: &str) -> ResourceNotFoundException {
    ResourceNotFoundException {
        // `Type` is "the exception type", a value no documented rule gives, so
        // it is left unset rather than invented.
        r#type: None,
        message: Some(format!("{NOT_FOUND_PREFIX}{addressed}")),
    }
}

/// What [`connector_not_found`] states before the resource the request addressed.
pub const NOT_FOUND_PREFIX: &str = "Network connector not found: ";

/// The `ResourceConflictException` payload for a request that conflicts with
/// what the store already holds.
pub fn connector_conflict(message: String) -> ResourceConflictException {
    ResourceConflictException {
        r#type: None,
        message: Some(message),
    }
}

/// What a name already held in this account is refused with: the `Name` is "a
/// unique name for the network connector within your account and Region", and
/// the sentence names the colliding name in its quoted slot.
pub fn duplicate_connector_name(name: &str) -> ResourceConflictException {
    connector_conflict(format!(
        "A network connector with name '{name}' already exists in this account"
    ))
}

/// What a `ClientToken` presented again with a different parameter set is
/// refused with — a fixed sentence, because the conflict is between the request
/// and one the service no longer has to describe.
pub const TOKEN_PARAMETER_MISMATCH: &str =
    "A request with the same client token but different parameters was already submitted";

/// The `NetworkConnectorState` value `state` as the service SPELLS it in a sentence: the
/// name of the state rather than the screaming-snake wire value, so each `_`-separated
/// word is capitalised and the rest lowercased (`PENDING` -> `Pending`, `DELETE_FAILED` ->
/// `DeleteFailed`).
fn state_in_words(state: &str) -> String {
    state
        .split('_')
        .map(|word| {
            let mut characters = word.chars();
            match characters.next() {
                Some(first) => {
                    first.to_uppercase().collect::<String>() + &characters.as_str().to_lowercase()
                }
                None => String::new(),
            }
        })
        .collect()
}

/// What an update of a connector that is not `ACTIVE` is refused with: "the connector must
/// be in ACTIVE state to accept updates", which is what `ResourceConflictException` is
/// modelled for ("for example, attempting to update a connector that is not in ACTIVE
/// state").
///
/// The sentence is BUILT FROM THE RESOURCE: it names the connector by the `Id` the service
/// assigned it — the identifier every answer about that connector carries, whichever of
/// the three forms the request addressed it in — and the state it is in, spelled as the
/// service spells a state's name ([`state_in_words`]), then states the requirement.
pub fn update_requires_active(id: &str, state: &str) -> ResourceConflictException {
    connector_conflict(format!(
        "Network connector {id} is in state {} and cannot be updated. Only {STATE_ACTIVE} \
         connectors can be updated.",
        state_in_words(state)
    ))
}

/// What a delete of a connector in a state it cannot be deleted from is refused with:
/// "you can delete connectors in ACTIVE or FAILED states", so a connector in any other
/// state conflicts with the request.
///
/// The sentence names the state the connector is in, as the wire value of
/// `NetworkConnectorState` spells it, and nothing else: the deletion is refused by the
/// state of the resource the identifier already named.
pub fn delete_refused_in_state(state: &str) -> ResourceConflictException {
    connector_conflict(format!(
        "Network connector is in {state} state and cannot be deleted"
    ))
}

/// The `ResourceConflictException` payload for a reused `ClientToken` whose
/// parameters are not the ones it was first presented with.
pub fn token_parameter_mismatch() -> ResourceConflictException {
    connector_conflict(TOKEN_PARAMETER_MISMATCH.to_string())
}

/// The `Type` every `InvalidParameterValueException` this service answers carries.
///
/// `Type` is "the exception type", and the recorded refusals of this class report the fault
/// as the CALLER's: a parameter the request carried is the thing at fault, so the value is
/// the same for every refusal of the class and is set in one place
/// ([`invalid_parameter`]). The other error classes this service answers report none, which
/// is what their recordings show, so nothing else sets it.
pub const EXCEPTION_TYPE_USER: &str = "User";

/// The `InvalidParameterValueException` payload for a parameter the service will not accept:
/// the sentence, and the `Type` the class carries ([`EXCEPTION_TYPE_USER`]).
pub fn invalid_parameter(message: String) -> InvalidParameterValueException {
    InvalidParameterValueException {
        r#type: Some(EXCEPTION_TYPE_USER.to_string()),
        message: Some(message),
    }
}

/// What a request that omits the `OperatorRole` its connector type needs is
/// refused with: the role is "the IAM role that Lambda assumes to manage elastic
/// network interfaces in your VPC", so a connector of a type that provisions
/// them cannot be created without one. `connector_type` is the
/// `NetworkConnectorType` the request's `Configuration` selects.
pub fn operator_role_required(connector_type: &str) -> InvalidParameterValueException {
    invalid_parameter(format!(
        "NetworkConnectorOperatorRole is required for {connector_type} connector type"
    ))
}

/// The `ServiceException` payload for a store this emulator could not reach.
pub fn store_unavailable() -> ServiceException {
    ServiceException {
        r#type: None,
        message: Some("the network connector store is unavailable".to_string()),
    }
}

/// The page size a `MaxItems` asks for, bounded by what the answering list can
/// carry (`limit`).
///
/// `MaxItems` is modelled `@range(min: 1, max: 100)` and the boundary validator
/// refused anything outside it before a handler ran, so the only case left is
/// an absent value: serve the largest page the answer admits.
pub fn page_size(max_items: Option<i32>, limit: i32) -> usize {
    let requested = max_items.unwrap_or(limit);
    debug_assert!(
        (1..=MAX_ITEMS_CEILING).contains(&requested),
        "MaxItems {requested} should have been refused at the boundary"
    );
    usize::try_from(requested.clamp(1, limit)).expect("a page size clamped into 1..=100 fits usize")
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsLambdacoreLambdacoreapiserviceState>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsLambdacoreLambdacoreapiserviceState>)
}
