// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-supportapp-supportapp service.
//!
//! Besides the state struct itself this module holds the few rules the Support App
//! handlers share — the stored-record-to-modelled-shape conversion, the listing
//! pagination rule, the documented `notifyOnCaseSeverity` consistency rule, the account
//! a caller-supplied `channelRoleArn` must name together with which refusal the members of
//! a channel write draw, in which order those rules are judged and the one counted envelope
//! every clause of a single request travels in, the parts of a modelled `@pattern` the
//! request boundary's walk cannot apply (`channelName`'s `.`, `accountAlias`'s `\w` and the
//! `\S` the Slack ids and the pagination token share among them),
//! which stored Slack channel configuration a `(teamId, channelId)` pair names and how a
//! pair the account holds none for is refused,
//! whether a Slack workspace is registered
//! to the account, the Slack workspaces the Support Center
//! console has authorized, the organization membership an
//! Organizations-scoped call needs, and the wording of the failures
//! several operations can answer with — so the operation files
//! agree on one implementation of each instead of carrying copies that can drift apart.

use std::collections::{BTreeMap, BTreeSet};

use crate::ops::types::{SlackChannelConfiguration, SlackWorkspaceConfiguration};

/// Whether IAM lets a request perform the action it asks for — the decision every operation
/// of this service takes before it reads or writes anything. Re-exported here, so a handler
/// reaches it the way it reaches every other rule of this module.
mod authorization;
pub use authorization::*;

/// The most Slack channel configurations this account can hold.
///
/// The service documents the quota on `CreateSlackChannelConfiguration`: "You can add
/// up to 20 Slack channels for your account". It is what bounds the store: a create
/// that would take the account past it is refused with the operation's modelled
/// `ServiceQuotaExceededException`, so no caller can grow this map without end.
pub const MAX_SLACK_CHANNEL_CONFIGURATIONS: usize = 20;

/// The most Slack workspaces this account can have registered.
///
/// Documented on `CreateSlackChannelConfiguration`: "You can add up to 5 Slack
/// workspaces for your account". It bounds the workspace registry below, so no caller
/// can grow it without end. `RegisterSlackWorkspaceForOrganization` models no
/// `ServiceQuotaExceededException` (its errors are `AccessDeniedException`,
/// `ConflictException`, `InternalServerException`, `ResourceNotFoundException` and
/// `ValidationException`), so a registration past this quota is refused with that
/// operation's modelled `ValidationException`.
pub const MAX_SLACK_WORKSPACE_CONFIGURATIONS: usize = 5;

/// The Slack workspaces the Amazon Web Services Support Center console has authorized,
/// which a fresh account starts from.
///
/// A Slack workspace reaches this API only through the interactive flow in that console —
/// "Sign in to the Amazon Web Services Support Center and authorize the Slack workspaces
/// where you want your organization to have access to", after which
/// `RegisterSlackWorkspaceForOrganization` "authorize[s] each Slack workspace for the
/// organization"
/// (`smithy_query(query="doc",shape="RegisterSlackWorkspaceForOrganization")`) — and NO
/// operation of this API performs that flow, so which workspaces it authorized is state no
/// call of this service writes. This is the set it leaves behind here: the workspace the
/// API's own documentation presents as one of the account's, "This ID uniquely identifies a
/// Slack workspace, such as `T012ABCDEFG`"
/// (`smithy_query(query="doc",shape="RegisterSlackWorkspaceForOrganizationRequest",member="teamId")`).
///
/// Any other team id names a workspace no authorization recorded, which is what
/// [`ComAmazonawsSupportappSupportappState::unauthorized_workspace_not_found`] reports.
/// The set holds exactly the ids the documentation gives and no more: a further entry
/// would be a workspace id no AWS documentation names, and a workspace this emulator
/// never saw authorized is precisely what the miss is about. A caller or a suite that
/// needs another workspace records its authorization in
/// [`ComAmazonawsSupportappSupportappState::console_authorized_slack_workspaces`], which
/// is where the console's own flow would have left it.
pub const CONSOLE_AUTHORIZED_SLACK_WORKSPACES: [&str; 1] = ["T012ABCDEFG"];

/// The longest Slack workspace name this API can report.
///
/// `teamName` is `len 1..256, pattern ^.+$` on both
/// `RegisterSlackWorkspaceForOrganizationResult` and `SlackWorkspaceConfiguration`
/// (`smithy_query(query="shape",shape="teamName")`), so a name this service stores is
/// clipped to it: a longer one could not go on the wire.
pub const MAX_TEAM_NAME_LEN: usize = 256;

/// What a workspace's stored name says it is, after the team id that identifies it.
///
/// A Slack workspace's name lives in Slack, which this emulator cannot ask, so the name
/// it records is built from the one thing the registration carries about the workspace —
/// its team id — plus this. See [`derived_slack_workspace_name`].
const SLACK_WORKSPACE_NAME_SUFFIX: &str = "Slack workspace";

/// The name recorded for the Slack workspace `team_id` identifies.
///
/// A registered workspace HAS a name, which both `RegisterSlackWorkspaceForOrganization`
/// and `ListSlackWorkspaceConfigurations` report in `teamName`. The name is a property of
/// the workspace in Slack, which this emulator cannot query and which no request carries,
/// so it is derived from the workspace's own identifier: one workspace therefore has one
/// name, the same one every time it is reported, and two different workspaces have
/// different names. The result is clipped to [`MAX_TEAM_NAME_LEN`] characters so even the
/// longest legal `teamId` yields a name the model lets this service put on the wire, and
/// it is never empty: `teamId` is itself `len 1..256` and leads the name, and the trim
/// that drops what the clip may leave dangling in the suffix never empties a name it
/// would otherwise be the whole of.
pub fn derived_slack_workspace_name(team_id: &str) -> String {
    let clipped: String = format!("{team_id} {SLACK_WORKSPACE_NAME_SUFFIX}")
        .chars()
        .take(MAX_TEAM_NAME_LEN)
        .collect();
    let trimmed = clipped.trim_end();
    if trimmed.is_empty() {
        clipped
    } else {
        trimmed.to_string()
    }
}

/// The `accountType` of an account that MANAGES the organization it is in, as
/// `RegisterSlackWorkspaceForOrganization` reports it. One of
/// [`crate::ops::types::ACCOUNT_TYPE_VALUES`].
pub const ACCOUNT_TYPE_MANAGEMENT: &str = "management";

/// The `accountType` of an account that is a MEMBER of an organization some other
/// account manages; the other of [`crate::ops::types::ACCOUNT_TYPE_VALUES`].
pub const ACCOUNT_TYPE_MEMBER: &str = "member";

/// That the Organizations store cannot be reached from this engine AT ALL, which is not
/// a fact about the account and so is not the conflict a caller is told about: a handler
/// answers its own modelled server error for it, exactly as it does for this service's
/// own store being unreachable. Only reachable on an engine that is not the shared
/// [`::engine::InMemoryEngine`], which the router never mounts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OrganizationsStoreUnavailable;

/// Which `accountType` the calling account holds in its organization, or `Ok(None)` when
/// it belongs to no organization in Organizations.
///
/// Registering a Slack workspace for the organization is an Organizations-scoped call —
/// "To call this API, your account must be part of an organization in Organizations"
/// (`smithy_query(query="doc",shape="RegisterSlackWorkspaceForOrganization")`) — and
/// whether an account is in an organization, and which account manages that
/// organization, is state the Organizations control plane owns. So it is READ from that
/// store through the one shared engine rather than assumed: `Ok(None)` is exactly the
/// condition the operation's `ConflictException` enumerates ("Call the
/// RegisterSlackWorkspaceForOrganization API from an Amazon Web Services account that
/// doesn't belong to an organization"), and when an organization does hold the account
/// the role follows from that organization's own management account —
/// [`ACCOUNT_TYPE_MANAGEMENT`] for the account that manages it, [`ACCOUNT_TYPE_MEMBER`]
/// for one of its other member accounts. A store that cannot be reached at all is told
/// apart from both, as [`OrganizationsStoreUnavailable`].
///
/// A one-directional, guard-free read of another service's store: it takes that store's
/// lock, holds no other while it does and hands back a copied value, so it is called
/// with NO guard of this service's own store held. Nothing is written — membership of an
/// organization is not something the clock makes due.
pub fn caller_account_type<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Result<Option<&'static str>, OrganizationsStoreUnavailable> {
    let organizations =
        ::service_com_amazonaws_organizations_awsorganizationsv20161128::state::service_state(
            engine,
        )
        .ok_or(OrganizationsStoreUnavailable)?;
    let organizations = organizations.lock();
    let Some(organization) = organizations.organization.as_ref() else {
        return Ok(None);
    };
    let caller = ::engine::emulator_account_id();
    if organization.master_account_id == caller {
        Ok(Some(ACCOUNT_TYPE_MANAGEMENT))
    } else if organizations.accounts.contains_key(caller) {
        Ok(Some(ACCOUNT_TYPE_MEMBER))
    } else {
        Ok(None)
    }
}

