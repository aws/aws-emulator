// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-apigatewayv2-apigatewayv2 service.
//!
//! A single store, [`ComAmazonawsApigatewayv2Apigatewayv2State::apis`], keyed by
//! `ApiId`. An [`Api`] OWNS its child [`Route`]s, [`Integration`]s and [`Stage`]s, so
//! deleting an api cascades to its children in one `remove` and no second map can
//! drift out of step with it.
//!
//! The records here are the service's own, not the wire shapes: a record keeps a
//! value the api always has (its id, its name, its creation instant) as a plain
//! field, and `to_wire` projects it onto the modelled shape the handlers serialize.
//!
//! The pagination a collection reader serves — the page size a caller may ask for and
//! the opaque `NextToken` that resumes a walk — lives in [`paging`], the module that
//! holds those bounds, and the selection expressions several members carry live in
//! [`selection_expression`]. The statuses the service answers where they differ from the
//! model's `@http code` live in [`wire_status`]. All of them are re-exported here so a
//! handler reaches them the way it reaches everything else in this module.
//!
//! One fact a request names is NOT this service's to hold: the subnets of the account a
//! VPC link is placed in are Amazon EC2's resources, so they are resolved against the
//! service that owns them in [`ec2_subnets`], and how a certificate a custom domain name
//! presents was issued is Certificate Manager's, resolved in
//! [`acm_certificate_issuance`].

pub mod acm_certificate_issuance;
pub mod authorizer_cache;
pub mod cors;
pub mod cors_origin;
pub mod duplicate_identity;
pub mod domain_name_transport;
pub mod ec2_subnets;
pub mod integration_response_key;
pub mod invoke_transport;
pub mod jwt_issuer;
pub mod listener_registry;
pub mod model_template;
mod network;
pub mod paging;
pub mod route_request_parameters;
pub mod route_throttle;
pub mod selection_expression;
pub mod websocket_transport;
pub mod wire_status;

/// The statuses a mutating operation of this service answers where that is not its
/// modelled `@http code` — see [`wire_status`], which says which operations each governs.
pub use wire_status::{
    ACCEPTED_TEARDOWN_STATUS, FLUSHED_CACHE_OK_STATUS, REWRITTEN_RESOURCE_STATUS,
    WRITTEN_RESOURCE_OK_STATUS,
};

use std::collections::{BTreeMap, BTreeSet};
use std::time::{Duration, Instant, SystemTime};

use crate::ops::types as wire;

/// The alphabet API Gateway mints an `ApiId`/`RouteId` from: lowercase
/// alphanumerics. `Id` is an unconstrained string in the Smithy model
/// (`smithy_query(query="shape", shape="Id")`), so the SHAPE of a minted id is a
/// service convention, not a modelled constraint; its uniqueness within the store IS
/// an invariant, and [`mint_id`] guarantees it by construction.
const ID_ALPHABET: &[u8] = b"abcdefghijklmnopqrstuvwxyz0123456789";

/// How many characters an id of a resource the ACCOUNT holds carries: an api, a VPC
/// link, a custom domain name, a portal, a portal product, and the pages and mappings
/// nested in those. It is also the shortest the portal and portal-product members admit
/// (`__stringMin10Max30PatternAZ09`: len 10..30, pattern `^[a-z0-9]+$`), so an id of this
/// length satisfies that constraint by construction.
const ACCOUNT_ID_LEN: u32 = 10;

/// How many characters the id of a resource NESTED INSIDE AN API carries — an
/// authorizer, a model, a deployment, a route response, an integration response: a short
/// opaque token, which is the form API Gateway v2 answers for those resources.
const NESTED_ID_LEN: u32 = 6;

/// How many characters the id of a route or an integration carries: one more than
/// [`NESTED_ID_LEN`], the form API Gateway v2 answers for the two api children a route
/// target names (`integrations/{IntegrationId}`).
const ROUTE_ID_LEN: u32 = 7;

/// The scope of an id the account holds DIRECTLY rather than inside another resource.
/// There is one such collection of each kind per store, so it needs no discriminator
/// beyond its [`IdKind`]; see [`mint_id`].
const ACCOUNT_SCOPE: &str = "";

/// Which KIND of resource an id is minted for.
///
/// Every collection counts its own ids from a seed of its own, so an api's first route
/// and its first integration are minted from the SAME seed. An identifier has to denote
/// one resource, though — a client that reads an `IntegrationId` out of a refusal must
/// not get a string that is also a `RouteId` — so the kind is folded into the id's
/// pre-image (see [`mint_id`]) and ids of two different kinds are never equal, whatever
/// their seeds. The kind also says how LONG an id is ([`IdKind::id_len`]).
#[derive(Debug, Clone, Copy)]
enum IdKind {
    Api,
    Portal,
    PortalProduct,
    VpcLink,
    DomainName,
    Route,
    Integration,
    Model,
    Deployment,
    Authorizer,
    RouteResponse,
    IntegrationResponse,
    ProductPage,
    ProductRestEndpointPage,
    ApiMapping,
    RoutingRule,
}

impl IdKind {
    /// Every kind there is, so [`ID_KIND_STRIDE`] can be checked to cover them all.
    const ALL: &'static [IdKind] = &[
        IdKind::Api,
        IdKind::Portal,
        IdKind::PortalProduct,
        IdKind::VpcLink,
        IdKind::DomainName,
        IdKind::Route,
        IdKind::Integration,
        IdKind::Model,
        IdKind::Deployment,
        IdKind::Authorizer,
        IdKind::RouteResponse,
        IdKind::IntegrationResponse,
        IdKind::ProductPage,
        IdKind::ProductRestEndpointPage,
        IdKind::ApiMapping,
        IdKind::RoutingRule,
    ];

    /// The kind declared LAST, whose ordinal is therefore the largest. Named so the
    /// assertions below can check that [`Self::ALL`] lists every variant (its length is
    /// the last ordinal plus one) and that the largest ordinal still has a residue class
    /// of its own: a variant added to the enum and forgotten in `ALL`, or one added past
    /// [`ID_KIND_STRIDE`], fails the build rather than minting another kind's id.
    const LAST: IdKind = IdKind::RoutingRule;

    /// This kind's residue class in the id space, below [`ID_KIND_STRIDE`].
    const fn ordinal(self) -> u128 {
        self as u128
    }

    /// How many characters an id of this kind carries.
    ///
    /// `Id` is unconstrained in the model, so the LENGTH of a minted id is this service's
    /// convention per kind of resource: [`ACCOUNT_ID_LEN`] for a resource the account
    /// holds and for the pages and mappings nested in one, and the short opaque form —
    /// [`NESTED_ID_LEN`], [`ROUTE_ID_LEN`] for a route and an integration — for a
    /// resource nested inside an api, which is the form API Gateway v2 answers for those.
    const fn id_len(self) -> u32 {
        match self {
            IdKind::Api
            | IdKind::Portal
            | IdKind::PortalProduct
            | IdKind::VpcLink
            | IdKind::DomainName
            | IdKind::ProductPage
            | IdKind::ProductRestEndpointPage
            | IdKind::ApiMapping
            | IdKind::RoutingRule => ACCOUNT_ID_LEN,
            IdKind::Route | IdKind::Integration => ROUTE_ID_LEN,
            IdKind::Model
            | IdKind::Deployment
            | IdKind::Authorizer
            | IdKind::RouteResponse
            | IdKind::IntegrationResponse => NESTED_ID_LEN,
        }
    }
}

/// How many residue classes of the id space are reserved for [`IdKind`]s: one per kind,
/// so `seed * ID_KIND_STRIDE + kind` is injective in `(kind, seed)`.
const ID_KIND_STRIDE: u128 = 16;

/// Every kind must have a residue class of its own, or two kinds would mint one id: the
/// kinds are counted AND the largest ordinal is checked, so a variant added to
/// [`IdKind`] without a seat in [`ID_KIND_STRIDE`] fails the build.
const _: () = assert!(IdKind::ALL.len() as u128 <= ID_KIND_STRIDE);
const _: () = assert!(IdKind::LAST.ordinal() + 1 == IdKind::ALL.len() as u128);
const _: () = assert!(IdKind::LAST.ordinal() < ID_KIND_STRIDE);

/// The `RouteSelectionExpression` an HTTP API gets when the request names none.
///
/// From the `CreateApi$RouteSelectionExpression` doc: "For HTTP APIs, the
/// routeSelectionExpression must be ${request.method} ${request.path}. If not
/// provided, this will be the default for HTTP APIs."
pub const HTTP_ROUTE_SELECTION_EXPRESSION: &str = "$request.method $request.path";

/// The `ApiKeySelectionExpression` an api gets when the request names none; the
/// default AWS documents for API key selection expressions.
pub const DEFAULT_API_KEY_SELECTION_EXPRESSION: &str = "$request.header.x-api-key";

/// The `IpAddressType` an api gets when the request names none. One of
/// `IP_ADDRESS_TYPE_VALUES` (`dualstack | ipv4`); AWS documents IPv4-only as the
/// default for a new API.
pub const DEFAULT_IP_ADDRESS_TYPE: &str = "ipv4";

/// The `ProtocolType` value that selects a WebSocket API, from
/// `PROTOCOL_TYPE_VALUES` (`HTTP | WEBSOCKET`) — the protocol whose two-way
/// communication the route-response resources belong to, and the one whose
/// `ApiEndpoint` carries the WebSocket scheme (see [`execute_api_scheme`]).
pub const WEBSOCKET_PROTOCOL_TYPE: &str = "WEBSOCKET";

/// The `AuthorizationType` a route gets when the request names none. One of
/// `AUTHORIZATION_TYPE_VALUES`; `NONE` is the value the `Route$AuthorizationType`
/// doc gives for open access.
pub const DEFAULT_AUTHORIZATION_TYPE: &str = "NONE";

/// The region an unsigned request's resources are reported in — the region every
/// emulator defaults to (see `emulator_server::emulator_arn`).
pub const DEFAULT_REGION: &str = "us-east-1";

/// The prefix a `Route$Target` carries when the route is wired to an integration of
/// its own api: API Gateway addresses an integration target as
/// `integrations/{IntegrationId}` — the form `CreateRoute$Target` takes and the one
/// every route reader reports back. [`Route::target_integration_id`] is what reads an
/// `IntegrationId` out of a target, so no handler has to know the spelling.
pub const INTEGRATION_TARGET_PREFIX: &str = "integrations/";

/// The most apis one account may hold, after which CreateApi is refused.
///
/// AWS's published API Gateway quota is 600 HTTP/WebSocket APIs per account per
/// Region, so this is the faithful bound AND the one that stops a loop of CreateApi
/// calls growing this store without end.
pub const MAX_APIS: usize = 600;

/// The length window the shape an api's `Name` targets documents:
/// `CreateApi$Name` / `UpdateApi$Name` / `ImportApi`'s `title` all target
/// `StringWithLengthBetween1And128`, whose own documentation is "A string with a length
/// between [1-128]" (`smithy_query(query="shape",
/// shape="StringWithLengthBetween1And128")`).
pub const API_NAME_LEN: (usize, usize) = (1, 128);

/// The length window an api's `Description` falls in: `StringWithLengthBetween0And1024`,
/// "A string with a length between [0-1024]".
pub const API_DESCRIPTION_LEN: (usize, usize) = (0, 1024);

/// The length window an api's `Version` falls in: `StringWithLengthBetween1And64`,
/// "A string with a length between [1-64]".
pub const API_VERSION_LEN: (usize, usize) = (1, 64);

/// The length window an `IntegrationUri` falls in: `UriWithLengthBetween1And2048`, "a
/// string representation of a URI with a length between [1-2048]"
/// (`smithy_query(query="shape", shape="UriWithLengthBetween1And2048")`). The shape carries
/// no `@length` trait, so the window is a handler's to enforce — and it is also the bound
/// the data plane resolves a stage variable into that URI under
/// (`crate::service::data_plane`), so the stored template and the upstream it resolves to
/// are held to one length.
pub const INTEGRATION_URI_LEN: (usize, usize) = (1, 2048);

/// Refuse a string member whose length falls outside the window the shape it targets
/// documents.
///
/// None of this model's `StringWithLengthBetween…` shapes carries an `@length` trait, so
/// the boundary validator does not enforce the window its name and documentation state
/// and the operation that writes the member does — otherwise a value no AWS API Gateway
/// would store is stored here and reported back by every later reader. This is the one
/// implementation of that check, so no two operations can state the same bound
/// differently.
///
/// `Ok(())`, or the message for the operation's `BadRequestException`: it names the
/// member, the window and the length the request carried.
pub fn validate_member_length(
    member: &str,
    value: &str,
    (min, max): (usize, usize),
) -> Result<(), String> {
    // Characters, not bytes: the window the shape documents is a string length, so a
    // multi-byte character counts once.
    let length = value.chars().count();
    if length < min || length > max {
        return Err(format!(
            "Invalid value for {member}: must have length between {min} and {max}, got {length}."
        ));
    }
    Ok(())
}

/// The most routes one api may hold, after which CreateRoute is refused. AWS's
/// published quota is 300 routes per API.
pub const MAX_ROUTES_PER_API: usize = 300;

/// The most integrations one api may hold, after which CreateIntegration is refused.
/// AWS's published API Gateway quota is 300 integrations per API.
pub const MAX_INTEGRATIONS_PER_API: usize = 300;

/// The most route responses one route may hold, after which CreateRouteResponse is
/// refused.
///
/// AWS publishes no separate route-responses-per-route quota, so this is a CHOSEN
/// bound, set to the per-parent quota this service already enforces for routes
/// ([`MAX_ROUTES_PER_API`]): what it buys is that no loop of CreateRouteResponse calls
/// can grow this store without end, and that the per-route duplicate-key scan stays
/// bounded. `TooManyRequestsException` ("A limit has been exceeded") is the modelled
/// error the operation refuses with.
pub const MAX_ROUTE_RESPONSES_PER_ROUTE: usize = 300;

/// The most integration responses one integration may hold, after which
/// CreateIntegrationResponse is refused.
///
/// AWS publishes no separate integration-responses-per-integration quota, so this is a
/// CHOSEN bound, set to the per-parent quota this service already enforces for an api's
/// integrations ([`MAX_INTEGRATIONS_PER_API`]): what it buys is that no loop of
/// CreateIntegrationResponse calls can grow this store without end, and that the
/// per-integration duplicate-key scan stays bounded. `TooManyRequestsException` ("A
/// limit has been exceeded") is the modelled error the operation refuses with.
pub const MAX_INTEGRATION_RESPONSES_PER_INTEGRATION: usize = 300;

/// The most models one api may hold, after which CreateModel is refused.
///
/// An EMULATOR growth bound, not a figure AWS publishes: API Gateway publishes per-api
/// quotas for the other child resources of an api (300 routes, 300 integrations) and
/// this takes the same bound for models, so no loop of CreateModel calls can grow this
/// store without end. Cite the routes/integrations quotas for those; do not cite this
/// as a published models quota.
pub const MAX_MODELS_PER_API: usize = 300;

/// The most custom domain names one account may hold in one Region, after which
/// CreateDomainName is refused.
///
/// API Gateway publishes a quota of 120 custom domain names per account per Region, so
/// this is the faithful bound AND what stops a loop of CreateDomainName calls growing
/// this store without end. `TooManyRequestsException` ("A limit has been exceeded") is
/// the modelled error the operation refuses with.
pub const MAX_DOMAIN_NAMES: usize = 120;

/// The longest custom domain name this service accepts.
///
/// `DomainName$DomainName` targets `StringWithLengthBetween1And512`, whose own
/// documentation is "A string with a length between [1-512]"
/// (`smithy_query(query="shape", shape="StringWithLengthBetween1And512")`). The shape
/// carries no `@length` trait, so the bound is a handler's to enforce rather than the
/// boundary validator's.
pub const DOMAIN_NAME_MAX_LENGTH: usize = 512;

/// The service signing name this emulator's ARNs are built under — the `apigateway`
/// signing name and ARN namespace this model shares with the API Gateway control plane
/// (`smithy_query(query="service_info")`).
pub const ARN_SERVICE: &str = "apigateway";

/// The `RoutingMode` a domain name gets when the request names none, out of
/// `ROUTING_MODE_VALUES` (`API_MAPPING_ONLY | ROUTING_RULE_ONLY |
/// ROUTING_RULE_THEN_API_MAPPING`).
///
/// A CHOSEN default, not a figure AWS publishes: the model documents the member only as
/// "The routing mode", so this takes the one value that needs no routing rule to resolve
/// a request — a domain name reached through its `ApiMapping`s alone, which is what a
/// domain name created without any routing rule can actually serve.
pub const DEFAULT_ROUTING_MODE: &str = "API_MAPPING_ONLY";

/// The `EndpointType` a domain name configuration gets when the request names none, out
/// of `ENDPOINT_TYPE_VALUES` (`EDGE | REGIONAL`).
///
/// Every operation of this model is bound under `/v2/…` and every api it creates is
/// reported on a REGIONAL `{api-id}.execute-api.{region}.amazonaws.com` host
/// (`CreateApiResponse$ApiEndpoint`, see [`execute_api_endpoint`]), so `REGIONAL` is the
/// endpoint type a domain name fronting one of them carries; `EDGE` is the
/// edge-optimized form the member's certificate wording belongs to.
pub const DEFAULT_ENDPOINT_TYPE: &str = "REGIONAL";

/// The `SecurityPolicy` every custom domain name of this api type is served under,
/// whatever a request names, out of `SECURITY_POLICY_VALUES` (`TLS_1_0 | TLS_1_2`).
///
/// `DomainNameConfiguration$SecurityPolicy` models both values, but API Gateway v2
/// enforces the later TLS version as the FLOOR for a domain name: a configuration naming
/// the earlier one is refused rather than stored (see
/// [`unsupported_security_policy_message`]), so the catalog's lower value can never be the
/// policy one of this service's endpoints is reached over.
pub const ENFORCED_SECURITY_POLICY: &str = "TLS_1_2";

/// The `SecurityPolicy` a domain name configuration gets when the request names none:
/// the one policy the service serves a domain name under, so an entry that names it and
/// one that leaves it out describe the same endpoint.
pub const DEFAULT_SECURITY_POLICY: &str = ENFORCED_SECURITY_POLICY;

/// The sentence a `DomainNameConfigurations` entry naming a `SecurityPolicy` other than
/// [`ENFORCED_SECURITY_POLICY`] is refused with.
///
/// API Gateway states this refusal in terms of the POLICY that was asked for and the api
/// type that does not serve it, naming neither the member's path nor the domain name; the
/// policy is the one variable part and comes from the request.
pub fn unsupported_security_policy_message(security_policy: &str) -> String {
    format!("{security_policy} is not supported for APIGatewayV2 domainName.")
}

/// The `ApiMappingSelectionExpression` every domain name reports.
///
/// An `ApiMapping` selects an api by the base path of the incoming request
/// (`CreateApiMapping$ApiMappingKey`), so the expression that extracts the value the
/// mapping is matched on is the request's base path. It is service-computed: no member
/// of any request sets it.
pub const API_MAPPING_SELECTION_EXPRESSION: &str = "$request.basepath";

/// The `DomainNameStatus` of a domain name whose endpoint migration has finished, out of
/// `DOMAIN_NAME_STATUS_VALUES`. From `DomainNameConfiguration$DomainNameStatus`: "If it
/// is AVAILABLE, the domain can be updated."
pub const DOMAIN_NAME_STATUS_AVAILABLE: &str = "AVAILABLE";

/// The `DomainNameStatus` of a domain name whose endpoint migration is still in flight,
/// out of `DOMAIN_NAME_STATUS_VALUES`. From
/// `DomainNameConfiguration$DomainNameStatus`: "If the status is UPDATING, the domain
/// cannot be modified further until the existing operation is complete."
pub const DOMAIN_NAME_STATUS_UPDATING: &str = "UPDATING";

/// How long a domain name's endpoint migration takes before the clock owes it
/// `AVAILABLE` again.
///
/// A CHOSEN span, not one AWS documents: AWS takes as long as propagating a certificate
/// to its endpoints takes. What matters is that it is never zero — a caller that updates
/// a domain name's `DomainNameConfigurations` and immediately reads it back still sees
/// the `UPDATING` the model documents, and the refusal that status carries — and that
/// nothing sleeps for it: the due instant is stored and the status is derived from the
/// clock on every read.
pub const DOMAIN_NAME_MIGRATION: Duration = Duration::from_millis(500);

/// How long the teardown of a deleted domain name takes before the clock owes the
/// release of the certificates its endpoints were presenting.
///
/// `DeleteDomainName` ACCEPTS the removal rather than completing it — the recorded API
/// answers `202 Accepted` — so the endpoints the domain name was served on are torn down
/// after the call returns, and a certificate one of them presents is still in use until
/// that teardown is done. A CHOSEN span, not one AWS documents: what matters is that it
/// is never zero, so a client that deletes a domain name and immediately asks ACM to
/// delete the certificate it was built on still gets the refusal the real service answers
/// there, and that nothing sleeps for it — the instant is stored and the release is
/// applied by whichever call next reads the directory.
pub const DOMAIN_NAME_DELETION: Duration = Duration::from_secs(3);

/// The refusal a request carries when its `DomainNameConfigurations` names no endpoint:
/// the message of the `BadRequestException` both domain-name writers answer with (see
/// [`resolve_domain_name_endpoints`]). It names the member's constraint — one
/// configuration is expected — and quotes nothing from the request.
pub const EMPTY_DOMAIN_NAME_CONFIGURATIONS: &str =
    "Invalid input. Expected one domain name configuration";

/// The punctuation an `ApiMappingKey` segment may carry, beside letters and digits.
///
/// `SelectionKey` carries no `@pattern` trait, so the character class is the service's
/// rule rather than the boundary validator's: API Gateway accepts a base path built from
/// letters, numbers and these characters, and refuses anything else with
/// [`API_MAPPING_KEY_ILLEGAL_CHARACTER`].
pub const API_MAPPING_KEY_PUNCTUATION: &str = "$-_.+!*'(),";

/// The refusal an `ApiMappingKey` carrying a character outside the class
/// [`API_MAPPING_KEY_PUNCTUATION`] describes takes: fixed text for the whole
/// illegal-character class, naming nothing from the request.
pub const API_MAPPING_KEY_ILLEGAL_CHARACTER: &str =
    "An API mapping key may contain only letters, numbers and one of $-_.+!*'(), characters.";

/// The refusal an `ApiMappingKey` whose SHAPE as a path is wrong takes: a base path is
/// the part of a request path after the host, written without the leading separator and
/// with exactly one separator between segments.
pub const API_MAPPING_KEY_MALFORMED_PATH: &str =
    "API mapping key should not start with a '/' or have consecutive '/'s.";

/// The refusal a `CreateApiMapping`/`UpdateApiMapping` request whose `Stage` does not
/// name a stage of the mapped api takes. Fixed text: the stage the caller asked for is
/// already the value it sent, and API Gateway reports the kind of identifier that was
/// not usable rather than echoing it back.
pub const INVALID_STAGE_IDENTIFIER: &str = "Invalid stage identifier specified";

/// The refusal an operation addressing a domain name the account does not hold takes:
/// the message of the modelled `NotFoundException`. Fixed text, naming no value from the
/// request — the domain name the caller asked for is already the resource it addressed.
pub const INVALID_DOMAIN_NAME_IDENTIFIER: &str = "Invalid domain name identifier specified";

/// The sentence an operation addressing an api mapping its domain name does not hold
/// refuses with: the message of the modelled `NotFoundException`.
///
/// API Gateway reports an api mapping that resolves to nothing by naming the resource
/// TYPE and the `ApiMappingId` the request addressed it by — the form
/// [`ROUTING_RULE_NOT_FOUND`] also takes, rather than the `Invalid {resource} identifier
/// specified` sentence the api-level resources use. The id in the sentence is the one the
/// request sent. Shared by every reader and writer of a single mapping, so the same
/// identifier is never reported two ways.
pub fn api_mapping_not_found_message(api_mapping_id: &str) -> String {
    format!("Unable to find ApiMapping with ID {api_mapping_id}")
}

/// The most API mappings one custom domain name may hold, after which
/// `CreateApiMapping` is refused.
///
/// An EMULATOR growth bound, not a figure AWS publishes: API Gateway documents no quota
/// for API mappings per domain name, so this takes the same shape as the other
/// per-parent bounds here and is what stops a loop of CreateApiMapping calls growing
/// this store without end. `TooManyRequestsException` ("A limit has been exceeded") is
/// the modelled error the operation refuses with.
pub const MAX_API_MAPPINGS_PER_DOMAIN: usize = 200;

/// The sentence a writer of an api mapping refuses a base path the named domain name
/// already maps with: the message of the modelled `ConflictException` ("The requested
/// operation would cause a conflict with the current state of a service resource
/// associated with the request").
///
/// Fixed text for the whole class, interpolating nothing from the request: the domain name
/// and the base path the caller asked for are already the values it sent. Held here once,
/// so the mint and the patch of a mapping cannot report the same conflict two ways.
pub const API_MAPPING_KEY_CONFLICT: &str = "ApiMapping key already exists for this domain name";

/// Refuse an `ApiMappingKey` that is not a base path API Gateway can route on.
///
/// Two rules, in the order API Gateway applies them, and each is exactly what its refusal
/// states: the key's SHAPE as a path — it does not start with a separator and carries no
/// consecutive separators ([`API_MAPPING_KEY_MALFORMED_PATH`]) — and then the character
/// class each segment is drawn from ([`API_MAPPING_KEY_ILLEGAL_CHARACTER`]). Neither rule
/// speaks of a TRAILING separator, so `a/` is accepted. A key the request omitted, or
/// sent empty, is the root mapping — the base path of the domain itself — so it has no
/// segment to check and is accepted.
///
/// `Err` carries the message for the operation's `BadRequestException`.
pub fn validate_api_mapping_key(key: &str) -> Result<(), String> {
    if key.is_empty() {
        return Ok(());
    }
    if key.starts_with('/') || key.contains("//") {
        return Err(API_MAPPING_KEY_MALFORMED_PATH.to_string());
    }
    let legal = |character: char| {
        character.is_ascii_alphanumeric() || API_MAPPING_KEY_PUNCTUATION.contains(character)
    };
    if key.split('/').any(|segment| !segment.chars().all(legal)) {
        return Err(API_MAPPING_KEY_ILLEGAL_CHARACTER.to_string());
    }
    Ok(())
}

/// The refusal an operation addressing a routing rule its domain name does not hold
/// takes: the message of the modelled `NotFoundException`. Fixed text, naming no value
/// from the request — the rule the caller asked for is already the resource it
/// addressed. Shared by every reader and writer of a single routing rule, so the same
/// identifier is never reported two ways.
pub const ROUTING_RULE_NOT_FOUND: &str = "Unable to find RoutingRule.";

/// The most routing rules one custom domain name may hold, after which
/// `CreateRoutingRule` is refused.
///
/// An EMULATOR growth bound, not a figure AWS publishes: this takes the same shape as
/// [`MAX_API_MAPPINGS_PER_DOMAIN`], the other per-domain-name collection, and is what
/// stops a loop of `CreateRoutingRule` calls growing this store without end.
/// `TooManyRequestsException` ("A limit has been exceeded") is the modelled error the
/// operation refuses with.
pub const MAX_ROUTING_RULES_PER_DOMAIN: usize = 200;

/// The most `matchHeaders` conditions one routing rule may carry, from
/// `CreateRoutingRule$Conditions`: "Conditions can contain up to two matchHeaders
/// conditions and one matchBasePaths conditions."
pub const MAX_MATCH_HEADERS_CONDITIONS: usize = 2;

/// The most `matchBasePaths` conditions one routing rule may carry — the same documented
/// sentence as [`MAX_MATCH_HEADERS_CONDITIONS`].
pub const MAX_MATCH_BASE_PATHS_CONDITIONS: usize = 1;

/// The refusal a `Conditions` entry that does not name EXACTLY ONE match clause takes —
/// one naming neither, which selects no request, and one naming both, which says two
/// things where the structure offers a choice.
///
/// Fixed text: it names the two alternatives in the wire spelling
/// (`RoutingRuleCondition$MatchBasePaths` carries `jsonName` `matchBasePaths` and
/// `$MatchHeaders` carries `matchHeaders`) and carries no index and no value from the
/// request.
pub const ROUTING_RULE_CONDITION_ALTERNATIVES: &str = "Invalid conditions for RoutingRule. \
     Each condition can only be either matchBasePaths or matchHeaders.";

