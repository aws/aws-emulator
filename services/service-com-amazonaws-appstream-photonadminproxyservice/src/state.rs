// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-appstream-photonadminproxyservice service.
//!
//! The stack/fleet lifecycle lives here: `CreateStack`/`CreateFleet` write a record,
//! `UpdateStack` applies the members it is given to the stored record and clears the
//! fields its `AttributesToDelete` names, `DescribeStacks`/`DescribeFleets` render the
//! stored record back, `AssociateFleet` records the fleet/stack edge,
//! `ListAssociatedFleets` reports the edges a stack has, `ListAssociatedStacks` reports
//! the edges a fleet has, and `DeleteStack`/`DeleteFleet` refuse while an edge still
//! names the resource.
//!
//! A stack's custom branding lives here as well: `CreateThemeForStack` writes one
//! [`ThemeRecord`] under the stack it brands, `UpdateThemeForStack` applies the members
//! it is given to that record and clears the fields its `AttributesToDelete` names,
//! `DescribeThemeForStack` renders the stored record back, and `DeleteThemeForStack`
//! removes it — as does deleting the stack itself, which is what the branding belongs to.
//!
//! App block builders live here as well: `CreateAppBlockBuilder` writes one
//! [`AppBlockBuilderRecord`] under the name it allocates and
//! `DescribeAppBlockBuilders` renders the stored records back, a page at a time.
//!
//! Usage reporting lives here too: it is ONE account-and-Region-wide setting, so
//! `CreateUsageReportSubscription` writes the single [`UsageReportSubscriptionRecord`]
//! this struct holds and `DescribeUsageReportSubscriptions` renders that same record
//! back.
//!
//! App blocks live here too: `CreateAppBlock` writes one [`AppBlockRecord`] under the
//! name it is created with, and `DescribeAppBlocks` renders the stored records the ARNs it
//! is asked about name back, a page at a time.
//!
//! Applications live here as well: `CreateApplication` resolves the app block the request
//! names against the stored app blocks and then writes one [`ApplicationRecord`] under the
//! name it allocates, `UpdateApplication` applies the members it is given to that record
//! and clears the fields its `AttributesToDelete` names, `DescribeApplications` renders
//! the stored records back, a page at
//! a time, and `DeleteApplication` removes one while nothing still holds it. Which
//! application an Elastic fleet streams lives here too: `AssociateApplicationFleet` writes
//! the `(fleet, application)` edge once the fleet's platform is one the application
//! supports, `DescribeApplicationFleetAssociations` reports the edges of one endpoint, a
//! page at a time, and `DisassociateApplicationFleet` removes one. An app block a stored
//! application still names as the source of its binaries is in use, which is what
//! `DeleteAppBlock` refuses on.
//!
//! A fleet's streaming state lives here too, with the transition the clock owes it:
//! `StartFleet` moves the stored record to `STARTING` and records the instant from which
//! `RUNNING` is due ([`PendingFleetState`]), `DescribeFleets` materialises what is already
//! due before it reports ([`ComAmazonawsAppstreamPhotonadminproxyserviceState::settle_fleet_states`]),
//! and `CreateStreamingURL` reads that same state as the gate on issuing a URL.
//!
//! Streaming sessions live here too: `CreateStreamingURL` dedicates one [`SessionRecord`]
//! per `(stack, fleet, user)` identity it issues a URL for, a GET of that URL lands on the
//! session's own address ([`streaming_session_endpoint_path`]), that session is expired by
//! the clock once the URL it was issued for stops verifying, and `DescribeSessions`
//! renders the sessions of one stack's fleet back, a page at a time.
//!
//! The user pool lives here as well: `CreateUser` writes a [`UserRecord`] under the
//! `(AuthenticationType, UserName)` identity it allocates, a create racing that allocation
//! collapses into it while one issued after the identity is established is refused
//! ([`UserRecord::is_established`]), `DisableUser` and `EnableUser`
//! move that record's sign-in flag
//! ([`ComAmazonawsAppstreamPhotonadminproxyserviceState::set_user_enabled`]), `DeleteUser`
//! removes the identity with what it entitled
//! ([`ComAmazonawsAppstreamPhotonadminproxyserviceState::remove_user`]), and
//! `DescribeUsers` renders the stored records of one authentication type back, a page at
//! a time.
//!
//! Which users may reach which stacks lives here too: `BatchAssociateUserStack` writes
//! one edge per association it applies, `BatchDisassociateUserStack` removes the edges it
//! is given, and `DescribeUserStackAssociations` reports the edges matching the stack or
//! the user identity it is asked about, a page at a time.
//!
//! Records hold the MODELLED sub-shapes (`StorageConnector`, `UserSetting`,
//! `VpcConfig`, …) rather than copies of them, so what a Describe returns is literally
//! what the Create persisted.
//!
//! A resource's tags live on that resource's own record, reached by ARN
//! ([`ComAmazonawsAppstreamPhotonadminproxyserviceState::resource_tags`]): the map a
//! `CreateStack`/`CreateFleet` persisted is the one `TagResource` adds to,
//! `UntagResource` removes from and `ListTagsForResource` reports, so there is no second
//! copy to drift.
//!
//! The image catalog lives here as well, in its own module: `DescribeImages` renders the
//! [`ImageRecord`]s this store holds — the base images AWS publishes, seeded into it, and
//! any image an operation of this API mints — a page at a time.

mod images;
mod wire_refusals;

pub use images::*;
pub use wire_refusals::*;

use std::collections::{BTreeMap, BTreeSet};

use crate::ops::types::{
    AccessEndpoint, AgentAccessConfig, AppBlock, AppBlockBuilder, AppBlockBuilderStateChangeReason,
    Application, ApplicationFleetAssociation, ApplicationSettingsResponse, ComputeCapacityStatus,
    ContentRedirection, DomainJoinInfo, ErrorDetails, Fleet, FleetError,
    LastReportGenerationExecutionError, ResourceError, S3Location, ScriptDetails, Session, Stack,
    StackError, StorageConnector, StreamingExperienceSettings, Theme, ThemeFooterLink,
    UsageReportSubscription, User, UserSetting, UserStackAssociation, UserStackAssociationError,
    VolumeConfig, VpcConfig,
};

/// The ARN namespace this service stamps on every resource ARN
/// (`OPERATIONS.md`: "ARN namespace: `appstream`").
pub const ARN_NAMESPACE: &str = "appstream";

/// The region an ARN carries when the request is unsigned, so no SigV4 credential
/// scope names one. Matches the region the emulator's own shared config declares.
pub const DEFAULT_REGION: &str = "us-east-1";

/// The `FleetType` a `CreateFleet` request that omits it gets. The model's doc for
/// `CreateFleet` states the service default is ON_DEMAND.
pub const DEFAULT_FLEET_TYPE: &str = "ON_DEMAND";

/// The `FleetState` a freshly created fleet carries: a fleet is created stopped and
/// starts streaming only once `StartFleet` runs, so this is a terminal status the clock
/// owes no transition out of.
pub const FLEET_STATE_CREATED: &str = "STOPPED";

/// The `FleetState` a fleet carries while the instances `StartFleet` asked for are coming
/// up, one of `FLEET_STATE_VALUES` (`ops/types/shared_types.rs`). It is not terminal: the
/// clock owes the fleet [`FLEET_STATE_RUNNING`] once [`FLEET_START_SPAN`] has passed.
pub const FLEET_STATE_STARTING: &str = "STARTING";

/// The `FleetState` of a fleet whose instances are up and streaming, which is the state
/// `CreateStreamingURL` requires of the stack's fleet.
pub const FLEET_STATE_RUNNING: &str = "RUNNING";

/// How long a fleet stays [`FLEET_STATE_STARTING`] before the clock owes it
/// [`FLEET_STATE_RUNNING`].
///
/// AWS takes minutes to bring streaming instances up; the span here is the emulator's
/// own, chosen so that a caller acting straight after `StartFleet` still observes the
/// transient state and the refusals that go with it, without any caller having to wait
/// minutes. Nothing sleeps for it: the due instant is stored on the fleet and the
/// transition is applied the next time the fleet is read.
pub const FLEET_START_SPAN: ::std::time::Duration = ::std::time::Duration::from_secs(3);

/// The `AppBlockBuilderState` a freshly created app block builder carries.
///
/// A builder is created stopped: `StartAppBlockBuilder`'s doc states that "an app block
/// builder can only be started when it's associated with an app block", and a create
/// associates none — so nothing the clock owes can start it, and `STOPPED` is a terminal
/// status here. The value is `AppBlockBuilderState`'s
/// (`APP_BLOCK_BUILDER_STATE_VALUES` in `ops/types/shared_types.rs`).
pub const APP_BLOCK_BUILDER_STATE_CREATED: &str = "STOPPED";

/// How many records one `DescribeStacks`/`DescribeFleets` page carries.
///
/// The model declares `NextToken` on both operations but publishes no page limit, so
/// this is the emulator's own page size; what matters on the wire is that a caller who
/// follows `NextToken` sees every record exactly once, in a stable order.
pub const DESCRIBE_PAGE_SIZE: usize = 50;

/// How many records one `DescribeAppBlockBuilders` page carries.
///
/// That operation's `MaxResults` is documented as "the maximum size of each page of
/// results. The maximum value is 25", so 25 is the page AWS itself publishes for this
/// read: a request asking for more is answered with this page and the token that resumes
/// after it.
pub const APP_BLOCK_BUILDER_PAGE_SIZE: usize = 25;

/// The maximum number of records this emulator keeps in ANY ONE of its caller-driven
/// stores (stacks, fleets, app blocks, app block builders, fleet/stack associations, user
/// pool users).
///
/// This is NOT AWS's service quota — AppStream's published stack and fleet limits are
/// per-account and adjustable, and nothing here claims to reproduce them. It is the
/// emulator's own memory ceiling: every successful `CreateStack`, `CreateFleet`,
/// `CreateAppBlock`, `CreateAppBlockBuilder`, `AssociateFleet`, `CreateUser` and
/// `BatchAssociateUserStack` inserts a record that
/// lives until a caller deletes it, so without a ceiling a caller could grow the process
/// until the host runs out of memory. Each of those handlers checks the store it writes
/// against this ceiling before inserting, and a request that would cross it is refused
/// with `LimitExceededException`, the modelled error AWS itself uses for "the requested
/// limit exceeds the permitted limit for an account", so the refusal is one a real client
/// already handles. A batch operation, whose modelled result reports per-item failures
/// instead of refusing the whole call, reports it as a per-item error
/// ([`ASSOCIATION_ERROR_INTERNAL_ERROR`]) carrying that same sentence.
pub const MAX_RECORDS_PER_STORE: usize = 1_000;

/// The message a store at [`MAX_RECORDS_PER_STORE`] refuses a new record with; `store`
/// names the records it holds, e.g. `"stacks"`.
pub fn limit_exceeded_message(store: &str) -> String {
    format!(
        "The requested limit exceeds the permitted limit for an account: \
         this emulator holds at most {MAX_RECORDS_PER_STORE} {store}"
    )
}

/// The word AWS's refusal sentences use for a stack (`"stack"`), the `<kind>` slot of
/// [`resource_not_found_message`] and [`resource_already_exists_message`].
pub const STACK_KIND: &str = "stack";

/// The word AWS's refusal sentences use for a fleet.
pub const FLEET_KIND: &str = "fleet";

/// The word AWS's refusal sentences use for an app block builder — one word, unlike the
/// hyphenated `resource-type` segment its ARN carries
/// ([`APP_BLOCK_BUILDER_ARN_RESOURCE_TYPE`]).
pub const APP_BLOCK_BUILDER_KIND: &str = "appblockbuilder";

/// The words AWS's refusal sentences use for an app block, the `<kind>` slot of
/// [`resource_not_found_message`] and [`resource_already_exists_message`].
pub const APP_BLOCK_KIND: &str = "app block";

/// The words AWS's `CreateApplication` refusal uses for the app block an application is
/// created against, where AWS capitalises the resource word and the `<name>` slot carries
/// the ARN the request named: `The App block <ARN> for account <account id> does not
/// exist.`
pub const APP_BLOCK_ARN_KIND: &str = "App block";