/// The `ConflictException` message for an Organizations-scoped call made by an account
/// that belongs to no organization.
///
/// `ConflictException` enumerates the condition — "Call the
/// RegisterSlackWorkspaceForOrganization API from an Amazon Web Services account that
/// doesn't belong to an organization" — and the rule it breaks is the called
/// operation's own: "To call this API, your account must be part of an organization in
/// Organizations". The message states the account the emulator presents, so a caller
/// reads which account is in no organization, and restates that rule for the operation
/// that was called; `operation` is the caller's `Operation::NAME`.
pub fn account_not_in_organization_message(operation: &str) -> String {
    format!(
        "Account {account} doesn't belong to an organization in Organizations. To call the \
         {operation} API, your account must be part of an organization in Organizations.",
        account = ::engine::emulator_account_id()
    )
}

/// The `ConflictException` message for a registration made by an account that is a
/// MEMBER of its organization, of a workspace the organization's management account has
/// not registered for the organization yet.
///
/// `ConflictException` enumerates the condition — "Call the
/// RegisterSlackWorkspaceForOrganization API from a member account, but the management
/// account hasn't registered that workspace yet for the organization" — and the rule it
/// protects is the two-step authorization the operation documents: "After the management
/// account authorizes the Slack workspace, member accounts can call this API to authorize
/// the same Slack workspace for their individual accounts". The message is built from the
/// request's `teamId` and the account the emulator presents, so a caller reads which
/// account was refused for which workspace, and it restates that rule for the operation
/// that was called; `operation` is the caller's `Operation::NAME`.
pub fn workspace_not_authorized_for_organization_message(operation: &str, team_id: &str) -> String {
    format!(
        "Account {account} is a member account of its organization, and the management \
         account hasn't registered Slack workspace {team_id} yet for the organization. The \
         management account must call the {operation} API for a Slack workspace before a \
         member account can.",
        account = ::engine::emulator_account_id()
    )
}

/// One stored Slack channel configuration: everything
/// `CreateSlackChannelConfiguration` was given, after the documented defaults were
/// applied, so `List`/`Update` report the values that were persisted rather than
/// echoing a request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SlackChannelConfig {
    pub team_id: String,
    pub channel_id: String,
    pub channel_name: Option<String>,
    pub notify_on_create_or_reopen_case: Option<bool>,
    pub notify_on_add_correspondence_to_case: Option<bool>,
    pub notify_on_resolve_case: Option<bool>,
    /// One of `NOTIFICATION_SEVERITY_LEVEL_VALUES` (`all` | `high` | `none`); the
    /// member is `@required` on create, so a stored record always carries one.
    pub notify_on_case_severity: String,
    pub channel_role_arn: String,
}

impl SlackChannelConfig {
    /// This record as the modelled `SlackChannelConfiguration`.
    pub fn to_modeled(&self) -> SlackChannelConfiguration {
        SlackChannelConfiguration {
            team_id: self.team_id.clone(),
            channel_id: self.channel_id.clone(),
            channel_name: self.channel_name.clone(),
            notify_on_create_or_reopen_case: self.notify_on_create_or_reopen_case,
            notify_on_add_correspondence_to_case: self.notify_on_add_correspondence_to_case,
            notify_on_resolve_case: self.notify_on_resolve_case,
            notify_on_case_severity: Some(self.notify_on_case_severity.clone()),
            channel_role_arn: Some(self.channel_role_arn.clone()),
        }
    }
}

/// One stored Slack workspace configuration, as
/// `RegisterSlackWorkspaceForOrganization` records it and
/// `ListSlackWorkspaceConfigurations` reports it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SlackWorkspaceConfig {
    pub team_id: String,
    /// The workspace's name, recorded when it was registered (see
    /// [`derived_slack_workspace_name`]). A registered workspace always has one, so this
    /// is not optional even though the modelled member is: every report of this record
    /// carries the same name.
    pub team_name: String,
    pub allow_organization_member_account: Option<bool>,
}

impl SlackWorkspaceConfig {
    /// This record as the modelled `SlackWorkspaceConfiguration`.
    pub fn to_modeled(&self) -> SlackWorkspaceConfiguration {
        SlackWorkspaceConfiguration {
            team_id: self.team_id.clone(),
            team_name: Some(self.team_name.clone()),
            allow_organization_member_account: self.allow_organization_member_account,
        }
    }
}

/// Per-service state for com-amazonaws-supportapp-supportapp. A handler reaches it through [`service_state`].
///
/// `Default` is written out below rather than derived, because a fresh account does not
/// start empty: it starts from the console authorizations in
/// [`CONSOLE_AUTHORIZED_SLACK_WORKSPACES`]. A field added here needs its starting value
/// there too.
#[derive(Debug)]
pub struct ComAmazonawsSupportappSupportappState {
    /// Every stored Slack channel configuration, keyed by `(teamId, channelId)` —
    /// the pair that identifies a channel configuration in this API. A `BTreeMap`,
    /// so list order, and therefore pagination, is deterministic.
    ///
    /// A handler that names ONE configuration reaches it through the accessors that own
    /// that identity rule — [`ComAmazonawsSupportappSupportappState::channel_configuration`],
    /// [`ComAmazonawsSupportappSupportappState::channel_configuration_is_stored`],
    /// [`ComAmazonawsSupportappSupportappState::remove_channel_configuration`] and
    /// [`ComAmazonawsSupportappSupportappState::store_channel_configuration`] — rather
    /// than building the key itself; the map stays reachable for the operations that read
    /// it WHOLE (the listing, and the workspace delete's cascade).
    pub slack_channel_configurations: BTreeMap<(String, String), SlackChannelConfig>,
    /// The account alias `PutAccountAlias` wrote, which `GetAccountAlias` reports
    /// and `DeleteAccountAlias` clears. `None` until one is put: the account has no
    /// alias to report.
    pub account_alias: Option<String>,
    /// Every Slack workspace registered to this account, keyed by `teamId`, written by
    /// `RegisterSlackWorkspaceForOrganization`, removed by
    /// `DeleteSlackWorkspaceConfiguration` and bounded by
    /// [`MAX_SLACK_WORKSPACE_CONFIGURATIONS`]. It is the account's workspace registry:
    /// a Slack channel configuration lives INSIDE a registered workspace, so a request to
    /// ADD one whose `teamId` is absent here has no workspace to add it to (see
    /// [`ComAmazonawsSupportappSupportappState::unregistered_workspace_conflict`]) and
    /// deleting a workspace takes that workspace's channel configurations with it — which
    /// is why a stored configuration is always one of a registered workspace.
    pub slack_workspace_configurations: BTreeMap<String, SlackWorkspaceConfig>,
    /// Every Slack workspace the Amazon Web Services Support Center console has
    /// authorized, keyed by `teamId` and seeded from
    /// [`CONSOLE_AUTHORIZED_SLACK_WORKSPACES`]. It is what makes a workspace EXIST for
    /// this API — the two are different facts, so authorizing a workspace in the console
    /// and registering it for the account are tracked apart: a registration is only
    /// possible for a workspace this set holds (see
    /// [`Self::unauthorized_workspace_not_found`]), and
    /// `DeleteSlackWorkspaceConfiguration` removes the account's REGISTRATION, which
    /// revokes no console authorization, so the same workspace can be registered again.
    /// No operation of this API writes it: the console's interactive authorization flow
    /// is not an API call.
    pub console_authorized_slack_workspaces: BTreeSet<String>,
}