/// The refusal a routing rule whose `Conditions` carry more of one kind than the
/// documented sentence allows takes: the constraint itself, naming nothing from the
/// request.
pub const ROUTING_RULE_CONDITION_LIMITS: &str = "Conditions can contain up to two \
     matchHeaders conditions and one matchBasePaths conditions.";

/// The refusal a routing rule naming no action takes. `Actions` is a required member and
/// a rule with no action would do nothing to a request it matched, so a list with no
/// entry is incomplete input — the same form as [`EMPTY_DOMAIN_NAME_CONFIGURATIONS`].
pub const EMPTY_ROUTING_RULE_ACTIONS: &str = "Invalid input. Expected one routing rule action";

/// The refusal a routing rule naming no condition takes. `Conditions` is a required
/// member and a rule with no condition selects no request, so a list with no entry is
/// incomplete input.
pub const EMPTY_ROUTING_RULE_CONDITIONS: &str =
    "Invalid input. Expected one routing rule condition";

/// Refuse a routing rule whose `Actions` or `Conditions` do not describe a rule API
/// Gateway could evaluate.
///
/// The rules, each from what the model documents for the two required members: a rule
/// names at least one action (`Actions`, "the only supported action is invokeApi") and at
/// least one condition (`Conditions`); every condition names EXACTLY ONE of the two
/// alternatives its structure offers, since a condition naming neither says nothing and
/// one naming both says two things; a condition's `AnyOf` names at least one value to
/// match on; and the whole list carries at most [`MAX_MATCH_HEADERS_CONDITIONS`]
/// `matchHeaders` and [`MAX_MATCH_BASE_PATHS_CONDITIONS`] `matchBasePaths` conditions.
///
/// Shared by every writer of the resource, so `CreateRoutingRule` and `PutRoutingRule`
/// cannot drift. `Err` carries the message for the operation's `BadRequestException`.
pub fn validate_routing_rule(
    actions: &[wire::RoutingRuleAction],
    conditions: &[wire::RoutingRuleCondition],
) -> Result<(), String> {
    if actions.is_empty() {
        return Err(EMPTY_ROUTING_RULE_ACTIONS.to_string());
    }
    if conditions.is_empty() {
        return Err(EMPTY_ROUTING_RULE_CONDITIONS.to_string());
    }
    let mut match_headers = 0usize;
    let mut match_base_paths = 0usize;
    for (index, condition) in conditions.iter().enumerate() {
        let member = format!("Conditions[{index}]");
        // Exactly one of the two alternatives, refused with the service's own fixed
        // sentence for this member rather than with the generic exclusive-choice wording
        // ([`validate_exclusive_choice`]) the other members of this model take.
        let named = usize::from(condition.match_base_paths.is_some())
            + usize::from(condition.match_headers.is_some());
        if named != 1 {
            return Err(ROUTING_RULE_CONDITION_ALTERNATIVES.to_string());
        }
        if let Some(base_paths) = &condition.match_base_paths {
            match_base_paths += 1;
            if base_paths.any_of.is_empty() {
                return Err(empty_any_of(&member, "MatchBasePaths"));
            }
        }
        if let Some(headers) = &condition.match_headers {
            match_headers += 1;
            if headers.any_of.is_empty() {
                return Err(empty_any_of(&member, "MatchHeaders"));
            }
        }
    }
    if match_headers > MAX_MATCH_HEADERS_CONDITIONS
        || match_base_paths > MAX_MATCH_BASE_PATHS_CONDITIONS
    {
        return Err(ROUTING_RULE_CONDITION_LIMITS.to_string());
    }
    Ok(())
}

/// The refusal a condition whose `AnyOf` carries no value takes: the member is what the
/// condition is matched on, so a condition with nothing in it matches nothing.
fn empty_any_of(member: &str, alternative: &str) -> String {
    format!("{member}.{alternative}.AnyOf must name at least one value.")
}

/// The published bounds of `RoutingRulePriority` (`smithy_query(query="shape",
/// shape="RoutingRulePriority")`: `range` 1..1000000), the shape every routing-rule
/// writer's `Priority` targets. The boundary validator refuses a value outside them; the
/// bounds are named here so the sentence that explains the refusal
/// ([`routing_rule_priority_range_message`]) cannot drift from the constraint.
pub const MIN_ROUTING_RULE_PRIORITY: i32 = 1;

/// The upper bound of the range [`MIN_ROUTING_RULE_PRIORITY`] opens.
pub const MAX_ROUTING_RULE_PRIORITY: i32 = 1_000_000;

/// The sentence API Gateway refuses a `Priority` outside its published range with.
///
/// The service states this member's refusal in the member's OWN terms — the whole range
/// it has to fall in, not the single bound that was crossed — rather than in the generic
/// "value at path failed to satisfy constraint" form, and it writes the bound with the
/// thousands separators a number of that size is written with. Built from the modelled
/// bounds, so it cannot drift from the constraint it explains.
pub fn routing_rule_priority_range_message() -> String {
    format!(
        "Priority must be between {} and {}.",
        grouped(MIN_ROUTING_RULE_PRIORITY),
        grouped(MAX_ROUTING_RULE_PRIORITY)
    )
}

/// `value` with its digits grouped in threes by commas, the way a bound of that size is
/// written in a refusal.
fn grouped(value: i32) -> String {
    let digits = value.unsigned_abs().to_string();
    let mut out = String::with_capacity(digits.len() + digits.len() / 3 + 1);
    if value < 0 {
        out.push('-');
    }
    for (position, digit) in digits.char_indices() {
        if position > 0 && (digits.len() - position).is_multiple_of(3) {
            out.push(',');
        }
        out.push(digit);
    }
    out
}

/// The ARN of one `RoutingRule` of the custom domain name `domain_name`, in `region`.
///
/// Built from the resource path the operations address the rule at
/// (`/v2/domainnames/{DomainName}/routingrules/{RoutingRuleId}`) under the same account
/// and namespace as [`domain_name_arn`], the resource that owns it.
pub fn routing_rule_arn(domain_name: &str, routing_rule_id: &str, region: &str) -> String {
    ::engine::emulator_arn(
        ARN_SERVICE,
        region,
        &format!("/domainnames/{domain_name}/routingrules/{routing_rule_id}"),
    )
}
/// The most authorizers one api may hold, after which CreateAuthorizer is refused.
///
/// API Gateway publishes a quota of 10 authorizers per API, so this is the faithful
/// bound AND what stops a loop of CreateAuthorizer calls growing this store without
/// end — every call mints a new authorizer, so nothing else bounds it.
pub const MAX_AUTHORIZERS_PER_API: usize = 10;

/// The most `IdentitySource` entries one authorizer may carry, after which
/// CreateAuthorizer is refused.
///
/// An EMULATOR growth bound, not a figure AWS publishes: `IdentitySourceList` carries no
/// modelled length constraint, so nothing otherwise bounds the list one request stores —
/// and every request a WebSocket api's authorization step serves then reads the whole of
/// it (`crate::service`'s connection-request authorization), which is what makes an
/// unbounded list per-request work rather than only stored bytes. The figure is set far
/// above the one to three identity sources a real authorizer names, so what it refuses is
/// an absurd list rather than a configuration a caller would write. Do not cite it as a
/// published quota.
pub const MAX_IDENTITY_SOURCES_PER_AUTHORIZER: usize = 100;

/// The sentence every writer of an authorizer refuses a list past
/// [`MAX_IDENTITY_SOURCES_PER_AUTHORIZER`] with, stated once here so the create and the
/// update cannot drift apart about the bound they share.
pub fn identity_source_limit_message() -> String {
    format!(
        "The maximum number of identity sources ({MAX_IDENTITY_SOURCES_PER_AUTHORIZER}) for \
         an authorizer has been reached."
    )
}

/// The `AuthorizerType` value that selects a JWT authorizer, from the generated
/// `AUTHORIZER_TYPE_VALUES` catalog (`JWT | REQUEST`); the integration test
/// `the_authorizer_type_names_are_the_modelled_catalog` pins these two names against
/// that catalog so the spelling here cannot drift from the model. From
/// `CreateAuthorizer$AuthorizerType`: "Specify JWT to use JSON Web Tokens (supported
/// only for HTTP APIs)."
pub const AUTHORIZER_TYPE_JWT: &str = "JWT";

/// The `AuthorizerType` value that selects a Lambda authorizer taking the incoming
/// request, from the generated `AUTHORIZER_TYPE_VALUES` catalog (`JWT | REQUEST`). From
/// `CreateAuthorizer$AuthorizerType`: "Specify REQUEST for a Lambda function using
/// incoming request parameters."
pub const AUTHORIZER_TYPE_REQUEST: &str = "REQUEST";

/// The `AuthorizerPayloadFormatVersion` values the service accepts.
///
/// `StringWithLengthBetween1And64` carries no pattern, so the catalog is the member's
/// own documentation rather than a modelled constraint
/// (`CreateAuthorizer$AuthorizerPayloadFormatVersion`: "Supported values are 1.0 and
/// 2.0").
pub const AUTHORIZER_PAYLOAD_FORMAT_VERSIONS: &[&str] = &["1.0", "2.0"];

/// The `AuthorizationType` values that hand a route's authorization decision to an
/// `Authorizer` resource, out of the generated `AUTHORIZATION_TYPE_VALUES` catalog
/// (`AWS_IAM | CUSTOM | JWT | NONE`).
///
/// From `Route$AuthorizationType`: "JWT for using JSON Web Tokens" and "CUSTOM for
/// using a Lambda authorizer" — both name a resource, and `Route$AuthorizerId` is "the
/// identifier of the Authorizer resource to be associated with this route". The other
/// two decide without one: `NONE` is "open access" and `AWS_IAM` uses "AWS IAM
/// permissions".
pub const AUTHORIZER_BACKED_AUTHORIZATION_TYPES: &[&str] = &["CUSTOM", "JWT"];

/// The most deployments one api may hold, after which CreateDeployment is refused.
///
/// API Gateway publishes a quota of 500 deployments per API, so this is the faithful
/// bound AND what stops a loop of CreateDeployment calls growing this store without
/// end — every call mints a new deployment, so nothing else bounds it.
pub const MAX_DEPLOYMENTS_PER_API: usize = 500;

/// The `DeploymentStatus` a deployment carries once the service has released it, from
/// `DEPLOYMENT_STATUS_VALUES` (`DEPLOYED | FAILED | PENDING`). Deploying an api is
/// synchronous here — the deployment captures the api's current routes and
/// integrations, which this store already holds — so a minted deployment is reported
/// released, the terminal status, and never moves again.
pub const DEPLOYMENT_STATUS_DEPLOYED: &str = "DEPLOYED";

/// The `ProtocolType` value that selects an HTTP API, from `PROTOCOL_TYPE_VALUES`
/// (`smithy_query(query="shape", shape="ProtocolType")`: `HTTP | WEBSOCKET`).
pub const HTTP_PROTOCOL_TYPE: &str = "HTTP";

/// The `ConnectionType` an integration gets when the request names none. From
/// `Integration$ConnectionType`: "Specify INTERNET for connections through the public
/// routable internet … The default value is INTERNET."
pub const DEFAULT_CONNECTION_TYPE: &str = "INTERNET";

/// The `IntegrationType` values only a WebSocket api supports, drawn from the member's
/// own documentation rather than from a hand-written allowlist.
///
/// `CreateIntegration$IntegrationType` documents each value of
/// `INTEGRATION_TYPE_VALUES` (`AWS | AWS_PROXY | HTTP | HTTP_PROXY | MOCK`) and marks
/// exactly three "Supported only for WebSocket APIs": "AWS: for integrating the route
/// or method request with an AWS service action … Supported only for WebSocket APIs",
/// "HTTP: for integrating the route or method request with an HTTP endpoint … Supported
/// only for WebSocket APIs" and "MOCK: for integrating the route or method request with
/// API Gateway as a 'loopback' endpoint without invoking any backend. Supported only for
/// WebSocket APIs". The two proxy values carry no such clause, so they are what is left
/// for an HTTP api.
pub const WEBSOCKET_ONLY_INTEGRATION_TYPES: &[&str] = &["AWS", "HTTP", "MOCK"];

/// The `IntegrationType` of a proxy integration to a Lambda function or another AWS
/// service action, from `INTEGRATION_TYPE_VALUES`: "AWS_PROXY: for integrating the route
/// or method request with a Lambda function or other AWS service action. This
/// integration is also referred to as a Lambda proxy integration"
/// (`CreateIntegration$IntegrationType`).
pub const AWS_PROXY_INTEGRATION_TYPE: &str = "AWS_PROXY";

/// The `IntegrationType` of a custom integration with an AWS service action, from
/// `INTEGRATION_TYPE_VALUES`: "AWS: for integrating the route or method request with an
/// AWS service action, including the Lambda function-invoking action"
/// (`CreateIntegration$IntegrationType`).
pub const AWS_INTEGRATION_TYPE: &str = "AWS";

/// The `IntegrationType`s whose `IntegrationUri` names an AWS RESOURCE, so the URI is an
/// ARN rather than a URL.
///
/// Both values integrate "with an AWS service action"
/// (`CreateIntegration$IntegrationType`, for `AWS` and for `AWS_PROXY`), and an AWS
/// service action is addressed by the ARN of the resource it runs on — "For a Lambda
/// integration, specify the URI of a Lambda function"
/// (`Integration$IntegrationUri`). The other values name an endpoint rather than a
/// resource: `HTTP`/`HTTP_PROXY` take "a fully-qualified URL" and `MOCK` invokes nothing.
pub const AWS_RESOURCE_INTEGRATION_TYPES: &[&str] =
    &[AWS_INTEGRATION_TYPE, AWS_PROXY_INTEGRATION_TYPE];

/// The sentence API Gateway refuses a `CreateIntegration` that names no readable
/// `IntegrationType` with.
///
/// The member is `@required` on the input (`smithy_query(query="operation",
/// operation="CreateIntegration")`) and targets the `IntegrationType` enum, so a request
/// that left it out and one that sent a value outside the enum both name no integration
/// type the service can read: the refusal is the same, and it names the member and what
/// it is required for. [`crate::service`] is where a refusal the framework raises before
/// any handler runs is given this wording.
pub const INTEGRATION_TYPE_REQUIRED: &str =
    "IntegrationType is a required parameter for creating integration.";

/// The sentence API Gateway refuses a caller-supplied ARN it cannot read with.
///
/// It states that the request carried an ARN that is not one and names neither the member
/// nor the value, so it is the same sentence for every such member and every value.
pub const INVALID_ARN_MESSAGE: &str = "Invalid ARN specified in the request";

/// The `IntegrationType` of a proxy integration to an HTTP endpoint, from
/// `INTEGRATION_TYPE_VALUES` ("HTTP_PROXY: for integrating the route or method request
/// with an HTTP endpoint … also referred to as the HTTP proxy integration").
pub const HTTP_PROXY_INTEGRATION_TYPE: &str = "HTTP_PROXY";

/// The one `PayloadFormatVersion` every integration supports: "For all other
/// integrations, 1.0 is the only supported value"
/// (`Integration$PayloadFormatVersion`).
pub const ONLY_PAYLOAD_FORMAT_VERSION: &str = "1.0";

/// The later `PayloadFormatVersion` a Lambda proxy integration supports: "Supported
/// values for Lambda proxy integrations are 1.0 and 2.0"
/// (`Integration$PayloadFormatVersion`).
pub const LAMBDA_PROXY_PAYLOAD_FORMAT_VERSION: &str = "2.0";

/// Every `PayloadFormatVersion` a Lambda proxy integration supports, from the same
/// sentence ("Supported values for Lambda proxy integrations are 1.0 and 2.0").
pub const LAMBDA_PROXY_PAYLOAD_FORMAT_VERSIONS: &[&str] =
    &[ONLY_PAYLOAD_FORMAT_VERSION, LAMBDA_PROXY_PAYLOAD_FORMAT_VERSION];

/// The `PassthroughBehavior` an integration gets when the request names none: "There are
/// three valid values: WHEN_NO_MATCH, WHEN_NO_TEMPLATES, and NEVER … WHEN_NO_MATCH
/// passes the request body for unmapped content types through to the integration backend
/// without transformation" (`Integration$PassthroughBehavior`) — the behaviour of an
/// integration that maps no content type at all, so it is the one the service applies by
/// itself.
pub const DEFAULT_PASSTHROUGH_BEHAVIOR: &str = "WHEN_NO_MATCH";

/// The `PassthroughBehavior` an integration of an api of this `ProtocolType` carries
/// when its create named none, or `None` for an api whose protocol does not support the
/// member at all.
///
/// `Integration$PassthroughBehavior` is "Supported only for WebSocket APIs" — the same
/// clause [`validate_protocol_restricted_members`] refuses the member under for an api of
/// the other protocol — so only a WebSocket api's integration carries one, and it carries
/// [`DEFAULT_PASSTHROUGH_BEHAVIOR`] until a request names another.
pub fn default_passthrough_behavior(protocol_type: &str) -> Option<&'static str> {
    (protocol_type == WEBSOCKET_PROTOCOL_TYPE).then_some(DEFAULT_PASSTHROUGH_BEHAVIOR)
}

/// The HTTP method Lambda's own function-invoking action is reached with, and so the
/// `IntegrationMethod` of an integration that invokes a function: the Invoke action is
/// served at `POST /2015-03-31/functions/{FunctionArn}/invocations`, the resource
/// `Integration$IntegrationUri` names for a Lambda integration ("For a Lambda
/// integration, specify the URI of a Lambda function").
pub const LAMBDA_INVOKE_INTEGRATION_METHOD: &str = "POST";

/// The `IntegrationMethod` an integration of this type and subtype carries when its
/// create named none, or `None` when the method is the request's to name.
///
/// An integration whose endpoint is a Lambda FUNCTION invokes it through Lambda's Invoke
/// action, which is served for one method only ([`LAMBDA_INVOKE_INTEGRATION_METHOD`]), so
/// the method is not a choice the caller has to make and the service supplies it. That is
/// the Lambda proxy integration — `AWS_PROXY` naming no `IntegrationSubtype`, the same
/// pair [`validate_payload_format_version`] reads as one. Every other integration's
/// method belongs to its own endpoint: `HTTP`/`HTTP_PROXY` pass the backend's method, an
/// `AWS_PROXY` integration naming a subtype invokes that service action rather than a
/// function, an `AWS` integration names whichever action its URI addresses, and `MOCK`
/// invokes nothing — so none of those is defaulted here.
pub fn default_integration_method(
    integration_type: &str,
    integration_subtype: Option<&str>,
) -> Option<&'static str> {
    is_lambda_proxy_integration(integration_type, integration_subtype)
        .then_some(LAMBDA_INVOKE_INTEGRATION_METHOD)
}

/// Whether this type and subtype name the LAMBDA PROXY integration: `AWS_PROXY` naming no
/// `IntegrationSubtype`.
///
/// The one reading of that pair, because three rules turn on it — the method the service
/// supplies ([`default_integration_method`]), the payload format versions the integration
/// admits ([`validate_payload_format_version`]), and whether the data plane serves the route
/// by invoking a function (`crate::service::invoke`) — and they must not disagree about
/// which integrations are Lambda proxy ones.
pub fn is_lambda_proxy_integration(
    integration_type: &str,
    integration_subtype: Option<&str>,
) -> bool {
    integration_type == AWS_PROXY_INTEGRATION_TYPE && integration_subtype.is_none()
}

/// The AWS SERVICE ACTION this type and subtype name, or `None` when they name none.
///
/// The other half of the pair [`is_lambda_proxy_integration`] reads: `AWS_PROXY` naming an
/// `IntegrationSubtype` "specifies the AWS service action to invoke"
/// (`CreateIntegration$IntegrationSubtype`), so such an integration is performed by invoking
/// that action rather than by running a function. The subtype is reported TRIMMED, because a
/// stored value surrounded by whitespace names the same action, and a value that is only
/// whitespace names none.
pub fn service_action_subtype<'a>(
    integration_type: &str,
    integration_subtype: Option<&'a str>,
) -> Option<&'a str> {
    if integration_type != AWS_PROXY_INTEGRATION_TYPE {
        return None;
    }
    integration_subtype
        .map(str::trim)
        .filter(|subtype| !subtype.is_empty())
}

/// The `TimeoutInMillis` an integration of an HTTP API gets when the request names
/// none: "The default timeout is … 30 seconds for HTTP APIs"
/// (`CreateIntegration$TimeoutInMillis`).
pub const DEFAULT_HTTP_TIMEOUT_MILLIS: i32 = 30_000;

/// The `TimeoutInMillis` window an integration's timeout has to fall in, from the shape
/// `CreateIntegration$TimeoutInMillis` targets: `IntegerWithLengthBetween50And30000`,
/// `range` 50..30000 (`smithy_query(query="shape",
/// shape="IntegerWithLengthBetween50And30000")`). The boundary validator is what refuses
/// a value outside it; these name the window the refusal states.
pub const MIN_INTEGRATION_TIMEOUT_MILLIS: i32 = 50;

/// The upper end of that window (see [`MIN_INTEGRATION_TIMEOUT_MILLIS`]).
pub const MAX_INTEGRATION_TIMEOUT_MILLIS: i32 = 30_000;

/// The `TimeoutInMillis` an integration of a WebSocket API gets when the request names
/// none: "The default timeout is 29 seconds for WebSocket APIs"
/// (`CreateIntegration$TimeoutInMillis`).
pub const DEFAULT_WEBSOCKET_TIMEOUT_MILLIS: i32 = 29_000;

/// The largest page either paginated operation serves, and the page size it uses when
/// the request names no `MaxResults` — see [`paging::MAX_PAGE_SIZE`], which is defined
/// with the helpers that enforce it.
pub use paging::MAX_PAGE_SIZE;

/// The most VPC links one account may hold, after which CreateVpcLink is refused.
///
/// API Gateway's published quota for the HTTP/WebSocket (ApiGatewayV2) API is 10 VPC
/// links per account per Region, so this is the faithful bound AND what stops a loop of
/// CreateVpcLink calls growing this store without end — every call mints a new link, so
/// nothing else bounds it.
pub const MAX_VPC_LINKS: usize = 10;

/// The `VpcLinkStatus` a VPC link carries while the service is still provisioning it,
/// from `VPC_LINK_STATUS_VALUES` (`AVAILABLE | DELETING | FAILED | INACTIVE |
/// PENDING`).
pub const VPC_LINK_STATUS_PENDING: &str = "PENDING";

/// The `VpcLinkStatus` of a VPC link that finished provisioning, from
/// `VPC_LINK_STATUS_VALUES`.
pub const VPC_LINK_STATUS_AVAILABLE: &str = "AVAILABLE";

/// The `VpcLinkStatus` a VPC link carries once its deletion has been accepted and while
/// the service is still tearing its network interfaces down, from
/// `VPC_LINK_STATUS_VALUES`. A link in this status is still readable; it stops being
/// readable when its deprovisioning is done (see [`VPC_LINK_DELETE_SPAN`]).
pub const VPC_LINK_STATUS_DELETING: &str = "DELETING";

/// The only `VpcLinkVersion` this API models (`VPC_LINK_VERSION_VALUES`: `V2`), so it is
/// the version every link this service mints reports.
pub const VPC_LINK_VERSION_V2: &str = "V2";

/// The `VpcLinkStatusMessage` a VPC link in [`VPC_LINK_STATUS_AVAILABLE`] reports: "a
/// message summarizing the cause of the status of the VPC link"
/// (`VpcLink$VpcLinkStatusMessage`).
///
/// API Gateway serves this sentence, verbatim and in full, for every settled link — it
/// interpolates nothing from the request or from the link's own record, so it is fixed
/// text rather than a value this emulator composes. See [`VpcLink::status_message`].
pub const VPC_LINK_AVAILABLE_MESSAGE: &str = "VPC link is ready to route traffic";

/// The `VpcLinkStatusMessage` a VPC link in [`VPC_LINK_STATUS_PENDING`] reports.
///
/// Unlike [`VPC_LINK_AVAILABLE_MESSAGE`], no sentence for this status has been observed
/// from the service and the model states none, so this one is not verified text: it is
/// written in the form the observed one takes — the status summarised, with nothing
/// interpolated from the request or from state.
pub const VPC_LINK_PENDING_MESSAGE: &str = "VPC link is being provisioned";

/// The `VpcLinkStatusMessage` a VPC link in [`VPC_LINK_STATUS_DELETING`] reports.
///
/// Not verified text either, for the reason [`VPC_LINK_PENDING_MESSAGE`] gives.
pub const VPC_LINK_DELETING_MESSAGE: &str = "VPC link is being deleted";

/// How long a created VPC link stays [`VPC_LINK_STATUS_PENDING`] before it reports
/// [`VPC_LINK_STATUS_AVAILABLE`]: API Gateway attaches the link's network interfaces to
/// the subnets it was given, which is not done when the create call answers.
///
/// API Gateway documents no duration for it ("it can take up to 10 minutes" is
/// guidance, not a contract), so this span is the emulator's own, chosen short enough
/// not to slow a caller down and long enough that a caller reading straight after
/// `CreateVpcLink` still observes [`VPC_LINK_STATUS_PENDING`].
pub const VPC_LINK_CREATE_SPAN: Duration = Duration::from_millis(500);

/// How long a deleted VPC link reports [`VPC_LINK_STATUS_DELETING`] before it is gone
/// from the store: the delete call is ACCEPTED (HTTP 202) while the link's network
/// interfaces are still being detached, which is why the status enum carries a
/// `DELETING` value at all.
///
/// API Gateway documents no duration for it, so this span is the emulator's own — the
/// one [`VPC_LINK_CREATE_SPAN`] uses for the same kind of clock-owed transition, short
/// enough not to slow a caller down and long enough that a caller reading straight
/// after the delete still observes the link going away.
pub const VPC_LINK_DELETE_SPAN: Duration = Duration::from_millis(500);

/// Per-service state for com-amazonaws-apigatewayv2-apigatewayv2. A handler reaches it through [`service_state`].
#[derive(Debug, Default)]
pub struct ComAmazonawsApigatewayv2Apigatewayv2State {
    /// Every persisted api, keyed by `ApiId`. `BTreeMap` so every listing walks
    /// the same stable key order, which is what makes `NextToken` paging
    /// resumable.
    pub apis: BTreeMap<String, Api>,
    /// Drives [`ComAmazonawsApigatewayv2Apigatewayv2State::mint_api_id`]; see
    /// [`mint_id`] for how an id is derived from it.
    pub api_id_seed: u64,
    /// Every persisted portal, keyed by `PortalId`. A portal is an account-level
    /// resource, not a child of an api, so it lives beside [`Self::apis`] rather than
    /// inside one; `BTreeMap` for the same stable key order `ListPortals` pages over.
    ///
    /// It also holds the TOMBSTONE of a portal `DeletePortal` has taken away, which
    /// `GetPortal` still reports in its terminal [`PUBLISH_STATUS_DELETED`] while no other
    /// reader or writer resolves it — so reach a portal through
    /// [`Self::live_portal`]/[`Self::live_portal_mut`]/[`Self::live_portals`] rather than
    /// through this map unless a deleted portal is meant to resolve.
    pub portals: BTreeMap<String, Portal>,
    /// Drives [`ComAmazonawsApigatewayv2Apigatewayv2State::mint_portal_id`]; see
    /// [`mint_id`] for how an id is derived from it.
    pub portal_id_seed: u64,
    /// Every persisted portal product, keyed by `PortalProductId`. A portal product is
    /// an ACCOUNT-level resource, not a child of an api, so it is stored here beside
    /// the apis; `BTreeMap` for the same stable key order
    /// [`ComAmazonawsApigatewayv2Apigatewayv2State::apis`] is kept in, which is what
    /// makes `ListPortalProducts` paging resumable.
    pub portal_products: BTreeMap<String, PortalProduct>,
    /// Drives [`ComAmazonawsApigatewayv2Apigatewayv2State::mint_portal_product_id`].
    pub portal_product_id_seed: u64,
    /// Every persisted VPC link, keyed by `VpcLinkId`. A VPC link is an
    /// account-and-Region resource rather than a child of an api — an integration only
    /// REFERENCES one through its `ConnectionId` — so it is stored beside [`Self::apis`]
    /// and not inside one. `BTreeMap` for the same stable key order [`Self::apis`] has.
    pub vpc_links: BTreeMap<String, VpcLink>,
    /// Drives [`ComAmazonawsApigatewayv2Apigatewayv2State::mint_vpc_link_id`]; see
    /// [`mint_id`] for how an id is derived from it.
    pub vpc_link_id_seed: u64,
    /// Every persisted custom domain name, keyed by its `DomainName` — the name IS the
    /// resource's identifier on the wire (`GET|PATCH|DELETE /v2/domainnames/{DomainName}`),
    /// so there is no second id to key on. `BTreeMap` for the same stable key order the
    /// api store relies on.
    pub domain_names: BTreeMap<String, DomainName>,
    /// Drives [`ComAmazonawsApigatewayv2Apigatewayv2State::mint_domain_name_id`]; see
    /// [`mint_id`] for how an id is derived from it.
    pub domain_name_id_seed: u64,
    /// The decisions this account's Lambda authorizers have already made, while they are
    /// live: "API Gateway caches authorizer responses" for the TTL the authorizer declares
    /// (`Authorizer$AuthorizerResultTtlInSeconds`). It lives beside the apis rather than
    /// inside one because it is DATA-PLANE state — what has been decided, not what was
    /// configured — and `ResetAuthorizersCache` takes away the entries of one stage
    /// ([`authorizer_cache::AuthorizerCache::clear_stage`]).
    pub authorizer_cache: authorizer_cache::AuthorizerCache,
    /// How much of each metered route's throttle the requests already served have taken:
    /// the token bucket a stage's `RouteSettings` meter a route with
    /// (`ThrottlingRateLimit` / `ThrottlingBurstLimit`). Data-plane state, like
    /// [`Self::authorizer_cache`], so it lives beside the apis rather than inside one.
    pub route_throttles: route_throttle::RouteThrottles,
    /// The SOCKET each WebSocket api is reached at and the connections open on it: the
    /// transport its data plane runs over ([`websocket_transport::WebSocketTransport`]).
    /// Data-plane state, like [`Self::authorizer_cache`], and held beside the apis under the
    /// same lock so an api's records and the socket it announces cannot drift apart.
    pub websocket_transport: websocket_transport::WebSocketTransport,
    /// The SOCKET each custom domain name is reached at: the address its record publishes as
    /// its `ApiGatewayDomainName` ([`domain_name_transport::DomainNameTransport`]).
    /// Data-plane state, like [`Self::websocket_transport`], and held beside the domain names
    /// under the same lock so a record and the address it publishes cannot drift apart.
    pub domain_name_transport: domain_name_transport::DomainNameTransport,
    /// The in-process transport this service performs an HTTP api's integrations over when
    /// the request being served carries none of its own
    /// ([`invoke_transport::InvokeTransport`]). Held beside the apis under the same lock, for
    /// the reason [`Self::websocket_transport`] is.
    pub invoke_transport: invoke_transport::InvokeTransport,
}