/// The word AWS's refusal sentences use for an application.
pub const APPLICATION_KIND: &str = "application";

/// The words AWS's refusal sentences use for a stack's custom branding theme.
///
/// A theme has no name of its own — the theme operations take only a `StackName` — so it
/// is identified by the stack it brands: the `<kind>` slot of
/// [`resource_not_found_message`] and [`resource_already_exists_message`] carries
/// `theme for stack` and the `<name>` slot that stack's name, giving AWS's sentence
/// `The theme for stack <stack name> for account <account id> does not exist.`
pub const THEME_KIND: &str = "theme for stack";

/// AWS's `ResourceNotFoundException` sentence for a stack that has no custom branding
/// theme: `The theme for stack <stack name> for account <account id>  does not exist.`
///
/// The two spaces after the account id are AWS's: the sentence is built from a template
/// whose slot between the account and `does not exist` is empty, and the doubled space
/// survives into the message clients see, exactly as it does in
/// [`resource_already_exists_message`]. The resource the sentence names is the theme and
/// the stack it brands, because a theme has no name of its own.
pub fn theme_not_found_message(stack_name: &str) -> String {
    let account = ::emulator_server::emulator_account_id();
    format!("The {THEME_KIND} {stack_name} for account {account}  does not exist.")
}

/// The `ThemeState` a freshly created theme carries.
///
/// `CreateThemeForStack` is what turns custom branding ON for a stack, and
/// `UpdateThemeForStack` is the only operation that models a `State` member (so it is
/// the only way to disable the branding again): a created theme is therefore ENABLED,
/// a terminal state the clock owes no transition out of. The value is `ThemeState`'s
/// (`THEME_STATE_VALUES` in `ops/types/shared_types.rs`).
pub const THEME_STATE_CREATED: &str = "ENABLED";

/// AWS's `ResourceAlreadyExistsException` sentence for a user pool identity that is
/// already taken.
///
/// Unlike the named control-plane resources above, this sentence has no variable part:
/// it states that the username is taken without echoing it back, so the refusal carries
/// nothing from the request.
pub const USER_ALREADY_EXISTS_MESSAGE: &str = "A user with that username already exists.";

/// The only `AuthenticationType` the user pool operations accept: the model's docs for
/// `CreateUser.AuthenticationType` and `DescribeUsers.AuthenticationType` both state
/// "You must specify USERPOOL", because the users these operations manage are the
/// account's user pool users rather than directory or API identities.
pub const USER_POOL_AUTHENTICATION_TYPE: &str = "USERPOOL";

/// The `Status` a freshly created user pool user carries. `User.Status`'s documented
/// values include "UNCONFIRMED - The user is created but not confirmed": a user becomes
/// CONFIRMED by setting their own password from the welcome email, which no clock makes
/// due, so this is where a created user stays until that happens.
pub const USER_STATUS_UNCONFIRMED: &str = "UNCONFIRMED";

/// AWS's sentence for a user pool identity that is not in the pool.
///
/// The mirror image of [`USER_ALREADY_EXISTS_MESSAGE`], and variable-part-free for the
/// same reason: the sentence states that no user has that username without echoing the
/// username back, so the refusal carries nothing from the request.
pub const USER_NOT_FOUND_MESSAGE: &str = "A user with that username does not exist.";

/// The `UserStackAssociationError.ErrorCode` a batch reports for an association naming a
/// stack this account does not hold, one of `USER_STACK_ASSOCIATION_ERROR_CODE_VALUES`
/// (`ops/types/shared_types.rs`).
pub const ASSOCIATION_ERROR_STACK_NOT_FOUND: &str = "STACK_NOT_FOUND";

/// The `ErrorCode` a batch reports for an association naming a user the pool does not
/// hold.
pub const ASSOCIATION_ERROR_USER_NAME_NOT_FOUND: &str = "USER_NAME_NOT_FOUND";

/// The `ErrorCode` a batch reports for an association it could not apply for a reason on
/// the service's side rather than in the request — here, the association store standing
/// at [`MAX_RECORDS_PER_STORE`].
pub const ASSOCIATION_ERROR_INTERNAL_ERROR: &str = "INTERNAL_ERROR";

/// How many user/stack associations one `DescribeUserStackAssociations` page carries.
///
/// The model caps `UserStackAssociationList` at 25 members and that list is what a page
/// of the read is rendered into, so a page holds at most 25 associations however large a
/// `MaxResults` a request asks for.
pub const USER_STACK_ASSOCIATION_PAGE_SIZE: usize = 25;

/// The sentence a request refused for naming an authentication type the user pool
/// operations do not manage carries; the only variable part is the value the request
/// asked for.
pub fn authentication_type_not_user_pool_message(authentication_type: &str) -> String {
    format!(
        "AuthenticationType must be {USER_POOL_AUTHENTICATION_TYPE} for user pool \
         operations, was {authentication_type}"
    )
}

/// AWS's `ResourceNotFoundException` sentence for a named control-plane resource:
/// `The <kind> <name> for account <account id> does not exist.`
///
/// `kind` is the resource word ([`STACK_KIND`], [`FLEET_KIND`]) and `name` the name the
/// request asked for; the account id is this emulator's own, so the sentence names the
/// account that was searched rather than a value from any one run.
pub fn resource_not_found_message(kind: &str, name: &str) -> String {
    let account = ::emulator_server::emulator_account_id();
    format!("The {kind} {name} for account {account} does not exist.")
}

/// AWS's `ResourceAlreadyExistsException` sentence for a name already taken:
/// `The <kind> <name> for account <account id>  already exists. Use a unique name within
/// an AWS region in your AWS account.`
///
/// The two spaces after the account id are AWS's: the sentence is built from a template
/// whose slot between the account and `already exists` is empty, and the doubled space
/// survives into the message clients see.
pub fn resource_already_exists_message(kind: &str, name: &str) -> String {
    let account = ::emulator_server::emulator_account_id();
    format!(
        "The {kind} {name} for account {account}  already exists. \
         Use a unique name within an AWS region in your AWS account."
    )
}

/// AWS's `ResourceInUseException` sentence for a fleet a stack still uses: it states the
/// association and the remediation, and names no stack — AWS does not enumerate the
/// stacks the fleet is associated with. The only variable part is the fleet name the
/// request asked for.
pub fn fleet_in_use_by_stack_message(fleet_name: &str) -> String {
    format!(
        "The fleet {fleet_name} is associated with a stack. To delete the fleet, \
         you must dissociate it from the stack."
    )
}

/// AWS's `ResourceInUseException` sentence for a stack that still has fleets associated
/// with it: it states what blocks the delete and what makes a stack deletable, and names
/// no fleet. The only variable part is the stack name the request asked for.
pub fn stack_in_use_by_fleets_message(stack_name: &str) -> String {
    format!(
        "Cannot delete stack {stack_name}. This stack has fleets associated with it. \
         Only stacks with no fleets associated with it can be deleted."
    )
}

/// The service principal AppStream assumes a caller-supplied IAM role as, which is what a
/// role's trust policy has to admit for the service to be able to use it.
pub const SERVICE_PRINCIPAL: &str = "appstream.amazonaws.com";

/// The signing name this service is addressed under (the model's `sigv4Name`,
/// `appstream`), which is what a URL leading back to this service is signed as.
pub const SIGNING_NAME: &str = "appstream";

/// The path a streaming URL addresses, before the stack, fleet and user it was issued
/// for: `/<prefix>/<stack>/<fleet>/<user id>`.
pub const STREAMING_SESSION_PATH_PREFIX: &str = "streaming-sessions";

/// The path a `CreateStreamingURL` signs: the identity the session was dedicated for,
/// which is also the address the route serving that URL reads the identity back out of —
/// ONE spelling, so the URL handed out and the route that resolves it cannot drift.
pub fn streaming_url_path(stack_name: &str, fleet_name: &str, user_id: &str) -> String {
    format!("/{STREAMING_SESSION_PATH_PREFIX}/{stack_name}/{fleet_name}/{user_id}")
}

/// The segment that tells a session's own address apart from the address a streaming URL
/// is signed for: `/<prefix>/session/<session id>` against
/// `/<prefix>/<stack>/<fleet>/<user id>`.
pub const STREAMING_SESSION_ENDPOINT_SEGMENT: &str = "session";

/// The query field a session endpoint names the fleet the session was allocated against
/// in, and the `<resource type>/<name>` reference it carries: AppStream's session address
/// identifies the resource streaming the session as `fleet/<fleet name>`, percent-encoded
/// in the URL.
pub const STREAMING_SESSION_REFERENCE_FIELD: &str = "reference";

/// The address of ONE dedicated session: where a GET of the streaming URL issued for it
/// lands, carrying the session's own id and the percent-encoded `fleet/<fleet name>`
/// reference of the fleet the session was allocated against.
///
/// A relative reference, so the session is reached on whatever address the caller reached
/// the emulator on — the browser that follows the redirect resolves it against the URL it
/// was handed.
pub fn streaming_session_endpoint_path(session_id: &str, fleet_name: &str) -> String {
    format!(
        "/{STREAMING_SESSION_PATH_PREFIX}/{STREAMING_SESSION_ENDPOINT_SEGMENT}/{session_id}\
         ?{STREAMING_SESSION_REFERENCE_FIELD}={FLEET_ARN_RESOURCE_TYPE}%2F{fleet_name}"
    )
}

/// How long a streaming URL stays valid when the request names no `Validity`.
///
/// `CreateStreamingURL.Validity` is documented "The time that the streaming URL will be
/// valid, in seconds. Specify a value between 1 and 604800 seconds. The default is 60
/// seconds", so this is AWS's own default rather than a span chosen here.
pub const STREAMING_URL_DEFAULT_VALIDITY_SECONDS: i64 = 60;

/// The shortest `Validity` that member's documentation allows ("between 1 and 604800
/// seconds").
pub const STREAMING_URL_MIN_VALIDITY_SECONDS: i64 = 1;

/// The longest `Validity` that member's documentation allows ("between 1 and 604800
/// seconds"), which is seven days.
pub const STREAMING_URL_MAX_VALIDITY_SECONDS: i64 = 604_800;

/// The sentence a request asking for a validity outside the documented range is refused
/// with; its variable part is the value the request asked for.
pub fn streaming_url_validity_message(validity: i64) -> String {
    format!(
        "Validity must be between {STREAMING_URL_MIN_VALIDITY_SECONDS} and \
         {STREAMING_URL_MAX_VALIDITY_SECONDS} seconds, was {validity}"
    )
}

/// AWS's `InvalidRoleException` sentence for an `IamRoleArn` the service cannot assume:
/// the ARN is quoted in single quotes and the sentence carries no trailing period. The
/// only variable part is the ARN the request asked for.
pub fn invalid_role_message(role_arn: &str) -> String {
    format!(
        "WorkSpaces Applications encountered an error because your IAM role \
         '{role_arn}' is invalid"
    )
}

/// AWS's `ResourceNotAvailableException` sentence, as this service's model documents that
/// error shape (`smithy_query(query="doc", shape="ResourceNotAvailableException")`: "The
/// specified resource exists and is not in use, but isn't available."): the one wording a
/// request refused because the resource it needs cannot serve it carries, naming the rule
/// and nothing of the request.
pub fn resource_not_available_message() -> String {
    "The specified resource exists and is not in use, but isn't available.".to_string()
}

/// AWS's `OperationNotPermittedException` sentence for a streaming URL asked for against
/// a fleet that is not streaming: it states the fleet's condition and the rule, with no
/// trailing period. The only variable part is the fleet name the request asked for.
pub fn fleet_not_running_message(fleet_name: &str) -> String {
    format!(
        "The fleet {fleet_name} is not running. The stack must have a running fleet \
         to get a streaming URL"
    )
}

/// AWS's `ResourceInUseException` sentence for an app block another resource still holds:
/// it states that the app block is in use and names it, and names nothing that holds it.
/// The only variable part is the app block name the request asked for.
pub fn app_block_in_use_message(name: &str) -> String {
    format!("The app-block {name} is currently in use.")
}