impl Default for ComAmazonawsSupportappSupportappState {
    fn default() -> Self {
        Self {
            slack_channel_configurations: BTreeMap::new(),
            account_alias: None,
            slack_workspace_configurations: BTreeMap::new(),
            console_authorized_slack_workspaces: CONSOLE_AUTHORIZED_SLACK_WORKSPACES
                .iter()
                .map(|team_id| (*team_id).to_string())
                .collect(),
        }
    }
}

impl ComAmazonawsSupportappSupportappState {
    /// Whether this account's Slack workspace registry holds the workspace `team_id`
    /// identifies.
    ///
    /// The registry-presence rule itself, owned where the registry is owned: a workspace
    /// is registered to this account exactly when
    /// [`Self::slack_workspace_configurations`] holds its `teamId`. Both decisions that
    /// turn on it read it from here — the channel-configuration create's
    /// [`Self::unregistered_workspace_conflict`] and
    /// `RegisterSlackWorkspaceForOrganization`'s member-account conflict and quota
    /// accounting — so the handlers cannot come to disagree about whether a workspace
    /// is registered.
    pub fn workspace_is_registered(&self, team_id: &str) -> bool {
        self.slack_workspace_configurations.contains_key(team_id)
    }

    /// The key [`Self::slack_channel_configurations`] holds one configuration under.
    ///
    /// The identity rule itself, owned where the map is owned: a Slack channel
    /// configuration is identified by its whole `(teamId, channelId)` pair, so the pair
    /// is built here and nowhere else — the accessors below are the only readers and
    /// writers of that map a handler calls, so no two handlers can come to disagree
    /// about how a configuration is keyed.
    fn channel_configuration_key(team_id: &str, channel_id: &str) -> (String, String) {
        (team_id.to_string(), channel_id.to_string())
    }

    /// Whether this account holds a Slack channel configuration for the pair
    /// `(team_id, channel_id)` names.
    ///
    /// The presence half of the identity rule, for the create path: configuring a
    /// channel this account already holds a configuration for is the conflict
    /// [`slack_channel_configuration_exists`] words, not a second configuration and not
    /// an overwrite.
    pub fn channel_configuration_is_stored(&self, team_id: &str, channel_id: &str) -> bool {
        self.slack_channel_configurations
            .contains_key(&Self::channel_configuration_key(team_id, channel_id))
    }

    /// The stored Slack channel configuration the pair `(team_id, channel_id)` names, or
    /// the message a pair this account holds no configuration for is refused with.
    ///
    /// The lookup-and-miss rule, owned here so every operation that acts on an EXISTING
    /// configuration draws the same answer from it: a pair the account holds nothing for
    /// is the modelled `ResourceNotFoundException`, worded by
    /// [`slack_channel_configuration_not_found`]. A caller lifts that message into its
    /// own operation's modelled error and adds nothing of its own.
    ///
    /// An operation that acts on an existing configuration settles its answer HERE, on the
    /// resource the request identifies: a pair this account holds nothing for is the miss
    /// whatever the workspace registry holds for that `teamId`, because a workspace the
    /// account has not registered holds no configuration of its own — deleting a workspace
    /// takes its configurations with it ([`Self::slack_channel_configurations`]), so every
    /// stored configuration is one of a registered workspace.
    pub fn channel_configuration(
        &self,
        team_id: &str,
        channel_id: &str,
    ) -> Result<&SlackChannelConfig, String> {
        self.slack_channel_configurations
            .get(&Self::channel_configuration_key(team_id, channel_id))
            .ok_or_else(|| slack_channel_configuration_not_found(team_id, channel_id))
    }

    /// Take the Slack channel configuration the pair `(team_id, channel_id)` names out of
    /// this account's store, or report the miss exactly as [`Self::channel_configuration`]
    /// reports it.
    ///
    /// The same rule as the read above, for the operation that REMOVES what it looked up:
    /// one map touch, so a delete cannot find a configuration a read would miss, and a
    /// pair the account holds nothing for draws the one miss message.
    pub fn remove_channel_configuration(
        &mut self,
        team_id: &str,
        channel_id: &str,
    ) -> Result<SlackChannelConfig, String> {
        self.slack_channel_configurations
            .remove(&Self::channel_configuration_key(team_id, channel_id))
            .ok_or_else(|| slack_channel_configuration_not_found(team_id, channel_id))
    }

    /// Store `configuration` under the pair it carries, replacing whatever this account
    /// held for that pair.
    ///
    /// The write side of the identity rule: a record is keyed by its OWN `teamId` and
    /// `channelId`, so a create and an update of the same channel reach the same entry
    /// and a stored record can never be filed under a pair it does not name.
    pub fn store_channel_configuration(&mut self, configuration: SlackChannelConfig) {
        let key =
            Self::channel_configuration_key(&configuration.team_id, &configuration.channel_id);
        self.slack_channel_configurations.insert(key, configuration);
    }

    /// Why a request that ADDS a Slack channel configuration to the
    /// workspace `team_id` names must be refused with the modelled `ConflictException`,
    /// or `None` when that workspace is registered to this account.
    ///
    /// `ConflictException`'s documentation names this condition first — "Add, update, or
    /// delete a Slack channel configuration before you add a Slack workspace to your
    /// Amazon Web Services account" — and it is the answer of the write that ADDS one: a
    /// `teamId` absent from [`Self::slack_workspace_configurations`] has no workspace for a
    /// channel configuration to be added to, so the create consults the registry before it
    /// stores anything.
    ///
    /// The two writes that act on a configuration that must already EXIST settle their
    /// answer on the resource the request identifies instead
    /// ([`Self::channel_configuration`], [`Self::remove_channel_configuration`]): a pair
    /// the account holds nothing for is the miss, and an unregistered workspace holds no
    /// configuration of its own, so there is nothing to change or remove either way.
    pub fn unregistered_workspace_conflict(&self, team_id: &str) -> Option<String> {
        (!self.workspace_is_registered(team_id)).then(|| workspace_not_registered_message(team_id))
    }

    /// Why a call that AUTHORIZES the Slack workspace `team_id` names must be refused
    /// with the modelled `ResourceNotFoundException`, or `None` when this account's
    /// console authorizations hold that workspace.
    ///
    /// `RegisterSlackWorkspaceForOrganization` acts on a workspace that ALREADY exists
    /// for this API: the workspace is authorized through the interactive flow in the
    /// Amazon Web Services Support Center console, and the API then "authorize[s] each
    /// Slack workspace for the organization"
    /// (`smithy_query(query="doc",shape="RegisterSlackWorkspaceForOrganization")`). So a
    /// `teamId` absent from [`Self::console_authorized_slack_workspaces`] names no
    /// workspace this API can act on, which is the resource miss
    /// `ResourceNotFoundException` reports — "The specified resource is missing or
    /// doesn't exist, such as an account alias, Slack channel configuration, or Slack
    /// workspace configuration".
    ///
    /// That console step is the step the model places on the account that registers a
    /// workspace for its organization, so this is the precondition of a caller the model
    /// asks to take it, settled — like every other lookup of the resource a request
    /// IDENTIFIES — before any further precondition on the CALLING ACCOUNT. It is NOT
    /// the precondition of a member account: "Member accounts don't need to authorize the
    /// Slack workspace manually through the Amazon Web Services Support Center", so a
    /// member account's call is decided by whether the management account has registered
    /// the workspace for the organization ([`Self::workspace_is_registered`]) and this is
    /// not consulted for it.
    pub fn unauthorized_workspace_not_found(&self, team_id: &str) -> Option<String> {
        (!self.console_authorized_slack_workspaces.contains(team_id))
            .then(|| slack_workspace_not_authorized_message(team_id))
    }
}