impl ComAmazonawsApigatewayv2Apigatewayv2State {
    /// Mint a fresh `ApiId`: [`ACCOUNT_ID_LEN`] lowercase alphanumerics, never one
    /// already minted.
    pub fn mint_api_id(&mut self) -> String {
        self.api_id_seed = self.api_id_seed.wrapping_add(1);
        mint_id(IdKind::Api, ACCOUNT_SCOPE, self.api_id_seed)
    }

    /// Mint a fresh `PortalId`: ten lowercase alphanumerics, never one already minted.
    ///
    /// `PortalId` is modelled as `__stringMin10Max30PatternAZ09` (len 10..30, pattern
    /// `^[a-z0-9]+$`), and [`mint_id`] renders exactly [`ACCOUNT_ID_LEN`] characters of
    /// [`ID_ALPHABET`], so every minted id satisfies that constraint by construction.
    pub fn mint_portal_id(&mut self) -> String {
        self.portal_id_seed = self.portal_id_seed.wrapping_add(1);
        mint_id(IdKind::Portal, ACCOUNT_SCOPE, self.portal_id_seed)
    }

    /// Mint a fresh `PortalProductId`: ten lowercase alphanumerics, never one already
    /// minted. The modelled `__stringMin10Max30PatternAZ09` the member targets is
    /// `^[a-z0-9]+$`, 10..30 characters, which [`mint_id`]'s ten-character lowercase
    /// alphanumeric id satisfies.
    pub fn mint_portal_product_id(&mut self) -> String {
        self.portal_product_id_seed = self.portal_product_id_seed.wrapping_add(1);
        mint_id(
            IdKind::PortalProduct,
            ACCOUNT_SCOPE,
            self.portal_product_id_seed,
        )
    }

    /// Mint a fresh `VpcLinkId`: [`ACCOUNT_ID_LEN`] lowercase alphanumerics, never one
    /// already minted.
    pub fn mint_vpc_link_id(&mut self) -> String {
        self.vpc_link_id_seed = self.vpc_link_id_seed.wrapping_add(1);
        mint_id(IdKind::VpcLink, ACCOUNT_SCOPE, self.vpc_link_id_seed)
    }

    /// Drop every VPC link whose deprovisioning the CLOCK has already finished (see
    /// [`VpcLink::is_deprovisioned`]).
    ///
    /// The transition is owed by the clock, not by a caller: it was due at the same
    /// instant whether or not anybody called, so materialising it here leaves every
    /// reader agreeing on which links the account still holds — and frees the quota slot
    /// a gone link no longer occupies.
    pub fn purge_deprovisioned_vpc_links(&mut self) {
        self.vpc_links.retain(|_, link| !link.is_deprovisioned());
    }

    /// The portal `portal_id` names while the account still HOLDS it, or `None` when no
    /// portal of that id is held any more — an id nothing minted, and one a
    /// `DeletePortal` has already taken away ([`Portal::is_deleted`]).
    ///
    /// Every portal reader and writer resolves its `PortalId` through this, so one deleted
    /// portal is absent from all of them at once. The single exception is `GetPortal`,
    /// which resolves through [`Self::readable_portal`]: a deleted portal stays READABLE in
    /// its terminal status (see [`PUBLISH_STATUS_DELETED`]) until its removal comes due.
    pub fn live_portal(&self, portal_id: &str) -> Option<&Portal> {
        self.portals
            .get(portal_id)
            .filter(|portal| !portal.is_deleted())
    }

    /// [`Self::live_portal`] for a writer: the portal `portal_id` names while the account
    /// still holds it.
    pub fn live_portal_mut(&mut self, portal_id: &str) -> Option<&mut Portal> {
        self.portals
            .get_mut(portal_id)
            .filter(|portal| !portal.is_deleted())
    }

    /// Every portal the account still holds, in the key order [`Self::portals`] is kept
    /// in: the collection `ListPortals` pages over and the one the portal data plane
    /// resolves a host against, neither of which reports a portal that is gone.
    pub fn live_portals(&self) -> impl Iterator<Item = &Portal> {
        self.portals
            .values()
            .filter(|portal| !portal.is_deleted())
    }

    /// The portal `portal_id` names as a READER of one portal sees it as of `now`: the
    /// portal the account holds, or the tombstone of a deleted one whose removal has not
    /// come due yet — which `GetPortal` reports in its terminal
    /// [`PUBLISH_STATUS_DELETED`]. `None` once that removal is due, and for an id nothing
    /// minted.
    pub fn readable_portal(&self, portal_id: &str, now: SystemTime) -> Option<&Portal> {
        self.portals
            .get(portal_id)
            .filter(|portal| !portal.is_gone(now))
    }

    /// Take the portal `portal_id` names away as of `now`, answering whether this call is
    /// the one that took it (`false` for an id the account holds no live portal under, so
    /// a repeated delete is not a second removal).
    ///
    /// The record is kept as a TOMBSTONE rather than dropped: AWS keeps answering
    /// `GetPortal` for the portal it is removing with its body and the terminal
    /// [`PUBLISH_STATUS_DELETED`], so the record a reader needs has to survive the delete
    /// until the removal comes due ([`PORTAL_DELETION`]). Everything the portal was DOING
    /// stops at once ([`Portal::delete`]), and the slot it occupied in the account's quota
    /// is free, because a tombstone is not a portal the account holds.
    pub fn delete_portal(&mut self, portal_id: &str, now: SystemTime) -> bool {
        // Every removal the clock has already finished, materialised: it was owed at its
        // due instant whether or not anybody called, and doing it here is what keeps the
        // store from holding records no reader reports.
        self.purge_gone_portals(now);
        let Some(record) = self.live_portal_mut(portal_id) else {
            return false;
        };
        record.delete(now);
        self.evict_oldest_portal_tombstones();
        true
    }

    /// Drop every portal whose removal the CLOCK has already finished
    /// ([`Portal::is_gone`]).
    pub fn purge_gone_portals(&mut self, now: SystemTime) {
        self.portals.retain(|_, portal| !portal.is_gone(now));
    }

    /// Drop the longest-deleted tombstones once the store holds more than
    /// [`MAX_PORTAL_TOMBSTONES`] of them, so a loop of create-and-delete cannot grow this
    /// store without end. A tombstone this drops is a portal no reader reports at all any
    /// more, exactly as if it had never been kept.
    fn evict_oldest_portal_tombstones(&mut self) {
        loop {
            let mut tombstones: Vec<(&String, SystemTime)> = self
                .portals
                .iter()
                .filter_map(|(id, portal)| portal.deleted_at.map(|at| (id, at)))
                .collect();
            if tombstones.len() <= MAX_PORTAL_TOMBSTONES {
                return;
            }
            tombstones.sort_by_key(|(_, at)| *at);
            let Some((oldest, _)) = tombstones.first() else {
                return;
            };
            let oldest = (*oldest).clone();
            self.portals.remove(&oldest);
        }
    }

    /// Mint the id of the regional host a custom domain name is served on:
    /// [`ACCOUNT_ID_LEN`] lowercase alphanumerics, never one already minted. Drawn from a
    /// seed of its own, so minting a domain name never shifts the ids apis get.
    pub fn mint_domain_name_id(&mut self) -> String {
        self.domain_name_id_seed = self.domain_name_id_seed.wrapping_add(1);
        mint_id(IdKind::DomainName, ACCOUNT_SCOPE, self.domain_name_id_seed)
    }
}

/// A persisted `Api` resource.
#[derive(Debug, Clone)]
pub struct Api {
    pub api_id: String,
    pub name: String,
    /// One of `PROTOCOL_TYPE_VALUES` (`HTTP | WEBSOCKET`).
    pub protocol_type: String,
    pub api_endpoint: String,
    /// The Region this api lives in: the Region the request that created it was signed for
    /// ([`request_region`]).
    ///
    /// A property of the RESOURCE rather than of the endpoint it announces, so it is stored
    /// rather than read back out of [`Api::api_endpoint`] — that value is the address this
    /// emulator serves the api at ([`announced_execute_api_endpoint`]), which names no
    /// Region when a client reached the emulator's own endpoint. It is the Region the api's
    /// own `execute-api` host carries ([`execute_api_host`]) and the one the `execute-api`
    /// resource ARN an invoke is authorized against names.
    pub region: String,
    pub description: Option<String>,
    pub version: Option<String>,
    /// Required on the modelled `Api` shape, so the record always carries one.
    pub route_selection_expression: String,
    pub api_key_selection_expression: String,
    pub cors_configuration: Option<wire::Cors>,
    pub disable_execute_api_endpoint: bool,
    pub disable_schema_validation: bool,
    pub ip_address_type: String,
    pub created_date: SystemTime,
    pub tags: BTreeMap<String, String>,
    /// This api's routes, keyed by `RouteId`.
    pub routes: BTreeMap<String, Route>,
    /// Drives [`Api::mint_route_id`]. Per-api, because a `RouteId` only has to be
    /// unique within the api that owns the route.
    pub route_id_seed: u64,
    /// This api's integrations, keyed by `IntegrationId`.
    pub integrations: BTreeMap<String, Integration>,
    /// Drives [`Api::mint_integration_id`]. Per-api, for the same reason as
    /// [`Api::route_id_seed`]: an `IntegrationId` is unique within its api.
    pub integration_id_seed: u64,
    /// This api's stages, keyed by `StageName`.
    pub stages: BTreeMap<String, Stage>,
    /// This api's models, keyed by `ModelId`.
    pub models: BTreeMap<String, Model>,
    /// Drives [`Api::mint_model_id`]. Per-api, for the same reason as
    /// [`Api::route_id_seed`]: a `ModelId` is unique within its api.
    pub model_id_seed: u64,
    /// This api's deployments, keyed by `DeploymentId`.
    pub deployments: BTreeMap<String, Deployment>,
    /// Drives [`Api::mint_deployment_id`]. Per-api, for the same reason as
    /// [`Api::route_id_seed`]: a `DeploymentId` is unique within its api.
    pub deployment_id_seed: u64,
    /// This api's authorizers, keyed by `AuthorizerId`.
    pub authorizers: BTreeMap<String, Authorizer>,
    /// Drives [`Api::mint_authorizer_id`]. Per-api, for the same reason as
    /// [`Api::route_id_seed`]: an `AuthorizerId` is unique within its api.
    pub authorizer_id_seed: u64,
}

impl Api {
    /// Mint a fresh `RouteId` for this api: [`ROUTE_ID_LEN`] lowercase alphanumerics,
    /// never one this api already minted, and never one another api minted either — the
    /// api's own id is the SCOPE the id is drawn within (see [`mint_id`]).
    pub fn mint_route_id(&mut self) -> String {
        self.route_id_seed = self.route_id_seed.wrapping_add(1);
        mint_id(IdKind::Route, &self.api_id, self.route_id_seed)
    }

    /// Mint a fresh `IntegrationId` for this api: [`ROUTE_ID_LEN`] lowercase
    /// alphanumerics, never one this api already minted, and never one another api
    /// minted either — the api's own id is the SCOPE the id is drawn within.
    pub fn mint_integration_id(&mut self) -> String {
        self.integration_id_seed = self.integration_id_seed.wrapping_add(1);
        mint_id(IdKind::Integration, &self.api_id, self.integration_id_seed)
    }

    /// Mint a fresh `ModelId` for this api: [`NESTED_ID_LEN`] lowercase alphanumerics,
    /// never one this api already minted, and never one another api minted either — the
    /// api's own id is the SCOPE the id is drawn within.
    pub fn mint_model_id(&mut self) -> String {
        self.model_id_seed = self.model_id_seed.wrapping_add(1);
        mint_id(IdKind::Model, &self.api_id, self.model_id_seed)
    }

    /// Mint a fresh `DeploymentId` for this api: [`NESTED_ID_LEN`] lowercase
    /// alphanumerics, never one this api already minted, and never one another api
    /// minted either — the api's own id is the SCOPE the id is drawn within.
    pub fn mint_deployment_id(&mut self) -> String {
        self.deployment_id_seed = self.deployment_id_seed.wrapping_add(1);
        mint_id(IdKind::Deployment, &self.api_id, self.deployment_id_seed)
    }

    /// Mint a fresh `AuthorizerId` for this api: [`NESTED_ID_LEN`] lowercase
    /// alphanumerics, never one this api already minted, and never one another api
    /// minted either — the api's own id is the SCOPE the id is drawn within.
    pub fn mint_authorizer_id(&mut self) -> String {
        self.authorizer_id_seed = self.authorizer_id_seed.wrapping_add(1);
        mint_id(IdKind::Authorizer, &self.api_id, self.authorizer_id_seed)
    }

    /// Release a deployment of this api's current configuration BY THE SERVICE, for a
    /// stage whose `AutoDeploy` is on.
    ///
    /// Such a stage has its deployments released by the service rather than by a caller
    /// ("Specifies whether a deployment was automatically released",
    /// [`Deployment::auto_deployed`]), so the api's routes and integrations — which this
    /// record already holds — are released to it at `now`, the same instant the call
    /// that asked for the stage is resolving at. Both the stage quick create mints
    /// alongside an api and a `CreateStage` that asks for auto-deploy go through here,
    /// so one api cannot release two differently shaped deployments.
    ///
    /// Answers `(DeploymentId, LastDeploymentStatusMessage)`: the id of the deployment
    /// released, with the message the stage reports for it, or no id at all when the api
    /// is already at [`MAX_DEPLOYMENTS_PER_API`] — the quota belongs to the deployment,
    /// not to the stage, so the stage still exists and reports why its last deployment
    /// did not happen.
    pub fn release_auto_deployment(
        &mut self,
        now: SystemTime,
    ) -> (Option<String>, Option<String>) {
        if self.deployments.len() >= MAX_DEPLOYMENTS_PER_API {
            return (
                None,
                Some(format!(
                    "Failed to deploy stage: the maximum number of deployments \
                     ({MAX_DEPLOYMENTS_PER_API}) for this API has been reached."
                )),
            );
        }
        let configuration = self.deployable_configuration();
        let released = self.mint_deployment_id();
        self.deployments.insert(
            released.clone(),
            Deployment {
                deployment_id: released.clone(),
                // The service released this one; no caller described it.
                description: None,
                deployment_status: DEPLOYMENT_STATUS_DEPLOYED.to_string(),
                deployment_status_message: None,
                auto_deployed: true,
                created_date: now,
                configuration,
            },
        );
        let message = format!("Successfully deployed stage with deployment id '{released}'");
        (Some(released), Some(message))
    }

    /// The configuration a deployment of this api FREEZES: the routes and integrations it
    /// holds right now, each with the responses that belong to it.
    ///
    /// A deployment is "an immutable representation of an API that can be called by users"
    /// (the `Deployment` shape's own doc), so releasing one captures the api's routes and
    /// integrations rather than referring to them: a later change to either leaves every
    /// deployment already released exactly as it was, and a stage serving one of those
    /// serves what it froze ([`Deployment::configuration`]).
    ///
    /// What one snapshot holds is bounded by the quotas the creates enforce
    /// ([`MAX_ROUTES_PER_API`] routes and [`MAX_INTEGRATIONS_PER_API`] integrations), and
    /// how many an api can hold by [`MAX_DEPLOYMENTS_PER_API`], so neither a loop of
    /// configuration changes nor a loop of deployments grows this store without end.
    pub fn deployable_configuration(&self) -> DeployedConfiguration {
        DeployedConfiguration {
            routes: self.routes.clone(),
            integrations: self.integrations.clone(),
        }
    }

    /// Release this api's configuration afresh to every stage of it that auto-deploys,
    /// because that configuration has just changed.
    ///
    /// "Specifies whether updates to an API automatically trigger a new deployment"
    /// (`Stage$AutoDeploy`): such a stage does not stay on the deployment it was last
    /// released when the api's routes or integrations move under it — the service releases
    /// the new configuration to it by itself, which is what [`Deployment::auto_deployed`]
    /// marks. A stage whose `AutoDeploy` is off keeps the deployment it was pinned to,
    /// because that deployment is the one its caller chose.
    ///
    /// ONE release serves every auto-deploying stage of the api, so one change mints one
    /// deployment however many stages are on it, and an api with no auto-deploying stage
    /// mints none at all. Every writer of this api's routes and integrations calls this
    /// once it has written, so no two of them release differently.
    pub fn redeploy_auto_deploying_stages(&mut self, now: SystemTime) {
        if !self.stages.values().any(Stage::auto_deploys) {
            return;
        }
        let (released, status_message) = self.release_auto_deployment(now);
        for stage in self
            .stages
            .values_mut()
            .filter(|stage| stage.auto_deploys())
        {
            // A release the deployment quota refused leaves the stage on the deployment it
            // already serves, reporting why the last one did not happen.
            if released.is_some() {
                stage.deployment_id = released.clone();
            }
            stage.last_deployment_status_message = status_message.clone();
            stage.last_updated_date = now;
        }
    }
}

impl Api {
    /// The ids of this api's routes that are wired to `integration_id`, in the store's
    /// stable `RouteId` order.
    ///
    /// A route holds the reference (through its `Target`), so the integration is the
    /// one that cannot be removed while any of them exists: a deleted integration
    /// would leave those routes pointing at nothing.
    pub fn routes_targeting_integration(&self, integration_id: &str) -> Vec<String> {
        self.routes
            .values()
            .filter(|route| route.target_integration_id() == Some(integration_id))
            .map(|route| route.route_id.clone())
            .collect()
    }

    /// The names of this api's stages that are serving `deployment_id`, in the store's
    /// stable `StageName` order.
    ///
    /// A stage holds the reference (through its `DeploymentId`: "The identifier of the
    /// Deployment that the Stage is associated with"), so the deployment is the one that
    /// cannot be removed while any of them serves it: removing it would leave those
    /// stages serving nothing, and "A Deployment must be associated with a Stage for it
    /// to be callable over the internet" (the `Deployment` shape's own doc).
    pub fn stages_serving_deployment(&self, deployment_id: &str) -> Vec<String> {
        self.stages
            .values()
            .filter(|stage| stage.deployment_id.as_deref() == Some(deployment_id))
            .map(|stage| stage.stage_name.clone())
            .collect()
    }

    /// The route KEYS of this api's routes that are associated with `authorizer_id`, in
    /// the store's stable `RouteId` order.
    ///
    /// A route holds the reference (through its `AuthorizerId`), so the authorizer is the
    /// one that cannot be removed while any of them exists: a deleted authorizer would
    /// leave those routes naming nothing. An empty `AuthorizerId` names no authorizer, the
    /// same reading [`Api::unresolved_route_reference`] gives it.
    ///
    /// The KEY is what identifies a route to a caller — it is the route's name within its
    /// api — so it is what a refusal about the reference names.
    pub fn route_keys_using_authorizer(&self, authorizer_id: &str) -> Vec<String> {
        self.routes
            .values()
            .filter(|route| {
                route
                    .authorizer_id
                    .as_deref()
                    .filter(|id| !id.is_empty())
                    .is_some_and(|id| id == authorizer_id)
            })
            .map(|route| route.route_key.clone())
            .collect()
    }

    /// The refusal message for a reference of this route that this api cannot resolve,
    /// or `None` when every reference names a child resource this api holds.
    ///
    /// A route references two resources of its OWN api: the integration its `Target`
    /// addresses (`integrations/{IntegrationId}`) and the authorizer its `AuthorizerId`
    /// names. Neither may be stored unresolved — a route wired to nothing could never be
    /// invoked, and the api it belongs to could not be reproduced on AWS — so every
    /// writer of a route resolves them here before persisting anything. The reference is
    /// part of the REQUEST, so a request naming one that does not resolve is "not valid,
    /// for example, the input is incomplete or incorrect": the message is the
    /// operation's `BadRequestException`'s.
    pub fn unresolved_route_reference(
        &self,
        target: Option<&str>,
        authorizer_id: Option<&str>,
    ) -> Option<String> {
        if let Some(integration_id) = target.and_then(target_integration_id) {
            if !self.integrations.contains_key(integration_id) {
                // The message names the kind of target the key addresses and the id it
                // carried, both read out of the request's own `Target`.
                return Some(format!(
                    "Unable to find route target of type {} with ID {integration_id}",
                    INTEGRATION_TARGET_PREFIX.trim_end_matches('/')
                ));
            }
        }
        if let Some(authorizer_id) = authorizer_id.filter(|id| !id.is_empty()) {
            if !self.authorizers.contains_key(authorizer_id) {
                return Some(UNRESOLVED_AUTHORIZER_MESSAGE.to_string());
            }
        }
        None
    }

    /// Project the record onto the modelled `Api` shape.
    pub fn to_wire(&self) -> wire::Api {
        wire::Api {
            api_endpoint: Some(self.api_endpoint.clone()),
            // Only a resource another service created on the caller's behalf is
            // API-Gateway-managed; an api created through CreateApi is not.
            api_gateway_managed: Some(false),
            api_id: Some(self.api_id.clone()),
            api_key_selection_expression: Some(self.api_key_selection_expression.clone()),
            cors_configuration: self.cors_configuration.clone(),
            created_date: Some(to_timestamp(self.created_date)),
            description: self.description.clone(),
            disable_schema_validation: Some(self.disable_schema_validation),
            disable_execute_api_endpoint: Some(self.disable_execute_api_endpoint),
            // `ImportInfo`/`Warnings` report on an OpenAPI import. Nothing was
            // imported into this record, so there is nothing to report.
            import_info: None,
            ip_address_type: Some(self.ip_address_type.clone()),
            name: self.name.clone(),
            protocol_type: self.protocol_type.clone(),
            route_selection_expression: self.route_selection_expression.clone(),
            tags: Some(self.tags.iter().map(|(k, v)| (k.clone(), v.clone())).collect()),
            version: self.version.clone(),
            warnings: None,
        }
    }
}

/// A persisted `Route` resource, owned by its parent [`Api`].
#[derive(Debug, Clone, Default)]
pub struct Route {
    pub route_id: String,
    pub route_key: String,
    pub target: Option<String>,
    /// One of `AUTHORIZATION_TYPE_VALUES`.
    pub authorization_type: String,
    pub authorizer_id: Option<String>,
    pub api_key_required: bool,
    pub operation_name: Option<String>,
    pub authorization_scopes: Vec<String>,
    pub model_selection_expression: Option<String>,
    pub route_response_selection_expression: Option<String>,
    pub request_models: BTreeMap<String, String>,
    pub request_parameters: BTreeMap<String, wire::ParameterConstraints>,
    /// True only for the `$default` route quick create mints alongside the api:
    /// "If you created an API using quick create, the $default route is managed by
    /// API Gateway" (`Route$ApiGatewayManaged`).
    pub api_gateway_managed: bool,
    /// This route's route responses, keyed by `RouteResponseId`. Owned by the route,
    /// so deleting the route (or its api) cascades to them in one `remove`.
    pub route_responses: BTreeMap<String, RouteResponse>,
    /// Drives [`Route::mint_route_response_id`]. Per-route, because a
    /// `RouteResponseId` only has to be unique within the route that owns the
    /// response.
    pub route_response_id_seed: u64,
}

impl Route {
    /// The `IntegrationId` this route is wired to, when its target names an
    /// integration of the same api (`integrations/{IntegrationId}`). `None` for a
    /// route with no target at all, and for a target of any other form — a target
    /// this service does not resolve to an integration references none of them.
    pub fn target_integration_id(&self) -> Option<&str> {
        self.target.as_deref().and_then(target_integration_id)
    }

    /// Mint a fresh `RouteResponseId` for this route: [`NESTED_ID_LEN`] lowercase
    /// alphanumerics, never one this route already minted, and never one another route
    /// minted either — the route's own id is the SCOPE the id is drawn within.
    pub fn mint_route_response_id(&mut self) -> String {
        self.route_response_id_seed = self.route_response_id_seed.wrapping_add(1);
        mint_id(
            IdKind::RouteResponse,
            &self.route_id,
            self.route_response_id_seed,
        )
    }

    /// Project the record onto the modelled `Route` shape.
    pub fn to_wire(&self) -> wire::Route {
        wire::Route {
            api_gateway_managed: Some(self.api_gateway_managed),
            api_key_required: Some(self.api_key_required),
            authorization_scopes: Some(self.authorization_scopes.clone()),
            authorization_type: Some(self.authorization_type.clone()),
            authorizer_id: self.authorizer_id.clone(),
            model_selection_expression: self.model_selection_expression.clone(),
            operation_name: self.operation_name.clone(),
            request_models: Some(
                self.request_models
                    .iter()
                    .map(|(k, v)| (k.clone(), v.clone()))
                    .collect(),
            ),
            request_parameters: Some(
                self.request_parameters
                    .iter()
                    .map(|(k, v)| (k.clone(), v.clone()))
                    .collect(),
            ),
            route_id: Some(self.route_id.clone()),
            route_key: self.route_key.clone(),
            route_response_selection_expression: self.route_response_selection_expression.clone(),
            target: self.target.clone(),
        }
    }
}

/// A persisted `RouteResponse` resource, owned by its parent [`Route`].
///
/// A route response is WebSocket-only: `RouteResponse$ModelSelectionExpression` is
/// "Supported only for WebSocket APIs", and the selection keys it is matched against
/// are the WebSocket API's route response selection expressions.
#[derive(Debug, Clone, Default)]
pub struct RouteResponse {
    pub route_response_id: String,
    /// Required on the modelled `RouteResponse` shape, so the record always carries
    /// one. Unique within the owning route.
    pub route_response_key: String,
    pub model_selection_expression: Option<String>,
    pub response_models: BTreeMap<String, String>,
    pub response_parameters: BTreeMap<String, wire::ParameterConstraints>,
}

impl RouteResponse {
    /// Project the record onto the modelled `RouteResponse` shape.
    pub fn to_wire(&self) -> wire::RouteResponse {
        wire::RouteResponse {
            model_selection_expression: self.model_selection_expression.clone(),
            response_models: Some(
                self.response_models
                    .iter()
                    .map(|(k, v)| (k.clone(), v.clone()))
                    .collect(),
            ),
            response_parameters: Some(
                self.response_parameters
                    .iter()
                    .map(|(k, v)| (k.clone(), v.clone()))
                    .collect(),
            ),
            route_response_id: Some(self.route_response_id.clone()),
            route_response_key: self.route_response_key.clone(),
        }
    }
}