/// The same sentence for an application another resource still holds, which is what
/// blocks deleting it: the app-block wording with the resource word the condition is
/// about.
pub fn application_in_use_message(name: &str) -> String {
    format!("The application {name} is currently in use.")
}

/// The sentence a request refused for associating an application with a fleet whose
/// platform the application does not support carries.
///
/// Fixed text is AWS's; the variable parts are the ARN and the fleet name the request
/// named, the fleet's own `Platform` and the `Platforms` the stored application supports.
pub fn application_platform_mismatch_message(
    application_arn: &str,
    fleet_name: &str,
    fleet_platform: &str,
    application_platforms: &[String],
) -> String {
    let platforms = application_platforms.join(", ");
    format!(
        "Application '{application_arn}' may not be associated with fleet '{fleet_name}' \
         because fleet Platform '{fleet_platform}' is not supported by application \
         Platforms: [{platforms}]"
    )
}

/// The `FleetType` an application may be associated with: `AssociateApplicationFleet` is
/// documented "This is only supported for Elastic fleets", so a fleet of any other type
/// is one no application can be attached to. One of `FLEET_TYPE_VALUES`
/// (`ops/types/shared_types.rs`).
pub const ELASTIC_FLEET_TYPE: &str = "ELASTIC";

/// The sentence a request refused for associating an application with a fleet that is not
/// an Elastic fleet carries: the operation's own documented rule, with the fleet named.
pub fn fleet_not_elastic_message(fleet_name: &str, fleet_type: &str) -> String {
    format!(
        "Fleet {fleet_name} is a {fleet_type} fleet. Associating an application with a \
         fleet is only supported for Elastic fleets."
    )
}

/// The sentence a read of the application/fleet associations is refused with when it names
/// neither endpoint, or both: `DescribeApplicationFleetAssociations`'s own documentation
/// states "Either ApplicationArn or FleetName must be specified", so exactly one of them
/// selects what the read reports.
pub fn application_fleet_association_selector_message() -> String {
    "Either ApplicationArn or FleetName must be specified.".to_string()
}

/// How many tags ONE resource may carry.
///
/// The model bounds a single `TagResource` request's `Tags` map and a single
/// `UntagResource` request's `TagKeys` list at 50 entries
/// (`smithy_query(query="shape", shape="Tags")`: `length 1..50`), and the same 50 is the
/// per-resource ceiling: tagging that would leave a resource carrying more than this many
/// tags is refused with `LimitExceededException`, the modelled error for "the requested
/// limit exceeds the permitted limit for an account", and persists nothing.
pub const MAX_TAGS_PER_RESOURCE: usize = 50;

/// AWS's `LimitExceededException` sentence, as this service's model documents that error
/// shape (`smithy_query(query="doc", shape="LimitExceededException")`: "The requested
/// limit exceeds the permitted limit for an account."): the one wording a request refused
/// for crossing a limit carries, naming the rule and nothing of the request.
pub fn tags_limit_exceeded_message() -> String {
    "The requested limit exceeds the permitted limit for an account.".to_string()
}

/// The ARN `resource-type` segment of a stack, the segment before the stack's name in
/// `arn:aws:appstream:<region>:<account>:stack/<name>`.
pub const STACK_ARN_RESOURCE_TYPE: &str = "stack";

/// The ARN `resource-type` segment of a fleet.
pub const FLEET_ARN_RESOURCE_TYPE: &str = "fleet";

/// The ARN `resource-type` segment of an app block builder.
pub const APP_BLOCK_BUILDER_ARN_RESOURCE_TYPE: &str = "app-block-builder";

/// The ARN `resource-type` segment of an app block, the segment before the app block's
/// name in `arn:aws:appstream:<region>:<account>:app-block/<name>`.
pub const APP_BLOCK_ARN_RESOURCE_TYPE: &str = "app-block";

/// The ARN `resource-type` segment of an application.
pub const APPLICATION_ARN_RESOURCE_TYPE: &str = "application";

/// The `<kind>` word a refusal uses for an ARN whose resource part carries no
/// `<resource-type>/<resource-id>` to name a kind with.
pub const UNTYPED_RESOURCE_KIND: &str = "resource";

/// The `resource-type` and `resource-id` an ARN names, from its sixth colon-separated
/// field: `arn:<partition>:<service>:<region>:<account>:<resource-type>/<resource-id>`,
/// where the separator between the type and the id is `/` or `:`.
///
/// An ARN whose resource part carries no type (`…:<resource-id>`) answers
/// [`UNTYPED_RESOURCE_KIND`] and the whole resource part, so a caller is still told which
/// resource was searched for.
pub fn arn_resource(arn: &str) -> (String, String) {
    let Some(resource) = arn.splitn(6, ':').nth(5) else {
        return (UNTYPED_RESOURCE_KIND.to_string(), arn.to_string());
    };
    match resource.split_once(['/', ':']) {
        Some((resource_type, resource_id))
            if !resource_type.is_empty() && !resource_id.is_empty() =>
        {
            (resource_type.to_string(), resource_id.to_string())
        }
        _ => (UNTYPED_RESOURCE_KIND.to_string(), resource.to_string()),
    }
}

/// AWS's `ResourceNotFoundException` sentence for a resource addressed by ARN: the same
/// `The <kind> <name> for account <account id> does not exist.` template every named
/// resource is refused with ([`resource_not_found_message`]), with the kind and the name
/// read out of the ARN.
///
/// The ARN's `resource-type` is the kind, spelled the way AWS's sentences spell it — the
/// hyphens an ARN segment uses between words are spaces in a sentence.
pub fn tagged_resource_not_found_message(resource_arn: &str) -> String {
    let (resource_type, resource_id) = arn_resource(resource_arn);
    let kind = match resource_type.as_str() {
        STACK_ARN_RESOURCE_TYPE => STACK_KIND.to_string(),
        FLEET_ARN_RESOURCE_TYPE => FLEET_KIND.to_string(),
        APP_BLOCK_BUILDER_ARN_RESOURCE_TYPE => APP_BLOCK_BUILDER_KIND.to_string(),
        APP_BLOCK_ARN_RESOURCE_TYPE => APP_BLOCK_KIND.to_string(),
        APPLICATION_ARN_RESOURCE_TYPE => APPLICATION_KIND.to_string(),
        other => other.replace('-', " "),
    };
    resource_not_found_message(&kind, &resource_id)
}

/// AWS's `ResourceNotFoundException` sentence for an app block this account does not
/// hold, named by the ARN the request addressed it with: `The App block <ARN> for account
/// <account id> does not exist.`
///
/// An app block is addressed by ARN wherever it is addressed at all — `DescribeAppBlocks`
/// takes `Arns`, `CreateApplication` an `AppBlockArn` — so the refusal names the ARN the
/// request gave rather than the name inside it, under the capitalised resource word AWS
/// spells that sentence with ([`APP_BLOCK_ARN_KIND`]). `DeleteAppBlock` names the app
/// block by NAME, so it refuses with the ARN that name addresses in the caller's account
/// and Region: an app block that is gone is unusable under either address.
pub fn app_block_not_found_message(app_block_arn: &str) -> String {
    resource_not_found_message(APP_BLOCK_ARN_KIND, app_block_arn)
}

/// The `Schedule` a usage report subscription carries. The model's
/// `UsageReportSchedule` enum has the single value `DAILY`
/// (`USAGE_REPORT_SCHEDULE_VALUES` in `ops/types/shared_types.rs`), matching the
/// operation's documented "Usage reports are generated daily".
pub const USAGE_REPORT_SCHEDULE: &str = "DAILY";

/// The region to stamp into the ARNs a request mints: the SigV4 credential scope's
/// region, else [`DEFAULT_REGION`].
pub fn request_region(parts: &::http::request::Parts) -> String {
    ::emulator_server::sigv4_credential_scope(&parts.headers)
        .map(|scope| scope.region)
        .unwrap_or_else(|| DEFAULT_REGION.to_string())
}

/// `arn:aws:appstream:<region>:<account>:stack/<name>`.
pub fn stack_arn(region: &str, name: &str) -> String {
    ::emulator_server::emulator_arn(ARN_NAMESPACE, region, &format!("stack/{name}"))
}

/// `arn:aws:appstream:<region>:<account>:fleet/<name>`.
pub fn fleet_arn(region: &str, name: &str) -> String {
    ::emulator_server::emulator_arn(ARN_NAMESPACE, region, &format!("fleet/{name}"))
}

/// `arn:aws:appstream:<region>:<account>:app-block-builder/<name>`.
pub fn app_block_builder_arn(region: &str, name: &str) -> String {
    ::emulator_server::emulator_arn(
        ARN_NAMESPACE,
        region,
        &format!("{APP_BLOCK_BUILDER_ARN_RESOURCE_TYPE}/{name}"),
    )
}

/// `arn:aws:appstream:<region>:<account>:app-block/<name>`.
pub fn app_block_arn(region: &str, name: &str) -> String {
    ::emulator_server::emulator_arn(
        ARN_NAMESPACE,
        region,
        &format!("{APP_BLOCK_ARN_RESOURCE_TYPE}/{name}"),
    )
}

/// `arn:aws:appstream:<region>:<account>:application/<name>`.
pub fn application_arn(region: &str, name: &str) -> String {
    ::emulator_server::emulator_arn(
        ARN_NAMESPACE,
        region,
        &format!("{APPLICATION_ARN_RESOURCE_TYPE}/{name}"),
    )
}

/// `arn:aws:appstream:<region>:<account>:user/<user pool type>/<user name>`.
///
/// AppStream's `user` resource ARN names the user pool an identity belongs to before
/// the user's own id, and that pool segment is the authentication type in lower case.
pub fn user_arn(region: &str, authentication_type: &str, user_name: &str) -> String {
    let pool = authentication_type.to_lowercase();
    ::emulator_server::emulator_arn(ARN_NAMESPACE, region, &format!("user/{pool}/{user_name}"))
}

/// The S3 bucket a usage report subscription stores its generated reports in.
///
/// AppStream names the bucket it creates for log and usage-report output
/// `appstream-logs-<region>-<account>-<identifier>`, where the trailing identifier is
/// what makes the name globally unique. The model documents that bucket as "unique to
/// your account and Region", so the identifier here is a digest of exactly those two:
/// enabling usage reporting again for the same account and Region reports the SAME
/// bucket rather than a name that changes from run to run.
pub fn usage_report_bucket_name(region: &str) -> String {
    let account = ::emulator_server::emulator_account_id();
    let digest =
        <::sha2::Sha256 as ::sha2::Digest>::digest(format!("appstream-logs:{region}:{account}"));
    let identifier: String = hex::encode(digest).chars().take(8).collect();
    format!("appstream-logs-{region}-{account}-{identifier}")
}

/// The instant a record is created, as the wire type the modelled `CreatedTime`
/// members carry.
pub fn now() -> ::aws_smithy_types::DateTime {
    let since_epoch = ::std::time::SystemTime::now()
        .duration_since(::std::time::UNIX_EPOCH)
        .unwrap_or_default();
    ::aws_smithy_types::DateTime::from_secs_f64(since_epoch.as_secs_f64())
}

/// A persisted AppStream stack.
#[derive(Debug, Clone)]
pub struct StackRecord {
    pub arn: String,
    pub name: String,
    pub description: Option<String>,
    pub display_name: Option<String>,
    pub created_time: ::aws_smithy_types::DateTime,
    pub storage_connectors: Vec<StorageConnector>,
    pub redirect_url: Option<String>,
    pub feedback_url: Option<String>,
    pub user_settings: Vec<UserSetting>,
    pub application_settings: Option<ApplicationSettingsResponse>,
    pub access_endpoints: Vec<AccessEndpoint>,
    pub embed_host_domains: Vec<String>,
    pub streaming_experience_settings: Option<StreamingExperienceSettings>,
    pub content_redirection: Option<ContentRedirection>,
    pub agent_access_config: Option<AgentAccessConfig>,
    pub storage_connector_authentication: Option<String>,
    pub stack_errors: Vec<StackError>,
    pub tags: BTreeMap<String, String>,
}