/// The `ConflictException` message for a Slack channel configuration refused because
/// `team_id` is not in this account's workspace registry.
///
/// `ConflictException`'s documentation names the condition — "Add, update, or delete a
/// Slack channel configuration before you add a Slack workspace to your Amazon Web
/// Services account" — and the message is built from the request's `teamId` and the
/// account the emulator presents, so a caller reads the condition it hit.
pub fn workspace_not_registered_message(team_id: &str) -> String {
    format!(
        "Workspace installation not found for account {account} and team {team_id}",
        account = ::engine::emulator_account_id()
    )
}

/// Why the `nextToken` a listing request carried must be refused, or `None` when the
/// request carried none — the pagination rule both listings of this service follow.
///
/// Neither listing splits its answer. Each reports a collection the service's own
/// documented account quota bounds ([`MAX_SLACK_CHANNEL_CONFIGURATIONS`],
/// [`MAX_SLACK_WORKSPACE_CONFIGURATIONS`]) and neither operation takes a page size
/// member, so the whole collection fits the one response and there is never a further
/// page to point at: the output `nextToken` stays absent, exactly as the member
/// documents for the last set of results.
///
/// A listing that mints no token therefore has no position any token could name, so a
/// request that carries one is refused rather than silently read as "from the
/// beginning" — the caller is told the member is wrong instead of being handed a page
/// it did not ask for. The refusal is a handler-raised request-validation refusal like
/// every other one of this service, so it travels in the 400 `ValidationException` this
/// SERVICE models — widened onto each listing's own error enum, since neither
/// operation's `errors` list names it — worded by
/// [`invalid_pagination_token_message`]; `operation` is the caller's `Operation::NAME`.
///
/// The member's own declared `@pattern` is judged FIRST, because a value outside it is not a
/// token this service could have minted in any state: the member is `paginationToken`, which
/// declares the same `^\S+$` the Slack ids do, and the part of that expression the request
/// boundary's walk cannot apply is applied through [`slack_id_refusal`] — so a malformed
/// member is refused as the declared constraint it breaks, before this service's own rule
/// about a token no listing returned is stated.
pub fn pagination_token_refusal(operation: &str, next_token: Option<&str>) -> Option<String> {
    let next_token = next_token?;
    slack_id_refusal(&[(NEXT_TOKEN_PATH, next_token)])
        .or_else(|| Some(invalid_pagination_token_message(operation)))
}

/// The three notification flags as this service's PROSE names them.
///
/// A Support App error message states a parameter in the API's PascalCase spelling
/// (`NotifyOnCreateOrReopenCase`), not in the lowerCamel spelling the wire member
/// uses, and lists them in this order. The wire members are
/// `notifyOnCreateOrReopenCase`, `notifyOnAddCorrespondenceToCase` and
/// `notifyOnResolveCase` on both
/// `CreateSlackChannelConfigurationRequest` and `UpdateSlackChannelConfigurationRequest`
/// (`smithy_query(query="operation",operation="CreateSlackChannelConfiguration")`).
const NOTIFICATION_FLAG_PARAMETERS: [&str; 3] = [
    "NotifyOnCreateOrReopenCase",
    "NotifyOnAddCorrespondenceToCase",
    "NotifyOnResolveCase",
];

/// The `NotificationSeverityLevel` value that asks for notifications on no case at
/// all, as the wire spells it (one of
/// [`crate::ops::types::NOTIFICATION_SEVERITY_LEVEL_VALUES`]).
///
/// It is the severity ONE clause of the documented `notifyOnCaseSeverity` rule is
/// keyed to, so every operation that judges whether that clause applies to its
/// request names the value from here rather than spelling it again (see
/// [`notification_rule_violation`]).
pub const NOTIFICATION_SEVERITY_NONE: &str = "none";

/// A Support App `ValidationException` message: service prose that names the
/// operation whose request was refused, states the rule the request broke, and asks
/// the caller to retry.
///
/// This is the shape every Support App request-validation message has — "Your
/// `<Operation>` request isn't valid. `<rule>` Try your request again." — so the
/// operation's own handler supplies only the clause that states its rule and cannot
/// spell the envelope differently from its neighbour. `rule` is a complete sentence,
/// ending in its own full stop.
pub fn invalid_request_message(operation: &str, rule: &str) -> String {
    format!("Your {operation} request isn't valid. {rule} Try your request again.")
}

/// The `ValidationException` message for a `nextToken` no response of this service
/// returned, shared by every paginated operation of this service.
///
/// Stated through [`invalid_request_message`], like every other request-validation
/// refusal this service raises from a handler, so a caller reading only the `message`
/// learns which request was refused and which member of it was wrong. The rule
/// RESTATES what the member documents — "reissue the search request and include the
/// returned token"
/// (`smithy_query(query="doc",shape="ListSlackChannelConfigurationsRequest",member="nextToken")`)
/// — and names the member the way this service's prose names a parameter, in the
/// API's PascalCase spelling. `operation` is the caller's `Operation::NAME`.
pub fn invalid_pagination_token_message(operation: &str) -> String {
    invalid_request_message(
        operation,
        &format!(
            "NextToken must be a pagination token that an earlier {operation} response \
             returned."
        ),
    )
}

/// The `ValidationException` message for a registration past
/// [`MAX_SLACK_WORKSPACE_CONFIGURATIONS`], in the same envelope.
///
/// The rule is the documented quota — "You can add up to 5 Slack workspaces for your
/// account" (`smithy_query(query="doc",shape="CreateSlackChannelConfiguration")`) —
/// restated from the constant that enforces it, so the bound the store applies and the
/// bound the message states cannot drift apart.
pub fn workspace_quota_exceeded_message(operation: &str) -> String {
    invalid_request_message(
        operation,
        &format!(
            "You can add up to {MAX_SLACK_WORKSPACE_CONFIGURATIONS} Slack workspaces for your \
             account."
        ),
    )
}

/// Why the `channelRoleArn` a write carried names no role this account can pass, as the
/// message the refusal carries, or `None` when it does.
///
/// Private, and reached only through [`channel_write_refusal`]: WHICH refusal a request's
/// members draw, and in which order the rules are judged, is that function's rule, not a
/// handler's.
///
/// The member documents the role as one "that you want to use to perform operations on
/// Amazon Web Services"
/// (`smithy_query(query="operation",operation="CreateSlackChannelConfiguration")`) for
/// the calling account, so the value is not request INPUT the service merely stores: it
/// names a role the service is asked to assume, which is a pass-role attempt. That
/// attempt is what judges the value — its FORM included — so the member's `@pattern` is
/// off the request boundary (`skip_validation` on both inputs that carry it, in
/// `src/ops/types/`) and every value reaches this rule, as the recorded refusal of a
/// value outside that pattern shows: AWS answers the modelled `AccessDeniedException`
/// (HTTP 403) for it, not a request-validation `ValidationException`.
///
/// A service acting for one account cannot be handed a role another account owns, so the
/// test is the one the member's own form states: the value has to be an IAM role ARN of
/// the caller's account NAMING a role, which is that form with the account field filled
/// in from [`::engine::emulator_account_id`] and a role name the member's `(.+)` admits
/// after `role/` ([`is_role_name`]). It therefore needs no ARN reading of its own and
/// it fails CLOSED — anything that is not a role ARN of this account, whatever else it
/// is, is a role this service cannot pass. EVERY operation that takes the member answers
/// this refusal rather than persisting the value, before the account's store is
/// consulted, so nothing is written when it holds.
fn channel_role_account_refusal(role_arn: &str) -> Option<String> {
    let own_account_role = format!("arn:aws:iam::{}:role/", ::engine::emulator_account_id());
    let names_a_role_of_this_account = role_arn
        .strip_prefix(&own_account_role)
        .is_some_and(is_role_name);
    (!names_a_role_of_this_account).then(|| CROSS_ACCOUNT_PASS_ROLE_MESSAGE.to_string())
}

/// The characters a modelled `@pattern`'s `.` does NOT match.
///
/// A Smithy `@pattern` is an ECMA-262 regular expression, and there `.` matches every
/// character except a LINE TERMINATOR: line feed, carriage return, line separator and
/// paragraph separator. A value carrying any of them therefore cannot be matched by a `.`
/// of such an expression, however many characters it has.
///
/// Stated here because such an expression has to be applied by hand (see
/// [`channel_role_account_refusal`] and [`channel_name_refusal`]), and because the engine
/// the framework's centralized constraint walk compiles a `@pattern` with reads `.` as
/// every character except a line FEED alone, so the other three would be admitted there
/// whatever the modelled expression says.
const PATTERN_DOT_EXCLUDED_CHARS: [char; 4] = ['\n', '\r', '\u{2028}', '\u{2029}'];