/// A persisted `Integration` resource, owned by its parent [`Api`].
///
/// The record keeps what the create resolved — the id it minted, the connection type and
/// the timeout the request left to their documented defaults — so every reader of the
/// integration reports one value, and [`Integration::to_wire`] projects it onto the
/// modelled shape the handlers serialize.
#[derive(Debug, Clone, Default)]
pub struct Integration {
    pub integration_id: String,
    /// One of `INTEGRATION_TYPE_VALUES` (`AWS | AWS_PROXY | HTTP | HTTP_PROXY | MOCK`).
    pub integration_type: String,
    pub integration_uri: Option<String>,
    pub integration_method: Option<String>,
    pub integration_subtype: Option<String>,
    /// One of `CONNECTION_TYPE_VALUES`; [`DEFAULT_CONNECTION_TYPE`] when the request
    /// named none.
    pub connection_type: String,
    pub connection_id: Option<String>,
    pub content_handling_strategy: Option<String>,
    pub credentials_arn: Option<String>,
    pub description: Option<String>,
    pub passthrough_behavior: Option<String>,
    pub payload_format_version: Option<String>,
    pub request_parameters: BTreeMap<String, String>,
    pub request_templates: BTreeMap<String, String>,
    pub response_parameters: BTreeMap<String, BTreeMap<String, String>>,
    pub template_selection_expression: Option<String>,
    /// The documented default for the parent api's protocol when the request named no
    /// timeout.
    pub timeout_in_millis: i32,
    pub tls_config: Option<wire::TlsConfig>,
    /// True only for an integration another operation created on the caller's behalf:
    /// "If you created an API using quick create, the resulting integration is managed
    /// by API Gateway" (`Integration$ApiGatewayManaged`).
    pub api_gateway_managed: bool,
    /// This integration's integration responses, keyed by `IntegrationResponseId`.
    /// Owned by the integration, so deleting the integration (or its api) cascades to
    /// them in one `remove`.
    pub integration_responses: BTreeMap<String, IntegrationResponse>,
    /// Drives [`Integration::mint_integration_response_id`]. Per-integration, because
    /// an `IntegrationResponseId` only has to be unique within the integration that
    /// owns the response.
    pub integration_response_id_seed: u64,
}

impl Integration {
    /// Mint a fresh `IntegrationResponseId` for this integration: [`NESTED_ID_LEN`]
    /// lowercase alphanumerics, never one this integration already minted, and never one
    /// another integration minted either — the integration's own id is the SCOPE the id
    /// is drawn within.
    pub fn mint_integration_response_id(&mut self) -> String {
        self.integration_response_id_seed = self.integration_response_id_seed.wrapping_add(1);
        mint_id(
            IdKind::IntegrationResponse,
            &self.integration_id,
            self.integration_response_id_seed,
        )
    }

    /// Project the record onto the modelled `Integration` shape.
    pub fn to_wire(&self) -> wire::Integration {
        wire::Integration {
            api_gateway_managed: Some(self.api_gateway_managed),
            connection_id: self.connection_id.clone(),
            connection_type: Some(self.connection_type.clone()),
            content_handling_strategy: self.content_handling_strategy.clone(),
            credentials_arn: self.credentials_arn.clone(),
            description: self.description.clone(),
            integration_id: Some(self.integration_id.clone()),
            integration_method: self.integration_method.clone(),
            // "The integration response selection expression for the integration.
            // Supported only for WebSocket APIs"
            // (`Integration$IntegrationResponseSelectionExpression`): no request member
            // sets it, and an integration response of this service is selected by the
            // `IntegrationResponseKey` it was stored under, so the record holds no such
            // expression to report.
            integration_response_selection_expression: None,
            integration_subtype: self.integration_subtype.clone(),
            integration_type: Some(self.integration_type.clone()),
            integration_uri: self.integration_uri.clone(),
            passthrough_behavior: self.passthrough_behavior.clone(),
            payload_format_version: self.payload_format_version.clone(),
            request_parameters: Some(
                self.request_parameters
                    .iter()
                    .map(|(k, v)| (k.clone(), v.clone()))
                    .collect(),
            ),
            request_templates: Some(
                self.request_templates
                    .iter()
                    .map(|(k, v)| (k.clone(), v.clone()))
                    .collect(),
            ),
            response_parameters: Some(
                self.response_parameters
                    .iter()
                    .map(|(selection, parameters)| {
                        (
                            selection.clone(),
                            parameters
                                .iter()
                                .map(|(k, v)| (k.clone(), v.clone()))
                                .collect(),
                        )
                    })
                    .collect(),
            ),
            template_selection_expression: self.template_selection_expression.clone(),
            timeout_in_millis: Some(self.timeout_in_millis),
            tls_config: self.tls_config.clone(),
        }
    }
}

/// A persisted `IntegrationResponse` resource, owned by its parent [`Integration`].
///
/// An integration response is WebSocket-only: its
/// `IntegrationResponse$ContentHandlingStrategy` is "Supported only for WebSocket
/// APIs", and the selection keys it is matched against are the WebSocket API's
/// integration response selection expressions.
#[derive(Debug, Clone, Default)]
pub struct IntegrationResponse {
    pub integration_response_id: String,
    /// Required on the modelled `IntegrationResponse` shape, so the record always
    /// carries one. Unique within the owning integration.
    pub integration_response_key: String,
    /// One of `CONTENT_HANDLING_STRATEGY_VALUES` when the request named one.
    pub content_handling_strategy: Option<String>,
    pub response_parameters: BTreeMap<String, String>,
    pub response_templates: BTreeMap<String, String>,
    pub template_selection_expression: Option<String>,
}

impl IntegrationResponse {
    /// Project the record onto the modelled `IntegrationResponse` shape.
    pub fn to_wire(&self) -> wire::IntegrationResponse {
        wire::IntegrationResponse {
            content_handling_strategy: self.content_handling_strategy.clone(),
            integration_response_id: Some(self.integration_response_id.clone()),
            integration_response_key: self.integration_response_key.clone(),
            response_parameters: Some(
                self.response_parameters
                    .iter()
                    .map(|(k, v)| (k.clone(), v.clone()))
                    .collect(),
            ),
            response_templates: Some(
                self.response_templates
                    .iter()
                    .map(|(k, v)| (k.clone(), v.clone()))
                    .collect(),
            ),
            template_selection_expression: self.template_selection_expression.clone(),
        }
    }
}

/// A persisted `Model` resource, owned by its parent [`Api`].
///
/// The record keeps the schema SOURCE the create accepted, verbatim: every reader of
/// the model reports the document the caller supplied, and [`Model::to_wire`] projects
/// the record onto the modelled shape the handlers serialize.
#[derive(Debug, Clone, Default)]
pub struct Model {
    pub model_id: String,
    pub name: String,
    pub content_type: Option<String>,
    pub description: Option<String>,
    pub schema: String,
}

impl Model {
    /// Project the record onto the modelled `Model` shape.
    pub fn to_wire(&self) -> wire::Model {
        wire::Model {
            content_type: self.content_type.clone(),
            description: self.description.clone(),
            model_id: Some(self.model_id.clone()),
            name: self.name.clone(),
            schema: Some(self.schema.clone()),
        }
    }
}

/// A persisted `Authorizer` resource, owned by its parent [`Api`].
///
/// The record keeps the configuration the create accepted, so every reader of the
/// authorizer reports one record rather than its own reconstruction of it, and
/// [`Authorizer::to_wire`] projects it onto the modelled shape the handlers serialize.
#[derive(Debug, Clone, Default)]
pub struct Authorizer {
    pub authorizer_id: String,
    /// Required on the modelled `Authorizer` shape, so the record always carries one.
    pub name: String,
    /// One of `AUTHORIZER_TYPE_VALUES` (`JWT | REQUEST`).
    pub authorizer_type: String,
    pub authorizer_uri: Option<String>,
    pub authorizer_credentials_arn: Option<String>,
    pub authorizer_payload_format_version: Option<String>,
    pub authorizer_result_ttl_in_seconds: Option<i32>,
    pub enable_simple_responses: Option<bool>,
    /// `IdentitySource` is a REQUIRED member of `CreateAuthorizerRequest`, so the
    /// record always carries the list — possibly empty, which a REQUEST authorizer
    /// with caching disabled is allowed to be.
    pub identity_source: Vec<String>,
    pub identity_validation_expression: Option<String>,
    pub jwt_configuration: Option<wire::JWTConfiguration>,
}

impl Authorizer {
    /// Project the record onto the modelled `Authorizer` shape.
    pub fn to_wire(&self) -> wire::Authorizer {
        wire::Authorizer {
            authorizer_credentials_arn: self.authorizer_credentials_arn.clone(),
            authorizer_id: Some(self.authorizer_id.clone()),
            authorizer_payload_format_version: self.authorizer_payload_format_version.clone(),
            authorizer_result_ttl_in_seconds: self.authorizer_result_ttl_in_seconds,
            authorizer_type: Some(self.authorizer_type.clone()),
            authorizer_uri: self.authorizer_uri.clone(),
            enable_simple_responses: self.enable_simple_responses,
            // The list the record holds, reported whether or not it is empty: the
            // member is required on the request, so the resource always has one.
            identity_source: Some(self.identity_source.clone()),
            identity_validation_expression: self.identity_validation_expression.clone(),
            jwt_configuration: self.jwt_configuration.clone(),
            name: self.name.clone(),
        }
    }
}

/// `Ok(())`, or the reason this configuration is not one its own `AuthorizerType`
/// admits — the message for the caller's `BadRequestException`.
///
/// Every clause is a member's own documentation, which makes a member required for one
/// authorizer type, or one api protocol, and undefined for another:
///
/// * `AuthorizerType` — "Specify JWT to use JSON Web Tokens (supported only for HTTP
///   APIs)", so a WebSocket api takes only `REQUEST` authorizers.
/// * `JwtConfiguration` — "Required for the JWT authorizer type", and its `Issuer` is
///   "the base domain of the identity provider that issues JSON Web Tokens", from which
///   API Gateway fetches the provider's OpenID discovery document when the authorizer is
///   created — so a JWT authorizer cannot validate a token without an issuer that can carry
///   one ([`jwt_issuer`]);
/// * `AuthorizerUri` — "For REQUEST authorizers, this must be a well-formed Lambda
///   function URI": the Lambda to invoke, so that type cannot authorize without it.
/// * `AuthorizerPayloadFormatVersion` — "Specifies the format of the payload sent to an
///   HTTP API Lambda authorizer. Required for HTTP API Lambda authorizers. Supported
///   values are 1.0 and 2.0."
///
/// `EnableSimpleResponses` is deliberately NOT cross-checked against the payload format
/// version: a simple-response authorizer under payload format 1.0 is a configuration the
/// service accepts and that fails when the authorizer is INVOKED, not when it is created.
///
/// Shared by every writer of an authorizer, so the requirement cannot differ between
/// the create and a later update.
pub fn validate_authorizer_config(
    protocol_type: &str,
    authorizer_type: &str,
    authorizer_uri: Option<&str>,
    payload_format_version: Option<&str>,
    jwt_configuration: Option<&wire::JWTConfiguration>,
) -> Result<(), String> {
    let is_websocket = protocol_type == WEBSOCKET_PROTOCOL_TYPE;
    if authorizer_type == AUTHORIZER_TYPE_JWT {
        if is_websocket {
            // API Gateway words this one as a statement about the TYPES a WebSocket api
            // admits rather than about the type the request named: the one type such an
            // api supports is `REQUEST`, and the sentence names it. Every part of it is
            // fixed text — the protocol type it names is the only one that restricts the
            // catalog this way.
            return Err(format!(
                "Invalid authorizer type. Only {AUTHORIZER_TYPE_REQUEST} authorizer type \
                 is supported on {WEBSOCKET_PROTOCOL_TYPE} protocol Apis."
            ));
        }
        let Some(configuration) = jwt_configuration else {
            return Err(jwt_issuer::JWT_CONFIGURATION_REQUIRED_MESSAGE.to_string());
        };
        jwt_issuer::validate_jwt_issuer(configuration.issuer.as_deref())?;
    }
    if authorizer_type == AUTHORIZER_TYPE_REQUEST {
        if authorizer_uri.is_none_or(str::is_empty) {
            return Err(format!(
                "AuthorizerUri is required for an authorizer of type {AUTHORIZER_TYPE_REQUEST}"
            ));
        }
        if !is_websocket && payload_format_version.is_none_or(str::is_empty) {
            return Err(format!(
                "AuthorizerPayloadFormatVersion is required for an authorizer of type \
                 {AUTHORIZER_TYPE_REQUEST} on an API whose ProtocolType is {protocol_type}"
            ));
        }
    }
    if let Some(version) = payload_format_version {
        if !AUTHORIZER_PAYLOAD_FORMAT_VERSIONS.contains(&version) {
            return Err(format!(
                "AuthorizerPayloadFormatVersion must be one of {}, got {version}",
                AUTHORIZER_PAYLOAD_FORMAT_VERSIONS.join(", ")
            ));
        }
    }
    Ok(())
}

/// `Ok(())`, or why this route's authorization configuration cannot hold together —
/// the message for the operation's `BadRequestException`.
///
/// One rule: a route whose `AuthorizationType` hands the decision to an `Authorizer`
/// resource ([`AUTHORIZER_BACKED_AUTHORIZATION_TYPES`]) must be associated with one,
/// because `Route$AuthorizerId` is what names it and nothing else on the route does.
///
/// Shared by every writer of a route, so the requirement cannot differ between the
/// create and a later update; an update validates the MERGED record, so a route that
/// already names an authorizer may switch type without resending the id.
pub fn validate_route_authorization(
    authorization_type: &str,
    authorizer_id: Option<&str>,
) -> Result<(), String> {
    if AUTHORIZER_BACKED_AUTHORIZATION_TYPES.contains(&authorization_type)
        && authorizer_id.is_none_or(str::is_empty)
    {
        return Err(invalid_authorizer_reference_message(authorization_type));
    }
    Ok(())
}

/// The sentence this service refuses a route that does not name a valid authorizer
/// with, in the form for the `AuthorizationType` the request set.
///
/// The refusal is about the authorizer REFERENCE — a route of an authorizer-backed
/// type that names no authorizer at all is as unauthorizable as one naming an id no
/// authorizer carries — so the sentence states the requirement the type imposes and
/// echoes no id back. `JWT` is the one authorizer-backed `AuthorizationType` that names
/// an authorizer of its own kind (`Route$AuthorizationType`: "JWT for using JSON Web
/// Tokens", and `AUTHORIZER_TYPE_VALUES` carries `JWT`), so its sentence names that
/// kind; `CUSTOM` is "for using a Lambda authorizer", whose requirement is the one
/// [`UNRESOLVED_AUTHORIZER_MESSAGE`] states.
pub fn invalid_authorizer_reference_message(authorization_type: &str) -> String {
    if authorization_type == AUTHORIZER_TYPE_JWT {
        return format!(
            "Invalid authorizer ID specified. Setting the authorization type to \
             {AUTHORIZER_TYPE_JWT} requires a valid {AUTHORIZER_TYPE_JWT} authorizer."
        );
    }
    UNRESOLVED_AUTHORIZER_MESSAGE.to_string()
}

/// The CORS configuration an api carries — the wildcard entry, the protocol gate, the
/// grammar of an allowed origin and of an allowed method — lives in [`cors`], re-exported
/// in full so a handler reaches each name as `crate::state::…`.
pub use cors::{validate_cors_configuration, validate_cors_protocol, CORS_WILDCARD};

// Selection expressions — the `$`-variable expressions several members of this service
// carry, and what each member's protocol lets one select — live in
// [`selection_expression`], whose public items this module re-exports below.

/// `Ok(())`, or why this integration's payload format version cannot be stored — the
/// message for the operation's `BadRequestException`.
///
/// One rule, straight from `Integration$PayloadFormatVersion`: "Supported values for
/// Lambda proxy integrations are 1.0 and 2.0. For all other integrations, 1.0 is the
/// only supported value." A Lambda proxy integration is the `AWS_PROXY` type invoking a
/// function, so an `AWS_PROXY` integration that names an AWS service action through an
/// `IntegrationSubtype` instead is one of the "all other integrations".
pub fn validate_payload_format_version(
    integration_type: &str,
    integration_subtype: Option<&str>,
    payload_format_version: Option<&str>,
) -> Result<(), String> {
    let Some(version) = payload_format_version else {
        return Ok(());
    };
    let supported = if is_lambda_proxy_integration(integration_type, integration_subtype) {
        LAMBDA_PROXY_PAYLOAD_FORMAT_VERSIONS
    } else {
        &[ONLY_PAYLOAD_FORMAT_VERSION]
    };
    if !supported.contains(&version) {
        // API Gateway's own sentence for this refusal: fixed text naming the member, and
        // the value the request sent for it. The TYPE it is unsupported for is not in the
        // sentence, so the one wording serves every type.
        return Err(format!("Unsupported PayloadFormatVersion: {version}"));
    }
    Ok(())
}

/// The one `IntegrationType` that names no endpoint, from `INTEGRATION_TYPE_VALUES`:
/// "MOCK: for integrating the route or method request with API Gateway as a 'loopback'
/// endpoint without invoking any backend."
pub const MOCK_INTEGRATION_TYPE: &str = "MOCK";

/// The `ConnectionType` of a private integration, from `CONNECTION_TYPE_VALUES`: the
/// one that connects through a VPC link and so needs the link's id.
pub const VPC_LINK_CONNECTION_TYPE: &str = "VPC_LINK";

/// Every `IntegrationType` an api whose protocol is not WebSocket may be associated
/// with: the values of the modelled enum that [`WEBSOCKET_ONLY_INTEGRATION_TYPES`] does
/// not claim, in the enum's own order.
///
/// Those are exactly the two the member documents as proxy integrations — "also
/// referred to as a Lambda proxy integration" (`AWS_PROXY`) and "also referred to as the
/// HTTP proxy integration" (`HTTP_PROXY`) — which is what the refusal below calls them.
pub fn non_websocket_integration_types() -> Vec<&'static str> {
    wire::INTEGRATION_TYPE_VALUES
        .iter()
        .copied()
        .filter(|value| !WEBSOCKET_ONLY_INTEGRATION_TYPES.contains(value))
        .collect()
}

/// `Ok(())`, or why an api of this `ProtocolType` cannot be associated with an
/// integration of this `IntegrationType` — the message for the operation's
/// `BadRequestException`.
///
/// One rule: an api supports only the integration types its own protocol has behaviour
/// for, and the three the member's documentation marks "Supported only for WebSocket
/// APIs" ([`WEBSOCKET_ONLY_INTEGRATION_TYPES`]) are therefore not ones an api of any
/// other protocol may carry. The refusal names the types that api MAY be associated
/// with (see [`non_websocket_integration_types`]) rather than the value it refused, so
/// what the caller has to send instead is in the message.
///
/// Every writer of an integration routes through here — the create, the update and an
/// import — so the gate and its wording cannot differ between them.
pub fn validate_integration_type_for_protocol(
    protocol_type: &str,
    integration_type: &str,
) -> Result<(), String> {
    if protocol_type == WEBSOCKET_PROTOCOL_TYPE
        || !WEBSOCKET_ONLY_INTEGRATION_TYPES.contains(&integration_type)
    {
        return Ok(());
    }
    Err(format!(
        "Currently, an API with a protocol type of {protocol_type} may only be \
         associated with proxy integrations ({})",
        non_websocket_integration_types().join(", ")
    ))
}

/// `Ok(())`, or why this integration's configuration cannot hold together — the message
/// for the operation's `BadRequestException`.
///
/// The rules an integration RECORD has to satisfy, whatever wrote it, so a create and a
/// later update cannot disagree about them; an update checks the MERGED record, which is
/// the one the api would carry:
///
/// * an `IntegrationSubtype` names the AWS service action an AWS_PROXY integration
///   invokes ("Supported only for HTTP API AWS_PROXY integrations. Specifies the AWS
///   service action to invoke", `CreateIntegration$IntegrationSubtype` — the protocol
///   half of that clause is the protocol gate's, this is the type half);
/// * an `IntegrationUri` is needed only when the URI is what names what the integration
///   invokes: `MOCK` invokes nothing and a subtype names the action itself;
/// * the payload format version is the one the type supports (see
///   [`validate_payload_format_version`]);
/// * "The ID of the VPC link for a private integration" is what a `VPC_LINK` connection
///   connects through, and it means nothing without one;
/// * a template selection expression names only variables the service evaluates (see
///   [`validate_selection_expression`]).
pub fn validate_integration(
    integration_type: &str,
    integration_subtype: Option<&str>,
    integration_uri: Option<&str>,
    payload_format_version: Option<&str>,
    connection_type: &str,
    connection_id: Option<&str>,
    template_selection_expression: Option<&str>,
) -> Result<(), String> {
    if integration_subtype.is_some() && integration_type != AWS_PROXY_INTEGRATION_TYPE {
        return Err(format!(
            "IntegrationSubtype is supported only for integrations of type \
             {AWS_PROXY_INTEGRATION_TYPE}, and this integration's IntegrationType is \
             {integration_type}"
        ));
    }
    if integration_uri.is_none_or(str::is_empty)
        && integration_type != MOCK_INTEGRATION_TYPE
        && integration_subtype.is_none()
    {
        return Err(format!(
            "IntegrationUri is required for an integration of type {integration_type}."
        ));
    }
    if let Some(uri) = integration_uri.filter(|uri| !uri.is_empty()) {
        validate_integration_uri(integration_type, uri)?;
    }
    validate_payload_format_version(
        integration_type,
        integration_subtype,
        payload_format_version,
    )?;
    if connection_type == VPC_LINK_CONNECTION_TYPE && connection_id.is_none_or(str::is_empty) {
        return Err(format!(
            "ConnectionId must be set to vpcLinkId for ConnectionType {VPC_LINK_CONNECTION_TYPE}"
        ));
    }
    if let Some(expression) = template_selection_expression {
        validate_selection_expression(expression)?;
    }
    Ok(())
}

/// `Ok(())`, or why this `IntegrationUri` does not name what an integration of this
/// `IntegrationType` invokes — the message for the operation's `BadRequestException`.
///
/// Two rules. The LENGTH window the member's own shape documents
/// ([`INTEGRATION_URI_LEN`]) holds for every integration type, because the value is a URI
/// of that member whatever the backend is — and because the data plane resolves a stage
/// variable INTO this value under the same bound, so a template admitted here cannot grow
/// an upstream past what the member may hold.
///
/// Then: a type that integrates "with an AWS service action"
/// ([`AWS_RESOURCE_INTEGRATION_TYPES`]) names the resource that action runs on, so its
/// URI has to be a well-formed ARN — read through [`::service_common::arn::parse`], the
/// tree's one caller-supplied-ARN parse — and an ARN of AWS Lambda has to name the
/// `function:<name>` resource the invoking action runs on ([`target_names_a_function`],
/// the same reading a quick-create `Target` is admitted under). A URI that is neither is
/// not an address this integration could ever invoke, so it is refused rather than
/// persisted on an api that then looks deployable. The other types name an endpoint of
/// their own protocol, whose shape this leaves as it came.
pub fn validate_integration_uri(
    integration_type: &str,
    integration_uri: &str,
) -> Result<(), String> {
    validate_member_length("IntegrationUri", integration_uri, INTEGRATION_URI_LEN)?;
    if !AWS_RESOURCE_INTEGRATION_TYPES.contains(&integration_type) {
        return Ok(());
    }
    let Some(parsed) = ::service_common::arn::parse(integration_uri) else {
        return Err(INVALID_ARN_MESSAGE.to_string());
    };
    if parsed.vendor == LAMBDA_ARN_VENDOR && !target_names_a_function(integration_uri) {
        return Err(INVALID_ARN_MESSAGE.to_string());
    }
    Ok(())
}

/// How the sentence below names the route a malformed `Target` was sent for when the
/// service has no identifier for it: a CREATE has not minted a `RouteId` yet, and API
/// Gateway reports that absent identifier rather than leaving the sentence incomplete.
const UNNAMED_ROUTE: &str = "null";

/// The sentence API Gateway refuses a route `Target` that is not of the form
/// `integrations/{IntegrationId}` with, naming the route the target was sent for and the
/// form a target takes.
///
/// `route` is the `RouteId` of the route being written, which only an UPDATE has: a
/// create is refused before it mints one, so the sentence names no route
/// ([`UNNAMED_ROUTE`]).
pub fn malformed_route_target_message(route: Option<&str>) -> String {
    format!(
        "Unexpected or malformed target in route {}. Correct format should be \
         {INTEGRATION_TARGET_PREFIX}<integration_id>.",
        route.unwrap_or(UNNAMED_ROUTE)
    )
}

/// `Ok(())`, or why this route `Target` is not a target at all — the message for the
/// operation's `BadRequestException` ([`malformed_route_target_message`]).
///
/// One rule: a route's target ADDRESSES an integration of the route's own api, as
/// `integrations/{IntegrationId}` ([`INTEGRATION_TARGET_PREFIX`], the form
/// `CreateRoute$Target` takes and the one every route reader reports back), so a value of
/// any other FORM names nothing this service could route to. Checked by every writer of a
/// route before anything is persisted, and before the id it carries is resolved
/// ([`Api::unresolved_route_reference`]): the form of the reference is a property of the
/// request alone, while resolving it needs the api. A route the request gave no target at
/// all is not wired to an integration and is not refused — `Target` is an optional member
/// of the input (`smithy_query(query="operation", operation="CreateRoute")`).
pub fn validate_route_target(target: Option<&str>, route: Option<&str>) -> Result<(), String> {
    let Some(target) = target else {
        return Ok(());
    };
    let names_one_integration = target_integration_id(target)
        .is_some_and(|integration_id| !integration_id.is_empty() && !integration_id.contains('/'));
    if names_one_integration {
        return Ok(());
    }
    Err(malformed_route_target_message(route))
}

/// The name of the path variable a route-key path segment declares, or `None` for a
/// literal segment. `{id}` names `id` and the greedy `{proxy+}` names `proxy+`.
fn route_key_path_variable(segment: &str) -> Option<&str> {
    segment.strip_prefix('{')?.strip_suffix('}')
}

/// The method and the path segments of a route key of the `METHOD /path` form, or `None`
/// for a key of any other form — a WebSocket key (`$connect`, `$default`,
/// `sendmessage`) names no path, so it declares no path variables at all.
fn route_key_parts(route_key: &str) -> Option<(&str, Vec<&str>)> {
    let (method, path) = route_key.split_once(' ')?;
    let path = path.strip_prefix('/')?;
    Some((method, path.split('/').collect()))
}

/// `Ok(())`, or why this route key cannot live beside the api's existing ones — the
/// message for the operation's `BadRequestException`.
///
/// One rule: within a method's path space a position that is a path VARIABLE has ONE
/// name. Two route keys of the same method that agree on every earlier segment therefore
/// cannot declare differently named variables at the same position, because a request
/// matching both would have to bind two different path parameters at once. A literal
/// beside a variable is not a conflict (the more specific key simply wins), and the two
/// keys part company there, so nothing deeper can conflict either.
///
/// Shared by every writer of a route key, so the requirement cannot differ between the
/// create and a later update. A duplicate key is a different refusal, `ConflictException`.
pub fn validate_route_key_variables<'a>(
    route_key: &str,
    existing_keys: impl Iterator<Item = &'a str>,
) -> Result<(), String> {
    let Some((method, segments)) = route_key_parts(route_key) else {
        return Ok(());
    };
    for existing in existing_keys {
        let Some((other_method, other_segments)) = route_key_parts(existing) else {
            continue;
        };
        // One method's path space at a time: two methods are routed separately, so each
        // binds its own path parameters.
        if method != other_method {
            continue;
        }
        for (segment, other) in segments.iter().zip(other_segments.iter()) {
            match (
                route_key_path_variable(segment),
                route_key_path_variable(other),
            ) {
                (Some(name), Some(other_name)) if name != other_name => {
                    // The message is built from the request: the key it asked for and
                    // the key already on the api whose variable sits at that level.
                    return Err(format!(
                        "The provided route key \"{route_key}\" has a conflicting variable \
                         on the same hierarchical level as \"{existing}\""
                    ));
                }
                // The same variable at the same position, or the same literal: the keys
                // still share a path space, so keep comparing.
                (Some(_), Some(_)) => continue,
                _ if segment == other => continue,
                // A literal beside a variable, or two different literals: the keys part
                // company here.
                _ => break,
            }
        }
    }
    Ok(())
}