/// Render a stored configuration collection the way the resource reports it: a
/// collection with no entries is a configuration the stack does not have, so the member
/// is ABSENT from the rendered resource rather than present-and-empty.
///
/// This is the representation side of clearing a stack attribute: once an `UpdateStack`
/// request drops a stack's access endpoints, user settings, embedded host domains or
/// storage connectors, the corresponding key is gone from the returned `Stack` and from
/// every later `DescribeStacks`, exactly as it is for a stack that was never given one.
fn reported<T: Clone>(entries: &[T]) -> Option<Vec<T>> {
    (!entries.is_empty()).then(|| entries.to_vec())
}

impl StackRecord {
    /// Render the stored record as the modelled `Stack` every reader returns.
    pub fn to_stack(&self) -> Stack {
        Stack {
            arn: Some(self.arn.clone()),
            name: self.name.clone(),
            description: self.description.clone(),
            display_name: self.display_name.clone(),
            created_time: Some(self.created_time),
            storage_connectors: reported(&self.storage_connectors),
            redirect_url: self.redirect_url.clone(),
            feedback_url: self.feedback_url.clone(),
            // A stack always reports its error list; an empty one says "no errors",
            // which is a value, not a missing configuration.
            stack_errors: Some(self.stack_errors.clone()),
            user_settings: reported(&self.user_settings),
            application_settings: self.application_settings.clone(),
            access_endpoints: reported(&self.access_endpoints),
            embed_host_domains: reported(&self.embed_host_domains),
            streaming_experience_settings: self.streaming_experience_settings.clone(),
            content_redirection: self.content_redirection.clone(),
            agent_access_config: self.agent_access_config.clone(),
            storage_connector_authentication: self.storage_connector_authentication.clone(),
        }
    }
}

/// The URL a theme reports for one of its branding assets: the virtual-hosted-style URL
/// of the S3 object the request named.
///
/// A theme's logo and favicon are given as an `S3Location` (a bucket and a key) and
/// reported back as URLs, so the URL is DERIVED from that location: the bucket's
/// regional endpoint followed by the object key. Nothing is copied or fetched — the
/// object belongs to S3 and keeps its own lifecycle — so what this addresses is exactly
/// the object the caller pointed at, and a later read reports the same URL. `region` is
/// the request's own ([`request_region`]), the Region whose endpoint addresses a bucket.
pub fn theme_asset_url(region: &str, location: &S3Location) -> String {
    let bucket = &location.s3_bucket;
    let key = location.s3_key.as_deref().unwrap_or_default();
    format!("https://{bucket}.s3.{region}.amazonaws.com/{key}")
}

/// A persisted AppStream custom branding theme — one stack's catalog-page branding,
/// written by `CreateThemeForStack` and rendered back by `DescribeThemeForStack`.
#[derive(Debug, Clone)]
pub struct ThemeRecord {
    pub stack_name: String,
    pub state: String,
    pub title_text: String,
    pub styling: String,
    pub footer_links: Vec<ThemeFooterLink>,
    pub organization_logo_url: String,
    pub favicon_url: String,
    pub created_time: ::aws_smithy_types::DateTime,
}

impl ThemeRecord {
    /// Render the stored record as the modelled `Theme` every reader returns.
    pub fn to_theme(&self) -> Theme {
        Theme {
            stack_name: Some(self.stack_name.clone()),
            state: Some(self.state.clone()),
            theme_title_text: Some(self.title_text.clone()),
            theme_styling: Some(self.styling.clone()),
            // A theme with no footer links is a theme that displays none, so the member
            // is ABSENT rather than present-and-empty — the same representation the
            // stack's own collections use.
            theme_footer_links: reported(&self.footer_links),
            theme_organization_logo_url: Some(self.organization_logo_url.clone()),
            theme_favicon_url: Some(self.favicon_url.clone()),
            created_time: Some(self.created_time),
        }
    }
}

/// A persisted AppStream fleet.
#[derive(Debug, Clone)]
pub struct FleetRecord {
    pub arn: String,
    pub name: String,
    pub instance_type: String,
    pub fleet_type: String,
    pub image_name: Option<String>,
    pub image_arn: Option<String>,
    /// One of `FLEET_STATE_VALUES`; `STOPPED` for a freshly created fleet.
    pub state: String,
    /// The state the clock owes this fleet, and the instant from which it is owed: what
    /// `StartFleet` records so that a fleet reports `STARTING` until its instances are
    /// up and `RUNNING` afterwards, without anything sleeping for the span.
    pub pending_state: Option<PendingFleetState>,
    pub desired_instances: i32,
    pub desired_user_sessions: Option<i32>,
    pub running: i32,
    pub in_use: i32,
    pub available: i32,
    pub vpc_config: Option<VpcConfig>,
    pub description: Option<String>,
    pub display_name: Option<String>,
    pub max_user_duration_in_seconds: Option<i32>,
    pub disconnect_timeout_in_seconds: Option<i32>,
    pub idle_disconnect_timeout_in_seconds: Option<i32>,
    pub max_concurrent_sessions: Option<i32>,
    pub max_sessions_per_instance: Option<i32>,
    pub stream_view: Option<String>,
    pub platform: Option<String>,
    pub enable_default_internet_access: Option<bool>,
    pub domain_join_info: Option<DomainJoinInfo>,
    pub iam_role_arn: Option<String>,
    pub usb_device_filter_strings: Vec<String>,
    pub session_script_s3_location: Option<S3Location>,
    pub root_volume_config: Option<VolumeConfig>,
    pub disable_imdsv1: Option<bool>,
    pub created_time: ::aws_smithy_types::DateTime,
    pub fleet_errors: Vec<FleetError>,
    pub tags: BTreeMap<String, String>,
}

/// A fleet-state transition the clock owes: the state the fleet moves to, and the instant
/// at which it is due.
#[derive(Debug, Clone, Copy)]
pub struct PendingFleetState {
    pub state: &'static str,
    pub due_at: ::std::time::Instant,
}

impl FleetRecord {
    /// Apply the transition the clock owes this fleet, if `now` is past its due instant.
    ///
    /// This materialises a change the CLOCK already made due — the fleet would have
    /// reached the state at that instant even if nobody had called — so a read may apply
    /// it. Nothing else about the fleet changes, and once applied there is nothing owed.
    pub fn settle_state(&mut self, now: ::std::time::Instant) {
        let Some(pending) = self.pending_state else {
            return;
        };
        if now < pending.due_at {
            return;
        }
        self.state = pending.state.to_string();
        self.pending_state = None;
        // The instances the start asked for are up once the fleet is running, and none of
        // them is serving a session yet.
        if self.state == FLEET_STATE_RUNNING {
            self.running = self.desired_instances;
            self.available = self.desired_instances.saturating_sub(self.in_use);
        }
    }

    /// Whether this fleet is streaming, which is what issuing a streaming URL against the
    /// stack it is associated with requires of it.
    pub fn is_running(&self) -> bool {
        self.state == FLEET_STATE_RUNNING
    }

    /// Render the stored record as the modelled `Fleet` every reader returns.
    pub fn to_fleet(&self) -> Fleet {
        Fleet {
            arn: self.arn.clone(),
            name: self.name.clone(),
            display_name: self.display_name.clone(),
            description: self.description.clone(),
            image_name: self.image_name.clone(),
            image_arn: self.image_arn.clone(),
            instance_type: self.instance_type.clone(),
            fleet_type: Some(self.fleet_type.clone()),
            compute_capacity_status: ComputeCapacityStatus {
                desired: self.desired_instances,
                running: Some(self.running),
                in_use: Some(self.in_use),
                available: Some(self.available),
                desired_user_sessions: self.desired_user_sessions,
                ..ComputeCapacityStatus::default()
            },
            max_user_duration_in_seconds: self.max_user_duration_in_seconds,
            disconnect_timeout_in_seconds: self.disconnect_timeout_in_seconds,
            state: self.state.clone(),
            vpc_config: self.vpc_config.clone(),
            created_time: Some(self.created_time),
            fleet_errors: Some(self.fleet_errors.clone()),
            enable_default_internet_access: self.enable_default_internet_access,
            domain_join_info: self.domain_join_info.clone(),
            idle_disconnect_timeout_in_seconds: self.idle_disconnect_timeout_in_seconds,
            iam_role_arn: self.iam_role_arn.clone(),
            stream_view: self.stream_view.clone(),
            platform: self.platform.clone(),
            max_concurrent_sessions: self.max_concurrent_sessions,
            usb_device_filter_strings: Some(self.usb_device_filter_strings.clone()),
            session_script_s3_location: self.session_script_s3_location.clone(),
            max_sessions_per_instance: self.max_sessions_per_instance,
            root_volume_config: self.root_volume_config.clone(),
            disable_imdsv1: self.disable_imdsv1,
        }
    }
}

/// A persisted AppStream app block builder — the instance that packages applications
/// into app blocks, written by `CreateAppBlockBuilder` and rendered back by
/// `DescribeAppBlockBuilders`.
#[derive(Debug, Clone)]
pub struct AppBlockBuilderRecord {
    pub arn: String,
    pub name: String,
    pub display_name: Option<String>,
    pub description: Option<String>,
    /// One of `APP_BLOCK_BUILDER_PLATFORM_TYPE_VALUES`.
    pub platform: String,
    pub instance_type: String,
    pub enable_default_internet_access: Option<bool>,
    pub iam_role_arn: Option<String>,
    /// The MODELLED sub-shape rather than a copy of its members, so what a Describe
    /// returns is literally what the create persisted.
    pub vpc_config: VpcConfig,
    /// One of `APP_BLOCK_BUILDER_STATE_VALUES`; [`APP_BLOCK_BUILDER_STATE_CREATED`] for a
    /// freshly created builder.
    pub state: String,
    pub created_time: ::aws_smithy_types::DateTime,
    /// The errors that stopped the builder reaching its state; empty while none has
    /// occurred.
    pub app_block_builder_errors: Vec<ResourceError>,
    /// Why the builder last changed state; `None` until something changed it.
    pub state_change_reason: Option<AppBlockBuilderStateChangeReason>,
    pub access_endpoints: Vec<AccessEndpoint>,
    pub disable_imdsv1: Option<bool>,
    pub tags: BTreeMap<String, String>,
}

impl AppBlockBuilderRecord {
    /// Render the stored record as the modelled `AppBlockBuilder` every reader returns.
    pub fn to_app_block_builder(&self) -> AppBlockBuilder {
        AppBlockBuilder {
            arn: self.arn.clone(),
            name: self.name.clone(),
            display_name: self.display_name.clone(),
            description: self.description.clone(),
            platform: self.platform.clone(),
            instance_type: self.instance_type.clone(),
            enable_default_internet_access: self.enable_default_internet_access,
            iam_role_arn: self.iam_role_arn.clone(),
            vpc_config: self.vpc_config.clone(),
            state: self.state.clone(),
            created_time: Some(self.created_time),
            // A builder with no errors and no state change behind it reports neither:
            // these members describe something that happened, so until it has, they are
            // ABSENT rather than present-and-empty.
            app_block_builder_errors: reported(&self.app_block_builder_errors),
            state_change_reason: self.state_change_reason.clone(),
            access_endpoints: reported(&self.access_endpoints),
            disable_imdsv1: self.disable_imdsv1,
        }
    }
}

/// The `PackagingType` an app block created without one carries.
///
/// The packaging type is what decides which script members a create may carry
/// (`SetupScriptDetails` "must be provided for the CUSTOM PackagingType",
/// `PostSetupScriptDetails` "can only be provided for the APPSTREAM2 PackagingType") and
/// the state the app block starts in, so the resource always reports one. `CUSTOM` is the
/// packaging that needs no app block builder to produce its application package: a
/// request that names no packaging type is one whose virtual hard disk the caller brings
/// itself. The value is `PackagingType`'s (`PACKAGING_TYPE_VALUES` in
/// `ops/types/shared_types.rs`).
pub const PACKAGING_TYPE_CUSTOM: &str = "CUSTOM";

/// The packaging an app block whose application package an app block builder produces
/// carries, the other `PackagingType` value.
pub const PACKAGING_TYPE_APPSTREAM2: &str = "APPSTREAM2";

