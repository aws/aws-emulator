// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-gamelift-gamelift service.
//!
//! The hosting resources this emulator persists — fleets, the aliases that route at
//! them, the game sessions placed on them, and the player slots reserved in those
//! sessions — plus the behaviour every handler shares over them: resolving a fleet, an
//! alias or a game session by id or ARN, materialising the status changes the clock has
//! already made due, minting ids and ARNs in the formats the model documents, and the
//! opaque pagination token the paginated describes share.

use std::collections::{HashMap, HashSet};
use std::sync::OnceLock;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use sha2::{Digest, Sha256};

pub mod managed_locations;

pub use managed_locations::*;

use crate::ops::types::{
    Alias, AnywhereConfiguration, Build, CertificateConfiguration, Compute, DesiredPlayerSession,
    FilterConfiguration, FleetAttributes, GameProperty, GameSession, GameSessionPlacement,
    GameSessionQueue, GameSessionQueueDestination, IpPermission, LocationModel, LocationState,
    MatchmakingConfiguration, MatchmakingRuleSet, MatchmakingTicket, PlacedPlayerSession, Player,
    PlayerGatewayConfiguration, PlayerLatency, PlayerLatencyPolicy, PlayerSession,
    PriorityConfiguration, PriorityConfigurationOverride, ResourceCreationLimitPolicy,
    RoutingStrategy, RuntimeConfiguration, S3Location, Script, Tag, VpcPeeringAuthorization,
};

/// `ComputeType` values the model declares (`smithy_query shape=ComputeType`).
pub const COMPUTE_TYPE_EC2: &str = "EC2";
/// See [`COMPUTE_TYPE_EC2`].
pub const COMPUTE_TYPE_ANYWHERE: &str = "ANYWHERE";

/// `RoutingStrategyType` values the model declares (`smithy_query
/// shape=RoutingStrategyType`), i.e. the two routings an alias can carry.
pub const ROUTING_STRATEGY_TYPE_SIMPLE: &str = "SIMPLE";
/// See [`ROUTING_STRATEGY_TYPE_SIMPLE`].
pub const ROUTING_STRATEGY_TYPE_TERMINAL: &str = "TERMINAL";

/// `FleetStatus` values this slice uses (`smithy_query shape=FleetStatus`).
pub const FLEET_STATUS_NEW: &str = "NEW";
/// See [`FLEET_STATUS_NEW`].
pub const FLEET_STATUS_ACTIVE: &str = "ACTIVE";
/// See [`FLEET_STATUS_NEW`].
pub const FLEET_STATUS_DELETING: &str = "DELETING";

/// `BuildStatus` values this slice uses (`smithy_query shape=BuildStatus`), as
/// `Build$Status` documents them: "`INITIALIZED` -- A new build has been defined, but no
/// files have been uploaded. You cannot create fleets for builds that are in this status.
/// When a build is successfully created, the build status is set to this value."
pub const BUILD_STATUS_INITIALIZED: &str = "INITIALIZED";
/// "`READY` -- The game build has been successfully uploaded. You can now create new
/// fleets for this build."
pub const BUILD_STATUS_READY: &str = "READY";

/// `ComputeStatus` value a compute that can host game servers carries
/// (`smithy_query shape=ComputeStatus`), as `Compute$ComputeStatus` documents it: "A
/// compute must have an `ACTIVE` status to host game sessions."
pub const COMPUTE_STATUS_ACTIVE: &str = "ACTIVE";

/// `GameSessionStatus` values this slice uses (`smithy_query shape=GameSessionStatus`).
pub const GAME_SESSION_STATUS_ACTIVATING: &str = "ACTIVATING";
/// See [`GAME_SESSION_STATUS_ACTIVATING`].
pub const GAME_SESSION_STATUS_ACTIVE: &str = "ACTIVE";

/// `PlayerSessionStatus` value a freshly reserved slot carries
/// (`smithy_query shape=PlayerSessionStatus`).
pub const PLAYER_SESSION_STATUS_RESERVED: &str = "RESERVED";

/// The `FlexMatchMode` that forms matches without starting a game session: that member's
/// own doc — "STANDALONE - FlexMatch forms matches and returns match information,
/// including players and team assignments, in a MatchmakingSucceeded event." Both values
/// come from the modelled enum (`smithy_query shape=FlexMatchMode`), which
/// `FLEX_MATCH_MODE_VALUES` carries.
pub const FLEX_MATCH_MODE_STANDALONE: &str = "STANDALONE";

/// The `BackfillMode` that has Amazon GameLift Servers create backfill requests itself
/// (`smithy_query shape=BackfillMode`): that member's own doc — "Specify AUTOMATIC to have
/// GameLift create a match backfill request whenever a game session has one or more open
/// slots."
pub const BACKFILL_MODE_AUTOMATIC: &str = "AUTOMATIC";

/// The `BackfillMode` a configuration created without one carries: that member's own doc —
/// "MANUAL indicates that the game makes backfill requests or does not use the match
/// backfill feature", which is what a configuration that asked for no backfilling does.
/// Both values come from the modelled enum (`smithy_query shape=BackfillMode`), which
/// `BACKFILL_MODE_VALUES` carries.
pub const DEFAULT_BACKFILL_MODE: &str = "MANUAL";

/// The `AdditionalPlayerCount` a configuration created without one carries: that member is
/// "the number of player slots in a match to keep open for future players", so a
/// configuration that asked for none keeps none open — the minimum of the modelled
/// `WholeNumber` (range min 0).
pub const DEFAULT_ADDITIONAL_PLAYER_COUNT: i32 = 0;

/// `MatchmakingConfigurationStatus` value a ticket `StartMatchmaking` has just accepted
/// carries: that operation's own doc — "If successful, a matchmaking ticket is returned
/// with status set to `QUEUED`."
pub const MATCHMAKING_TICKET_STATUS_QUEUED: &str = "QUEUED";
/// `MatchmakingConfigurationStatus` a ticket moves to once FlexMatch picks it up:
/// "`SEARCHING` -- The matchmaking request is currently being processed."
pub const MATCHMAKING_TICKET_STATUS_SEARCHING: &str = "SEARCHING";
/// `MatchmakingConfigurationStatus` a ticket ends in when no match is formed for it:
/// "`TIMED_OUT` -- The matchmaking request was not successful within the duration
/// specified in the matchmaking configuration."
pub const MATCHMAKING_TICKET_STATUS_TIMED_OUT: &str = "TIMED_OUT";
/// `MatchmakingConfigurationStatus` a ticket a caller stopped ends in: "`CANCELLED` -- The
/// matchmaking request was canceled. This may be the result of a `StopMatchmaking`
/// operation or a proposed match that one or more players failed to accept."
pub const MATCHMAKING_TICKET_STATUS_CANCELLED: &str = "CANCELLED";
/// `MatchmakingConfigurationStatus` a ticket whose match was made carries: "`COMPLETED` --
/// Players have been matched and a game session is ready to host the players."
pub const MATCHMAKING_TICKET_STATUS_COMPLETED: &str = "COMPLETED";
/// `MatchmakingConfigurationStatus` a ticket that did not complete carries: "`FAILED` --
/// The matchmaking request was not completed."
pub const MATCHMAKING_TICKET_STATUS_FAILED: &str = "FAILED";

/// The statuses a matchmaking request has STOPPED being processed in, i.e. the ones from
/// which it never moves again.
///
/// `MatchmakingTicket$Status`'s own docs call `COMPLETED` the status of a request whose
/// players "have been matched and a game session is ready to host" them, and note that
/// "matchmaking requests that fail to successfully complete (statuses `FAILED`,
/// `CANCELLED`, `TIMED_OUT`) can be resubmitted as new requests with new ticket IDs" — a
/// request in any of those four has finished, so the rest of the modelled statuses
/// (`QUEUED`, `SEARCHING`, `REQUIRES_ACCEPTANCE`, `PLACING`) are the ones a request is
/// still being worked on in.
pub const MATCHMAKING_TICKET_TERMINAL_STATUSES: [&str; 4] = [
    MATCHMAKING_TICKET_STATUS_COMPLETED,
    MATCHMAKING_TICKET_STATUS_FAILED,
    MATCHMAKING_TICKET_STATUS_CANCELLED,
    MATCHMAKING_TICKET_STATUS_TIMED_OUT,
];

/// `GameSessionPlacementState` a new placement request carries: `GameSessionPlacement`'s
/// own doc — "If the game session placement status is `PENDING`, the properties for game
/// session ID/ARN, region, IP address/DNS, and port aren't final."
pub const PLACEMENT_STATUS_PENDING: &str = "PENDING";
/// `GameSessionPlacementState` a placement reaches once a game session has started for
/// it: "When the request status is `FULFILLED`, a new game session has started and the
/// placement request is updated with connection information for the game session (IP
/// address and port)."
pub const PLACEMENT_STATUS_FULFILLED: &str = "FULFILLED";
/// `GameSessionPlacementState` a placement a caller stopped ends in:
/// StopGameSessionPlacement's doc — "this operation removes the placement request from the
/// queue and moves the `GameSessionPlacement` to `CANCELLED` status."
pub const PLACEMENT_STATUS_CANCELLED: &str = "CANCELLED";
/// `GameSessionPlacementState` a placement ends in when no destination of its queue could
/// host the session: StartGameSessionPlacement's doc — "Amazon GameLift Servers continues
/// to retry each placement request until it reaches the queue's timeout setting. If a
/// request times out, you can resubmit the request to the same queue or try a different
/// queue."
pub const PLACEMENT_STATUS_TIMED_OUT: &str = "TIMED_OUT";

/// The prefix every custom location's name carries, from
/// `CustomInputLocationStringModel`'s pattern (`^custom-[A-Za-z0-9\-]+$`): it is what
/// tells a custom location apart from an Amazon Web Services one, which `ListLocations`
/// reports under the `CUSTOM` and `AWS` filters respectively.
pub const CUSTOM_LOCATION_PREFIX: &str = "custom-";

/// Whether `location` names a custom location rather than an Amazon Web Services one.
pub fn is_custom_location(location: &str) -> bool {
    location.starts_with(CUSTOM_LOCATION_PREFIX)
}

/// `PlayerSessionCreationPolicy` a new game session carries: CreateGameSession's own
/// doc — "By default, newly created game sessions are open to new players."
pub const PLAYER_SESSION_CREATION_POLICY_ACCEPT_ALL: &str = "ACCEPT_ALL";

/// `ProtectionPolicy` default: CreateFleet's `NewGameSessionProtectionPolicy` doc —
/// "By default, this property is set to NoProtection."
pub const PROTECTION_POLICY_NO_PROTECTION: &str = "NoProtection";

/// `FleetType` default: CreateFleet's `FleetType` doc — "By default, this property is
/// set to ON_DEMAND."
pub const FLEET_TYPE_ON_DEMAND: &str = "ON_DEMAND";

/// `CertificateConfiguration.CertificateType` that puts a fleet behind TLS, which
/// decides which of the two documented `DnsName` forms a game session reports.
pub const CERTIFICATE_TYPE_GENERATED: &str = "GENERATED";

/// `CertificateConfiguration.CertificateType` default: that member's own doc —
/// "DISABLED - (default) Do not generate a TLS/SSL certificate for this fleet." A fleet
/// created without a `CertificateConfiguration` carries this one, so every fleet reports
/// the member the service decided for it.
pub const CERTIFICATE_TYPE_DISABLED: &str = "DISABLED";

/// `PlayerGatewayMode` default: CreateFleet's `PlayerGatewayMode` doc — "`DISABLED`
/// (default) -- Game clients connect to the game server endpoint."
pub const PLAYER_GATEWAY_MODE_DISABLED: &str = "DISABLED";

/// The CloudWatch metric group a fleet created without one is added to.
///
/// `FleetAttributes$MetricGroups`' doc: "Name of a metric group that metrics for this
/// fleet are added to. [...] A fleet can be included in only one metric group at a
/// time" — so a fleet is always in exactly one group, and a `CreateFleet` that names
/// none leaves the fleet in the service's own group. The model does not name that group;
/// the recorded wire responses of this service report it as `default`.
pub const DEFAULT_METRIC_GROUP: &str = "default";

/// The upper bound on `MaximumPlayerSessionCount`. CreatePlayerSession's doc: "The
/// maximum number of players per game session is 200. It is not adjustable." The model
/// leaves the member unbounded above, so the service rule is enforced here.
pub const MAX_PLAYERS_PER_GAME_SESSION: i32 = 200;

/// How many server processes one fleet compute may run at once, and so how many game
/// sessions it can host: `RuntimeConfiguration`'s doc — "An Amazon GameLift Servers
/// instance is limited to 50 processes running concurrently. To calculate the total
/// number of processes defined in a runtime configuration, add the values of the
/// `ConcurrentExecutions` parameter for each server process" — and each running server
/// process hosts one game session at a time.
pub const MAX_SERVER_PROCESSES_PER_INSTANCE: i32 = 50;

/// How many fleets this emulator holds before `CreateFleet` answers the modelled
/// `LimitExceededException` ("The requested operation would cause the resource to exceed
/// the allowed service limit").
///
/// Amazon GameLift Servers caps active fleets per account per Region; this emulator
/// keeps ONE store for the account rather than one per Region — a fleet is reported by
/// every describe whatever Region the request is signed for, and carries the Region of
/// the request that created it in its ARN and home location — so this cap, like the
/// store, is account-wide. The model carries no quota, so the count is CHOSEN: far above
/// any realistic script, and the bound that keeps the fleet store (and, through each
/// fleet's capacity, the game-session and player-session stores) from growing without
/// limit.
pub const MAX_FLEETS_PER_ACCOUNT: usize = 100;

/// How many computes one fleet holds before `RegisterCompute` answers the modelled
/// `LimitExceededException` ("The requested operation would cause the resource to exceed
/// the allowed service limit").
///
/// Like [`MAX_FLEETS_PER_ACCOUNT`], the model carries no quota, so the count is CHOSEN:
/// far above any realistic script — a compute is one hosting resource of a fleet — and the
/// bound that keeps the compute store from growing without limit as computes are
/// registered.
pub const MAX_COMPUTES_PER_FLEET: usize = 100;

/// How many fleet identifiers one `DescribeFleetAttributes` request may name.
///
/// That operation's own doc: "Some API operations limit the number of fleet IDs that are
/// allowed in one request. If a request exceeds this limit, the request fails and the
/// error message contains the maximum allowed number." The model bounds
/// `FleetIdOrArnList` only below (`len 1..`) and the number itself is not documented, so
/// this count is CHOSEN: it is [`MAX_FLEETS_PER_ACCOUNT`], the most distinct fleets that
/// can exist, so no legitimate request is refused — and naming more than that can only
/// be repetition, which would otherwise let one request drive an unbounded response.
pub const MAX_FLEET_IDS_PER_REQUEST: usize = MAX_FLEETS_PER_ACCOUNT;

/// How many aliases this emulator holds before `CreateAlias` answers the modelled
/// `LimitExceededException` ("The requested operation would cause the resource to exceed
/// the allowed service limit").
///
/// Like [`MAX_FLEETS_PER_ACCOUNT`], Amazon GameLift Servers caps aliases per account per
/// Region while this emulator keeps ONE store for the account — an alias is reported
/// whatever Region the request is signed for, and carries the Region of the request that
/// created it in its ARN — so the cap is account-wide. The model carries no quota, so the
/// count is CHOSEN: far above any realistic script ("a fleet can have multiple aliases"),
/// and the bound that keeps the alias store from growing without limit.
pub const MAX_ALIASES_PER_ACCOUNT: usize = 100;

/// How many game session queues this emulator holds before `CreateGameSessionQueue`
/// answers the modelled `LimitExceededException` ("The requested operation would cause
/// the resource to exceed the allowed service limit").
///
/// Like [`MAX_FLEETS_PER_ACCOUNT`], Amazon GameLift Servers caps queues per account per
/// Region while this emulator keeps ONE store for the account, so the cap is
/// account-wide. The model carries no quota, so the count is CHOSEN: far above any
/// realistic script, and the bound that keeps the queue store from growing without limit.
pub const MAX_GAME_SESSION_QUEUES_PER_ACCOUNT: usize = 50;

/// How many queue identifiers one `DescribeGameSessionQueues` request may name.
///
/// The model bounds `GameSessionQueueNameOrArnList` neither above nor below, and each
/// entry that is an ARN is resolved by a scan of the queue store, so the work one request
/// can ask for is the PRODUCT of the list length and the stored queue count. This count
/// is CHOSEN, exactly as [`MAX_FLEET_IDS_PER_REQUEST`] is: it is
/// [`MAX_GAME_SESSION_QUEUES_PER_ACCOUNT`], the most distinct queues that can exist, so
/// no legitimate request is refused — and naming more than that can only be repetition.
pub const MAX_QUEUE_NAMES_PER_REQUEST: usize = MAX_GAME_SESSION_QUEUES_PER_ACCOUNT;

/// How many destinations one game session queue may list.
///
/// `GameSessionQueueDestinationList` carries no modelled length, and a stored queue holds
/// the list verbatim for as long as the queue exists, so this count is CHOSEN: far above
/// any realistic queue — a destination is a fleet or alias of the account, which
/// [`MAX_FLEETS_PER_ACCOUNT`] already bounds — and the bound that keeps one request from
/// deciding how much a stored record occupies.
pub const MAX_QUEUE_DESTINATIONS: usize = MAX_FLEETS_PER_ACCOUNT;

/// How many player latency policies one game session queue may list.
///
/// `PlayerLatencyPolicyList` carries no modelled length either, and the policies are
/// "evaluated in order starting with the lowest maximum latency value" to "gradually
/// relax the cap over time", so a queue needs a handful. This count is CHOSEN for the
/// same reason as [`MAX_QUEUE_DESTINATIONS`]: it bounds what one request can make a
/// stored record occupy.
pub const MAX_QUEUE_PLAYER_LATENCY_POLICIES: usize = 10;

/// The bounds a game session queue's `TimeoutInSeconds` must lie within.
///
/// `CreateGameSessionQueue$TimeoutInSeconds` and `GameSessionQueue$TimeoutInSeconds`
/// carry the same note: "The minimum value is 10 and the maximum value is 600." The
/// modelled `WholeNumber` range bounds the member only below (`0..`), so the documented
/// bounds are enforced here.
pub const QUEUE_TIMEOUT_RANGE_SECONDS: (i32, i32) = (10, 600);

/// The `TimeoutInSeconds` a queue created without one carries.
///
/// `CreateGameSessionQueue$TimeoutInSeconds`' doc: "If you don't specify a request
/// timeout, the queue uses a default value." Neither the model nor the docs name that
/// value, so it is CHOSEN: the documented maximum
/// ([`QUEUE_TIMEOUT_RANGE_SECONDS`]`.1`), the most permissive setting, which keeps a
/// placement request queued for as long as the service allows before it times out.
pub const DEFAULT_QUEUE_TIMEOUT_SECONDS: i32 = QUEUE_TIMEOUT_RANGE_SECONDS.1;

