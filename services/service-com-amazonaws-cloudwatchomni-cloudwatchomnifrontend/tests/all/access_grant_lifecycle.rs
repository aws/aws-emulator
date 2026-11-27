// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for CloudWatchOmni's access grants.
//!
//! Every call goes through a real operation handler holding the SAME shared engine the
//! router hands every service, so a create is observable by the reader that runs next.
//! Failures are the operations' own modelled errors, so each assertion checks the wire
//! facts that are requirements: the HTTP status and the error code.
//!
//! The suite asserts the RULES rather than one recorded exchange:
//!
//! * `CreateAccessGrant` mints a grant in an EXISTING space of an EXISTING domain,
//!   persists it and returns the persisted record, and `GetAccessGrant` answers from
//!   that same store;
//! * every derived member (`grantId`, `grantArn`, `accountId`, `grantType`, `createdBy`,
//!   `createdAt`/`updatedAt`) is computed from the store and the request rather than
//!   echoed from the body;
//! * a grant belongs to one space of one domain, so neither an unknown domain nor a
//!   space of another domain resolves;
//! * a name already identifying a grant of that space cannot identify a second one;
//! * `@idempotencyToken` replays return the original grant;
//! * the member combinations the model documents as invalid are refused with
//!   `ValidationException`;
//! * a grant whose scoped actions apply to an access profile is recorded against that
//!   profile, which is what `AccessProfile.assumeStatus` reports to a principal that does
//!   not own the profile;
//! * creating a space PROVISIONS the access control the service owns for it — one
//!   SERVICE_MANAGED access profile and two SERVICE_MANAGED grants, in the same store
//!   every reader of a profile or a grant reads — a call that would update or delete one
//!   of those rows is refused with the modelled `ConflictException`, and deleting the
//!   space releases them;
//! * `ListAccessGrants` reports the grants matching every filter supplied, in creation
//!   order, cut into pages of at most `maxResults` with a `nextToken` exactly while
//!   grants remain.
//!
//! Each assertion names its case and the actual value, so a failure identifies the row
//! of the table that broke.

use std::collections::HashMap;
use std::sync::Arc;

use aws_smithy_schema::codec::{Codec, FinishSerializer};
use aws_smithy_schema::serde::{SerializableStruct, ShapeSerializer};
use aws_smithy_schema::Schema;
use aws_smithy_schema_server::{CborCodec, DeserializableStruct};
use smithy_server_runtime::{
    EmbeddedEmulator, MultiServiceRouter, Operation, OperationContext, OperationError, Service,
};

use service_com_amazonaws_cloudwatchomni_cloudwatchomnifrontend as omni;

use omni::ops::create_access_grant::{
    CreateAccessGrant, CreateAccessGrantError, CreateAccessGrantErrorModeled,
};
use omni::ops::create_access_profile::CreateAccessProfile;
use omni::ops::create_domain::CreateDomain;
use omni::ops::create_space::CreateSpace;
use omni::ops::delete_access_grant::{DeleteAccessGrant, DeleteAccessGrantError};
use omni::ops::delete_access_profile::{DeleteAccessProfile, DeleteAccessProfileError};
use omni::ops::delete_space::DeleteSpace;
use omni::ops::get_access_grant::{GetAccessGrant, GetAccessGrantError};
use omni::ops::get_access_profile::{GetAccessProfile, GetAccessProfileError};
use omni::ops::list_access_grants::{ListAccessGrants, ListAccessGrantsError};
use omni::ops::list_access_profiles::{ListAccessProfiles, ListAccessProfilesError};
use omni::ops::update_access_profile::{UpdateAccessProfile, UpdateAccessProfileError};
use omni::ops::types::{
    AccessGrant, AccessGrantPrincipal, AccessGrantPrincipalAttribute, AccessGrantSummary,
    AccessProfileSummary, CreateAccessGrantInput, CreateAccessGrantOutput,
    CreateAccessProfileInput, CreateDomainInput, CreateSpaceInput, DeleteAccessGrantInput,
    DeleteAccessGrantOutput, DeleteAccessProfileInput, DeleteAccessProfileOutput, DeleteSpaceInput,
    GetAccessGrantInput, GetAccessGrantOutput, GetAccessProfileInput, GetAccessProfileOutput,
    ListAccessGrantsInput, ListAccessGrantsOutput, ListAccessProfilesInput,
    ListAccessProfilesOutput, ResourceScope, ScopedActions, UpdateAccessProfileInput,
    UpdateAccessProfileOutput, CREATE_ACCESS_GRANT_INPUT_SCHEMA, DELETE_ACCESS_GRANT_INPUT_SCHEMA,
    DELETE_ACCESS_PROFILE_INPUT_SCHEMA, GET_ACCESS_GRANT_INPUT_SCHEMA,
    LIST_ACCESS_GRANTS_INPUT_SCHEMA, UPDATE_ACCESS_PROFILE_INPUT_SCHEMA,
};
use omni::scaffolded::Scaffolded;
use omni::state::{
    agent_principal_id, ACCESS_GRANT_NAME_TAKEN, ACCESS_GRANT_PERMISSION_SPACE_ADMIN,
    ACCESS_GRANT_PRINCIPAL_TYPE_AGENT, ACCESS_GRANT_PRINCIPAL_TYPE_IAM_ROOT,
    ACCESS_GRANT_TYPE_CUSTOMER_MANAGED, ACCESS_GRANT_TYPE_SERVICE_MANAGED,
    ACCESS_PROFILE_RESOURCE_TYPE, ACCESS_PROFILE_TYPE_CUSTOMER_MANAGED,
    ACCESS_PROFILE_TYPE_SERVICE_MANAGED, AGENT_ASSUME_GRANT_NAME, ASSUME_STATUS_ALLOWED,
    ASSUME_STATUS_DENIED, ASYNC_WORKFLOWS_PROFILE_NAME, SERVICE_MANAGED_GRANT_NOT_DELETABLE,
    SERVICE_MANAGED_PROFILE_NOT_DELETABLE, SERVICE_MANAGED_PROFILE_NOT_UPDATABLE,
    SPACE_ADMIN_GRANT_NAME,
};

/// The region the suite signs for; the handlers record the signed region on each
/// resource, so every ARN below is expected to carry it.
const REGION: &str = "us-west-2";

/// The access key id the suite's `Authorization` header carries. `createdBy` is derived
/// from it, so a grant reports who called rather than a fabricated identity.
const ACCESS_KEY_ID: &str = "AKIDEMULATOR";

/// A role ARN satisfying `Arn`'s `^arn:.+$`, which `CreateSpace` requires.
const DATA_ACCESS_ROLE_ARN: &str = "arn:aws:iam::000000000000:role/OmniData";

/// `GrantId`'s modelled `@pattern` — the model doc reads "a v4 UUID assigned when the
/// grant is created".
const GRANT_ID_PATTERN: &str =
    r"^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$";

/// A well-formed `DomainId` (its modelled `^d-[0-9a-z]{1,25}$`) that names nothing.
const MISSING_DOMAIN_ID: &str = "d-0000000000";

/// A well-formed `SpaceId` (its modelled pattern) that names nothing.
const MISSING_SPACE_ID: &str = "00000000-0000-4000-8000-000000000000";

/// A well-formed `GrantId` that names nothing.
const MISSING_GRANT_ID: &str = "11111111-2222-4333-8444-555555555555";

/// Whether `value` satisfies [`GRANT_ID_PATTERN`].
///
/// Checked by hand rather than through a regex engine, so this crate declares no
/// dependency beyond the ones it already has. The predicate is exercised on values that
/// must FAIL it as well as pass ([`the_grant_id_pattern_is_checked_as_written`]).
fn is_grant_id(value: &str) -> bool {
    let groups: Vec<&str> = value.split('-').collect();
    groups.len() == 5
        && groups.iter().map(|group| group.len()).eq([8, 4, 4, 4, 12])
        && groups
            .iter()
            .all(|group| group.chars().all(|c| c.is_ascii_digit() || ('a'..='f').contains(&c)))
}

/// Whether `value` satisfies `Arn`'s modelled `^arn:.+$`.
fn is_arn(value: &str) -> bool {
    value
        .strip_prefix("arn:")
        .is_some_and(|rest| !rest.is_empty())
}

/// Whether `value` satisfies `AccountId`'s modelled `^[0-9]{12}$`.
fn is_account_id(value: &str) -> bool {
    value.len() == 12 && value.chars().all(|c| c.is_ascii_digit())
}

/// A timestamp at millisecond resolution — the resolution rpcv2Cbor encodes a
/// `Timestamp` at, so a value compared across a wire round trip is compared there.
fn millis(at: aws_smithy_types::DateTime) -> (i64, u32) {
    (at.secs(), at.subsec_nanos() / 1_000_000)
}

/// A SigV4 `Authorization` header whose credential scope names this service's signing
/// name (the router's routing key), the region the handlers record, and `access_key_id` —
/// which is the principal the handlers read the caller's identity from.
fn signed_request_parts(access_key_id: &str) -> http::request::Parts {
    signed_request_parts_in(access_key_id, REGION)
}

/// The same, signed for `region` rather than for the suite's own.
///
/// An account holds at most ONE space per Region, so a case needing a second space signs
/// that create for another Region ([`SPACE_REGIONS`]).
fn signed_request_parts_in(access_key_id: &str, region: &str) -> http::request::Parts {
    let (parts, ()) = http::Request::builder()
        .method("POST")
        .uri("/service/CloudWatchOmniFrontend/operation/Unused")
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential={access_key_id}/20260101/{region}/cloudwatch/aws4_request, \
                 SignedHeaders=host, Signature=emulator"
            ),
        )
        .body(())
        .expect("a well-formed request")
        .into_parts();
    parts
}

/// The Regions this suite signs its space creates for, from the suite that owns the
/// space rule: an account holds at most one space per Region, so each space a case needs
/// is created in a Region of its own.
use super::domain_space_lifecycle::SPACE_REGIONS;

/// One emulator: one shared engine, the service mounted on a router, and the request
/// metadata a handler reads.
struct Omni {
    engine: Arc<engine::InMemoryEngine>,
    service: omni::EmulatedService<engine::InMemoryEngine>,
    router: MultiServiceRouter,
    parts: http::request::Parts,
    /// The request metadata of a SECOND principal of the same account: a profile's own
    /// owner may always assume it, so what a GRANT authorizes is observed through a
    /// principal that owns nothing here.
    other_parts: http::request::Parts,
    /// How many spaces this emulator has created, which picks each create's Region out
    /// of [`SPACE_REGIONS`].
    spaces_created: std::sync::atomic::AtomicUsize,
}