/// The `AppBlockState` of an app block that can be used as it stands, one of
/// `APP_BLOCK_STATE_VALUES` (`ops/types/shared_types.rs`).
pub const APP_BLOCK_STATE_ACTIVE: &str = "ACTIVE";

/// The `AppBlockState` of an app block that has no application package yet.
pub const APP_BLOCK_STATE_INACTIVE: &str = "INACTIVE";

/// The state a freshly created app block carries.
///
/// `AppBlock.State`'s documentation states the rule: an app block with WorkSpaces
/// Applications packaging "will be in the INACTIVE state if no application package (VHD)
/// is assigned to it" and becomes ACTIVE once an app block builder has created one, while
/// "Custom app blocks are always in the ACTIVE state". What assigns a package at create
/// time is the source location's `S3Key`, which `S3Location.S3Key`'s documentation
/// describes as referring to the VHD file when an existing application package is used
/// and as not required when a new one is to be built. So the only app block created
/// INACTIVE is an APPSTREAM2 one whose source names no object yet — and it leaves that
/// state when a builder produces the package, which no clock makes due.
pub fn app_block_state_created(packaging_type: &str, source: &S3Location) -> &'static str {
    let has_application_package = source.s3_key.as_deref().is_some_and(|key| !key.is_empty());
    if packaging_type == PACKAGING_TYPE_APPSTREAM2 && !has_application_package {
        APP_BLOCK_STATE_INACTIVE
    } else {
        APP_BLOCK_STATE_ACTIVE
    }
}

/// The sentence a create refused for carrying no setup script where the packaging needs
/// one carries: `SetupScriptDetails` "must be provided for the CUSTOM PackagingType", and
/// a custom app block's setup script is what says how to mount its virtual hard disk.
pub fn setup_script_details_required_message() -> String {
    format!("SetupScriptDetails must be provided for the {PACKAGING_TYPE_CUSTOM} PackagingType")
}

/// The sentence a create refused for carrying a post setup script where the packaging
/// admits none carries: `PostSetupScriptDetails` "can only be provided for the APPSTREAM2
/// PackagingType". The only variable part is the packaging type the request asked for.
pub fn post_setup_script_details_not_permitted_message(packaging_type: &str) -> String {
    format!(
        "PostSetupScriptDetails can only be provided for the {PACKAGING_TYPE_APPSTREAM2} \
         PackagingType, was {packaging_type}"
    )
}

/// A persisted AppStream app block — the virtual hard disk an elastic fleet's
/// applications are served from, written by `CreateAppBlock` and rendered back by
/// `DescribeAppBlocks`.
#[derive(Debug, Clone)]
pub struct AppBlockRecord {
    pub arn: String,
    pub name: String,
    pub description: Option<String>,
    pub display_name: Option<String>,
    pub source_s3_location: S3Location,
    pub setup_script_details: Option<ScriptDetails>,
    pub post_setup_script_details: Option<ScriptDetails>,
    /// One of `PACKAGING_TYPE_VALUES`; [`PACKAGING_TYPE_CUSTOM`] for a create that named
    /// none.
    pub packaging_type: String,
    /// One of `APP_BLOCK_STATE_VALUES`, as [`app_block_state_created`] decided it.
    pub state: String,
    pub created_time: ::aws_smithy_types::DateTime,
    /// The errors of the app block; empty for one nothing has gone wrong with.
    pub app_block_errors: Vec<ErrorDetails>,
    pub tags: BTreeMap<String, String>,
}

/// Render a stored text attribute the way the resource reports it: the modelled
/// `AppBlock.Description` and `AppBlock.DisplayName` members are `@length` 1.., so a
/// value a create was given as the empty string is an attribute the resource does not
/// have rather than one it reports empty.
fn reported_text(text: Option<&str>) -> Option<String> {
    text.filter(|value| !value.is_empty())
        .map(::std::string::ToString::to_string)
}

impl AppBlockRecord {
    /// Render the stored record as the modelled `AppBlock` every reader returns.
    pub fn to_app_block(&self) -> AppBlock {
        AppBlock {
            name: self.name.clone(),
            arn: self.arn.clone(),
            description: reported_text(self.description.as_deref()),
            display_name: reported_text(self.display_name.as_deref()),
            source_s3_location: Some(self.source_s3_location.clone()),
            setup_script_details: self.setup_script_details.clone(),
            created_time: Some(self.created_time),
            post_setup_script_details: self.post_setup_script_details.clone(),
            packaging_type: Some(self.packaging_type.clone()),
            state: Some(self.state.clone()),
            // An app block with nothing wrong with it has no errors to report, so the
            // member is ABSENT rather than present-and-empty.
            app_block_errors: reported(&self.app_block_errors),
        }
    }
}

/// A persisted AppStream application: the launch details, icon and display name an
/// Elastic fleet streams, bound to the app block that carries its binaries.
#[derive(Debug, Clone)]
pub struct ApplicationRecord {
    pub arn: String,
    pub name: String,
    pub display_name: Option<String>,
    pub description: Option<String>,
    pub icon_s3_location: S3Location,
    pub launch_path: String,
    pub launch_parameters: Option<String>,
    pub working_directory: Option<String>,
    pub platforms: Vec<String>,
    pub instance_families: Vec<String>,
    /// The ARN of the app block the application's binaries live in — resolved against the
    /// stored app blocks before the application was created, so it names one this
    /// account held at that point.
    pub app_block_arn: String,
    pub created_time: ::aws_smithy_types::DateTime,
    pub tags: BTreeMap<String, String>,
}

/// How long the icon URL an application reports stays valid.
///
/// `Application.IconURL` is "the URL for the application icon. This URL might be
/// time-limited", and the model publishes no span for it, so this one is the emulator's
/// own: long enough for a client to fetch the icon it was handed, short enough that the
/// URL is time-limited as documented.
pub const ICON_URL_VALIDITY: ::std::time::Duration = ::std::time::Duration::from_secs(3600);

impl ApplicationRecord {
    /// Render the stored record as the modelled `Application` every reader returns.
    ///
    /// The icon URL is signed per render rather than stored: it is a time-limited address
    /// of the S3 object the create named, so it has to be minted against the clock of the
    /// call that reports it.
    pub fn to_application(&self, signer: &::smithy_server_runtime::UrlSigner) -> Application {
        Application {
            name: Some(self.name.clone()),
            display_name: reported_text(self.display_name.as_deref()),
            icon_url: icon_url(signer, &self.icon_s3_location),
            launch_path: Some(self.launch_path.clone()),
            launch_parameters: reported_text(self.launch_parameters.as_deref()),
            // An application is created enabled; only a problem found while an image is
            // built disables one.
            enabled: Some(true),
            // The additional attributes an image's own application catalog carries;
            // an application created through this operation has none.
            metadata: None,
            working_directory: reported_text(self.working_directory.as_deref()),
            description: reported_text(self.description.as_deref()),
            arn: Some(self.arn.clone()),
            app_block_arn: Some(self.app_block_arn.clone()),
            icon_s3_location: Some(self.icon_s3_location.clone()),
            platforms: reported(&self.platforms),
            instance_families: reported(&self.instance_families),
            created_time: Some(self.created_time),
        }
    }
}

/// The time-limited URL the application icon stored in S3 is fetched from, signed so that
/// it leads back to this emulator's own S3 front door.
///
/// `None` when the stored location names no object to fetch: `S3Location.S3Key` is
/// optional on the wire, and a location with no key addresses the bucket rather than an
/// icon.
fn icon_url(
    signer: &::smithy_server_runtime::UrlSigner,
    location: &S3Location,
) -> Option<String> {
    let key = location.s3_key.as_deref().filter(|key| !key.is_empty())?;
    let path = format!("/{}/{key}", location.s3_bucket);
    let request = ::smithy_server_runtime::SignedUrlRequest::new(
        ::http::Method::GET,
        S3_SIGNING_NAME,
        &path,
    );
    signer
        .sign(request, ICON_URL_VALIDITY)
        .ok()
        .map(|url| url.as_str().to_string())
}

/// The signing name of the service an application's icon is stored in, which is the
/// service the icon URL addresses.
const S3_SIGNING_NAME: &str = "s3";

/// The account's usage report subscription: the daily usage reports it has enabled, and
/// where they are written.
///
/// There is no name or id in the modelled shape because usage reporting is a single
/// account-and-Region-wide setting — hence one record, not a map.
#[derive(Debug, Clone)]
pub struct UsageReportSubscriptionRecord {
    pub s3_bucket_name: String,
    /// One of `USAGE_REPORT_SCHEDULE_VALUES`; [`USAGE_REPORT_SCHEDULE`] for a
    /// subscription this emulator created.
    pub schedule: String,
    /// When the last usage report was generated. `None` until one has been: reports are
    /// generated daily, so a freshly enabled subscription has none, and this emulator
    /// never claims a report it did not produce.
    pub last_generated_report_date: Option<::aws_smithy_types::DateTime>,
    /// The errors that stopped a usage report being generated; empty while none has.
    pub subscription_errors: Vec<LastReportGenerationExecutionError>,
}

impl UsageReportSubscriptionRecord {
    /// Render the stored record as the modelled `UsageReportSubscription` a reader
    /// returns.
    pub fn to_usage_report_subscription(&self) -> UsageReportSubscription {
        UsageReportSubscription {
            s3_bucket_name: Some(self.s3_bucket_name.clone()),
            schedule: Some(self.schedule.clone()),
            last_generated_report_date: self.last_generated_report_date,
            // "The errors that were returned if usage reports couldn't be generated":
            // with no error to report the member is a configuration the subscription
            // does not have, so it is ABSENT rather than present-and-empty.
            subscription_errors: reported(&self.subscription_errors),
        }
    }
}

/// How long the allocation of a user pool identity stays open to a create arriving
/// together with it.
///
/// AppStream's user pool is written through an index a create checks the username
/// against, and that index is eventually consistent: creates issued together all find the
/// identity free, all succeed, and the pool ends up holding it exactly once. A create
/// issued once the identity is ESTABLISHED — the case AWS refuses as an already-taken
/// username — is one a caller sends after that view has caught up.
///
/// AWS publishes no propagation window, so this span is the emulator's own, and it is the
/// same span every other transition this crate owes the clock takes ([`FLEET_START_SPAN`]):
/// a caller acting straight after the create still observes the allocation that is open —
/// however many calls it makes in between and however slowly the host runs them — while a
/// caller that waits for the span sees the identity established and its next create refused
/// as AWS refuses it. A span short enough to elapse between two calls would make the
/// OUTCOME of a create depend on how fast the host executed the calls before it, which is
/// not something this service decides on. The window is ABSOLUTE — measured from the create
/// that allocated the identity, and never extended by a later one — so the identity becomes
/// established at an instant that create fixed, whatever arrives in the meantime. Nothing
/// sleeps for it: the instant the identity is established is stored on the record.
pub const USER_ALLOCATION_GRACE: ::std::time::Duration = FLEET_START_SPAN;

/// A persisted user pool user.
///
/// The identity is the `(AuthenticationType, UserName)` pair this record is stored
/// under: email addresses are case-sensitive, so two names differing only in
/// capitalisation are two users.
#[derive(Debug, Clone)]
pub struct UserRecord {
    pub arn: String,
    pub user_name: String,
    /// One of `AUTHENTICATION_TYPE_VALUES`; [`USER_POOL_AUTHENTICATION_TYPE`] for a user
    /// this emulator created.
    pub authentication_type: String,
    /// Whether the user may sign in. A created user is enabled; `DisableUser` and
    /// `EnableUser` move it.
    pub enabled: bool,
    /// The user's status in the user pool, one of `User.Status`'s documented values;
    /// [`USER_STATUS_UNCONFIRMED`] for a user this emulator created.
    pub status: String,
    pub first_name: Option<String>,
    pub last_name: Option<String>,
    pub created_time: ::aws_smithy_types::DateTime,
    /// The instant from which this identity counts as established, so that a further
    /// create of it is refused as a username that is taken: the create that allocated it
    /// plus [`USER_ALLOCATION_GRACE`].
    pub established_at: ::std::time::Instant,
}