/// How many custom locations this emulator holds before `CreateLocation` answers the
/// modelled `LimitExceededException`.
///
/// Like [`MAX_FLEETS_PER_ACCOUNT`], Amazon GameLift Servers caps custom locations per
/// account per Region while this emulator keeps ONE store for the account, so the cap is
/// account-wide. The model carries no quota, so the count is CHOSEN: far above any
/// realistic script, and the bound that keeps the location store from growing without
/// limit.
pub const MAX_CUSTOM_LOCATIONS_PER_ACCOUNT: usize = 100;

/// How many Realtime scripts this emulator holds before `CreateScript` refuses the next
/// create.
///
/// CHOSEN for the same reason as [`MAX_CUSTOM_LOCATIONS_PER_ACCOUNT`]: the model carries
/// no quota, a stored script record lives until `DeleteScript` removes it, and this is the
/// bound that keeps the script store from growing without limit. `CreateScript` models no
/// `LimitExceededException`, so the create past the bound answers the modelled
/// `ConflictException` — "the requested operation would cause a conflict with the current
/// state of a service resource associated with the request" — the one modelled client
/// error that is about the state of the account's resources rather than about a parameter
/// value.
pub const MAX_SCRIPTS_PER_ACCOUNT: usize = 100;

/// How many custom game builds this emulator holds before `CreateBuild` refuses the next
/// create. CHOSEN for the same reason as [`MAX_SCRIPTS_PER_ACCOUNT`], and answered with
/// the same modelled `ConflictException`: a stored build record lives until `DeleteBuild`
/// removes it, and `CreateBuild` models no `LimitExceededException`.
pub const MAX_BUILDS_PER_ACCOUNT: usize = 100;

/// How many Regions this service stands a build-upload bucket up in before `CreateBuild`
/// refuses to hand out an upload location in a further one.
///
/// The service runs one upload bucket per Region ([`service_owned_build_bucket`]) and the
/// Region is the one the request was served in, so this is the bound that keeps the Regions
/// a sequence of requests names from growing the object store without limit: a bucket is
/// stood up for the Region's uploads rather than for one build, so nothing releases it when
/// a build goes, and [`MAX_BUILDS_PER_ACCOUNT`] does not bound it. CHOSEN for the same
/// reason as [`MAX_CUSTOM_LOCATIONS_PER_ACCOUNT`]: far above the Regions Amazon GameLift
/// Servers hosts in, so no realistic script reaches it.
pub const MAX_BUILD_UPLOAD_BUCKETS: usize = 100;

/// How many matchmaking rule sets this emulator holds before `CreateMatchmakingRuleSet`
/// answers the modelled `LimitExceededException`. CHOSEN for the same reason as
/// [`MAX_CUSTOM_LOCATIONS_PER_ACCOUNT`]: the model carries no quota, and a stored rule set
/// holds a body of up to the modelled 65535 characters for as long as it exists.
pub const MAX_MATCHMAKING_RULE_SETS_PER_ACCOUNT: usize = 100;

/// How many matchmaking configurations this emulator holds before
/// `CreateMatchmakingConfiguration` answers the modelled `LimitExceededException`. CHOSEN
/// for the same reason as [`MAX_CUSTOM_LOCATIONS_PER_ACCOUNT`].
pub const MAX_MATCHMAKING_CONFIGURATIONS_PER_ACCOUNT: usize = 100;

/// How many matchmaking tickets this store holds before `StartMatchmaking` refuses the
/// next one with the modelled `InvalidRequestException` (the operation models no
/// `LimitExceededException`).
///
/// A ticket lives until it is deleted, and nothing in this emulator deletes one, so this
/// count is CHOSEN: far above any realistic script, and the bound that keeps the ticket
/// store from growing without limit.
pub const MAX_MATCHMAKING_TICKETS_PER_ACCOUNT: usize = 200;

/// How many game session placements this store holds before `StartGameSessionPlacement`
/// refuses the next one with the modelled `InvalidRequestException` (that operation models
/// no `LimitExceededException` either). CHOSEN for the same reason as
/// [`MAX_MATCHMAKING_TICKETS_PER_ACCOUNT`].
pub const MAX_GAME_SESSION_PLACEMENTS_PER_ACCOUNT: usize = 200;

/// How many ticket ids one `DescribeMatchmaking` request may name: that operation's own
/// doc — "To request matchmaking tickets, provide a list of up to 10 ticket IDs." The
/// modelled `MatchmakingIdList` carries no length, so the documented maximum is enforced
/// where the request is served.
pub const MAX_TICKET_IDS_PER_REQUEST: usize = 10;

/// How many configuration names one `DescribeMatchmakingConfigurations` request may name.
///
/// The model bounds `MatchmakingConfigurationNameList` neither above nor below, and each
/// entry that is an ARN is resolved by a scan of the configuration store, so the work one
/// request can ask for is the PRODUCT of the list length and the stored configuration
/// count. This count is CHOSEN exactly as [`MAX_QUEUE_NAMES_PER_REQUEST`] is: it is
/// [`MAX_MATCHMAKING_CONFIGURATIONS_PER_ACCOUNT`], the most distinct configurations that
/// can exist, so no legitimate request is refused.
pub const MAX_CONFIGURATION_NAMES_PER_REQUEST: usize = MAX_MATCHMAKING_CONFIGURATIONS_PER_ACCOUNT;

/// The port range a fleet that opens no inbound permissions draws game-session ports
/// from, and the range an opened one must lie within. `IpPermission$FromPort`'s doc:
/// "For fleets using Windows builds, only ports `1026-60000` are valid."
pub const DEFAULT_PORT_RANGE: (i32, i32) = (1026, 60000);

/// The one port outside [`DEFAULT_PORT_RANGE`] a fleet may open. Same doc: "For fleets
/// using Linux builds, only ports `22` and `1026-60000` are valid."
pub const REMOTE_ACCESS_PORT: i32 = 22;

/// Whether `from`-`to` is a port range a fleet may open: it must run forwards and lie
/// within [`DEFAULT_PORT_RANGE`], or be [`REMOTE_ACCESS_PORT`] alone.
pub fn valid_port_range(from: i32, to: i32) -> bool {
    from <= to
        && ((from == REMOTE_ACCESS_PORT && to == REMOTE_ACCESS_PORT)
            || (from >= DEFAULT_PORT_RANGE.0 && to <= DEFAULT_PORT_RANGE.1))
}

/// The `GameSessionStatus` values `DescribeGameSessions`'s `StatusFilter` accepts. Its
/// doc: "You can filter on the following states: `ACTIVE`, `TERMINATED`, `ACTIVATING`,
/// and `TERMINATING`."
pub const STATUS_FILTER_VALUES: [&str; 4] = ["ACTIVE", "TERMINATED", "ACTIVATING", "TERMINATING"];

/// The region an unsigned request (no SigV4 credential scope to read a region from) is
/// served in.
pub const DEFAULT_REGION: &str = "us-east-1";

/// How long after the call that starts it a status change this service owes comes due:
/// `CreateFleet` places a managed EC2 fleet in `NEW` and it becomes `ACTIVE` when the
/// fleet-creation workflow finishes; `CreateGameSession` returns a session in
/// `ACTIVATING` and "when the game session status is `ACTIVE`, it is updated with
/// connection information"; `DeleteFleet` moves a fleet to `DELETING` until the
/// deletion completes.
///
/// Amazon GameLift Servers takes minutes over each of those and documents no duration,
/// so this span is CHOSEN: short enough for a caller to wait for, and long enough that
/// a caller acting straight after the call still observes the transient status and the
/// state errors it causes.
pub const WORKFLOW_DELAY: Duration = Duration::from_millis(400);

/// How long an authentication token issued for a compute stays valid.
///
/// `GetComputeAuthToken`'s own doc: "Authentication tokens are valid for a limited time
/// span, so you need to request a fresh token before the current token expires", and
/// `GetComputeAuthTokenOutput$ExpirationTimestamp` is "the amount of time until the
/// authentication token is no longer valid". Neither the model nor the docs name that
/// span, so it is CHOSEN: long enough for the game server processes on a compute to start
/// and pass the token they hold to the server SDK's `InitSDK()`, and short enough that the
/// token is plainly temporary and has to be re-requested over the life of a compute.
pub const COMPUTE_AUTH_TOKEN_VALIDITY: Duration = Duration::from_secs(15 * 60);

/// A fresh opaque id of the shape AWS mints for a GameLift resource: a UUID, which the
/// modelled patterns accept (`FleetId` `^[a-z]*fleet-[a-zA-Z0-9\-]+$`, `PlayerSessionId`
/// `^psess-\S+$`, `IdStringModel` `^[a-zA-Z0-9-]+$`) and the docs show
/// (`arn:aws:gamelift:<region>::fleet/fleet-a1234567-...`).
///
/// The UUID is a digest of this store's id counter — unique within the process — folded
/// with a per-process seed so two emulator processes do not mint the same ids, hashed so
/// the id discloses neither. It takes the counter rather than `&mut self` so a handler
/// holding a stored record borrowed can still mint, which is what lets it mint only
/// AFTER every refusal has been answered. This mirrors `mint_workspace_id` in the
/// Amazon Managed Prometheus slice rather than adding a dependency for it.
pub fn mint_uuid(counter: &mut u64, kind: &str) -> String {
    *counter += 1;
    let mut hasher = Sha256::new();
    hasher.update(kind.as_bytes());
    hasher.update(counter.to_be_bytes());
    hasher.update(process_seed().to_be_bytes());
    let digest: [u8; 32] = hasher.finalize().into();
    let hex = hex::encode(digest);
    // A SHA-256 digest renders as 64 hexadecimal characters, so the grouping always applies.
    as_uuid(&hex).unwrap_or(hex)
}

/// The first 32 hexadecimal characters of `hex`, written the way a UUID is (8-4-4-4-12),
/// or `None` when `hex` holds fewer than 32.
///
/// The one place this emulator writes that grouping, so every UUID it hands out — a
/// resource id ([`mint_uuid`]) and a compute's authentication token
/// ([`mint_compute_auth_token`]) alike — reads the same way.
fn as_uuid(hex: &str) -> Option<String> {
    (hex.len() >= 32).then(|| {
        format!(
            "{}-{}-{}-{}-{}",
            &hex[0..8],
            &hex[8..12],
            &hex[12..16],
            &hex[16..20],
            &hex[20..32]
        )
    })
}

/// A seed drawn once per process, so ids minted by two emulator processes from the same
/// counter still differ.
fn process_seed() -> u128 {
    static SEED: OnceLock<u128> = OnceLock::new();
    *SEED.get_or_init(|| {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|since| since.as_nanos())
            .unwrap_or_default()
    })
}

/// An ARN for a GameLift resource: `arn:aws:gamelift:<region>:<account>:<resource>`.
///
/// Every segment the model documents for `FleetAttributes$FleetArn` ("Format is
/// `arn:aws:gamelift:<region>::fleet/fleet-a1234567-...`") and for
/// `GameSession$GameSessionId` ("`arn:aws:gamelift:<home_region>::gamesession/<fleet
/// ID>/<ID string>`"), with the account segment POPULATED: those prose examples render
/// the account empty, but the service's own responses carry the account id of the
/// calling account, and an ARN is only usable as an IAM resource — or splittable back
/// into its owning account — when that segment is there. So this delegates to
/// [`::engine::emulator_arn`], the single source of the one account every emulated
/// response presents.
pub fn gamelift_arn(region: &str, resource: &str) -> String {
    ::engine::emulator_arn("gamelift", region, resource)
}

/// The region the caller addressed, read from the request's SigV4 credential scope.
pub fn request_region(ctx: &smithy_server_runtime::OperationContext<'_>) -> String {
    ::emulator_server::sigv4_credential_scope(&ctx.parts().headers)
        .map_or_else(|| DEFAULT_REGION.to_string(), |scope| scope.region)
}

/// A modelled timestamp for a wall-clock instant.
///
/// `aws_smithy_types` owns the epoch handling: this is its own `From<SystemTime>`
/// conversion, named here only so every record renders its instants the one way.
pub fn to_date_time(instant: SystemTime) -> ::aws_smithy_types::DateTime {
    ::aws_smithy_types::DateTime::from(instant)
}

/// Split a `<kind>IdOrArn` member into its id and its ARN, whichever of the two the
/// caller supplied: the model's ARN docs say "the resource ID matches the `<kind>Id`
/// value", so each is derivable from the other.
pub fn id_and_arn(value: &str, region: &str, kind: &str) -> (String, String) {
    if value.starts_with("arn:") {
        let id = value.rsplit('/').next().unwrap_or(value).to_string();
        (id, value.to_string())
    } else {
        let arn = gamelift_arn(region, &format!("{kind}/{value}"));
        (value.to_string(), arn)
    }
}

/// Which collection a page token walks. Mixed into the token so one paginated describe
/// refuses a token another minted, instead of reading it as an offset into its own set.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PageKind {
    /// `ListAliases`.
    Aliases,
    /// `DescribeFleetAttributes`.
    Fleets,
    /// `DescribeGameSessions`.
    GameSessions,
    /// `DescribeGameSessionQueues`.
    GameSessionQueues,
    /// `ListLocations`.
    Locations,
    /// `ListCompute`.
    Computes,
    /// `DescribeMatchmakingRuleSets`.
    MatchmakingRuleSets,
    /// `DescribeMatchmakingConfigurations`.
    MatchmakingConfigurations,
}

impl PageKind {
    /// The discriminator this collection's tokens carry.
    const fn tag(self) -> char {
        match self {
            Self::Aliases => 'a',
            Self::Fleets => 'f',
            Self::GameSessions => 'g',
            Self::GameSessionQueues => 'q',
            Self::Locations => 'l',
            Self::Computes => 'm',
            Self::MatchmakingRuleSets => 'r',
            Self::MatchmakingConfigurations => 'c',
        }
    }
}

/// Encode a page boundary as the opaque `NextToken` the paginated describes hand out.
///
/// A collection tag plus fixed-width hex, so a token carries no readable offset and
/// every token has the same shape; [`decode_page_token`] is the only reader.
pub fn encode_page_token(kind: PageKind, offset: usize) -> String {
    format!("{}{offset:015x}", kind.tag())
}

/// Decode a token [`encode_page_token`] produced for `kind`. `None` for anything else —
/// including a token of another collection — which a handler answers with
/// `InvalidRequestException`.
pub fn decode_page_token(kind: PageKind, token: &str) -> Option<usize> {
    let offset = token.strip_prefix(kind.tag())?;
    if offset.len() != 15 {
        return None;
    }
    usize::from_str_radix(offset, 16).ok()
}

/// One page of `items`, starting at `next_token`'s boundary and at most `limit` long,
/// plus the token for the page after it.
///
/// `None` means the token was not one this service minted for `kind`.
pub fn paginate<T: Clone>(
    kind: PageKind,
    items: &[T],
    limit: Option<i32>,
    next_token: Option<&str>,
) -> Option<(Vec<T>, Option<String>)> {
    let start = match next_token {
        Some(token) => decode_page_token(kind, token)?.min(items.len()),
        None => 0,
    };
    // `Limit` is modelled `PositiveInteger` with range min 1, so the boundary walk has
    // already refused 0 and below; absent means "no page size", i.e. the whole tail.
    let page_size = limit.map_or(items.len(), |limit| limit as usize);
    let end = start.saturating_add(page_size).min(items.len());
    let page = items[start..end].to_vec();
    let next = if end < items.len() {
        Some(encode_page_token(kind, end))
    } else {
        None
    };
    Some((page, next))
}

/// The most tags one resource carries: the length the modelled `TagList` declares
/// (`smithy_query shape=TagList`, len 0..200), which is the longest list a tag operation
/// can carry for a resource and so the ceiling on what one can hold. `TagResource`'s own
/// `Tags` doc says "the maximum tag limit may be lower than stated" without stating one,
/// so the modelled length is the bound this store keeps.
pub const MAX_TAGS_PER_RESOURCE: usize = 200;

/// The first key a tag list assigns more than once, or `None` when every key is distinct.
///
/// "Tags are developer-defined and structured as key-value pairs", so one key cannot
/// carry two values — a list that assigns one twice is not a tag set.
pub fn first_repeated_tag_key(tags: &[Tag]) -> Option<&str> {
    let mut keys = HashSet::new();
    tags.iter()
        .find(|tag| !keys.insert(tag.key.as_str()))
        .map(|tag| tag.key.as_str())
}

/// What the service answers when a tag set assigns one key twice — the message of the
/// modelled `TaggingFailedException`, whose doc names the class: "This may be due to
/// invalid tag format".
pub fn repeated_tag_key_message(key: &str) -> String {
    format!(
        "Tags assigns the key {key} more than once; a resource's tags carry each key at \
         most once."
    )
}

/// What the service answers when a `ResourceARN` names no taggable resource this store
/// holds — the message of the modelled `NotFoundException` ("The resource was either not
/// created yet or deleted"), shared by the tag operations so the same missing resource
/// reads the same way whichever of them was called.
///
/// The ARN the CALLER supplied is the sentence's one variable part, and the sentence is
/// the shape this service answers a failed lookup in elsewhere
/// ([`matchmaking_rule_set_lookup_failure_message`]).
pub fn tagged_resource_not_found_message(resource_arn: &str) -> String {
    format!("Failed to find resource: {resource_arn}")
}

/// The resource segment of an ARN, split into the kind it names and the resource id:
/// `arn:<partition>:<service>:<region>:<account>:<kind>/<id>`.
///
/// `None` for anything that is not an ARN with a `<kind>/<id>` resource, which names no
/// resource of this service.
fn arn_resource_parts(arn: &str) -> Option<(&str, &str)> {
    let resource = arn.strip_prefix("arn:")?.splitn(5, ':').nth(4)?;
    resource.split_once('/')
}

/// A fleet of compute resources, as `CreateFleet` persisted it.
#[derive(Debug, Clone)]
pub struct Fleet {
    pub fleet_id: String,
    pub fleet_arn: String,
    pub name: String,
    pub description: Option<String>,
    pub status: String,
    /// When `NEW` becomes `ACTIVE`; `None` once the fleet is no longer provisioning.
    pub active_at: Option<SystemTime>,
    /// When a `DELETING` fleet and everything placed on it leave state.
    pub purge_at: Option<SystemTime>,
    pub fleet_type: String,
    pub instance_type: Option<String>,
    pub compute_type: String,
    pub build_id: Option<String>,
    pub build_arn: Option<String>,
    pub script_id: Option<String>,
    pub script_arn: Option<String>,
    pub server_launch_path: Option<String>,
    pub server_launch_parameters: Option<String>,
    pub log_paths: Option<Vec<String>>,
    pub runtime_configuration: Option<RuntimeConfiguration>,
    pub resource_creation_limit_policy: Option<ResourceCreationLimitPolicy>,
    pub certificate_configuration: Option<CertificateConfiguration>,
    pub anywhere_configuration: Option<AnywhereConfiguration>,
    pub player_gateway_mode: Option<String>,
    pub player_gateway_configuration: Option<PlayerGatewayConfiguration>,
    pub instance_role_arn: Option<String>,
    pub instance_role_credentials_provider: Option<String>,
    pub ec2_inbound_permissions: Vec<IpPermission>,
    pub new_game_session_protection_policy: String,
    pub metric_groups: Vec<String>,
    /// The fleet's home location followed by every remote location it was created with.
    pub locations: Vec<FleetLocation>,
    pub home_location: String,
    pub region: String,
    pub tags: Vec<Tag>,
    pub creation_time: SystemTime,
    pub termination_time: Option<SystemTime>,
    /// The address of the instance hosting this fleet's game servers.
    pub ip_address: String,
    /// Where the next port handout starts: a rotating cursor over the ports this fleet
    /// opens, advanced past every port it hands out and never rewound, so consecutive
    /// sessions get consecutive ports (see [`Fleet::next_game_session_port`]).
    pub port_cursor: u32,
}