impl Omni {
    fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        router.set_request_authentication(false);
        // The router's service and the operations constructed below all hold the SAME
        // engine, so both front doors read and write one store.
        router.register_service(omni::EmulatedService::new(Arc::clone(&engine)));
        Self {
            service: omni::EmulatedService::new(Arc::clone(&engine)),
            engine,
            router,
            parts: signed_request_parts(ACCESS_KEY_ID),
            other_parts: signed_request_parts("AKIDSOMEONEELSE"),
            spaces_created: std::sync::atomic::AtomicUsize::new(0),
        }
    }

    fn ctx(&self) -> OperationContext<'_> {
        OperationContext::new(
            &self.parts,
            self.service.definition().identity(),
            EmbeddedEmulator::from_router(self.router.clone()),
        )
    }

    /// The same request metadata, signed by another principal of the account.
    fn other_ctx(&self) -> OperationContext<'_> {
        OperationContext::new(
            &self.other_parts,
            self.service.definition().identity(),
            EmbeddedEmulator::from_router(self.router.clone()),
        )
    }

    /// A domain, through the handler that owns it.
    ///
    /// The create carries the `clientToken` it is admitted only with, derived from the
    /// domain's own name.
    async fn domain(&self, case: &str, name: &str) -> String {
        CreateDomain::new(Arc::clone(&self.engine))
            .handle(
                CreateDomainInput {
                    name: format!("omni-{name}"),
                    identity_providers: vec!["IAM".to_string()],
                    client_token: Some(format!("token-{name}")),
                    ..Default::default()
                },
                &self.ctx(),
            )
            .await
            .unwrap_or_else(|error| panic!("case {case:?}: CreateDomain failed: {error:?}"))
            .domain
            .domain_id
    }

    /// The Region the next space this emulator creates is signed for.
    ///
    /// An account holds at most ONE space per Region, so each space a case needs is
    /// created in a Region of its own; everything after the create addresses the space by
    /// its id, which no Region scopes.
    fn next_space_region(&self) -> &'static str {
        let index = self
            .spaces_created
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        SPACE_REGIONS
            .get(index)
            .copied()
            .expect("the suite signs each space it creates for a Region of its own")
    }

    /// The request metadata a space create signed for `region` carries.
    fn space_parts(&self, region: &str) -> http::request::Parts {
        signed_request_parts_in(ACCESS_KEY_ID, region)
    }

    /// A space of `domain_id`, through the handler that owns it.
    async fn space(&self, case: &str, domain_id: &str, name: &str) -> String {
        let parts = self.space_parts(self.next_space_region());
        let ctx = OperationContext::new(
            &parts,
            self.service.definition().identity(),
            EmbeddedEmulator::from_router(self.router.clone()),
        );
        CreateSpace::new(Arc::clone(&self.engine))
            .handle(
                CreateSpaceInput {
                    name: name.to_string(),
                    domain_id: domain_id.to_string(),
                    data_access_role_arn: DATA_ACCESS_ROLE_ARN.to_string(),
                    ..Default::default()
                },
                &ctx,
            )
            .await
            .unwrap_or_else(|error| panic!("case {case:?}: CreateSpace failed: {error:?}"))
            .space
            .space_id
    }

    /// A domain with one space in it, which is what a grant needs.
    async fn domain_and_space(&self, case: &str, name: &str) -> (String, String) {
        let domain_id = self.domain(case, name).await;
        let space_id = self.space(case, &domain_id, name).await;
        (domain_id, space_id)
    }

    /// An access profile of `space_id`, and its ARN.
    async fn access_profile(&self, case: &str, space_id: &str, name: &str) -> (String, String) {
        let profile = CreateAccessProfile::new(Arc::clone(&self.engine))
            .handle(
                CreateAccessProfileInput {
                    space_id: space_id.to_string(),
                    name: name.to_string(),
                    ..Default::default()
                },
                &self.ctx(),
            )
            .await
            .unwrap_or_else(|error| panic!("case {case:?}: CreateAccessProfile failed: {error:?}"))
            .access_profile;
        (profile.profile_id, profile.arn)
    }

    /// The `assumeStatus` the reader reports for a profile to a principal that does NOT
    /// own it — which is the principal a trust grant has to authorize, since the
    /// profile's own owner may assume it with no grant in place.
    async fn assume_status(&self, case: &str, space_id: &str, profile_id: &str) -> String {
        GetAccessProfile::new(Arc::clone(&self.engine))
            .handle(
                GetAccessProfileInput {
                    space_id: space_id.to_string(),
                    profile_id: profile_id.to_string(),
                },
                &self.other_ctx(),
            )
            .await
            .unwrap_or_else(|error| panic!("case {case:?}: GetAccessProfile failed: {error:?}"))
            .access_profile
            .assume_status
            .unwrap_or_else(|| panic!("case {case:?}: the profile reported no assumeStatus"))
    }

    /// The `assumeStatus` the reader reports for a profile to the principal that CREATED
    /// it — its owner, which the member allows whatever grants exist.
    async fn assume_status_for_the_owner(
        &self,
        case: &str,
        space_id: &str,
        profile_id: &str,
    ) -> String {
        GetAccessProfile::new(Arc::clone(&self.engine))
            .handle(
                GetAccessProfileInput {
                    space_id: space_id.to_string(),
                    profile_id: profile_id.to_string(),
                },
                &self.ctx(),
            )
            .await
            .unwrap_or_else(|error| panic!("case {case:?}: GetAccessProfile failed: {error:?}"))
            .access_profile
            .assume_status
            .unwrap_or_else(|| panic!("case {case:?}: the profile reported no assumeStatus"))
    }

    async fn create_access_grant(
        &self,
        input: &CreateAccessGrantInput,
    ) -> Result<CreateAccessGrantOutput, CreateAccessGrantError> {
        CreateAccessGrant::new(Arc::clone(&self.engine))
            .handle(input.clone(), &self.ctx())
            .await
    }

    async fn get_access_grant(
        &self,
        grant_id: &str,
    ) -> Result<GetAccessGrantOutput, GetAccessGrantError> {
        GetAccessGrant::new(Arc::clone(&self.engine))
            .handle(
                GetAccessGrantInput {
                    grant_id: grant_id.to_string(),
                },
                &self.ctx(),
            )
            .await
    }

    /// Dispatch one rpcv2Cbor request over HTTP and return the status and body.
    async fn over_the_wire<I: SerializableStruct>(
        &self,
        operation: &str,
        input_schema: &Schema,
        input: &I,
    ) -> (u16, bytes::Bytes) {
        let codec = CborCodec::default();
        let mut serializer = codec.create_serializer();
        serializer
            .write_struct(input_schema, input)
            .expect("the input shape serializes");
        let mut builder = http::Request::builder()
            .method("POST")
            .uri(format!(
                "/service/CloudWatchOmniFrontend/operation/{operation}"
            ))
            .header("content-type", "application/cbor")
            .header("smithy-protocol", "rpc-v2-cbor");
        for (name, value) in &self.parts.headers {
            builder = builder.header(name, value);
        }
        let request = builder
            .body(bytes::Bytes::from(serializer.finish()))
            .expect("a well-formed rpcv2Cbor request");
        let response = self.router.dispatch(request).await;
        (response.status().as_u16(), response.body().clone())
    }
}

/// Require a modelled failure with the status and error code the model declares for it.
fn expect_modelled_error<O: std::fmt::Debug, E: OperationError + std::fmt::Debug>(
    case: &str,
    result: Result<O, E>,
    expected_status: u16,
    expected_code: &str,
) {
    match result {
        Ok(output) => panic!(
            "case {case:?}: expected {expected_code} ({expected_status}), got Ok({output:?})"
        ),
        Err(error) => {
            assert_eq!(
                error.shape_name(),
                expected_code,
                "case {case:?}: expected error code {expected_code}, got {error:?}"
            );
            assert_eq!(
                error.http_status(),
                expected_status,
                "case {case:?}: expected HTTP {expected_status} for {expected_code}, got {}",
                error.http_status()
            );
        }
    }
}

/// A principal naming an id, which is what every principal type accepts.
fn principal(principal_type: &str, principal_id: &str) -> AccessGrantPrincipal {
    AccessGrantPrincipal {
        principal_type: principal_type.to_string(),
        principal_id: Some(principal_id.to_string()),
        principal_attributes: None,
    }
}

/// One group of scoped actions, limited to the resources `resources` name.
fn scoped_actions(actions: &[&str], resources: Option<Vec<ResourceScope>>) -> ScopedActions {
    ScopedActions {
        actions: actions.iter().map(|action| action.to_string()).collect(),
        resources,
        context_conditions: None,
    }
}

/// A resource scope naming specific ARNs of one resource type.
fn resource_scope(resource_type: &str, resource_arns: Option<Vec<String>>) -> ResourceScope {
    ResourceScope {
        resource_type: resource_type.to_string(),
        resource_arns,
        tags: None,
        signal_types: None,
        row_scope_groups: None,
    }
}

/// The smallest grant request the model admits, for the parents `domain_id`/`space_id`.
fn create_input(domain_id: &str, space_id: &str, name: &str) -> CreateAccessGrantInput {
    CreateAccessGrantInput {
        domain_id: domain_id.to_string(),
        space_id: space_id.to_string(),
        name: name.to_string(),
        principal: principal("IAM_ROLE", "arn:aws:iam::000000000000:role/Reader"),
        permission: "READ".to_string(),
        scoped_actions: None,
        tags: None,
        client_token: None,
    }
}

/// Every member of a grant but its timestamps, as a comparable tuple. The structures
/// (`principal`, `scopedActions`) are compared through their rendering, which is what
/// keeps them comparable without the model's shapes deriving equality.
type GrantFields = (
    String,
    String,
    Option<String>,
    String,
    String,
    String,
    String,
    String,
    String,
    String,
    String,
);

fn fields(grant: &AccessGrant) -> GrantFields {
    (
        grant.grant_id.clone(),
        grant.grant_arn.clone(),
        grant.name.clone(),
        grant.account_id.clone(),
        grant.domain_id.clone(),
        grant.space_id.clone(),
        grant.permission.clone(),
        grant.grant_type.clone(),
        grant.created_by.clone(),
        format!("{:?}", grant.principal),
        format!("{:?}", grant.scoped_actions),
    )
}