/// Whether `value` carries a character a modelled `@pattern`'s `.` does not match (one of
/// the [`PATTERN_DOT_EXCLUDED_CHARS`]) — the one test every `.`-based expression this
/// module applies by hand shares, so no member's rule spells it out again.
fn carries_pattern_dot_excluded_char(value: &str) -> bool {
    value
        .chars()
        .any(|character| PATTERN_DOT_EXCLUDED_CHARS.contains(&character))
}

/// Whether `role_name` is what the member's `(.+)` admits after `role/`, with the `$` that
/// anchors it: at least one character, and every one of them a character that expression's
/// `.` matches — so none of the [`PATTERN_DOT_EXCLUDED_CHARS`] it does not.
///
/// This is the whole of the member's form that is left once the prefix the account test
/// already pinned is stripped, so a value this rejects is outside the member's declared
/// `@pattern` and could not be reported back through the same shape.
fn is_role_name(role_name: &str) -> bool {
    !role_name.is_empty() && !carries_pattern_dot_excluded_char(role_name)
}

/// `channelName`'s declared `@pattern`, as the model spells it — ANCHORED, which is the
/// text a clause reporting it is rendered from
/// (`smithy_query(query="shape",shape="channelName")`).
pub const CHANNEL_NAME_PATTERN: &str = "^.+$";

/// The member path a clause about `channelName` names, in the lowerCamel spelling the
/// validation envelope states a member with.
const CHANNEL_NAME_PATH: &str = "channelName";

/// Why the `channelName` a write carried is outside that member's declared `@pattern`
/// ([`CHANNEL_NAME_PATTERN`]), as the clause the refusal names it in, or `None` when the
/// value is one the expression admits.
///
/// The member's expression is `.`-based, and the framework's centralized constraint walk
/// cannot apply such an expression in full: it compiles a `@pattern` with an engine whose
/// `.` excludes a line FEED alone, where the modelled ECMA-262 expression's `.` excludes
/// every one of the [`PATTERN_DOT_EXCLUDED_CHARS`]. So the part of the expression that walk
/// admits but the model does not is applied HERE, by both operations that write the member
/// (`CreateSlackChannelConfiguration` and `UpdateSlackChannelConfiguration`), before the
/// store is reached: the same shape with the same `@pattern` is an OUTPUT member —
/// `UpdateSlackChannelConfiguration` answers with `channelName` and
/// `ListSlackChannelConfigurations` reports it inside every `SlackChannelConfiguration` —
/// so a value the expression does not admit could not be reported back through the shape it
/// was stored in. The rest of the expression (its `+`, and the member's `@length` 1..256)
/// is the walk's, which applies it to every request that carries the member.
///
/// The refusal is the one a declared constraint draws, because this IS the member's
/// declared `@pattern`, and the clause is built by [`declared_pattern_violation`] — the one
/// every `@pattern` this module applies by hand goes through, so one request is refused one
/// way whichever reading of the expression caught it. What this rule yields is the CLAUSE
/// and not a whole refusal, because the other clauses the same request draws travel in the
/// envelope with it: [`channel_write_refusal`] is what renders them.
fn channel_name_violation(
    channel_name: &str,
) -> Option<::aws_smithy_schema_server::constraints::Violation> {
    if !carries_pattern_dot_excluded_char(channel_name) {
        return None;
    }
    Some(declared_pattern_violation(
        CHANNEL_NAME_PATH,
        CHANNEL_NAME_PATTERN,
        channel_name,
    ))
}

/// The message a value outside its member's own declared `@pattern` is refused with.
///
/// The refusal is the one a declared constraint draws, because such a value IS outside the
/// member's declared `@pattern`: the standard validation envelope over one clause naming the
/// rejected value, the member path and the expression, built by the framework's own renderer
/// ([`::aws_smithy_schema_server::constraints::envelope`]) from the same violation the walk
/// raises for a value its own reading of the expression does catch, and with the `^` / `$`
/// pair this service drops from a reported expression (`src/service.rs`, `as_stated`)
/// dropped by the same shared rendering. So one request is refused one way whichever reading
/// of the expression caught it. The value is echoed through the tree's shared
/// [`::service_common::validation::quoted_echo`], so a handler-worded clause quotes a value
/// under the same ceiling as one the walk restored.
///
/// `declared` is the expression as the MODEL spells it, anchors and all, and `path` is the
/// member the clause names, in the lowerCamel spelling the envelope states a member with.
fn declared_pattern_refusal(path: &str, declared: &str, value: &str) -> Option<String> {
    let violation = declared_pattern_violation(path, declared, value);
    ::aws_smithy_schema_server::constraints::envelope(::std::slice::from_ref(&violation))
}

/// The single clause a value outside its member's own declared `@pattern` is refused with,
/// as the framework's own violation — the one [`declared_pattern_refusal`] renders, kept
/// apart from the rendering so a request whose SEVERAL members are each outside their own
/// declared expression is refused in ONE counted envelope naming every clause, exactly as
/// the walk refuses several violations it caught itself.
fn declared_pattern_violation(
    path: &str,
    declared: &str,
    value: &str,
) -> ::aws_smithy_schema_server::constraints::Violation {
    ::aws_smithy_schema_server::constraints::Violation {
        path: path.to_string(),
        kind: ::aws_smithy_schema_server::constraints::ViolationKind::Pattern(
            ::service_common::validation::unanchored(declared),
        ),
        value: Some(::service_common::validation::quoted_echo(value).into_owned()),
        sensitive: false,
    }
}

/// The declared `@pattern` `teamId`, `channelId` and `paginationToken` share, as the model
/// spells it — ANCHORED, which is the text a clause reporting it is rendered from
/// (`smithy_query(query="shape",shape="teamId,channelId,paginationToken")`).
pub const SLACK_ID_PATTERN: &str = r"^\S+$";

/// The member path a clause about `teamId` names, in the lowerCamel spelling the validation
/// envelope states a member with.
const TEAM_ID_PATH: &str = "teamId";

/// The same, for `channelId`.
const CHANNEL_ID_PATH: &str = "channelId";

/// The same, for the member every listing of this service carries `paginationToken` as.
const NEXT_TOKEN_PATH: &str = "nextToken";

/// The characters a modelled `@pattern`'s `\S` does NOT match that the engine the
/// framework's centralized constraint walk compiles a `@pattern` with lets through.
///
/// A Smithy `@pattern` is an ECMA-262 regular expression, and there `\s` matches every
/// WhiteSpace and every LineTerminator: the tab, the vertical tab, the form feed, the space,
/// the no-break space, every character of the Unicode `Space_Separator` category, the ZERO
/// WIDTH NO-BREAK SPACE U+FEFF, and the four [`PATTERN_DOT_EXCLUDED_CHARS`]. `\S` is that
/// class complemented, so it matches none of them.
///
/// Stated here because such a class has to be applied by hand (see [`slack_id_refusal`]),
/// and because the engine that walk compiles with reads `\s` as the Unicode `White_Space`
/// property, which holds every one of those characters EXCEPT U+FEFF — so U+FEFF is the
/// whole of what its `\S` admits and the modelled expression's `\S` does not.
const PATTERN_SPACE_CHARS_BEYOND_WHITE_SPACE: [char; 1] = ['\u{FEFF}'];

/// Whether `value` carries a character a modelled `@pattern`'s `\S` does not match (one of
/// the [`PATTERN_SPACE_CHARS_BEYOND_WHITE_SPACE`]) — the one test every `\S`-based
/// expression this module applies by hand shares, so no member's rule spells it out again.
fn carries_pattern_space_char(value: &str) -> bool {
    value
        .chars()
        .any(|character| PATTERN_SPACE_CHARS_BEYOND_WHITE_SPACE.contains(&character))
}