impl UserRecord {
    /// Whether this identity is established at `now`: the allocation that minted it is no
    /// longer open to a create racing it, so a further create of the same username is
    /// refused rather than answered like the first.
    pub fn is_established(&self, now: ::std::time::Instant) -> bool {
        now >= self.established_at
    }

    /// Render the stored record as the modelled `User` every reader returns.
    pub fn to_user(&self) -> User {
        User {
            arn: Some(self.arn.clone()),
            user_name: Some(self.user_name.clone()),
            enabled: Some(self.enabled),
            status: Some(self.status.clone()),
            first_name: self.first_name.clone(),
            last_name: self.last_name.clone(),
            created_time: Some(self.created_time),
            authentication_type: self.authentication_type.clone(),
        }
    }
}

/// The identity one user/stack association is stored under:
/// `(AuthenticationType, UserName, StackName)`.
///
/// All three members are required on the modelled association, and together they are
/// what a `BatchDisassociateUserStack` names to remove an edge, so they are the key.
/// Ordering the authentication type and the user name before the stack name puts one
/// user's associations in a contiguous range.
pub type UserStackAssociationKey = (String, String, String);

/// The key the given association is stored under.
pub fn user_stack_association_key(association: &UserStackAssociation) -> UserStackAssociationKey {
    (
        association.authentication_type.clone(),
        association.user_name.clone(),
        association.stack_name.clone(),
    )
}

/// The per-item failure a batch reports for one association it did not apply: the
/// association as the request gave it, the modelled error code, and the sentence for that
/// error class.
pub fn user_stack_association_error(
    association: UserStackAssociation,
    error_code: &str,
    error_message: impl Into<String>,
) -> UserStackAssociationError {
    UserStackAssociationError {
        user_stack_association: Some(association),
        error_code: Some(error_code.to_string()),
        error_message: Some(error_message.into()),
    }
}

/// Which associations a read reports: every member left `None` filters nothing, so a
/// read asked only about a stack reports that stack's associations whoever the user is.
#[derive(Debug, Default)]
pub struct UserStackAssociationFilter<'a> {
    pub stack_name: Option<&'a str>,
    pub user_name: Option<&'a str>,
    pub authentication_type: Option<&'a str>,
}

impl UserStackAssociationFilter<'_> {
    /// Whether the association stored under `key` is one this filter reports.
    fn matches(&self, key: &UserStackAssociationKey) -> bool {
        let (authentication_type, user_name, stack_name) = key;
        self.stack_name.is_none_or(|name| name == stack_name)
            && self.user_name.is_none_or(|name| name == user_name)
            && self
                .authentication_type
                .is_none_or(|kind| kind == authentication_type)
    }
}

/// The `SessionState` of a session a streaming URL has been issued for and nobody has
/// connected to yet, one of `SESSION_STATE_VALUES` (`ops/types/shared_types.rs`):
/// `User.Status`-style, the state's own documentation is "PENDING - The session is
/// pending".
pub const SESSION_STATE_PENDING: &str = "PENDING";

/// The `SessionState` of a session whose streaming URL stopped verifying before anybody
/// connected to it: "EXPIRED - The session is expired".
pub const SESSION_STATE_EXPIRED: &str = "EXPIRED";

/// The `SessionConnectionState` of a session no user is connected to, one of
/// `SESSION_CONNECTION_STATE_VALUES`. A session a URL was issued for has nobody on it
/// until a client opens that URL.
pub const SESSION_CONNECTION_STATE_NOT_CONNECTED: &str = "NOT_CONNECTED";

/// The `AuthenticationType` of a session a streaming URL authenticates: that member's
/// documentation is "The user is authenticated using a streaming URL (API)".
pub const SESSION_AUTHENTICATION_TYPE_API: &str = "API";

/// How many sessions one `DescribeSessions` page carries when the request names no
/// `Limit`, and the most one page carries at all: that member is documented "The size of
/// each page of results. The default value is 20 and the maximum value is 50", and both
/// numbers are AWS's.
pub const SESSION_PAGE_SIZE_DEFAULT: usize = 20;
pub const SESSION_PAGE_SIZE_MAX: usize = 50;

/// The identity a streaming session is held under: the stack, the fleet and the user it
/// was dedicated for. A user streams one session of a stack's fleet at a time, so a
/// streaming URL issued again for the same identity re-dedicates that session rather than
/// adding a second one.
pub type SessionKey = (String, String, String);

/// A streaming session a `CreateStreamingURL` dedicated for a user.
#[derive(Debug, Clone)]
pub struct SessionRecord {
    pub id: String,
    pub user_id: String,
    pub stack_name: String,
    pub fleet_name: String,
    /// One of `SESSION_STATE_VALUES`: [`SESSION_STATE_PENDING`] until the URL the session
    /// was dedicated for stops verifying.
    pub state: String,
    pub start_time: ::aws_smithy_types::DateTime,
    /// "The time when the streaming session is set to expire. This time is based on the
    /// `MaxUserDurationinSeconds` value" — so a fleet that publishes no maximum user
    /// duration has no such time to report.
    pub max_expiration_time: Option<::aws_smithy_types::DateTime>,
    /// The instant the URL issued for this session stops verifying. A streaming URL is
    /// temporary, so a session nobody connected to before its URL expired is expired
    /// too — a transition the clock owes, like a starting fleet's.
    pub expires_at: ::std::time::Instant,
}

impl SessionRecord {
    /// The identity this session is held under.
    pub fn key(&self) -> SessionKey {
        (
            self.stack_name.clone(),
            self.fleet_name.clone(),
            self.user_id.clone(),
        )
    }

    /// Apply the transition the clock owes this session, if `now` is past the instant its
    /// URL stops verifying. Nothing else about the session changes.
    pub fn settle_state(&mut self, now: ::std::time::Instant) {
        if self.state == SESSION_STATE_PENDING && now >= self.expires_at {
            self.state = SESSION_STATE_EXPIRED.to_string();
        }
    }

    /// Render the stored record as the modelled `Session` every reader returns.
    pub fn to_session(&self) -> Session {
        Session {
            id: self.id.clone(),
            user_id: self.user_id.clone(),
            stack_name: self.stack_name.clone(),
            fleet_name: self.fleet_name.clone(),
            state: self.state.clone(),
            // Nobody has opened the URL this session was dedicated for, so no user is
            // connected to it.
            connection_state: Some(SESSION_CONNECTION_STATE_NOT_CONNECTED.to_string()),
            start_time: Some(self.start_time),
            max_expiration_time: self.max_expiration_time,
            authentication_type: Some(SESSION_AUTHENTICATION_TYPE_API.to_string()),
            // A session nothing is streaming runs on no instance, so there is no
            // interface to report an address of, no instance to name, and no drain status
            // for one ("this only applies to multi-session fleets").
            network_access_configuration: None,
            instance_id: None,
            instance_drain_status: None,
        }
    }
}

/// The opaque id a session is reported under.
///
/// `Session.Id` is "the identifier of the streaming session", an opaque value the service
/// mints: this one is a digest of the identity the session was dedicated for and the
/// instant it was dedicated at, so two sessions of one identity never share an id and the
/// id carries nothing a caller could read a secret out of.
pub fn session_id(key: &SessionKey, start_time: ::aws_smithy_types::DateTime) -> String {
    let (stack_name, fleet_name, user_id) = key;
    let digest = <::sha2::Sha256 as ::sha2::Digest>::digest(format!(
        "appstream-session:{stack_name}:{fleet_name}:{user_id}:{}",
        start_time.as_secs_f64()
    ));
    hex::encode(digest)
}

/// Which sessions a read reports. The stack and the fleet are required of the read, so
/// they always narrow it; every member left `None` filters nothing.
#[derive(Debug)]
pub struct SessionFilter<'a> {
    pub stack_name: &'a str,
    pub fleet_name: &'a str,
    pub user_id: Option<&'a str>,
    pub authentication_type: Option<&'a str>,
    pub instance_id: Option<&'a str>,
}

impl SessionFilter<'_> {
    /// Whether the session stored under `key` is one this filter reports.
    fn matches(&self, key: &SessionKey) -> bool {
        let (stack_name, fleet_name, user_id) = key;
        self.stack_name == stack_name
            && self.fleet_name == fleet_name
            && self.user_id.is_none_or(|asked| asked == user_id)
            && self
                .authentication_type
                .is_none_or(|kind| kind == SESSION_AUTHENTICATION_TYPE_API)
            // No session here is hosted on an instance, so a read narrowed to one names
            // an instance no session of this account runs on and reports none.
            && self.instance_id.is_none()
    }
}

/// One page of a `DescribeSessions`: the sessions to render, and the opaque token that
/// resumes after them.
#[derive(Debug)]
pub struct SessionPage {
    pub sessions: Vec<Session>,
    pub next_token: Option<String>,
}

/// The identity one application/fleet association is stored under: `(FleetName,
/// ApplicationArn)`, the two members the modelled `ApplicationFleetAssociation` carries.
pub type ApplicationFleetAssociationKey = (String, String);

/// How many application/fleet associations one `DescribeApplicationFleetAssociations`
/// page carries.
///
/// The model caps `ApplicationFleetAssociationList` at 25 members and that list is what a
/// page of the read is rendered into, so a page holds at most 25 associations however
/// large a `MaxResults` a request asks for.
pub const APPLICATION_FLEET_ASSOCIATION_PAGE_SIZE: usize = 25;

/// Which application/fleet associations a read reports: exactly one of the two endpoints
/// selects them ("Either ApplicationArn or FleetName must be specified"), and the other
/// filters nothing.
#[derive(Debug)]
pub struct ApplicationFleetAssociationFilter<'a> {
    pub fleet_name: Option<&'a str>,
    pub application_arn: Option<&'a str>,
}

impl ApplicationFleetAssociationFilter<'_> {
    /// Whether the association stored under `key` is one this read reports.
    fn matches(&self, key: &ApplicationFleetAssociationKey) -> bool {
        let (fleet_name, application_arn) = key;
        self.fleet_name
            .is_none_or(|asked| asked == fleet_name.as_str())
            && self
                .application_arn
                .is_none_or(|asked| asked == application_arn.as_str())
    }
}

/// One page of a `DescribeApplicationFleetAssociations`: the associations to render, and
/// the opaque token that resumes after them.
#[derive(Debug)]
pub struct ApplicationFleetAssociationPage {
    pub associations: Vec<ApplicationFleetAssociation>,
    pub next_token: Option<String>,
}

/// Encode an application/fleet association cursor: the key of the first record of the
/// next page, hex-encoded for the same reason [`encode_cursor`] is.
fn encode_fleet_application_cursor(key: &ApplicationFleetAssociationKey) -> String {
    let (fleet_name, application_arn) = key;
    encode_cursor(&format!(
        "{fleet_name}{ASSOCIATION_CURSOR_SEPARATOR}{application_arn}"
    ))
}

/// The inverse of [`encode_fleet_application_cursor`]; `None` for a token this service
/// did not mint.
fn decode_fleet_application_cursor(token: &str) -> Option<ApplicationFleetAssociationKey> {
    let decoded = decode_cursor(token)?;
    let mut parts = decoded.splitn(2, ASSOCIATION_CURSOR_SEPARATOR);
    Some((parts.next()?.to_string(), parts.next()?.to_string()))
}