/// One location a fleet deploys to, and that location's own life-cycle status.
#[derive(Debug, Clone)]
pub struct FleetLocation {
    pub location: String,
    pub status: String,
    /// How many server processes are running at this location and available to host a
    /// game session, i.e. this location's game-session capacity. See
    /// [`server_processes_per_location`] for where the number comes from.
    pub server_processes: usize,
}

/// How many server processes a fleet location runs, and so how many game sessions that
/// location can host at once.
///
/// A managed (`EC2`) fleet is hosted on instances Amazon GameLift Servers provisions and
/// launches the fleet's server processes on itself, so its locations run the processes
/// the fleet's `RuntimeConfiguration` asks for — `RuntimeConfiguration`'s doc: "To
/// calculate the total number of processes defined in a runtime configuration, add the
/// values of the `ConcurrentExecutions` parameter for each server process" — capped at
/// the [`MAX_SERVER_PROCESSES_PER_INSTANCE`] one instance may run, and given that same
/// per-instance maximum when the fleet carries no runtime configuration, which
/// `CreateFleet`'s member doc allows. `ComputeType` declares exactly `EC2` and `ANYWHERE`
/// (`smithy_query shape=ComputeType`), and `CreateFleet` applies the managed-fleet rules
/// to everything that is not `ANYWHERE`, so this does too: the two stay in step.
///
/// An `ANYWHERE` fleet runs NO server process of its own. Its game servers run on compute
/// resources the customer registers (`RegisterCompute`'s doc: "Registers a compute
/// resource in an Amazon GameLift Servers Anywhere fleet"), and each server process on
/// such a compute becomes active and available only by announcing itself through the
/// server SDK ("Each server process includes the SDK endpoint in its call to the Amazon
/// GameLift Servers server SDK action `InitSDK()`") — a game-server-side protocol, not an
/// API this emulator serves. So no process on an Anywhere fleet location is ever active
/// and available here, its capacity is zero, and `CreateGameSession` against it answers
/// the modelled `FleetCapacityExceededException`, as the real service does for an
/// Anywhere fleet with no active, available server process.
pub fn server_processes_per_location(compute_type: &str, configured_processes: i32) -> usize {
    if compute_type == COMPUTE_TYPE_ANYWHERE {
        return 0;
    }
    let processes = if configured_processes > 0 {
        configured_processes.min(MAX_SERVER_PROCESSES_PER_INSTANCE)
    } else {
        MAX_SERVER_PROCESSES_PER_INSTANCE
    };
    usize::try_from(processes).unwrap_or(0)
}

/// The total `ConcurrentExecutions` a runtime configuration asks for, saturating rather
/// than overflowing on a sum no instance could run (the caller refuses it).
pub fn configured_server_processes(configuration: Option<&RuntimeConfiguration>) -> i32 {
    configuration
        .and_then(|configuration| configuration.server_processes.as_ref())
        .map_or(0, |processes| {
            processes.iter().fold(0_i32, |total, process| {
                total.saturating_add(process.concurrent_executions)
            })
        })
}

impl Fleet {
    /// This fleet as the modelled `FleetAttributes`.
    pub fn attributes(&self) -> FleetAttributes {
        FleetAttributes {
            fleet_id: Some(self.fleet_id.clone()),
            fleet_arn: Some(self.fleet_arn.clone()),
            fleet_type: Some(self.fleet_type.clone()),
            instance_type: self.instance_type.clone(),
            description: self.description.clone(),
            name: Some(self.name.clone()),
            creation_time: Some(to_date_time(self.creation_time)),
            termination_time: self.termination_time.map(to_date_time),
            status: Some(self.status.clone()),
            build_id: self.build_id.clone(),
            build_arn: self.build_arn.clone(),
            script_id: self.script_id.clone(),
            script_arn: self.script_arn.clone(),
            server_launch_path: self.server_launch_path.clone(),
            server_launch_parameters: self.server_launch_parameters.clone(),
            log_paths: self.log_paths.clone(),
            new_game_session_protection_policy: Some(
                self.new_game_session_protection_policy.clone(),
            ),
            // `OperatingSystem` reports the build's OS, and builds are a separate slice
            // that still answers 501, so there is nothing stored to report here.
            operating_system: None,
            resource_creation_limit_policy: self.resource_creation_limit_policy.clone(),
            metric_groups: Some(self.metric_groups.clone()),
            // Fleet actions are started and stopped by StartFleetActions /
            // StopFleetActions, which are a separate slice: nothing has stopped.
            stopped_actions: None,
            instance_role_arn: self.instance_role_arn.clone(),
            certificate_configuration: self.certificate_configuration.clone(),
            compute_type: Some(self.compute_type.clone()),
            anywhere_configuration: self.anywhere_configuration.clone(),
            instance_role_credentials_provider: self.instance_role_credentials_provider.clone(),
            player_gateway_mode: self.player_gateway_mode.clone(),
            player_gateway_configuration: self.player_gateway_configuration.clone(),
        }
    }

    /// This fleet's locations and the life-cycle status of each.
    pub fn location_states(&self) -> Vec<LocationState> {
        self.locations
            .iter()
            .map(|location| LocationState {
                location: Some(location.location.clone()),
                status: Some(location.status.clone()),
                player_gateway_status: None,
            })
            .collect()
    }

    /// Whether Amazon GameLift Servers owns and runs this fleet's compute.
    ///
    /// `ComputeType`'s doc distinguishes the two: an `EC2` fleet's instances are managed
    /// by the service, while an `ANYWHERE` fleet "represents compute resources that are
    /// not owned or managed by Amazon GameLift Servers". Deleting a fleet "shuts down
    /// any currently running fleet instances", so only a fleet WITH managed instances
    /// has anything to shut down — which is what makes its deletion take time.
    pub fn hosts_managed_instances(&self) -> bool {
        self.compute_type != COMPUTE_TYPE_ANYWHERE
    }

    /// Whether this fleet deploys to `location`.
    pub fn has_location(&self, location: &str) -> bool {
        self.locations
            .iter()
            .any(|candidate| candidate.location == location)
    }

    /// How many game sessions this fleet can host at once in `location`.
    ///
    /// Each active, available server process hosts one game session at a time, so the
    /// capacity of a fleet location is the number of such processes running there (see
    /// [`server_processes_per_location`]). A location this fleet does not deploy to, and
    /// one whose own life cycle has not reached `ACTIVE` — so its processes are not
    /// running yet — has none.
    ///
    /// The ports the fleet opens bound its live sessions too, but they are opened for the
    /// WHOLE fleet rather than per location, so that bound is [`open_port_count`] against
    /// the fleet's live sessions and is applied beside this one.
    ///
    /// [`open_port_count`]: Fleet::open_port_count
    pub fn game_session_capacity(&self, location: &str) -> usize {
        self.locations
            .iter()
            .find(|known| known.location == location)
            .filter(|known| known.status == FLEET_STATUS_ACTIVE)
            .map_or(0, |known| known.server_processes)
    }

    /// The inbound port ranges this fleet opens, lowest first and with overlaps merged,
    /// or [`DEFAULT_PORT_RANGE`] when it opens none.
    ///
    /// `CreateFleet` refuses a range outside [`DEFAULT_PORT_RANGE`] (see
    /// [`valid_port_range`]), so a stored range always runs forwards; a record that
    /// somehow held one that does not is dropped here rather than trusted.
    pub fn open_port_ranges(&self) -> Vec<(i32, i32)> {
        let mut ranges: Vec<(i32, i32)> = self
            .ec2_inbound_permissions
            .iter()
            .map(|permission| (permission.from_port, permission.to_port))
            .filter(|(from, to)| from <= to)
            .collect();
        if ranges.is_empty() {
            return vec![DEFAULT_PORT_RANGE];
        }
        ranges.sort_unstable();
        let mut merged: Vec<(i32, i32)> = Vec::with_capacity(ranges.len());
        for (from, to) in ranges {
            match merged.last_mut() {
                // `from - 1` cannot underflow: a stored port is positive.
                Some(last) if from <= last.1.saturating_add(1) => last.1 = last.1.max(to),
                _ => merged.push((from, to)),
            }
        }
        merged
    }

    /// How many distinct ports this fleet opens, i.e. how many game sessions can listen
    /// on it at once.
    pub fn open_port_count(&self) -> usize {
        self.open_port_ranges().iter().map(port_span).sum()
    }

    /// The next port a game session on this fleet listens on: the first port the fleet
    /// opens that no live session already holds, starting the walk where the last
    /// handout left off so consecutive sessions get consecutive ports.
    ///
    /// `None` only when every opened port is taken, which `CreateGameSession`'s port
    /// bound — the fleet's live sessions against its [`open_port_count`] — keeps
    /// unreachable for a session the fleet admitted.
    ///
    /// [`open_port_count`]: Fleet::open_port_count
    pub fn next_game_session_port(
        &mut self,
        taken: &HashSet<i32>,
    ) -> Option<i32> {
        let ranges = self.open_port_ranges();
        let total: usize = ranges.iter().map(port_span).sum();
        if total == 0 {
            return None;
        }
        let start = usize::try_from(self.port_cursor).unwrap_or(0) % total;
        let ports = || ranges.iter().flat_map(|&(from, to)| from..=to);
        // The two passes chained, skipped to `start` and cut at `total`, are the opened
        // ports in order starting from where the last handout left off.
        for (step, port) in ports().chain(ports()).skip(start).take(total).enumerate() {
            if !taken.contains(&port) {
                let advance = u32::try_from(step).unwrap_or(0).saturating_add(1);
                self.port_cursor = self.port_cursor.wrapping_add(advance);
                return Some(port);
            }
        }
        None
    }

    /// The DNS name of the instance hosting this fleet's game servers, in whichever of
    /// the two forms `GameSession$DnsName` documents applies: a TLS-enabled fleet
    /// reports `<unique identifier>.<region identifier>.amazongamelift.com`, a
    /// non-TLS-enabled one `ec2-<unique identifier>.compute.amazonaws.com`, where the
    /// unique identifier of an EC2 instance is its address.
    pub fn dns_name(&self) -> String {
        let tls_enabled = self
            .certificate_configuration
            .as_ref()
            .is_some_and(|config| config.certificate_type == CERTIFICATE_TYPE_GENERATED);
        if tls_enabled {
            format!("{}.{}.amazongamelift.com", self.fleet_id, self.region)
        } else {
            format!(
                "ec2-{}.compute.amazonaws.com",
                self.ip_address.replace('.', "-")
            )
        }
    }
}

/// How many ports an inbound range covers, saturating rather than overflowing on bounds
/// a stored record should never hold.
fn port_span(range: &(i32, i32)) -> usize {
    range
        .1
        .checked_sub(range.0)
        .and_then(|width| width.checked_add(1))
        .and_then(|width| usize::try_from(width).ok())
        .unwrap_or(1)
}

/// What the service answers when no fleet is stored under the `FleetIdOrArn` an operation
/// was asked to act on, in the template `CreateGameSession` answers an absent fleet with:
/// the identifier the CALLER supplied, verbatim, since either of the two forms the member
/// accepts selects the same fleet.
///
/// Each operation has its own template for this (`DeleteFleet` answers `Unable to find
/// fleet '<id>'.`), so this one is shared only by the operations that answer THIS
/// sentence, and is the one place that sentence is written.
pub fn fleet_not_found_message(id_or_arn: &str) -> String {
    format!("Cannot find fleet with ID {id_or_arn}")
}

/// What the service answers when the fleet holds no compute under the `ComputeNameOrArn`
/// an operation was asked to act on — the message of the modelled `NotFoundException`,
/// shared by every operation that names ONE compute, so the same missing compute reads the
/// same way whichever of them was called.
///
/// The service calls a compute of a fleet a HOST in this sentence, and ends it with a full
/// stop: that is the wording recorded for a compute an Anywhere fleet no longer holds, on
/// the `DescribeCompute` of fixture `gamelift-multi-compute-isolation` (step `task#11`) and
/// on the reads of the behavioural unit
/// `gamelift.anywhere-fleet-register-compute-lifecycle`. The one variable part is the
/// identifier the CALLER supplied, carried verbatim in parentheses, since either of the two
/// forms the member accepts
/// (`^([a-zA-Z0-9\-]+(\/[a-zA-Z0-9\-]+)?|arn:.*:compute\/...)$`) selects the same compute.
pub fn compute_not_found_message(name_or_arn: &str) -> String {
    format!("Host ({name_or_arn}) not found.")
}

/// The Amazon GameLift Servers SDK endpoint a game server process on a registered compute
/// connects to.
///
/// `Compute$GameLiftServiceSdkEndpoint` is "the Amazon GameLift Servers SDK endpoint
/// connection for a registered compute resource in an Anywhere fleet. The game servers on
/// the compute use this endpoint to connect to the Amazon GameLift Servers service", and
/// `RegisterCompute`'s own doc has each server process pass it to "the Amazon GameLift
/// Servers server SDK action `InitSDK()`" — a WebSocket connection to the service in the
/// Region the compute is registered in, which is the one variable part of the endpoint.
pub fn service_sdk_endpoint(region: &str) -> String {
    format!("wss://{region}.api.amazongamelift.com")
}

/// A fresh authentication token for the game server processes running on one compute, for
/// `GetComputeAuthToken`.
///
/// The token authenticates a game server process to the Amazon GameLift Servers service
/// through the server SDK rather than a request to this emulator, so it must never be
/// accepted as a SigV4 credential — but it is still a secret handed to a caller, and
/// nothing in a service may construct a secret itself. It is therefore minted by the one
/// component that generates secrets, the emulator's credential store, and removed from
/// that store again: the caller gets the secret, the emulator keeps no credential it
/// would accept, and nothing accumulates per call.
///
/// `ComputeAuthToken` is modelled `len 1..64, pattern ^[a-zA-Z0-9\-]+$` and the docs of
/// this service write the values it mints as UUIDs
/// (`compute-a1234567-b8c9-0d1e-2fa3-b45c6d7e8912`), so the token is the minted secret's
/// own randomness read off as hexadecimal characters and grouped the way [`as_uuid`]
/// writes one.
///
/// `None` when the store minted no secret material long enough to read a token off, which
/// leaves the operation to refuse rather than report a token it made up. Every secret of
/// the minted credential is read, so the 32 characters a token needs do not depend on one
/// of them alone.
pub fn mint_compute_auth_token(store: &::emulator_server::CredentialStore) -> Option<String> {
    let minted = store.create(&::emulator_server::aws_credentials::AWS_SESSION);
    let secrets: String = [
        ::emulator_server::aws_credentials::SESSION_TOKEN,
        ::emulator_server::aws_credentials::SECRET_ACCESS_KEY,
    ]
    .iter()
    .filter_map(|name| minted.secret(name))
    .collect();
    // A compute's auth token is not a credential of this emulator; see above.
    if let Err(error) = store.delete(&minted.id) {
        ::tracing::warn!(
            reason = ?error,
            "a compute authentication token could not be withdrawn from the credential store"
        );
    }
    let hex: String = secrets
        .chars()
        .filter(char::is_ascii_hexdigit)
        .map(|character| character.to_ascii_lowercase())
        .collect();
    as_uuid(&hex)
}

/// An authentication token issued to a compute, and the instant it stops being valid.
#[derive(Clone)]
pub struct ComputeAuthTokenRecord {
    /// The token value itself, as `GetComputeAuthToken` reported it
    /// ([`mint_compute_auth_token`]).
    pub token: String,
    /// When the token stops being valid, which `GetComputeAuthTokenOutput`'s
    /// `ExpirationTimestamp` reports ([`COMPUTE_AUTH_TOKEN_VALIDITY`] after it was
    /// issued).
    pub expires_at: SystemTime,
}

/// Prints when the token expires but NOT the token, so a formatted or logged value — of
/// this record, of the compute holding it, or of the whole store — leaks no secret
/// material; the shape the credential store's own minted-credential type takes for the
/// same reason.
impl ::std::fmt::Debug for ComputeAuthTokenRecord {
    fn fmt(&self, f: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
        f.debug_struct("ComputeAuthTokenRecord")
            .field("expires_at", &self.expires_at)
            .finish_non_exhaustive()
    }
}

impl ComputeAuthTokenRecord {
    /// Whether this token is still valid at `now`: "authentication tokens are valid for a
    /// limited time span", which ends at its expiration.
    pub fn is_valid_at(&self, now: SystemTime) -> bool {
        now < self.expires_at
    }
}

/// A compute resource registered with an Anywhere fleet, as `RegisterCompute` persisted
/// it.
///
/// Every member of the modelled `Compute` is either a setting the request supplied or one
/// the service decided, so the record holds them all and [`ComputeRecord::to_modelled`] is
/// the one projection every read of the compute is served from.
#[derive(Debug, Clone)]
pub struct ComputeRecord {
    /// The fleet the compute belongs to, and the ARN of that fleet.
    pub fleet_id: String,
    pub fleet_arn: String,
    pub compute_name: String,
    pub compute_arn: String,
    /// The address the request gave the compute. "Amazon GameLift Servers requires a DNS
    /// name or IP address for a compute", so at least one of the two is always held.
    pub ip_address: Option<String>,
    pub dns_name: Option<String>,
    pub status: String,
    /// The fleet location the compute resides in.
    pub location: String,
    pub creation_time: SystemTime,
    /// The endpoint this compute's game server processes connect to the service through
    /// (see [`service_sdk_endpoint`]).
    pub service_sdk_endpoint: String,
    /// The authentication token this compute's game server processes present to the
    /// service, while it is still valid: "all game servers that are running on [a] compute
    /// have access to a valid auth token", so a compute holds the ONE token
    /// `GetComputeAuthToken` last issued for it, and none until the first request for one.
    pub auth_token: Option<ComputeAuthTokenRecord>,
}

impl ComputeRecord {
    /// This compute as the modelled `Compute`.
    ///
    /// The members a registered Anywhere compute does not have are absent rather than
    /// invented: `Type` is the fleet's EC2 instance type and "for registered computes in an
    /// Amazon GameLift Servers Anywhere fleet, this property is empty"; `InstanceId` names
    /// "the EC2 instance that is hosting the compute" and an Anywhere compute is hardware
    /// the service does not own; `OperatingSystem` is read off a managed instance's build;
    /// `GameLiftAgentEndpoint` belongs to a fleet running the Amazon GameLift Servers
    /// Agent, which "handles all compute registry tasks for you" instead of this
    /// operation; and `ContainerAttributes` and `GameServerContainerGroupDefinitionArn`
    /// belong to a compute in a container fleet.
    pub fn to_modelled(&self) -> Compute {
        Compute {
            fleet_id: Some(self.fleet_id.clone()),
            fleet_arn: Some(self.fleet_arn.clone()),
            compute_name: Some(self.compute_name.clone()),
            compute_arn: Some(self.compute_arn.clone()),
            ip_address: self.ip_address.clone(),
            dns_name: self.dns_name.clone(),
            compute_status: Some(self.status.clone()),
            location: Some(self.location.clone()),
            creation_time: Some(to_date_time(self.creation_time)),
            operating_system: None,
            r#type: None,
            game_lift_service_sdk_endpoint: Some(self.service_sdk_endpoint.clone()),
            game_lift_agent_endpoint: None,
            instance_id: None,
            container_attributes: None,
            game_server_container_group_definition_arn: None,
        }
    }
}