/// One row of the `CreateAccessGrant` table: the request members that vary.
struct GrantCase {
    case: &'static str,
    name: String,
    permission: &'static str,
    principal: AccessGrantPrincipal,
    scoped_actions: Option<Vec<ScopedActions>>,
    tags: Option<HashMap<String, String>>,
}

/// The rows span what the rule generalises over: every `AccessGrantPermission` the model
/// declares (`smithy_query(query="shape",shape="AccessGrantPermission")`: SPACE_ADMIN,
/// READ, READ_WRITE_DELETE, CUSTOM), principals named by id and by attributes across
/// several `AccessGrantPrincipalType` values, `name` over its whole `@length(1..64)` and
/// `^[a-zA-Z0-9_-]+$` class, `scopedActions` absent, one group and the model's
/// `@length(max: 20)`, and `tags` absent, one and many.
fn grant_cases() -> Vec<GrantCase> {
    vec![
        GrantCase {
            case: "the shortest name the model admits, READ for an IAM role",
            name: "a".to_string(),
            permission: "READ",
            principal: principal("IAM_ROLE", "arn:aws:iam::000000000000:role/Reader"),
            scoped_actions: None,
            tags: None,
        },
        GrantCase {
            case: "SPACE_ADMIN for an Identity Center user named by attributes",
            name: "space-admins".to_string(),
            permission: "SPACE_ADMIN",
            principal: AccessGrantPrincipal {
                principal_type: "IDC_USER".to_string(),
                principal_id: None,
                principal_attributes: Some(vec![AccessGrantPrincipalAttribute {
                    key: "department".to_string(),
                    value: "observability".to_string(),
                }]),
            },
            scoped_actions: None,
            tags: Some(HashMap::from([("team".to_string(), "sre".to_string())])),
        },
        GrantCase {
            case: "READ_WRITE_DELETE for an alert, narrowed to one resource type",
            name: "alert_writer".to_string(),
            permission: "READ_WRITE_DELETE",
            principal: principal("ALERT", "alert-high-latency"),
            scoped_actions: Some(vec![scoped_actions(
                &["cloudwatch:GetTelemetry"],
                Some(vec![resource_scope("DataSet", None)]),
            )]),
            tags: Some(HashMap::new()),
        },
        GrantCase {
            case: "the longest name the model admits, CUSTOM with an explicit action list",
            name: "n".repeat(64),
            permission: "CUSTOM",
            principal: principal("AGENT", "agent/telemetry-reader"),
            scoped_actions: Some(vec![scoped_actions(
                &["cloudwatch:GetTelemetry", "logs:StartQuery"],
                Some(vec![resource_scope(
                    "OmniDashboard",
                    Some(vec![
                        "arn:aws:cloudwatch:us-west-2:000000000000:omni-dashboard/one".to_string(),
                    ]),
                )]),
            )]),
            tags: None,
        },
        GrantCase {
            case: "the whole name character class, many tags, scopedActions at its bound",
            name: "Grant_9-NAME".to_string(),
            permission: "CUSTOM",
            principal: principal("IAM_USER", "arn:aws:iam::000000000000:user/analyst"),
            scoped_actions: Some(
                (0..20)
                    .map(|index| {
                        scoped_actions(
                            &["cloudwatch:GetTelemetry"],
                            Some(vec![resource_scope(&format!("DataSet{index}"), None)]),
                        )
                    })
                    .collect(),
            ),
            tags: Some(
                (0..50)
                    .map(|index| (format!("key{index}"), format!("value{index}")))
                    .collect(),
            ),
        },
    ]
}

#[tokio::test]
async fn the_grant_id_pattern_is_checked_as_written() {
    // Each row: the value, and whether the modelled pattern admits it.
    for (value, expected) in [
        ("11111111-2222-4333-8444-555555555555", true),
        ("00000000-0000-0000-0000-000000000000", true),
        ("abcdefab-cdef-abcd-efab-cdefabcdefab", true),
        ("ABCDEFAB-CDEF-ABCD-EFAB-CDEFABCDEFAB", false),
        ("11111111-2222-4333-8444-55555555555", false),
        ("11111111222243338444555555555555", false),
        ("gggggggg-2222-4333-8444-555555555555", false),
        ("", false),
    ] {
        assert_eq!(
            is_grant_id(value),
            expected,
            "case {value:?}: {GRANT_ID_PATTERN} should {} it",
            if expected { "admit" } else { "reject" }
        );
    }

    for (value, expected) in [("000000000000", true), ("00000000000", false), ("0000000000a", false)] {
        assert_eq!(
            is_account_id(value),
            expected,
            "case {value:?}: ^[0-9]{{12}}$ should {} it",
            if expected { "admit" } else { "reject" }
        );
    }

    for (value, expected) in [("arn:aws:cloudwatch:us-west-2:000000000000:access-grant/g", true), ("arn:", false)] {
        assert_eq!(
            is_arn(value),
            expected,
            "case {value:?}: ^arn:.+$ should {} it",
            if expected { "admit" } else { "reject" }
        );
    }
}

#[tokio::test]
async fn create_access_grant_persists_a_record_get_access_grant_returns_it() {
    let omni = Omni::new();
    let (domain_id, space_id) = omni.domain_and_space("table", "grant-table").await;
    let mut minted: Vec<String> = Vec::new();

    for row in grant_cases() {
        let case = row.case;
        let created = omni
            .create_access_grant(&CreateAccessGrantInput {
                domain_id: domain_id.clone(),
                space_id: space_id.clone(),
                name: row.name.clone(),
                principal: row.principal.clone(),
                permission: row.permission.to_string(),
                scoped_actions: row.scoped_actions.clone(),
                tags: row.tags.clone(),
                client_token: None,
            })
            .await
            .unwrap_or_else(|error| panic!("case {case:?}: CreateAccessGrant failed: {error:?}"))
            .access_grant;

        // The id is minted by the service, inside what `GrantId` models.
        assert!(
            is_grant_id(&created.grant_id),
            "case {case:?}: grantId {:?} does not satisfy {GRANT_ID_PATTERN}",
            created.grant_id
        );
        assert!(
            !minted.contains(&created.grant_id),
            "case {case:?}: grantId {:?} repeats an earlier grant's id",
            created.grant_id
        );
        minted.push(created.grant_id.clone());

        // The ARN is derived from this grant alone, in the signed region.
        assert!(
            is_arn(&created.grant_arn),
            "case {case:?}: grantArn {:?} does not satisfy ^arn:.+$",
            created.grant_arn
        );
        assert!(
            created
                .grant_arn
                .ends_with(&format!(":access-grant/{}", created.grant_id)),
            "case {case:?}: grantArn {:?} does not name this grant alone",
            created.grant_arn
        );
        assert!(
            created.grant_arn.contains(&format!(":{REGION}:")),
            "case {case:?}: grantArn {:?} does not carry the signed region {REGION}",
            created.grant_arn
        );

        // The request's own members come back as sent, and the derived ones are the
        // documented values for a freshly created, customer-created grant.
        assert_eq!(
            (
                created.domain_id.as_str(),
                created.space_id.as_str(),
                created.name.as_deref(),
                created.permission.as_str(),
                created.grant_type.as_str(),
            ),
            (
                domain_id.as_str(),
                space_id.as_str(),
                Some(row.name.as_str()),
                row.permission,
                ACCESS_GRANT_TYPE_CUSTOMER_MANAGED,
            ),
            "case {case:?}: the created grant is {created:?}"
        );
        // The principal comes back as sent, with its attribute conditions reported as
        // an empty list when the request named none: `principalAttributes` is a member
        // of every principal, so it is reported empty rather than dropped.
        assert_eq!(
            format!("{:?}", created.principal),
            format!(
                "{:?}",
                AccessGrantPrincipal {
                    principal_attributes: Some(
                        row.principal.principal_attributes.clone().unwrap_or_default()
                    ),
                    ..row.principal.clone()
                }
            ),
            "case {case:?}: expected the principal as sent, got {:?}",
            created.principal
        );
        assert_eq!(
            format!("{:?}", created.scoped_actions),
            format!("{:?}", row.scoped_actions),
            "case {case:?}: expected the scoped actions as sent, got {:?}",
            created.scoped_actions
        );
        assert!(
            is_account_id(&created.account_id),
            "case {case:?}: accountId {:?} does not satisfy ^[0-9]{{12}}$",
            created.account_id
        );
        // `createdBy` reports the principal that called, which is the request's own
        // access key id rather than a constant.
        assert!(
            is_arn(&created.created_by) && created.created_by.ends_with(ACCESS_KEY_ID),
            "case {case:?}: createdBy {:?} does not name the calling principal {ACCESS_KEY_ID}",
            created.created_by
        );
        assert_eq!(
            millis(created.created_at),
            millis(created.updated_at),
            "case {case:?}: a grant nothing has updated reports createdAt {:?} and updatedAt {:?}",
            created.created_at,
            created.updated_at
        );

        // The reader answers from the store, verbatim and stably.
        for read in ["first read", "second read"] {
            let fetched = omni
                .get_access_grant(&created.grant_id)
                .await
                .unwrap_or_else(|error| {
                    panic!("case {case:?} ({read}): GetAccessGrant failed: {error:?}")
                })
                .access_grant;
            assert_eq!(
                fields(&fetched),
                fields(&created),
                "case {case:?} ({read}): the stored grant is {fetched:?}, the created one {created:?}"
            );
            assert_eq!(
                (millis(fetched.created_at), millis(fetched.updated_at)),
                (millis(created.created_at), millis(created.updated_at)),
                "case {case:?} ({read}): the stored timestamps differ from the created ones"
            );
        }
    }
}