/// Why the `\S`-patterned members a request carried are outside their declared `@pattern`
/// ([`SLACK_ID_PATTERN`]), as the message the refusal carries, or `None` when every value is
/// one the expression admits.
///
/// The expression is `\S`-based, and the framework's centralized constraint walk cannot apply
/// such an expression in full: it compiles a `@pattern` with an engine whose `\s` is the
/// Unicode `White_Space` property, where the modelled ECMA-262 expression's `\s` also matches
/// the [`PATTERN_SPACE_CHARS_BEYOND_WHITE_SPACE`]. So the part of the expression that walk
/// admits but the model does not is applied HERE, by every operation that CARRIES one of
/// these members, before the store is reached: the same shapes with the same `@pattern` are
/// OUTPUT members — `SlackChannelConfiguration$teamId` and `$channelId` are `@required`
/// members of what `ListSlackChannelConfigurations` reports,
/// `SlackWorkspaceConfiguration$teamId` of what `ListSlackWorkspaceConfigurations` reports,
/// and `UpdateSlackChannelConfiguration` answers with both — so a value the expression does
/// not admit could not be reported back through the shape it was stored under. The rest of
/// the expression (its `+`, and each member's declared `@length` 1..256) is the walk's, which
/// applies it to every request that carries the member.
///
/// `members` are the members the request carries, each as its path and its value, in the
/// order the model declares them: a request whose members are each outside the expression is
/// refused in ONE counted envelope naming every clause, as the walk refuses a request whose
/// members it caught itself.
fn slack_id_refusal(members: &[(&str, &str)]) -> Option<String> {
    ::aws_smithy_schema_server::constraints::envelope(&slack_id_violations(members))
}

/// The clauses those members draw, as the framework's own violations and in the order
/// `members` were handed over — what [`slack_id_refusal`] renders, kept apart from the
/// rendering so the clauses a request's OTHER members draw from a rule this module applies
/// by hand travel in the same envelope ([`channel_write_refusal`]).
fn slack_id_violations(
    members: &[(&str, &str)],
) -> Vec<::aws_smithy_schema_server::constraints::Violation> {
    members
        .iter()
        .filter(|(_, value)| carries_pattern_space_char(value))
        .map(|(path, value)| declared_pattern_violation(path, SLACK_ID_PATTERN, value))
        .collect()
}

/// Why the `teamId` a request carried is outside that member's declared `@pattern`, or
/// `None` when the value is one the expression admits — the entry point the operations that
/// carry the member ALONE (`RegisterSlackWorkspaceForOrganization` and
/// `DeleteSlackWorkspaceConfiguration`) go through. See [`slack_id_refusal`] for the rule.
pub fn team_id_refusal(team_id: &str) -> Option<String> {
    slack_id_refusal(&[(TEAM_ID_PATH, team_id)])
}

/// Why the `(teamId, channelId)` pair a request carried is outside those members' declared
/// `@pattern`, or `None` when both values are ones the expression admits — the entry point
/// the operations that name a channel configuration by its pair
/// (`CreateSlackChannelConfiguration`, `UpdateSlackChannelConfiguration` and
/// `DeleteSlackChannelConfiguration`) go through. Both clauses travel in one envelope, in
/// the order the model declares the members. See [`slack_id_refusal`] for the rule.
pub fn channel_pair_refusal(team_id: &str, channel_id: &str) -> Option<String> {
    slack_id_refusal(&[(TEAM_ID_PATH, team_id), (CHANNEL_ID_PATH, channel_id)])
}

/// `accountAlias`'s declared `@pattern`, as the model spells it — ANCHORED, which is the
/// text a clause reporting it is rendered from
/// (`smithy_query(query="shape",shape="awsAccountAlias")`).
pub const ACCOUNT_ALIAS_PATTERN: &str = r"^[\w\- ]+$";

/// The member path a clause about `accountAlias` names, in the lowerCamel spelling the
/// validation envelope states a member with.
const ACCOUNT_ALIAS_PATH: &str = "accountAlias";

/// The characters [`ACCOUNT_ALIAS_PATTERN`]'s class admits BESIDE its `\w`: the escaped
/// literal hyphen the expression writes, and a space.
const ACCOUNT_ALIAS_LITERAL_CHARS: [char; 2] = ['-', ' '];

/// Whether `character` is one a modelled `@pattern`'s `\w` matches.
///
/// A Smithy `@pattern` is an ECMA-262 regular expression, and there `\w` is the ASCII class
/// `[A-Za-z0-9_]`: a letter of another script, a digit of another numbering system, a
/// combining mark and a connector punctuation character are none of them, however much a
/// reader would call them word characters.
///
/// Stated here because such a class has to be applied by hand (see
/// [`account_alias_refusal`]), and because the engine the framework's centralized constraint
/// walk compiles a `@pattern` with reads `\w` as the UNICODE class, so every character
/// outside the ASCII one would be admitted there whatever the modelled expression says.
fn is_pattern_word_char(character: char) -> bool {
    character.is_ascii_alphanumeric() || character == '_'
}

/// Whether `account_alias` is what [`ACCOUNT_ALIAS_PATTERN`] admits, with the `^` / `$` pair
/// that anchors it: at least one character, and every one of them a member of the
/// expression's class — an ECMA-262 `\w` ([`is_pattern_word_char`]) or one of
/// [`ACCOUNT_ALIAS_LITERAL_CHARS`].
pub fn satisfies_account_alias_pattern(account_alias: &str) -> bool {
    !account_alias.is_empty()
        && account_alias.chars().all(|character| {
            is_pattern_word_char(character) || ACCOUNT_ALIAS_LITERAL_CHARS.contains(&character)
        })
}

/// Why the `accountAlias` a write carried is outside that member's declared `@pattern`
/// ([`ACCOUNT_ALIAS_PATTERN`]), as the message the refusal carries, or `None` when the value
/// is one the expression admits.
///
/// The member's expression is `\w`-based, and the framework's centralized constraint walk
/// cannot apply such an expression in full: it compiles a `@pattern` with an engine whose
/// `\w` is the Unicode class, where the modelled ECMA-262 expression's `\w` is the ASCII one
/// ([`is_pattern_word_char`]). So the part of the expression that walk admits but the model
/// does not is applied HERE, by the one operation that writes the member
/// (`PutAccountAlias`), before the store is reached: the same shape with the same `@pattern`
/// is an OUTPUT member — `GetAccountAlias` answers the stored alias through
/// `GetAccountAliasResult$accountAlias` — so a value the expression does not admit could not
/// be reported back through the shape it was stored in. The rest of the expression (its `+`,
/// and the member's declared `@length` 1..30) is the walk's, which applies it to every
/// request that carries the member.
pub fn account_alias_refusal(account_alias: &str) -> Option<String> {
    if satisfies_account_alias_pattern(account_alias) {
        return None;
    }
    declared_pattern_refusal(ACCOUNT_ALIAS_PATH, ACCOUNT_ALIAS_PATTERN, account_alias)
}

/// The `AccessDeniedException` message a refused pass-role attempt carries, as AWS words
/// that refusal: fixed text, naming the rule the attempt broke and quoting no value of the
/// request back.
pub const CROSS_ACCOUNT_PASS_ROLE_MESSAGE: &str = "Cross-account pass role is not allowed.";

/// The longest `channelRoleArn` this service accepts: the member's own declared maximum
/// (`roleArn`, `@length min 31 max 2048`,
/// `smithy_query(query="operation",operation="CreateSlackChannelConfiguration")`).
pub const MAX_CHANNEL_ROLE_ARN_LEN: usize = 2048;

/// The member path a clause about `channelRoleArn` names, in the lowerCamel spelling the
/// validation envelope states a member with.
const CHANNEL_ROLE_ARN_PATH: &str = "channelRoleArn";