/// Per-service state for com-amazonaws-appstream-photonadminproxyservice. A handler reaches it through [`service_state`].
#[derive(Debug)]
pub struct ComAmazonawsAppstreamPhotonadminproxyserviceState {
    /// Stacks by `Name`. `BTreeMap` so every Describe page walks a stable order.
    pub stacks: BTreeMap<String, StackRecord>,
    /// Custom branding themes by the `StackName` of the stack they brand — one theme per
    /// stack, which is what makes the stack name the key.
    ///
    /// This store needs no ceiling of its own: a theme can only be created for a stack
    /// that exists, so it holds at most as many records as `stacks`, which is itself
    /// bounded by [`MAX_RECORDS_PER_STORE`].
    pub themes: BTreeMap<String, ThemeRecord>,
    /// Fleets by `Name`.
    pub fleets: BTreeMap<String, FleetRecord>,
    /// App block builders by `Name`. `BTreeMap` so every Describe page walks a stable
    /// order.
    pub app_block_builders: BTreeMap<String, AppBlockBuilderRecord>,
    /// App blocks by `Name` — the name a `CreateAppBlock` mints the resource under, which
    /// is also what a `DeleteAppBlock` names. `BTreeMap` so every page of a read walks a
    /// stable order.
    pub app_blocks: BTreeMap<String, AppBlockRecord>,
    /// Applications by `Name` — the name a `CreateApplication` mints the resource under.
    /// `BTreeMap` so every page of a read walks a stable order.
    pub applications: BTreeMap<String, ApplicationRecord>,
    /// Streaming sessions by [`SessionKey`] — one per user dedicated to one stack's
    /// fleet, written by `CreateStreamingURL`. `BTreeMap` so every page of a read walks a
    /// stable order, and so one stack-and-fleet's sessions form a contiguous range.
    ///
    /// Bounded by [`MAX_RECORDS_PER_STORE`], like every other store a request can grow:
    /// the `UserId` a streaming URL is issued for is the caller's to choose and is not
    /// drawn from `users`, so the identities are unbounded unless the store is capped.
    /// `CreateStreamingURL` takes the room for a NEW session through
    /// [`Self::session_room`], which first reclaims the sessions the clock has already
    /// expired — nothing can stream from those — and refuses the request when the
    /// store is full even so.
    pub sessions: BTreeMap<SessionKey, SessionRecord>,
    /// Images by `Name` — the name a fleet or an image builder names an image by.
    ///
    /// SEEDED with the base images AWS publishes ([`seed_image_catalog`]), which no
    /// account creates, and the same map an operation that mints a private image writes
    /// into, so `DescribeImages` reads ONE catalog. `BTreeMap` so every page of a read
    /// walks a stable order.
    pub images: BTreeMap<String, ImageRecord>,
    /// `(FleetName, StackName)` edges written by `AssociateFleet`.
    pub fleet_stack_associations: BTreeSet<(String, String)>,
    /// `(FleetName, ApplicationArn)` edges written by `AssociateApplicationFleet` — which
    /// application an Elastic fleet streams. `BTreeSet` so every page of a read walks a
    /// stable order, and so one fleet's associations form a contiguous range.
    ///
    /// Bounded by [`MAX_RECORDS_PER_STORE`] like every other store a request can grow.
    pub application_fleet_associations: BTreeSet<ApplicationFleetAssociationKey>,
    /// The account's usage report subscription, once one has been created; `None` while
    /// usage reporting is not enabled.
    pub usage_report_subscription: Option<UsageReportSubscriptionRecord>,
    /// Users by `(AuthenticationType, UserName)` — the identity a user pool user is
    /// created under. `BTreeMap` so every page of a read walks a stable order, and so
    /// one authentication type's users form a contiguous range.
    pub users: BTreeMap<(String, String), UserRecord>,
    /// User/stack associations by [`UserStackAssociationKey`] — one edge per user
    /// entitled to one stack, written by `BatchAssociateUserStack` and removed by
    /// `BatchDisassociateUserStack`.
    ///
    /// The stored value is the MODELLED association rather than a copy of its members, so
    /// what `DescribeUserStackAssociations` renders is literally what the batch
    /// persisted. `BTreeMap` so every page of a read walks a stable order.
    pub user_stack_associations: BTreeMap<UserStackAssociationKey, UserStackAssociation>,
}

/// A fresh store holds no caller-created resource and the WHOLE published image catalog:
/// the base images are the one part of this state nothing creates, so they are seeded
/// rather than written by a request.
impl Default for ComAmazonawsAppstreamPhotonadminproxyserviceState {
    fn default() -> Self {
        Self {
            stacks: BTreeMap::new(),
            themes: BTreeMap::new(),
            fleets: BTreeMap::new(),
            app_block_builders: BTreeMap::new(),
            app_blocks: BTreeMap::new(),
            applications: BTreeMap::new(),
            sessions: BTreeMap::new(),
            images: seed_image_catalog(),
            fleet_stack_associations: BTreeSet::new(),
            application_fleet_associations: BTreeSet::new(),
            usage_report_subscription: None,
            users: BTreeMap::new(),
            user_stack_associations: BTreeMap::new(),
        }
    }
}

impl ComAmazonawsAppstreamPhotonadminproxyserviceState {
    /// Whether any association still names this fleet. A predicate, not a list: an
    /// in-use refusal states that the fleet is associated with a stack without naming
    /// the stacks, so nothing needs the names materialised.
    pub fn fleet_is_associated(&self, fleet_name: &str) -> bool {
        self.fleet_stack_associations
            .range((fleet_name.to_string(), String::new())..)
            .take_while(|(fleet, _)| fleet == fleet_name)
            .next()
            .is_some()
    }

    /// Whether any association still names this stack.
    pub fn stack_is_associated(&self, stack_name: &str) -> bool {
        self.fleet_stack_associations
            .iter()
            .any(|(_, stack)| stack == stack_name)
    }

    /// The app block stored under the ARN asked about; `None` when this service holds
    /// none under it.
    ///
    /// An app block is addressed by ARN rather than by name on a read, and the stored ARN
    /// must match in full, so an ARN naming another Region or account addresses no app
    /// block here. The name in the ARN is what the record is keyed by, so the lookup is
    /// one map probe rather than a scan.
    pub fn app_block_by_arn(&self, arn: &str) -> Option<&AppBlockRecord> {
        let (resource_type, name) = arn_resource(arn);
        (resource_type == APP_BLOCK_ARN_RESOURCE_TYPE)
            .then(|| self.app_blocks.get(&name))
            .flatten()
            .filter(|record| record.arn == arn)
    }

    /// The application stored under the ARN asked about; `None` when this service holds
    /// none under it.
    ///
    /// An application is addressed by ARN rather than by name on a read, the way an app
    /// block is ([`Self::app_block_by_arn`]), and the stored ARN must match in full, so an
    /// ARN naming another Region or account addresses no application here.
    pub fn application_by_arn(&self, arn: &str) -> Option<&ApplicationRecord> {
        let (resource_type, name) = arn_resource(arn);
        (resource_type == APPLICATION_ARN_RESOURCE_TYPE)
            .then(|| self.applications.get(&name))
            .flatten()
            .filter(|record| record.arn == arn)
    }

    /// Whether the store can take ONE more session, reclaiming the records the clock has
    /// already expired if it has to.
    ///
    /// An expired session is one nobody can stream from: its streaming URL stopped
    /// verifying before anybody opened it, so dropping it frees the room a live session
    /// needs rather than losing anything a caller can still use. `false` means the store
    /// is at [`MAX_RECORDS_PER_STORE`] with nothing expired to reclaim, and the request
    /// asking for the room is refused.
    pub fn session_room(&mut self) -> bool {
        if self.sessions.len() < MAX_RECORDS_PER_STORE {
            return true;
        }
        self.settle_session_states();
        self.sessions
            .retain(|_, session| session.state != SESSION_STATE_EXPIRED);
        self.sessions.len() < MAX_RECORDS_PER_STORE
    }

    /// Apply every session-state transition the clock has already made due: a session
    /// whose streaming URL stopped verifying before anybody connected to it is expired at
    /// that instant, whether or not anybody calls again.
    pub fn settle_session_states(&mut self) {
        let now = ::std::time::Instant::now();
        for session in self.sessions.values_mut() {
            session.settle_state(now);
        }
    }

    /// One page of the sessions `filter` reports: the sessions to render, and the token
    /// that resumes after them. `None` means `next_token` is not a token this service
    /// minted.
    ///
    /// The sessions are read straight from the stored map in key order, so a page renders
    /// exactly the records the store holds — there is no second lookup that could miss
    /// one — and following the token walks every matching session exactly once even when
    /// sessions are dedicated between calls.
    pub fn sessions_page(
        &self,
        filter: &SessionFilter<'_>,
        next_token: Option<&str>,
        page_size: usize,
    ) -> Option<SessionPage> {
        let resume_at = match next_token {
            Some(token) => Some(decode_association_cursor(token)?),
            None => None,
        };
        let mut remaining = self
            .sessions
            .iter()
            .filter(|(key, _)| resume_at.as_ref().is_none_or(|from| *key >= from))
            .filter(|(key, _)| filter.matches(key));
        // A page holds at least one record, so following the token always advances.
        let sessions: Vec<Session> = remaining
            .by_ref()
            .take(page_size.max(1))
            .map(|(_, record)| record.to_session())
            .collect();
        Some(SessionPage {
            next_token: remaining
                .next()
                .map(|(key, _)| encode_association_cursor(key)),
            sessions,
        })
    }

    /// Whether any application this account holds still names this app block as the
    /// source of its binaries, which is what makes the app block in use.
    ///
    /// A predicate, not a list: the refusal states that the app block is in use without
    /// naming what holds it.
    pub fn app_block_is_in_use(&self, app_block_arn: &str) -> bool {
        self.applications
            .values()
            .any(|application| application.app_block_arn == app_block_arn)
    }

    /// Whether any fleet association still names this application, which is what makes
    /// the application in use.
    pub fn application_is_associated(&self, application_arn: &str) -> bool {
        self.application_fleet_associations
            .iter()
            .any(|(_, arn)| arn == application_arn)
    }

    /// One page of the application/fleet associations `filter` reports: the associations
    /// to render, and the token that resumes after them. `None` means `next_token` is not
    /// a token this service minted.
    ///
    /// The edges are read straight from the stored set in key order, so a page renders
    /// exactly the associations the store holds — there is no second lookup that could
    /// miss one — and following the token walks every matching association exactly once
    /// even when associations are added between calls.
    pub fn application_fleet_associations_page(
        &self,
        filter: &ApplicationFleetAssociationFilter<'_>,
        next_token: Option<&str>,
        page_size: usize,
    ) -> Option<ApplicationFleetAssociationPage> {
        let resume_at = match next_token {
            Some(token) => Some(decode_fleet_application_cursor(token)?),
            None => None,
        };
        let mut remaining = self
            .application_fleet_associations
            .iter()
            .filter(|key| resume_at.as_ref().is_none_or(|from| *key >= from))
            .filter(|key| filter.matches(key));
        // A page holds at least one record, so following the token always advances.
        let associations: Vec<ApplicationFleetAssociation> = remaining
            .by_ref()
            .take(page_size.max(1))
            .map(|(fleet_name, application_arn)| ApplicationFleetAssociation {
                fleet_name: fleet_name.clone(),
                application_arn: application_arn.clone(),
            })
            .collect();
        Some(ApplicationFleetAssociationPage {
            next_token: remaining.next().map(encode_fleet_application_cursor),
            associations,
        })
    }

    /// Apply every fleet-state transition the clock has already made due.
    ///
    /// A fleet `StartFleet` asked for reaches `RUNNING` at an instant that call fixed, so
    /// the transition is owed whether or not anybody calls again: materialising it here is
    /// what makes a later read report the state the fleet is already in, and it changes
    /// nothing the clock had not already decided.
    pub fn settle_fleet_states(&mut self) {
        let now = ::std::time::Instant::now();
        for fleet in self.fleets.values_mut() {
            fleet.settle_state(now);
        }
    }

    /// The tags the resource with this ARN carries, for reading; `None` when this
    /// service holds no resource under that ARN.
    ///
    /// The tags live ON the resource's own record — the map a `CreateStack`/`CreateFleet`
    /// persisted — so a read reports exactly what the create and every later tag write
    /// left there, and nothing can drift between two copies. The stored ARN must match
    /// the one asked about in full, so an ARN naming another Region or account addresses
    /// no resource here.
    pub fn resource_tags(&self, resource_arn: &str) -> Option<&BTreeMap<String, String>> {
        let (resource_type, name) = arn_resource(resource_arn);
        match resource_type.as_str() {
            STACK_ARN_RESOURCE_TYPE => self
                .stacks
                .get(&name)
                .filter(|record| record.arn == resource_arn)
                .map(|record| &record.tags),
            FLEET_ARN_RESOURCE_TYPE => self
                .fleets
                .get(&name)
                .filter(|record| record.arn == resource_arn)
                .map(|record| &record.tags),
            APP_BLOCK_BUILDER_ARN_RESOURCE_TYPE => self
                .app_block_builders
                .get(&name)
                .filter(|record| record.arn == resource_arn)
                .map(|record| &record.tags),
            APP_BLOCK_ARN_RESOURCE_TYPE => self
                .app_blocks
                .get(&name)
                .filter(|record| record.arn == resource_arn)
                .map(|record| &record.tags),
            APPLICATION_ARN_RESOURCE_TYPE => self
                .applications
                .get(&name)
                .filter(|record| record.arn == resource_arn)
                .map(|record| &record.tags),
            _ => None,
        }
    }