/// The sentence this service refuses a route whose `AuthorizerId` names no authorizer of
/// its api with.
///
/// The refusal is about the ASSOCIATION, not about the id — an id a route may not name is
/// one no authorizer of that api carries — so the sentence states the requirement rather
/// than echoing the id back.
pub const UNRESOLVED_AUTHORIZER_MESSAGE: &str = "Invalid authorizer ID specified. Setting \
    the authorization type to CUSTOM or COGNITO_USER_POOLS requires a valid authorizer.";

/// The sentence this service answers `NotFoundException` with for an identifier that
/// names no resource of `resource`'s kind.
///
/// API Gateway words a not-found PER KIND OF RESOURCE, and for many of the kinds
/// addressed within an api — a route, an integration, a deployment, a stage, an
/// authorizer, a VPC link — the sentence is this one: the resource's kind alone,
/// carrying neither the identifier the request already holds nor the account the api is
/// scoped to. The kinds whose sentence is NOT this form have one of their own beside
/// this function ([`ROUTE_RESPONSE_NOT_FOUND_MESSAGE`],
/// [`integration_response_not_found_message`], [`invalid_model_identifier_message`],
/// [`portal_product_not_found_message`], [`PRODUCT_PAGE_NOT_FOUND_MESSAGE`]), and the
/// API's own identifier is the one reported account-qualified, by
/// [`invalid_api_identifier_message`].
///
/// Shared by every reader and writer of a child resource, so one kind of resource
/// cannot be reported two ways, and the operation names the kind it ADDRESSED: a caller
/// reading the refusal learns which resource of the request resolved to nothing.
pub fn invalid_identifier_message(resource: &str) -> String {
    format!("Invalid {resource} identifier specified")
}

/// How [`invalid_identifier_message`] names a ROUTE of an api. API Gateway spells this
/// kind in lower case ("Invalid route identifier specified"), where it capitalises
/// others — the sentence is per kind, so each kind's spelling lives in one place and
/// every operation that addresses a route reports the same one.
pub const ROUTE_RESOURCE: &str = "route";

/// The sentence this service answers `NotFoundException` with for a `RouteResponseId`
/// the addressed route holds no response under — including one a `DeleteRouteResponse`
/// has already removed.
///
/// A route response is reported by the RESOURCE it addressed rather than by the
/// identifier it was addressed with, so the sentence is a constant for the whole class:
/// it carries neither the id the request already holds nor the account — the wording
/// AWS's own answer for an unresolved route response carries. Held here once, so the
/// reader, the writer and the delete of one route response cannot word it three ways.
pub const ROUTE_RESPONSE_NOT_FOUND_MESSAGE: &str = "Invalid route response specified";

/// The sentence this service answers `NotFoundException` with for a `RouteKey` the
/// addressed stage holds no route settings under — including one a `DeleteRouteSettings`
/// has already removed.
///
/// The route settings of one route are addressed by the ROUTE KEY they are stored under
/// (`routesettings/{RouteKey}`), and the sentence states exactly that: it names the kind
/// of resource that resolved to nothing and quotes the key the request addressed it with,
/// so a caller can tell "this stage carries no settings for that key" from a refusal of
/// the key itself. The key is the REQUEST's — the template is the sentence, the value is
/// each call's own.
pub fn route_settings_not_found_message(route_key: &str) -> String {
    format!("The settings for the Route with key '{route_key}' does not exist")
}

/// How [`invalid_identifier_message`] names a DEPLOYMENT of an api ("Invalid Deployment
/// identifier specified").
pub const DEPLOYMENT_RESOURCE: &str = "Deployment";

/// How [`invalid_identifier_message`] names an INTEGRATION of an api.
pub const INTEGRATION_RESOURCE: &str = "Integration";

/// The sentence this service answers `NotFoundException` with for an
/// `IntegrationResponseId` the addressed integration holds no response under —
/// including one a `DeleteIntegrationResponse` has already removed.
///
/// An integration response is resolvable only THROUGH the integration that owns it, and
/// the sentence AWS answers states exactly that: it names both ids the request addressed
/// the response with, so a caller learns which pair resolved to nothing. The ids are the
/// REQUEST's — the template is the sentence, the values are each call's own.
pub fn integration_response_not_found_message(
    integration_response_id: &str,
    integration_id: &str,
) -> String {
    format!(
        "IntegrationResponse with ID {integration_response_id} is not present in \
         integration with ID {integration_id}"
    )
}

/// How [`invalid_identifier_message`] names a STAGE of an api, which API Gateway spells
/// in lower case.
pub const STAGE_RESOURCE: &str = "stage";

/// How [`invalid_model_identifier_message`] names a MODEL of an api. API Gateway spells
/// this kind in lower case, where it capitalises others — the sentence is per kind, so
/// each kind's spelling lives in one place.
pub const MODEL_RESOURCE: &str = "model";

/// The sentence this service answers `NotFoundException` with for a `ModelId` the
/// addressed api holds no model under — including one a `DeleteModel` has already
/// removed.
///
/// A model's sentence is [`invalid_identifier_message`]'s, EXTENDED with the identifier
/// the request supplied: that id is the only thing the caller can act on, since no model
/// of that api is addressable by it. Shared by every operation that addresses one model,
/// so `GetModel` and `GetModelTemplate` cannot word the same miss two ways.
pub fn invalid_model_identifier_message(model_id: &str) -> String {
    format!(
        "{}: {model_id}",
        invalid_identifier_message(MODEL_RESOURCE)
    )
}

/// How [`invalid_identifier_message`] names an AUTHORIZER of an api ("Invalid Authorizer
/// identifier specified").
pub const AUTHORIZER_RESOURCE: &str = "Authorizer";

/// The sentence this service answers `NotFoundException` with for a `PortalId` the account
/// holds no portal under.
///
/// A portal is one of this API's PORTAL-plane resources, and those report an identifier
/// that resolves to nothing by naming the resource TYPE alone — the form
/// [`PRODUCT_REST_ENDPOINT_PAGE_NOT_FOUND_MESSAGE`] also takes — rather than with the
/// `Invalid {resource} identifier specified` sentence the older api-level resources use
/// (see [`invalid_identifier_message`]). So the sentence carries neither the account nor
/// the identifier the request already holds. Held here once, so the wording cannot drift
/// between the readers, the writers and the delete of one portal.
pub const PORTAL_NOT_FOUND_MESSAGE: &str = "Portal not found";

/// The sentence this service answers `NotFoundException` with for a `PortalProductId` the
/// account holds no product under — including one a `DeletePortalProduct` has already
/// removed.
///
/// A portal product is an ACCOUNT-level resource, so API Gateway reports an identifier
/// that resolves to nothing account-qualified, the way it reports an unknown `ApiId`
/// (see [`invalid_api_identifier_message`]): the account the lookup was made in, and the
/// id the request supplied. Shared by every operation that addresses one product —
/// its readers and writers, its delete, and the operations that work UNDER it — so the
/// wording cannot drift between them.
pub fn portal_product_not_found_message(portal_product_id: &str) -> String {
    format!(
        "Product not found in account {} with id {portal_product_id}",
        ::engine::emulator_account_id()
    )
}

/// The sentence this service answers `NotFoundException` with for a product page that
/// resolves to nothing — an id the addressed product holds no page under (including one
/// a `DeleteProductPage` has already removed), or a product that is itself unknown.
///
/// A product page is one of this API's PORTAL-plane resources, and those report an
/// unresolved identifier by naming the resource TYPE alone — the form
/// [`PORTAL_NOT_FOUND_MESSAGE`] and [`PRODUCT_REST_ENDPOINT_PAGE_NOT_FOUND_MESSAGE`]
/// also take — so the sentence carries neither the account nor the ids the request
/// already holds. This is the whole sentence AWS's own answer for an unresolved product
/// page carries: fixed text with no variable part, as a recorded read of a page that had
/// just been deleted shows.
pub const PRODUCT_PAGE_NOT_FOUND_MESSAGE: &str = "ProductPage not found";

/// How [`invalid_identifier_message`] names a VPC LINK ("Invalid Vpc Link identifier
/// specified"). The casing is the service's own in THIS sentence — each word
/// capitalised, not the `VPC link` of its prose — so do not "correct" it to match the
/// documentation.
pub const VPC_LINK_RESOURCE: &str = "Vpc Link";

/// The sentence this service answers `NotFoundException` with for an `ApiId` no api
/// holds: the account-qualified form API Gateway reports an unknown API identifier in
/// ("Invalid API identifier specified {account}:{ApiId}"), because an `ApiId` is
/// addressed within an account rather than within another resource.
pub fn invalid_api_identifier_message(api_id: &str) -> String {
    format!(
        "Invalid API identifier specified {}:{api_id}",
        ::engine::emulator_account_id()
    )
}

/// The `IntegrationId` a route target names, when it addresses an integration of the
/// same api (`integrations/{IntegrationId}`). `None` for a target of any other form — a
/// target this service does not resolve to an integration references none of them.
pub fn target_integration_id(target: &str) -> Option<&str> {
    target.strip_prefix(INTEGRATION_TARGET_PREFIX)
}

/// The sentence this service refuses a quick-create `Target` that names neither of the
/// two backends quick create can proxy to.
///
/// Quick create builds ONE integration, and its type follows the form of the target: "For
/// HTTP integrations, specify a fully qualified URL. For Lambda integrations, specify a
/// function ARN. The type of the integration will be HTTP_PROXY or AWS_PROXY,
/// respectively" (`CreateApi$Target`). A value of any other form selects neither, so
/// there is no integration to build and the request is refused rather than stored with a
/// backend nothing could call.
pub const TARGET_IS_NOT_A_PROXY: &str = "Target only supports HTTP Proxy or Lambda Proxy";

/// The two schemes a quick-create `Target` names an HTTP proxy upstream with: a fully
/// qualified URL of the protocol an `HTTP_PROXY` integration speaks.
const HTTP_TARGET_SCHEMES: &[&str] = &["http://", "https://"];

/// The ARN namespace of AWS Lambda, whose FUNCTION a quick-create `Target` names to select
/// an `AWS_PROXY` integration.
const LAMBDA_ARN_VENDOR: &str = "lambda";

/// The resource portion of a Lambda FUNCTION ARN
/// (`arn:<partition>:lambda:<region>:<account>:function:<name>[:<qualifier>]`), which is
/// the only Lambda resource an integration invokes.
const LAMBDA_FUNCTION_RESOURCE_PREFIX: &str = "function:";

/// Does `target` name one of the two backends quick create proxies to
/// ([`TARGET_IS_NOT_A_PROXY`])?
///
/// The two forms are the ones [`target_names_a_function`] and
/// [`target_names_an_http_upstream`] decide, so the refusal and the integration TYPE a
/// target selects are read off one rule: a target this answers `true` for is one the
/// writers can build an `AWS_PROXY` or an `HTTP_PROXY` integration from, and no other
/// value is stored.
pub fn target_selects_a_proxy(target: &str) -> bool {
    target_names_an_http_upstream(target) || target_names_a_function(target)
}

/// Does `target` name a fully qualified URL, the form that selects an `HTTP_PROXY`
/// integration ("For HTTP integrations, specify a fully qualified URL … HTTP_PROXY",
/// `CreateApi$Target`)?
///
/// One carrying one of [`HTTP_TARGET_SCHEMES`] and a host after it; a scheme with nothing
/// after it addresses no upstream.
pub fn target_names_an_http_upstream(target: &str) -> bool {
    HTTP_TARGET_SCHEMES
        .iter()
        .find_map(|scheme| target.strip_prefix(scheme))
        .is_some_and(|host| !host.is_empty() && !host.starts_with('/'))
}

/// Does `target` name a Lambda FUNCTION, the form that selects an `AWS_PROXY` integration
/// ("For Lambda integrations, specify a function ARN … AWS_PROXY", `CreateApi$Target`)?
///
/// A well-formed ARN of AWS Lambda naming a `function:<name>` resource, read through
/// [`::service_common::arn::parse_of`], the tree's one caller-supplied-ARN parse. Every
/// writer of a quick-create target asks this both to admit the value and to pick the
/// integration type it builds, so the two cannot disagree about which form a target is.
pub fn target_names_a_function(target: &str) -> bool {
    ::service_common::arn::parse_of(target, LAMBDA_ARN_VENDOR).is_some_and(|parsed| {
        parsed
            .resource
            .strip_prefix(LAMBDA_FUNCTION_RESOURCE_PREFIX)
            .is_some_and(|name| !name.is_empty())
    })
}

/// Is `name` a `Model$Name` the service accepts?
///
/// "The name of the model. Must be alphanumeric" (`CreateModel$Name`), so a name
/// carrying anything but letters and digits is refused. The length bounds (1..128) are
/// the modelled constraint and are already refused before a handler runs, so this is
/// only the alphabet.
///
/// `CreateModel` and `UpdateModel` both route through here: the requirement is a
/// property of the request, so it cannot differ between the two.
pub fn model_name_is_alphanumeric(name: &str) -> bool {
    !name.is_empty() && name.chars().all(|character| character.is_ascii_alphanumeric())
}

/// Does `content_type` name a media type whose schema language is JSON Schema?
///
/// A JSON media type is one whose subtype is `json` or ends in `+json`
/// (RFC 6839's `+json` structured syntax suffix), with any parameters
/// (`; charset=utf-8`) ignored. A model that names NO content type is a JSON model
/// too: `application/json` is the content type the service's own documentation gives
/// for a model (`Model$ContentType`: "for example, \"application/json\"").
pub fn is_json_media_type(content_type: Option<&str>) -> bool {
    let Some(content_type) = content_type else {
        return true;
    };
    let media_type = content_type
        .split(';')
        .next()
        .unwrap_or_default()
        .trim()
        .to_ascii_lowercase();
    let subtype = media_type.split('/').nth(1).unwrap_or_default();
    subtype == "json" || subtype.ends_with("+json")
}

/// Refuse a `Schema` that is not a schema document of the model's content type.
///
/// For a JSON model (see [`is_json_media_type`]) the schema is a JSON Schema document:
/// "For application/json models, this should be JSON schema draft 4 model"
/// (`CreateModel$Schema`), and "A JSON Schema is a JSON document, and that document
/// MUST be an object" (JSON Schema draft-04, section 3.2). A schema that does not
/// parse as JSON, or parses as a value that is not an object, is therefore no JSON
/// Schema at all and nothing reading the model could treat it as one.
///
/// A model of any OTHER content type carries a schema in that content type's own
/// language, which this service has no reader for, so its schema is stored verbatim
/// rather than refused for not being JSON.
///
/// What this does NOT do is check a document against the whole of draft-04 — that
/// every keyword is one draft-04 defines and carries a value of the type draft-04
/// gives it. A partial schema is legitimate input (`{}` matches everything), and the
/// document is handed back verbatim by every reader of the model.
///
/// `Ok(())` or the message for the operation's `BadRequestException`, which is the one
/// sentence the whole class is refused with (see [`INVALID_MODEL_SCHEMA_MESSAGE`]).
pub fn validate_model_schema(content_type: Option<&str>, schema: &str) -> Result<(), String> {
    if !is_json_media_type(content_type) {
        return Ok(());
    }
    let document = ::serde_json::from_str::<::serde_json::Value>(schema).ok();
    if document.is_some_and(|document| document.is_object()) {
        return Ok(());
    }
    Err(INVALID_MODEL_SCHEMA_MESSAGE.to_string())
}

/// The sentence API Gateway refuses a `Schema` that is no schema document with.
///
/// The service answers the RESULT of validating the model, and a document it could not
/// read as a schema yields that result's one error and no warnings. It is one sentence
/// for the whole class: it names nothing from the request, so neither the schema nor any
/// reader's diagnostic (a parser's position, the JSON type the document turned out to
/// be) is echoed back, and the refusal a caller sees does not vary with either.
pub const INVALID_MODEL_SCHEMA_MESSAGE: &str = "Invalid model specified: Validation \
    Result: warnings : [], errors : [Invalid model schema specified]";

/// A persisted `Stage` resource, owned by its parent [`Api`].
///
/// The record keeps what the create resolved — the name it is keyed by, the instants
/// it was created and last changed at, and the settings maps the request supplied — so
/// every reader of the stage reports one value, and [`Stage::to_wire`] projects it onto
/// the modelled shape the handlers serialize.
#[derive(Debug, Clone)]
pub struct Stage {
    pub stage_name: String,
    pub description: Option<String>,
    /// Whether the caller CONFIGURED auto-deploy on this stage, and what they set it to —
    /// `None` until a request supplies a value.
    ///
    /// `Stage$AutoDeploy` is neither `@required` nor `@default` (`smithy_query(query="shape",
    /// shape="Stage")`), so the stage reports it only once a request has set it: the
    /// presence of the member is what tells "auto-deploy was never configured here" from
    /// "it was configured and turned off", and a round-trip that echoes a read back into
    /// `UpdateStage` must not write a setting the caller never made. Its documented default
    /// ("The default value is false", `CreateStage$AutoDeploy`) governs BEHAVIOUR only —
    /// see [`Stage::auto_deploys`] — never what is stored or reported.
    pub auto_deploy: Option<bool>,
    pub tags: BTreeMap<String, String>,
    /// The `DeploymentId` this stage serves, set by the create that deployed to it:
    /// "A Deployment must be associated with a Stage for it to be callable over the
    /// internet" (the `Deployment` shape's own doc).
    pub deployment_id: Option<String>,
    pub access_log_settings: Option<wire::AccessLogSettings>,
    /// "Supported only for WebSocket APIs" (`Stage$ClientCertificateId`).
    pub client_certificate_id: Option<String>,
    pub default_route_settings: Option<wire::RouteSettings>,
    /// Route settings by `routeKey`. `BTreeMap`, so the stage reports them in one
    /// stable order however the request ordered them.
    pub route_settings: BTreeMap<String, wire::RouteSettings>,
    /// The stage variable map, by variable name.
    pub stage_variables: BTreeMap<String, String>,
    /// "Describes the status of the last deployment of a stage. Supported only for
    /// stages with autoDeploy enabled" (`Stage$LastDeploymentStatusMessage`), so a
    /// stage that never auto-released one has none.
    pub last_deployment_status_message: Option<String>,
    pub created_date: SystemTime,
    /// "The timestamp when the stage was last updated" (`Stage$LastUpdatedDate`): the
    /// creation instant until something rewrites the record.
    pub last_updated_date: SystemTime,
    /// True only for a stage another operation created on the caller's behalf: "If you
    /// created an API using quick create, the $default stage is managed by API Gateway"
    /// (`Stage$ApiGatewayManaged`). A stage the caller asked for is its own.
    pub api_gateway_managed: bool,
}

impl Stage {
    /// Whether this stage auto-deploys, which is what the stored value MEANS to the rest
    /// of the service — [`auto_deploys`] applied to this record's own setting.
    pub fn auto_deploys(&self) -> bool {
        auto_deploys(self.auto_deploy)
    }

    /// Project the record onto the modelled `Stage` shape.
    pub fn to_wire(&self) -> wire::Stage {
        wire::Stage {
            access_log_settings: self.access_log_settings.clone(),
            api_gateway_managed: Some(self.api_gateway_managed),
            auto_deploy: self.auto_deploy,
            client_certificate_id: self.client_certificate_id.clone(),
            created_date: Some(to_timestamp(self.created_date)),
            default_route_settings: self.default_route_settings.clone(),
            deployment_id: self.deployment_id.clone(),
            description: self.description.clone(),
            last_deployment_status_message: self.last_deployment_status_message.clone(),
            last_updated_date: Some(to_timestamp(self.last_updated_date)),
            route_settings: Some(
                self.route_settings
                    .iter()
                    .map(|(key, settings)| (key.clone(), settings.clone()))
                    .collect(),
            ),
            stage_name: self.stage_name.clone(),
            stage_variables: Some(
                self.stage_variables
                    .iter()
                    .map(|(name, value)| (name.clone(), value.clone()))
                    .collect(),
            ),
            tags: Some(self.tags.iter().map(|(k, v)| (k.clone(), v.clone())).collect()),
        }
    }
}

/// The most stages one api may hold, after which CreateStage is refused. AWS's
/// published API Gateway quota is 10 stages per API.
pub const MAX_STAGES_PER_API: usize = 10;

/// Whether a stage whose `AutoDeploy` is `configured` auto-deploys — the ONE place the
/// documented default of that member lives.
///
/// "Specifies whether updates to an API automatically trigger a new deployment. The
/// default value is false" (`CreateStage$AutoDeploy`): a stage no request configured
/// behaves as one that does not auto-deploy, without the record claiming the caller said
/// so. Every decision that turns on auto-deploy reads this, so the default governs
/// behaviour in one spelling and never reaches what is stored or reported.
pub fn auto_deploys(configured: Option<bool>) -> bool {
    configured.unwrap_or(false)
}

/// The `LoggingLevel` of a route that pushes no entries to CloudWatch Logs, from
/// `LOGGING_LEVEL_VALUES` (`ERROR | INFO | OFF`): "Specifies the logging level for this
/// route: INFO, ERROR, or OFF" (`RouteSettings$LoggingLevel`).
pub const LOGGING_LEVEL_OFF: &str = "OFF";

/// The `DefaultRouteSettings` a stage of an api of this `ProtocolType` carries, resolving
/// MEMBER BY MEMBER whatever the request configured (`None` for a request that configured
/// nothing at all).
///
/// Every stage has route settings — they are what its routes are served under — so each
/// member of them whose value the request did not name is the service's to supply, not a
/// member the stage simply lacks: every facility `RouteSettings` switches is OFF, which is
/// `DataTraceEnabled` and `DetailedMetricsEnabled` false and the logging level that pushes
/// no log entries ([`LOGGING_LEVEL_OFF`]). The two members documented "Supported only for
/// WebSocket APIs" (`RouteSettings$DataTraceEnabled` and `RouteSettings$LoggingLevel` —
/// the same clause [`validate_protocol_restricted_members`] refuses them under for an api
/// of the other protocol) are carried only by a stage of the protocol that supports them.
/// The throttling members name a LIMIT rather than a facility, and a stage that named none
/// is not throttled at a figure the service could report, so they are left as they came.
pub fn resolve_route_settings(
    protocol_type: &str,
    configured: Option<wire::RouteSettings>,
) -> wire::RouteSettings {
    let is_websocket = protocol_type == WEBSOCKET_PROTOCOL_TYPE;
    let configured = configured.unwrap_or_default();
    wire::RouteSettings {
        data_trace_enabled: configured
            .data_trace_enabled
            .or_else(|| is_websocket.then_some(false)),
        detailed_metrics_enabled: configured.detailed_metrics_enabled.or(Some(false)),
        logging_level: configured
            .logging_level
            .or_else(|| is_websocket.then(|| LOGGING_LEVEL_OFF.to_string())),
        throttling_burst_limit: configured.throttling_burst_limit,
        throttling_rate_limit: configured.throttling_rate_limit,
    }
}

/// The sentence API Gateway refuses a `CreateStage` naming a stage its api already
/// holds with.
///
/// A stage is identified by its name within its api, so the conflict is that the name
/// is taken; the sentence states that and names nothing from the request, so it is the
/// same for every name.
pub const STAGE_ALREADY_EXISTS_MESSAGE: &str = "Stage already exists";

/// The `StageName` of the stage that serves an api's root: "the $default stage"
/// (`Stage$ApiGatewayManaged`), the one name that is not a plain stage name.
pub const DEFAULT_STAGE_NAME: &str = "$default";

/// The longest `StageName` the service accepts: the upper bound of
/// `StringWithLengthBetween1And128`, the shape `CreateStage$StageName` targets — the
/// same window an api's `Name` falls in ([`API_NAME_LEN`]).
pub const MAX_STAGE_NAME_LEN: usize = API_NAME_LEN.1;

/// The longest stage variable VALUE the service accepts: the upper bound of
/// `StringWithLengthBetween0And2048`, the value shape of `StageVariablesMap`
/// (`smithy_query(query="shape", shape="StageVariablesMap")`).
pub const MAX_STAGE_VARIABLE_VALUE_LEN: usize = 2048;

/// Refuse a `StageName` the service does not accept.
///
/// "Stage names can only contain alphanumeric characters, hyphens, and underscores.
/// Maximum length is 128 characters" (the `CreateStage$StageName` API reference), plus
/// [`DEFAULT_STAGE_NAME`], the reserved name of the stage that serves an api's root.
/// The length bound of `StringWithLengthBetween1And128` carries no `@length` trait in
/// the model, so the request walk does not enforce it and this does.
///
/// `Ok(())` or the message for the operation's `BadRequestException`: it names the
/// member and the rule it broke.
pub fn validate_stage_name(stage_name: &str) -> Result<(), String> {
    if stage_name == DEFAULT_STAGE_NAME {
        return Ok(());
    }
    validate_member_length("StageName", stage_name, API_NAME_LEN)?;
    if !stage_name
        .chars()
        .all(|character| character.is_ascii_alphanumeric() || character == '-' || character == '_')
    {
        return Err(format!(
            "Invalid value for StageName: stage names can only contain alphanumeric \
             characters, hyphens and underscores, or be the reserved name \
             {DEFAULT_STAGE_NAME}, got '{stage_name}'."
        ));
    }
    Ok(())
}

/// Refuse a stage variable whose name or value is not one the service accepts.
///
/// "Variable names can have alphanumeric and underscore characters, and the values
/// must match [A-Za-z0-9-._~:/?#&=,]+" (`Stage$StageVariables`), and a value is bounded
/// by [`MAX_STAGE_VARIABLE_VALUE_LEN`], the upper bound of the map's value shape.
/// Neither is a modelled trait, so the request walk does not enforce them and this
/// does.
///
/// `Ok(())` or the message for the operation's `BadRequestException`: it names the
/// variable and the rule it broke.
pub fn validate_stage_variables<'a, I>(variables: I) -> Result<(), String>
where
    I: IntoIterator<Item = (&'a String, &'a String)>,
{
    for (name, value) in variables {
        if name.is_empty()
            || !name
                .chars()
                .all(|character| character.is_ascii_alphanumeric() || character == '_')
        {
            return Err(format!(
                "Invalid value for StageVariables: variable names can have alphanumeric \
                 and underscore characters, got '{name}'."
            ));
        }
        let length = value.chars().count();
        if length > MAX_STAGE_VARIABLE_VALUE_LEN {
            return Err(format!(
                "Invalid value for StageVariables: the value of '{name}' must have length \
                 less than or equal to {MAX_STAGE_VARIABLE_VALUE_LEN}, got {length}."
            ));
        }
        if value.is_empty() || !value.chars().all(is_stage_variable_value_character) {
            return Err(format!(
                "Invalid value for StageVariables: the value of '{name}' must match \
                 [A-Za-z0-9-._~:/?#&=,]+."
            ));
        }
    }
    Ok(())
}

/// Is `character` one the stage variable value pattern `[A-Za-z0-9-._~:/?#&=,]`
/// admits (`Stage$StageVariables`)?
fn is_stage_variable_value_character(character: char) -> bool {
    character.is_ascii_alphanumeric()
        || matches!(
            character,
            '-' | '.' | '_' | '~' | ':' | '/' | '?' | '#' | '&' | '=' | ','
        )
}

/// The `$context` variable an access log format must select, so an access log entry can
/// be tied to the request that produced it: "The format must include at least
/// $context.requestId" (`AccessLogSettings$Format`).
pub const REQUIRED_ACCESS_LOG_CONTEXT_VARIABLE: &str = "$context.requestId";

/// Refuse `AccessLogSettings` whose `Format` could not identify the request it logged.
///
/// "The format must include at least $context.requestId"
/// (`AccessLogSettings$Format`), so a format selecting no request id is one no access
/// log entry could be traced back to a request with. A settings block that names no
/// format at all configures no access logging, so there is no format to require
/// anything of.
///
/// Every operation that stores access log settings routes through here: the requirement
/// is a property of the settings, so it cannot differ between them.
///
/// `Ok(())` or the message for the operation's `BadRequestException`: it names the
/// member and the rule it broke.
pub fn validate_access_log_settings(settings: &wire::AccessLogSettings) -> Result<(), String> {
    let Some(format) = settings.format.as_ref() else {
        return Ok(());
    };
    if format.contains(REQUIRED_ACCESS_LOG_CONTEXT_VARIABLE) {
        return Ok(());
    }
    Err(format!(
        "Invalid value for AccessLogSettings: the Format must include at least \
         {REQUIRED_ACCESS_LOG_CONTEXT_VARIABLE}, got '{format}'."
    ))
}