/// A game session placed on a fleet, as `CreateGameSession` persisted it.
#[derive(Debug, Clone)]
pub struct GameSessionRecord {
    pub game_session_id: String,
    pub name: Option<String>,
    pub fleet_id: String,
    pub fleet_arn: String,
    pub status: String,
    /// When `ACTIVATING` becomes `ACTIVE`; `None` once the session is running.
    pub active_at: Option<SystemTime>,
    pub creation_time: SystemTime,
    pub termination_time: Option<SystemTime>,
    pub current_player_session_count: i32,
    pub maximum_player_session_count: i32,
    pub game_properties: Vec<GameProperty>,
    pub game_session_data: Option<String>,
    pub creator_id: Option<String>,
    pub location: String,
    pub player_session_creation_policy: String,
    /// Connection information, which a session carries only once it is `ACTIVE`.
    pub ip_address: Option<String>,
    pub port: Option<i32>,
    pub dns_name: Option<String>,
}

impl GameSessionRecord {
    /// This game session as the modelled `GameSession`.
    pub fn to_modelled(&self) -> GameSession {
        GameSession {
            game_session_id: Some(self.game_session_id.clone()),
            name: self.name.clone(),
            fleet_id: Some(self.fleet_id.clone()),
            fleet_arn: Some(self.fleet_arn.clone()),
            creation_time: Some(to_date_time(self.creation_time)),
            termination_time: self.termination_time.map(to_date_time),
            current_player_session_count: Some(self.current_player_session_count),
            maximum_player_session_count: Some(self.maximum_player_session_count),
            status: Some(self.status.clone()),
            status_reason: None,
            game_properties: Some(self.game_properties.clone()),
            ip_address: self.ip_address.clone(),
            dns_name: self.dns_name.clone(),
            port: self.port,
            player_session_creation_policy: Some(self.player_session_creation_policy.clone()),
            creator_id: self.creator_id.clone(),
            game_session_data: self.game_session_data.clone(),
            // FlexMatch fills this in, and matchmaking is a separate slice.
            matchmaker_data: None,
            location: Some(self.location.clone()),
            // The compute a session runs on is reported by the compute slice.
            compute_name: None,
            player_gateway_status: None,
        }
    }
}

/// A player slot reserved in a game session, as `CreatePlayerSession` persisted it.
#[derive(Debug, Clone)]
pub struct PlayerSessionRecord {
    pub player_session_id: String,
    pub player_id: String,
    pub game_session_id: String,
    pub fleet_id: String,
    pub fleet_arn: String,
    pub status: String,
    pub creation_time: SystemTime,
    pub termination_time: Option<SystemTime>,
    pub ip_address: Option<String>,
    pub port: Option<i32>,
    pub dns_name: Option<String>,
    pub player_data: Option<String>,
}

impl PlayerSessionRecord {
    /// This player session as the modelled `PlayerSession`.
    pub fn to_modelled(&self) -> PlayerSession {
        PlayerSession {
            player_session_id: Some(self.player_session_id.clone()),
            player_id: Some(self.player_id.clone()),
            game_session_id: Some(self.game_session_id.clone()),
            fleet_id: Some(self.fleet_id.clone()),
            fleet_arn: Some(self.fleet_arn.clone()),
            creation_time: Some(to_date_time(self.creation_time)),
            termination_time: self.termination_time.map(to_date_time),
            status: Some(self.status.clone()),
            ip_address: self.ip_address.clone(),
            dns_name: self.dns_name.clone(),
            port: self.port,
            player_data: self.player_data.clone(),
        }
    }
}

/// A game session placement queue, as `CreateGameSessionQueue` persisted it.
///
/// `GameSessionQueue` is "configuration for a game session placement mechanism": every
/// member of it is a setting the request supplied or the service decided, so the record
/// holds them all and [`GameSessionQueueRecord::to_modelled`] is the one projection every
/// read of the queue is served from.
#[derive(Debug, Clone)]
pub struct GameSessionQueueRecord {
    pub name: String,
    pub queue_arn: String,
    pub timeout_in_seconds: i32,
    pub player_latency_policies: Vec<PlayerLatencyPolicy>,
    pub destinations: Vec<GameSessionQueueDestination>,
    pub filter_configuration: Option<FilterConfiguration>,
    pub priority_configuration: Option<PriorityConfiguration>,
    pub custom_event_data: Option<String>,
    pub notification_target: Option<String>,
    /// The labels assigned to the queue resource. `GameSessionQueue` has no `Tags`
    /// member — tags are read back through `ListTagsForResource` — so they are held on
    /// the record and reported by no projection of it yet.
    pub tags: Vec<Tag>,
    pub creation_time: SystemTime,
    /// The Region of the request that created the queue, which its ARN names.
    pub region: String,
}

impl GameSessionQueueRecord {
    /// This queue as the modelled `GameSessionQueue`.
    ///
    /// The two list members are always reported — empty for a queue configured with
    /// none, since "a queue can't make placements without at least one destination" is a
    /// property of an empty list, not of an absent one — while the two optional
    /// configuration structures are reported only when the queue carries them.
    pub fn to_modelled(&self) -> GameSessionQueue {
        GameSessionQueue {
            name: Some(self.name.clone()),
            game_session_queue_arn: Some(self.queue_arn.clone()),
            timeout_in_seconds: Some(self.timeout_in_seconds),
            player_latency_policies: Some(self.player_latency_policies.clone()),
            destinations: Some(self.destinations.clone()),
            filter_configuration: self.filter_configuration.clone(),
            priority_configuration: self.priority_configuration.clone(),
            custom_event_data: self.custom_event_data.clone(),
            notification_target: self.notification_target.clone(),
        }
    }
}

/// What the service answers when no queue is stored under the
/// `GameSessionQueueNameOrArn` the request named — the message of the modelled
/// `NotFoundException`, as the queue's delete path and the placement path both word a
/// queue they cannot resolve.
///
/// The identifier the CALLER supplied is the sentence's one variable part: either of the
/// two forms `GameSessionQueueNameOrArn` accepts selects the same queue, so the refusal
/// states what was asked for rather than anything the service resolved it to.
///
/// A missing queue is NOT worded the same way by every operation that names one — see
/// [`game_session_queue_name_does_not_exist_message`] for the sentence the queue's own
/// update path answers with — so each path uses the text of its own refusal rather than
/// one service-wide sentence.
pub fn game_session_queue_not_found_message(name_or_arn: &str) -> String {
    format!("Game session queue {name_or_arn} does not exist.")
}

/// What the service answers a request to update a queue whose `Name` resolves to no
/// stored queue, whatever made it unresolvable: a name never created, one whose queue was
/// deleted, or an ARN naming another account or Region.
///
/// Fixed text `"Queue name "` and `" does not exist"`, with no trailing period; the one
/// variable part is the `GameSessionQueueNameOrArn` the request supplied, copied verbatim.
/// Paired with the modelled `InvalidRequestException`, which is the class this refusal
/// carries — the status alone does not distinguish it, since the model defaults both that
/// error and `NotFoundException` to HTTP 400.
pub fn game_session_queue_name_does_not_exist_message(name_or_arn: &str) -> String {
    format!("Queue name {name_or_arn} does not exist")
}

/// The refusal a request that configures a game session queue earns, or `None` when every
/// setting it supplies is one the service accepts.
///
/// `CreateGameSessionQueue` and `UpdateGameSessionQueue` configure the SAME resource from
/// the same members, so the rules over those members — and the sentence each refusal
/// answers with — live here rather than once per operation. Each argument is the value the
/// REQUEST supplied, so a member the request left out is `None` and is not checked: it
/// keeps whatever the service already decided for it.
pub fn game_session_queue_setting_refusal(
    destinations: Option<&[GameSessionQueueDestination]>,
    player_latency_policies: Option<&[PlayerLatencyPolicy]>,
    timeout_in_seconds: Option<i32>,
    priority_configuration: Option<&PriorityConfiguration>,
) -> Option<String> {
    // The two list members the model leaves unbounded are what one request could
    // otherwise make a stored queue occupy — the record holds them for as long as the
    // queue exists — so they are refused before anything is stored. (`Name`,
    // `CustomEventData` and `NotificationTarget` carry modelled lengths, and the
    // `PriorityConfiguration` and `FilterConfiguration` lists are bounded by the model at
    // 4 and 100 entries, so the boundary walk has already refused an oversized one of
    // those.)
    let destination_count = destinations.map_or(0, <[GameSessionQueueDestination]>::len);
    if destination_count > MAX_QUEUE_DESTINATIONS {
        return Some(format!(
            "Destinations lists {destination_count} destinations; a queue may list at most \
             {MAX_QUEUE_DESTINATIONS}."
        ));
    }
    let policy_count = player_latency_policies.map_or(0, <[PlayerLatencyPolicy]>::len);
    if policy_count > MAX_QUEUE_PLAYER_LATENCY_POLICIES {
        return Some(format!(
            "PlayerLatencyPolicies lists {policy_count} policies; a queue may list at most \
             {MAX_QUEUE_PLAYER_LATENCY_POLICIES}."
        ));
    }

    // "The maximum time, in seconds, that a new game session placement request remains in
    // the queue [...] The minimum value is 10 and the maximum value is 600." The modelled
    // `WholeNumber` range bounds the member only below, so the documented bounds are what
    // this refusal enforces — and the message is the service's fixed sentence, naming the
    // member and those two documented bounds and reading the same whatever value was
    // rejected.
    let (min_timeout, max_timeout) = QUEUE_TIMEOUT_RANGE_SECONDS;
    if let Some(requested) = timeout_in_seconds {
        if !(min_timeout..=max_timeout).contains(&requested) {
            return Some(format!(
                "TimeoutInSeconds must be between {min_timeout} and {max_timeout} seconds"
            ));
        }
    }

    // `PriorityConfiguration$PriorityOrder`: "Each priority type is listed once."
    // `$LocationOrder`: "Each location must be listed only once." The modelled list
    // lengths and the `PriorityType` enum are refused by the boundary walk before a
    // handler runs; the uniqueness of each sequence is the service's own rule.
    if let Some(priority) = priority_configuration {
        if let Some(duplicate) = first_duplicate(priority.priority_order.as_deref()) {
            return Some(format!(
                "PriorityConfiguration.PriorityOrder lists each priority type once; {duplicate} \
                 is listed more than once."
            ));
        }
        if let Some(duplicate) = first_duplicate(priority.location_order.as_deref()) {
            return Some(format!(
                "PriorityConfiguration.LocationOrder lists each location once; {duplicate} is \
                 listed more than once."
            ));
        }
    }
    None
}

/// The first entry a sequence lists twice, if any: what a "listed only once" refusal
/// names. An absent sequence lists nothing twice.
fn first_duplicate(entries: Option<&[String]>) -> Option<String> {
    let mut seen = HashSet::new();
    entries?
        .iter()
        .find(|entry| !seen.insert((*entry).clone()))
        .cloned()
}

/// What the service answers when no alias is stored under the `AliasIdOrArn` an operation
/// was asked to act on — the message of the modelled `NotFoundException`, shared by every
/// operation that names ONE alias, so the same missing alias reads the same way whichever
/// of them was called.
///
/// The resource kind leads the sentence and the identifier the CALLER supplied is its one
/// variable part, carried verbatim in parentheses: either of the two forms the member
/// accepts selects the same alias, so the refusal states what was asked for rather than
/// anything the service resolved it to. [`script_not_found_message`] answers a missing
/// script in the same shape, closing with a full stop where this one does not: each of
/// the two reproduces the sentence the service answers for that resource kind.
pub fn alias_not_found_message(id_or_arn: &str) -> String {
    format!("Alias ({id_or_arn}) not found")
}

/// An alias for a fleet, as `CreateAlias` persisted it.
///
/// Every member of the modelled `Alias` is either a setting the request supplied or one
/// the service decided, so the record holds them all and [`AliasRecord::to_modelled`] is
/// the one projection every read of the alias is served from.
#[derive(Debug, Clone)]
pub struct AliasRecord {
    pub alias_id: String,
    pub alias_arn: String,
    pub name: String,
    pub description: Option<String>,
    /// The routing configuration as the request configured it: what every read of the
    /// alias reports, and what [`AliasRecord::routing`] routes a request through.
    pub routing_strategy: RoutingStrategy,
    /// The labels assigned to the alias resource. `Alias` has no `Tags` member — tags are
    /// read back through `ListTagsForResource` — so they are held on the record and
    /// reported by no projection of it yet.
    pub tags: Vec<Tag>,
    pub creation_time: SystemTime,
    /// When the alias was last modified, which `UpdateAlias` moves forward.
    pub last_updated_time: SystemTime,
    /// The Region of the request that created the alias, which its ARN names.
    pub region: String,
}

/// Where a request that names an alias is routed, which is what the alias's
/// `RoutingStrategy$Type` decides.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AliasRouting<'a> {
    /// `SIMPLE` — "The alias resolves to one specific fleet": the `FleetId` it names.
    Fleet(&'a str),
    /// `TERMINAL` — "The alias does not resolve to a fleet but instead can be used to
    /// display a message to the user. A terminal alias throws a
    /// TerminalRoutingStrategyException with the message embedded": that message.
    Terminal(Option<&'a str>),
}

impl AliasRecord {
    /// This alias as the modelled `Alias`.
    pub fn to_modelled(&self) -> Alias {
        Alias {
            alias_id: Some(self.alias_id.clone()),
            alias_arn: Some(self.alias_arn.clone()),
            name: Some(self.name.clone()),
            description: self.description.clone(),
            routing_strategy: Some(self.routing_strategy.clone()),
            creation_time: Some(to_date_time(self.creation_time)),
            last_updated_time: Some(to_date_time(self.last_updated_time)),
        }
    }

    /// Where this alias routes a request that names it.
    ///
    /// Only the two forms [`validate_routing_strategy`] accepts are ever stored, so a
    /// strategy that is not a `SIMPLE` one naming a fleet is the `TERMINAL` one, which
    /// resolves to no fleet at all.
    pub fn routing(&self) -> AliasRouting<'_> {
        match (
            self.routing_strategy.r#type.as_deref(),
            self.routing_strategy.fleet_id.as_deref(),
        ) {
            (Some(ROUTING_STRATEGY_TYPE_SIMPLE), Some(fleet_id)) => AliasRouting::Fleet(fleet_id),
            _ => AliasRouting::Terminal(self.routing_strategy.message.as_deref()),
        }
    }
}

/// Whether `strategy` is one of the two routings an alias can carry, or the reason it is
/// not — which a handler answers with the modelled `InvalidRequestException`.
///
/// `RoutingStrategy$Type`'s doc names both forms and what each one needs: "SIMPLE - The
/// alias resolves to one specific fleet", so a simple strategy names the fleet it points
/// at; "TERMINAL - The alias does not resolve to a fleet but instead can be used to
/// display a message to the user. A terminal alias throws a
/// TerminalRoutingStrategyException with the message embedded", so a terminal strategy
/// carries that message. The modelled `RoutingStrategyType` enum is refused by the
/// boundary walk before a handler runs, so an unknown type cannot reach here; which
/// member each type requires is the service's own rule, enforced here.
///
/// Whether the named fleet EXISTS is deliberately not checked: an alias outlives the
/// fleets it points at ("You can reassign an alias to another fleet by calling
/// UpdateAlias"), so resolution is what discovers that a target has gone.
pub fn validate_routing_strategy(strategy: &RoutingStrategy) -> Result<(), String> {
    match strategy.r#type.as_deref() {
        Some(ROUTING_STRATEGY_TYPE_SIMPLE) => {
            if strategy.fleet_id.is_none() {
                return Err(routing_strategy_member_required(
                    ROUTING_STRATEGY_TYPE_SIMPLE,
                    "FleetId",
                ));
            }
            Ok(())
        }
        Some(ROUTING_STRATEGY_TYPE_TERMINAL) => {
            if strategy.message.is_none() {
                return Err(routing_strategy_member_required(
                    ROUTING_STRATEGY_TYPE_TERMINAL,
                    "Message",
                ));
            }
            Ok(())
        }
        _ => Err(format!(
            "RoutingStrategy.Type is required, and must be {ROUTING_STRATEGY_TYPE_SIMPLE} or \
             {ROUTING_STRATEGY_TYPE_TERMINAL}."
        )),
    }
}

/// What the service answers a routing strategy that does not carry the member its own type
/// requires: the type, the member and the one sentence, with nothing of the request
/// interpolated — the text is the same for every request that misses that member, which is
/// what lets a client match on it.
fn routing_strategy_member_required(strategy_type: &str, member: &str) -> String {
    format!("{strategy_type} RoutingStrategy must contain a valid {member}.")
}

/// What the service answers when no custom location is stored under the
/// `CustomLocationNameOrArnModel` an operation was asked to act on — the message of the
/// modelled `NotFoundException`, shared by every operation that names ONE custom location,
/// so the same missing location reads the same way whichever of them was called.
///
/// The same template every other GameLift resource this store holds is reported missing
/// with ([`alias_not_found_message`], [`build_not_found_message`],
/// [`script_not_found_message`]): the resource kind leads the sentence and the identifier
/// the CALLER supplied is its one variable part, carried verbatim in parentheses, since
/// either of the two forms the member accepts (`^(custom-[A-Za-z0-9\-]+|arn:.*:location\/
/// custom-\S+)$`) selects the same location.
pub fn custom_location_not_found_message(name_or_arn: &str) -> String {
    format!("Location ({name_or_arn}) not found")
}

/// A custom location, as `CreateLocation` persisted it.
///
/// `LocationModel` is "properties of a location, which can include its name, ARN (for
/// custom locations), and ping beacon information", so the record holds the name the
/// request chose and the ARN the service minted for it, and
/// [`CustomLocationRecord::to_modelled`] is the one projection every read of the location
/// is served from.
#[derive(Debug, Clone)]
pub struct CustomLocationRecord {
    pub location_name: String,
    pub location_arn: String,
    /// The labels assigned to the location resource. `LocationModel` has no `Tags`
    /// member — tags are read back through `ListTagsForResource` — so they are held on
    /// the record and reported by no projection of it yet.
    pub tags: Vec<Tag>,
    pub creation_time: SystemTime,
    /// The Region of the request that created the location, which its ARN names.
    pub region: String,
}