/// Why the `channelRoleArn` a write carried is too long to hold, as the clause the refusal
/// names it in, or `None` when it fits.
///
/// Private for the same reason as [`channel_role_account_refusal`]: a handler asks
/// [`channel_write_refusal`] what this request's answer is.
///
/// The member's declared `@length` maximum is a bound on what the API can carry — the
/// same member reports the stored value back on `UpdateSlackChannelConfiguration` and
/// `ListSlackChannelConfigurations` — and it is the one rule of this member the request
/// boundary no longer applies, because taking the value into the pass-role step took the
/// whole member off that boundary (see [`channel_role_account_refusal`]). So the bound is
/// applied HERE instead, by the operations that write the member, and a value past it is
/// the modelled `ValidationException` (HTTP 400) rather than something stored: it is a
/// caller-input mistake, not a role this service may not pass, and it is settled before
/// the pass-role attempt for that reason.
///
/// A declared `@length` maximum is a constraint the MODEL states, so the refusal is the one
/// AWS words for that class — the standard validation envelope over one clause naming the
/// rejected value, the member path and the bound, built by the framework's own renderer
/// ([`::aws_smithy_schema_server::constraints::envelope`]) from the same violation the
/// request boundary would have raised. That is what keeps this clause worded exactly as the
/// clause every other member of this service is refused with (`accountAlias`'s length clause
/// among them) instead of in the prose envelope this service reserves for a rule the model
/// cannot express. The value is echoed through the tree's shared
/// [`::service_common::validation::quoted_echo`], so a handler-worded clause quotes a value
/// under the same ceiling as one the walk restored and no message is sized by the request.
/// Like every other constraint this module applies by hand it yields the CLAUSE and not a
/// whole refusal, so the clauses one request draws travel in one envelope
/// ([`channel_write_refusal`]).
fn channel_role_arn_length_violation(
    role_arn: &str,
) -> Option<::aws_smithy_schema_server::constraints::Violation> {
    if role_arn.chars().count() <= MAX_CHANNEL_ROLE_ARN_LEN {
        return None;
    }
    Some(::aws_smithy_schema_server::constraints::Violation {
        path: CHANNEL_ROLE_ARN_PATH.to_string(),
        kind: ::aws_smithy_schema_server::constraints::ViolationKind::MaxLength(
            i64::try_from(MAX_CHANNEL_ROLE_ARN_LEN).unwrap_or(i64::MAX),
        ),
        value: Some(::service_common::validation::quoted_echo(role_arn).into_owned()),
        sensitive: false,
    })
}

/// Which refusal a `channelRoleArn` draws, as the modelled error class it is answered in
/// plus the message that class carries.
///
/// The two classes are not interchangeable — a client branches on which one it got — so the
/// class belongs to the rule that decided the refusal rather than to the handler that
/// writes the response: see [`channel_role_arn_refusal`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ChannelRoleArnRefusal {
    /// The value is outside what this API can carry: the modelled `ValidationException`
    /// (HTTP 400), from the member's declared `@length` maximum
    /// ([`MAX_CHANNEL_ROLE_ARN_LEN`]).
    Validation(String),
    /// The value names no role this service may pass for the account: the modelled
    /// `AccessDeniedException` (HTTP 403), worded as
    /// [`CROSS_ACCOUNT_PASS_ROLE_MESSAGE`].
    AccessDenied(String),
}

/// Which refusal the members of a Slack channel write draw, as the modelled error class the
/// request is answered in plus the message that class carries — the request-wide answer
/// [`ChannelRoleArnRefusal`] is the one-member view of.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ChannelWriteRefusal {
    /// At least one member the request carries is outside its own declared constraint: the
    /// modelled `ValidationException` (HTTP 400), as the standard validation envelope over
    /// EVERY clause the request drew.
    Validation(String),
    /// Every value this API can carry, and the `channelRoleArn` names no role this service
    /// may pass for the account: the modelled `AccessDeniedException` (HTTP 403), worded as
    /// [`CROSS_ACCOUNT_PASS_ROLE_MESSAGE`].
    AccessDenied(String),
}

impl From<ChannelWriteRefusal> for ChannelRoleArnRefusal {
    fn from(refusal: ChannelWriteRefusal) -> Self {
        match refusal {
            ChannelWriteRefusal::Validation(message) => Self::Validation(message),
            ChannelWriteRefusal::AccessDenied(message) => Self::AccessDenied(message),
        }
    }
}

/// The members of a Slack channel write whose declared constraints this service applies by
/// hand, each as the value the request CARRIES and `None` for a member it does not carry —
/// a rule judges the request in front of it, so an absent optional member has no value to
/// be outside anything.
///
/// The fields are in the order the model declares the members
/// (`smithy_query(query="operation",operation="CreateSlackChannelConfiguration")`), which is
/// the order [`channel_write_refusal`] states their clauses in.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ChannelWriteMembers<'a> {
    /// The `teamId` the request carries, whose `@pattern` is [`SLACK_ID_PATTERN`].
    pub team_id: Option<&'a str>,
    /// The `channelId` the request carries, whose `@pattern` is [`SLACK_ID_PATTERN`].
    pub channel_id: Option<&'a str>,
    /// The `channelName` the request carries, whose `@pattern` is [`CHANNEL_NAME_PATTERN`].
    pub channel_name: Option<&'a str>,
    /// The `channelRoleArn` the request carries, whose declared `@length` maximum is
    /// [`MAX_CHANNEL_ROLE_ARN_LEN`] and which names the role the service is asked to pass.
    pub channel_role_arn: Option<&'a str>,
}

/// Every clause the members a write carries draw from a declared constraint this service
/// applies by hand, in the order the model declares those members: `teamId` and `channelId`
/// ([`slack_id_violations`]), `channelName` ([`channel_name_violation`]) and
/// `channelRoleArn`'s declared `@length` maximum ([`channel_role_arn_length_violation`]).
///
/// Collected rather than returned one at a time because the clauses of ONE request belong in
/// ONE envelope: see [`channel_write_refusal`].
fn channel_write_violations(
    members: &ChannelWriteMembers<'_>,
) -> Vec<::aws_smithy_schema_server::constraints::Violation> {
    let patterned_ids: Vec<(&str, &str)> = [
        members.team_id.map(|value| (TEAM_ID_PATH, value)),
        members.channel_id.map(|value| (CHANNEL_ID_PATH, value)),
    ]
    .into_iter()
    .flatten()
    .collect();
    let mut violations = slack_id_violations(&patterned_ids);
    violations.extend(members.channel_name.and_then(channel_name_violation));
    violations.extend(
        members
            .channel_role_arn
            .and_then(channel_role_arn_length_violation),
    );
    violations
}

/// Why the members a Slack channel write carried cannot be used — which modelled error class
/// answers the request and with which message — or `None` when every value is one this
/// service can take.
///
/// This is the single entry point every operation that writes a channel configuration goes
/// through, so the ORDER of the rules, and the class each refusal travels in, is stated once
/// here instead of once per writing handler:
///
/// * every member that is outside its OWN declared constraint is reported together, in the
///   standard validation envelope over every clause the request drew
///   ([`channel_write_violations`]) — a request breaking two of these rules is refused the
///   same way the request boundary's own walk refuses a request whose members IT caught, in
///   one counted envelope naming every clause in model order, rather than with whichever
///   clause a handler happened to test first;
/// * and only a request this API can carry WHOLE reaches the pass-role attempt that judges
///   which role the `channelRoleArn` names ([`channel_role_account_refusal`]), because that
///   attempt is about a value the API could have carried.
///
/// A handler's whole part is lifting the answer into its own operation error enum, which is
/// the one thing it cannot share; no operation can reorder, drop or split a step, so two
/// operations cannot answer different classes, or different clauses, for the same request.
///
/// Nothing is read or written for a request this refuses: every rule is a fact about the
/// REQUEST, so each is answered before the account's store is reached.
pub fn channel_write_refusal(members: &ChannelWriteMembers<'_>) -> Option<ChannelWriteRefusal> {
    if let Some(message) =
        ::aws_smithy_schema_server::constraints::envelope(&channel_write_violations(members))
    {
        return Some(ChannelWriteRefusal::Validation(message));
    }
    members
        .channel_role_arn
        .and_then(channel_role_account_refusal)
        .map(ChannelWriteRefusal::AccessDenied)
}