#[tokio::test]
async fn a_grant_is_created_in_a_space_of_a_domain_the_store_holds() {
    let omni = Omni::new();
    let (domain_id, space_id) = omni.domain_and_space("home", "grant-home").await;
    // A second space of the SAME domain: an account holds at most one domain, so a
    // space under another domain cannot exist beside this one, and the rows below
    // name a domain the store does not hold instead.
    let other_space_id = omni.space("other", &domain_id, "grant-other-space").await;

    // The pair the store holds resolves.
    omni.create_access_grant(&create_input(&domain_id, &space_id, "resolves"))
        .await
        .expect("case \"its own domain and space\": the grant is created");

    // Rows: each is a parent the store does not hold under the pair the request names.
    for (case, domain, space) in [
        ("a domain the store does not hold", MISSING_DOMAIN_ID, space_id.as_str()),
        ("a space the store does not hold", domain_id.as_str(), MISSING_SPACE_ID),
        (
            "a domain the store does not hold with a space it does",
            MISSING_DOMAIN_ID,
            other_space_id.as_str(),
        ),
    ] {
        expect_modelled_error(
            case,
            omni.create_access_grant(&create_input(domain, space, "orphan"))
                .await,
            404,
            "ResourceNotFoundException",
        );
    }

    // A read names the grant alone, so an id the store does not hold is the same 404 —
    // and AWS's sentence for it names the kind of resource alone, quoting no `grantId`.
    let missed = omni
        .get_access_grant(MISSING_GRANT_ID)
        .await
        .expect_err("case \"a read of a grant the store does not hold\": expected a refusal");
    assert_eq!(
        (missed.shape_name(), missed.http_status()),
        ("ResourceNotFoundException", 404),
        "case \"a read of a grant the store does not hold\": got {missed:?}"
    );
    let rendered = format!("{missed:?}");
    assert!(
        rendered.contains("message: \"AccessGrant not found\""),
        "case \"a read of a grant the store does not hold\": expected the message \
         \"AccessGrant not found\", got {rendered:?}"
    );
}

#[tokio::test]
async fn a_name_identifies_one_grant_per_space() {
    let omni = Omni::new();
    let (domain_id, first) = omni.domain_and_space("first", "grant-names-a").await;
    let second = omni.space("second", &domain_id, "grant-names-b").await;

    // Rows span the name class again: each is created once, refused the second time in
    // the same space, and accepted in another space.
    for name in ["a", "Grant_9-NAME", &"n".repeat(64)] {
        let created = omni
            .create_access_grant(&create_input(&domain_id, &first, name))
            .await
            .unwrap_or_else(|error| panic!("case {name:?}: the first create failed: {error:?}"))
            .access_grant;
        expect_modelled_error(
            name,
            omni.create_access_grant(&create_input(&domain_id, &first, name))
                .await,
            409,
            "ConflictException",
        );
        let elsewhere = omni
            .create_access_grant(&create_input(&domain_id, &second, name))
            .await
            .unwrap_or_else(|error| {
                panic!("case {name:?}: the same name in another space failed: {error:?}")
            })
            .access_grant;
        assert_ne!(
            elsewhere.grant_id, created.grant_id,
            "case {name:?}: the same name in another space is a different grant, got {:?} twice",
            elsewhere.grant_id
        );
        assert_eq!(
            elsewhere.space_id, second,
            "case {name:?}: expected spaceId {second}, got {:?}",
            elsewhere.space_id
        );
    }
}

/// The message `CreateAccessGrant`'s modelled `ConflictException` carries, or a panic
/// naming what came back instead.
fn grant_conflict_message<O: std::fmt::Debug>(
    case: &str,
    result: Result<O, CreateAccessGrantError>,
) -> String {
    match result {
        Ok(output) => panic!("case {case:?}: expected a ConflictException, got Ok({output:?})"),
        Err(error) => {
            assert_eq!(
                error.http_status(),
                409,
                "case {case:?}: expected HTTP 409, got {} ({error:?})",
                error.http_status()
            );
            match error {
                Scaffolded::Modeled(CreateAccessGrantErrorModeled::ConflictException(refusal)) => {
                    refusal.message
                }
                other => {
                    panic!("case {case:?}: expected the modelled ConflictException, got {other:?}")
                }
            }
        }
    }
}

#[tokio::test]
async fn a_name_a_space_already_holds_is_refused_with_the_services_one_sentence_for_a_grant() {
    let omni = Omni::new();
    let (domain_id, space_id) = omni.domain_and_space("conflict", "grant-conflict").await;

    // Rows span `AccessGrantName`'s class again, because the refusal is one sentence
    // that states neither the name nor the space: nothing the request carried is a
    // variable of it.
    for name in ["a", "Grant_9-NAME", &"n".repeat(64)] {
        omni.create_access_grant(&create_input(&domain_id, &space_id, name))
            .await
            .unwrap_or_else(|error| panic!("case {name:?}: the first create failed: {error:?}"));
        let message = grant_conflict_message(
            name,
            omni.create_access_grant(&create_input(&domain_id, &space_id, name))
                .await,
        );
        assert_eq!(
            message, ACCESS_GRANT_NAME_TAKEN,
            "case {name:?}: expected {ACCESS_GRANT_NAME_TAKEN:?}, got {message:?}"
        );
        assert!(
            !message.contains(&space_id),
            "case {name:?}: the refusal names the space: {message:?}"
        );
        // A one- or two-character name can occur inside the sentence's own words, so the
        // name is required withheld for the rows long enough to be recognisable.
        assert!(
            name.len() <= 3 || !message.contains(name),
            "case {name:?}: the refusal names the requested name: {message:?}"
        );
    }
}

#[tokio::test]
async fn an_idempotency_token_replay_returns_the_original_grant() {
    let omni = Omni::new();
    let (domain_id, space_id) = omni.domain_and_space("tokens", "grant-tokens").await;

    // Rows span `ClientToken`'s `@length(1..64)` and its `^[!-~]+$` character class.
    for (index, token) in ["!", "a-token", "~{}[]|\\^`", &"t".repeat(64)]
        .into_iter()
        .enumerate()
    {
        let input = CreateAccessGrantInput {
            client_token: Some(token.to_string()),
            ..create_input(&domain_id, &space_id, &format!("token-grant-{index}"))
        };
        let created = omni
            .create_access_grant(&input)
            .await
            .unwrap_or_else(|error| panic!("case {token:?}: the first create failed: {error:?}"))
            .access_grant;
        let replayed = omni
            .create_access_grant(&input)
            .await
            .unwrap_or_else(|error| panic!("case {token:?}: the replay failed: {error:?}"))
            .access_grant;
        assert_eq!(
            fields(&replayed),
            fields(&created),
            "case {token:?}: the replay returned {replayed:?} rather than the original {created:?}"
        );

        // A DIFFERENT token for the same request is a different grant, so the replay is
        // the token's doing rather than the name's.
        let fresh = omni
            .create_access_grant(&CreateAccessGrantInput {
                client_token: Some(format!("{token}x")),
                name: format!("token-grant-{index}-fresh"),
                ..input.clone()
            })
            .await
            .unwrap_or_else(|error| panic!("case {token:?}: a fresh token failed: {error:?}"))
            .access_grant;
        assert_ne!(
            fresh.grant_id, created.grant_id,
            "case {token:?}: a fresh token must mint a new grant, got {:?} twice",
            fresh.grant_id
        );
    }
}

#[tokio::test]
async fn the_member_combinations_the_model_forbids_are_refused() {
    let omni = Omni::new();
    let (domain_id, space_id) = omni.domain_and_space("validation", "grant-validation").await;

    // Rows: each is a request the model's own documentation rules out —
    // a value outside an enum's declared set, a principal naming nobody
    // (`AccessGrantPrincipal`: "Specify principalId, principalAttributes, or both"),
    // attributes on a principal type that does not support them ("Supported only for
    // IDC_USER principals"), and CUSTOM without the action list it consists of
    // ("Access limited to an explicit list of actions, specified in scopedActions").
    let attributes = Some(vec![AccessGrantPrincipalAttribute {
        key: "department".to_string(),
        value: "observability".to_string(),
    }]);
    for (case, input) in [
        (
            "a permission the model does not declare",
            CreateAccessGrantInput {
                permission: "FULL_CONTROL".to_string(),
                ..create_input(&domain_id, &space_id, "bad-permission")
            },
        ),
        (
            "a principalType the model does not declare",
            CreateAccessGrantInput {
                principal: principal("IAM_GROUP", "analysts"),
                ..create_input(&domain_id, &space_id, "bad-principal-type")
            },
        ),
        (
            "a principal naming neither an id nor attributes",
            CreateAccessGrantInput {
                principal: AccessGrantPrincipal {
                    principal_type: "IDC_USER".to_string(),
                    principal_id: None,
                    principal_attributes: None,
                },
                ..create_input(&domain_id, &space_id, "no-principal")
            },
        ),
        (
            "a principal whose only attributes list is empty",
            CreateAccessGrantInput {
                principal: AccessGrantPrincipal {
                    principal_type: "IDC_USER".to_string(),
                    principal_id: None,
                    principal_attributes: Some(Vec::new()),
                },
                ..create_input(&domain_id, &space_id, "empty-attributes")
            },
        ),
        (
            "principalAttributes on a principal type that does not support them",
            CreateAccessGrantInput {
                principal: AccessGrantPrincipal {
                    principal_type: "IAM_USER".to_string(),
                    principal_id: Some("arn:aws:iam::000000000000:user/analyst".to_string()),
                    principal_attributes: attributes.clone(),
                },
                ..create_input(&domain_id, &space_id, "attributes-on-iam-user")
            },
        ),
        (
            "CUSTOM with no scopedActions",
            CreateAccessGrantInput {
                permission: "CUSTOM".to_string(),
                ..create_input(&domain_id, &space_id, "custom-without-actions")
            },
        ),
        (
            "CUSTOM with an empty scopedActions list",
            CreateAccessGrantInput {
                permission: "CUSTOM".to_string(),
                scoped_actions: Some(Vec::new()),
                ..create_input(&domain_id, &space_id, "custom-with-empty-actions")
            },
        ),
    ] {
        expect_modelled_error(
            case,
            omni.create_access_grant(&input).await,
            400,
            "ValidationException",
        );
    }

    // The same requests with the forbidden combination removed are accepted, so the
    // refusals above are the rule rather than a blanket no.
    omni.create_access_grant(&CreateAccessGrantInput {
        principal: AccessGrantPrincipal {
            principal_type: "IDC_USER".to_string(),
            principal_id: None,
            principal_attributes: attributes,
        },
        ..create_input(&domain_id, &space_id, "attributes-on-an-idc-user")
    })
    .await
    .expect("case \"attributes on an IDC_USER principal\": the grant is created");
    omni.create_access_grant(&CreateAccessGrantInput {
        permission: "CUSTOM".to_string(),
        scoped_actions: Some(vec![scoped_actions(&["cloudwatch:GetTelemetry"], None)]),
        ..create_input(&domain_id, &space_id, "custom-with-actions")
    })
    .await
    .expect("case \"CUSTOM with an action list\": the grant is created");
}