impl CustomLocationRecord {
    /// This custom location as the modelled `LocationModel`.
    ///
    /// A `PingBeacon` is a fixed endpoint of "an Amazon GameLift Servers hosting
    /// location" — published for the locations the service itself operates
    /// ([`managed_locations`]). A custom location is the caller's own hardware, which the
    /// service operates no beacon for, so the member is absent rather than carrying an
    /// address nothing answers on.
    pub fn to_modelled(&self) -> LocationModel {
        LocationModel {
            location_name: Some(self.location_name.clone()),
            location_arn: Some(self.location_arn.clone()),
            ping_beacon: None,
        }
    }
}

/// What the service answers when no Realtime script is stored under the `ScriptIdOrArn`
/// an operation was asked to act on — the message of the modelled `NotFoundException`,
/// shared by every operation that names ONE script, so the same missing script reads the
/// same way whichever of them was called.
///
/// The resource kind leads the sentence and the identifier the CALLER supplied is its one
/// variable part, carried verbatim in parentheses: either of the two forms the member
/// accepts selects the same script, so the refusal states what was asked for rather than
/// anything the service resolved it to.
pub fn script_not_found_message(id_or_arn: &str) -> String {
    format!("Script ({id_or_arn}) not found.")
}

/// A Realtime script, as `CreateScript` persisted it — the one projection every read of
/// the script is served from.
#[derive(Debug, Clone)]
pub struct ScriptRecord {
    pub script_id: String,
    pub script_arn: String,
    /// "A descriptive label that is associated with a script. Script names do not need to
    /// be unique", and the member is optional, so a script may carry no name at all.
    pub name: Option<String>,
    pub version: Option<String>,
    /// `Script$SizeOnDisk`: "The file size of the uploaded Realtime script, expressed in
    /// bytes. When files are uploaded from an S3 location, this value remains at `0`."
    pub size_on_disk: i64,
    pub creation_time: SystemTime,
    /// Where the script's zip file is stored: the location the request named for an S3
    /// upload, or the service-owned one ([`service_owned_script_location`]) for a local
    /// zip file.
    pub storage_location: S3Location,
    pub node_js_version: String,
    /// The labels assigned to the script resource, read back through
    /// `ListTagsForResource`.
    pub tags: Vec<Tag>,
    /// The Region of the request that created the script, which its ARN names.
    pub region: String,
}

impl ScriptRecord {
    /// This script as the modelled `Script`.
    pub fn to_modelled(&self) -> Script {
        Script {
            script_id: Some(self.script_id.clone()),
            script_arn: Some(self.script_arn.clone()),
            name: self.name.clone(),
            version: self.version.clone(),
            size_on_disk: Some(self.size_on_disk),
            creation_time: Some(to_date_time(self.creation_time)),
            storage_location: Some(self.storage_location.clone()),
            node_js_version: Some(self.node_js_version.clone()),
        }
    }
}

/// The `NodeJsVersion` values a Realtime script may declare: that member's own doc — "The
/// valid values are `10.x | 24.x`". The modelled pattern (`^[0-9]+\.(?:x|[0-9]+)$`)
/// accepts any version string at all, so the documented catalog is enforced where the
/// request is served.
pub const NODE_JS_VERSIONS: [&str; 2] = ["10.x", "24.x"];

/// The `NodeJsVersion` a script created without one carries: same doc — "By default,
/// `NodeJsVersion` is `10.x`."
pub const DEFAULT_NODE_JS_VERSION: &str = NODE_JS_VERSIONS[0];

/// The Amazon GameLift Servers-owned S3 location a script uploaded as a local zip file is
/// held at.
///
/// CreateScript's doc: "If the script file is provided as a local file, the file is
/// uploaded to an Amazon GameLift Servers-owned S3 bucket and the script record's storage
/// location reflects this location." The bucket belongs to the service rather than to the
/// caller, so the location carries no `RoleArn` — that member "allows Amazon GameLift
/// Servers to access the S3 bucket" of ANOTHER account, and none is involved here — and no
/// `ObjectVersion`, which is the caller's own bucket versioning.
///
/// The service names one script bucket per Region ([`SCRIPT_UPLOAD_FAMILY`]) and holds each
/// account's scripts under their own prefix, so the key is the account that owns the script
/// and the script that holds the files: two scripts never share a location, and nothing the
/// caller supplies names either part.
pub fn service_owned_script_location(region: &str, script_id: &str) -> S3Location {
    service_owned_location(SCRIPT_UPLOAD_FAMILY, region, script_id)
}

/// The resource family whose uploads a service-owned bucket holds: the custom game builds
/// `CreateBuild` hands an upload location to.
pub const BUILD_UPLOAD_FAMILY: &str = "builds";

/// The resource family whose uploads a service-owned bucket holds: the Realtime scripts
/// `CreateScript` and `UpdateScript` store a local zip file for.
pub const SCRIPT_UPLOAD_FAMILY: &str = "scripts";

/// The Amazon GameLift Servers-owned bucket that holds one resource family's uploads for one
/// Region: the service runs its own S3 bucket per Region per family, named
/// `prod-gamescale-<family>-<region>`.
///
/// Nothing a caller supplies is part of the name: the family is fixed by the operation and
/// the Region is the one the request was served in ([`request_region`]), so the names this
/// can produce are the Regions requests name, not values a request carries.
pub fn service_owned_bucket(family: &str, region: &str) -> String {
    format!("prod-gamescale-{family}-{region}")
}

/// The Amazon GameLift Servers-owned S3 location one resource's uploaded files are held at:
/// the family's bucket for the Region ([`service_owned_bucket`]), holding each account's
/// resources under their own prefix — so the key is the account that owns the resource and
/// the resource that holds the files, and two resources never share a location.
///
/// The ONE derivation every service-owned location comes from, so a build's and a script's
/// locations can only follow the same rule, and whatever reports one reports what was
/// stored.
///
/// The bucket belongs to the service rather than to the caller, so the location carries no
/// `RoleArn` — that member "allows Amazon GameLift Servers to access the S3 bucket" of
/// ANOTHER account, and none is involved here — and no `ObjectVersion`, which is the
/// caller's own bucket versioning.
pub fn service_owned_location(family: &str, region: &str, resource_id: &str) -> S3Location {
    S3Location {
        bucket: Some(service_owned_bucket(family, region)),
        key: Some(format!(
            "{}/{resource_id}",
            ::engine::emulator_account_id()
        )),
        role_arn: None,
        object_version: None,
    }
}

/// What the service answers when no custom game build is stored under the `BuildIdOrArn`
/// an operation was asked to act on — the message of the modelled `NotFoundException`,
/// shared by every operation that names ONE build, so the same missing build reads the
/// same way whichever of them was called.
///
/// The resource kind leads the sentence and the identifier the CALLER supplied is its one
/// variable part, carried verbatim in parentheses: either of the two forms `BuildIdOrArn`
/// accepts (`^(build-\S+|arn:.*:build\/build-\S+)$`) selects the same build, so the
/// refusal states what was asked for rather than anything the service resolved it to.
pub fn build_not_found_message(id_or_arn: &str) -> String {
    format!("Build ({id_or_arn}) not found")
}

/// A custom game build, as `CreateBuild` persisted it — the one projection every read of
/// the build is served from.
#[derive(Debug, Clone)]
pub struct BuildRecord {
    pub build_id: String,
    pub build_arn: String,
    /// "A descriptive label that is associated with a build. Build names do not need to be
    /// unique", and the member is optional, so a build may carry no name at all.
    pub name: Option<String>,
    pub version: Option<String>,
    /// One of [`BUILD_STATUS_INITIALIZED`] and [`BUILD_STATUS_READY`].
    pub status: String,
    /// "The operating system that your game server binaries run on", which "you can't
    /// change ... later", so the create decides it once.
    pub operating_system: String,
    pub server_sdk_version: String,
    pub creation_time: SystemTime,
    /// Where the build's files are stored: the location the request named, or the
    /// service-owned one ([`service_owned_build_location`]) the caller uploads to.
    pub storage_location: S3Location,
    /// When the status change the clock owes this build comes due, i.e. when the service
    /// has finished reading the build files it found at the location the request named.
    /// `None` for a build whose files are not there: that build has no change owed to it,
    /// because `READY` is "the game build has been successfully uploaded" and the upload is
    /// the caller's own next step, not something the clock does.
    pub ready_at: Option<SystemTime>,
    /// The credential-store id of the upload credential this build was handed, so the
    /// credential can be rotated or withdrawn with the build it belongs to. `None` for a
    /// build whose files are in the caller's own bucket, which needs no upload credential.
    pub upload_credential_id: Option<String>,
    /// The labels the create assigned to the build resource, kept with it for the
    /// tag operations to read and write once they are implemented.
    pub tags: Vec<Tag>,
    /// The Region of the request that created the build, which its ARN names.
    pub region: String,
}

impl BuildRecord {
    /// This build as the modelled `Build`.
    ///
    /// `SizeOnDisk` is "file size of the uploaded game build, expressed in bytes[.] When
    /// the build status is `INITIALIZED` or when using a custom Amazon S3 storage location,
    /// this value is 0" — and CreateBuild's own `StorageLocation` doc: "Amazon GameLift
    /// Servers will report a `SizeOnDisk` of 0." Those are the only two ways a build here
    /// comes about, so every build reports 0 rather than a size nothing measured.
    pub fn to_modelled(&self) -> Build {
        Build {
            build_id: Some(self.build_id.clone()),
            build_arn: Some(self.build_arn.clone()),
            name: self.name.clone(),
            version: self.version.clone(),
            status: Some(self.status.clone()),
            size_on_disk: Some(0),
            operating_system: Some(self.operating_system.clone()),
            creation_time: Some(to_date_time(self.creation_time)),
            server_sdk_version: Some(self.server_sdk_version.clone()),
        }
    }
}

/// The `ServerSdkVersion` a build created without one carries: that member's own doc — "By
/// default Amazon GameLift Servers sets this value to `4.0.2`."
pub const DEFAULT_SERVER_SDK_VERSION: &str = "4.0.2";

/// The Amazon GameLift Servers-owned S3 location a build's files are uploaded to when the
/// request named no storage location of its own.
///
/// CreateBuild's doc: "Upload your build files to a Amazon GameLift Servers Amazon S3
/// location. To use this option, specify a build name and operating system. This operation
/// creates a new build resource and also returns an Amazon S3 location with temporary
/// access credentials." The one derivation every service-owned location comes from
/// ([`service_owned_location`]), for the builds family: the service's own build bucket for
/// the Region the create was served in, under the account that owns the build and the build
/// that holds the files.
pub fn service_owned_build_location(region: &str, build_id: &str) -> S3Location {
    service_owned_location(BUILD_UPLOAD_FAMILY, region, build_id)
}

/// The Amazon GameLift Servers-owned bucket every build uploaded through
/// [`service_owned_build_location`] in one Region is held in, with a key per build.
pub fn service_owned_build_bucket(region: &str) -> String {
    service_owned_bucket(BUILD_UPLOAD_FAMILY, region)
}

/// What the Amazon GameLift Servers-owned upload bucket is recorded as provisioned by in
/// the one S3 object store (`Bucket::provisioned_by`), which keeps it out of the account's
/// own `ListBuckets`: the bucket belongs to this service, not to the caller.
pub const BUILD_UPLOAD_BUCKET_PROVISIONER: &str = "gamelift";

/// Stand up the object store behind the service-owned location a build's files are uploaded
/// to, so the location and credentials `CreateBuild` hands back reach something: "this
/// operation creates a new build resource and also returns an Amazon S3 location with
/// temporary access credentials. Use the credentials to manually upload your build files to
/// the specified Amazon S3 location."
///
/// The S3 object protocol in this emulator is served by the Amazon S3 service, so the
/// bucket is provisioned THERE rather than copied into an object map of this service's own:
/// the caller's `PutObject` and everything this service later reads about the upload are
/// then the one store.
///
/// IDEMPOTENT and BOUNDED: the name is the family's bucket for ONE Region
/// ([`service_owned_build_bucket`]), so a call either records that Region's bucket or finds
/// it already recorded — no value a request CARRIES keys an entry, and the count of buckets
/// this service stands up is capped at [`MAX_BUILD_UPLOAD_BUCKETS`], past which a create
/// served in a further Region is refused rather than recording another. A bucket of that
/// name the store holds for anybody else is NOT adopted: the upload would then be vended
/// into a bucket that is not this service's, so the call answers `false` and the create is
/// refused instead.
///
/// `false` when the store holds no such bucket of this service's and could not record one
/// (a name in use, its own bucket quota, or this service's cap), `None` when that store
/// cannot be reached at all — a router that mounts this service without Amazon S3.
///
/// LOCK ORDER: this takes the S3 store's lock and holds no other while it does, and hands
/// back an owned value rather than a guard — so it must be called with NO guard of this
/// service's store held. Keeping the cross-service access one-directional and guard-free is
/// what makes a lock cycle between the two stores unreachable.
pub fn provision_build_upload_bucket<Eng: Send + Sync + 'static>(
    engine: &Eng,
    region: &str,
) -> Option<bool> {
    use ::service_com_amazonaws_s3_amazons3::state::BucketCreation;

    let bucket = service_owned_build_bucket(region);
    let state = ::service_com_amazonaws_s3_amazons3::state::service_state(engine)?;
    let mut store = state.lock();
    // The Region is read from the request, so the cap is what keeps the Regions a sequence
    // of requests names from growing the object store without limit. A bucket this service
    // already stands up is not a further one, so it is served however many there are.
    if store.bucket(&bucket).is_none() && service_owned_buckets(&store) >= MAX_BUILD_UPLOAD_BUCKETS
    {
        return Some(false);
    }
    let recorded = match store.provision_bucket(&bucket, region, BUILD_UPLOAD_BUCKET_PROVISIONER) {
        BucketCreation::Created => true,
        // The name is in use, which for this derived name is ordinarily the bucket an
        // earlier create recorded — but `provision_bucket` reports a name taken by ANY
        // bucket, so the one that holds it is what decides.
        BucketCreation::NameTaken => store
            .bucket(&bucket)
            .is_some_and(|held| held.provisioned_by() == Some(BUILD_UPLOAD_BUCKET_PROVISIONER)),
        BucketCreation::QuotaReached => false,
    };
    Some(recorded)
}

/// How many buckets this service has stood up in the one object store, the count
/// [`MAX_BUILD_UPLOAD_BUCKETS`] bounds.
fn service_owned_buckets(
    store: &::service_com_amazonaws_s3_amazons3::state::ComAmazonawsS3Amazons3State,
) -> usize {
    store
        .buckets()
        .filter(|(_, bucket)| bucket.provisioned_by() == Some(BUILD_UPLOAD_BUCKET_PROVISIONER))
        .count()
}

/// Whether the build files a storage location names are in the one object store: the
/// service can only read a build's files where they are.
///
/// `Build$Status`: "`READY` -- the game build has been successfully uploaded", so a location
/// holding nothing is a build whose files nobody has uploaded. The CURRENT object at the
/// location's key, so a key whose object has been deleted again reads as absent, exactly as
/// a `GetObject` of it would.
///
/// The same LOCK ORDER as [`provision_build_upload_bucket`], and a pure read of that store.
pub fn build_files_present<Eng: Send + Sync + 'static>(engine: &Eng, location: &S3Location) -> bool {
    let (Some(bucket), Some(key)) = (location.bucket.as_deref(), location.key.as_deref()) else {
        return false;
    };
    let Some(state) = ::service_com_amazonaws_s3_amazons3::state::service_state(engine) else {
        return false;
    };
    let store = state.lock();
    store
        .bucket(bucket)
        .is_some_and(|bucket| bucket.object(key).is_some())
}

/// Delete the build files held at a SERVICE-OWNED storage location, the way a
/// `DeleteObject` of that key would.
///
/// `DeleteBuild` "permanently deletes the build resource and any uploaded build files", and
/// the files this service owns are the ones at the location it handed the caller to upload
/// to ([`service_owned_build_location`]) — a location in the CALLER's own bucket holds the
/// caller's objects, which this service was only given read access to, so a location that
/// is not this service's own is left untouched.
///
/// The same LOCK ORDER as [`provision_build_upload_bucket`]: it takes the object store's
/// lock and holds no other while it does, so it must be called with NO guard of this
/// service's store held.
pub fn remove_build_files<Eng: Send + Sync + 'static>(
    engine: &Eng,
    region: &str,
    location: &S3Location,
) {
    let (Some(bucket), Some(key)) = (location.bucket.as_deref(), location.key.as_deref()) else {
        return;
    };
    if bucket != service_owned_build_bucket(region) {
        return;
    }
    let Some(state) = ::service_com_amazonaws_s3_amazons3::state::service_state(engine) else {
        return;
    };
    let mut store = state.lock();
    store.remove_object(bucket, key);
}

/// What `DeleteMatchmakingRuleSet` answers when no rule set is stored under the
/// `MatchmakingRuleSetName` it was asked to delete — the message of the modelled
/// `NotFoundException` ("the requested resource was not found. The resource was either not
/// created yet or deleted").
///
/// Fixed text `"Matchmaking rule set "` and `" not found"`, with no trailing period: the
/// service words the same missing rule set differently per operation — the lookup this
/// delete performs reads as the resource kind followed by the identifier and the verdict,
/// while `DescribeMatchmakingRuleSets` reads as
/// [`matchmaking_rule_set_lookup_failure_message`] and the matchmaking-configuration
/// operations as `RuleSet with name <name> does not exist` — so each caller words its own
/// refusal and none of them is a blanket text.
///
/// The identifier the CALLER supplied is the sentence's one variable part: either of the
/// two forms the member accepts (`^([a-zA-Z0-9-\.]*|arn:.*:matchmakingruleset\/[a-zA-Z0-9-
/// \.]*)$`) selects the same rule set, so the refusal states what was asked for rather
/// than anything the service resolved it to.
pub fn matchmaking_rule_set_not_found_message(name_or_arn: &str) -> String {
    format!("Matchmaking rule set {name_or_arn} not found")
}

/// What `DescribeMatchmakingRuleSets` answers when one of the `Names` it was asked to
/// retrieve details for is stored under no rule set — the message of the modelled
/// `InvalidRequestException` it declares for a name that resolves to nothing.
///
/// Fixed text `"Failed to find rule set: "` and the identifier the CALLER supplied, which
/// is the sentence's one variable part, for the same reason as in
/// [`matchmaking_rule_set_not_found_message`].
pub fn matchmaking_rule_set_lookup_failure_message(name_or_arn: &str) -> String {
    format!("Failed to find rule set: {name_or_arn}")
}

/// What the service answers when no matchmaking configuration is stored under the
/// `MatchmakingConfigurationName` an operation asked to act ON — the message of the
/// modelled `NotFoundException`, shared by the operations that mutate ONE named
/// configuration, so the same missing matchmaker reads the same way whichever of them was
/// called.
///
/// Fixed text `"Configuration with name "` and `" does not exist"`, with no trailing
/// period — the same sentence shape the matchmaking family words a missing rule set with
/// (`RuleSet with name <name> does not exist`), for the resource this one names.
///
/// The identifier the CALLER supplied is the sentence's one variable part, for the same
/// reason as in [`matchmaking_rule_set_lookup_failure_message`].
pub fn matchmaking_configuration_not_found_message(name_or_arn: &str) -> String {
    format!("Configuration with name {name_or_arn} does not exist")
}