/// Why the `channelRoleArn` a write carried cannot be used — which modelled error class
/// answers it and with which message — or `None` when the value names a role this account
/// can pass.
///
/// The member's own view of [`channel_write_refusal`], over a request carrying this member
/// and no other member that rule reads: the same two rules in the same order — the declared
/// `@length` maximum ([`MAX_CHANNEL_ROLE_ARN_LEN`]) is a caller-input mistake and is settled
/// FIRST, and only a value this API can carry reaches the pass-role attempt that judges
/// which role it names — so the member cannot be answered one way on its own and another way
/// inside a request.
pub fn channel_role_arn_refusal(role_arn: &str) -> Option<ChannelRoleArnRefusal> {
    channel_write_refusal(&ChannelWriteMembers {
        channel_role_arn: Some(role_arn),
        ..ChannelWriteMembers::default()
    })
    .map(ChannelRoleArnRefusal::from)
}

/// The three notification flags as ONE clause of the documented
/// `notifyOnCaseSeverity` rule reads them.
///
/// A clause reads the flags over the scope its own wording gives it, and the two
/// clauses of that rule are not worded over the same scope, so each is handed its own
/// set of values rather than sharing one (see [`notification_rule_violation`]). A flag
/// that is absent from the scope a clause reads counts as `false` for it: "null or
/// false" is exactly what the rule admits.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct NotificationFlags {
    pub notify_on_create_or_reopen_case: bool,
    pub notify_on_add_correspondence_to_case: bool,
    pub notify_on_resolve_case: bool,
}

impl NotificationFlags {
    /// The three flags read out of optional members, an absent one counting as
    /// `false`.
    pub fn new(
        notify_on_create_or_reopen_case: Option<bool>,
        notify_on_add_correspondence_to_case: Option<bool>,
        notify_on_resolve_case: Option<bool>,
    ) -> Self {
        Self {
            notify_on_create_or_reopen_case: notify_on_create_or_reopen_case.unwrap_or(false),
            notify_on_add_correspondence_to_case: notify_on_add_correspondence_to_case
                .unwrap_or(false),
            notify_on_resolve_case: notify_on_resolve_case.unwrap_or(false),
        }
    }

    /// Whether any of the three asks for a notification.
    pub fn any_set(&self) -> bool {
        self.notify_on_create_or_reopen_case
            || self.notify_on_add_correspondence_to_case
            || self.notify_on_resolve_case
    }
}

/// Why `severity` cannot hold together with the three notification flags, as the
/// message the refusal carries, or `None` when it can.
///
/// The rule is documented on `notifyOnCaseSeverity` on both requests that carry it,
/// and each of its two clauses is evaluated over the scope ITS OWN wording gives it:
///
/// * with `high` or `all`, "at least one of the following parameters must be `true`"
///   — worded over the configuration, so it reads `will_hold`: the values the record
///   will carry once the write lands (create applies its documented default of
///   `false` to an absent flag, update keeps the stored value, as that member's note
///   states for an unspecified parameter).
/// * with `none`, the UPDATE request words the clause over the REQUEST, both in its
///   antecedent and in its scope: "If you specify `none`, any of the following
///   parameters **that you specify in your request** must be `false`"
///   (`smithy_query(query="doc",shape="UpdateSlackChannelConfigurationRequest",member="notifyOnCaseSeverity")`),
///   so it reads `in_scope_for_none`: `Some` of the flags the request itself carried
///   when the request itself carried that severity, and `None` when it did not — a
///   request that specifies no severity keeps the stored one ("If you don't specify
///   these parameters in your request, the Amazon Web Services Support App uses the
///   current values by default") and says nothing this clause constrains. The CREATE
///   request words the same clause over the whole configuration ("the following
///   parameters must be null or false") because a create has no prior values to keep
///   — and there both scopes are the same set, since the request always carries the
///   severity and an unspecified flag is created `false`.
///
/// The message RESTATES that documented rule in service prose through
/// [`invalid_request_message`], so the operation that was called is named and the
/// parameters are named the way the API names them; `operation` is the caller's
/// `Operation::NAME`.
pub fn notification_rule_violation(
    operation: &str,
    severity: &str,
    will_hold: NotificationFlags,
    in_scope_for_none: Option<NotificationFlags>,
) -> Option<String> {
    let [create_or_reopen, add_correspondence, resolve] = NOTIFICATION_FLAG_PARAMETERS;
    let rule = match severity {
        "high" | "all" if !will_hold.any_set() => format!(
            "If NotifyOnCaseSeverity is all or high, you must enter true for at least \
             one of the following parameters: {create_or_reopen}, {add_correspondence} \
             or {resolve}."
        ),
        NOTIFICATION_SEVERITY_NONE if in_scope_for_none.is_some_and(|flags| flags.any_set()) => {
            format!(
                "If NotifyOnCaseSeverity is none, the following parameters must be null \
                 or false: {create_or_reopen}, {add_correspondence} or {resolve}."
            )
        }
        _ => return None,
    };
    Some(invalid_request_message(operation, &rule))
}

/// How this service's prose names the Slack channel configuration a request
/// identified: by the two members that identify it, `teamId` then `channelId`, in
/// the order the request carries them.
///
/// Both members are a Slack id of its own kind — a team id is not a workspace
/// configuration — so each is named for the member it came from. Every message about
/// one configuration is built from here, so the not-found and the already-exists
/// refusals cannot name the same resource two different ways.
pub fn slack_channel_configuration_subject(team_id: &str, channel_id: &str) -> String {
    format!("Slack channel configuration for team {team_id} and channel {channel_id}")
}

/// The `ResourceNotFoundException` message for a `(teamId, channelId)` pair this
/// account holds no configuration for ("The specified resource is missing or doesn't
/// exist, such as an account alias, Slack channel configuration, or Slack workspace
/// configuration" — `ResourceNotFoundException`). Shared by every operation that
/// looks one up, so they all state a miss the same way.
pub fn slack_channel_configuration_not_found(team_id: &str, channel_id: &str) -> String {
    format!(
        "{} not found",
        slack_channel_configuration_subject(team_id, channel_id)
    )
}

/// The `ResourceNotFoundException` message for a `teamId` this account holds no Slack
/// workspace configuration for ("The specified resource is missing or doesn't exist,
/// such as an account alias, Slack channel configuration, or Slack workspace
/// configuration" — `ResourceNotFoundException`).
///
/// Worded exactly as [`slack_channel_configuration_not_found`] words a channel miss,
/// naming the resource and the member that identifies it, so the two misses this
/// service can report read the same way and differ only in which resource was looked
/// up.
pub fn slack_workspace_configuration_not_found(team_id: &str) -> String {
    format!("Slack workspace configuration for team {team_id} not found")
}

/// The `ResourceNotFoundException` message for a `teamId` the Support Center console has
/// authorized no Slack workspace for ("The specified resource is missing or doesn't
/// exist, such as an account alias, Slack channel configuration, or Slack workspace
/// configuration" — `ResourceNotFoundException`).
///
/// Built from the request's `teamId` and the account the emulator presents, so a caller
/// reads which workspace was not found for which account, and it closes with the step
/// that makes such a workspace exist, as the operation documents it: "Sign in to the
/// Amazon Web Services Support Center and authorize the Slack workspaces where you want
/// your organization to have access to"
/// (`smithy_query(query="doc",shape="RegisterSlackWorkspaceForOrganization")`) — the
/// action a human has to take, which is what tells this miss apart from the conflicts the
/// same operation answers.
pub fn slack_workspace_not_authorized_message(team_id: &str) -> String {
    format!(
        "Slack workspace {team_id} not found for account {account}. Sign in to the Amazon Web \
         Services Support Center and authorize the Slack workspace before you register it.",
        account = ::engine::emulator_account_id()
    )
}

/// The `ConflictException` message for a `(teamId, channelId)` pair that is already
/// configured ("Add a Slack channel configuration that already exists in your Amazon
/// Web Services account" — `ConflictException`), naming the resource exactly as
/// [`slack_channel_configuration_not_found`] does.
pub fn slack_channel_configuration_exists(team_id: &str, channel_id: &str) -> String {
    format!(
        "{} already exists",
        slack_channel_configuration_subject(team_id, channel_id)
    )
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsSupportappSupportappState>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsSupportappSupportappState>)
}
