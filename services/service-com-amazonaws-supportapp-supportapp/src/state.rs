// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-supportapp-supportapp service.
//!
//! Besides the state struct itself this module holds the few rules the Support App
//! handlers share — the stored-record-to-modelled-shape conversion, the listing
//! pagination rule, the documented `notifyOnCaseSeverity` consistency rule, the account
//! a caller-supplied `channelRoleArn` must name, whether a Slack workspace is registered
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
    pub slack_channel_configurations: BTreeMap<(String, String), SlackChannelConfig>,
    /// The account alias `PutAccountAlias` wrote, which `GetAccountAlias` reports
    /// and `DeleteAccountAlias` clears. `None` until one is put: the account has no
    /// alias to report.
    pub account_alias: Option<String>,
    /// Every Slack workspace registered to this account, keyed by `teamId`, written by
    /// `RegisterSlackWorkspaceForOrganization`, removed by
    /// `DeleteSlackWorkspaceConfiguration` and bounded by
    /// [`MAX_SLACK_WORKSPACE_CONFIGURATIONS`]. It is the account's workspace registry:
    /// a Slack channel configuration lives INSIDE a registered workspace, so a create
    /// whose `teamId` is absent here has nothing to configure a channel in (see
    /// [`ComAmazonawsSupportappSupportappState::unregistered_workspace_conflict`]) and
    /// deleting a workspace takes that workspace's channel configurations with it.
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
    /// turn on it read it from here — the create path's
    /// [`Self::unregistered_workspace_conflict`] and
    /// `RegisterSlackWorkspaceForOrganization`'s member-account conflict and quota
    /// accounting — so the two handlers cannot come to disagree about whether a workspace
    /// is registered.
    pub fn workspace_is_registered(&self, team_id: &str) -> bool {
        self.slack_workspace_configurations.contains_key(team_id)
    }

    /// Why a request that ADDS a Slack channel configuration to the workspace `team_id`
    /// names must be refused with the modelled `ConflictException`, or `None` when that
    /// workspace is registered to this account.
    ///
    /// `ConflictException`'s documentation names this condition first — "Add, update, or
    /// delete a Slack channel configuration before you add a Slack workspace to your
    /// Amazon Web Services account" — and the create path is where it is the applicable
    /// answer: a `teamId` absent from [`Self::slack_workspace_configurations`] has no
    /// workspace for a new channel configuration to live in, and the configuration the
    /// request names cannot already exist for the account to report as found.
    ///
    /// An operation that acts on an EXISTING configuration settles that configuration's
    /// existence first instead, and answers the miss with
    /// [`slack_channel_configuration_not_found`]: a stored configuration always lives in
    /// a registered workspace — nothing else can be created, and
    /// `DeleteSlackWorkspaceConfiguration` takes a workspace's configurations with it —
    /// so this registry has no answer to add once the lookup has missed.
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
    /// workspace configuration" — and is settled, like every other lookup of the
    /// resource a request IDENTIFIES, before any precondition on the CALLING ACCOUNT.
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
pub fn pagination_token_refusal(operation: &str, next_token: Option<&str>) -> Option<String> {
    next_token.map(|_| invalid_pagination_token_message(operation))
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

/// Why the `channelRoleArn` a write carried cannot be used, as the message the
/// refusal carries, or `None` when the value names a role this account can pass.
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
pub fn channel_role_account_refusal(role_arn: &str) -> Option<String> {
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
/// Stated here because the expression has to be applied by hand (see
/// [`channel_role_account_refusal`]), and because the engine the framework's centralized
/// constraint walk compiles a `@pattern` with reads `.` as every character except a line
/// FEED alone, so the other three would be admitted there whatever the modelled expression
/// says.
const PATTERN_DOT_EXCLUDED_CHARS: [char; 4] = ['\n', '\r', '\u{2028}', '\u{2029}'];

/// Whether `role_name` is what the member's `(.+)` admits after `role/`, with the `$` that
/// anchors it: at least one character, and every one of them a character that expression's
/// `.` matches — so none of the [`PATTERN_DOT_EXCLUDED_CHARS`] it does not.
///
/// This is the whole of the member's form that is left once the prefix the account test
/// already pinned is stripped, so a value this rejects is outside the member's declared
/// `@pattern` and could not be reported back through the same shape.
fn is_role_name(role_name: &str) -> bool {
    !role_name.is_empty()
        && !role_name
            .chars()
            .any(|character| PATTERN_DOT_EXCLUDED_CHARS.contains(&character))
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

/// Why the `channelRoleArn` a write carried is too long to hold, or `None` when it fits.
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
pub fn channel_role_arn_length_refusal(role_arn: &str) -> Option<String> {
    if role_arn.chars().count() <= MAX_CHANNEL_ROLE_ARN_LEN {
        return None;
    }
    let violation = ::aws_smithy_schema_server::constraints::Violation {
        path: CHANNEL_ROLE_ARN_PATH.to_string(),
        kind: ::aws_smithy_schema_server::constraints::ViolationKind::MaxLength(
            i64::try_from(MAX_CHANNEL_ROLE_ARN_LEN).unwrap_or(i64::MAX),
        ),
        value: Some(::service_common::validation::quoted_echo(role_arn).into_owned()),
        sensitive: false,
    };
    ::aws_smithy_schema_server::constraints::envelope(::std::slice::from_ref(&violation))
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
/// * with `none`, the UPDATE request narrows the clause to "any of the following
///   parameters **that you specify in your request** must be `false`"
///   (`smithy_query(query="doc",shape="UpdateSlackChannelConfigurationRequest",member="notifyOnCaseSeverity")`),
///   so it reads `in_scope_for_none`: only the flags the request itself carried. The
///   CREATE request words the same clause over the whole configuration ("the
///   following parameters must be null or false") because a create has no prior
///   values to keep — and there both scopes are the same set, since an unspecified
///   flag is created `false`.
///
/// The message RESTATES that documented rule in service prose through
/// [`invalid_request_message`], so the operation that was called is named and the
/// parameters are named the way the API names them; `operation` is the caller's
/// `Operation::NAME`.
pub fn notification_rule_violation(
    operation: &str,
    severity: &str,
    will_hold: NotificationFlags,
    in_scope_for_none: NotificationFlags,
) -> Option<String> {
    let [create_or_reopen, add_correspondence, resolve] = NOTIFICATION_FLAG_PARAMETERS;
    let rule = match severity {
        "high" | "all" if !will_hold.any_set() => format!(
            "If NotifyOnCaseSeverity is all or high, you must enter true for at least \
             one of the following parameters: {create_or_reopen}, {add_correspondence} \
             or {resolve}."
        ),
        "none" if in_scope_for_none.any_set() => format!(
            "If NotifyOnCaseSeverity is none, the following parameters must be null \
             or false: {create_or_reopen}, {add_correspondence} or {resolve}."
        ),
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