#[tokio::test]
async fn a_grant_scoped_to_an_access_profile_is_recorded_against_it() {
    let omni = Omni::new();
    let (domain_id, space_id) = omni.domain_and_space("trust", "grant-trust").await;
    let (first_profile, first_arn) = omni.access_profile("trust", &space_id, "first").await;
    let (second_profile, _) = omni.access_profile("trust", &space_id, "second").await;
    let elsewhere_space = omni.space("trust", &domain_id, "grant-trust-other").await;
    let (elsewhere_profile, _) = omni
        .access_profile("trust", &elsewhere_space, "elsewhere")
        .await;

    // Before any grant, no profile is assumable by a principal that does not own it:
    // nothing authorizes it.
    for (case, space, profile) in [
        ("the named profile", space_id.as_str(), first_profile.as_str()),
        ("another profile of the space", space_id.as_str(), second_profile.as_str()),
        (
            "a profile of another space",
            elsewhere_space.as_str(),
            elsewhere_profile.as_str(),
        ),
    ] {
        let status = omni.assume_status(case, space, profile).await;
        assert_eq!(
            status, ASSUME_STATUS_DENIED,
            "case {case:?}: with no grant recorded, expected {ASSUME_STATUS_DENIED}, got {status}"
        );
    }

    // A grant whose scoped actions are limited to one profile's ARN applies to that
    // profile and to no other.
    omni.create_access_grant(&CreateAccessGrantInput {
        permission: "CUSTOM".to_string(),
        principal: principal("ALERT", "alert-high-latency"),
        scoped_actions: Some(vec![scoped_actions(
            &["cloudwatch:GetTelemetry"],
            Some(vec![resource_scope(
                ACCESS_PROFILE_RESOURCE_TYPE,
                Some(vec![first_arn.clone()]),
            )]),
        )]),
        ..create_input(&domain_id, &space_id, "assume-the-first-profile")
    })
    .await
    .expect("the grant naming one profile's ARN is created");
    for (case, space, profile, expected) in [
        (
            "the profile the grant names",
            space_id.as_str(),
            first_profile.as_str(),
            ASSUME_STATUS_ALLOWED,
        ),
        (
            "a profile the grant does not name",
            space_id.as_str(),
            second_profile.as_str(),
            ASSUME_STATUS_DENIED,
        ),
        (
            "a profile of another space",
            elsewhere_space.as_str(),
            elsewhere_profile.as_str(),
            ASSUME_STATUS_DENIED,
        ),
    ] {
        let status = omni.assume_status(case, space, profile).await;
        assert_eq!(
            status, expected,
            "case {case:?}: expected {expected}, got {status}"
        );
    }

    // A resource scope of the profile type that narrows to no ARNs defaults to "*"
    // (`ResourceScope.resourceArns`: "When absent, defaults to \"*\""), so it applies to
    // every profile of ITS space — and still to none of another space's.
    omni.create_access_grant(&CreateAccessGrantInput {
        permission: "CUSTOM".to_string(),
        principal: principal("ALERT", "alert-every-profile"),
        scoped_actions: Some(vec![scoped_actions(
            &["cloudwatch:GetTelemetry"],
            Some(vec![resource_scope(ACCESS_PROFILE_RESOURCE_TYPE, None)]),
        )]),
        ..create_input(&domain_id, &space_id, "assume-every-profile")
    })
    .await
    .expect("the grant scoping the whole profile type is created");
    for (case, space, profile, expected) in [
        (
            "every profile of the grant's space",
            space_id.as_str(),
            second_profile.as_str(),
            ASSUME_STATUS_ALLOWED,
        ),
        (
            "a profile of another space",
            elsewhere_space.as_str(),
            elsewhere_profile.as_str(),
            ASSUME_STATUS_DENIED,
        ),
    ] {
        let status = omni.assume_status(case, space, profile).await;
        assert_eq!(
            status, expected,
            "case {case:?}: expected {expected}, got {status}"
        );
    }

    // A grant with no scoped actions at all names no resource, so it records nothing
    // against the profiles of its own space.
    let (bare_space, bare_profile) = {
        let space = omni.space("bare", &domain_id, "grant-trust-bare").await;
        let (profile, _) = omni.access_profile("bare", &space, "bare").await;
        (space, profile)
    };
    omni.create_access_grant(&create_input(&domain_id, &bare_space, "no-scoped-actions"))
        .await
        .expect("the grant with no scoped actions is created");
    let status = omni
        .assume_status("a grant naming no resource", &bare_space, &bare_profile)
        .await;
    assert_eq!(
        status, ASSUME_STATUS_DENIED,
        "case \"a grant naming no resource\": expected {ASSUME_STATUS_DENIED}, got {status}"
    );

    // The other half of the member: the principal that CREATED a profile owns it, so every
    // profile above reports ALLOWED to it — the grant decides for everyone else.
    for (case, space, profile) in [
        (
            "the owner of a profile no grant names",
            bare_space.as_str(),
            bare_profile.as_str(),
        ),
        (
            "the owner of a profile a grant does not reach",
            space_id.as_str(),
            second_profile.as_str(),
        ),
    ] {
        let status = omni.assume_status_for_the_owner(case, space, profile).await;
        assert_eq!(
            status, ASSUME_STATUS_ALLOWED,
            "case {case:?}: expected {ASSUME_STATUS_ALLOWED}, got {status}"
        );
    }
}

#[tokio::test]
async fn the_access_grant_lifecycle_over_the_rpcv2cbor_wire() {
    let omni = Omni::new();
    // The parents are created through the handlers holding the same engine the router's
    // service holds, which is what makes the wire calls below see them.
    let (domain_id, space_id) = omni.domain_and_space("wire", "grant-wire").await;

    let codec = CborCodec::default();
    let (status, body) = omni
        .over_the_wire(
            "CreateAccessGrant",
            CREATE_ACCESS_GRANT_INPUT_SCHEMA,
            &create_input(&domain_id, &space_id, "wire-grant"),
        )
        .await;
    assert_eq!(
        status,
        200,
        "CreateAccessGrant over the wire answered {status}: {:?}",
        String::from_utf8_lossy(&body)
    );
    let created = {
        let mut deserializer = codec.create_deserializer(&body);
        CreateAccessGrantOutput::deserialize(&mut deserializer)
            .expect("the output deserializes")
            .access_grant
    };
    assert!(
        is_grant_id(&created.grant_id),
        "grantId {:?} does not satisfy {GRANT_ID_PATTERN}",
        created.grant_id
    );

    // The grant the wire call created is readable over the wire too, as persisted.
    let (status, body) = omni
        .over_the_wire(
            "GetAccessGrant",
            GET_ACCESS_GRANT_INPUT_SCHEMA,
            &GetAccessGrantInput {
                grant_id: created.grant_id.clone(),
            },
        )
        .await;
    assert_eq!(
        status,
        200,
        "GetAccessGrant over the wire answered {status}: {:?}",
        String::from_utf8_lossy(&body)
    );
    let fetched = {
        let mut deserializer = codec.create_deserializer(&body);
        GetAccessGrantOutput::deserialize(&mut deserializer)
            .expect("the output deserializes")
            .access_grant
    };
    assert_eq!(
        fields(&fetched),
        fields(&created),
        "the wire read answered {fetched:?} for the created {created:?}"
    );
    assert_eq!(
        (millis(fetched.created_at), millis(fetched.updated_at)),
        (millis(created.created_at), millis(created.updated_at)),
        "the wire read and the create disagree on the timestamps"
    );

    // A miss over the wire is the modelled 404 with its modelled error code.
    let (status, body) = omni
        .over_the_wire(
            "GetAccessGrant",
            GET_ACCESS_GRANT_INPUT_SCHEMA,
            &GetAccessGrantInput {
                grant_id: MISSING_GRANT_ID.to_string(),
            },
        )
        .await;
    let rendered = String::from_utf8_lossy(&body).to_string();
    assert_eq!(
        status, 404,
        "GetAccessGrant over the wire answered {status} for a missing grant: {rendered:?}"
    );
    assert!(
        rendered.contains("ResourceNotFoundException"),
        "the wire error body does not name ResourceNotFoundException: {rendered:?}"
    );

    // A modelled constraint is refused before the handler runs, as a 400.
    let (status, body) = omni
        .over_the_wire(
            "CreateAccessGrant",
            CREATE_ACCESS_GRANT_INPUT_SCHEMA,
            &create_input(&domain_id, &space_id, "a name with spaces"),
        )
        .await;
    let rendered = String::from_utf8_lossy(&body).to_string();
    assert_eq!(
        status, 400,
        "a name outside ^[a-zA-Z0-9_-]+$ answered {status}: {rendered:?}"
    );
    assert!(
        rendered.contains("ValidationException"),
        "the wire refusal does not name ValidationException: {rendered:?}"
    );
}

// ---------------------------------------------------------------------------
// The access control a space is PROVISIONED with: the rows the service owns,
// what refuses a caller that would change them, and what releases them.
// ---------------------------------------------------------------------------

/// The operations this part of the suite drives beyond the ones above.
impl Omni {
    async fn list_access_profiles(
        &self,
        input: &ListAccessProfilesInput,
    ) -> Result<ListAccessProfilesOutput, ListAccessProfilesError> {
        ListAccessProfiles::new(Arc::clone(&self.engine))
            .handle(input.clone(), &self.ctx())
            .await
    }

    async fn list_access_grants(
        &self,
        input: &ListAccessGrantsInput,
    ) -> Result<ListAccessGrantsOutput, ListAccessGrantsError> {
        ListAccessGrants::new(Arc::clone(&self.engine))
            .handle(input.clone(), &self.ctx())
            .await
    }

    async fn get_access_profile(
        &self,
        space_id: &str,
        profile_id: &str,
    ) -> Result<GetAccessProfileOutput, GetAccessProfileError> {
        GetAccessProfile::new(Arc::clone(&self.engine))
            .handle(
                GetAccessProfileInput {
                    space_id: space_id.to_string(),
                    profile_id: profile_id.to_string(),
                },
                &self.ctx(),
            )
            .await
    }

    async fn update_access_profile(
        &self,
        input: &UpdateAccessProfileInput,
    ) -> Result<UpdateAccessProfileOutput, UpdateAccessProfileError> {
        UpdateAccessProfile::new(Arc::clone(&self.engine))
            .handle(input.clone(), &self.ctx())
            .await
    }

