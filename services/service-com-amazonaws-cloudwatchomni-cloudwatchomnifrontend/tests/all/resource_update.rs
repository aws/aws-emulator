// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for the PARTIAL-update rule of CloudWatchOmni's three updatable leaf
//! resources: `UpdateAccessProfile`, `UpdateOmniDashboard` and `UpdateView`.
//!
//! Each of the three is documented as changing "only the provided fields", so this suite
//! asserts that rule over a TABLE rather than one recorded exchange. Each row starts from
//! a resource in a different shape, patches a different subset of the updatable members,
//! and requires
//!
//! * every member the request carried to hold its new value;
//! * every member the request omitted — including the ones the input shape does not even
//!   carry (ids, ARNs, the parent, the ownership category, `createdBy`, tags,
//!   `createdAt`) — to hold exactly what it held before;
//! * the patched record, and not a second copy of it, to be what the resource's own
//!   reader answers afterwards: the write has to reach the store the read comes from;
//! * `updatedAt` to move when a member moves and to stay put when the patch asks for the
//!   values the record already holds.
//!
//! The refusals are asserted the same way: a record the store does not hold is the
//! modelled 404, a rename onto a sibling's name is the modelled 409 where that kind's
//! name is its identity (an access profile) and no refusal at all where the identity is a
//! minted id (a dashboard), a definition that
//! violates its own rule is the modelled 400 — and none of them writes anything. The
//! MESSAGE each refusal carries is a requirement too, so the suite also asserts it over a
//! table, for EVERY resource kind this service reports a miss for: one text per kind,
//! stated identically by every operation that reports a miss of that kind, carrying only
//! the variable parts that kind's text has — a dashboard's id, a view's name, an alert's
//! id with the space it was looked for in, and nothing at all for a domain, a space, an
//! access profile or an access grant. A dashboard body is held to a schema CLOSED at every
//! object it declares, one clause per violated constraint.
//!
//! Each assertion names its case and the actual value, so a failure identifies the row of
//! the table that broke.

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
use omni::ops::create_access_profile::{
    CreateAccessProfile, CreateAccessProfileError, CreateAccessProfileErrorModeled,
};
use omni::ops::create_domain::CreateDomain;
use omni::ops::create_omni_dashboard::CreateOmniDashboard;
use omni::ops::create_space::{CreateSpace, CreateSpaceError, CreateSpaceErrorModeled};
use omni::ops::create_view::CreateView;
use omni::ops::delete_view::{DeleteView, DeleteViewError, DeleteViewErrorModeled};
use omni::ops::get_access_grant::{
    GetAccessGrant, GetAccessGrantError, GetAccessGrantErrorModeled,
};
use omni::ops::get_access_profile::{
    GetAccessProfile, GetAccessProfileError, GetAccessProfileErrorModeled,
};
use omni::ops::get_alert::{GetAlert, GetAlertError, GetAlertErrorModeled};
use omni::ops::get_domain::{GetDomain, GetDomainError, GetDomainErrorModeled};
use omni::ops::get_omni_dashboard::{
    GetOmniDashboard, GetOmniDashboardError, GetOmniDashboardErrorModeled,
};
use omni::ops::get_space::{GetSpace, GetSpaceError, GetSpaceErrorModeled};
use omni::ops::get_view::{GetView, GetViewError, GetViewErrorModeled};
use omni::ops::list_access_grants::{ListAccessGrants, ListAccessGrantsError};
use omni::ops::list_omni_dashboards::{
    ListOmniDashboards, ListOmniDashboardsError, ListOmniDashboardsErrorModeled,
};
use omni::ops::list_spaces::{ListSpaces, ListSpacesError};
use omni::ops::types::{
    AccessGrant, AccessGrantPrincipal, AccessProfile, CreateAccessGrantInput,
    CreateAccessProfileInput, CreateDomainInput, CreateOmniDashboardInput, CreateSpaceInput,
    CreateViewInput, DeleteViewInput, DeleteViewOutput, GetAccessGrantInput, GetAccessGrantOutput,
    GetAccessProfileInput, GetAccessProfileOutput, GetAlertInput, GetAlertOutput, GetDomainInput,
    GetDomainOutput, GetOmniDashboardInput, GetSpaceInput, GetSpaceOutput, GetViewInput,
    GetViewOutput, ListAccessGrantsInput, ListAccessGrantsOutput, ListOmniDashboardsInput,
    ListOmniDashboardsOutput, ListSpacesInput, ListSpacesOutput, OmniDashboard, Space,
    UpdateAccessProfileInput, UpdateAccessProfileOutput, UpdateDomainInput, UpdateDomainOutput,
    UpdateOmniDashboardInput, UpdateSpaceInput, UpdateSpaceOutput, UpdateViewInput,
    UpdateViewOutput, GET_ACCESS_PROFILE_INPUT_SCHEMA, UPDATE_ACCESS_PROFILE_INPUT_SCHEMA,
};
use omni::ops::update_access_profile::{
    UpdateAccessProfile, UpdateAccessProfileError, UpdateAccessProfileErrorModeled,
};
use omni::ops::update_domain::{UpdateDomain, UpdateDomainError, UpdateDomainErrorModeled};
use omni::ops::update_omni_dashboard::{
    UpdateOmniDashboard, UpdateOmniDashboardError, UpdateOmniDashboardErrorModeled,
};
use omni::ops::update_space::{UpdateSpace, UpdateSpaceError, UpdateSpaceErrorModeled};
use omni::ops::update_view::{UpdateView, UpdateViewError, UpdateViewErrorModeled};
use omni::scaffolded::Scaffolded;
use omni::state::{
    ACCESS_GRANT_NOT_FOUND_MESSAGE, ACCESS_PROFILE_NAME_TAKEN, ACCESS_PROFILE_NOT_FOUND_MESSAGE,
    ASSUME_STATUS_ALLOWED, DOMAIN_NOT_FOUND_MESSAGE, SPACE_NOT_FOUND_MESSAGE, VIEW_TYPE_USER,
};

/// The region the suite signs for.
const REGION: &str = "us-west-2";

/// A role ARN satisfying `Arn`'s `^arn:.+$`, which `CreateSpace` requires.
const DATA_ACCESS_ROLE_ARN: &str = "arn:aws:iam::000000000000:role/OmniData";

/// A well-formed `SpaceId` (its modelled pattern) that names nothing.
const MISSING_SPACE_ID: &str = "00000000-0000-4000-8000-000000000000";

/// A well-formed `ProfileId` that names nothing.
const MISSING_PROFILE_ID: &str = "no-such-profile";

/// A well-formed `DashboardId` (its modelled pattern) that names nothing.
const MISSING_DASHBOARD_ID: &str = "11111111-2222-4333-8444-555555555555";

/// A well-formed `ViewName` (its modelled pattern) that names nothing.
const MISSING_VIEW_NAME: &str = "view.no-such-view";

/// A dashboard body that satisfies the dashboard-body rule: a closed JSON object whose
/// `panels` member is a list of panel objects, each carrying the `type` and `layout` the
/// rule requires of a panel. This one lays out no panel yet.
const PANELS_BODY: &str = r#"{"panels":[]}"#;

/// A conforming body holding ONE panel, so a row can patch the body to a document that
/// is not empty.
const ONE_PANEL_BODY: &str = r#"{"panels":[{"type":"logs","layout":{"row":0}}]}"#;

/// A timestamp at millisecond resolution — the resolution rpcv2Cbor encodes a `Timestamp`
/// at, so a value compared across a wire round trip is compared there.
fn millis(at: aws_smithy_types::DateTime) -> (i64, u32) {
    (at.secs(), at.subsec_nanos() / 1_000_000)
}

/// A SigV4 `Authorization` header whose credential scope names this service's signing
/// name (the router's routing key) and the region the handlers record.
fn signed_request_parts() -> http::request::Parts {
    signed_request_parts_in(REGION)
}