/// A FlexMatch rule set, as `CreateMatchmakingRuleSet` persisted it.
#[derive(Debug, Clone)]
pub struct MatchmakingRuleSetRecord {
    pub rule_set_name: String,
    pub rule_set_arn: String,
    /// The body exactly as the request supplied it: "a collection of matchmaking rules,
    /// formatted as a JSON string", reported back verbatim.
    pub rule_set_body: String,
    /// The labels assigned to the rule set resource, read back through
    /// `ListTagsForResource`.
    pub tags: Vec<Tag>,
    pub creation_time: SystemTime,
    /// The Region of the request that created the rule set, which its ARN names.
    pub region: String,
}

impl MatchmakingRuleSetRecord {
    /// This rule set as the modelled `MatchmakingRuleSet`.
    pub fn to_modelled(&self) -> MatchmakingRuleSet {
        MatchmakingRuleSet {
            rule_set_name: Some(self.rule_set_name.clone()),
            rule_set_arn: Some(self.rule_set_arn.clone()),
            rule_set_body: self.rule_set_body.clone(),
            creation_time: Some(to_date_time(self.creation_time)),
        }
    }
}

/// A matchmaker, as `CreateMatchmakingConfiguration` persisted it.
///
/// Every member of the modelled `MatchmakingConfiguration` is either a setting the
/// request supplied or one the service decided, so the record holds them all and
/// [`MatchmakingConfigurationRecord::to_modelled`] is the one projection every read of
/// the configuration is served from.
#[derive(Debug, Clone)]
pub struct MatchmakingConfigurationRecord {
    pub name: String,
    pub configuration_arn: String,
    pub description: Option<String>,
    pub game_session_queue_arns: Vec<String>,
    pub request_timeout_seconds: i32,
    pub acceptance_timeout_seconds: Option<i32>,
    pub acceptance_required: bool,
    /// The `RuleSetName` of the rule set this configuration matches with, resolved from
    /// whichever of the name and the ARN the request supplied.
    pub rule_set_name: String,
    pub rule_set_arn: String,
    pub notification_target: Option<String>,
    pub additional_player_count: Option<i32>,
    pub custom_event_data: Option<String>,
    pub game_properties: Vec<GameProperty>,
    pub game_session_data: Option<String>,
    pub backfill_mode: Option<String>,
    pub flex_match_mode: Option<String>,
    /// The labels assigned to the configuration resource, read back through
    /// `ListTagsForResource`.
    pub tags: Vec<Tag>,
    pub creation_time: SystemTime,
    /// The Region of the request that created the configuration, which its ARN names.
    pub region: String,
}

impl MatchmakingConfigurationRecord {
    /// This matchmaker as the modelled `MatchmakingConfiguration`.
    pub fn to_modelled(&self) -> MatchmakingConfiguration {
        MatchmakingConfiguration {
            name: Some(self.name.clone()),
            configuration_arn: Some(self.configuration_arn.clone()),
            description: self.description.clone(),
            game_session_queue_arns: Some(self.game_session_queue_arns.clone()),
            request_timeout_seconds: Some(self.request_timeout_seconds),
            acceptance_timeout_seconds: self.acceptance_timeout_seconds,
            acceptance_required: Some(self.acceptance_required),
            rule_set_name: Some(self.rule_set_name.clone()),
            rule_set_arn: Some(self.rule_set_arn.clone()),
            notification_target: self.notification_target.clone(),
            additional_player_count: self.additional_player_count,
            custom_event_data: self.custom_event_data.clone(),
            creation_time: Some(to_date_time(self.creation_time)),
            game_properties: Some(self.game_properties.clone()),
            game_session_data: self.game_session_data.clone(),
            backfill_mode: self.backfill_mode.clone(),
            flex_match_mode: self.flex_match_mode.clone(),
        }
    }
}

/// What the service answers when no matchmaking ticket is stored under the
/// `MatchmakingIdStringModel` an operation was asked about — the one text the service's
/// ticket lookup failure reads as, shared by every operation that names a ticket, so the
/// same missing ticket reads the same way whichever of them was called. The modelled error
/// CLASS it is carried in is each operation's own (`smithy_query`).
///
/// The ticket id the CALLER supplied is the sentence's one variable part: a ticket is
/// "identified by a unique ticket ID", so the refusal states which id was asked for.
pub fn matchmaking_ticket_not_found_message(ticket_id: &str) -> String {
    format!("Matchmaking ticket not found: {ticket_id}")
}

/// A matchmaking request, as `StartMatchmaking` persisted it.
#[derive(Debug, Clone)]
pub struct MatchmakingTicketRecord {
    pub ticket_id: String,
    pub configuration_name: String,
    pub configuration_arn: String,
    pub status: String,
    pub status_reason: Option<String>,
    pub status_message: Option<String>,
    pub start_time: SystemTime,
    /// "Time stamp indicating when the matchmaking request stopped being processed due to
    /// successful completion, timeout, or cancellation": set once the ticket is in a
    /// terminal status.
    pub end_time: Option<SystemTime>,
    /// The players to find matches for, exactly as the request supplied them.
    pub players: Vec<Player>,
    /// When `QUEUED` becomes `SEARCHING`; `None` once FlexMatch has picked the ticket up.
    pub searching_at: Option<SystemTime>,
    /// When the request times out: `start_time` plus the configuration's
    /// `RequestTimeoutSeconds` — "the maximum duration, in seconds, that a matchmaking
    /// ticket can remain in process before timing out".
    pub timeout_at: SystemTime,
}

impl MatchmakingTicketRecord {
    /// Whether this ticket is still being worked on, i.e. it has not reached a terminal
    /// status.
    ///
    /// `EndTime` is set only "when the matchmaking request stopped being processed due to
    /// successful completion, timeout, or cancellation", so a ticket in none of the
    /// terminal statuses ([`MATCHMAKING_TICKET_TERMINAL_STATUSES`]) is one the service is
    /// still processing — which is what decides the timeout the clock owes it, whether a
    /// caller can stop it, and whether its matchmaker may be deleted.
    pub fn is_in_process(&self) -> bool {
        !MATCHMAKING_TICKET_TERMINAL_STATUSES.contains(&self.status.as_str())
    }

    /// This ticket as the modelled `MatchmakingTicket`.
    ///
    /// `GameSessionConnectionInfo` is "connection information for a new game session[,]
    /// once a match is made", and `EstimatedWaitTime` is reported only when "there is
    /// enough recent data", so a ticket that has matched nothing reports neither.
    pub fn to_modelled(&self) -> MatchmakingTicket {
        MatchmakingTicket {
            ticket_id: Some(self.ticket_id.clone()),
            configuration_name: Some(self.configuration_name.clone()),
            configuration_arn: Some(self.configuration_arn.clone()),
            status: Some(self.status.clone()),
            status_reason: self.status_reason.clone(),
            status_message: self.status_message.clone(),
            start_time: Some(to_date_time(self.start_time)),
            end_time: self.end_time.map(to_date_time),
            players: Some(self.players.clone()),
            game_session_connection_info: None,
            estimated_wait_time: None,
        }
    }
}

/// What the service answers when no game session placement is stored under the
/// `PlacementId` an operation was asked about — the message of the modelled
/// `NotFoundException`, shared by every operation that must act on a placement named by
/// its id.
///
/// A placement is the service's reservation for the game session a queue is asked to
/// start (`StartGameSessionPlacement` "generates a new game session placement request and
/// adds it to the game session queue"), and a reservation is held per account, so the
/// refusal states which reservation was asked for and which account it was looked for in.
/// Those two are its only variable parts: the id comes from the request, the account from
/// the request's own context — the one account every emulated response presents.
pub fn placement_not_found_message(placement_id: &str) -> String {
    format!(
        "No GameSessionReservation found for reservation {placement_id} accountId {}",
        ::engine::emulator_account_id()
    )
}

/// A game session placement request, as `StartGameSessionPlacement` persisted it.
#[derive(Debug, Clone)]
pub struct GameSessionPlacementRecord {
    pub placement_id: String,
    /// The `Name` of the queue that processes this placement, resolved from whichever of
    /// the name and the ARN the request supplied.
    pub queue_name: String,
    pub status: String,
    pub game_properties: Vec<GameProperty>,
    pub maximum_player_session_count: i32,
    pub game_session_name: Option<String>,
    pub player_latencies: Vec<PlayerLatency>,
    /// The players a fulfilled placement reserves a slot for: "you can request new player
    /// sessions for a group of players[;] include the DesiredPlayerSessions parameter".
    pub desired_player_sessions: Vec<DesiredPlayerSession>,
    pub game_session_data: Option<String>,
    pub priority_configuration_override: Option<PriorityConfigurationOverride>,
    pub start_time: SystemTime,
    /// "Time stamp indicating when this request was completed, canceled, or timed out."
    pub end_time: Option<SystemTime>,
    /// When the queue next tries to find a destination that can host the session. "Amazon
    /// GameLift Servers may attempt to place a game session multiple times before
    /// succeeding", so a failed attempt sets this forward instead of failing the request.
    pub attempt_at: SystemTime,
    /// When the request times out: `start_time` plus the queue's `TimeoutInSeconds` —
    /// "the maximum time, in seconds, that a new game session placement request remains
    /// in the queue".
    pub timeout_at: SystemTime,
    /// The game session the queue started for this placement, once one is running. The
    /// `GameSessionId`, `GameSessionArn` and connection information a placement reports
    /// are read from THAT session: "this value isn't final until placement status is
    /// FULFILLED".
    pub game_session_id: Option<String>,
    /// The Region the game session this placement started runs in, which is the Region of
    /// the fleet hosting it.
    pub game_session_region: Option<String>,
    pub placed_player_sessions: Vec<PlacedPlayerSession>,
}

impl GameSessionPlacementRecord {
    /// This placement as the modelled `GameSessionPlacement`, with the connection
    /// information read from the game session it placed — so the placement and that
    /// session can only agree.
    pub fn to_modelled(&self, session: Option<&GameSessionRecord>) -> GameSessionPlacement {
        let fulfilled = self.status == PLACEMENT_STATUS_FULFILLED;
        let session = session.filter(|_| fulfilled);
        GameSessionPlacement {
            placement_id: Some(self.placement_id.clone()),
            game_session_queue_name: Some(self.queue_name.clone()),
            status: Some(self.status.clone()),
            game_properties: Some(self.game_properties.clone()),
            maximum_player_session_count: Some(self.maximum_player_session_count),
            game_session_name: self.game_session_name.clone(),
            game_session_id: session.map(|session| session.game_session_id.clone()),
            game_session_arn: session.map(|session| session.game_session_id.clone()),
            game_session_region: session.and_then(|_| self.game_session_region.clone()),
            player_latencies: Some(self.player_latencies.clone()),
            start_time: Some(to_date_time(self.start_time)),
            end_time: self.end_time.map(to_date_time),
            ip_address: session.and_then(|session| session.ip_address.clone()),
            dns_name: session.and_then(|session| session.dns_name.clone()),
            port: session.and_then(|session| session.port),
            placed_player_sessions: Some(self.placed_player_sessions.clone()),
            game_session_data: self.game_session_data.clone(),
            // `MatchmakerData` is "information on the matchmaking process for this game",
            // which only a placement FlexMatch started carries; nothing here was started
            // by a match.
            matchmaker_data: None,
            priority_configuration_override: self.priority_configuration_override.clone(),
            // `PlayerGatewayStatus` is reported by the player-gateway slice, which no
            // fleet in this emulator enables.
            player_gateway_status: None,
        }
    }
}

/// How long a VPC peering authorization stays valid for after it was issued.
///
/// `VpcPeeringAuthorization$ExpirationTime`'s own doc states the span: "Time stamp
/// indicating when this authorization expires (24 hours after issuance)", and
/// `CreateVpcPeeringAuthorization`'s doc repeats it — "The authorization remains valid
/// for 24 hours unless it is canceled." So this duration is DOCUMENTED, not chosen.
pub const VPC_PEERING_AUTHORIZATION_VALIDITY: Duration = Duration::from_secs(24 * 60 * 60);

/// An authorization to peer a VPC with the VPC of an Amazon GameLift Servers fleet, as
/// `CreateVpcPeeringAuthorization` persisted it — the one projection every read of the
/// authorization is served from.
///
/// `VpcPeeringAuthorization`'s doc: "Represents an authorization for a VPC peering
/// connection between the VPC for an Amazon GameLift Servers fleet and another VPC on an
/// account you have access to." The pair it is issued for — the account that manages the
/// fleets, and the VPC to be peered with them — is what identifies it, so that pair is
/// the record's key in [`ComAmazonawsGameliftGameliftState::vpc_peering_authorizations`].
#[derive(Debug, Clone)]
pub struct VpcPeeringAuthorizationRecord {
    /// The account "that you use to manage your Amazon GameLift Servers fleet", as the
    /// request named it.
    pub game_lift_aws_account_id: String,
    /// "The authorization's peer VPC Amazon Web Services account ID": the account the
    /// call was made from, which owns the VPC being authorized — the doc directs the
    /// caller to "call this operation from the Amazon Web Services account with the VPC
    /// that you want to peer".
    pub peer_vpc_aws_account_id: String,
    pub peer_vpc_id: String,
    pub creation_time: SystemTime,
    /// When the authorization stops being valid: [`VPC_PEERING_AUTHORIZATION_VALIDITY`]
    /// after it was issued.
    pub expiration_time: SystemTime,
}

impl VpcPeeringAuthorizationRecord {
    /// This authorization as the modelled `VpcPeeringAuthorization`.
    pub fn to_modelled(&self) -> VpcPeeringAuthorization {
        VpcPeeringAuthorization {
            game_lift_aws_account_id: Some(self.game_lift_aws_account_id.clone()),
            peer_vpc_aws_account_id: Some(self.peer_vpc_aws_account_id.clone()),
            peer_vpc_id: Some(self.peer_vpc_id.clone()),
            creation_time: Some(to_date_time(self.creation_time)),
            expiration_time: Some(to_date_time(self.expiration_time)),
        }
    }
}

/// What the service answers when the rule set document cannot be read as a rule set at
/// all, so the rule language version it is written in cannot be extracted from it: the
/// service's fixed text for that failure, whose only variable part is the position in the
/// document it was reached at — "top level" for a document nothing could be read from.
///
/// The reading failure is reported as the failure it is, never as the diagnostic of
/// whichever parser did the reading: an expected token, a line and a column describe the
/// implementation that tried, not the rule set the caller sent, and they differ for every
/// body, so nothing can match on them.
pub const RULE_SET_VERSION_EXTRACTION_FAILURE: &str =
    "Encountered IOException attempting to extract version; location: top level";

/// Whether `body` is a FlexMatch rule set, or the reason it is not — which both
/// `CreateMatchmakingRuleSet` and `ValidateMatchmakingRuleSet` answer with the modelled
/// `InvalidRequestException`.
///
/// `MatchmakingRuleSet$RuleSetBody`'s doc: "A collection of matchmaking rules, formatted
/// as a JSON string." `MatchmakingRuleSet`'s own doc names the elements a rule set defines
/// and which of them the service requires: "A rule set may define the following elements
/// for a match[:] [...] A rule set must specify teams" — so the two elements a body cannot
/// omit are the rule language version it is written in and the team definitions the match
/// is built from, each team naming itself and the player counts it holds. Everything else
/// the rule language allows (`name`, `playerAttributes`, `algorithm`, `rules`,
/// `expansions`) is optional, and a body is refused ONLY for what is required: a rule this
/// emulator does not interpret is still stored and reported verbatim.
///
/// The reason names the element and the constraint it missed, in the form the service's
/// own rule-set errors take (`<what is wrong>; location: <where>`).
pub fn validate_rule_set_body(body: &str) -> Result<(), String> {
    // The version is read out of the rule set document itself, so a body that is not a
    // readable document — unparseable, or parseable but not an object with elements to
    // read — fails that one extraction, and is refused with the text the service gives it
    // rather than with the parser's own complaint.
    let parsed: serde_json::Value = serde_json::from_str(body)
        .map_err(|_| RULE_SET_VERSION_EXTRACTION_FAILURE.to_string())?;
    let Some(object) = parsed.as_object() else {
        return Err(RULE_SET_VERSION_EXTRACTION_FAILURE.to_string());
    };

    match object.get("ruleLanguageVersion") {
        None => {
            return Err(
                "Rule set does not define ruleLanguageVersion; location: top level".to_string(),
            )
        }
        Some(version) if !version.is_string() => {
            return Err(
                "ruleLanguageVersion must be a string, such as \"1.0\"; location: top level"
                    .to_string(),
            )
        }
        Some(_) => {}
    }

    let Some(teams) = object.get("teams") else {
        return Err("Rule set does not define team definitions; location: top level".to_string());
    };
    let Some(teams) = teams.as_array() else {
        return Err("teams must be a list of team definitions; location: top level".to_string());
    };
    if teams.is_empty() {
        return Err("teams must define at least one team; location: top level".to_string());
    }
    let mut names = HashSet::new();
    for (index, team) in teams.iter().enumerate() {
        let Some(team) = team.as_object() else {
            return Err(format!("team must be an object; location: teams[{index}]"));
        };
        let name = match team.get("name").and_then(serde_json::Value::as_str) {
            Some(name) if !name.is_empty() => name,
            _ => {
                return Err(format!(
                    "team does not define a non-empty name; location: teams[{index}]"
                ))
            }
        };
        if !names.insert(name.to_string()) {
            return Err(format!(
                "team name {name} is defined more than once; location: teams[{index}]"
            ));
        }
        let Some(max_players) = team.get("maxPlayers").and_then(serde_json::Value::as_i64) else {
            return Err(format!(
                "team {name} does not define maxPlayers as a whole number; location: \
                 teams[{index}]"
            ));
        };
        if max_players < 1 {
            return Err(format!(
                "team {name} defines maxPlayers {max_players}; a team must hold at least one \
                 player; location: teams[{index}]"
            ));
        }
        if let Some(min_players) = team.get("minPlayers") {
            let Some(min_players) = min_players.as_i64() else {
                return Err(format!(
                    "team {name} defines minPlayers that is not a whole number; location: \
                     teams[{index}]"
                ));
            };
            if min_players < 0 || min_players > max_players {
                return Err(format!(
                    "team {name} defines minPlayers {min_players}, which is not between 0 and \
                     maxPlayers {max_players}; location: teams[{index}]"
                ));
            }
        }
    }

    // The remaining elements the rule language lists are optional, but each is a LIST
    // where it appears, so a body that spells one as something else is not a rule set.
    for element in ["playerAttributes", "rules", "expansions"] {
        if let Some(value) = object.get(element) {
            if !value.is_array() {
                return Err(format!(
                    "{element} must be a list; location: top level"
                ));
            }
        }
    }
    Ok(())
}