    async fn delete_access_profile(
        &self,
        space_id: &str,
        profile_id: &str,
    ) -> Result<DeleteAccessProfileOutput, DeleteAccessProfileError> {
        DeleteAccessProfile::new(Arc::clone(&self.engine))
            .handle(
                DeleteAccessProfileInput {
                    space_id: space_id.to_string(),
                    profile_id: profile_id.to_string(),
                },
                &self.ctx(),
            )
            .await
    }

    async fn delete_access_grant(
        &self,
        grant_id: &str,
    ) -> Result<DeleteAccessGrantOutput, DeleteAccessGrantError> {
        DeleteAccessGrant::new(Arc::clone(&self.engine))
            .handle(
                DeleteAccessGrantInput {
                    grant_id: grant_id.to_string(),
                },
                &self.ctx(),
            )
            .await
    }

    /// Delete a space, which is what releases the access control it was provisioned
    /// with.
    async fn delete_space(&self, case: &str, space_id: &str) {
        DeleteSpace::new(Arc::clone(&self.engine))
            .handle(
                DeleteSpaceInput {
                    space_id: space_id.to_string(),
                },
                &self.ctx(),
            )
            .await
            .unwrap_or_else(|error| panic!("case {case:?}: DeleteSpace failed: {error:?}"));
    }
}

/// One row of the `CreateSpace` table: the request members that vary while the access
/// control the create provisions may not.
struct SpaceRow {
    case: &'static str,
    name: String,
    tags: Option<HashMap<String, String>>,
    client_token: Option<String>,
    agent_core_evaluation_role_arn: Option<String>,
}

/// The rows span what the provisioning rule generalises over — nothing of the request
/// decides it — so they differ along the `CreateSpace` members that vary: `name` over its
/// documented 3..64 lowercase-and-hyphen class, `tags` absent, one and many, a
/// `clientToken` absent and present, and the optional AgentCore role absent and present.
fn space_rows() -> Vec<SpaceRow> {
    vec![
        SpaceRow {
            case: "the shortest name the documented class admits",
            name: "abc".to_string(),
            tags: None,
            client_token: None,
            agent_core_evaluation_role_arn: None,
        },
        SpaceRow {
            case: "a hyphenated name, one tag and an idempotency token",
            name: "omni-space-1".to_string(),
            tags: Some(HashMap::from([("team".to_string(), "sre".to_string())])),
            client_token: Some("provision-token".to_string()),
            agent_core_evaluation_role_arn: None,
        },
        SpaceRow {
            case: "the longest name the documented class admits, many tags",
            name: format!("s{}", "a".repeat(63)),
            tags: Some(
                (0..50)
                    .map(|index| (format!("key{index}"), format!("value{index}")))
                    .collect(),
            ),
            client_token: None,
            agent_core_evaluation_role_arn: None,
        },
        SpaceRow {
            case: "a space that uses AgentCore online evaluation",
            name: "agentcore-space".to_string(),
            tags: Some(HashMap::new()),
            client_token: None,
            agent_core_evaluation_role_arn: Some(
                "arn:aws:iam::000000000000:role/OmniAgentCore".to_string(),
            ),
        },
    ]
}

/// The profile `space_id` was provisioned with, discovered the way a caller discovers it:
/// the SERVICE_MANAGED record of the space's own profile listing.
async fn provisioned_profile(omni: &Omni, case: &str, space_id: &str) -> AccessProfileSummary {
    let page = omni
        .list_access_profiles(&ListAccessProfilesInput {
            space_id: space_id.to_string(),
            ..Default::default()
        })
        .await
        .unwrap_or_else(|error| panic!("case {case:?}: ListAccessProfiles failed: {error:?}"));
    let provisioned: Vec<&AccessProfileSummary> = page
        .items
        .iter()
        .filter(|item| item.profile_type.as_deref() == Some(ACCESS_PROFILE_TYPE_SERVICE_MANAGED))
        .collect();
    assert_eq!(
        provisioned.len(),
        1,
        "case {case:?}: expected exactly one provisioned profile, got {:?}",
        page.items
    );
    provisioned[0].clone()
}

/// The grants `space_id` was provisioned with, in the order the listing reports them.
async fn provisioned_grants(omni: &Omni, case: &str, space_id: &str) -> Vec<AccessGrantSummary> {
    omni.list_access_grants(&ListAccessGrantsInput {
        space_id: Some(space_id.to_string()),
        ..Default::default()
    })
    .await
    .unwrap_or_else(|error| panic!("case {case:?}: ListAccessGrants failed: {error:?}"))
    .items
    .into_iter()
    .filter(|item| item.grant_type == ACCESS_GRANT_TYPE_SERVICE_MANAGED)
    .collect()
}

/// Each grant as the members this part of the suite pins, so a row is compared as a
/// whole.
type ProvisionedGrant = (
    Option<String>,
    String,
    String,
    String,
    Option<String>,
    String,
    String,
);

fn provisioned_fields(grant: &AccessGrantSummary) -> ProvisionedGrant {
    (
        grant.name.clone(),
        grant.permission.clone(),
        grant.grant_type.clone(),
        grant.principal.principal_type.clone(),
        grant.principal.principal_id.clone(),
        grant.domain_id.clone(),
        grant.space_id.clone(),
    )
}

#[tokio::test]
async fn creating_a_space_provisions_its_service_managed_access_control() {
    let omni = Omni::new();
    // An account holds at most one domain, so every row's space is created in the SAME
    // domain: what the provisioning depends on is the space, not its parent.
    let domain_id = omni.domain("provision", "grant-provision").await;
    // No two spaces may share a provisioned record, so the ids are collected across the
    // rows and checked for repeats.
    let mut minted: Vec<String> = Vec::new();

    for (index, row) in space_rows().into_iter().enumerate() {
        let case = row.case;
        let region = omni.next_space_region();
        let parts = omni.space_parts(region);
        let ctx = OperationContext::new(
            &parts,
            omni.service.definition().identity(),
            EmbeddedEmulator::from_router(omni.router.clone()),
        );
        let space = CreateSpace::new(Arc::clone(&omni.engine))
            .handle(
                CreateSpaceInput {
                    name: row.name.clone(),
                    domain_id: domain_id.clone(),
                    data_access_role_arn: DATA_ACCESS_ROLE_ARN.to_string(),
                    tags: row.tags.clone(),
                    client_token: row.client_token.clone(),
                    agent_core_evaluation_role_arn: row.agent_core_evaluation_role_arn.clone(),
                    ..Default::default()
                },
                &ctx,
            )
            .await
            .unwrap_or_else(|error| panic!("case {case:?}: CreateSpace failed: {error:?}"))
            .space;
        let space_id = space.space_id.clone();

        // ONE access profile, the service's own, under the name the service gives it.
        let profiles = omni
            .list_access_profiles(&ListAccessProfilesInput {
                space_id: space_id.clone(),
                ..Default::default()
            })
            .await
            .unwrap_or_else(|error| panic!("case {case:?}: ListAccessProfiles failed: {error:?}"));
        let reported: Vec<(String, Option<String>)> = profiles
            .items
            .iter()
            .map(|item| (item.name.clone(), item.profile_type.clone()))
            .collect();
        assert_eq!(
            (reported, profiles.next_token.clone()),
            (
                vec![(
                    ASYNC_WORKFLOWS_PROFILE_NAME.to_string(),
                    Some(ACCESS_PROFILE_TYPE_SERVICE_MANAGED.to_string()),
                )],
                None,
            ),
            "case {case:?}: the new space's profiles are {:?}",
            profiles.items
        );

        // The profile reads back through the reader a caller resolves it with, as
        // persisted, and its ARN is derived from the space's region and its own id.
        let profile_id = profiles.items[0].profile_id.clone();
        let profile = omni
            .get_access_profile(&space_id, &profile_id)
            .await
            .unwrap_or_else(|error| panic!("case {case:?}: GetAccessProfile failed: {error:?}"))
            .access_profile;
        assert_eq!(
            (
                profile.space_id.as_str(),
                profile.name.as_str(),
                profile.profile_type.as_deref(),
            ),
            (
                space_id.as_str(),
                ASYNC_WORKFLOWS_PROFILE_NAME,
                Some(ACCESS_PROFILE_TYPE_SERVICE_MANAGED),
            ),
            "case {case:?}: the provisioned profile reads back as {profile:?}"
        );
        assert!(
            profile
                .arn
                .ends_with(&format!(":access-profile/{profile_id}"))
                && profile.arn.contains(&format!(":{region}:")),
            "case {case:?}: the provisioned profile's ARN {:?} does not name it in {region}",
            profile.arn
        );
        assert!(
            !minted.contains(&profile_id),
            "case {case:?}: the provisioned profile id {profile_id:?} repeats another space's"
        );
        minted.push(profile_id.clone());

        // TWO access grants, the service's own, in the order the service wrote them: the
        // space's administrator, then the agent principal that may assume the profile
        // above. Every value that is not fixed text is read from the space.
        let grants = provisioned_grants(&omni, case, &space_id).await;
        let actual: Vec<ProvisionedGrant> = grants.iter().map(provisioned_fields).collect();
        assert_eq!(
            actual,
            vec![
                (
                    Some(SPACE_ADMIN_GRANT_NAME.to_string()),
                    ACCESS_GRANT_PERMISSION_SPACE_ADMIN.to_string(),
                    ACCESS_GRANT_TYPE_SERVICE_MANAGED.to_string(),
                    ACCESS_GRANT_PRINCIPAL_TYPE_IAM_ROOT.to_string(),
                    Some(space.owner_account_id.clone()),
                    domain_id.clone(),
                    space_id.clone(),
                ),
                (
                    Some(AGENT_ASSUME_GRANT_NAME.to_string()),
                    "CUSTOM".to_string(),
                    ACCESS_GRANT_TYPE_SERVICE_MANAGED.to_string(),
                    ACCESS_GRANT_PRINCIPAL_TYPE_AGENT.to_string(),
                    Some(agent_principal_id(&space_id)),
                    domain_id.clone(),
                    space_id.clone(),
                ),
            ],
            "case {case:?}: the new space's grants are {grants:?}"
        );
        // The agent principal is derived from the space rather than fixed text.
        assert!(
            agent_principal_id(&space_id).ends_with(&space_id),
            "case {case:?}: the agent principal must carry the space id {space_id:?}"
        );

        for grant in &grants {
            assert!(
                !minted.contains(&grant.grant_id),
                "case {case:?}: the provisioned grant id {:?} repeats another space's",
                grant.grant_id
            );
            minted.push(grant.grant_id.clone());
            // Each grant reads back through the reader a caller resolves it with.
            let full = omni
                .get_access_grant(&grant.grant_id)
                .await
                .unwrap_or_else(|error| {
                    panic!(
                        "case {case:?}: GetAccessGrant failed for {:?}: {error:?}",
                        grant.name
                    )
                })
                .access_grant;
            assert_eq!(
                (
                    full.name.clone(),
                    full.space_id.clone(),
                    full.domain_id.clone(),
                    full.grant_type.clone(),
                    full.account_id.clone(),
                ),
                (
                    grant.name.clone(),
                    space_id.clone(),
                    domain_id.clone(),
                    ACCESS_GRANT_TYPE_SERVICE_MANAGED.to_string(),
                    space.owner_account_id.clone(),
                ),
                "case {case:?}: the provisioned grant reads back as {full:?}"
            );
            assert_eq!(
                millis(full.created_at),
                millis(full.updated_at),
                "case {case:?}: a grant nothing has updated reports createdAt {:?} and \
                 updatedAt {:?}",
                full.created_at,
                full.updated_at
            );
        }

        // The agent grant is what records the trust against the provisioned profile, so
        // a principal that does not own that profile may assume it — and only it.
        let status = omni.assume_status(case, &space_id, &profile_id).await;
        assert_eq!(
            status, ASSUME_STATUS_ALLOWED,
            "case {case:?}: expected the provisioned profile to report \
             {ASSUME_STATUS_ALLOWED}, got {status}"
        );
        let (customer_profile, _) = omni
            .access_profile(case, &space_id, &format!("Customer {index}"))
            .await;
        let status = omni.assume_status(case, &space_id, &customer_profile).await;
        assert_eq!(
            status, ASSUME_STATUS_DENIED,
            "case {case:?}: the provisioned grant must not reach another profile of the \
             space, got {status}"
        );
    }
}