    /// The same tags, for writing: the map `TagResource` adds to and `UntagResource`
    /// removes from.
    pub fn resource_tags_mut(
        &mut self,
        resource_arn: &str,
    ) -> Option<&mut BTreeMap<String, String>> {
        let (resource_type, name) = arn_resource(resource_arn);
        match resource_type.as_str() {
            STACK_ARN_RESOURCE_TYPE => self
                .stacks
                .get_mut(&name)
                .filter(|record| record.arn == resource_arn)
                .map(|record| &mut record.tags),
            FLEET_ARN_RESOURCE_TYPE => self
                .fleets
                .get_mut(&name)
                .filter(|record| record.arn == resource_arn)
                .map(|record| &mut record.tags),
            APP_BLOCK_BUILDER_ARN_RESOURCE_TYPE => self
                .app_block_builders
                .get_mut(&name)
                .filter(|record| record.arn == resource_arn)
                .map(|record| &mut record.tags),
            APP_BLOCK_ARN_RESOURCE_TYPE => self
                .app_blocks
                .get_mut(&name)
                .filter(|record| record.arn == resource_arn)
                .map(|record| &mut record.tags),
            APPLICATION_ARN_RESOURCE_TYPE => self
                .applications
                .get_mut(&name)
                .filter(|record| record.arn == resource_arn)
                .map(|record| &mut record.tags),
            _ => None,
        }
    }

    /// One page of this authentication type's users: the records to render, and the
    /// token that resumes after them. `None` means `next_token` is not a token this
    /// service minted.
    ///
    /// The users are read straight from the stored map in key order, so a page renders
    /// exactly the records the store holds for that authentication type — there is no
    /// second lookup that could miss one — and following the token walks every user
    /// exactly once even when users are added between calls.
    pub fn users_page(
        &self,
        authentication_type: &str,
        next_token: Option<&str>,
        page_size: usize,
    ) -> Option<UserPage> {
        let resume_at = match next_token {
            Some(token) => decode_cursor(token)?,
            None => String::new(),
        };
        let mut remaining = self
            .users
            .range((authentication_type.to_string(), resume_at)..)
            .take_while(|((stored_type, _), _)| stored_type == authentication_type)
            .map(|(_, record)| record);
        // A page holds at least one record, so following the token always advances.
        let users: Vec<User> = remaining
            .by_ref()
            .take(page_size.max(1))
            .map(UserRecord::to_user)
            .collect();
        Some(UserPage {
            next_token: remaining
                .next()
                .map(|record| encode_cursor(&record.user_name)),
            users,
        })
    }

    /// Move the sign-in flag of the user the given `(AuthenticationType, UserName)`
    /// identity names, reporting whether the pool holds that user at all.
    ///
    /// `Enabled` is the only thing `DisableUser` and `EnableUser` move: neither touches
    /// the user's other attributes, so a disabled user is still in the pool with the ARN,
    /// the names and the creation instant it was created with. Writing the value the
    /// record already carries leaves it exactly as it was, which is why a repeat of
    /// either call is answered like the first.
    pub fn set_user_enabled(
        &mut self,
        authentication_type: &str,
        user_name: &str,
        enabled: bool,
    ) -> bool {
        // Email addresses are case-sensitive, so the lookup uses the name exactly as the
        // request spells it, under the authentication type it names: an identity no
        // record is stored under is simply not in the pool.
        let identity = (authentication_type.to_string(), user_name.to_string());
        match self.users.get_mut(&identity) {
            Some(record) => {
                record.enabled = enabled;
                true
            }
            None => false,
        }
    }

    /// Remove the user the given `(AuthenticationType, UserName)` identity names, with
    /// everything that identity entitled it to, reporting whether the pool held it.
    ///
    /// A deleted user is gone rather than merely unlisted: the stack associations written
    /// for that identity go with the record, so nothing names a user the pool no longer
    /// holds and a later create of the same username allocates a fresh identity.
    pub fn remove_user(&mut self, authentication_type: &str, user_name: &str) -> bool {
        let identity = (authentication_type.to_string(), user_name.to_string());
        if self.users.remove(&identity).is_none() {
            return false;
        }
        self.user_stack_associations
            .retain(|(stored_type, stored_name, _), _| {
                (stored_type.as_str(), stored_name.as_str())
                    != (authentication_type, user_name)
            });
        true
    }

    /// The fleets associated with the named stack, in the stable order a page of
    /// `ListAssociatedFleets` walks.
    pub fn fleets_for_stack(&self, stack_name: &str) -> BTreeSet<String> {
        self.fleet_stack_associations
            .iter()
            .filter(|(_, stack)| stack == stack_name)
            .map(|(fleet, _)| fleet.clone())
            .collect()
    }

    /// The stacks associated with the named fleet, in the stable order a page of
    /// `ListAssociatedStacks` walks. The mirror image of [`Self::fleets_for_stack`]: both
    /// read the one stored edge set, so neither read can report an edge the other does
    /// not.
    pub fn stacks_for_fleet(&self, fleet_name: &str) -> BTreeSet<String> {
        self.fleet_stack_associations
            .iter()
            .filter(|(fleet, _)| fleet == fleet_name)
            .map(|(_, stack)| stack.clone())
            .collect()
    }

    /// One page of the user/stack associations `filter` reports: the associations to
    /// render, and the token that resumes after them. `None` means `next_token` is not a
    /// token this service minted.
    ///
    /// The associations are read straight from the stored map in key order, so a page
    /// renders exactly the records the store holds — there is no second lookup that could
    /// miss one — and following the token walks every matching association exactly once
    /// even when associations are added between calls.
    pub fn user_stack_associations_page(
        &self,
        filter: &UserStackAssociationFilter<'_>,
        next_token: Option<&str>,
        page_size: usize,
    ) -> Option<UserStackAssociationPage> {
        let resume_at = match next_token {
            Some(token) => Some(decode_association_cursor(token)?),
            None => None,
        };
        let mut remaining = self
            .user_stack_associations
            .iter()
            .filter(|(key, _)| resume_at.as_ref().is_none_or(|from| *key >= from))
            .filter(|(key, _)| filter.matches(key));
        // A page holds at least one record, so following the token always advances.
        let associations: Vec<UserStackAssociation> = remaining
            .by_ref()
            .take(page_size.max(1))
            .map(|(_, association)| association.clone())
            .collect();
        Some(UserStackAssociationPage {
            next_token: remaining
                .next()
                .map(|(key, _)| encode_association_cursor(key)),
            associations,
        })
    }
}

/// One page of a Describe: the names to render, and the opaque token that resumes after
/// them.
///
/// `names` is sorted and deduplicated, so following `next_token` walks every record
/// exactly once even when records are added between calls.
#[derive(Debug)]
pub struct DescribePage {
    pub names: Vec<String>,
    pub next_token: Option<String>,
}

/// One page of a `DescribeUsers`: the users to render, and the opaque token that resumes
/// after them.
#[derive(Debug)]
pub struct UserPage {
    pub users: Vec<User>,
    pub next_token: Option<String>,
}

/// One page of a `DescribeUserStackAssociations`: the associations to render, and the
/// opaque token that resumes after them.
#[derive(Debug)]
pub struct UserStackAssociationPage {
    pub associations: Vec<UserStackAssociation>,
    pub next_token: Option<String>,
}

/// Encode a pagination cursor (the name of the first record of the next page).
///
/// A real AppStream `NextToken` is opaque, so the cursor is hex-encoded rather than
/// handed to the caller in the clear: a client cannot read structure into it or
/// hand-craft one. `hex` is the maintained crate for the codec.
fn encode_cursor(name: &str) -> String {
    hex::encode(name)
}

/// The inverse of [`encode_cursor`]; `None` for a token this service did not mint.
fn decode_cursor(token: &str) -> Option<String> {
    String::from_utf8(hex::decode(token).ok()?).ok()
}

/// The byte that joins the three parts of a user/stack association cursor, and of a
/// streaming-session cursor.
///
/// A newline cannot appear in any of them: `Username`'s pattern admits only letters,
/// marks, symbols, numbers and punctuation, and a stored stack's name only
/// `[a-zA-Z0-9_.-]`, so the joined form parses back to exactly the key it was built from.
/// The same holds for a session's key: a session is dedicated only for a STORED stack and
/// fleet, whose names carry that pattern, and for a `UserId` whose own pattern is
/// `^[\w+=,.@-]*$`.
const ASSOCIATION_CURSOR_SEPARATOR: char = '\n';

/// Encode a user/stack association cursor: the key of the first record of the next page,
/// hex-encoded for the same reason [`encode_cursor`] is.
fn encode_association_cursor(key: &UserStackAssociationKey) -> String {
    let (authentication_type, user_name, stack_name) = key;
    encode_cursor(&format!(
        "{authentication_type}{ASSOCIATION_CURSOR_SEPARATOR}{user_name}\
         {ASSOCIATION_CURSOR_SEPARATOR}{stack_name}"
    ))
}

/// The inverse of [`encode_association_cursor`]; `None` for a token this service did not
/// mint.
fn decode_association_cursor(token: &str) -> Option<UserStackAssociationKey> {
    let decoded = decode_cursor(token)?;
    let mut parts = decoded.splitn(3, ASSOCIATION_CURSOR_SEPARATOR);
    Some((
        parts.next()?.to_string(),
        parts.next()?.to_string(),
        parts.next()?.to_string(),
    ))
}

/// How many records one page of a read carries, from the `MaxResults` the request asked
/// for and the `ceiling` that read renders at most.
///
/// `MaxResults` is "the maximum size of each page of results", so a page of fewer than
/// one record is not a size a read can answer with: such a request is refused, and the
/// `Err` carries the sentence stating the rule. An ask above the ceiling is clamped to it,
/// so no request sizes the page it is answered with — a caller asking for more still sees
/// every record by following the token.
pub fn page_size(max_results: Option<i32>, ceiling: usize) -> Result<usize, String> {
    match max_results {
        Some(max_results) if max_results < 1 => {
            Err(format!("MaxResults must be at least 1, was {max_results}"))
        }
        Some(max_results) => Ok(usize::try_from(max_results).unwrap_or(ceiling).min(ceiling)),
        None => Ok(ceiling),
    }
}

/// Cut `all` (sorted, deduplicated record names) into the page that resumes at
/// `next_token`. `None` means the token is not one this service minted.
pub fn paginate(all: &BTreeSet<String>, next_token: Option<&str>) -> Option<DescribePage> {
    paginate_sized(all, next_token, DESCRIBE_PAGE_SIZE)
}

/// The same cut, for a read whose `MaxResults` sizes the page: at most `page_size`
/// records, and never fewer than one, so following the token always advances.
pub fn paginate_sized(
    all: &BTreeSet<String>,
    next_token: Option<&str>,
    page_size: usize,
) -> Option<DescribePage> {
    let cursor = match next_token {
        Some(token) => Some(decode_cursor(token)?),
        None => None,
    };
    let mut remaining = all
        .iter()
        .filter(|name| {
            cursor
                .as_deref()
                .is_none_or(|resume_at| name.as_str() >= resume_at)
        })
        .cloned();
    let names: Vec<String> = remaining.by_ref().take(page_size.max(1)).collect();
    Some(DescribePage {
        next_token: remaining.next().map(|name| encode_cursor(&name)),
        names,
    })
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsAppstreamPhotonadminproxyserviceState>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsAppstreamPhotonadminproxyserviceState>)
}