/// Per-service state for com-amazonaws-gamelift-gamelift. A handler reaches it through [`service_state`].
#[derive(Debug, Default)]
pub struct ComAmazonawsGameliftGameliftState {
    /// Fleets by `FleetId`.
    pub fleets: HashMap<String, Fleet>,
    /// Aliases by `AliasId`: "Alias IDs are unique within a Region", while "alias names
    /// do not need to be unique", so the id is the alias's key.
    pub aliases: HashMap<String, AliasRecord>,
    /// Game session queues by `Name`: "Queue names must be unique within each Region",
    /// and this store — like the fleet store — is the account's one store, so the name
    /// is the queue's key.
    pub game_session_queues: HashMap<String, GameSessionQueueRecord>,
    /// Custom locations by `LocationName`, which `CreateLocation` keeps unique: a second
    /// create under a name a location already holds answers `ConflictException`.
    pub custom_locations: HashMap<String, CustomLocationRecord>,
    /// Custom game builds by `BuildId`, "a unique identifier for the build", while build
    /// names "do not need to be unique" — so the id is the build's key.
    pub builds: HashMap<String, BuildRecord>,
    /// Realtime scripts by `ScriptId`, "a unique identifier for the Realtime script",
    /// while script names "do not need to be unique" — so the id is the script's key.
    pub scripts: HashMap<String, ScriptRecord>,
    /// Matchmaking rule sets by `RuleSetName`, which "a unique identifier for the
    /// matchmaking rule set" makes the record's key.
    pub matchmaking_rule_sets: HashMap<String, MatchmakingRuleSetRecord>,
    /// Matchmaking configurations by `Name`: "a unique identifier for the matchmaking
    /// configuration[,] used to identify the configuration associated with a matchmaking
    /// request or ticket".
    pub matchmaking_configurations: HashMap<String, MatchmakingConfigurationRecord>,
    /// Matchmaking tickets by `TicketId`, which `StartMatchmaking` calls "a unique ticket
    /// ID".
    pub matchmaking_tickets: HashMap<String, MatchmakingTicketRecord>,
    /// Game session placements by `PlacementId`, "a unique identifier for a game session
    /// placement".
    pub game_session_placements: HashMap<String, GameSessionPlacementRecord>,
    /// Compute resources registered with a fleet, keyed by that fleet's `FleetId` and the
    /// compute's own name: `RegisterCompute`'s doc — "give the compute a name (must be
    /// unique within the fleet)" — makes the pair the record's key, so the same name on
    /// another fleet is another compute.
    pub computes: HashMap<(String, String), ComputeRecord>,
    /// Game sessions by their full-ARN `GameSessionId`.
    pub game_sessions: HashMap<String, GameSessionRecord>,
    /// Player sessions by `PlayerSessionId`.
    pub player_sessions: HashMap<String, PlayerSessionRecord>,
    /// VPC peering authorizations by the pair they are issued for: the account that
    /// manages the Amazon GameLift Servers fleets, and the VPC to be peered with them.
    /// Re-issuing an authorization for a pair that already holds one replaces it, since
    /// an authorization is what "must exist and be valid for the peering connection to be
    /// established" for exactly that pair.
    pub vpc_peering_authorizations: HashMap<(String, String), VpcPeeringAuthorizationRecord>,
    /// `IdempotencyToken` (or caller-supplied `GameSessionId`) to the game session the
    /// request that first carried it created, keyed by the fleet it was created on:
    /// `IdempotentParameterMismatchException`'s doc scopes the custom string to one
    /// fleet ("A game session with this custom ID string already exists in this fleet"),
    /// so the same string on another fleet is a fresh session, not a mismatch.
    pub idempotency_tokens: HashMap<(String, String), String>,
    /// How many fleet instances have been provisioned, for minting their addresses.
    pub provisioned_instances: u32,
    /// How many resource ids this store has minted, which is what makes the next one
    /// unique (see [`mint_uuid`]).
    pub minted_ids: u64,
}

impl ComAmazonawsGameliftGameliftState {
    /// Apply every status change the clock has already made due, so that each handler
    /// reads and reports the state the service would be in at `now` whether or not
    /// anybody called in between.
    pub fn tick(&mut self, now: SystemTime) {
        // A deletion that has completed takes the fleet and everything placed on it.
        let purged: Vec<String> = self
            .fleets
            .iter()
            .filter(|(_, fleet)| fleet.purge_at.is_some_and(|due| now >= due))
            .map(|(fleet_id, _)| fleet_id.clone())
            .collect();
        for fleet_id in purged {
            self.remove_fleet(&fleet_id);
        }

        // A finished fleet-creation workflow leaves the fleet, and each of its
        // locations, ACTIVE.
        for fleet in self.fleets.values_mut() {
            if fleet.status == FLEET_STATUS_NEW && fleet.active_at.is_some_and(|due| now >= due) {
                fleet.status = FLEET_STATUS_ACTIVE.to_string();
                fleet.active_at = None;
                for location in &mut fleet.locations {
                    location.status = FLEET_STATUS_ACTIVE.to_string();
                }
            }
        }

        // An authentication token issued for a compute stops being valid at its
        // expiration, which the clock makes due whether or not anybody called: the compute
        // then holds none until a fresh one is requested, and the emulator keeps no secret
        // past its life.
        for compute in self.computes.values_mut() {
            if compute
                .auth_token
                .as_ref()
                .is_some_and(|token| !token.is_valid_at(now))
            {
                compute.auth_token = None;
            }
        }

        // A build whose files the service has finished reading is READY: "the game build
        // has been successfully uploaded. You can now create new fleets for this build."
        for build in self.builds.values_mut() {
            if build.status == BUILD_STATUS_INITIALIZED
                && build.ready_at.is_some_and(|due| now >= due)
            {
                build.status = BUILD_STATUS_READY.to_string();
                build.ready_at = None;
            }
        }

        // A started game session becomes ACTIVE and only then carries connection
        // information, which comes from the fleet instance hosting it.
        let starting: Vec<String> = self
            .game_sessions
            .iter()
            .filter(|(_, session)| {
                session.status == GAME_SESSION_STATUS_ACTIVATING
                    && session.active_at.is_some_and(|due| now >= due)
            })
            .map(|(id, _)| id.clone())
            .collect();
        for game_session_id in starting {
            let Some(fleet_id) = self
                .game_sessions
                .get(&game_session_id)
                .map(|session| session.fleet_id.clone())
            else {
                continue;
            };
            // The ports the fleet's other live sessions hold: no two of them may listen
            // on the same one.
            let held: HashSet<i32> = self
                .game_sessions
                .values()
                .filter(|session| session.fleet_id == fleet_id)
                .filter_map(|session| session.port)
                .collect();
            let Some(fleet) = self.fleets.get_mut(&fleet_id) else {
                continue;
            };
            let ip_address = fleet.ip_address.clone();
            let dns_name = fleet.dns_name();
            // `None` would mean the fleet opened no port this session could listen on,
            // which its capacity refuses at CreateGameSession; such a session stays
            // ACTIVATING rather than reporting a port another one holds.
            let Some(port) = fleet.next_game_session_port(&held) else {
                continue;
            };
            if let Some(session) = self.game_sessions.get_mut(&game_session_id) {
                session.status = GAME_SESSION_STATUS_ACTIVE.to_string();
                session.active_at = None;
                session.ip_address = Some(ip_address);
                session.dns_name = Some(dns_name);
                session.port = Some(port);
            }
        }

        self.tick_matchmaking_tickets(now);
        self.tick_game_session_placements(now);

        // An authorization that has reached its expiry is no longer one: it "must exist
        // and be valid for the peering connection to be established", and it expires 24
        // hours after issuance unless it is canceled first, so the clock alone retires it.
        self.vpc_peering_authorizations
            .retain(|_, authorization| now < authorization.expiration_time);
    }

    /// Move every matchmaking ticket the clock has carried forward.
    ///
    /// A ticket FlexMatch has picked up is `SEARCHING` ("the matchmaking request is
    /// currently being processed"), and one for which no match is formed within the
    /// configuration's `RequestTimeoutSeconds` is `TIMED_OUT` ("the matchmaking request
    /// was not successful within the duration specified in the matchmaking
    /// configuration").
    ///
    /// Forming a match is FlexMatch evaluating a rule set over the tickets of a
    /// matchmaker, which this emulator does not do — so no ticket here completes, and a
    /// ticket ends in the modelled `TIMED_OUT` status carrying the reason it did.
    fn tick_matchmaking_tickets(&mut self, now: SystemTime) {
        for ticket in self.matchmaking_tickets.values_mut() {
            if ticket.status == MATCHMAKING_TICKET_STATUS_QUEUED
                && ticket.searching_at.is_some_and(|due| now >= due)
            {
                ticket.status = MATCHMAKING_TICKET_STATUS_SEARCHING.to_string();
                ticket.searching_at = None;
            }
            let in_process = ticket.is_in_process();
            if in_process && now >= ticket.timeout_at {
                ticket.status = MATCHMAKING_TICKET_STATUS_TIMED_OUT.to_string();
                ticket.searching_at = None;
                // The instant the request became due to time out, not the instant
                // somebody happened to read it.
                ticket.end_time = Some(ticket.timeout_at);
                ticket.status_reason = Some("TIMED_OUT".to_string());
                ticket.status_message = Some(
                    "No match was formed for this ticket within the matchmaking \
                     configuration's RequestTimeoutSeconds."
                        .to_string(),
                );
            }
        }
    }

    /// Move every game session placement the clock has carried forward.
    ///
    /// A `PENDING` placement is one the queue is still working on: at each attempt it
    /// looks for a destination that can host the session and starts one there ("after
    /// selecting an available resource, Amazon GameLift Servers prompts the resource to
    /// start a game session"), and the placement is `FULFILLED` once that session is
    /// running — "when the request status is FULFILLED, a new game session has started
    /// and the placement request is updated with connection information for the game
    /// session", and "if the request included player session data, Amazon GameLift
    /// Servers creates a player session for each player ID in the request". A placement
    /// no destination can host keeps being retried "until it reaches the queue's timeout
    /// setting", and then times out.
    fn tick_game_session_placements(&mut self, now: SystemTime) {
        let pending: Vec<String> = self
            .game_session_placements
            .iter()
            .filter(|(_, placement)| placement.status == PLACEMENT_STATUS_PENDING)
            .map(|(placement_id, _)| placement_id.clone())
            .collect();
        for placement_id in pending {
            let Some(placement) = self.game_session_placements.get(&placement_id) else {
                continue;
            };
            match placement.game_session_id.clone() {
                // The queue already started a session for this placement: the placement
                // is fulfilled once that session is running and carries the connection
                // information the placement reports.
                Some(game_session_id) => {
                    let started = self
                        .game_sessions
                        .get(&game_session_id)
                        .is_some_and(|session| session.status == GAME_SESSION_STATUS_ACTIVE);
                    if started {
                        self.fulfil_placement(&placement_id, &game_session_id, now);
                    } else if !self.game_sessions.contains_key(&game_session_id) {
                        // The session the queue started is gone (its fleet was deleted),
                        // so this attempt placed nothing: the queue tries again.
                        if let Some(placement) =
                            self.game_session_placements.get_mut(&placement_id)
                        {
                            placement.game_session_id = None;
                            placement.game_session_region = None;
                        }
                    }
                }
                None if now >= placement.timeout_at => {
                    if let Some(placement) = self.game_session_placements.get_mut(&placement_id) {
                        placement.status = PLACEMENT_STATUS_TIMED_OUT.to_string();
                        placement.end_time = Some(placement.timeout_at);
                    }
                }
                None if now >= placement.attempt_at => {
                    let started = self.start_placed_game_session(&placement_id, now);
                    if let Some(placement) = self.game_session_placements.get_mut(&placement_id) {
                        match started {
                            Some((game_session_id, region)) => {
                                placement.game_session_id = Some(game_session_id);
                                placement.game_session_region = Some(region);
                            }
                            // "Amazon GameLift Servers may attempt to place a game
                            // session multiple times before succeeding": the next attempt
                            // is one span away, until the queue's timeout is reached.
                            None => placement.attempt_at = now + WORKFLOW_DELAY,
                        }
                    }
                }
                None => {}
            }
        }
    }

    /// Start a game session for `placement_id` on the first destination of its queue that
    /// can host one, and return that session's id.
    ///
    /// The queue's destinations are "a list of fleets and/or aliases that can be used to
    /// fulfill game session placement requests in the queue[, in] the order of when to
    /// place the game session", so they are walked in order; a destination that names no
    /// stored fleet, a fleet that is not `ACTIVE`, or one whose server processes and
    /// opened ports are all taken cannot host, exactly as `CreateGameSession` decides.
    /// `None` means no destination could, which leaves the placement pending.
    fn start_placed_game_session(
        &mut self,
        placement_id: &str,
        now: SystemTime,
    ) -> Option<(String, String)> {
        let placement = self.game_session_placements.get(placement_id)?.clone();
        let queue = self.game_session_queues.get(&placement.queue_name)?.clone();
        for destination in &queue.destinations {
            let reference = destination.destination_arn.as_deref().unwrap_or_default();
            // "In most situations, you can use an alias ID in place of a fleet ID": a
            // destination may name either, and a terminal alias resolves to no fleet.
            let fleet_id = match self.resolve_fleet_id(reference) {
                Some(fleet_id) => fleet_id,
                None => match self.resolve_alias(reference).map(AliasRecord::routing) {
                    Some(AliasRouting::Fleet(fleet_id)) => {
                        let fleet_id = fleet_id.to_string();
                        match self.resolve_fleet_id(&fleet_id) {
                            Some(fleet_id) => fleet_id,
                            None => continue,
                        }
                    }
                    _ => continue,
                },
            };
            let Some(fleet) = self.fleets.get(&fleet_id) else {
                continue;
            };
            if fleet.status != FLEET_STATUS_ACTIVE {
                continue;
            }
            let location = fleet.home_location.clone();
            let fleet_arn = fleet.fleet_arn.clone();
            let region = fleet.region.clone();
            let capacity = fleet.game_session_capacity(&location);
            let open_ports = fleet.open_port_count();
            if self.live_game_sessions_at(&fleet_id, &location) >= capacity
                || self.live_game_sessions_on(&fleet_id) >= open_ports
            {
                continue;
            }

            let id_string = mint_uuid(&mut self.minted_ids, "game-session");
            let game_session_id =
                gamelift_arn(&region, &format!("gamesession/{fleet_id}/{id_string}"));
            let record = GameSessionRecord {
                game_session_id: game_session_id.clone(),
                name: placement.game_session_name.clone(),
                fleet_id,
                fleet_arn,
                status: GAME_SESSION_STATUS_ACTIVATING.to_string(),
                active_at: Some(now + WORKFLOW_DELAY),
                creation_time: now,
                termination_time: None,
                current_player_session_count: 0,
                maximum_player_session_count: placement.maximum_player_session_count,
                game_properties: placement.game_properties.clone(),
                game_session_data: placement.game_session_data.clone(),
                // A placed session has no creator: the queue started it.
                creator_id: None,
                location,
                player_session_creation_policy: PLAYER_SESSION_CREATION_POLICY_ACCEPT_ALL
                    .to_string(),
                ip_address: None,
                port: None,
                dns_name: None,
            };
            self.game_sessions.insert(game_session_id.clone(), record);
            return Some((game_session_id, region));
        }
        None
    }

    /// Mark `placement_id` fulfilled on the game session that is now running for it, and
    /// reserve a player slot in that session for each player the request asked for.
    fn fulfil_placement(&mut self, placement_id: &str, game_session_id: &str, now: SystemTime) {
        let Some(placement) = self.game_session_placements.get(placement_id).cloned() else {
            return;
        };
        let Some(session) = self.game_sessions.get(game_session_id).cloned() else {
            return;
        };
        // "If the request included player session data, Amazon GameLift Servers creates a
        // player session for each player ID in the request" — never more than the slots
        // the session was created with, which `StartGameSessionPlacement` refuses up
        // front.
        let open_slots = session
            .maximum_player_session_count
            .min(MAX_PLAYERS_PER_GAME_SESSION);
        let mut placed: Vec<PlacedPlayerSession> = Vec::new();
        let mut reserved = session.current_player_session_count;
        for desired in &placement.desired_player_sessions {
            let Some(player_id) = desired.player_id.clone() else {
                continue;
            };
            if reserved >= open_slots {
                break;
            }
            let player_session_id =
                format!("psess-{}", mint_uuid(&mut self.minted_ids, "player-session"));
            self.player_sessions.insert(
                player_session_id.clone(),
                PlayerSessionRecord {
                    player_session_id: player_session_id.clone(),
                    player_id: player_id.clone(),
                    game_session_id: session.game_session_id.clone(),
                    fleet_id: session.fleet_id.clone(),
                    fleet_arn: session.fleet_arn.clone(),
                    status: PLAYER_SESSION_STATUS_RESERVED.to_string(),
                    creation_time: now,
                    termination_time: None,
                    ip_address: session.ip_address.clone(),
                    port: session.port,
                    dns_name: session.dns_name.clone(),
                    player_data: desired.player_data.clone(),
                },
            );
            reserved += 1;
            placed.push(PlacedPlayerSession {
                player_id: Some(player_id),
                player_session_id: Some(player_session_id),
            });
        }
        if let Some(session) = self.game_sessions.get_mut(game_session_id) {
            session.current_player_session_count = reserved;
        }
        if let Some(placement) = self.game_session_placements.get_mut(placement_id) {
            placement.status = PLACEMENT_STATUS_FULFILLED.to_string();
            placement.end_time = Some(now);
            placement.placed_player_sessions = placed;
        }
    }

    /// Drop a fleet, the computes registered with it, the game sessions placed on it, the
    /// player slots reserved in those sessions, and the idempotency tokens that named them.
    pub fn remove_fleet(&mut self, fleet_id: &str) {
        self.fleets.remove(fleet_id);
        // A compute belongs to one fleet (`Compute$FleetId`), so the fleet going away takes
        // its registered computes with it rather than leaving them readable.
        self.computes.retain(|(fleet, _), _| fleet != fleet_id);
        let sessions: HashSet<String> = self
            .game_sessions
            .iter()
            .filter(|(_, session)| session.fleet_id == fleet_id)
            .map(|(id, _)| id.clone())
            .collect();
        for game_session_id in &sessions {
            self.game_sessions.remove(game_session_id);
            self.player_sessions
                .retain(|_, player| &player.game_session_id != game_session_id);
        }
        self.idempotency_tokens
            .retain(|_, game_session_id| !sessions.contains(game_session_id));
    }

    /// How many game sessions are placed on `fleet_id` in `location` and have not reached
    /// a terminal status, i.e. how much of that fleet location's capacity is in use.
    pub fn live_game_sessions_at(&self, fleet_id: &str, location: &str) -> usize {
        self.live_game_sessions(fleet_id, Some(location))
    }

    /// How many game sessions are placed on `fleet_id`, in any of its locations, and have
    /// not reached a terminal status — so how many of the ports the fleet opens are held.
    pub fn live_game_sessions_on(&self, fleet_id: &str) -> usize {
        self.live_game_sessions(fleet_id, None)
    }

    /// The live game sessions of a fleet, optionally narrowed to one of its locations.
    fn live_game_sessions(&self, fleet_id: &str, location: Option<&str>) -> usize {
        self.game_sessions
            .values()
            .filter(|session| {
                session.fleet_id == fleet_id
                    && location.is_none_or(|location| session.location == location)
                    && (session.status == GAME_SESSION_STATUS_ACTIVATING
                        || session.status == GAME_SESSION_STATUS_ACTIVE)
            })
            .count()
    }