/// A persisted `Deployment` resource, owned by its parent [`Api`].
///
/// "An immutable representation of an API that can be called by users" (the
/// `Deployment` shape doc), so nothing but its description is ever rewritten.
#[derive(Debug, Clone)]
pub struct Deployment {
    pub deployment_id: String,
    pub description: Option<String>,
    /// One of `DEPLOYMENT_STATUS_VALUES`.
    pub deployment_status: String,
    pub deployment_status_message: Option<String>,
    /// True only for a deployment the service released by itself, for a stage whose
    /// `AutoDeploy` is on: "Specifies whether a deployment was automatically released"
    /// (`Deployment$AutoDeployed`). A deployment the caller asked for is not one.
    pub auto_deployed: bool,
    pub created_date: SystemTime,
    /// The api configuration this deployment froze when it was released
    /// ([`Api::deployable_configuration`]): the routes and integrations a stage serving
    /// this deployment serves. Modelled on no member of the `Deployment` shape — a
    /// deployment reports its id, status and date — so no reader projects it; it is what
    /// makes the deployment "immutable" rather than a pointer at the api's current state.
    pub configuration: DeployedConfiguration,
}

/// The api configuration one [`Deployment`] froze: the routes and integrations the api
/// held when that deployment was released, each with the responses that belong to it.
///
/// Two stages of one api can therefore serve two different configurations of it — the one
/// each one's own deployment froze — which is what pinning a stage to a deployment is for.
#[derive(Debug, Clone, Default)]
pub struct DeployedConfiguration {
    /// The routes the deployment froze, keyed by `RouteId` exactly as [`Api::routes`] keys
    /// them.
    pub routes: BTreeMap<String, Route>,
    /// The integrations the deployment froze, keyed by `IntegrationId` exactly as
    /// [`Api::integrations`] keys them.
    pub integrations: BTreeMap<String, Integration>,
}

impl Deployment {
    /// Project the record onto the modelled `Deployment` shape.
    pub fn to_wire(&self) -> wire::Deployment {
        wire::Deployment {
            auto_deployed: Some(self.auto_deployed),
            created_date: Some(to_timestamp(self.created_date)),
            deployment_id: Some(self.deployment_id.clone()),
            deployment_status: Some(self.deployment_status.clone()),
            deployment_status_message: self.deployment_status_message.clone(),
            description: self.description.clone(),
        }
    }
}

/// The most portals one account may hold, after which CreatePortal is refused.
///
/// An EMULATOR growth bound, not a figure AWS publishes: API Gateway publishes
/// per-account quotas for its other top-level resources (600 HTTP/WebSocket APIs, see
/// [`MAX_APIS`]) but none for portals, so this takes a smaller bound of its own. What
/// it buys is that no loop of CreatePortal calls can grow this store without end.
/// `TooManyRequestsException` ("A limit has been exceeded") is the modelled error the
/// operation refuses with. Do not cite this as a published portals quota.
pub const MAX_PORTALS: usize = 100;

/// The most tombstones of deleted portals this store keeps, after which the
/// longest-deleted is dropped ([`ComAmazonawsApigatewayv2Apigatewayv2State::delete_portal`]).
///
/// A deleted portal stays READABLE in its terminal status ([`PUBLISH_STATUS_DELETED`]), so
/// a delete keeps the record rather than dropping it — and a loop of create-and-delete
/// would otherwise grow this store without end, which is the growth [`MAX_PORTALS`] bounds
/// for live portals. An EMULATOR bound, not a retention AWS publishes: it is set to the
/// number of portals the account can hold at all, so every portal of a full account can be
/// deleted and still read back.
pub const MAX_PORTAL_TOMBSTONES: usize = MAX_PORTALS;

/// The most portal products one portal may INCLUDE, after which a write that would store
/// more is refused.
///
/// An EMULATOR growth bound, not a figure AWS publishes: `IncludedPortalProductArns` is
/// modelled as an unconstrained list, and every entry of it is rendered into the page the
/// portal's domain serves ([`crate::service`]'s portal plane), so an uncapped list is an
/// uncapped response built from one small request. It is set to the number of products this
/// store can hold at all ([`MAX_PORTAL_PRODUCTS`]), which is the most a portal could include
/// of this account's own products; a portal including products another account shared is
/// bounded by the same figure rather than by what this store holds.
/// `TooManyRequestsException` ("A limit has been exceeded") is the modelled error the
/// writers refuse with. Do not cite this as a published quota.
pub const MAX_INCLUDED_PORTAL_PRODUCTS: usize = MAX_PORTAL_PRODUCTS;

/// Refuse a list of included portal products that would grow a portal past
/// [`MAX_INCLUDED_PORTAL_PRODUCTS`]. `Err` carries the message for the writer's
/// `TooManyRequestsException`, which also names the limit in its `LimitType`.
pub fn validate_included_portal_product_count(arns: &[String]) -> Result<(), String> {
    if arns.len() <= MAX_INCLUDED_PORTAL_PRODUCTS {
        return Ok(());
    }
    Err(format!(
        "The maximum number of portal products ({MAX_INCLUDED_PORTAL_PRODUCTS}) a portal \
         may include has been reached, got {}.",
        arns.len()
    ))
}

/// The `PublishStatus` a portal carries while it serves nothing: the value
/// [`Portal`] is created with, since a portal that has never been published is not
/// serving its content to anyone.
///
/// `PublishStatus` is exactly `PUBLISHED | PUBLISH_IN_PROGRESS | PUBLISH_FAILED |
/// DISABLE_IN_PROGRESS | DISABLE_FAILED | DISABLED`
/// (`smithy_query(query="shape", shape="PublishStatus")`) — it has no "never published"
/// value, and `DISABLED` is its one terminal not-serving value, the status a portal also
/// settles in once `DisablePortal` has run.
pub const PUBLISH_STATUS_DISABLED: &str = "DISABLED";

/// The `PublishStatus` a portal whose publish FAILED reports, out of
/// `PUBLISH_STATUS_VALUES`. Nothing in this emulator fails a build, so no portal is ever
/// left in it; it is named because it is one of the statuses a publication can be taken
/// down from ([`PORTAL_DISABLE_FROM_STATUSES`]).
pub const PUBLISH_STATUS_PUBLISH_FAILED: &str = "PUBLISH_FAILED";

/// The `PublishStatus` a portal whose disable FAILED reports, out of
/// `PUBLISH_STATUS_VALUES`. Named for the same reason as
/// [`PUBLISH_STATUS_PUBLISH_FAILED`]: a failed take-down can be retried.
pub const PUBLISH_STATUS_DISABLE_FAILED: &str = "DISABLE_FAILED";

/// The `PublishStatus` a portal reports once `DeletePortal` has taken it away: the
/// terminal status of a portal that no longer exists.
///
/// NOT one of `PUBLISH_STATUS_VALUES` — the modelled `PublishStatus` enum does not declare
/// it — but the value AWS answers for a deleted portal, whose body it keeps serving so a
/// caller can confirm the deletion by reading the terminal status. The model is taken as
/// incomplete here rather than as a refutation: a reader that only accepts the modelled
/// catalog would reject AWS's own answer.
pub const PUBLISH_STATUS_DELETED: &str = "DELETED";

/// The `PublishStatus` values a portal's publication can be taken DOWN from, in the order
/// [`PORTAL_DISABLE_PRECONDITION_MESSAGE`] names them: there is a publication to remove
/// ([`PUBLISH_STATUS_PUBLISHED`]), or a build that was trying to put one up or take one
/// down ended in failure, which a retry may clear.
///
/// A status not in this set means the service is mid-build or the portal publishes nothing,
/// so `DisablePortal` is refused `ConflictException` ("the requested operation would cause
/// a conflict with the current state of a service resource associated with the request").
pub const PORTAL_DISABLE_FROM_STATUSES: &[&str] = &[
    PUBLISH_STATUS_PUBLISHED,
    PUBLISH_STATUS_PUBLISH_FAILED,
    PUBLISH_STATUS_DISABLE_FAILED,
];

/// The sentence `DisablePortal` refuses a portal that is in no status it can be disabled
/// from with. FIXED text: it names the statuses of [`PORTAL_DISABLE_FROM_STATUSES`] and
/// carries nothing of the request or of the portal's own state.
pub const PORTAL_DISABLE_PRECONDITION_MESSAGE: &str =
    "Portal must be PUBLISHED, PUBLISH_FAILED, or DISABLE_FAILED to disable";

/// The `PublishStatus` of a portal that is serving its content, out of
/// `PUBLISH_STATUS_VALUES`: the value a portal settles in once the publish
/// `PublishPortal` started has come due.
pub const PUBLISH_STATUS_PUBLISHED: &str = "PUBLISHED";

/// The `PublishStatus` a portal reports while the publish `PublishPortal` started is
/// still being built, out of `PUBLISH_STATUS_VALUES`. Not a terminal value: the portal
/// leaves it for [`PUBLISH_STATUS_PUBLISHED`] once [`PORTAL_BUILD`] has passed.
pub const PUBLISH_STATUS_PUBLISH_IN_PROGRESS: &str = "PUBLISH_IN_PROGRESS";

/// The `PublishStatus` a portal reports while the publication `DisablePortal` is taking
/// down is still being removed, out of `PUBLISH_STATUS_VALUES`. Not a terminal value:
/// the portal leaves it for [`PUBLISH_STATUS_DISABLED`] once [`PORTAL_BUILD`] has
/// passed.
///
/// It reports the service's remaining work, NOT whether public access is still granted:
/// the take-down revokes what the portal's domain serves the moment `DisablePortal` is
/// accepted (see [`PortalTransition::revokes_publication`] and [`Portal::live_site`]).
pub const PUBLISH_STATUS_DISABLE_IN_PROGRESS: &str = "DISABLE_IN_PROGRESS";

/// The `PreviewStatus` a portal's preview reports while it is still being built, out of
/// `PREVIEW_STATUS_VALUES` (`PREVIEW_FAILED | PREVIEW_IN_PROGRESS | PREVIEW_READY`,
/// `smithy_query(query="shape", shape="PreviewStatus")`).
pub const PREVIEW_STATUS_PREVIEW_IN_PROGRESS: &str = "PREVIEW_IN_PROGRESS";

/// The `PreviewStatus` of a preview that has finished building and can be opened, out
/// of `PREVIEW_STATUS_VALUES`.
pub const PREVIEW_STATUS_PREVIEW_READY: &str = "PREVIEW_READY";

/// How long one server-managed portal build takes: the span a publish, a disable or a
/// preview is still in flight for, after which the clock owes the portal the status
/// that build settles it in.
///
/// A CHOSEN span, not one AWS documents — AWS takes as long as building and
/// distributing a portal's content takes. What matters is that it is never zero, so a
/// caller that publishes a portal and immediately reads it back still sees
/// [`PUBLISH_STATUS_PUBLISH_IN_PROGRESS`] and the refusal that status carries, and that
/// nothing sleeps for it: the due instant is stored and the status derived from the
/// clock on every read.
pub const PORTAL_BUILD: Duration = Duration::from_millis(500);

/// How long a portal `DeletePortal` has taken away keeps answering `GetPortal` in its
/// terminal [`PUBLISH_STATUS_DELETED`], after which the record is gone for every reader
/// and the id names nothing at all.
///
/// Removing a portal is work the SERVICE carries out once the delete has answered, so a
/// client that reads the portal straight afterwards is told it is `DELETED` — which is how
/// it confirms the deletion it asked for — and a client that polls the portal until it is
/// gone converges on 404 instead of polling for ever. The span is one portal build
/// ([`PORTAL_BUILD`]), CHOSEN like it: what matters is that it is never zero, so the
/// terminal status is observable, and that nothing sleeps for it — the instant the record
/// is due to be gone is stored and compared against the clock on every read.
pub const PORTAL_DELETION: Duration = PORTAL_BUILD;

/// A publish or a disable of a portal that the SERVICE carries out on its own once the
/// call that asked for it has answered.
///
/// Holds the two statuses the build moves between and the instant it is due, so the
/// status a portal reports is derived from the clock rather than written by whoever
/// reads it; see [`Portal::settle`].
#[derive(Debug, Clone)]
pub struct PortalTransition {
    /// The `PublishStatus` the portal reports while this build is still in flight.
    pub in_progress: &'static str,
    /// The `PublishStatus` the portal settles in once this build is due.
    pub settled: &'static str,
    /// When this build is due to finish.
    pub due_at: SystemTime,
    /// The publication this build makes, reported once it has settled, or `None` for a
    /// disable: taking a publication down does not make one, so it leaves the record of
    /// the last one alone.
    pub published: Option<PortalPublication>,
    /// Whether this build is building the portal's content: a publish is, a disable is
    /// not. Decided by [`PortalBuild::builds_content`] and carried here so
    /// [`Portal::content_build_in_flight`] can answer it off the build in flight.
    pub builds_content: bool,
}

impl PortalTransition {
    /// Whether this build TAKES a publication down rather than making one, which is what
    /// a disable does: it makes no publication, so there is none to report when it
    /// settles.
    ///
    /// What the portal's domain serves turns on this from the instant the build starts
    /// rather than from the instant it settles ([`Portal::live_site`]): a take-down is a
    /// revocation of PUBLIC ACCESS, and access the service has been told to revoke is not
    /// still granted while the service finishes tearing the publication down. The
    /// `PublishStatus` the portal reports meanwhile
    /// ([`PUBLISH_STATUS_DISABLE_IN_PROGRESS`]) says what the service has left to do, not
    /// that the site is still up.
    pub fn revokes_publication(&self) -> bool {
        self.published.is_none()
    }
}

/// What a settled publish reports as the portal's last publication.
#[derive(Debug, Clone)]
pub struct PortalPublication {
    /// The instant the portal was published — the instant the publish finished, which
    /// is when the content it built went live.
    pub at: SystemTime,
    /// The description the publish request carried: "When the portal is published, this
    /// description becomes the last published description"
    /// (`PublishPortal$Description`). `None` for a publish that carried none, which
    /// publishes a version with no description.
    pub description: Option<String>,
    /// The site this publish BUILDS, captured when the publish started: a publish
    /// "publishes a portal" by building the content the portal held at that moment, so a
    /// later write to the portal's content changes nothing about what is already live
    /// until another publish builds it.
    pub site: PublishedSite,
}

/// The build a portal's domain is SERVING: what one settled publish put live.
///
/// Held apart from the portal's current content because the two are different things —
/// `PortalContent` is what the next publish would build, this is what the last one built
/// and what a visitor of the portal's domain is served until a `DisablePortal` takes it
/// down.
#[derive(Debug, Clone)]
pub struct PublishedSite {
    /// When this build went live, which is the portal's `LastPublished`.
    pub at: SystemTime,
    /// The portal content this build was made from.
    pub content: wire::PortalContent,
    /// The portal products the portal included when this build was made, by ARN.
    pub included_portal_product_arns: Vec<String>,
    /// The logo the portal carried when this build was made, if any.
    pub logo_uri: Option<String>,
}

/// What one of the three portal build calls asks the service to do.
///
/// Each names what it NEEDS before it can start — the portal's publication still, its
/// content build free, or nothing at all — which is what decides whether it can start
/// while the service is already busy with this portal (see [`Portal::start_build`]).
#[derive(Debug, Clone)]
pub enum PortalBuild {
    /// `PublishPortal`: build the portal's content and serve it, recording the
    /// description the request carried as the last published one.
    Publish { description: Option<String> },
    /// `DisablePortal`: take the publication down, after which the portal serves
    /// nothing. Started only from a status a publication can be taken down from
    /// ([`PORTAL_DISABLE_FROM_STATUSES`]).
    Disable,
    /// `PreviewPortal`: build a preview of the portal's content, which changes nothing
    /// about what the portal publishes.
    Preview,
}

impl PortalBuild {
    /// Whether this build needs the portal's publication to be STILL before it can
    /// start: a publish and a disable both do, since moving a publication on top of a
    /// change still in flight would leave two answers for what the portal serves. A
    /// preview does not — it changes nothing about what the portal publishes.
    ///
    /// For a disable this is what the status precondition on `DisablePortal` already
    /// implies ([`PORTAL_DISABLE_FROM_STATUSES`]): a portal with a publication change in
    /// flight reports an `..._IN_PROGRESS` status, which is none of the statuses a
    /// publication can be taken down from.
    pub fn needs_a_still_publication(&self) -> bool {
        matches!(
            self,
            PortalBuild::Publish { .. } | PortalBuild::Disable
        )
    }

    /// Whether this build builds the portal's CONTENT, so it both occupies the content
    /// build and cannot start while another content build is still in flight: a publish
    /// builds the content it serves and a preview builds the content it shows, while a
    /// disable builds nothing — it only stops serving what is already built.
    pub fn builds_content(&self) -> bool {
        matches!(self, PortalBuild::Publish { .. } | PortalBuild::Preview)
    }
}

/// The portal-product records — a `PortalProduct`, the product pages and the product
/// REST endpoint pages it owns, and the bounds, defaults and refusals those three share
/// — live in [`portal_products`], which this module re-exports in full so a handler
/// reaches them the way it reaches the rest of this module.
pub mod portal_products;

pub use portal_products::*;

/// Whether the REST endpoint a product REST endpoint page documents resolves at all is a
/// question for the service that OWNS REST APIs, so it is asked there, in
/// [`rest_endpoint`] — re-exported in full, like every other part of this module.
pub mod rest_endpoint;

pub use rest_endpoint::*;

/// Which data type a model's schema has to declare for a MAPPING TEMPLATE to be generated
/// from it lives in [`model_template`], re-exported in full like every other part of this
/// module.
pub use model_template::{unsupported_template_data_type, unsupported_template_data_type_message};

/// The PORTAL record itself — what a create resolved, how a reader projects it, and the
/// order the account's portals are reported in — lives in [`portal`], re-exported in full
/// like every other part of this module.
mod portal;
mod protocol_restriction;
pub use protocol_restriction::*;

pub use portal::*;

/// A persisted `VpcLink` resource, owned by the account and Region rather than by an
/// api.
#[derive(Debug, Clone)]
pub struct VpcLink {
    pub vpc_link_id: String,
    pub name: String,
    /// The security groups the request named, in the order it named them; empty for a
    /// request that named none. Required on the modelled `VpcLink` shape, so the record
    /// always carries the list even when it is empty.
    pub security_group_ids: Vec<String>,
    /// The subnets the request named, in the order it named them. Required on the
    /// modelled `VpcLink` shape and never empty: a link with no subnet has nowhere to
    /// attach an interface (see [`validate_subnet_ids`]).
    pub subnet_ids: Vec<String>,
    pub tags: BTreeMap<String, String>,
    pub created_date: SystemTime,
    /// When this link finishes provisioning, after which it reports
    /// [`VPC_LINK_STATUS_AVAILABLE`]; before it, [`VPC_LINK_STATUS_PENDING`]. See
    /// [`VpcLink::status`] and [`VPC_LINK_CREATE_SPAN`].
    pub available_at: Instant,
    /// When this link's deprovisioning finishes, after which no reader reports it at
    /// all; `None` for a link whose deletion has not been accepted. Set by the delete
    /// call to one [`VPC_LINK_DELETE_SPAN`] from then, so the link reports
    /// [`VPC_LINK_STATUS_DELETING`] until the clock makes its removal due. See
    /// [`VpcLink::is_deprovisioned`].
    pub deprovisioned_at: Option<Instant>,
}

impl VpcLink {
    /// The `VpcLinkStatus` this link reports now.
    ///
    /// A created link is still being provisioned when the create call answers, so it is
    /// reported [`VPC_LINK_STATUS_PENDING`] until [`VPC_LINK_CREATE_SPAN`] has passed
    /// and [`VPC_LINK_STATUS_AVAILABLE`] after it; a link whose deletion was accepted is
    /// reported [`VPC_LINK_STATUS_DELETING`] for as long as it is still readable at all.
    /// Every transition is one the CLOCK owes, materialised from a stored due instant
    /// rather than by sleeping or by being read, so every reader agrees on it and a read
    /// changes nothing.
    pub fn status(&self) -> &'static str {
        if self.deprovisioned_at.is_some() {
            VPC_LINK_STATUS_DELETING
        } else if Instant::now() < self.available_at {
            VPC_LINK_STATUS_PENDING
        } else {
            VPC_LINK_STATUS_AVAILABLE
        }
    }

    /// Whether the clock has finished this link's deprovisioning, so the account no
    /// longer holds it: true only for a link whose deletion was accepted and whose due
    /// instant has passed. [`ComAmazonawsApigatewayv2Apigatewayv2State::purge_deprovisioned_vpc_links`]
    /// is what drops such a record.
    pub fn is_deprovisioned(&self) -> bool {
        self.deprovisioned_at
            .is_some_and(|due| Instant::now() >= due)
    }

    /// The `VpcLinkStatusMessage` this link reports now: "a message summarizing the
    /// cause of the status of the VPC link" (`VpcLink$VpcLinkStatusMessage`).
    ///
    /// The message is keyed off the status [`VpcLink::status`] reports at the same
    /// instant, and is fixed text for that status: no part of it is taken from the
    /// request or from the link's record, so two links in one status report one
    /// sentence. A settled link carries [`VPC_LINK_AVAILABLE_MESSAGE`], the sentence the
    /// service itself serves; the statuses the clock moves a link through carry
    /// [`VPC_LINK_PENDING_MESSAGE`] and [`VPC_LINK_DELETING_MESSAGE`], which are written
    /// in the form of that one rather than verified against the service.
    pub fn status_message(&self) -> &'static str {
        match self.status() {
            VPC_LINK_STATUS_PENDING => VPC_LINK_PENDING_MESSAGE,
            VPC_LINK_STATUS_DELETING => VPC_LINK_DELETING_MESSAGE,
            // VPC_LINK_STATUS_AVAILABLE: the only other status a stored record reaches.
            _ => VPC_LINK_AVAILABLE_MESSAGE,
        }
    }

    /// Project the record onto the modelled `VpcLink` shape.
    pub fn to_wire(&self) -> wire::VpcLink {
        wire::VpcLink {
            created_date: Some(to_timestamp(self.created_date)),
            name: self.name.clone(),
            security_group_ids: self.security_group_ids.clone(),
            subnet_ids: self.subnet_ids.clone(),
            tags: Some(self.tags.iter().map(|(k, v)| (k.clone(), v.clone())).collect()),
            vpc_link_id: self.vpc_link_id.clone(),
            vpc_link_status: Some(self.status().to_string()),
            vpc_link_status_message: Some(self.status_message().to_string()),
            vpc_link_version: Some(VPC_LINK_VERSION_V2.to_string()),
        }
    }
}

/// A persisted custom `DomainName` resource.
///
/// Keyed in the store by [`DomainName::domain_name`], which is the identifier every
/// operation on the resource addresses it by. `DomainNameStatus` is deliberately NOT a
/// field: it is derived from the clock on every read (see [`DomainName::status`]), so no
/// stored copy of it can go stale.
#[derive(Debug, Clone)]
pub struct DomainName {
    pub domain_name: String,
    /// The id this store minted for the resource, stable for its life. It is the
    /// `DomainNameId` the routing-rule operations take as an optional disambiguator
    /// beside the name in their path (see
    /// [`DomainName::answers_to_requested_id`]), and it is the id of the regional host
    /// the resource is served on (see [`api_gateway_domain_host`]).
    pub domain_name_id: String,
    /// The resource's ARN, built once at create time from the request's region so it
    /// stays stable for the life of the resource.
    pub domain_name_arn: String,
    /// The regional host API Gateway serves this domain name on, minted with the
    /// resource and stable across every update: it names the endpoint a caller points
    /// their own DNS record at, so changing a certificate or a routing mode cannot move
    /// it. See [`api_gateway_domain_host`].
    pub api_gateway_domain_name: String,
    /// The Region the resource lives in, resolved from the request that CREATED it — the
    /// same Region its ARN and its regional host carry. Held because the hosted zone a
    /// regional endpoint of this domain name is aliased through is one per Region (see
    /// [`endpoint_hosted_zone_id`]), and a later request signed for another Region must
    /// not move the zone a caller already aliased their DNS record to.
    pub region: String,
    /// This domain name's endpoint configurations, in the order the request gave them —
    /// `DomainNameConfigurations` is a modelled list, not a map, so order is the
    /// caller's and is preserved.
    pub configurations: Vec<DomainNameEndpoint>,
    /// The mutual-TLS truststore, held as the modelled INPUT shape: `TruststoreUri` and
    /// `TruststoreVersion` are the caller's, while `TruststoreWarnings` is the service's
    /// report on the truststore and is projected in [`DomainName::to_wire`].
    pub mutual_tls_authentication: Option<wire::MutualTlsAuthenticationInput>,
    /// One of `ROUTING_MODE_VALUES`.
    pub routing_mode: String,
    pub tags: BTreeMap<String, String>,
    /// The instant the endpoint migration started by the last change to
    /// [`DomainName::configurations`] is due to finish, or `None` for a domain name
    /// whose endpoints have never been migrated. Until it passes the domain name reports
    /// `UPDATING` and refuses a further update; after it, `AVAILABLE`.
    pub migration_due_at: Option<SystemTime>,
    /// This domain name's API mappings, keyed by `ApiMappingId`. A mapping is addressed
    /// only under the domain that owns it
    /// (`/v2/domainnames/{DomainName}/apimappings/{ApiMappingId}`), so it is stored
    /// INSIDE the record rather than in a second map that could outlive a deleted
    /// domain name. `BTreeMap` for the stable key order `GetApiMappings` pages over.
    pub api_mappings: BTreeMap<String, ApiMapping>,
    /// Drives [`DomainName::mint_api_mapping_id`]. Per-domain, because an
    /// `ApiMappingId` only has to be unique within the domain name that owns it.
    pub api_mapping_id_seed: u64,
    /// This domain name's routing rules, keyed by `RoutingRuleId`. A rule is addressed
    /// only under the domain that owns it
    /// (`/v2/domainnames/{DomainName}/routingrules/{RoutingRuleId}`), so it is stored
    /// INSIDE the record for the same reason the api mappings are. `BTreeMap` for the
    /// stable key order `ListRoutingRules` pages over.
    pub routing_rules: BTreeMap<String, RoutingRule>,
    /// Drives [`DomainName::mint_routing_rule_id`]. Per-domain, because a
    /// `RoutingRuleId` only has to be unique within the domain name that owns it.
    pub routing_rule_id_seed: u64,
}

impl DomainName {
    /// Every ACM certificate ARN this domain name's endpoints reference, in ARN order
    /// and each only once.
    ///
    /// A custom domain name PRESENTS the certificates its configuration entries name —
    /// the endpoint's own `CertificateArn` and, for a mutual-TLS endpoint, the
    /// `OwnershipVerificationCertificateArn` it was verified with — so those are the
    /// certificates this resource is using. Reported here so the whole set is read off
    /// the stored record in one place, which is what the create, the update and the
    /// delete record in the shared association directory.
    pub fn referenced_certificate_arns(&self) -> BTreeSet<String> {
        self.configurations
            .iter()
            .flat_map(|endpoint| {
                [
                    endpoint.certificate_arn.clone(),
                    endpoint.ownership_verification_certificate_arn.clone(),
                ]
            })
            .flatten()
            .collect()
    }

    /// Mint a fresh `RoutingRuleId` for this domain name: [`ACCOUNT_ID_LEN`] lowercase
    /// alphanumerics, never one this domain name already minted, and never one another
    /// domain name minted either — the domain name's own id is the SCOPE the id is drawn
    /// within.
    pub fn mint_routing_rule_id(&mut self) -> String {
        self.routing_rule_id_seed = self.routing_rule_id_seed.wrapping_add(1);
        mint_id(
            IdKind::RoutingRule,
            &self.domain_name_id,
            self.routing_rule_id_seed,
        )
    }

    /// Whether this record is the domain name a request addressing it by name ALSO
    /// addressed by id.
    ///
    /// The routing-rule operations name the domain name in their path and take
    /// `DomainNameId` beside it, so a request that names an id is addressing the domain
    /// name that carries it: this record answers to its own id and to a request that
    /// names none, and to nothing else — a request naming another id addresses another
    /// resource, which this account does not hold under this name.
    pub fn answers_to_requested_id(&self, requested: Option<&str>) -> bool {
        requested.is_none_or(|id| id == self.domain_name_id)
    }
}