/// The same, signed for `region` rather than for the suite's own.
///
/// An account holds at most ONE space per Region, so a case needing a second space signs
/// that create for another Region ([`SPACE_REGIONS`]).
fn signed_request_parts_in(region: &str) -> http::request::Parts {
    let (parts, ()) = http::Request::builder()
        .method("POST")
        .uri("/service/CloudWatchOmniFrontend/operation/Unused")
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDEMULATOR/20260101/{region}/cloudwatch/aws4_request, \
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
    /// How many spaces this emulator has created, which picks each create's Region out
    /// of [`SPACE_REGIONS`].
    spaces_created: std::sync::atomic::AtomicUsize,
    /// The account's one domain, remembered after the first space is created in it.
    domain: std::sync::Mutex<Option<String>>,
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
            parts: signed_request_parts(),
            spaces_created: std::sync::atomic::AtomicUsize::new(0),
            domain: std::sync::Mutex::new(None),
        }
    }

    fn ctx(&self) -> OperationContext<'_> {
        OperationContext::new(
            &self.parts,
            self.service.definition().identity(),
            EmbeddedEmulator::from_router(self.router.clone()),
        )
    }

    /// The account's domain, created on first use.
    ///
    /// An account holds at most ONE domain, so every space this suite creates hangs off
    /// the same one; the create carries the `clientToken` it is admitted only with.
    async fn domain(&self, case: &str) -> String {
        if let Some(held) = self
            .domain
            .lock()
            .expect("the domain cell is not poisoned")
            .clone()
        {
            return held;
        }
        let created = CreateDomain::new(Arc::clone(&self.engine))
            .handle(
                CreateDomainInput {
                    name: "omni-resource-update".to_string(),
                    identity_providers: vec!["IAM".to_string()],
                    client_token: Some("token-resource-update".to_string()),
                    ..Default::default()
                },
                &self.ctx(),
            )
            .await
            .unwrap_or_else(|error| panic!("case {case:?}: CreateDomain failed: {error:?}"))
            .domain
            .domain_id;
        *self.domain.lock().expect("the domain cell is not poisoned") = Some(created.clone());
        created
    }

    /// A space to hang the suite's profiles and dashboards off, through the handlers that
    /// own those resources.
    ///
    /// An account holds at most ONE space per Region, so each space is created in a
    /// Region of its own ([`SPACE_REGIONS`]); the calls that follow address the space by
    /// its id, which no Region scopes.
    async fn space(&self, case: &str, name: &str) -> String {
        let domain_id = self.domain(case).await;
        let index = self
            .spaces_created
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let region = SPACE_REGIONS
            .get(index)
            .copied()
            .expect("the suite signs each space it creates for a Region of its own");
        let parts = signed_request_parts_in(region);
        let ctx = OperationContext::new(
            &parts,
            self.service.definition().identity(),
            EmbeddedEmulator::from_router(self.router.clone()),
        );
        CreateSpace::new(Arc::clone(&self.engine))
            .handle(
                CreateSpaceInput {
                    name: name.to_string(),
                    domain_id,
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

    async fn create_profile(
        &self,
        case: &str,
        space_id: &str,
        name: &str,
        description: Option<&str>,
    ) -> AccessProfile {
        CreateAccessProfile::new(Arc::clone(&self.engine))
            .handle(
                CreateAccessProfileInput {
                    space_id: space_id.to_string(),
                    name: name.to_string(),
                    description: description.map(str::to_string),
                    ..Default::default()
                },
                &self.ctx(),
            )
            .await
            .unwrap_or_else(|error| panic!("case {case:?}: CreateAccessProfile failed: {error:?}"))
            .access_profile
    }

    async fn update_profile(
        &self,
        input: &UpdateAccessProfileInput,
    ) -> Result<UpdateAccessProfileOutput, UpdateAccessProfileError> {
        UpdateAccessProfile::new(Arc::clone(&self.engine))
            .handle(input.clone(), &self.ctx())
            .await
    }

    async fn get_profile(
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

    /// The stored profile, which every row reads back to check the write landed.
    async fn stored_profile(&self, case: &str, space_id: &str, profile_id: &str) -> AccessProfile {
        self.get_profile(space_id, profile_id)
            .await
            .unwrap_or_else(|error| panic!("case {case:?}: GetAccessProfile failed: {error:?}"))
            .access_profile
    }

    async fn create_dashboard(
        &self,
        case: &str,
        space_id: &str,
        name: &str,
        body: &str,
        description: Option<&str>,
    ) -> OmniDashboard {
        CreateOmniDashboard::new(Arc::clone(&self.engine))
            .handle(
                CreateOmniDashboardInput {
                    space_id: space_id.to_string(),
                    name: name.to_string(),
                    body: body.to_string(),
                    description: description.map(str::to_string),
                    ..Default::default()
                },
                &self.ctx(),
            )
            .await
            .unwrap_or_else(|error| panic!("case {case:?}: CreateOmniDashboard failed: {error:?}"))
            .omni_dashboard
    }

    async fn update_dashboard(
        &self,
        input: &UpdateOmniDashboardInput,
    ) -> Result<OmniDashboard, UpdateOmniDashboardError> {
        UpdateOmniDashboard::new(Arc::clone(&self.engine))
            .handle(input.clone(), &self.ctx())
            .await
            .map(|output| output.omni_dashboard)
    }

    async fn get_dashboard(
        &self,
        space_id: &str,
        dashboard_id: &str,
    ) -> Result<OmniDashboard, GetOmniDashboardError> {
        GetOmniDashboard::new(Arc::clone(&self.engine))
            .handle(
                GetOmniDashboardInput {
                    space_id: space_id.to_string(),
                    dashboard_id: dashboard_id.to_string(),
                },
                &self.ctx(),
            )
            .await
            .map(|output| output.omni_dashboard)
    }

    /// The stored dashboard, which every row reads back to check the write landed.
    async fn stored_dashboard(
        &self,
        case: &str,
        space_id: &str,
        dashboard_id: &str,
    ) -> OmniDashboard {
        self.get_dashboard(space_id, dashboard_id)
            .await
            .unwrap_or_else(|error| panic!("case {case:?}: GetOmniDashboard failed: {error:?}"))
    }

    async fn create_view(
        &self,
        case: &str,
        name: &str,
        definition: &str,
        description: Option<&str>,
    ) -> UpdateViewOutput {
        let created = CreateView::new(Arc::clone(&self.engine))
            .handle(
                CreateViewInput {
                    name: name.to_string(),
                    definition: definition.to_string(),
                    description: description.map(str::to_string),
                    ..Default::default()
                },
                &self.ctx(),
            )
            .await
            .unwrap_or_else(|error| panic!("case {case:?}: CreateView failed: {error:?}"));
        // `CreateView`, `UpdateView` and `GetView` answer with the same flat member set,
        // so the suite compares them as one shape.
        UpdateViewOutput {
            name: created.name,
            r#type: created.r#type,
            description: created.description,
            definition: created.definition,
            created_at: created.created_at,
            updated_at: created.updated_at,
            arn: created.arn,
        }
    }

    async fn update_view(
        &self,
        input: &UpdateViewInput,
    ) -> Result<UpdateViewOutput, UpdateViewError> {
        UpdateView::new(Arc::clone(&self.engine))
            .handle(input.clone(), &self.ctx())
            .await
    }

    async fn get_view(&self, name: &str) -> Result<GetViewOutput, GetViewError> {
        GetView::new(Arc::clone(&self.engine))
            .handle(
                GetViewInput {
                    name: name.to_string(),
                },
                &self.ctx(),
            )
            .await
    }

    async fn delete_view(&self, name: &str) -> Result<DeleteViewOutput, DeleteViewError> {
        DeleteView::new(Arc::clone(&self.engine))
            .handle(
                DeleteViewInput {
                    name: name.to_string(),
                },
                &self.ctx(),
            )
            .await
    }

    /// The stored view, as the same flat member set the update answers with.
    async fn stored_view(&self, case: &str, name: &str) -> UpdateViewOutput {
        let fetched = self
            .get_view(name)
            .await
            .unwrap_or_else(|error| panic!("case {case:?}: GetView failed: {error:?}"));
        UpdateViewOutput {
            name: fetched.name,
            r#type: fetched.r#type,
            description: fetched.description,
            definition: fetched.definition,
            created_at: fetched.created_at,
            updated_at: fetched.updated_at,
            arn: fetched.arn,
        }
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

/// The members of a profile that `UpdateAccessProfile`'s input cannot name, rendered for
/// comparison: a patch must bring every one of them through untouched.
fn untouched_profile(profile: &AccessProfile) -> String {
    format!(
        "{:?}",
        (
            &profile.profile_id,
            &profile.space_id,
            &profile.arn,
            &profile.profile_type,
            millis(profile.created_at),
        )
    )
}

/// The members of a dashboard that `UpdateOmniDashboard`'s input cannot name.
fn untouched_dashboard(dashboard: &OmniDashboard) -> String {
    format!(
        "{:?}",
        (
            &dashboard.dashboard_id,
            &dashboard.arn,
            &dashboard.created_by,
            dashboard
                .tags
                .as_ref()
                .map(|tags| {
                    let mut pairs: Vec<(&String, &String)> = tags.iter().collect();
                    pairs.sort();
                    pairs
                })
                .unwrap_or_default(),
            millis(dashboard.created_at),
        )
    )
}

/// The members of a view that `UpdateView`'s input cannot name — its name included, since
/// a view's name is its identity and the input names it only to address it.
fn untouched_view(view: &UpdateViewOutput) -> String {
    format!(
        "{:?}",
        (
            &view.name,
            &view.r#type,
            &view.arn,
            millis(view.created_at),
        )
    )
}

/// One row of the access-profile patch table.
struct ProfilePatch {
    case: &'static str,
    initial_name: String,
    initial_description: Option<String>,
    patch_name: Option<String>,
    patch_description: Option<String>,
    expected_name: String,
    expected_description: Option<String>,
}

/// The rows differ along WHICH members the patch carries (each alone, both, neither, and
/// the values the record already holds) and along the shape of the profile they start
/// from. `name`'s sizes are the published bounds of `AccessProfileName`'s
/// `@length(1..256)` and its character class is the whole of its
/// `^[a-zA-Z0-9 _.:/=+@-]+$`; the longest description is the bound of
/// `UpdateAccessProfileInput.description`'s `@length(max: 1024)`
/// (`smithy_query(query="shape",shape="UpdateAccessProfileInput")`).
fn profile_patches() -> Vec<ProfilePatch> {
    vec![
        ProfilePatch {
            case: "the description alone is replaced",
            initial_name: "profile one".to_string(),
            initial_description: Some("first".to_string()),
            patch_name: None,
            patch_description: Some("second".to_string()),
            expected_name: "profile one".to_string(),
            expected_description: Some("second".to_string()),
        },
        ProfilePatch {
            case: "the description alone is added to a profile created without one",
            initial_name: "profile two".to_string(),
            initial_description: None,
            patch_name: None,
            patch_description: Some("a description".to_string()),
            expected_name: "profile two".to_string(),
            expected_description: Some("a description".to_string()),
        },
        ProfilePatch {
            case: "the name alone keeps the description",
            initial_name: "profile three".to_string(),
            initial_description: Some("kept".to_string()),
            patch_name: Some("profile three renamed".to_string()),
            patch_description: None,
            expected_name: "profile three renamed".to_string(),
            expected_description: Some("kept".to_string()),
        },
        ProfilePatch {
            case: "both members at once, name at @length(min: 1)",
            initial_name: "profile four".to_string(),
            initial_description: None,
            patch_name: Some("a".to_string()),
            patch_description: Some("d".repeat(1024)),
            expected_name: "a".to_string(),
            expected_description: Some("d".repeat(1024)),
        },
        ProfilePatch {
            case: "a name at @length(max: 256)",
            initial_name: "profile five".to_string(),
            initial_description: Some("kept".to_string()),
            patch_name: Some("n".repeat(256)),
            patch_description: None,
            expected_name: "n".repeat(256),
            expected_description: Some("kept".to_string()),
        },
        ProfilePatch {
            case: "a name drawn from the whole modelled character class",
            initial_name: "profile six".to_string(),
            initial_description: None,
            patch_name: Some("ops team_a.b:c/d=e+f@g-h 9".to_string()),
            patch_description: Some(String::new()),
            expected_name: "ops team_a.b:c/d=e+f@g-h 9".to_string(),
            expected_description: Some(String::new()),
        },
        ProfilePatch {
            case: "a request carrying no updatable member leaves the profile whole",
            initial_name: "profile seven".to_string(),
            initial_description: Some("kept".to_string()),
            patch_name: None,
            patch_description: None,
            expected_name: "profile seven".to_string(),
            expected_description: Some("kept".to_string()),
        },
        ProfilePatch {
            case: "a patch asking for the values the profile already holds",
            initial_name: "profile eight".to_string(),
            initial_description: Some("kept".to_string()),
            patch_name: Some("profile eight".to_string()),
            patch_description: Some("kept".to_string()),
            expected_name: "profile eight".to_string(),
            expected_description: Some("kept".to_string()),
        },
    ]
}

#[tokio::test]
async fn update_access_profile_persists_only_the_members_the_request_carries() {
    let omni = Omni::new();
    let space_id = omni.space("profiles", "update-profiles").await;

    for row in profile_patches() {
        let case = row.case;
        let created = omni
            .create_profile(
                case,
                &space_id,
                &row.initial_name,
                row.initial_description.as_deref(),
            )
            .await;

        let updated = omni
            .update_profile(&UpdateAccessProfileInput {
                space_id: space_id.clone(),
                profile_id: created.profile_id.clone(),
                name: row.patch_name.clone(),
                description: row.patch_description.clone(),
            })
            .await
            .unwrap_or_else(|error| panic!("case {case:?}: UpdateAccessProfile failed: {error:?}"))
            .access_profile;

        assert_eq!(
            (updated.name.as_str(), updated.description.as_deref()),
            (
                row.expected_name.as_str(),
                row.expected_description.as_deref()
            ),
            "case {case:?}: expected name {:?} and description {:?}, got {updated:?}",
            row.expected_name,
            row.expected_description
        );
        assert_eq!(
            untouched_profile(&updated),
            untouched_profile(&created),
            "case {case:?}: a member UpdateAccessProfile's input cannot name was changed"
        );
        // The caller that created the profile owns it, so the patched record still reports
        // that caller may assume it.
        assert_eq!(
            updated.assume_status.as_deref(),
            Some(ASSUME_STATUS_ALLOWED),
            "case {case:?}: expected assumeStatus {ASSUME_STATUS_ALLOWED} for the owner, got {:?}",
            updated.assume_status
        );

        // The write has to reach the store the reader answers from, and it must be the
        // SAME record rather than a second copy of it.
        let fetched = omni
            .stored_profile(case, &space_id, &created.profile_id)
            .await;
        assert_eq!(
            format!("{fetched:?}"),
            format!("{updated:?}"),
            "case {case:?}: GetAccessProfile did not return the record the update persisted"
        );

        // `updatedAt` moves when a member moves, and stays where the create left it when
        // the patch asks for nothing new.
        let moved = row.expected_name != row.initial_name
            || row.expected_description != row.initial_description;
        if moved {
            assert!(
                updated.updated_at.secs() >= created.updated_at.secs(),
                "case {case:?}: updatedAt {:?} precedes the value the create left, {:?}",
                updated.updated_at,
                created.updated_at
            );
        } else {
            assert_eq!(
                millis(updated.updated_at),
                millis(created.updated_at),
                "case {case:?}: a patch that changed nothing moved updatedAt to {:?}",
                updated.updated_at
            );
        }
    }
}

/// One row of the dashboard patch table.
struct DashboardPatch {
    case: &'static str,
    initial_name: String,
    initial_body: String,
    initial_description: Option<String>,
    patch_name: Option<String>,
    patch_body: Option<String>,
    patch_description: Option<String>,
    expected_name: String,
    expected_body: String,
    expected_description: Option<String>,
}

/// The rows differ along WHICH members the patch carries and along the FORM of the
/// dashboard body — an empty `panels` list, one panel, many panels, a nested panel
/// document — because the rule is about the body being stored verbatim, not about the
/// text it is written as. The longest description is the bound of
/// `UpdateOmniDashboardInput.description`'s `@length(1..1024)`
/// (`smithy_query(query="shape",shape="UpdateOmniDashboardInput")`).
fn dashboard_patches() -> Vec<DashboardPatch> {
    let many_panels = format!(
        r#"{{"panels":[{}]}}"#,
        (0..12)
            .map(|index| format!(
                r#"{{"type":"timeSeries","layout":{{"row":{index},"title":"p{index}"}}}}"#
            ))
            .collect::<Vec<String>>()
            .join(",")
    );
    let nested_panel =
        "{\n  \"panels\": [\n    {\"type\": \"logs\", \"layout\": {\"rows\":[{\"cells\":[{\"widget\":{}}]}]}}\n  ]\n}";
    vec![
        DashboardPatch {
            case: "the description alone is replaced",
            initial_name: "dashboard-one".to_string(),
            initial_body: PANELS_BODY.to_string(),
            initial_description: Some("first".to_string()),
            patch_name: None,
            patch_body: None,
            patch_description: Some("second".to_string()),
            expected_name: "dashboard-one".to_string(),
            expected_body: PANELS_BODY.to_string(),
            expected_description: Some("second".to_string()),
        },
        DashboardPatch {
            case: "the body alone is replaced, by a document with one panel",
            initial_name: "dashboard-two".to_string(),
            initial_body: PANELS_BODY.to_string(),
            initial_description: None,
            patch_name: None,
            patch_body: Some(ONE_PANEL_BODY.to_string()),
            patch_description: None,
            expected_name: "dashboard-two".to_string(),
            expected_body: ONE_PANEL_BODY.to_string(),
            expected_description: None,
        },
        DashboardPatch {
            case: "the body alone is replaced, by a document with many panels",
            initial_name: "dashboard-three".to_string(),
            initial_body: ONE_PANEL_BODY.to_string(),
            initial_description: Some("kept".to_string()),
            patch_name: None,
            patch_body: Some(many_panels.clone()),
            patch_description: None,
            expected_name: "dashboard-three".to_string(),
            expected_body: many_panels,
            expected_description: Some("kept".to_string()),
        },
        DashboardPatch {
            case: "the name alone keeps the body and the description",
            initial_name: "dashboard-four".to_string(),
            initial_body: PANELS_BODY.to_string(),
            initial_description: Some("kept".to_string()),
            patch_name: Some("dashboard-four.renamed".to_string()),
            patch_body: None,
            patch_description: None,
            expected_name: "dashboard-four.renamed".to_string(),
            expected_body: PANELS_BODY.to_string(),
            expected_description: Some("kept".to_string()),
        },
        DashboardPatch {
            case: "every updatable member at once, description at @length(max: 1024)",
            initial_name: "dashboard-five".to_string(),
            initial_body: PANELS_BODY.to_string(),
            initial_description: None,
            patch_name: Some("dashboard_five@2".to_string()),
            patch_body: Some(nested_panel.to_string()),
            patch_description: Some("d".repeat(1024)),
            expected_name: "dashboard_five@2".to_string(),
            expected_body: nested_panel.to_string(),
            expected_description: Some("d".repeat(1024)),
        },
        DashboardPatch {
            case: "a request carrying no updatable member leaves the dashboard whole",
            initial_name: "dashboard-six".to_string(),
            initial_body: PANELS_BODY.to_string(),
            initial_description: Some("kept".to_string()),
            patch_name: None,
            patch_body: None,
            patch_description: None,
            expected_name: "dashboard-six".to_string(),
            expected_body: PANELS_BODY.to_string(),
            expected_description: Some("kept".to_string()),
        },
        DashboardPatch {
            case: "a patch asking for the values the dashboard already holds",
            initial_name: "dashboard-seven".to_string(),
            initial_body: PANELS_BODY.to_string(),
            initial_description: Some("kept".to_string()),
            patch_name: Some("dashboard-seven".to_string()),
            patch_body: Some(PANELS_BODY.to_string()),
            patch_description: Some("kept".to_string()),
            expected_name: "dashboard-seven".to_string(),
            expected_body: PANELS_BODY.to_string(),
            expected_description: Some("kept".to_string()),
        },
    ]
}

#[tokio::test]
async fn update_omni_dashboard_persists_only_the_members_the_request_carries() {
    let omni = Omni::new();
    let space_id = omni.space("dashboards", "update-dashboards").await;

    for row in dashboard_patches() {
        let case = row.case;
        let created = omni
            .create_dashboard(
                case,
                &space_id,
                &row.initial_name,
                &row.initial_body,
                row.initial_description.as_deref(),
            )
            .await;

        let updated = omni
            .update_dashboard(&UpdateOmniDashboardInput {
                space_id: space_id.clone(),
                dashboard_id: created.dashboard_id.clone(),
                body: row.patch_body.clone(),
                name: row.patch_name.clone(),
                description: row.patch_description.clone(),
            })
            .await
            .unwrap_or_else(|error| panic!("case {case:?}: UpdateOmniDashboard failed: {error:?}"));

        assert_eq!(
            (
                updated.name.as_str(),
                updated.body.as_str(),
                updated.description.as_deref()
            ),
            (
                row.expected_name.as_str(),
                row.expected_body.as_str(),
                row.expected_description.as_deref()
            ),
            "case {case:?}: the updated dashboard is {updated:?}"
        );
        assert_eq!(
            untouched_dashboard(&updated),
            untouched_dashboard(&created),
            "case {case:?}: a member UpdateOmniDashboard's input cannot name was changed"
        );

        let fetched = omni
            .stored_dashboard(case, &space_id, &created.dashboard_id)
            .await;
        assert_eq!(
            format!("{fetched:?}"),
            format!("{updated:?}"),
            "case {case:?}: GetOmniDashboard did not return the record the update persisted"
        );

        let moved = row.expected_name != row.initial_name
            || row.expected_body != row.initial_body
            || row.expected_description != row.initial_description;
        if moved {
            assert!(
                updated.updated_at.secs() >= created.updated_at.secs(),
                "case {case:?}: updatedAt {:?} precedes the value the create left, {:?}",
                updated.updated_at,
                created.updated_at
            );
        } else {
            assert_eq!(
                millis(updated.updated_at),
                millis(created.updated_at),
                "case {case:?}: a patch that changed nothing moved updatedAt to {:?}",
                updated.updated_at
            );
        }
    }
}

/// One row of the view patch table.
struct ViewPatch {
    case: &'static str,
    suffix: &'static str,
    initial_definition: String,
    initial_description: Option<String>,
    patch_definition: Option<String>,
    patch_description: Option<String>,
    expected_definition: String,
    expected_description: Option<String>,
}

/// The rows differ along WHICH members the patch carries and along the FORM of the SQL the
/// new definition is written as — projection, predicate, aggregate, set operation, common
/// table expression, a leading comment — because the rule is about the definition being
/// ONE query rather than about its text. The longest description is the bound of
/// `ViewDescription`'s `@length(max: 1024)`
/// (`smithy_query(query="shape",shape="ViewDescription")`).
fn view_patches() -> Vec<ViewPatch> {
    vec![
        ViewPatch {
            case: "the description alone is replaced",
            suffix: "one",
            initial_definition: "SELECT * FROM logs".to_string(),
            initial_description: Some("first".to_string()),
            patch_definition: None,
            patch_description: Some("second".to_string()),
            expected_definition: "SELECT * FROM logs".to_string(),
            expected_description: Some("second".to_string()),
        },
        ViewPatch {
            case: "the definition alone is replaced by a predicate query",
            suffix: "two",
            initial_definition: "SELECT * FROM logs".to_string(),
            initial_description: None,
            patch_definition: Some(
                "SELECT service, message FROM logs WHERE level = 'ERROR'".to_string(),
            ),
            patch_description: None,
            expected_definition: "SELECT service, message FROM logs WHERE level = 'ERROR'"
                .to_string(),
            expected_description: None,
        },
        ViewPatch {
            case: "the definition alone is replaced by an aggregate query",
            suffix: "three",
            initial_definition: "SELECT * FROM logs".to_string(),
            initial_description: Some("kept".to_string()),
            patch_definition: Some(
                "SELECT service, count(*) AS errors FROM logs GROUP BY service".to_string(),
            ),
            patch_description: None,
            expected_definition: "SELECT service, count(*) AS errors FROM logs GROUP BY service"
                .to_string(),
            expected_description: Some("kept".to_string()),
        },
        ViewPatch {
            case: "the definition alone is replaced by a set operation",
            suffix: "four",
            initial_definition: "SELECT * FROM logs".to_string(),
            initial_description: None,
            patch_definition: Some("SELECT id FROM logs UNION SELECT id FROM spans".to_string()),
            patch_description: None,
            expected_definition: "SELECT id FROM logs UNION SELECT id FROM spans".to_string(),
            expected_description: None,
        },
        ViewPatch {
            case: "both members at once, a common table expression and the longest description",
            suffix: "five",
            initial_definition: "SELECT * FROM logs".to_string(),
            initial_description: Some("first".to_string()),
            patch_definition: Some(
                "WITH errors AS (SELECT * FROM logs WHERE level = 'ERROR') SELECT count(*) FROM \
                 errors"
                    .to_string(),
            ),
            patch_description: Some("d".repeat(1024)),
            expected_definition:
                "WITH errors AS (SELECT * FROM logs WHERE level = 'ERROR') SELECT count(*) FROM \
                 errors"
                    .to_string(),
            expected_description: Some("d".repeat(1024)),
        },
        ViewPatch {
            case: "a definition behind a leading comment",
            suffix: "six",
            initial_definition: "SELECT 1".to_string(),
            initial_description: None,
            patch_definition: Some(
                "-- the day's errors\nSELECT count(*) FROM logs WHERE level = 'ERROR'".to_string(),
            ),
            patch_description: None,
            expected_definition:
                "-- the day's errors\nSELECT count(*) FROM logs WHERE level = 'ERROR'".to_string(),
            expected_description: None,
        },
        ViewPatch {
            case: "a request carrying no updatable member leaves the view whole",
            suffix: "seven",
            initial_definition: "SELECT * FROM logs".to_string(),
            initial_description: Some("kept".to_string()),
            patch_definition: None,
            patch_description: None,
            expected_definition: "SELECT * FROM logs".to_string(),
            expected_description: Some("kept".to_string()),
        },
        ViewPatch {
            case: "a patch asking for the values the view already holds",
            suffix: "eight",
            initial_definition: "SELECT * FROM logs".to_string(),
            initial_description: Some("kept".to_string()),
            patch_definition: Some("SELECT * FROM logs".to_string()),
            patch_description: Some("kept".to_string()),
            expected_definition: "SELECT * FROM logs".to_string(),
            expected_description: Some("kept".to_string()),
        },
    ]
}

#[tokio::test]
async fn update_view_persists_only_the_members_the_request_carries() {
    let omni = Omni::new();

    for row in view_patches() {
        let case = row.case;
        let name = format!("view.update-{}", row.suffix);
        let created = omni
            .create_view(
                case,
                &name,
                &row.initial_definition,
                row.initial_description.as_deref(),
            )
            .await;

        let updated = omni
            .update_view(&UpdateViewInput {
                name: name.clone(),
                definition: row.patch_definition.clone(),
                description: row.patch_description.clone(),
            })
            .await
            .unwrap_or_else(|error| panic!("case {case:?}: UpdateView failed: {error:?}"));

        assert_eq!(
            (
                updated.definition.as_str(),
                updated.description.as_deref(),
                updated.r#type.as_str()
            ),
            (
                row.expected_definition.as_str(),
                row.expected_description.as_deref(),
                VIEW_TYPE_USER
            ),
            "case {case:?}: the updated view is {updated:?}"
        );
        assert_eq!(
            untouched_view(&updated),
            untouched_view(&created),
            "case {case:?}: a member UpdateView's input cannot name was changed"
        );

        let fetched = omni.stored_view(case, &name).await;
        assert_eq!(
            format!("{fetched:?}"),
            format!("{updated:?}"),
            "case {case:?}: GetView did not return the record the update persisted"
        );

        let moved = row.expected_definition != row.initial_definition
            || row.expected_description != row.initial_description;
        if moved {
            assert!(
                updated.updated_at.secs() >= created.updated_at.secs(),
                "case {case:?}: updatedAt {:?} precedes the value the create left, {:?}",
                updated.updated_at,
                created.updated_at
            );
        } else {
            assert_eq!(
                millis(updated.updated_at),
                millis(created.updated_at),
                "case {case:?}: a patch that changed nothing moved updatedAt to {:?}",
                updated.updated_at
            );
        }
    }
}

#[tokio::test]
async fn an_update_of_a_record_the_store_does_not_hold_is_the_modelled_404() {
    let omni = Omni::new();
    let space_id = omni.space("misses", "update-misses").await;
    let other_space = omni.space("misses", "update-misses-other").await;
    let profile = omni
        .create_profile("misses", &space_id, "profile", None)
        .await;
    let dashboard = omni
        .create_dashboard("misses", &space_id, "dashboard", PANELS_BODY, None)
        .await;
    omni.create_view("misses", "view.update-miss", "SELECT 1", None)
        .await;

    // Rows: an id nothing names, and a LIVE id addressed under another space — a record
    // is addressed by its space AND its id, so the second is as much a miss as the first.
    // A space the ACCOUNT does not hold is a different answer and has its own test
    // ([`a_space_the_account_does_not_hold_authorizes_no_patch`]).
    for (case, input) in [
        (
            "an unknown profileId",
            UpdateAccessProfileInput {
                space_id: space_id.clone(),
                profile_id: MISSING_PROFILE_ID.to_string(),
                description: Some("second".to_string()),
                ..Default::default()
            },
        ),
        (
            "a live profile addressed under another space",
            UpdateAccessProfileInput {
                space_id: other_space.clone(),
                profile_id: profile.profile_id.clone(),
                description: Some("second".to_string()),
                ..Default::default()
            },
        ),
    ] {
        expect_modelled_error(
            case,
            omni.update_profile(&input).await,
            404,
            "ResourceNotFoundException",
        );
    }

    for (case, input) in [
        (
            "an unknown dashboardId",
            UpdateOmniDashboardInput {
                space_id: space_id.clone(),
                dashboard_id: MISSING_DASHBOARD_ID.to_string(),
                description: Some("second".to_string()),
                ..Default::default()
            },
        ),
        (
            "a live dashboard addressed under another space",
            UpdateOmniDashboardInput {
                space_id: other_space.clone(),
                dashboard_id: dashboard.dashboard_id.clone(),
                description: Some("second".to_string()),
                ..Default::default()
            },
        ),
    ] {
        expect_modelled_error(
            case,
            omni.update_dashboard(&input).await,
            404,
            "ResourceNotFoundException",
        );
    }

    expect_modelled_error(
        "an unknown view name",
        omni.update_view(&UpdateViewInput {
            name: MISSING_VIEW_NAME.to_string(),
            description: Some("second".to_string()),
            ..Default::default()
        })
        .await,
        404,
        "ResourceNotFoundException",
    );

    // None of the refusals touched the records the store does hold.
    let stored_profile = omni
        .stored_profile("misses", &space_id, &profile.profile_id)
        .await;
    assert_eq!(
        stored_profile.description, None,
        "a refused patch wrote a description onto the profile: {stored_profile:?}"
    );
    let stored_dashboard = omni
        .stored_dashboard("misses", &space_id, &dashboard.dashboard_id)
        .await;
    assert_eq!(
        stored_dashboard.description, None,
        "a refused patch wrote a description onto the dashboard: {stored_dashboard:?}"
    );
    let stored_view = omni.stored_view("misses", "view.update-miss").await;
    assert_eq!(
        stored_view.description, None,
        "a refused patch wrote a description onto the view: {stored_view:?}"
    );
}

/// The authorization refusal's required shape and text, and a `SpaceId` naming another
/// space, from the suite that owns the space rule.
use super::domain_space_lifecycle::{
    expect_authorization_failure, flip_last_digit, AUTHORIZATION_FAILED,
};

#[tokio::test]
async fn a_space_the_account_does_not_hold_authorizes_no_patch() {
    let omni = Omni::new();
    let space_id = omni.space("authorization", "update-authorization").await;
    let profile = omni
        .create_profile("authorization", &space_id, "profile", None)
        .await;
    let dashboard = omni
        .create_dashboard(
            "authorization",
            &space_id,
            "dashboard",
            PANELS_BODY,
            None,
        )
        .await;

    // Rows span the `SpaceId` class — nil digits, every hexadecimal digit, and an id one
    // character from the space the account holds — because the decision is about whether
    // the account holds a space under the id, not about its spelling.
    for (case, unheld) in [
        ("an id of nil digits", MISSING_SPACE_ID.to_string()),
        (
            "an id of every hexadecimal digit",
            "0123456f-4567-89ab-cdef-0123456789ab".to_string(),
        ),
        (
            "an id one character from the held space",
            flip_last_digit(&space_id),
        ),
    ] {
        expect_authorization_failure(
            &format!("UpdateAccessProfile, {case}"),
            omni.update_profile(&UpdateAccessProfileInput {
                space_id: unheld.clone(),
                profile_id: profile.profile_id.clone(),
                description: Some("second".to_string()),
                ..Default::default()
            })
            .await,
            &[unheld.as_str(), profile.profile_id.as_str()],
        );
        // The dashboard patch is refused for a body the schema ACCEPTS and for one it
        // refuses alike: authorization is decided before the body is read, so an
        // unauthorized caller is never told to repair its payload.
        for (body_case, body) in [
            ("no body at all", None),
            ("a body the schema accepts", Some(PANELS_BODY.to_string())),
            (
                "a body the schema refuses",
                Some(r#"{"widgets":[]}"#.to_string()),
            ),
        ] {
            expect_authorization_failure(
                &format!("UpdateOmniDashboard, {case}, {body_case}"),
                omni.update_dashboard(&UpdateOmniDashboardInput {
                    space_id: unheld.clone(),
                    dashboard_id: dashboard.dashboard_id.clone(),
                    body,
                    description: Some("second".to_string()),
                    ..Default::default()
                })
                .await,
                &[
                    unheld.as_str(),
                    dashboard.dashboard_id.as_str(),
                    "constraint",
                ],
            );
        }
    }

    // In the space the account DOES hold, that same refused body is the modelled 400: the
    // decision changes which refusal comes first, not whether the schema is enforced.
    expect_modelled_error(
        "a body the schema refuses, in the held space",
        omni.update_dashboard(&UpdateOmniDashboardInput {
            space_id: space_id.clone(),
            dashboard_id: dashboard.dashboard_id.clone(),
            body: Some(r#"{"widgets":[]}"#.to_string()),
            ..Default::default()
        })
        .await,
        400,
        "ValidationException",
    );

    // None of the refusals touched the records the account holds.
    let stored_profile = omni
        .stored_profile("authorization", &space_id, &profile.profile_id)
        .await;
    assert_eq!(
        stored_profile.description, None,
        "a refused patch wrote a description onto the profile: {stored_profile:?}"
    );
    let stored_dashboard = omni
        .stored_dashboard("authorization", &space_id, &dashboard.dashboard_id)
        .await;
    assert_eq!(
        (
            stored_dashboard.description.clone(),
            stored_dashboard.body.as_str()
        ),
        (None, PANELS_BODY),
        "a refused patch changed the dashboard: {stored_dashboard:?}"
    );
}


#[tokio::test]
async fn a_rename_onto_a_name_a_sibling_holds_is_the_modelled_conflict_and_writes_nothing() {
    let omni = Omni::new();
    let space_id = omni.space("conflict", "update-conflict").await;
    let elsewhere = omni.space("conflict", "update-conflict-other").await;

    let first = omni
        .create_profile("conflict", &space_id, "taken profile", None)
        .await;
    let second = omni
        .create_profile("conflict", &space_id, "free profile", Some("kept"))
        .await;
    // The same name in ANOTHER space identifies a different profile, so renaming onto it
    // is not a conflict.
    omni.create_profile("conflict", &elsewhere, "taken profile", None)
        .await;

    expect_modelled_error(
        "a profile renamed onto a sibling's name",
        omni.update_profile(&UpdateAccessProfileInput {
            space_id: space_id.clone(),
            profile_id: second.profile_id.clone(),
            name: Some("taken profile".to_string()),
            // The description rides along, so the refusal is shown to roll back the whole
            // patch rather than only the rename.
            description: Some("second".to_string()),
        })
        .await,
        409,
        "ConflictException",
    );
    let stored = omni
        .stored_profile("conflict", &space_id, &second.profile_id)
        .await;
    assert_eq!(
        (stored.name.as_str(), stored.description.as_deref()),
        ("free profile", Some("kept")),
        "the refused rename changed the profile: {stored:?}"
    );

    // Renaming a profile to the name it already holds is not a conflict with itself.
    let kept = omni
        .update_profile(&UpdateAccessProfileInput {
            space_id: space_id.clone(),
            profile_id: first.profile_id.clone(),
            name: Some("taken profile".to_string()),
            description: None,
        })
        .await
        .expect("renaming a profile to its own name succeeds")
        .access_profile;
    assert_eq!(
        kept.name, "taken profile",
        "the self-rename changed the name to {:?}",
        kept.name
    );
    // The same name is free in another space.
    let moved = omni
        .update_profile(&UpdateAccessProfileInput {
            space_id: elsewhere.clone(),
            profile_id: omni
                .create_profile("conflict", &elsewhere, "free profile", None)
                .await
                .profile_id,
            name: Some("renamed in the other space".to_string()),
            description: None,
        })
        .await
        .expect("a rename in another space succeeds")
        .access_profile;
    assert_eq!(
        moved.name, "renamed in the other space",
        "the rename in the other space answered {moved:?}"
    );

    // A dashboard's identity is its minted id, and its name carries no uniqueness rule:
    // a rename onto a sibling's name is accepted, and both dashboards keep their own
    // records under that one name.
    let taken = omni
        .create_dashboard("conflict", &space_id, "taken-dashboard", PANELS_BODY, None)
        .await;
    let free = omni
        .create_dashboard(
            "conflict",
            &space_id,
            "free-dashboard",
            PANELS_BODY,
            Some("kept"),
        )
        .await;
    let renamed = omni
        .update_dashboard(&UpdateOmniDashboardInput {
            space_id: space_id.clone(),
            dashboard_id: free.dashboard_id.clone(),
            name: Some("taken-dashboard".to_string()),
            description: Some("second".to_string()),
            body: Some(ONE_PANEL_BODY.to_string()),
        })
        .await
        .expect("renaming a dashboard onto a sibling's name succeeds");
    assert_eq!(
        (
            renamed.dashboard_id.as_str(),
            renamed.name.as_str(),
            renamed.body.as_str(),
            renamed.description.as_deref()
        ),
        (
            free.dashboard_id.as_str(),
            "taken-dashboard",
            ONE_PANEL_BODY,
            Some("second")
        ),
        "the rename answered {renamed:?}"
    );
    let stored = omni
        .stored_dashboard("conflict", &space_id, &free.dashboard_id)
        .await;
    assert_eq!(
        (
            stored.name.as_str(),
            stored.body.as_str(),
            stored.description.as_deref()
        ),
        ("taken-dashboard", ONE_PANEL_BODY, Some("second")),
        "the rename is not what the store holds: {stored:?}"
    );
    // The sibling it took its name from is untouched and still readable on its own id.
    let sibling = omni
        .stored_dashboard("conflict", &space_id, &taken.dashboard_id)
        .await;
    assert_eq!(
        (sibling.name.as_str(), sibling.body.as_str()),
        ("taken-dashboard", PANELS_BODY),
        "the rename changed the sibling: {sibling:?}"
    );
    let kept = omni
        .update_dashboard(&UpdateOmniDashboardInput {
            space_id: space_id.clone(),
            dashboard_id: taken.dashboard_id.clone(),
            name: Some("taken-dashboard".to_string()),
            ..Default::default()
        })
        .await
        .expect("renaming a dashboard to its own name succeeds");
    assert_eq!(
        kept.name, "taken-dashboard",
        "the self-rename changed the name to {:?}",
        kept.name
    );
}

#[tokio::test]
async fn a_rename_onto_a_taken_name_states_the_services_one_sentence_for_the_kind() {
    let omni = Omni::new();
    let space_id = omni.space("conflict wording", "update-conflict-wording").await;

    // Rows span the name class `AccessProfileName` admits (`@length(1..256)`,
    // `^[a-zA-Z0-9 _.:/=+@-]+$`), because the refusal is one sentence for the kind: it
    // states neither the name the patch asked for nor the space the profile lives in.
    for (index, name) in ["a", "Taken Profile _.:/=+@-", &"n".repeat(256)]
        .into_iter()
        .enumerate()
    {
        omni.create_profile("conflict wording", &space_id, name, None)
            .await;
        let free = omni
            .create_profile(
                "conflict wording",
                &space_id,
                &format!("free profile {index}"),
                None,
            )
            .await;
        let refusal = omni
            .update_profile(&UpdateAccessProfileInput {
                space_id: space_id.clone(),
                profile_id: free.profile_id.clone(),
                name: Some(name.to_string()),
                description: None,
            })
            .await
            .expect_err(&format!("case {name:?}: the rename was admitted"));
        assert_eq!(
            refusal.http_status(),
            409,
            "case {name:?}: expected HTTP 409, got {} ({refusal:?})",
            refusal.http_status()
        );
        let message = match refusal {
            Scaffolded::Modeled(UpdateAccessProfileErrorModeled::ConflictException(conflict)) => {
                conflict.message
            }
            other => panic!("case {name:?}: expected the modelled ConflictException, got {other:?}"),
        };
        assert_eq!(
            message, ACCESS_PROFILE_NAME_TAKEN,
            "case {name:?}: expected {ACCESS_PROFILE_NAME_TAKEN:?}, got {message:?}"
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
async fn a_definition_that_violates_its_own_rule_is_refused_and_the_record_is_untouched() {
    let omni = Omni::new();
    let space_id = omni.space("invalid", "update-invalid").await;
    let dashboard = omni
        .create_dashboard(
            "invalid",
            &space_id,
            "dashboard",
            PANELS_BODY,
            Some("kept"),
        )
        .await;

    // Rows span WHY a body is not a dashboard definition: it is not JSON, it is JSON of
    // the wrong kind, it carries no `panels` member, that member is not a list, an entry
    // of the list is not a panel object, an entry omits a member every panel must carry,
    // or the document carries a top-level member the closed schema does not declare.
    for (case, body) in [
        ("not JSON at all", "panels"),
        ("a JSON array", r#"[{"type":"logs"}]"#),
        ("no panels member", r#"{"widgets":[]}"#),
        ("panels as a string", r#"{"panels":"none"}"#),
        ("a panel that is a number", r#"{"panels":[1]}"#),
        ("a panel carrying neither required member", r#"{"panels":[{}]}"#),
        (
            "a panel carrying only its type",
            r#"{"panels":[{"type":"logs"}]}"#,
        ),
        (
            "a conforming list beside an undeclared member",
            r#"{"panels":[],"widgets":[]}"#,
        ),
    ] {
        expect_modelled_error(
            case,
            omni.update_dashboard(&UpdateOmniDashboardInput {
                space_id: space_id.clone(),
                dashboard_id: dashboard.dashboard_id.clone(),
                body: Some(body.to_string()),
                // The description rides along, so the refusal is shown to roll back the
                // whole patch.
                description: Some("second".to_string()),
                ..Default::default()
            })
            .await,
            400,
            "ValidationException",
        );
        let stored = omni
            .stored_dashboard(case, &space_id, &dashboard.dashboard_id)
            .await;
        assert_eq!(
            (stored.body.as_str(), stored.description.as_deref()),
            (PANELS_BODY, Some("kept")),
            "case {case:?}: the refused patch changed the dashboard: {stored:?}"
        );
    }

    let name = "view.update-invalid";
    omni.create_view("invalid", name, "SELECT * FROM logs", Some("kept"))
        .await;
    // Rows span WHY a definition is not ONE query: it is not SQL, it is incomplete, it is
    // a statement that is not a query, or it is more than one statement.
    for (case, definition) in [
        ("prose", "the daily error count"),
        ("a truncated query", "SELECT * FROM"),
        ("a comment only", "-- nothing but a comment"),
        ("a mutation", "DELETE FROM logs WHERE level = 'ERROR'"),
        ("a DDL statement", "CREATE TABLE logs (id INT)"),
        ("two queries", "SELECT 1; SELECT 2"),
    ] {
        expect_modelled_error(
            case,
            omni.update_view(&UpdateViewInput {
                name: name.to_string(),
                definition: Some(definition.to_string()),
                description: Some("second".to_string()),
            })
            .await,
            400,
            "ValidationException",
        );
        let stored = omni.stored_view(case, name).await;
        assert_eq!(
            (stored.definition.as_str(), stored.description.as_deref()),
            ("SELECT * FROM logs", Some("kept")),
            "case {case:?}: the refused patch changed the view: {stored:?}"
        );
    }
}

#[tokio::test]
async fn an_update_replayed_answers_the_same_record_and_later_patches_compose() {
    // All three operations are modelled `@idempotent`: repeating one patch answers the
    // same record, and a later patch of another member keeps what the earlier one set.
    let omni = Omni::new();
    let space_id = omni.space("compose", "update-compose").await;

    let profile = omni
        .create_profile("compose", &space_id, "profile", None)
        .await;
    let patch = UpdateAccessProfileInput {
        space_id: space_id.clone(),
        profile_id: profile.profile_id.clone(),
        description: Some("second".to_string()),
        ..Default::default()
    };
    let first = omni
        .update_profile(&patch)
        .await
        .expect("the profile patch succeeds")
        .access_profile;
    let replayed = omni
        .update_profile(&patch)
        .await
        .expect("replaying the profile patch succeeds")
        .access_profile;
    assert_eq!(
        format!("{replayed:?}"),
        format!("{first:?}"),
        "an @idempotent patch replayed must answer the same profile, got {replayed:?}"
    );
    let renamed = omni
        .update_profile(&UpdateAccessProfileInput {
            space_id: space_id.clone(),
            profile_id: profile.profile_id.clone(),
            name: Some("profile renamed".to_string()),
            ..Default::default()
        })
        .await
        .expect("the rename succeeds")
        .access_profile;
    assert_eq!(
        (renamed.name.as_str(), renamed.description.as_deref()),
        ("profile renamed", Some("second")),
        "a later patch dropped what the earlier one set: {renamed:?}"
    );

    let dashboard = omni
        .create_dashboard("compose", &space_id, "dashboard", PANELS_BODY, None)
        .await;
    let patch = UpdateOmniDashboardInput {
        space_id: space_id.clone(),
        dashboard_id: dashboard.dashboard_id.clone(),
        description: Some("second".to_string()),
        ..Default::default()
    };
    let first = omni
        .update_dashboard(&patch)
        .await
        .expect("the dashboard patch succeeds");
    let replayed = omni
        .update_dashboard(&patch)
        .await
        .expect("replaying the dashboard patch succeeds");
    assert_eq!(
        format!("{replayed:?}"),
        format!("{first:?}"),
        "an @idempotent patch replayed must answer the same dashboard, got {replayed:?}"
    );
    let rebodied = omni
        .update_dashboard(&UpdateOmniDashboardInput {
            space_id: space_id.clone(),
            dashboard_id: dashboard.dashboard_id.clone(),
            body: Some(ONE_PANEL_BODY.to_string()),
            ..Default::default()
        })
        .await
        .expect("the body patch succeeds");
    assert_eq!(
        (rebodied.body.as_str(), rebodied.description.as_deref()),
        (ONE_PANEL_BODY, Some("second")),
        "a later patch dropped what the earlier one set: {rebodied:?}"
    );

    let name = "view.update-compose";
    omni.create_view("compose", name, "SELECT * FROM logs", None)
        .await;
    let patch = UpdateViewInput {
        name: name.to_string(),
        description: Some("second".to_string()),
        ..Default::default()
    };
    let first = omni
        .update_view(&patch)
        .await
        .expect("the view patch succeeds");
    let replayed = omni
        .update_view(&patch)
        .await
        .expect("replaying the view patch succeeds");
    assert_eq!(
        format!("{replayed:?}"),
        format!("{first:?}"),
        "an @idempotent patch replayed must answer the same view, got {replayed:?}"
    );
    let redefined = omni
        .update_view(&UpdateViewInput {
            name: name.to_string(),
            definition: Some("SELECT count(*) FROM spans".to_string()),
            ..Default::default()
        })
        .await
        .expect("the definition patch succeeds");
    assert_eq!(
        (
            redefined.definition.as_str(),
            redefined.description.as_deref()
        ),
        ("SELECT count(*) FROM spans", Some("second")),
        "a later patch dropped what the earlier one set: {redefined:?}"
    );
}

#[tokio::test]
async fn an_update_and_the_read_that_follows_it_agree_over_the_rpcv2cbor_wire() {
    let omni = Omni::new();
    // The parents are created through the handlers holding the same engine the router's
    // service holds, which is what makes the wire calls below see them.
    let space_id = omni.space("wire", "update-wire").await;
    let created = omni
        .create_profile("wire", &space_id, "wire profile", Some("first"))
        .await;

    let codec = CborCodec::default();
    let (status, body) = omni
        .over_the_wire(
            "UpdateAccessProfile",
            UPDATE_ACCESS_PROFILE_INPUT_SCHEMA,
            &UpdateAccessProfileInput {
                space_id: space_id.clone(),
                profile_id: created.profile_id.clone(),
                description: Some("second".to_string()),
                ..Default::default()
            },
        )
        .await;
    assert_eq!(
        status,
        200,
        "UpdateAccessProfile over the wire answered {status}: {:?}",
        String::from_utf8_lossy(&body)
    );
    let updated = {
        let mut deserializer = codec.create_deserializer(&body);
        UpdateAccessProfileOutput::deserialize(&mut deserializer)
            .expect("the output deserializes")
            .access_profile
    };
    assert_eq!(
        updated.description.as_deref(),
        Some("second"),
        "the wire update answered {updated:?}"
    );

    let (status, body) = omni
        .over_the_wire(
            "GetAccessProfile",
            GET_ACCESS_PROFILE_INPUT_SCHEMA,
            &GetAccessProfileInput {
                space_id: space_id.clone(),
                profile_id: created.profile_id.clone(),
            },
        )
        .await;
    assert_eq!(
        status,
        200,
        "GetAccessProfile over the wire answered {status}: {:?}",
        String::from_utf8_lossy(&body)
    );
    let fetched = {
        let mut deserializer = codec.create_deserializer(&body);
        GetAccessProfileOutput::deserialize(&mut deserializer)
            .expect("the output deserializes")
            .access_profile
    };
    assert_eq!(
        (fetched.description.as_deref(), fetched.name.as_str()),
        (Some("second"), "wire profile"),
        "the wire read answered the pre-update record: {fetched:?}"
    );
    assert_eq!(
        untouched_profile(&fetched),
        untouched_profile(&created),
        "the wire update changed a member its input cannot name"
    );
}

/// The `message` of a modelled refusal, with its error code and HTTP status checked
/// first.
///
/// Each operation carries its OWN error enum, so the variant is named per call site; the
/// error struct behind every one of them carries the `message` the caller reads.
macro_rules! refusal_message {
    ($case:expr, $result:expr, $variant:path, $code:expr, $status:expr) => {{
        let case: &str = $case;
        match $result {
            Ok(output) => {
                panic!("case {case:?}: expected {} ({}), got Ok({output:?})", $code, $status)
            }
            Err(error) => {
                assert_eq!(
                    (error.shape_name(), error.http_status()),
                    ($code, $status),
                    "case {case:?}: expected {} ({}), got {error:?}",
                    $code,
                    $status
                );
                match error {
                    Scaffolded::Modeled($variant(modeled)) => modeled.message,
                    other => panic!(
                        "case {case:?}: expected the modelled {}, got {other:?}",
                        $code
                    ),
                }
            }
        }
    }};
}

/// The error code a miss carries, and its modelled status.
const NOT_FOUND: (&str, u16) = ("ResourceNotFoundException", 404);

#[tokio::test]
async fn a_dashboard_the_store_does_not_hold_is_named_by_its_id_alone() {
    let omni = Omni::new();
    let space_id = omni.space("miss-wording", "dashboard-miss").await;
    let other_space = omni.space("miss-wording", "dashboard-miss-other").await;
    let elsewhere = omni
        .create_dashboard(
            "miss-wording",
            &other_space,
            "dashboard-elsewhere",
            PANELS_BODY,
            None,
        )
        .await;

    // Rows span the ids the miss can be reported for: a `DashboardId` of nil digits, one
    // of mixed digits, one of the highest digit the pattern admits, and a LIVE id
    // addressed under a space that does not hold it — the identifier is taken from the
    // REQUEST, so the last row is reported under the id the request carried even though
    // the store holds a dashboard under it elsewhere.
    for (case, dashboard_id) in [
        (
            "an id of nil digits",
            // A `DashboardId` of the lowest digits its pattern admits, which names
            // nothing.
            "00000000-0000-4000-8000-000000000000".to_string(),
        ),
        ("an id of mixed digits", MISSING_DASHBOARD_ID.to_string()),
        (
            "an id at the top of the pattern's alphabet",
            "ffffffff-ffff-4fff-bfff-ffffffffffff".to_string(),
        ),
        (
            "a live id addressed under another space",
            elsewhere.dashboard_id.clone(),
        ),
    ] {
        // The template: the fixed "Dashboard not found: " and the dashboard id the
        // request carried — no space id, no quoting, no trailing period.
        let expected = format!("Dashboard not found: {dashboard_id}");
        let patched = refusal_message!(
            case,
            omni.update_dashboard(&UpdateOmniDashboardInput {
                space_id: space_id.clone(),
                dashboard_id: dashboard_id.clone(),
                name: Some("renamed".to_string()),
                body: Some(ONE_PANEL_BODY.to_string()),
                description: Some("second".to_string()),
            })
            .await,
            UpdateOmniDashboardErrorModeled::ResourceNotFoundException,
            NOT_FOUND.0,
            NOT_FOUND.1
        );
        assert_eq!(
            patched, expected,
            "case {case:?}: UpdateOmniDashboard expected {expected:?}, got {patched:?}"
        );
        // The reader of the same resource words the same miss the same way.
        let read = refusal_message!(
            case,
            omni.get_dashboard(&space_id, &dashboard_id).await,
            GetOmniDashboardErrorModeled::ResourceNotFoundException,
            NOT_FOUND.0,
            NOT_FOUND.1
        );
        assert_eq!(
            read, expected,
            "case {case:?}: GetOmniDashboard expected {expected:?}, got {read:?}"
        );
    }

    // The dashboard the other space DOES hold is untouched by the refusals above.
    let stored = omni
        .stored_dashboard("miss-wording", &other_space, &elsewhere.dashboard_id)
        .await;
    assert_eq!(
        (stored.name.as_str(), stored.body.as_str()),
        ("dashboard-elsewhere", PANELS_BODY),
        "a refused patch changed the dashboard it could not find: {stored:?}"
    );
}

#[tokio::test]
async fn a_view_the_store_does_not_hold_is_named_by_its_name_alone() {
    let omni = Omni::new();
    omni.create_view("miss-wording", "view.live-view", "SELECT 1", None)
        .await;

    // Rows span the `ViewName` class (`@length(6..256)`,
    // `^view\.[a-z0-9][a-z0-9._-]{0,250}$`): the shortest name the model admits, one
    // carrying the punctuation the pattern allows, one starting with a digit and the
    // longest name the model admits.
    for (case, name) in [
        ("the shortest name the model admits", "view.a".to_string()),
        ("a name with punctuation", MISSING_VIEW_NAME.to_string()),
        (
            "a name starting with a digit",
            "view.0_errors-by-service".to_string(),
        ),
        (
            "the longest name the model admits",
            format!("view.{}", "v".repeat(251)),
        ),
    ] {
        // The template: the fixed "View not found: " and the name the request carried —
        // unquoted, with no trailing period.
        let expected = format!("View not found: {name}");
        let patched = refusal_message!(
            case,
            omni.update_view(&UpdateViewInput {
                name: name.clone(),
                definition: Some("SELECT count(*) FROM logs".to_string()),
                description: Some("second".to_string()),
            })
            .await,
            UpdateViewErrorModeled::ResourceNotFoundException,
            NOT_FOUND.0,
            NOT_FOUND.1
        );
        assert_eq!(
            patched, expected,
            "case {case:?}: UpdateView expected {expected:?}, got {patched:?}"
        );
        let read = refusal_message!(
            case,
            omni.get_view(&name).await,
            GetViewErrorModeled::ResourceNotFoundException,
            NOT_FOUND.0,
            NOT_FOUND.1
        );
        assert_eq!(
            read, expected,
            "case {case:?}: GetView expected {expected:?}, got {read:?}"
        );
        let deleted = refusal_message!(
            case,
            omni.delete_view(&name).await,
            DeleteViewErrorModeled::ResourceNotFoundException,
            NOT_FOUND.0,
            NOT_FOUND.1
        );
        assert_eq!(
            deleted, expected,
            "case {case:?}: DeleteView expected {expected:?}, got {deleted:?}"
        );
    }

    // The view the store DOES hold is still there, and its name is not what any refusal
    // above reported.
    let stored = omni.stored_view("miss-wording", "view.live-view").await;
    assert_eq!(
        (stored.name.as_str(), stored.definition.as_str()),
        ("view.live-view", "SELECT 1"),
        "a refused patch changed the view it could not find: {stored:?}"
    );
}

/// One clause of a dashboard-body refusal: WHERE the document violates the schema and
/// WHICH constraint it violates there.
fn clause(location: &str, constraint: &str) -> String {
    format!("{location}: does not satisfy the '{constraint}' constraint of the dashboard body schema")
}

#[tokio::test]
async fn a_patched_body_is_held_to_the_whole_closed_schema_and_names_every_violation() {
    let omni = Omni::new();
    let space_id = omni.space("closed", "update-closed").await;
    let dashboard = omni
        .create_dashboard("closed", &space_id, "dashboard", PANELS_BODY, Some("kept"))
        .await;

    // Rows span the objects the schema closes and the violations one body can collect at
    // once: a panel carrying a member the panel schema does not declare, a panel also
    // missing what it is required to carry, several undeclared members in one panel, an
    // undeclared member in a LATER panel, and the document's own closedness beside a
    // panel's. The clauses come in schema order — within one object, what it must carry
    // before what it carries that is not declared.
    for (case, body, clauses) in [
        (
            "a panel carrying what it must and one member more",
            r#"{"panels":[{"type":"logs","layout":{},"title":"p99"}]}"#,
            vec![clause("panels[0]", "additionalProperties")],
        ),
        (
            "a panel missing both required members and carrying an undeclared one",
            r#"{"panels":[{"x":1}]}"#,
            vec![
                clause("panels[0].type", "required"),
                clause("panels[0].layout", "required"),
                clause("panels[0]", "additionalProperties"),
            ],
        ),
        (
            "a panel carrying several undeclared members",
            r#"{"panels":[{"type":"logs","layout":{},"title":"p99","id":7,"query":{}}]}"#,
            vec![clause("panels[0]", "additionalProperties")],
        ),
        (
            "the second panel carrying an undeclared member",
            r#"{"panels":[{"type":"logs","layout":{}},{"type":"logs","layout":{},"x":1}]}"#,
            vec![clause("panels[1]", "additionalProperties")],
        ),
        (
            "an undeclared member in a panel and one in the document",
            r#"{"panels":[{"type":"logs","layout":{},"x":1}],"widgets":[]}"#,
            vec![
                clause("panels[0]", "additionalProperties"),
                clause("$", "additionalProperties"),
            ],
        ),
    ] {
        let expected = clauses.join("; ");
        let actual = refusal_message!(
            case,
            omni.update_dashboard(&UpdateOmniDashboardInput {
                space_id: space_id.clone(),
                dashboard_id: dashboard.dashboard_id.clone(),
                body: Some(body.to_string()),
                // The description rides along, so the refusal is shown to roll back the
                // whole patch.
                description: Some("second".to_string()),
                ..Default::default()
            })
            .await,
            UpdateOmniDashboardErrorModeled::ValidationException,
            "ValidationException",
            400
        );
        assert_eq!(
            actual, expected,
            "case {case:?}: expected the message {expected:?}, got {actual:?}"
        );
        let stored = omni
            .stored_dashboard(case, &space_id, &dashboard.dashboard_id)
            .await;
        assert_eq!(
            (stored.body.as_str(), stored.description.as_deref()),
            (PANELS_BODY, Some("kept")),
            "case {case:?}: the refused patch changed the dashboard: {stored:?}"
        );
    }

    // A panel carrying exactly the members the schema declares is accepted, however
    // deeply it nests inside the ones it declares, and the body round-trips through the
    // reader that answers from the same store.
    for (case, body) in [
        ("a panel with an empty layout", r#"{"panels":[{"type":"logs","layout":{}}]}"#),
        (
            "a panel whose layout nests whatever it likes",
            r#"{"panels":[{"type":"timeSeries","layout":{"rows":[{"cells":[{"title":"p99"}]}]}}]}"#,
        ),
    ] {
        let patched = omni
            .update_dashboard(&UpdateOmniDashboardInput {
                space_id: space_id.clone(),
                dashboard_id: dashboard.dashboard_id.clone(),
                body: Some(body.to_string()),
                ..Default::default()
            })
            .await
            .unwrap_or_else(|error| panic!("case {case:?}: the body was refused: {error:?}"));
        let stored = omni
            .stored_dashboard(case, &space_id, &dashboard.dashboard_id)
            .await;
        assert_eq!(
            (patched.body.as_str(), stored.body.as_str()),
            (body, body),
            "case {case:?}: expected the body {body:?} back, got {:?} and {:?}",
            patched.body,
            stored.body
        );
    }
}

/// The refusal a body whose TEXT does not parse carries: the fixed text of the
/// dashboard-body rule, the line the parse stopped on and the position in that line,
/// counted from the start of the line with its first character at position 0.
fn not_valid_json(line: usize, column: usize) -> String {
    format!("dashboard body is not valid JSON at line {line} column {column}")
}

#[tokio::test]
async fn a_patched_body_that_is_not_json_is_refused_at_the_position_the_parse_failed() {
    let omni = Omni::new();
    let space_id = omni.space("parse", "update-parse").await;
    let dashboard = omni
        .create_dashboard("parse", &space_id, "dashboard", PANELS_BODY, Some("kept"))
        .await;
    let before = untouched_dashboard(&dashboard);

    // A patched body that is not JSON at all has no document to hold to the schema, so
    // the refusal reports the parse instead of a schema clause. The rows differ along
    // WHERE the text stops being JSON: at its very first character, with nothing in it at
    // all, at the end of a truncated document, at a member the document never names, past
    // the document's end, on a LATER line, and part-way into a later line — so the
    // position reported is a property of the submitted text rather than a fixed pair.
    for (case, body, expected) in [
        (
            "text that is not JSON at all",
            "panels".to_string(),
            not_valid_json(1, 0),
        ),
        ("a body with no text at all", String::new(), not_valid_json(1, 0)),
        (
            "a document truncated inside its first panel",
            r#"{"panels":[{"type":"logs"}"#.to_string(),
            not_valid_json(1, 25),
        ),
        (
            "a member missing after a comma",
            r#"{"panels":[],}"#.to_string(),
            not_valid_json(1, 13),
        ),
        (
            "text past the end of the document",
            format!("{PANELS_BODY} and more"),
            not_valid_json(1, 14),
        ),
        (
            "a document that fails on its third line",
            "{\n  \"panels\": [],\n}".to_string(),
            not_valid_json(3, 0),
        ),
        (
            "a document that fails part-way into its second line",
            "{\n  \"panels\": [,]\n}".to_string(),
            not_valid_json(2, 13),
        ),
    ] {
        let actual = refusal_message!(
            case,
            omni.update_dashboard(&UpdateOmniDashboardInput {
                space_id: space_id.clone(),
                dashboard_id: dashboard.dashboard_id.clone(),
                body: Some(body.clone()),
                // A name and a description ride along, so the refusal is shown to roll
                // the WHOLE patch back rather than only the member it refused.
                name: Some("renamed".to_string()),
                description: Some("second".to_string()),
            })
            .await,
            UpdateOmniDashboardErrorModeled::ValidationException,
            "ValidationException",
            400
        );
        assert_eq!(
            actual, expected,
            "case {case:?}: expected the message {expected:?}, got {actual:?}"
        );
        // Nothing of the refused patch reached the store the reader answers from: the
        // dashboard still carries the body, the name and the description it had, and its
        // `updatedAt` has not moved.
        let stored = omni
            .stored_dashboard(case, &space_id, &dashboard.dashboard_id)
            .await;
        assert_eq!(
            (
                stored.body.as_str(),
                stored.name.as_str(),
                stored.description.as_deref(),
                millis(stored.updated_at),
            ),
            (
                PANELS_BODY,
                "dashboard",
                Some("kept"),
                millis(dashboard.updated_at),
            ),
            "case {case:?}: the refused patch changed the dashboard: {stored:?}"
        );
        assert_eq!(
            untouched_dashboard(&stored),
            before,
            "case {case:?}: the refused patch changed a member the input cannot name"
        );
    }

    // The refusals left the dashboard patchable: a body that IS a dashboard definition is
    // persisted and read back through the reader that answers from the same store.
    let patched = omni
        .update_dashboard(&UpdateOmniDashboardInput {
            space_id: space_id.clone(),
            dashboard_id: dashboard.dashboard_id.clone(),
            body: Some(ONE_PANEL_BODY.to_string()),
            ..Default::default()
        })
        .await
        .unwrap_or_else(|error| panic!("the conforming patch was refused: {error:?}"));
    let stored = omni
        .stored_dashboard("parse", &space_id, &dashboard.dashboard_id)
        .await;
    assert_eq!(
        (
            patched.body.as_str(),
            stored.body.as_str(),
            stored.name.as_str(),
            stored.description.as_deref(),
        ),
        (ONE_PANEL_BODY, ONE_PANEL_BODY, "dashboard", Some("kept")),
        "expected the conforming body to persist, got {patched:?} and {stored:?}"
    );
}

// ---------------------------------------------------------------------------------------
// The remaining resource kinds this service reports a miss for.
//
// The two tables above pin the wording for the kinds whose text has a place for the
// identifier (a dashboard, a view). The tables below pin it for the rest — a domain, a
// space, an access profile, an access grant and an alert — against the SAME rule: one
// text per resource kind, stated identically by every operation that reports a miss of
// that kind, carrying only the variable parts that kind's text has and taking those from
// the request.
// ---------------------------------------------------------------------------------------

/// AWS's text for a miss of a domain: wholly fixed, naming no identifier.
const DOMAIN_MISS: &str = "Domain not found";

/// AWS's text for a miss of a space.
const SPACE_MISS: &str = "Space not found";

/// AWS's text for a miss of an access profile, in the sentence case it is worded in.
const ACCESS_PROFILE_MISS: &str = "Access profile not found";

/// AWS's text for a miss of an access grant, naming the resource by its modelled
/// CamelCase shape name.
const ACCESS_GRANT_MISS: &str = "AccessGrant not found";

/// The message a miss of an alert carries: the fixed words around the alert id and the
/// space id the request addressed it by, with no trailing period.
fn alert_miss(alert_id: &str, space_id: &str) -> String {
    format!("No alert '{alert_id}' in space '{space_id}'")
}

impl Omni {
    async fn get_domain(&self, domain_id: &str) -> Result<GetDomainOutput, GetDomainError> {
        GetDomain::new(Arc::clone(&self.engine))
            .handle(
                GetDomainInput {
                    domain_id: domain_id.to_string(),
                },
                &self.ctx(),
            )
            .await
    }

    /// A patch of `domain_id` that asks for nothing, so the request reaches the lookup
    /// whatever it would have changed.
    async fn update_domain(
        &self,
        domain_id: &str,
    ) -> Result<UpdateDomainOutput, UpdateDomainError> {
        UpdateDomain::new(Arc::clone(&self.engine))
            .handle(
                UpdateDomainInput {
                    domain_id: domain_id.to_string(),
                    ..Default::default()
                },
                &self.ctx(),
            )
            .await
    }

    /// A create that names `domain_id` as the parent of the space it would mint.
    async fn create_space_in(&self, domain_id: &str, name: &str) -> Result<Space, CreateSpaceError> {
        CreateSpace::new(Arc::clone(&self.engine))
            .handle(
                CreateSpaceInput {
                    name: name.to_string(),
                    domain_id: domain_id.to_string(),
                    data_access_role_arn: DATA_ACCESS_ROLE_ARN.to_string(),
                    ..Default::default()
                },
                &self.ctx(),
            )
            .await
            .map(|output| output.space)
    }

    /// A listing filtered by `domain_id`.
    async fn list_spaces_in(&self, domain_id: &str) -> Result<ListSpacesOutput, ListSpacesError> {
        ListSpaces::new(Arc::clone(&self.engine))
            .handle(
                ListSpacesInput {
                    domain_id: Some(domain_id.to_string()),
                    ..Default::default()
                },
                &self.ctx(),
            )
            .await
    }

    /// A grant listing filtered by whichever parent the row names.
    async fn list_grants(
        &self,
        domain_id: Option<&str>,
        space_id: Option<&str>,
    ) -> Result<ListAccessGrantsOutput, ListAccessGrantsError> {
        ListAccessGrants::new(Arc::clone(&self.engine))
            .handle(
                ListAccessGrantsInput {
                    domain_id: domain_id.map(str::to_string),
                    space_id: space_id.map(str::to_string),
                    ..Default::default()
                },
                &self.ctx(),
            )
            .await
    }

    async fn get_space(&self, space_id: &str) -> Result<GetSpaceOutput, GetSpaceError> {
        GetSpace::new(Arc::clone(&self.engine))
            .handle(
                GetSpaceInput {
                    space_id: space_id.to_string(),
                },
                &self.ctx(),
            )
            .await
    }

    /// A patch of `space_id` that asks for nothing, for the same reason as
    /// [`Omni::update_domain`].
    async fn update_space(&self, space_id: &str) -> Result<UpdateSpaceOutput, UpdateSpaceError> {
        UpdateSpace::new(Arc::clone(&self.engine))
            .handle(
                UpdateSpaceInput {
                    space_id: space_id.to_string(),
                    ..Default::default()
                },
                &self.ctx(),
            )
            .await
    }

    /// A create that names `space_id` as the parent of the profile it would mint.
    async fn create_profile_in(
        &self,
        space_id: &str,
        name: &str,
    ) -> Result<AccessProfile, CreateAccessProfileError> {
        CreateAccessProfile::new(Arc::clone(&self.engine))
            .handle(
                CreateAccessProfileInput {
                    space_id: space_id.to_string(),
                    name: name.to_string(),
                    ..Default::default()
                },
                &self.ctx(),
            )
            .await
            .map(|output| output.access_profile)
    }

    /// A dashboard listing scoped to `space_id`.
    async fn list_dashboards(
        &self,
        space_id: &str,
    ) -> Result<ListOmniDashboardsOutput, ListOmniDashboardsError> {
        ListOmniDashboards::new(Arc::clone(&self.engine))
            .handle(
                ListOmniDashboardsInput {
                    space_id: space_id.to_string(),
                    ..Default::default()
                },
                &self.ctx(),
            )
            .await
    }

    /// A grant of `space_id` in `domain_id`, through the handler that owns it.
    async fn create_grant(
        &self,
        domain_id: &str,
        space_id: &str,
        name: &str,
    ) -> Result<AccessGrant, CreateAccessGrantError> {
        CreateAccessGrant::new(Arc::clone(&self.engine))
            .handle(
                CreateAccessGrantInput {
                    domain_id: domain_id.to_string(),
                    space_id: space_id.to_string(),
                    name: name.to_string(),
                    permission: "READ".to_string(),
                    principal: AccessGrantPrincipal {
                        principal_type: "IAM_ROLE".to_string(),
                        principal_id: Some("role/OmniReader".to_string()),
                        principal_attributes: None,
                    },
                    ..Default::default()
                },
                &self.ctx(),
            )
            .await
            .map(|output| output.access_grant)
    }

    async fn get_grant(
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

    async fn get_alert(
        &self,
        space_id: &str,
        alert_id: &str,
    ) -> Result<GetAlertOutput, GetAlertError> {
        GetAlert::new(Arc::clone(&self.engine))
            .handle(
                GetAlertInput {
                    space_id: space_id.to_string(),
                    alert_id: alert_id.to_string(),
                },
                &self.ctx(),
            )
            .await
    }
}

/// GUARD on "one text per kind": the wording every handler reaches for is the one the
/// tables below require, so a second copy of it that drifted fails here rather than in
/// whichever operation happened to be asserted.
#[test]
fn every_kind_is_worded_in_exactly_one_place() {
    for (kind, shared, required) in [
        ("a domain", DOMAIN_NOT_FOUND_MESSAGE, DOMAIN_MISS),
        ("a space", SPACE_NOT_FOUND_MESSAGE, SPACE_MISS),
        (
            "an access profile",
            ACCESS_PROFILE_NOT_FOUND_MESSAGE,
            ACCESS_PROFILE_MISS,
        ),
        (
            "an access grant",
            ACCESS_GRANT_NOT_FOUND_MESSAGE,
            ACCESS_GRANT_MISS,
        ),
    ] {
        assert_eq!(
            shared, required,
            "case {kind:?}: expected the shared wording {required:?}, got {shared:?}"
        );
    }
}

#[tokio::test]
async fn a_domain_the_store_does_not_hold_is_one_text_whichever_operation_reports_it() {
    let omni = Omni::new();
    // A live domain and a live space, so every refusal below is a lookup that missed
    // rather than a store with nothing in it.
    let live_domain = omni.domain("domain-miss").await;
    let live_space = omni.space("domain-miss", "domain-miss-space").await;

    // Rows span the `DomainId` class (`^d-[0-9a-z]{1,25}$`): the shortest id the pattern
    // admits, one of mixed digits, and the longest it admits — the text is the same fixed
    // sentence whatever the request carried.
    for (case, domain_id) in [
        ("the shortest DomainId the pattern admits", "d-0".to_string()),
        (
            "a DomainId of mixed digits",
            "d-3f9q2xkb70wzna15cdt8r".to_string(),
        ),
        (
            "the longest DomainId the pattern admits",
            format!("d-{}", "z".repeat(25)),
        ),
    ] {
        let read = refusal_message!(
            case,
            omni.get_domain(&domain_id).await,
            GetDomainErrorModeled::ResourceNotFoundException,
            NOT_FOUND.0,
            NOT_FOUND.1
        );
        let patched = refusal_message!(
            case,
            omni.update_domain(&domain_id).await,
            UpdateDomainErrorModeled::ResourceNotFoundException,
            NOT_FOUND.0,
            NOT_FOUND.1
        );
        let created = refusal_message!(
            case,
            omni.create_space_in(&domain_id, "orphan-space").await,
            CreateSpaceErrorModeled::ResourceNotFoundException,
            NOT_FOUND.0,
            NOT_FOUND.1
        );
        // The listings take `domainId` as a FILTER rather than a resource they address,
        // so they report no miss at all: a value nothing matches is an empty page.
        let spaces = omni.list_spaces_in(&domain_id).await;
        assert_eq!(
            spaces.as_ref().map(|page| page.items.len()).ok(),
            Some(0),
            "case {case:?}: ListSpaces expected an empty page, got {spaces:?}"
        );
        let grants = omni.list_grants(Some(&domain_id), None).await;
        assert_eq!(
            grants.as_ref().map(|page| page.items.len()).ok(),
            Some(0),
            "case {case:?}: ListAccessGrants expected an empty page, got {grants:?}"
        );
        for (operation, actual) in [
            ("GetDomain", read),
            ("UpdateDomain", patched),
            ("CreateSpace", created),
        ] {
            assert_eq!(
                actual, DOMAIN_MISS,
                "case {case:?}: {operation} expected {DOMAIN_MISS:?}, got {actual:?}"
            );
            assert!(
                !actual.contains(&domain_id),
                "case {case:?}: {operation} disclosed the requested domainId: {actual:?}"
            );
        }
    }

    // Nothing above made the live records unreadable: the domain and its space still
    // answer, so the refusals are what the lookup found and not a blanket answer.
    let held = omni
        .get_domain(&live_domain)
        .await
        .unwrap_or_else(|error| panic!("the live domain stopped resolving: {error:?}"));
    assert_eq!(
        held.domain.domain_id, live_domain,
        "expected the live domain back, got {held:?}"
    );
    let listed = omni
        .list_spaces_in(&live_domain)
        .await
        .unwrap_or_else(|error| panic!("the live domain's spaces stopped listing: {error:?}"));
    assert!(
        listed
            .items
            .iter()
            .any(|summary| summary.space_id == live_space),
        "expected the live space listed under its domain, got {listed:?}"
    );
}

#[tokio::test]
async fn a_space_the_store_does_not_hold_is_one_text_whichever_operation_reports_it() {
    let omni = Omni::new();
    let live_domain = omni.domain("space-miss").await;
    let live_space = omni.space("space-miss", "space-miss-live").await;

    // Rows span the `SpaceId` class (its UUID pattern): an id of nil digits, one of mixed
    // digits, and one at the top of the alphabet the pattern admits.
    for (case, space_id) in [
        ("an id of nil digits", MISSING_SPACE_ID.to_string()),
        (
            "an id of mixed digits",
            "2b7d41e0-9c3a-4f55-8a16-0d4e7b6c9a12".to_string(),
        ),
        (
            "an id at the top of the pattern's alphabet",
            "ffffffff-ffff-4fff-bfff-ffffffffffff".to_string(),
        ),
    ] {
        // The operations that address the SPACE, or a resource its access control
        // governs, decide authorization from the account's space records first: an id the
        // account holds no space under authorizes nobody, so these report the
        // authorization refusal rather than a miss, and that refusal is one text too.
        let read = refusal_message!(
            case,
            omni.get_space(&space_id).await,
            GetSpaceErrorModeled::AccessDeniedException,
            "AccessDeniedException",
            403
        );
        let patched = refusal_message!(
            case,
            omni.update_space(&space_id).await,
            UpdateSpaceErrorModeled::AccessDeniedException,
            "AccessDeniedException",
            403
        );
        let profile = refusal_message!(
            case,
            omni.create_profile_in(&space_id, "orphan profile").await,
            CreateAccessProfileErrorModeled::AccessDeniedException,
            "AccessDeniedException",
            403
        );
        for (operation, actual) in [
            ("GetSpace", read),
            ("UpdateSpace", patched),
            ("CreateAccessProfile", profile),
        ] {
            assert_eq!(
                actual, AUTHORIZATION_FAILED,
                "case {case:?}: {operation} expected {AUTHORIZATION_FAILED:?}, got {actual:?}"
            );
            for disclosed in [space_id.as_str(), live_domain.as_str()] {
                assert!(
                    !actual.contains(disclosed),
                    "case {case:?}: {operation} disclosed {disclosed:?}: {actual:?}"
                );
            }
        }

        // The operations that report the COLLECTION a space holds, and the grant create
        // that names a space as a parent, are not gated on the space: they report the
        // space that is missing, in one text.
        let dashboards = refusal_message!(
            case,
            omni.list_dashboards(&space_id).await,
            ListOmniDashboardsErrorModeled::ResourceNotFoundException,
            NOT_FOUND.0,
            NOT_FOUND.1
        );
        // `ListAccessGrants` takes `spaceId` as a FILTER rather than a resource it
        // addresses, so a value nothing matches is an empty page, not a miss.
        let listed = omni.list_grants(None, Some(&space_id)).await;
        assert_eq!(
            listed.as_ref().map(|page| page.items.len()).ok(),
            Some(0),
            "case {case:?}: ListAccessGrants expected an empty page, got {listed:?}"
        );
        // A create whose space is missing UNDER a domain that exists reports the space
        // that is missing, and says nothing of the domain that is not.
        let grant = refusal_message!(
            case,
            omni.create_grant(&live_domain, &space_id, "orphan-grant")
                .await,
            CreateAccessGrantErrorModeled::ResourceNotFoundException,
            NOT_FOUND.0,
            NOT_FOUND.1
        );
        for (operation, actual) in [
            ("ListOmniDashboards", dashboards),
            ("CreateAccessGrant", grant),
        ] {
            assert_eq!(
                actual, SPACE_MISS,
                "case {case:?}: {operation} expected {SPACE_MISS:?}, got {actual:?}"
            );
            for disclosed in [space_id.as_str(), live_domain.as_str()] {
                assert!(
                    !actual.contains(disclosed),
                    "case {case:?}: {operation} disclosed {disclosed:?}: {actual:?}"
                );
            }
        }
    }

    // The live space still answers, so the refusals above are decisions and lookups about
    // the id each request carried, not a blanket answer.
    let held = omni
        .get_space(&live_space)
        .await
        .unwrap_or_else(|error| panic!("the live space stopped resolving: {error:?}"));
    assert_eq!(
        held.space.space_id, live_space,
        "expected the live space back, got {held:?}"
    );
}

#[tokio::test]
async fn an_access_profile_the_store_does_not_hold_is_one_text_whichever_operation_reports_it() {
    let omni = Omni::new();
    let space_id = omni.space("profile-miss", "profile-miss-space").await;
    let other_space = omni.space("profile-miss", "profile-miss-other").await;
    let elsewhere = omni
        .create_profile("profile-miss", &other_space, "profile elsewhere", None)
        .await;

    // Rows span the `ProfileId` class (`^[a-zA-Z0-9_-]+$`) and, as the last row, a LIVE
    // profile addressed under a space that does not hold it: a profile is addressed by its
    // space AND its id, so that row is a miss too — and the text names neither.
    for (case, profile_id) in [
        ("an id the store holds nothing under", MISSING_PROFILE_ID.to_string()),
        (
            "an id carrying the punctuation its pattern admits",
            "Profile_9-X".to_string(),
        ),
        (
            "a live profile addressed under another space",
            elsewhere.profile_id.clone(),
        ),
    ] {
        let read = refusal_message!(
            case,
            omni.get_profile(&space_id, &profile_id).await,
            GetAccessProfileErrorModeled::ResourceNotFoundException,
            NOT_FOUND.0,
            NOT_FOUND.1
        );
        let patched = refusal_message!(
            case,
            omni.update_profile(&UpdateAccessProfileInput {
                space_id: space_id.clone(),
                profile_id: profile_id.clone(),
                name: Some("renamed".to_string()),
                description: Some("second".to_string()),
            })
            .await,
            UpdateAccessProfileErrorModeled::ResourceNotFoundException,
            NOT_FOUND.0,
            NOT_FOUND.1
        );
        for (operation, actual) in [("GetAccessProfile", read), ("UpdateAccessProfile", patched)] {
            assert_eq!(
                actual, ACCESS_PROFILE_MISS,
                "case {case:?}: {operation} expected {ACCESS_PROFILE_MISS:?}, got {actual:?}"
            );
            for disclosed in [profile_id.as_str(), space_id.as_str()] {
                assert!(
                    !actual.contains(disclosed),
                    "case {case:?}: {operation} disclosed {disclosed:?}: {actual:?}"
                );
            }
        }
    }

    // The profile the other space DOES hold is untouched by the refusals above, and still
    // carries what it was created with.
    let stored = omni
        .stored_profile("profile-miss", &other_space, &elsewhere.profile_id)
        .await;
    assert_eq!(
        (stored.name.as_str(), stored.description.as_deref()),
        ("profile elsewhere", None),
        "a refused patch changed the profile it could not find: {stored:?}"
    );
}

#[tokio::test]
async fn an_access_grant_the_store_does_not_hold_is_named_by_its_kind_alone() {
    let omni = Omni::new();
    let domain_id = omni.domain("grant-miss").await;
    let space_id = omni.space("grant-miss", "grant-miss-space").await;
    let live = omni
        .create_grant(&domain_id, &space_id, "grant-live")
        .await
        .unwrap_or_else(|error| panic!("CreateAccessGrant failed: {error:?}"));

    // Rows span the `GrantId` class (its UUID pattern), as the space rows do: a grant is
    // addressed by its id alone, so the miss is the same text for each of them.
    for (case, grant_id) in [
        ("an id of nil digits", "00000000-0000-4000-8000-000000000000"),
        ("an id of mixed digits", "496d9e19-c704-4a21-9b65-0f3ad28e5c17"),
        (
            "an id at the top of the pattern's alphabet",
            "ffffffff-ffff-4fff-bfff-ffffffffffff",
        ),
    ] {
        let read = refusal_message!(
            case,
            omni.get_grant(grant_id).await,
            GetAccessGrantErrorModeled::ResourceNotFoundException,
            NOT_FOUND.0,
            NOT_FOUND.1
        );
        assert_eq!(
            read, ACCESS_GRANT_MISS,
            "case {case:?}: GetAccessGrant expected {ACCESS_GRANT_MISS:?}, got {read:?}"
        );
        assert!(
            !read.contains(grant_id),
            "case {case:?}: GetAccessGrant disclosed the requested grantId: {read:?}"
        );
    }

    // The grant the store DOES hold still reads back, so the refusals above are lookups
    // that missed rather than a reader that refuses everything.
    let held = omni
        .get_grant(&live.grant_id)
        .await
        .unwrap_or_else(|error| panic!("the live grant stopped resolving: {error:?}"));
    assert_eq!(
        (
            held.access_grant.grant_id.as_str(),
            held.access_grant.name.as_deref()
        ),
        (live.grant_id.as_str(), Some("grant-live")),
        "expected the live grant back, got {held:?}"
    );
}

#[tokio::test]
async fn an_alert_the_store_does_not_hold_is_named_with_the_space_it_was_looked_for_in() {
    let omni = Omni::new();
    let space_id = omni.space("alert-miss", "alert-miss-space").await;

    // Rows span the `AlertId` class (`^[0-9a-f]{32}$`) and the spaces an alert can be
    // looked for in: three ids under a LIVE space, and one under a space the store holds
    // nothing under either — both variables are taken from the REQUEST, so the last row
    // reports the space id it carried even though nothing is stored under it.
    for (case, space, alert_id) in [
        ("an alertId of nil digits", space_id.clone(), "0".repeat(32)),
        (
            "an alertId of mixed digits",
            space_id.clone(),
            "e3f90a17bc45d8206f1ae7cd39b4502f".to_string(),
        ),
        (
            "an alertId at the top of the pattern's alphabet",
            space_id.clone(),
            "f".repeat(32),
        ),
        (
            "an alertId under a space the store does not hold",
            MISSING_SPACE_ID.to_string(),
            "0123456789abcdef0123456789abcdef".to_string(),
        ),
    ] {
        let expected = alert_miss(&alert_id, &space);
        let read = refusal_message!(
            case,
            omni.get_alert(&space, &alert_id).await,
            GetAlertErrorModeled::ResourceNotFoundException,
            NOT_FOUND.0,
            NOT_FOUND.1
        );
        assert_eq!(
            read, expected,
            "case {case:?}: GetAlert expected {expected:?}, got {read:?}"
        );
        assert!(
            !read.ends_with('.'),
            "case {case:?}: GetAlert closed the message with a period: {read:?}"
        );
    }
}