#[tokio::test]
async fn the_access_control_a_space_was_provisioned_with_cannot_be_changed() {
    let omni = Omni::new();
    let (domain_id, space_id) = omni.domain_and_space("managed", "grant-managed").await;
    let profile = provisioned_profile(&omni, "managed", &space_id).await;
    let grants = provisioned_grants(&omni, "managed", &space_id).await;

    // Rows: every combination of `UpdateAccessProfile`'s own patchable members. None of
    // them is admitted on a row the service owns, so each is the modelled conflict.
    for (case, name, description) in [
        ("a description only", None, Some("changed".to_string())),
        (
            "a name only",
            Some("Renamed Profile".to_string()),
            None,
        ),
        (
            "both patchable members",
            Some("Renamed Profile".to_string()),
            Some("changed".to_string()),
        ),
        ("neither patchable member", None, None),
    ] {
        expect_modelled_error(
            case,
            omni.update_access_profile(&UpdateAccessProfileInput {
                space_id: space_id.clone(),
                profile_id: profile.profile_id.clone(),
                name,
                description,
            })
            .await,
            409,
            "ConflictException",
        );
    }
    expect_modelled_error(
        "a delete of the provisioned profile",
        omni.delete_access_profile(&space_id, &profile.profile_id)
            .await,
        409,
        "ConflictException",
    );
    for grant in &grants {
        expect_modelled_error(
            &format!("a delete of the provisioned grant {:?}", grant.name),
            omni.delete_access_grant(&grant.grant_id).await,
            409,
            "ConflictException",
        );
    }

    // Nothing moved: the profile keeps the name and the (absent) description it was
    // provisioned with, and both grants are still there.
    let after = omni
        .get_access_profile(&space_id, &profile.profile_id)
        .await
        .expect("the refused calls left the profile readable")
        .access_profile;
    assert_eq!(
        (after.name.as_str(), after.description.as_deref()),
        (ASYNC_WORKFLOWS_PROFILE_NAME, None),
        "the refused patches changed the profile to {after:?}"
    );
    assert_eq!(
        provisioned_grants(&omni, "managed", &space_id)
            .await
            .iter()
            .map(provisioned_fields)
            .collect::<Vec<ProvisionedGrant>>(),
        grants
            .iter()
            .map(provisioned_fields)
            .collect::<Vec<ProvisionedGrant>>(),
        "the refused deletes changed the space's grants"
    );

    // The message a caller branches on, read over the wire: fixed text per refusal, with
    // nothing of the request in it.
    let update_refusal = omni
        .over_the_wire(
            "UpdateAccessProfile",
            UPDATE_ACCESS_PROFILE_INPUT_SCHEMA,
            &UpdateAccessProfileInput {
                space_id: space_id.clone(),
                profile_id: profile.profile_id.clone(),
                name: None,
                description: Some("changed".to_string()),
            },
        )
        .await;
    let delete_profile_refusal = omni
        .over_the_wire(
            "DeleteAccessProfile",
            DELETE_ACCESS_PROFILE_INPUT_SCHEMA,
            &DeleteAccessProfileInput {
                space_id: space_id.clone(),
                profile_id: profile.profile_id.clone(),
            },
        )
        .await;
    let delete_grant_refusal = omni
        .over_the_wire(
            "DeleteAccessGrant",
            DELETE_ACCESS_GRANT_INPUT_SCHEMA,
            &DeleteAccessGrantInput {
                grant_id: grants[0].grant_id.clone(),
            },
        )
        .await;
    for (case, refusal, expected) in [
        (
            "UpdateAccessProfile",
            update_refusal,
            SERVICE_MANAGED_PROFILE_NOT_UPDATABLE,
        ),
        (
            "DeleteAccessProfile",
            delete_profile_refusal,
            SERVICE_MANAGED_PROFILE_NOT_DELETABLE,
        ),
        (
            "DeleteAccessGrant",
            delete_grant_refusal,
            SERVICE_MANAGED_GRANT_NOT_DELETABLE,
        ),
    ] {
        let (code, body) = refusal;
        let rendered = String::from_utf8_lossy(&body).to_string();
        assert_eq!(
            code, 409,
            "case {case:?}: expected HTTP 409 over the wire, got {code}: {rendered:?}"
        );
        assert!(
            rendered.contains("ConflictException") && rendered.contains(expected),
            "case {case:?}: expected the refusal to state {expected:?}, got {rendered:?}"
        );
        assert!(
            !rendered.contains(&space_id),
            "case {case:?}: the refusal must not name the space, got {rendered:?}"
        );
    }

    // Who manages the row is what decides: a customer-created profile of the SAME space
    // is patched and deleted, and so is a customer-created grant.
    let (customer_profile, _) = omni
        .access_profile("customer", &space_id, "Customer Managed")
        .await;
    let patched = omni
        .update_access_profile(&UpdateAccessProfileInput {
            space_id: space_id.clone(),
            profile_id: customer_profile.clone(),
            name: None,
            description: Some("changed".to_string()),
        })
        .await
        .expect("a customer-managed profile is patched")
        .access_profile;
    assert_eq!(
        (
            patched.description.as_deref(),
            patched.profile_type.as_deref(),
        ),
        (Some("changed"), Some(ACCESS_PROFILE_TYPE_CUSTOMER_MANAGED)),
        "a customer-managed profile must take the patch, got {patched:?}"
    );
    omni.delete_access_profile(&space_id, &customer_profile)
        .await
        .expect("a customer-managed profile is deleted");
    let customer_grant = omni
        .create_access_grant(&create_input(&domain_id, &space_id, "customer-grant"))
        .await
        .expect("a customer-managed grant is created")
        .access_grant;
    assert_eq!(
        customer_grant.grant_type, ACCESS_GRANT_TYPE_CUSTOMER_MANAGED,
        "a grant created through the API is {:?}",
        customer_grant.grant_type
    );
    omni.delete_access_grant(&customer_grant.grant_id)
        .await
        .expect("a customer-managed grant is deleted");
}

#[tokio::test]
async fn deleting_a_space_releases_the_access_control_it_was_provisioned_with() {
    let omni = Omni::new();
    let domain_id = omni.domain("release", "grant-release").await;
    let removed_space = omni.space("release", &domain_id, "grant-release-one").await;
    let kept_space = omni.space("release", &domain_id, "grant-release-two").await;
    let removed = provisioned_grants(&omni, "release", &removed_space).await;
    let removed_profile = provisioned_profile(&omni, "release", &removed_space).await;
    let kept = provisioned_grants(&omni, "release", &kept_space).await;
    // A customer-created grant of the space that survives: the release is about the
    // space that went, not about the store.
    let kept_customer = omni
        .create_access_grant(&create_input(&domain_id, &kept_space, "kept-grant"))
        .await
        .expect("the surviving space's grant is created")
        .access_grant;

    omni.delete_space("release", &removed_space).await;

    // The rows the deleted space was provisioned with are gone from every reader.
    for grant in &removed {
        expect_modelled_error(
            &format!("a read of the released grant {:?}", grant.name),
            omni.get_access_grant(&grant.grant_id).await,
            404,
            "ResourceNotFoundException",
        );
    }
    // The released profile is addressed in a space the account no longer holds, so the
    // read is refused rather than answered: the space is the unit of access, and a space
    // the account does not hold authorizes nobody.
    match omni
        .get_access_profile(&removed_space, &removed_profile.profile_id)
        .await
    {
        Ok(output) => panic!(
            "case \"a read of the released profile\": expected AccessDeniedException (403), \
             got Ok({output:?})"
        ),
        Err(error) => {
            assert_eq!(
                (error.shape_name(), error.http_status()),
                ("AccessDeniedException", 403),
                "case \"a read of the released profile\": expected AccessDeniedException (403), \
                 got {error:?}"
            );
        }
    }
    let listed: Vec<String> = omni
        .list_access_grants(&ListAccessGrantsInput::default())
        .await
        .expect("the account's grants list")
        .items
        .into_iter()
        .map(|item| item.grant_id)
        .collect();
    for grant in &removed {
        assert!(
            !listed.contains(&grant.grant_id),
            "the released grant {:?} is still listed in {listed:?}",
            grant.name
        );
    }
    // What the surviving space holds is untouched.
    let mut expected: Vec<String> = kept.iter().map(|item| item.grant_id.clone()).collect();
    expected.push(kept_customer.grant_id.clone());
    assert_eq!(
        listed, expected,
        "the surviving space's grants are {listed:?}"
    );
}