    /// The `FleetId` of the fleet a `FleetIdOrArn` names, or `None` when no stored
    /// fleet matches it.
    pub fn resolve_fleet_id(&self, id_or_arn: &str) -> Option<String> {
        if id_or_arn.starts_with("arn:") {
            self.fleets
                .values()
                .find(|fleet| fleet.fleet_arn == id_or_arn)
                .map(|fleet| fleet.fleet_id.clone())
        } else {
            self.fleets
                .get(id_or_arn)
                .map(|fleet| fleet.fleet_id.clone())
        }
    }

    /// The compute `name_or_arn` names in the fleet `fleet_id`, or `None` when that fleet
    /// holds none under it.
    ///
    /// `Compute$ComputeArn` is "the ARN that is assigned to a compute resource and uniquely
    /// identifies it", so either of the forms `ComputeNameOrArn` accepts selects the same
    /// compute.
    pub fn resolve_compute(&self, fleet_id: &str, name_or_arn: &str) -> Option<&ComputeRecord> {
        if name_or_arn.starts_with("arn:") {
            self.computes
                .values()
                .find(|compute| compute.fleet_id == fleet_id && compute.compute_arn == name_or_arn)
        } else {
            self.computes
                .get(&(fleet_id.to_string(), name_or_arn.to_string()))
        }
    }

    /// The authentication token the compute registered under `(fleet_id, compute_name)`
    /// presents at `now`, issuing `minted` as that compute's token when it holds no valid
    /// one.
    ///
    /// `GetComputeAuthToken`'s doc has every game server process on a compute working from
    /// the same token — "all game servers that are running on all fleet instances have
    /// access to a valid auth token" — and has a caller "request a fresh token before the
    /// current token expires", so a compute holds ONE token at a time: while that token is
    /// still valid every request for it is answered with it, and the first request after it
    /// expires is answered with a freshly minted one, valid for
    /// [`COMPUTE_AUTH_TOKEN_VALIDITY`] from the instant it was issued. `None` when no
    /// compute is registered under the pair.
    pub fn issue_compute_auth_token(
        &mut self,
        fleet_id: &str,
        compute_name: &str,
        minted: String,
        now: SystemTime,
    ) -> Option<ComputeAuthTokenRecord> {
        let compute = self
            .computes
            .get_mut(&(fleet_id.to_string(), compute_name.to_string()))?;
        let issued = compute
            .auth_token
            .take()
            .filter(|held| held.is_valid_at(now))
            .unwrap_or(ComputeAuthTokenRecord {
                token: minted,
                expires_at: now + COMPUTE_AUTH_TOKEN_VALIDITY,
            });
        // Reported from the record the compute keeps, so this response and the next
        // request for the same compute's token can only agree.
        Some(compute.auth_token.insert(issued).clone())
    }

    /// The computes registered with `fleet_id`, oldest first, so a paginated walk is
    /// stable across calls.
    pub fn computes_in_order(&self, fleet_id: &str) -> Vec<&ComputeRecord> {
        let mut computes: Vec<&ComputeRecord> = self
            .computes
            .values()
            .filter(|compute| compute.fleet_id == fleet_id)
            .collect();
        computes.sort_by(|left, right| {
            left.creation_time
                .cmp(&right.creation_time)
                .then_with(|| left.compute_name.cmp(&right.compute_name))
        });
        computes
    }

    /// Every stored fleet, oldest first, so a paginated walk is stable across calls.
    pub fn fleets_in_order(&self) -> Vec<&Fleet> {
        let mut fleets: Vec<&Fleet> = self.fleets.values().collect();
        fleets.sort_by(|left, right| {
            left.creation_time
                .cmp(&right.creation_time)
                .then_with(|| left.fleet_id.cmp(&right.fleet_id))
        });
        fleets
    }

    /// Every stored game session, oldest first, so a paginated walk is stable.
    pub fn game_sessions_in_order(&self) -> Vec<&GameSessionRecord> {
        let mut sessions: Vec<&GameSessionRecord> = self.game_sessions.values().collect();
        sessions.sort_by(|left, right| {
            left.creation_time
                .cmp(&right.creation_time)
                .then_with(|| left.game_session_id.cmp(&right.game_session_id))
        });
        sessions
    }

    /// The alias an `AliasIdOrArn` names, or `None` when no stored alias matches it.
    ///
    /// `AliasArn`'s doc: "In a GameLift alias ARN, the resource ID matches the alias ID
    /// value", so either form selects the same alias.
    pub fn resolve_alias(&self, id_or_arn: &str) -> Option<&AliasRecord> {
        if id_or_arn.starts_with("arn:") {
            self.aliases
                .values()
                .find(|alias| alias.alias_arn == id_or_arn)
        } else {
            self.aliases.get(id_or_arn)
        }
    }

    /// Every stored alias, oldest first, so a paginated walk is stable across calls.
    ///
    /// `ListAliases` documents that "returned aliases are not listed in any particular
    /// order", so any total order serves it; a stable one is what lets a `NextToken`
    /// resume where the previous page ended instead of skipping or repeating a record.
    /// "Alias names do not need to be unique" while ids are, so the id breaks a tie
    /// between two aliases created at the same instant.
    pub fn aliases_in_order(&self) -> Vec<&AliasRecord> {
        let mut aliases: Vec<&AliasRecord> = self.aliases.values().collect();
        aliases.sort_by(|left, right| {
            left.creation_time
                .cmp(&right.creation_time)
                .then_with(|| left.alias_id.cmp(&right.alias_id))
        });
        aliases
    }

    /// The build a `BuildIdOrArn` names, or `None` when no stored build matches it.
    ///
    /// `Build$BuildArn`'s doc: "In a GameLift build ARN, the resource ID matches the
    /// `BuildId` value", so either form selects the same build — which is why the member's
    /// pattern accepts both.
    pub fn resolve_build(&self, id_or_arn: &str) -> Option<&BuildRecord> {
        if id_or_arn.starts_with("arn:") {
            self.builds.values().find(|build| build.build_arn == id_or_arn)
        } else {
            self.builds.get(id_or_arn)
        }
    }

    /// The `BuildId` of the build a `BuildIdOrArn` names, or `None` when no stored build
    /// matches it — [`resolve_build`](Self::resolve_build) for a caller that needs the
    /// record rather than the key to reach it by.
    pub fn resolve_build_id(&self, id_or_arn: &str) -> Option<String> {
        self.resolve_build(id_or_arn)
            .map(|build| build.build_id.clone())
    }

    /// The script a `ScriptIdOrArn` names, or `None` when no stored script matches it.
    ///
    /// `Script$ScriptArn`'s doc: "In a GameLift script ARN, the resource ID matches the
    /// `ScriptId` value", so either form selects the same script — which is why the
    /// member's pattern accepts both.
    pub fn resolve_script(&self, id_or_arn: &str) -> Option<&ScriptRecord> {
        if id_or_arn.starts_with("arn:") {
            self.scripts
                .values()
                .find(|script| script.script_arn == id_or_arn)
        } else {
            self.scripts.get(id_or_arn)
        }
    }

    /// The `Name` of the queue a `GameSessionQueueNameOrArn` names, or `None` when no
    /// stored queue matches it.
    ///
    /// `GameSessionQueueArn`'s doc: "In a Amazon GameLift Servers game session queue ARN,
    /// the resource ID matches the `Name` value", so either form selects the same queue.
    pub fn resolve_queue_name(&self, name_or_arn: &str) -> Option<String> {
        if name_or_arn.starts_with("arn:") {
            self.game_session_queues
                .values()
                .find(|queue| queue.queue_arn == name_or_arn)
                .map(|queue| queue.name.clone())
        } else {
            self.game_session_queues
                .get(name_or_arn)
                .map(|queue| queue.name.clone())
        }
    }

    /// Every stored game session queue, oldest first, so a paginated walk is stable
    /// across calls.
    pub fn game_session_queues_in_order(&self) -> Vec<&GameSessionQueueRecord> {
        let mut queues: Vec<&GameSessionQueueRecord> = self.game_session_queues.values().collect();
        queues.sort_by(|left, right| {
            left.creation_time
                .cmp(&right.creation_time)
                .then_with(|| left.name.cmp(&right.name))
        });
        queues
    }

    /// The `RuleSetName` of the rule set a `MatchmakingRuleSetName` names, or `None` when
    /// no stored rule set matches it.
    ///
    /// `MatchmakingRuleSetArn`'s doc: "In a GameLift rule set ARN, the resource ID matches
    /// the `RuleSetName` value", so either form selects the same rule set — which is why
    /// the member's pattern accepts both.
    pub fn resolve_rule_set_name(&self, name_or_arn: &str) -> Option<String> {
        if name_or_arn.starts_with("arn:") {
            self.matchmaking_rule_sets
                .values()
                .find(|rule_set| rule_set.rule_set_arn == name_or_arn)
                .map(|rule_set| rule_set.rule_set_name.clone())
        } else {
            self.matchmaking_rule_sets
                .get(name_or_arn)
                .map(|rule_set| rule_set.rule_set_name.clone())
        }
    }

    /// Every stored matchmaking rule set, oldest first, so a paginated walk is stable.
    pub fn matchmaking_rule_sets_in_order(&self) -> Vec<&MatchmakingRuleSetRecord> {
        let mut rule_sets: Vec<&MatchmakingRuleSetRecord> =
            self.matchmaking_rule_sets.values().collect();
        rule_sets.sort_by(|left, right| {
            left.creation_time
                .cmp(&right.creation_time)
                .then_with(|| left.rule_set_name.cmp(&right.rule_set_name))
        });
        rule_sets
    }

    /// The `Name` of the matchmaking configuration a `MatchmakingConfigurationName` names,
    /// or `None` when no stored configuration matches it.
    ///
    /// `MatchmakingConfigurationArn`'s doc: "In a Amazon GameLift Servers configuration
    /// ARN, the resource ID matches the `Name` value", so either form selects the same
    /// configuration.
    pub fn resolve_matchmaking_configuration_name(&self, name_or_arn: &str) -> Option<String> {
        if name_or_arn.starts_with("arn:") {
            self.matchmaking_configurations
                .values()
                .find(|configuration| configuration.configuration_arn == name_or_arn)
                .map(|configuration| configuration.name.clone())
        } else {
            self.matchmaking_configurations
                .get(name_or_arn)
                .map(|configuration| configuration.name.clone())
        }
    }

    /// Every stored matchmaking configuration, oldest first, so a paginated walk is
    /// stable.
    pub fn matchmaking_configurations_in_order(&self) -> Vec<&MatchmakingConfigurationRecord> {
        let mut configurations: Vec<&MatchmakingConfigurationRecord> =
            self.matchmaking_configurations.values().collect();
        configurations.sort_by(|left, right| {
            left.creation_time
                .cmp(&right.creation_time)
                .then_with(|| left.name.cmp(&right.name))
        });
        configurations
    }

    /// Every stored VPC peering authorization, oldest first, so two reads of the
    /// collection can only agree on its order.
    pub fn vpc_peering_authorizations_in_order(&self) -> Vec<&VpcPeeringAuthorizationRecord> {
        let mut authorizations: Vec<&VpcPeeringAuthorizationRecord> =
            self.vpc_peering_authorizations.values().collect();
        authorizations.sort_by(|left, right| {
            left.creation_time
                .cmp(&right.creation_time)
                .then_with(|| left.peer_vpc_id.cmp(&right.peer_vpc_id))
                .then_with(|| {
                    left.game_lift_aws_account_id
                        .cmp(&right.game_lift_aws_account_id)
                })
        });
        authorizations
    }

    /// The `LocationName` of the custom location a `CustomLocationNameOrArnModel` names, or
    /// `None` when no stored custom location matches it.
    ///
    /// `LocationModel$LocationArn`'s pattern is `^arn:.*:location\/custom-\S+$`, whose
    /// resource is the location's own name, so either form selects the same location —
    /// which is why the member's pattern accepts both.
    pub fn resolve_custom_location_name(&self, name_or_arn: &str) -> Option<String> {
        if name_or_arn.starts_with("arn:") {
            self.custom_locations
                .values()
                .find(|location| location.location_arn == name_or_arn)
                .map(|location| location.location_name.clone())
        } else {
            self.custom_locations
                .get(name_or_arn)
                .map(|location| location.location_name.clone())
        }
    }

    /// Every stored custom location, oldest first, so a paginated walk is stable.
    pub fn custom_locations_in_order(&self) -> Vec<&CustomLocationRecord> {
        let mut locations: Vec<&CustomLocationRecord> = self.custom_locations.values().collect();
        locations.sort_by(|left, right| {
            left.creation_time
                .cmp(&right.creation_time)
                .then_with(|| left.location_name.cmp(&right.location_name))
        });
        locations
    }

    /// Every Amazon Web Services location this store hosts in, the published catalog
    /// first.
    ///
    /// `ListLocations` "lists all custom and Amazon Web Services locations where Amazon
    /// GameLift Servers can host game servers", so the Amazon Web Services ones are not
    /// resources a caller creates but the service's own published set of hosting
    /// locations: [`MANAGED_LOCATIONS`], in its order, followed — in name order — by any
    /// further location this store knows and that table does not carry, namely the Region
    /// the request is served in and every location a stored fleet deploys to that is not a
    /// custom one.
    pub fn aws_locations(&self, request_region: &str) -> Vec<String> {
        let mut locations: Vec<String> = MANAGED_LOCATIONS
            .iter()
            .map(|(location, _)| (*location).to_string())
            .collect();
        let held: HashSet<&str> = MANAGED_LOCATIONS
            .iter()
            .map(|(location, _)| *location)
            .collect();
        let mut extra: Vec<String> = self
            .fleets
            .values()
            .flat_map(|fleet| fleet.locations.iter())
            .map(|location| location.location.clone())
            .chain(::std::iter::once(request_region.to_string()))
            .filter(|location| !is_custom_location(location) && !held.contains(location.as_str()))
            .collect::<HashSet<String>>()
            .into_iter()
            .collect();
        extra.sort();
        locations.extend(extra);
        locations
    }

    /// An address for a newly provisioned fleet instance, held by no stored fleet.
    ///
    /// Drawn from 203.0.113.0/24, which RFC 5737 reserves for documentation, so an
    /// address this emulator reports can never name a real host. Two live fleets must
    /// never report the same connection information — a non-TLS fleet's `DnsName` is
    /// derived from this address — so the counter walks past an address a stored fleet
    /// already holds. [`MAX_FLEETS_PER_ACCOUNT`] is below the block's host count, so the
    /// walk always finds a free one.
    pub fn next_instance_address(&mut self) -> String {
        /// Host addresses in a /24 that this emulator hands out (`.1`-`.254`).
        const HOSTS: u32 = 254;
        const _: () = assert!(
            MAX_FLEETS_PER_ACCOUNT < HOSTS as usize,
            "the fleet quota must stay below the host count of the address block, or two \
             live fleets could be given the same address"
        );

        let mut candidate = String::new();
        for _ in 0..HOSTS {
            self.provisioned_instances = self.provisioned_instances.wrapping_add(1);
            candidate = format!("203.0.113.{}", self.provisioned_instances % HOSTS + 1);
            if !self
                .fleets
                .values()
                .any(|fleet| fleet.ip_address == candidate)
            {
                break;
            }
        }
        candidate
    }

    /// The tags assigned to the resource `resource_arn` names, or `None` when this store
    /// holds no taggable resource under that ARN.
    ///
    /// Tags live ON the record they label, so this is the one place that maps an ARN to
    /// them: the ARN's resource kind selects the collection and its resource id is that
    /// collection's key ("the resource ID matches the `<kind>Id` value"), and the record's
    /// own stored ARN is then compared WHOLE — so an ARN that differs from it in Region or
    /// account names no resource here. A kind this service does not tag (a game session,
    /// a player session) is `None` for the same reason: no record of it carries tags.
    pub fn tags_of(&self, resource_arn: &str) -> Option<&[Tag]> {
        let (kind, id) = arn_resource_parts(resource_arn)?;
        let tags = match kind {
            "fleet" => self
                .fleets
                .get(id)
                .filter(|fleet| fleet.fleet_arn == resource_arn)
                .map(|fleet| &fleet.tags),
            "alias" => self
                .aliases
                .get(id)
                .filter(|alias| alias.alias_arn == resource_arn)
                .map(|alias| &alias.tags),
            "build" => self
                .builds
                .get(id)
                .filter(|build| build.build_arn == resource_arn)
                .map(|build| &build.tags),
            "script" => self
                .scripts
                .get(id)
                .filter(|script| script.script_arn == resource_arn)
                .map(|script| &script.tags),
            "gamesessionqueue" => self
                .game_session_queues
                .get(id)
                .filter(|queue| queue.queue_arn == resource_arn)
                .map(|queue| &queue.tags),
            "matchmakingruleset" => self
                .matchmaking_rule_sets
                .get(id)
                .filter(|rule_set| rule_set.rule_set_arn == resource_arn)
                .map(|rule_set| &rule_set.tags),
            "matchmakingconfiguration" => self
                .matchmaking_configurations
                .get(id)
                .filter(|configuration| configuration.configuration_arn == resource_arn)
                .map(|configuration| &configuration.tags),
            "location" => self
                .custom_locations
                .get(id)
                .filter(|location| location.location_arn == resource_arn)
                .map(|location| &location.tags),
            _ => None,
        };
        tags.map(Vec::as_slice)
    }

    /// [`tags_of`](Self::tags_of) for a caller that WRITES the resource's tags, which
    /// `TagResource` and `UntagResource` do; it resolves the ARN by the same rule.
    pub fn tags_of_mut(&mut self, resource_arn: &str) -> Option<&mut Vec<Tag>> {
        let (kind, id) = arn_resource_parts(resource_arn)?;
        match kind {
            "fleet" => self
                .fleets
                .get_mut(id)
                .filter(|fleet| fleet.fleet_arn == resource_arn)
                .map(|fleet| &mut fleet.tags),
            "alias" => self
                .aliases
                .get_mut(id)
                .filter(|alias| alias.alias_arn == resource_arn)
                .map(|alias| &mut alias.tags),
            "build" => self
                .builds
                .get_mut(id)
                .filter(|build| build.build_arn == resource_arn)
                .map(|build| &mut build.tags),
            "script" => self
                .scripts
                .get_mut(id)
                .filter(|script| script.script_arn == resource_arn)
                .map(|script| &mut script.tags),
            "gamesessionqueue" => self
                .game_session_queues
                .get_mut(id)
                .filter(|queue| queue.queue_arn == resource_arn)
                .map(|queue| &mut queue.tags),
            "matchmakingruleset" => self
                .matchmaking_rule_sets
                .get_mut(id)
                .filter(|rule_set| rule_set.rule_set_arn == resource_arn)
                .map(|rule_set| &mut rule_set.tags),
            "matchmakingconfiguration" => self
                .matchmaking_configurations
                .get_mut(id)
                .filter(|configuration| configuration.configuration_arn == resource_arn)
                .map(|configuration| &mut configuration.tags),
            "location" => self
                .custom_locations
                .get_mut(id)
                .filter(|location| location.location_arn == resource_arn)
                .map(|location| &mut location.tags),
            _ => None,
        }
    }
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsGameliftGameliftState>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsGameliftGameliftState>)
}