impl DomainName {
    /// Mint a fresh `ApiMappingId` for this domain name: [`ACCOUNT_ID_LEN`] lowercase
    /// alphanumerics, never one this domain name already minted, and never one another
    /// domain name minted either — the domain name's own id is the SCOPE the id is drawn
    /// within.
    pub fn mint_api_mapping_id(&mut self) -> String {
        self.api_mapping_id_seed = self.api_mapping_id_seed.wrapping_add(1);
        mint_id(
            IdKind::ApiMapping,
            &self.domain_name_id,
            self.api_mapping_id_seed,
        )
    }

    /// This domain name's mapping whose base path is `key`, if it holds one.
    ///
    /// The base path is what a request is matched on, so it identifies the mapping
    /// within its domain name: no two mappings of one domain name may carry the same
    /// one. An absent and an empty `ApiMappingKey` are the SAME base path — the root of
    /// the domain — which is why the comparison goes through
    /// [`ApiMapping::base_path`].
    ///
    /// `other_than` is the mapping the request ADDRESSES, when it addresses one: a patch
    /// that leaves a mapping on the base path it already occupies conflicts with nothing,
    /// because a mapping is never a second claim on its own base path. A writer that
    /// mints a new mapping addresses none and passes `None`.
    pub fn api_mapping_for_key(&self, key: &str, other_than: Option<&str>) -> Option<&ApiMapping> {
        self.api_mappings.values().find(|mapping| {
            other_than.is_none_or(|addressed| mapping.api_mapping_id != addressed)
                && mapping.base_path() == key
        })
    }
}

/// A persisted `ApiMapping`: one base path of a custom domain name, and the api stage
/// requests on that base path are routed to.
///
/// Keyed in its owning [`DomainName`] by [`ApiMapping::api_mapping_id`].
#[derive(Debug, Clone)]
pub struct ApiMapping {
    pub api_mapping_id: String,
    pub api_id: String,
    /// The base path as the request wrote it, kept verbatim so a read reports back
    /// exactly what was stored; `None` is the root mapping (the request named no key).
    pub api_mapping_key: Option<String>,
    /// The `StageName` of the stage of [`ApiMapping::api_id`] this mapping routes to.
    pub stage: String,
}

impl ApiMapping {
    /// The base path this mapping occupies on its domain name: the key the request gave,
    /// or the empty path for the root mapping. This is the value mappings are unique on.
    pub fn base_path(&self) -> &str {
        self.api_mapping_key.as_deref().unwrap_or("")
    }

    /// Project the record onto the modelled `ApiMapping` shape.
    pub fn to_wire(&self) -> wire::ApiMapping {
        wire::ApiMapping {
            api_id: self.api_id.clone(),
            api_mapping_id: Some(self.api_mapping_id.clone()),
            api_mapping_key: self.api_mapping_key.clone(),
            stage: self.stage.clone(),
        }
    }
}

/// A persisted `RoutingRule`: one rule of a custom domain name, matching a request on
/// its conditions and invoking the api stage its action names.
///
/// Keyed in its owning [`DomainName`] by [`RoutingRule::routing_rule_id`]. `Actions` and
/// `Conditions` are held as the modelled shapes: both are the caller's own composite
/// values, reported back exactly as they were stored, and no member of either is
/// service-computed.
#[derive(Debug, Clone)]
pub struct RoutingRule {
    pub routing_rule_id: String,
    /// The rule's ARN, built once at write time from the request's region (see
    /// [`routing_rule_arn`]) so it stays stable for the life of the rule.
    pub routing_rule_arn: String,
    /// "The order in which API Gateway evaluates a rule" (`RoutingRule$Priority`), within
    /// the modelled 1..1000000 range.
    pub priority: i32,
    pub actions: Vec<wire::RoutingRuleAction>,
    pub conditions: Vec<wire::RoutingRuleCondition>,
}

impl RoutingRule {
    /// Project the record onto the modelled `RoutingRule` shape.
    pub fn to_wire(&self) -> wire::RoutingRule {
        wire::RoutingRule {
            actions: Some(self.actions.clone()),
            conditions: Some(self.conditions.clone()),
            priority: Some(self.priority),
            routing_rule_arn: Some(self.routing_rule_arn.clone()),
            routing_rule_id: Some(self.routing_rule_id.clone()),
        }
    }
}

impl DomainName {
    /// This domain name's `DomainNameStatus` as of `now`.
    ///
    /// `UPDATING` while the migration the last endpoint-configuration change started is
    /// still due, `AVAILABLE` otherwise — "If the status is UPDATING, the domain cannot
    /// be modified further until the existing operation is complete. If it is AVAILABLE,
    /// the domain can be updated" (`DomainNameConfiguration$DomainNameStatus`). Derived,
    /// not stored, so reading a domain name never has to write one.
    pub fn status(&self, now: SystemTime) -> &'static str {
        match self.migration_due_at {
            Some(due) if now < due => DOMAIN_NAME_STATUS_UPDATING,
            _ => DOMAIN_NAME_STATUS_AVAILABLE,
        }
    }

    /// Project the record onto the modelled `DomainName` shape, as of `now`.
    pub fn to_wire(&self, now: SystemTime) -> wire::DomainName {
        let status = self.status(now);
        wire::DomainName {
            api_mapping_selection_expression: Some(
                API_MAPPING_SELECTION_EXPRESSION.to_string(),
            ),
            domain_name: self.domain_name.clone(),
            domain_name_arn: Some(self.domain_name_arn.clone()),
            domain_name_configurations: Some(
                self.configurations
                    .iter()
                    .map(|endpoint| {
                        endpoint.to_wire(&self.api_gateway_domain_name, status, &self.region)
                    })
                    .collect(),
            ),
            mutual_tls_authentication: self.mutual_tls_authentication.as_ref().map(|input| {
                wire::MutualTlsAuthentication {
                    truststore_uri: input.truststore_uri.clone(),
                    truststore_version: input.truststore_version.clone(),
                    // "A list of warnings that API Gateway returns while processing your
                    // truststore. Invalid certificates produce warnings"
                    // (`MutualTlsAuthentication$TruststoreWarnings`). Nothing was
                    // fetched or parsed — the truststore is an S3 URL this emulator does
                    // not read — so there is no warning to report and the list is empty
                    // rather than invented.
                    truststore_warnings: Some(Vec::new()),
                }
            }),
            routing_mode: Some(self.routing_mode.clone()),
            tags: Some(self.tags.iter().map(|(k, v)| (k.clone(), v.clone())).collect()),
        }
    }
}

/// Refuse a structure that does not name EXACTLY one of its alternatives.
///
/// Several of this service's input structures are tagged unions modelled as structures
/// whose members are each one alternative — `Authorization` ("Supports Cognito-based
/// user authentication or no authentication": `CognitoConfig | None`) and
/// `EndpointConfigurationRequest` ("Use a default domain provided by API Gateway or
/// provide a fully-qualified domain name that you own": `AcmManaged | None`). The
/// structure's meaning is the ONE alternative it names, so a request naming none has
/// said nothing and one naming several has said two contradictory things; both are
/// incomplete-or-incorrect input, which is what `BadRequestException` reports.
///
/// `alternatives` pairs each alternative's wire name with whether the request carried
/// it, in the order the message should list them. `Err` carries the message for the
/// operation's `BadRequestException`.
pub fn validate_exclusive_choice(
    member: &str,
    alternatives: &[(&str, bool)],
) -> Result<(), String> {
    let names: Vec<&str> = alternatives.iter().map(|(name, _)| *name).collect();
    let named: Vec<&str> = alternatives
        .iter()
        .filter(|(_, present)| *present)
        .map(|(name, _)| *name)
        .collect();
    match named.len() {
        1 => Ok(()),
        0 => Err(exclusive_choice_required(member, &names)),
        _ => Err(format!(
            "{member} must specify exactly one of: {}, but specifies {}.",
            names.join(", "),
            named.join(", ")
        )),
    }
}

/// The message a structure naming NO alternative is refused with: the member, and the
/// alternatives it has to choose between. Declared once, so
/// [`validate_exclusive_choice`] and the readers of a one-alternative structure (see
/// [`rest_endpoint_identifier_parts`]) refuse the same shape of input with the same
/// wording.
fn exclusive_choice_required(member: &str, names: &[&str]) -> String {
    format!("{member} must specify exactly one of: {}.", names.join(", "))
}

/// The resource portion every portal-product ARN opens with: API Gateway names a
/// resource by its own REST path, so a portal product's is `/portalproducts/{id}` (see
/// [`portal_product_arn`]). Declared once, so the writer that BUILDS such an ARN and the
/// validator that READS a caller-supplied one cannot drift.
pub const PORTAL_PRODUCT_RESOURCE_PREFIX: &str = "/portalproducts/";

/// Refuse a member that must hold ARNs of a given resource type but holds a value that is
/// not a WELL-FORMED ARN of one.
///
/// `IncludedPortalProductArns` names portal products — account-owned resources of THIS
/// service's ARN namespace, under the `/portalproducts/` resource path (see
/// [`portal_product_arn`]) — so a well-formed value carries every field the name of such a
/// resource carries: the literal `arn`, a partition, this service's vendor, the Region the
/// product was created in, the ACCOUNT that owns it, and the resource path with a non-empty
/// id. A value missing any of them, or naming another vendor or another of this service's
/// resource types (an `/apis/{id}`, say), is not the name of a portal product at all, which
/// is what API Gateway reports rather than looking for a product under it.
///
/// The parse is [`::service_common::arn::parse_of`], the tree's ONE caller-supplied-ARN
/// parse (it is also what rejects an ARN with an empty partition, vendor or resource);
/// `resource_prefix` is what narrows it to one resource type, and the Region and account
/// fields — which that parse leaves empty for a partition-global or account-less resource —
/// are required here, because the resource this member names is neither.
///
/// `Err` carries the message for the operation's `BadRequestException`: API Gateway's own
/// sentence for a malformed ARN, which states the fact and echoes the value
/// ([`not_well_formed_arn_message`]).
pub fn validate_arns_of<'a, I>(vendor: &str, resource_prefix: &str, arns: I) -> Result<(), String>
where
    I: IntoIterator<Item = &'a str>,
{
    for arn in arns {
        // A prefix match alone would accept `…:/portalproducts/` — the resource type with
        // no id, which names no resource — so the remainder after the prefix must be
        // non-empty.
        let names_the_resource = ::service_common::arn::parse_of(arn, vendor).is_some_and(|parsed| {
            !parsed.region.is_empty()
                && !parsed.account.is_empty()
                && parsed
                    .resource
                    .strip_prefix(resource_prefix)
                    .is_some_and(|id| !id.is_empty())
        });
        if !names_the_resource {
            return Err(not_well_formed_arn_message(arn));
        }
    }
    Ok(())
}

/// How API Gateway words the refusal of a value that is not a well-formed ARN: the fixed
/// text `Invalid ARN. ARN is not well formed ` and the value the REQUEST sent, which is the
/// only thing the caller can act on.
///
/// Held here once, so every reader of a caller-supplied ARN states the same sentence.
pub fn not_well_formed_arn_message(arn: &str) -> String {
    format!("Invalid ARN. ARN is not well formed {arn}")
}

/// The ARN of a stored portal product.
///
/// API Gateway's own ARNs carry the resource portion as the URI path of the resource
/// (`/portalproducts/{portalProductId}`, the `GetPortalProduct` URI without its
/// version prefix), under this service's `arnNamespace`
/// (`smithy_query(query="service_info")` reports `apigateway`). Built through
/// [`::engine::emulator_arn`], so the account segment is the emulator's canonical
/// account rather than a literal: a portal product is an account-owned, shareable
/// resource (`GetPortalProduct$ResourceOwnerAccountId`), so the owning account is part
/// of its name. The namespace is [`ARN_SERVICE`], declared once for every resource this
/// store names.
pub fn portal_product_arn(region: &str, portal_product_id: &str) -> String {
    ::engine::emulator_arn(
        ARN_SERVICE,
        region,
        &format!("{PORTAL_PRODUCT_RESOURCE_PREFIX}{portal_product_id}"),
    )
}

/// The prefix of an Amazon VPC subnet id, the resource type `SubnetIdList`'s members
/// name ("A list of subnet IDs to include in the VPC link", `VpcLink$SubnetIds`).
pub const SUBNET_ID_PREFIX: &str = "subnet-";

/// The prefix of an Amazon EC2 security group id, the resource type
/// `SecurityGroupIdList`'s members name ("A list of security group IDs for the VPC
/// link", `VpcLink$SecurityGroupIds`).
pub const SECURITY_GROUP_ID_PREFIX: &str = "sg-";

/// How many hexadecimal characters an EC2 resource id carries after its prefix.
///
/// From the Amazon EC2 "Resource IDs" documentation: an id is the resource type's prefix
/// followed by 8 characters in the original form, or 17 in the longer form AWS moved to;
/// both forms remain valid, so both lengths are accepted.
pub const EC2_ID_HEX_LENGTHS: &[usize] = &[8, 17];

/// Whether `value` is an EC2 resource id of the type `prefix` names: the prefix followed
/// by one of [`EC2_ID_HEX_LENGTHS`] lowercase hexadecimal characters.
pub fn is_ec2_resource_id(value: &str, prefix: &str) -> bool {
    let Some(suffix) = value.strip_prefix(prefix) else {
        return false;
    };
    EC2_ID_HEX_LENGTHS.contains(&suffix.len())
        && suffix
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
}

/// Refuse a list whose members are not EC2 resource ids of the type `prefix` names.
///
/// `SubnetIdList` and `SecurityGroupIdList` are lists of unconstrained `__string` in the
/// model (`smithy_query(query="shape", shape="SubnetIdList")`), so the id form is the
/// handler's to enforce rather than the boundary validator's: a value that is not an id
/// of that resource type names no resource the link could ever attach to. `Err` carries
/// the message for the operation's `BadRequestException`.
pub fn validate_ec2_resource_ids(
    member: &str,
    prefix: &str,
    ids: &[String],
) -> Result<(), String> {
    for id in ids {
        if !is_ec2_resource_id(id, prefix) {
            return Err(format!(
                "Invalid value for {member}: '{id}'. Each value must be of the form \
                 {prefix}<8 or 17 hexadecimal characters>."
            ));
        }
    }
    Ok(())
}

/// The longest `Name` a VPC link accepts.
///
/// `CreateVpcLink$Name` and `UpdateVpcLink$Name` both target
/// `StringWithLengthBetween1And128`, whose own documentation is "A string with a length
/// between [1-128]" (`smithy_query(query="shape",
/// shape="StringWithLengthBetween1And128")`). The shape carries no `@length` trait, so
/// the bound is a handler's to enforce rather than the boundary validator's.
pub const MAX_VPC_LINK_NAME_LEN: usize = API_NAME_LEN.1;

/// Resolve every subnet a `CreateVpcLink` names against the account's own network — see
/// [`ec2_subnets::resolve_vpc_link_subnets`], which asks the service that OWNS subnets.
pub use ec2_subnets::resolve_vpc_link_subnets;

/// How a refusal names a subnet the account has none of — see
/// [`ec2_subnets::subnet_does_not_exist_message`].
pub use ec2_subnets::subnet_does_not_exist_message;

/// The sentence a create is refused with when its subnets span two VPCs — see
/// [`ec2_subnets::SUBNETS_NOT_IN_ONE_VPC_MESSAGE`].
pub use ec2_subnets::SUBNETS_NOT_IN_ONE_VPC_MESSAGE;

/// The most distinct subnets one create resolves — see
/// [`ec2_subnets::MAX_SUBNET_IDS_RESOLVED`], the server-side bound on the work one call
/// costs.
pub use ec2_subnets::MAX_SUBNET_IDS_RESOLVED;

/// Refuse a VPC link `Name` outside the documented bound of the shape it targets: at
/// least one character and at most [`MAX_VPC_LINK_NAME_LEN`].
///
/// `Ok(())` or the message for the operation's `BadRequestException`: it names the member
/// and the bound it broke.
pub fn validate_vpc_link_name(name: &str) -> Result<(), String> {
    validate_member_length("Name", name, API_NAME_LEN)
}

/// The ARN of a portal: API Gateway's ARNs name the resource by its own REST path
/// (`/apis/{apiId}` for an api, `/domainnames/{name}` for a domain name), so a portal's
/// is `/portals/{portalId}`. Built through `engine::emulator_arn`, so the account
/// segment is the emulator's canonical one.
pub fn portal_arn(region: &str, portal_id: &str) -> String {
    ::engine::emulator_arn(ARN_SERVICE, region, &format!("/portals/{portal_id}"))
}

/// The second label of the default domain API Gateway generates for a portal:
/// `{portal-id}.portal.{region}.amazonaws.com`. CHOSEN, like the rest of that host (see
/// [`portal_default_domain_name`]), and read back by the portal data plane for a request
/// that carries the endpoint in its path.
pub const PORTAL_ENDPOINT_LABEL: &str = "portal";

/// The default domain name API Gateway generates for a portal: the host a consumer
/// reaches the portal on when the account brought no domain of its own
/// (`EndpointConfigurationResponse$PortalDefaultDomainName`: "This domain name is
/// generated and managed by API Gateway").
///
/// AWS publishes no format for it, so the host follows this service's own convention
/// for a generated endpoint host — `{resource-id}.{subdomain}.{region}.amazonaws.com`,
/// as [`execute_api_endpoint`] builds for an api. What the member has to hold is a
/// name that is unique to the portal, stable across reads and within the modelled
/// 3..256 bound; the subdomain label is CHOSEN.
pub fn portal_default_domain_name(portal_id: &str, region: &str) -> String {
    format!("{portal_id}.{PORTAL_ENDPOINT_LABEL}.{region}.amazonaws.com")
}

/// The single path segment a portal's preview is served under, and the one
/// [`portal_preview_url`] announces it at. CHOSEN, like the rest of that URL, and read
/// back by the portal data plane so the path a client is given and the path that is served
/// cannot drift apart.
pub const PORTAL_PREVIEW_SEGMENT: &str = "preview";

/// The URL a portal's preview is served on.
///
/// A preview is the portal's own content served before anything is published, so it is
/// served on the domain API Gateway generates and manages for the portal
/// ([`portal_default_domain_name`]) rather than on a domain the account brought, which
/// need not resolve to the portal yet, under a path of its own.
///
/// AWS publishes no format for `Preview$PreviewUrl`, so the path label is CHOSEN; what
/// the member has to hold is a URL that names THIS portal and is stable across reads,
/// which it is because it is derived from the endpoint configuration the create
/// resolved once.
pub fn portal_preview_url(endpoint: &wire::EndpointConfigurationResponse) -> String {
    format!(
        "https://{}/{PORTAL_PREVIEW_SEGMENT}",
        endpoint.portal_default_domain_name
    )
}

/// How many characters follow the leading `Z` of a hosted zone id, matching the
/// fourteen-character ids AWS publishes for its endpoint hosted zones.
const HOSTED_ZONE_ID_LEN: usize = 13;

/// A hosted zone id DERIVED from the parts that decide WHICH endpoint it belongs to — the
/// fallback for an endpoint whose published id this service does not carry.
///
/// AWS assigns ONE opaque hosted zone id per service endpoint per Region and publishes no
/// formula for it, so an id it publishes is CARRIED verbatim where this service has it
/// ([`CLOUDFRONT_HOSTED_ZONE_ID`], [`PORTAL_DOMAIN_HOSTED_ZONES`]) and derived from those
/// parts otherwise, rather than invented afresh per read. The mixing is
/// [`::service_common::stable_hex_id`] — the tree's one derived-id mixer, reproducible
/// across builds — uppercased and cut to the published LENGTH: a leading `Z` and
/// [`HOSTED_ZONE_ID_LEN`] characters, which the mixer renders as uppercase hexadecimal
/// (a subset of the uppercase alphanumerics AWS's own ids use, e.g. `Z1UJRXOUMOOFQ8`).
/// The same parts therefore always answer the same id, and two different part lists
/// answer different ones.
fn derived_hosted_zone_id(parts: &[&str]) -> String {
    let mixed: String = ::service_common::stable_hex_id(parts)
        .to_uppercase()
        .chars()
        .take(HOSTED_ZONE_ID_LEN)
        .collect();
    format!("Z{mixed}")
}

/// One Region's hosted zone for the domains API Gateway generates and manages for a
/// developer portal: the zone, and the Region it is the zone OF.
#[derive(Debug)]
pub struct PortalDomainHostedZone {
    pub region: &'static str,
    pub hosted_zone_id: &'static str,
}

/// The hosted zones AWS PUBLISHES for portal default domains, one entry per Region.
///
/// Every portal in a Region is served on the one domain API Gateway generates and manages
/// there (`EndpointConfigurationResponse$PortalDefaultDomainName`), so the zone an alias
/// record for it targets is a fact about the REGION: AWS answers the same id for every
/// portal created in that Region and for every read of it, as the recorded AWS answers for
/// `us-west-2` show. AWS ASSIGNS the id and publishes no formula for it — the same as the
/// one it publishes for a CloudFront distribution ([`CLOUDFRONT_HOSTED_ZONE_ID`]) — so the
/// ids it publishes are CARRIED here rather than computed, and a Region this table does not
/// name falls back to [`derived_hosted_zone_id`] (see
/// [`portal_domain_hosted_zone_id`]).
///
/// `tests/all/portal_hosted_zone.rs` keeps its OWN copy of these ids and asserts the two
/// agree Region for Region, so an entry here always has a published AWS answer behind it
/// and a Region added to either belongs in both.
pub const PORTAL_DOMAIN_HOSTED_ZONES: &[PortalDomainHostedZone] = &[PortalDomainHostedZone {
    region: "us-west-2",
    hosted_zone_id: "Z091913938PZVWK6ZMXI7",
}];

/// The published hosted zone of the portal default domain in `region`, or `None` for a
/// Region [`PORTAL_DOMAIN_HOSTED_ZONES`] carries no published id for.
pub fn published_portal_domain_hosted_zone_id(region: &str) -> Option<&'static str> {
    PORTAL_DOMAIN_HOSTED_ZONES
        .iter()
        .find(|zone| zone.region == region)
        .map(|zone| zone.hosted_zone_id)
}

/// The hosted zone id of the portal domain
/// (`EndpointConfigurationResponse$PortalDomainHostedZoneId`), the zone an alias record
/// for the portal's domain points at.
///
/// The member is REQUIRED on the modelled response, so a portal always reports one, and it
/// is one id per Region: the id AWS publishes for the Region
/// ([`published_portal_domain_hosted_zone_id`]) wherever this service carries it, since a
/// caller writes a Route 53 alias record against that zone and a zone id of this emulator's
/// own names no zone their account can reach. A Region with no published id carried here
/// still reports one id of its own, derived by [`derived_hosted_zone_id`], so the member
/// keeps its per-Region invariance and its stability everywhere.
pub fn portal_domain_hosted_zone_id(region: &str) -> String {
    published_portal_domain_hosted_zone_id(region)
        .map_or_else(|| derived_hosted_zone_id(&[region]), str::to_string)
}

/// The `EndpointType` whose domain is fronted by a CloudFront distribution rather than by
/// a regional endpoint: "an AWS-managed certificate that will be used by the
/// edge-optimized endpoint for this domain name" (`DomainNameConfiguration$CertificateArn`
/// and the rest of that shape's edge-optimized members).
pub const EDGE_ENDPOINT_TYPE: &str = "EDGE";

/// The one hosted zone every CloudFront distribution is aliased through, whatever Region
/// the resource was created in. It is PUBLISHED rather than derived: Route 53's own
/// `AliasTarget$HostedZoneId` states it ("CloudFront distribution: Specify
/// `Z2FDTNDATAQYW2`"), and
/// `services/service-com-amazonaws-appsync-awsdeepdishcontrolplaneservice` reports the
/// same value for its own CloudFront-fronted custom domains.
pub const CLOUDFRONT_HOSTED_ZONE_ID: &str = "Z2FDTNDATAQYW2";

/// "The Amazon Route 53 Hosted Zone ID of the endpoint"
/// (`DomainNameConfiguration$HostedZoneId`) — the zone an ALIAS record for the custom
/// domain names as its target, which is a fact about the endpoint rather than about the
/// resource: one value per endpoint type, and for a regional endpoint one per Region.
///
/// * an `EDGE` endpoint is fronted by CloudFront, so every one of them is aliased through
///   the single published [`CLOUDFRONT_HOSTED_ZONE_ID`], in every Region;
/// * a `REGIONAL` endpoint lives in the Region the domain name was created in, so its zone
///   is one per Region ([`derived_hosted_zone_id`] over the endpoint type and the Region),
///   and never the CloudFront one.
///
/// Derived on every read from the stored endpoint type and the stored Region, so a domain
/// name reports the same id for the life of the endpoint and reports a different one once
/// an update moves it onto another endpoint type.
pub fn endpoint_hosted_zone_id(endpoint_type: &str, region: &str) -> String {
    if endpoint_type == EDGE_ENDPOINT_TYPE {
        return CLOUDFRONT_HOSTED_ZONE_ID.to_string();
    }
    derived_hosted_zone_id(&[endpoint_type, region])
}

/// Resolve a portal's requested domain configuration into the one the readers report,
/// generating the default domain of a portal with this id for the endpoint this REQUEST was
/// addressed at.
///
/// The request names either a domain the account owns together with its ACM certificate
/// (`AcmManaged`) or API Gateway's default domain (`None`); either way the response
/// carries the generated default domain name and its hosted zone id, and reports
/// `DomainName`/`CertificateArn` only for the domain the account brought.
///
/// The default domain is generated by [`announced_portal_default_domain_name`], which is
/// where the choice between the two addresses it can name is explained.
pub fn resolve_portal_endpoint_configuration(
    portal_id: &str,
    region: &str,
    parts: &::http::request::Parts,
    requested: &wire::EndpointConfigurationRequest,
) -> wire::EndpointConfigurationResponse {
    portal_endpoint_configuration(
        announced_portal_default_domain_name(portal_id, region, parts),
        portal_domain_hosted_zone_id(region),
        requested,
    )
}

/// The default domain this service ANNOUNCES for a portal: the address a consumer reaches
/// that portal's website on, resolved ONCE when the portal is created and stored, so every
/// later read reports the same value.
///
/// Which address that is follows the endpoint the CREATE was addressed at, because the
/// member exists for a client to fetch and a value nothing on the caller's machine resolves
/// names no portal it can reach:
///
/// * a request that addressed this service by one of AWS's own hosts comes from a client
///   that resolves them, so the portal is announced on the generated host
///   [`portal_default_domain_name`] builds — the shape
///   `EndpointConfigurationResponse$PortalDefaultDomainName` documents;
/// * a request that addressed the EMULATOR'S OWN endpoint ([`local_endpoint_host`]) comes
///   from a client that resolves nothing else, so the portal is announced on that same
///   endpoint with its own path behind the [`LOCAL_ENDPOINT_PREFIX`] marker. This is the
///   address the portal data plane serves, and the same shape this emulator's API Gateway
///   (REST) stages are reached at (`/_aws/execute-api/{api-id}/{stage}`).
///
/// Both forms are within the modelled 3..256 bound of the member.
pub fn announced_portal_default_domain_name(
    portal_id: &str,
    region: &str,
    parts: &::http::request::Parts,
) -> String {
    match local_endpoint_host(parts) {
        Some(host) => format!("{host}/{LOCAL_ENDPOINT_PREFIX}/{PORTAL_ENDPOINT_LABEL}/{portal_id}"),
        None => portal_default_domain_name(portal_id, region),
    }
}

/// Resolve a requested domain configuration onto a default domain the service has
/// ALREADY generated for the portal.
///
/// Which domain the account brought is the request's to say, while the default domain
/// name and its hosted zone id are generated from the portal's id and the Region the
/// portal lives in — so a later write to a portal's domain configuration carries those
/// two over rather than rebuilding them from whatever Region the writing request was
/// signed for. [`resolve_portal_endpoint_configuration`] is this with the pair generated
/// for a given id and Region.
pub fn portal_endpoint_configuration(
    portal_default_domain_name: String,
    portal_domain_hosted_zone_id: String,
    requested: &wire::EndpointConfigurationRequest,
) -> wire::EndpointConfigurationResponse {
    wire::EndpointConfigurationResponse {
        certificate_arn: requested
            .acm_managed
            .as_ref()
            .map(|managed| managed.certificate_arn.clone()),
        domain_name: requested
            .acm_managed
            .as_ref()
            .map(|managed| managed.domain_name.clone()),
        portal_default_domain_name,
        portal_domain_hosted_zone_id,
    }
}