#[tokio::test]
async fn the_grant_listing_reports_the_grants_matching_every_filter() {
    let omni = Omni::new();
    let (domain_id, space_id) = omni.domain_and_space("filters", "grant-filters").await;
    // A second space of the SAME domain: an account holds at most one domain, so what a
    // filter has to leave out lives beside this space rather than under another domain.
    let other_space = omni
        .space("filters", &domain_id, "grant-filters-other")
        .await;
    // One grant per space beyond the provisioned ones, so every filter below has
    // something it must leave out as well as something it must report.
    let here = omni
        .create_access_grant(&CreateAccessGrantInput {
            permission: "READ".to_string(),
            principal: principal("IAM_ROLE", "arn:aws:iam::000000000000:role/Reader"),
            ..create_input(&domain_id, &space_id, "reader")
        })
        .await
        .expect("this space's grant is created")
        .access_grant;
    let elsewhere = omni
        .create_access_grant(&CreateAccessGrantInput {
            permission: "READ_WRITE_DELETE".to_string(),
            ..create_input(&domain_id, &other_space, "elsewhere")
        })
        .await
        .expect("the other space's grant is created")
        .access_grant;
    let provisioned = provisioned_grants(&omni, "filters", &space_id).await;
    let other_provisioned = provisioned_grants(&omni, "filters", &other_space).await;
    let admin_grant = provisioned[0].grant_id.clone();
    let agent_grant = provisioned[1].grant_id.clone();
    let other_admin = other_provisioned[0].grant_id.clone();
    let other_agent = other_provisioned[1].grant_id.clone();

    // Rows: no filter, then each filter the model declares on its own, then two
    // together, then a conjunction nothing satisfies.
    for (case, input, expected) in [
        (
            "no filter at all",
            ListAccessGrantsInput::default(),
            vec![
                admin_grant.clone(),
                agent_grant.clone(),
                other_admin.clone(),
                other_agent.clone(),
                here.grant_id.clone(),
                elsewhere.grant_id.clone(),
            ],
        ),
        (
            "by space",
            ListAccessGrantsInput {
                space_id: Some(space_id.clone()),
                ..Default::default()
            },
            vec![
                admin_grant.clone(),
                agent_grant.clone(),
                here.grant_id.clone(),
            ],
        ),
        (
            "by the one domain the account holds",
            ListAccessGrantsInput {
                domain_id: Some(domain_id.clone()),
                ..Default::default()
            },
            vec![
                admin_grant.clone(),
                agent_grant.clone(),
                other_admin.clone(),
                other_agent.clone(),
                here.grant_id.clone(),
                elsewhere.grant_id.clone(),
            ],
        ),
        (
            "by permission, over the grants of two spaces",
            ListAccessGrantsInput {
                permission: Some("READ_WRITE_DELETE".to_string()),
                ..Default::default()
            },
            vec![elsewhere.grant_id.clone()],
        ),
        (
            "by principal type",
            ListAccessGrantsInput {
                principal_type: Some(ACCESS_GRANT_PRINCIPAL_TYPE_AGENT.to_string()),
                ..Default::default()
            },
            vec![agent_grant.clone(), other_agent.clone()],
        ),
        (
            "by principal id",
            ListAccessGrantsInput {
                principal_id: Some(agent_principal_id(&space_id)),
                ..Default::default()
            },
            vec![agent_grant.clone()],
        ),
        (
            "by permission",
            ListAccessGrantsInput {
                permission: Some(ACCESS_GRANT_PERMISSION_SPACE_ADMIN.to_string()),
                ..Default::default()
            },
            vec![admin_grant.clone(), other_admin.clone()],
        ),
        (
            "by space and permission together",
            ListAccessGrantsInput {
                space_id: Some(space_id.clone()),
                permission: Some("READ".to_string()),
                ..Default::default()
            },
            vec![here.grant_id.clone()],
        ),
        (
            "a conjunction nothing satisfies",
            ListAccessGrantsInput {
                space_id: Some(space_id.clone()),
                principal_id: Some(agent_principal_id(&other_space)),
                ..Default::default()
            },
            Vec::new(),
        ),
    ] {
        let page = omni
            .list_access_grants(&input)
            .await
            .unwrap_or_else(|error| panic!("case {case:?}: the listing failed: {error:?}"));
        let actual: Vec<String> = page.items.iter().map(|item| item.grant_id.clone()).collect();
        assert_eq!(
            (actual.clone(), page.next_token.clone()),
            (expected.clone(), None),
            "case {case:?}: expected {expected:?}, got {actual:?}"
        );
    }

    // Every page size walks the same order exactly once: rows are a page smaller than
    // the collection, one short of it, exactly it, one past it and the modelled maximum.
    let everything: Vec<String> = omni
        .list_access_grants(&ListAccessGrantsInput::default())
        .await
        .expect("the account's grants list")
        .items
        .into_iter()
        .map(|item| item.grant_id)
        .collect();
    for page_size in [1, 2, 3, 4, 5, 100] {
        let mut walked: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        loop {
            let page = omni
                .list_access_grants(&ListAccessGrantsInput {
                    max_results: Some(page_size),
                    next_token: token.clone(),
                    ..Default::default()
                })
                .await
                .unwrap_or_else(|error| panic!("case {page_size}: the page failed: {error:?}"));
            assert!(
                page.items.len() <= page_size as usize,
                "case {page_size}: a page carried {} items",
                page.items.len()
            );
            walked.extend(page.items.iter().map(|item| item.grant_id.clone()));
            token = page.next_token;
            if token.is_none() {
                break;
            }
            assert!(
                walked.len() <= everything.len(),
                "case {page_size}: paging ran past the collection with {walked:?}"
            );
        }
        assert_eq!(
            walked, everything,
            "case {page_size}: walking the pages gave {walked:?}"
        );
    }

    // A read: the same request twice answers the same page.
    let request = ListAccessGrantsInput {
        space_id: Some(space_id.clone()),
        ..Default::default()
    };
    let first = omni
        .list_access_grants(&request)
        .await
        .expect("the first listing succeeds");
    let again = omni
        .list_access_grants(&request)
        .await
        .expect("the second listing succeeds");
    assert_eq!(
        first
            .items
            .iter()
            .map(provisioned_fields)
            .collect::<Vec<ProvisionedGrant>>(),
        again
            .items
            .iter()
            .map(provisioned_fields)
            .collect::<Vec<ProvisionedGrant>>(),
        "listing twice answered different pages"
    );

    // A filter is not an existence precondition: a value the store holds no record for
    // matches no grant, so the listing answers an empty page rather than a miss.
    for (case, input) in [
        (
            "a domain the store does not hold",
            ListAccessGrantsInput {
                domain_id: Some(MISSING_DOMAIN_ID.to_string()),
                ..Default::default()
            },
        ),
        (
            "a space the store does not hold",
            ListAccessGrantsInput {
                space_id: Some(MISSING_SPACE_ID.to_string()),
                ..Default::default()
            },
        ),
    ] {
        let page = omni
            .list_access_grants(&input)
            .await
            .unwrap_or_else(|error| panic!("case {case:?}: the listing failed: {error:?}"));
        assert_eq!(
            (page.items.len(), page.next_token.clone()),
            (0, None),
            "case {case:?}: expected an empty page, got {:?}",
            page.items
        );
    }

    // A `nextToken` this service did not mint for THIS collection cannot name a resume
    // point: rows span text that is not a token, hex that decodes to nothing this
    // service writes, and a token minted for another collection's listing.
    let foreign_collection = hex::encode("access-profiles:1");
    for token in [
        "not-a-token",
        "6e6f7065",
        foreign_collection.as_str(),
        &"f".repeat(2048),
    ] {
        expect_modelled_error(
            token,
            omni.list_access_grants(&ListAccessGrantsInput {
                next_token: Some(token.to_string()),
                ..Default::default()
            })
            .await,
            400,
            "ValidationException",
        );
    }

    // An enumerated filter outside the values the model declares is the modelled 400,
    // not an empty page.
    for (case, input) in [
        (
            "a principal type the model does not declare",
            ListAccessGrantsInput {
                principal_type: Some("ROBOT".to_string()),
                ..Default::default()
            },
        ),
        (
            "a permission the model does not declare",
            ListAccessGrantsInput {
                permission: Some("SUPER_ADMIN".to_string()),
                ..Default::default()
            },
        ),
    ] {
        expect_modelled_error(case, omni.list_access_grants(&input).await, 400, "ValidationException");
    }

    // The listing answers over the rpcv2Cbor wire from the same store.
    let codec = CborCodec::default();
    let (status, body) = omni
        .over_the_wire(
            "ListAccessGrants",
            LIST_ACCESS_GRANTS_INPUT_SCHEMA,
            &ListAccessGrantsInput {
                space_id: Some(space_id.clone()),
                max_results: Some(2),
                ..Default::default()
            },
        )
        .await;
    assert_eq!(
        status,
        200,
        "ListAccessGrants over the wire answered {status}: {:?}",
        String::from_utf8_lossy(&body)
    );
    let page = {
        let mut deserializer = codec.create_deserializer(&body);
        ListAccessGrantsOutput::deserialize(&mut deserializer).expect("the output deserializes")
    };
    assert_eq!(
        page.items
            .iter()
            .map(|item| item.grant_id.clone())
            .collect::<Vec<String>>(),
        vec![admin_grant.clone(), agent_grant.clone()],
        "the wire page is {:?}",
        page.items
    );
    let token = page
        .next_token
        .clone()
        .expect("grants remain beyond the first wire page");
    let (status, body) = omni
        .over_the_wire(
            "ListAccessGrants",
            LIST_ACCESS_GRANTS_INPUT_SCHEMA,
            &ListAccessGrantsInput {
                space_id: Some(space_id.clone()),
                next_token: Some(token),
                ..Default::default()
            },
        )
        .await;
    assert_eq!(
        status,
        200,
        "the resumed wire page answered {status}: {:?}",
        String::from_utf8_lossy(&body)
    );
    let rest = {
        let mut deserializer = codec.create_deserializer(&body);
        ListAccessGrantsOutput::deserialize(&mut deserializer).expect("the output deserializes")
    };
    assert_eq!(
        (
            rest.items
                .iter()
                .map(|item| item.grant_id.clone())
                .collect::<Vec<String>>(),
            rest.next_token.clone(),
        ),
        (vec![here.grant_id.clone()], None),
        "the resumed wire page is {:?}",
        rest.items
    );
}