/// Refuse a `SubnetIds` list that no VPC link could be built from: one naming no subnet
/// at all, or one naming a value that is not a subnet id.
///
/// `SubnetIds` is required on both the request and the modelled `VpcLink` shape, and a
/// link's network interfaces live IN its subnets, so a link with none has nowhere to
/// attach one. `Err` carries the message for `BadRequestException`.
pub fn validate_subnet_ids(subnet_ids: &[String]) -> Result<(), String> {
    if subnet_ids.is_empty() {
        return Err("Invalid value for SubnetIds: a VPC link must specify at least one \
                    subnet."
            .to_string());
    }
    validate_ec2_resource_ids("SubnetIds", SUBNET_ID_PREFIX, subnet_ids)
}

/// One entry of a domain name's `DomainNameConfigurations`.
///
/// Holds only what a request sets plus the members the service resolves ONCE at write
/// time (the three defaulted enums). `DomainNameStatus` and `ApiGatewayDomainName` are
/// not here: the first is derived from the clock and the second belongs to the domain
/// name as a whole, so both are supplied by [`DomainNameEndpoint::to_wire`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DomainNameEndpoint {
    pub certificate_arn: Option<String>,
    pub certificate_name: Option<String>,
    pub ownership_verification_certificate_arn: Option<String>,
    /// One of `ENDPOINT_TYPE_VALUES`.
    pub endpoint_type: String,
    /// One of `SECURITY_POLICY_VALUES`.
    pub security_policy: String,
    /// One of `IP_ADDRESS_TYPE_VALUES`.
    pub ip_address_type: String,
}

impl DomainNameEndpoint {
    /// Project the entry onto the modelled `DomainNameConfiguration` shape, carrying the
    /// owning domain name's host and its current status.
    pub fn to_wire(
        &self,
        api_gateway_domain_name: &str,
        domain_name_status: &str,
        region: &str,
    ) -> wire::DomainNameConfiguration {
        wire::DomainNameConfiguration {
            api_gateway_domain_name: Some(api_gateway_domain_name.to_string()),
            certificate_arn: self.certificate_arn.clone(),
            certificate_name: self.certificate_name.clone(),
            // "The timestamp when the certificate … was uploaded"
            // (`DomainNameConfiguration$CertificateUploadDate`) is a fact about the
            // certificate in ACM, not about this resource: nothing was uploaded here and
            // no rule derives the instant from the request, so it is left unset rather
            // than filled with an instant that would claim an upload that never happened.
            certificate_upload_date: None,
            domain_name_status: Some(domain_name_status.to_string()),
            // Reported only alongside a status that needs explaining; `AVAILABLE` and
            // the migration this service runs carry no detail beyond the status itself.
            domain_name_status_message: None,
            endpoint_type: Some(self.endpoint_type.clone()),
            // "The Amazon Route 53 Hosted Zone ID of the endpoint"
            // (`DomainNameConfiguration$HostedZoneId`) is the alias target a caller's own
            // Route 53 ALIAS record for this domain points at, and it is a fact about the
            // ENDPOINT: one zone per endpoint type, and per Region for a regional one.
            // See [`endpoint_hosted_zone_id`].
            hosted_zone_id: Some(endpoint_hosted_zone_id(&self.endpoint_type, region)),
            ip_address_type: Some(self.ip_address_type.clone()),
            security_policy: Some(self.security_policy.clone()),
            ownership_verification_certificate_arn: self
                .ownership_verification_certificate_arn
                .clone(),
        }
    }
}

/// The ARN of a `DomainName` resource in `region`.
///
/// API Gateway's resource portion for a custom domain name is `/domainnames/{name}`, and
/// the account segment is the emulator's canonical one (see `engine::emulator_arn`).
pub fn domain_name_arn(domain_name: &str, region: &str) -> String {
    ::engine::emulator_arn(ARN_SERVICE, region, &format!("/domainnames/{domain_name}"))
}

/// The regional host API Gateway serves a custom domain name on: the
/// `d-{id}.execute-api.{region}.amazonaws.com` form a caller aliases their own DNS
/// record to, built from an id this store minted (see
/// [`ComAmazonawsApigatewayv2Apigatewayv2State::mint_domain_name_id`]) and the api
/// endpoint host `CreateApiResponse$ApiEndpoint` documents.
pub fn api_gateway_domain_host(domain_id: &str, region: &str) -> String {
    format!("d-{domain_id}.execute-api.{region}.amazonaws.com")
}

/// Resolve a request's whole `DomainNameConfigurations` list into the records to store.
///
/// A custom domain name is reached only through the endpoints this list describes — each
/// entry names the certificate the endpoint presents, its endpoint type, its security
/// policy and its address family — so a list with no entry at all describes a domain name
/// nothing can serve. A request that would leave the resource in that shape is refused
/// instead of stored: `Err` carries the message for the operation's `BadRequestException`
/// ("the input is incomplete or incorrect"). Each entry is resolved by
/// [`resolve_domain_name_endpoint`], so a create and an update cannot drift.
pub fn resolve_domain_name_endpoints(
    requested: &[wire::DomainNameConfiguration],
) -> Result<Vec<DomainNameEndpoint>, String> {
    if requested.is_empty() {
        return Err(EMPTY_DOMAIN_NAME_CONFIGURATIONS.to_string());
    }
    requested
        .iter()
        .enumerate()
        .map(|(index, entry)| resolve_domain_name_endpoint(index, entry))
        .collect()
}

/// Resolve one `DomainNameConfigurations` entry of a request into the record to store.
///
/// The three enum members are defaulted here and nowhere else, so a create and an update
/// resolve them identically. `Err` carries the message for the operation's
/// `BadRequestException`: `DomainNameStatus` is service-computed, so a request that sets
/// it is naming a value it does not own, and each enum is checked against its modelled
/// catalog so an unexpected value is named in the refusal rather than stored.
pub fn resolve_domain_name_endpoint(
    index: usize,
    requested: &wire::DomainNameConfiguration,
) -> Result<DomainNameEndpoint, String> {
    if let Some(status) = requested.domain_name_status.as_deref() {
        return Err(format!(
            "DomainNameConfigurations[{index}].DomainNameStatus is set by the service and \
             cannot be specified in a request; got '{status}'."
        ));
    }
    let endpoint_type = checked_enum(
        index,
        "EndpointType",
        requested.endpoint_type.as_deref(),
        DEFAULT_ENDPOINT_TYPE,
        wire::ENDPOINT_TYPE_VALUES,
    )?;
    let security_policy = checked_enum(
        index,
        "SecurityPolicy",
        requested.security_policy.as_deref(),
        DEFAULT_SECURITY_POLICY,
        wire::SECURITY_POLICY_VALUES,
    )?;
    // A policy the catalog carries but this service does not serve a domain name under is
    // refused here, in the ONE place every writer of the resource resolves an entry: see
    // [`ENFORCED_SECURITY_POLICY`].
    if security_policy != ENFORCED_SECURITY_POLICY {
        return Err(unsupported_security_policy_message(&security_policy));
    }
    let ip_address_type = checked_enum(
        index,
        "IpAddressType",
        requested.ip_address_type.as_deref(),
        DEFAULT_IP_ADDRESS_TYPE,
        wire::IP_ADDRESS_TYPE_VALUES,
    )?;
    Ok(DomainNameEndpoint {
        certificate_arn: requested.certificate_arn.clone(),
        certificate_name: requested.certificate_name.clone(),
        ownership_verification_certificate_arn: requested
            .ownership_verification_certificate_arn
            .clone(),
        endpoint_type,
        security_policy,
        ip_address_type,
    })
}

/// Resolve a request's `RoutingMode` against the modelled catalog.
///
/// `Ok(None)` means the request named none, leaving the defaulting decision with the
/// caller — a create applies [`DEFAULT_ROUTING_MODE`], an update leaves the stored value
/// alone — while the catalog check and its wording live here, so the two handlers cannot
/// drift. `Err` carries the message for the operation's `BadRequestException`.
///
/// `RoutingMode` is a modelled enum, so the boundary validator already refuses a value
/// outside the catalog; checking it here too means an unexpected value is named in the
/// refusal rather than stored and reported back.
pub fn resolve_routing_mode(requested: Option<&str>) -> Result<Option<String>, String> {
    match requested {
        None => Ok(None),
        Some(mode) if wire::ROUTING_MODE_VALUES.contains(&mode) => Ok(Some(mode.to_string())),
        Some(mode) => Err(format!(
            "Invalid value for RoutingMode: '{mode}'. Valid values are: {}.",
            wire::ROUTING_MODE_VALUES.join(", ")
        )),
    }
}

/// One enum member of a `DomainNameConfigurations` entry: the requested value when it is
/// in `catalog`, `default` when the request names none.
fn checked_enum(
    index: usize,
    member: &str,
    requested: Option<&str>,
    default: &str,
    catalog: &[&str],
) -> Result<String, String> {
    match requested {
        None => Ok(default.to_string()),
        Some(value) if catalog.contains(&value) => Ok(value.to_string()),
        Some(value) => Err(format!(
            "Invalid value for DomainNameConfigurations[{index}].{member}: '{value}'. \
             Valid values are: {}.",
            catalog.join(", ")
        )),
    }
}

/// Render a stored instant as the modelled `__timestampIso8601` member.
///
/// Sub-second precision is kept: `__timestampIso8601` carries
/// `@timestampFormat("date-time")`, whose RFC 3339 rendering states fractional seconds
/// when the instant has them, so truncating here would drop ordering a client uses to
/// tell two revisions of one resource apart.
pub fn to_timestamp(at: SystemTime) -> ::aws_smithy_types::DateTime {
    ::aws_smithy_types::DateTime::from(at)
}

/// How many ids of `len` characters [`mint_id`] can render: `36^len`.
const fn id_space(len: u32) -> u128 {
    36u128.pow(len)
}

/// The step [`mint_id`] takes through an id space of `space` values per seed: about
/// 0.618 of it — the golden ratio, which spreads an additive walk most evenly over a
/// range, so two consecutive seeds differ in every character of the id rather than only
/// in its lowest ones and no position of a minted id is stuck on one character.
///
/// Raised to the next value coprime with 36: `36^n = 2^2n * 3^2n`, so a step divisible by
/// neither 2 nor 3 makes `seed -> seed * step + offset (mod 36^n)` a BIJECTION, and two
/// seeds of one scope can never mint the same id — no collision retry, no exhaustion
/// branch and no random source.
const fn id_step(space: u128) -> u128 {
    let mut step = space / 1_000 * 618 + 1;
    while step.is_multiple_of(2) || step.is_multiple_of(3) {
        step += 1;
    }
    step
}

/// Where in the id space the ids of ONE scope begin: a stable mixing of the id of the
/// resource that owns the collection ([`ACCOUNT_SCOPE`] for a collection the account
/// holds directly).
///
/// A seed counts within its own collection — the first route of one api and the first
/// route of another are both seed 1 — so an id derived from the seed alone would be the
/// SAME string for two unrelated resources, which collides in any client keyed on it.
/// Starting each scope at its own point separates them. The mixing is
/// [`::service_common::stable_hex_id`], the tree's one derived-id mixer, so a scope
/// always begins at the same point across reads and builds.
fn scope_offset(scope: &str, space: u128) -> u128 {
    let mixed = ::service_common::stable_hex_id(&[scope]);
    u128::from(u64::from_str_radix(&mixed, 16).unwrap_or_default()) % space
}

/// Mint one id for `kind` within `scope` from `seed`: [`IdKind::id_len`] characters drawn
/// from [`ID_ALPHABET`].
///
/// `(kind, seed) -> seed * ID_KIND_STRIDE + kind` is injective and
/// `preimage -> preimage * id_step + scope_offset (mod 36^len)` is a bijection (see
/// [`id_step`]), so WITHIN one scope consecutive seeds give unpredictable-looking ids that
/// never repeat and two kinds of one length never mint the same id — uniqueness inside a
/// scope is structural, for every seed a store can reach (the space is `36^6` ids at its
/// smallest, and `ID_KIND_STRIDE` seeds short of that is more resources than a collection
/// here can hold), with no collision retry, no exhaustion branch and no random source.
///
/// ACROSS scopes the separation is the starting point each scope is given
/// ([`scope_offset`]) rather than a structural one: two collections walk the space from
/// different points, so the n-th resource of one and the n-th of another differ, and a
/// coincidence needs two walks to land on the same value — one id in `36^len`.
fn mint_id(kind: IdKind, scope: &str, seed: u64) -> String {
    let len = kind.id_len();
    let space = id_space(len);
    let preimage = (u128::from(seed) * ID_KIND_STRIDE + kind.ordinal()) % space;
    let mut scrambled = (preimage * id_step(space) + scope_offset(scope, space)) % space;
    let mut id = Vec::with_capacity(len as usize);
    for _ in 0..len {
        let digit = usize::try_from(scrambled % 36).unwrap_or(0);
        id.push(ID_ALPHABET[digit]);
        scrambled /= 36;
    }
    String::from_utf8(id).unwrap_or_default()
}

/// The region this request is scoped to: the region of its SigV4 credential
/// scope, or [`DEFAULT_REGION`] for a request that carries no scope.
pub fn request_region(parts: &::http::request::Parts) -> String {
    ::emulator_server::sigv4_credential_scope(&parts.headers)
        .map(|scope| scope.region)
        .unwrap_or_else(|| DEFAULT_REGION.to_string())
}

/// The largest request payload an HTTP api's `execute-api` endpoint accepts for a route,
/// in bytes: 10 MiB.
///
/// It is the payload quota AWS publishes for an HTTP api and it is not adjustable — no
/// member of this model names it, because it is a quota of the data plane rather than of
/// the api resource. A body of exactly this many bytes is delivered to the route's
/// integration; one byte more is refused by the gateway itself, before the integration is
/// invoked at all (see `crate::service::invoke`).
///
/// It is a FIDELITY bound and not an admission control: the front door buffers a request
/// before any service sees it, so a body is already in memory by the time this is applied.
/// Nothing here should be read as a cap on what one request can allocate.
pub const MAX_HTTP_API_PAYLOAD_BYTES: usize = 10 * 1024 * 1024;

/// The first path segment that marks one of this service's data-plane endpoints carried IN
/// THE PATH rather than in the host: `/_aws/{endpoint}/{resource-id}/{rest}`.
///
/// A client on the machine this emulator runs on resolves none of the hosts AWS serves
/// these endpoints on, so it reaches the emulator's own address and carries the endpoint in
/// the path behind this marker. The segment is reserved for the emulator — it is not a path
/// any AWS endpoint serves — so reading it cannot take a request away from a route of an api
/// or a page of a portal, and it is the SAME spelling the API Gateway (REST) data plane of
/// this emulator is reached at (`/_aws/execute-api/{api-id}/{stage}/{resource-path}`).
pub const LOCAL_ENDPOINT_PREFIX: &str = "_aws";

/// The second label of every `execute-api` endpoint host — `{api-id}.execute-api.…` — and
/// the segment that names THIS service's api data plane behind
/// [`LOCAL_ENDPOINT_PREFIX`].
///
/// From `Api$ApiEndpoint`: "The URI of the API, of the form
/// `{api-id}.execute-api.{region}.amazonaws.com`". It lives here, beside the marker,
/// because the control plane ANNOUNCES it ([`announced_execute_api_endpoint`]) and the data
/// planes read it back (`crate::service::data_plane` re-exports it), so what a caller is
/// given and what is served are one constant.
pub const EXECUTE_API_LABEL: &str = "execute-api";

/// The endpoint this request was addressed at when that endpoint is the EMULATOR'S OWN —
/// a loopback address, an IP literal or `localhost`, which is what a client configured with
/// one local endpoint URL sends — and `None` when the host names a service, which is a
/// client that resolves AWS's own hosts.
///
/// The front door's own host reader decides it (`emulator_server::service_from_host`, the
/// last tier of the routing key every front door resolves), so no second rule for "which
/// host is a service's" is written here.
pub fn local_endpoint_host(parts: &::http::request::Parts) -> Option<String> {
    if ::emulator_server::service_from_host(&parts.headers).is_some() {
        return None;
    }
    let host = parts
        .headers
        .get(::http::header::HOST)
        .and_then(|value| value.to_str().ok())
        .or_else(|| parts.uri.host())?
        .trim();
    (!host.is_empty()).then(|| host.to_string())
}

/// The URI scheme of an api's default endpoint, chosen by its `ProtocolType`.
///
/// `ApiEndpoint` is the value a caller hands to a client library to reach the api, so
/// its scheme has to name the protocol that api speaks: a WebSocket API is opened with
/// an RFC 6455 handshake, whose URI scheme is `wss` (secure) — `ws` is the cleartext
/// form, and nothing this emulator serves is cleartext — while an HTTP API is invoked
/// over `https` ("clients can invoke your API with the default
/// https://{api_id}.execute-api.{region}.amazonaws.com endpoint",
/// `Api$DisableExecuteApiEndpoint`).
///
/// `ProtocolType` is an enum of exactly `HTTP | WEBSOCKET`
/// (`smithy_query(query="shape", shape="ProtocolType")`) and the boundary validator
/// refuses anything else, so `WEBSOCKET` and the `HTTP` arm cover every api a record
/// can hold; that arm doubles as the fallback a future enum value would take, which
/// gives it the scheme an api is invoked over rather than one no client could open.
pub fn execute_api_scheme(protocol_type: &str) -> &'static str {
    match protocol_type {
        WEBSOCKET_PROTOCOL_TYPE => "wss",
        // HTTP_PROTOCOL_TYPE, and anything the enum gains later.
        _ => "https",
    }
}

/// The default execute-api endpoint of an api: the URI clients of AWS reach it on when
/// no custom domain name is configured.
///
/// The host is the one `CreateApiResponse$ApiEndpoint` documents
/// ([`execute_api_host`]) and the scheme comes from the api's `ProtocolType` (see
/// [`execute_api_scheme`]). What this emulator ANNOUNCES for an api is
/// [`announced_execute_api_endpoint`], which answers this form only for a caller that
/// resolves AWS's own hosts.
pub fn execute_api_endpoint(api_id: &str, region: &str, protocol_type: &str) -> String {
    format!(
        "{}://{}",
        execute_api_scheme(protocol_type),
        execute_api_host(api_id, region)
    )
}

/// The default execute-api HOST of an api: the name AWS gives the endpoint an api is
/// reached on, `{api-id}.execute-api.{region}.amazonaws.com`
/// (`CreateApiResponse$ApiEndpoint`).
///
/// The one place that spells the host, so the endpoint a caller that resolves AWS's own
/// hosts is announced and the `domainName` an invoke of the api reports cannot drift
/// apart — that domain is the api's own whichever address a client had to reach this
/// emulator at.
pub fn execute_api_host(api_id: &str, region: &str) -> String {
    format!("{api_id}.{EXECUTE_API_LABEL}.{region}.amazonaws.com")
}

/// The `ApiEndpoint` this service ANNOUNCES for an api whose data plane is served through
/// this emulator's front door, resolved once when the api is created and stored, so every
/// later read reports the same value.
///
/// The member exists for a client to DIAL — "the stage name is typically appended to this
/// URI to form a complete path to a deployed API stage" (`Api$ApiEndpoint`) — so the
/// address announced has to be one this emulator itself answers on. Which address that is
/// follows the same rule [`announced_portal_default_domain_name`] applies to a portal's
/// website:
///
/// * a request that addressed the EMULATOR'S OWN endpoint
///   ([`reached_emulator_endpoint`]) comes from a client that resolves nothing else, so the
///   api is announced on that same endpoint with its own id behind the
///   [`LOCAL_ENDPOINT_PREFIX`] marker (`/_aws/execute-api/{api-id}`), the addressing
///   `crate::service::invoke` serves;
/// * an api created through any other request on an emulator that IS served on a port
///   ([`served_endpoint_authority`]) is announced on that port, the one address this
///   process answers on, so an api minted by an in-process call of another service
///   announces an address its caller can reach too;
/// * a request that addressed one of AWS's own hosts on an emulator served on no port at
///   all comes from a client that resolves them, so the api is announced on the host
///   [`execute_api_host`] builds — the shape `Api$ApiEndpoint` documents, which the data
///   plane claims host-style.
pub fn announced_execute_api_endpoint(
    api_id: &str,
    region: &str,
    protocol_type: &str,
    parts: &::http::request::Parts,
    served_base: &str,
) -> String {
    match reached_emulator_endpoint(parts).or_else(|| served_endpoint_authority(served_base)) {
        Some(authority) => format!(
            "{}://{authority}/{LOCAL_ENDPOINT_PREFIX}/{EXECUTE_API_LABEL}/{api_id}",
            execute_api_scheme(protocol_type)
        ),
        None => execute_api_endpoint(api_id, region, protocol_type),
    }
}

/// The EMULATOR'S OWN endpoint this request reached — an address or a loopback name, which
/// is what a client configured with one local endpoint URL sends — and `None` for a request
/// whose host names a service, which is a client that resolves AWS's own hosts.
///
/// The tree's one reading of that question
/// (`service_common::data_plane_addressing::reached_emulator_endpoint`), so this service
/// writes no second rule for which host is the emulator's own; it reads the host off the
/// header or off an absolute-form URI, as both carry it.
fn reached_emulator_endpoint(parts: &::http::request::Parts) -> Option<String> {
    ::service_common::data_plane_addressing::reached_emulator_endpoint(
        &parts.uri.to_string(),
        parts
            .headers
            .get(::http::header::HOST)
            .and_then(|value| value.to_str().ok()),
    )
}

/// The `host[:port]` this emulator is SERVED at, read off the base URL
/// `OperationContext::endpoint` reports ("the base URL a later request addresses… Callers
/// send normally signed requests to it"), or `None` for an emulator that is served on no
/// port at all — which is what its unserved base names
/// (`smithy_server_runtime::UNSERVED_URL_BASE`, a host RFC 6761 reserves so that nothing
/// resolves it).
///
/// Only a value SHAPED like a host is taken
/// (`service_common::data_plane_addressing::is_host_shaped`), so nothing that is not an
/// address reaches a URL this service announces.
fn served_endpoint_authority(served_base: &str) -> Option<String> {
    if served_base == ::smithy_server_runtime::UNSERVED_URL_BASE {
        return None;
    }
    let authority = served_base
        .split_once("://")
        .map_or(served_base, |(_scheme, rest)| rest)
        .trim_end_matches('/');
    ::service_common::data_plane_addressing::is_host_shaped(authority)
        .then(|| authority.to_string())
}

/// Read a boolean out of a query parameter the model types as `__string`.
///
/// A query parameter modelled as a string carries its boolean as text, so the parse is
/// the handler's: `true` and `false` in any case are the two values a boolean query
/// parameter spells, and anything else says neither. `member` names the parameter in the
/// message; `Err` carries the message for the operation's `BadRequestException`. A
/// parameter the request left out answers `false`, the behaviour of a flag nobody set.
pub fn parse_boolean_query(member: &str, raw: Option<&str>) -> Result<bool, String> {
    match raw {
        None => Ok(false),
        Some(value) if value.eq_ignore_ascii_case("true") => Ok(true),
        Some(value) if value.eq_ignore_ascii_case("false") => Ok(false),
        Some(value) => Err(format!(
            "Invalid value for {member}: '{value}'. {member} must be true or false."
        )),
    }
}

/// Resolve a `MaxResults` query value into the page size to serve — see
/// [`paging::parse_max_results`], which clamps it to [`MAX_PAGE_SIZE`].
pub use paging::parse_max_results;

/// Resolve a modelled-integer `MaxResults` into the page size to serve — see
/// [`paging::resolve_max_results`].
pub use paging::resolve_max_results;

/// The sentence a paginated reader refuses a `NextToken` it did not issue with — see
/// [`paging::INVALID_NEXT_TOKEN_MESSAGE`].
pub use paging::INVALID_NEXT_TOKEN_MESSAGE;

/// The collection a pagination token is issued for — see [`paging::page_scope`].
pub use paging::page_scope;

/// One page of a key-ordered collection — see [`paging::page`].
pub use paging::page;

/// One page of a collection in the order its records were created — see
/// [`paging::page_in_creation_order`].
pub use paging::page_in_creation_order;

/// The position a caller-supplied `NextToken` resumes a creation-ordered collection at —
/// see [`paging::page_position`].
pub use paging::page_position;

/// Whether a token is one this service issued for a collection — see
/// [`paging::token_is_for_scope`].
pub use paging::token_is_for_scope;

/// The token that resumes a collection at its first record — see
/// [`paging::first_page_token`].
pub use paging::first_page_token;

/// A record a collection reader reports in creation order — see [`paging::CreatedInOrder`].
pub use paging::CreatedInOrder;

/// The variable namespaces a selection expression may read — see
/// [`selection_expression::SELECTION_EXPRESSION_VARIABLES`].
pub use selection_expression::SELECTION_EXPRESSION_VARIABLES;

/// The reference a WEBSOCKET api's `RouteSelectionExpression` selects a route key with —
/// see [`selection_expression::WEBSOCKET_ROUTE_SELECTION_REFERENCE_PREFIX`].
pub use selection_expression::WEBSOCKET_ROUTE_SELECTION_REFERENCE_PREFIX;

/// Every variable reference a selection expression makes — see
/// [`selection_expression::selection_expression_references`].
pub use selection_expression::selection_expression_references;

/// The sentence a selection expression naming an unevaluated variable is refused with —
/// see [`selection_expression::unexpected_selection_variable_message`].
pub use selection_expression::unexpected_selection_variable_message;

/// Whether a `RouteSelectionExpression` can select a route key on an api of this
/// protocol — see [`selection_expression::validate_route_selection_expression`].
pub use selection_expression::validate_route_selection_expression;

/// Whether a selection expression names only variables this service evaluates — see
/// [`selection_expression::validate_selection_expression`].
pub use selection_expression::validate_selection_expression;

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsApigatewayv2Apigatewayv2State>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsApigatewayv2Apigatewayv2State>)
}

/// The SHARED cell naming who judges a `@connections` endpoint addressing: call it with a
/// handler's `&*self.engine`.
///
/// The three `@connections` operations are served on an api's own invoke endpoint, so such a
/// request carries one of THIS service's stages in its path; the service that models those
/// operations cannot read this store (it is a dependency of this crate, not the other way
/// round), so this service installs its own reading into the cell
/// ([`crate::service::connections_endpoint`]) and that service calls it. One cell, so there
/// is no copy of which stages are deployed to go stale — the engine keys state by TYPE, so a
/// cell of either service's own would be invisible to the other.
///
/// LOCK ORDER: the asking service releases this cell before it calls the reading, and the
/// reading then takes [`service_state`], so nothing holds one store while taking another.
///
/// Answer `None` by leaving the cell unwritten: an engine that carries no state at all judges
/// no addressing, which that service reads as "nothing judges this one".
pub fn connections_endpoint_authority<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<
    ::engine::ServiceState<
        ::service_common::execute_api_connections_endpoint::ConnectionsEndpointAuthority,
    >,
> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(
            ::engine::InMemoryEngine::state::<
                ::service_common::execute_api_connections_endpoint::ConnectionsEndpointAuthority,
            >,
        )
}

/// The SHARED certificate-association directory in the shared engine: call it with a
/// handler's `&*self.engine`.
///
/// A custom domain name PRESENTS the ACM certificates its configuration entries name
/// ([`DomainName::referenced_certificate_arns`]), and ACM — which holds those
/// certificates but cannot see this store — has to report those references and refuse to
/// delete or revoke a certificate while one remains. So a domain name's references are
/// recorded in ONE directory under one shared type
/// ([`::service_common::acm_certificate_associations::CertificateAssociationDirectory`]),
/// written by whichever call creates, changes or deletes the resource that holds them.
/// The engine keys state by TYPE, so a copy of its own inside
/// [`ComAmazonawsApigatewayv2Apigatewayv2State`] would be invisible to the reader.
///
/// LOCK ORDER: a writer takes [`service_state`] first and this directory second, and
/// nothing holds this one while taking another store, so the order cannot invert.
///
/// Answer `None` by leaving the directory unwritten: the record is an aid to another
/// service, never part of this operation's own answer.
pub fn certificate_association_directory<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<
    ::engine::ServiceState<
        ::service_common::acm_certificate_associations::CertificateAssociationDirectory,
    >,
> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(
            ::engine::InMemoryEngine::state::<
                ::service_common::acm_certificate_associations::CertificateAssociationDirectory,
            >,
        )
}
