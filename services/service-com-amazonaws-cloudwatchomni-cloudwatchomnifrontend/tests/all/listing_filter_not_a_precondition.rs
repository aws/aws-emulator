// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for what an OPTIONAL filter on a CloudWatchOmni listing does.
//!
//! `ListAccessGrants` and `ListSpaces` enumerate a collection of the current account and
//! Region; every input member but `nextToken` and `maxResults` is documented as a filter
//! ("Filter by space ID.", "Filter by domain ID.", …) and the output's `items` is
//! `@required`. So a filter NARROWS the listing: a filter value the store matches no
//! record for — one whose resource was deleted, or one it never held — answers HTTP 200
//! with `items: []` and no `nextToken`, never a miss. The modelled
//! `ResourceNotFoundException` is for a resource a request ADDRESSES, which these
//! listings never do; the siblings whose `spaceId` is `@required`
//! (`ListAccessProfiles`, `ListOmniDashboards`) do address one, and keep refusing.
//!
//! The suite asserts that over a TABLE whose rows span what the rule generalises over:
//! which member carries the filter, whether its value names a resource that was deleted
//! or one that never existed, an id filter against an enumerated one, a filter alone and
//! in conjunction with a live one, and the page bound the request asks for. Live filters
//! are rows of the same tables, so a filter is shown to filter rather than to be ignored.
//!
//! Every call goes through a real operation handler holding the SAME shared engine the
//! router hands every service, and each assertion names its case and the actual value.

use std::sync::Arc;

use aws_smithy_schema::codec::{Codec, FinishSerializer};
use aws_smithy_schema::serde::{SerializableStruct, ShapeSerializer};
use aws_smithy_schema::Schema;
use aws_smithy_schema_server::{CborCodec, DeserializableStruct};
use smithy_server_runtime::{
    EmbeddedEmulator, MultiServiceRouter, Operation, OperationContext, OperationError, Service,
};

use service_com_amazonaws_cloudwatchomni_cloudwatchomnifrontend as omni;

use omni::ops::create_access_grant::CreateAccessGrant;
use omni::ops::create_domain::CreateDomain;
use omni::ops::create_space::CreateSpace;
use omni::ops::delete_domain::DeleteDomain;
use omni::ops::delete_space::DeleteSpace;
use omni::ops::list_access_grants::{ListAccessGrants, ListAccessGrantsError};
use omni::ops::list_access_profiles::{ListAccessProfiles, ListAccessProfilesError};
use omni::ops::list_omni_dashboards::{ListOmniDashboards, ListOmniDashboardsError};
use omni::ops::list_spaces::{ListSpaces, ListSpacesError};
use omni::ops::types::{
    AccessGrantPrincipal, CreateAccessGrantInput, CreateDomainInput, CreateSpaceInput,
    DeleteDomainInput, DeleteSpaceInput, ListAccessGrantsInput, ListAccessGrantsOutput,
    ListAccessProfilesInput, ListAccessProfilesOutput, ListOmniDashboardsInput,
    ListOmniDashboardsOutput, ListSpacesInput, ListSpacesOutput, LIST_ACCESS_GRANTS_INPUT_SCHEMA,
    LIST_SPACES_INPUT_SCHEMA,
};

/// The Regions a space create is signed for, from the suite that owns the space rule: an
/// account holds at most one space per Region.
use super::domain_space_lifecycle::SPACE_REGIONS;

/// A role ARN satisfying `Arn`'s `^arn:.+$`, which `CreateSpace` requires.
const DATA_ACCESS_ROLE_ARN: &str = "arn:aws:iam::000000000000:role/OmniData";

/// A well-formed `SpaceId` (its UUID pattern) the store never mints a space under.
const UNHELD_SPACE_ID: &str = "00000000-0000-4000-8000-000000000000";

/// A well-formed `DomainId` (`^d-[0-9a-z]{1,25}$`) the store never mints a domain under.
const UNHELD_DOMAIN_ID: &str = "d-0000000000";

/// The request metadata a call signed for `region` carries.
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

/// One emulator: one shared engine and the service mounted on a router.
struct Omni {
    engine: Arc<engine::InMemoryEngine>,
    service: omni::EmulatedService<engine::InMemoryEngine>,
    router: MultiServiceRouter,
    parts: http::request::Parts,
    /// How many spaces this emulator has created, which picks each create's Region out of
    /// [`SPACE_REGIONS`].
    spaces_created: std::sync::atomic::AtomicUsize,
}

impl Omni {
    fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        router.set_request_authentication(false);
        router.register_service(omni::EmulatedService::new(Arc::clone(&engine)));
        Self {
            service: omni::EmulatedService::new(Arc::clone(&engine)),
            engine,
            router,
            parts: signed_request_parts_in(SPACE_REGIONS[0]),
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

    /// The account's one domain.
    async fn domain(&self, case: &str, name: &str) -> String {
        CreateDomain::new(Arc::clone(&self.engine))
            .handle(
                CreateDomainInput {
                    name: name.to_string(),
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

    async fn delete_domain(&self, case: &str, domain_id: &str) {
        DeleteDomain::new(Arc::clone(&self.engine))
            .handle(
                DeleteDomainInput {
                    domain_id: domain_id.to_string(),
                },
                &self.ctx(),
            )
            .await
            .unwrap_or_else(|error| panic!("case {case:?}: DeleteDomain failed: {error:?}"));
    }

    /// A space of `domain_id`, created in a Region of its own.
    async fn space(&self, case: &str, domain_id: &str, name: &str) -> String {
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

    /// A customer-managed grant of `space_id`, so a space holds a row of its own beside
    /// the ones its create provisioned.
    async fn grant(&self, case: &str, domain_id: &str, space_id: &str, name: &str) -> String {
        CreateAccessGrant::new(Arc::clone(&self.engine))
            .handle(
                CreateAccessGrantInput {
                    domain_id: domain_id.to_string(),
                    space_id: space_id.to_string(),
                    name: name.to_string(),
                    permission: "READ".to_string(),
                    principal: AccessGrantPrincipal {
                        principal_type: "IAM_ROLE".to_string(),
                        principal_id: Some("arn:aws:iam::000000000000:role/Reader".to_string()),
                        principal_attributes: None,
                    },
                    ..Default::default()
                },
                &self.ctx(),
            )
            .await
            .unwrap_or_else(|error| panic!("case {case:?}: CreateAccessGrant failed: {error:?}"))
            .access_grant
            .grant_id
    }

    async fn list_grants(
        &self,
        input: &ListAccessGrantsInput,
    ) -> Result<ListAccessGrantsOutput, ListAccessGrantsError> {
        ListAccessGrants::new(Arc::clone(&self.engine))
            .handle(input.clone(), &self.ctx())
            .await
    }

    async fn list_spaces(
        &self,
        input: &ListSpacesInput,
    ) -> Result<ListSpacesOutput, ListSpacesError> {
        ListSpaces::new(Arc::clone(&self.engine))
            .handle(input.clone(), &self.ctx())
            .await
    }

    async fn list_profiles(
        &self,
        space_id: &str,
    ) -> Result<ListAccessProfilesOutput, ListAccessProfilesError> {
        ListAccessProfiles::new(Arc::clone(&self.engine))
            .handle(
                ListAccessProfilesInput {
                    space_id: space_id.to_string(),
                    next_token: None,
                    max_results: None,
                },
                &self.ctx(),
            )
            .await
    }

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

/// The grant ids a `ListAccessGrants` request reports, walking every page it hands back,
/// with the token of the LAST page — which must be absent.
async fn walk_grants(omni: &Omni, case: &str, input: &ListAccessGrantsInput) -> Vec<String> {
    let mut ids: Vec<String> = Vec::new();
    let mut token: Option<String> = None;
    loop {
        let page = omni
            .list_grants(&ListAccessGrantsInput {
                next_token: token.clone(),
                ..input.clone()
            })
            .await
            .unwrap_or_else(|error| panic!("case {case:?}: ListAccessGrants failed: {error:?}"));
        ids.extend(page.items.iter().map(|item| item.grant_id.clone()));
        token = page.next_token;
        if token.is_none() {
            return ids;
        }
        assert!(
            ids.len() < 64,
            "case {case:?}: the paginator ran past the collection with {ids:?}"
        );
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
        Err(error) => assert_eq!(
            (error.shape_name(), error.http_status()),
            (expected_code, expected_status),
            "case {case:?}: expected {expected_code} ({expected_status}), got {error:?}"
        ),
    }
}

/// The store every case of the grant table reads: one live domain holding one live space,
/// a space that was deleted, and a domain that was deleted.
struct Store {
    omni: Omni,
    live_domain: String,
    live_space: String,
    deleted_space: String,
    deleted_domain: String,
    /// The grants the live space holds, in creation order: the two its create provisioned
    /// and the customer-managed one added after them.
    live_grants: Vec<String>,
}

async fn store() -> Store {
    let omni = Omni::new();
    // A domain with nothing under it, deleted: its id is well-formed and names nothing.
    let deleted_domain = omni.domain("a deleted domain", "omni-filter-gone").await;
    omni.delete_domain("a deleted domain", &deleted_domain).await;

    let live_domain = omni.domain("the live domain", "omni-filter-live").await;
    let live_space = omni
        .space("the live space", &live_domain, "filter-live-space")
        .await;
    let deleted_space = omni
        .space("the deleted space", &live_domain, "filter-gone-space")
        .await;
    omni.grant("the live space", &live_domain, &live_space, "live-reader")
        .await;
    // The space that goes holds only the access control its create provisioned, which
    // the delete releases, so every row below is about the FILTER rather than about what
    // a delete takes with it (which `access_grant_lifecycle` owns).
    omni.delete_space("the deleted space", &deleted_space).await;

    let live_grants = walk_grants(
        &omni,
        "the live space's grants",
        &ListAccessGrantsInput {
            space_id: Some(live_space.clone()),
            ..Default::default()
        },
    )
    .await;
    assert_eq!(
        live_grants.len(),
        3,
        "the live space should hold its two provisioned grants and the one added, got \
         {live_grants:?}"
    );

    Store {
        omni,
        live_domain,
        live_space,
        deleted_space,
        deleted_domain,
        live_grants,
    }
}

#[tokio::test]
async fn a_grant_filter_the_store_matches_nothing_for_is_an_empty_page() {
    let store = store().await;
    let omni = &store.omni;

    // Rows span the filter class: which member carries it, whether its value named a
    // resource that was DELETED or one the store NEVER held, an id filter against an
    // enumerated one, and a dead filter in conjunction with a live one. The expected
    // listing is written out for every row, so a filter that was ignored fails too.
    let rows: Vec<(&str, ListAccessGrantsInput, Vec<String>)> = vec![
        (
            "spaceId naming a space that was deleted",
            ListAccessGrantsInput {
                space_id: Some(store.deleted_space.clone()),
                ..Default::default()
            },
            Vec::new(),
        ),
        (
            "spaceId naming a space the store never held",
            ListAccessGrantsInput {
                space_id: Some(UNHELD_SPACE_ID.to_string()),
                ..Default::default()
            },
            Vec::new(),
        ),
        (
            "domainId naming a domain that was deleted",
            ListAccessGrantsInput {
                domain_id: Some(store.deleted_domain.clone()),
                ..Default::default()
            },
            Vec::new(),
        ),
        (
            "domainId naming a domain the store never held",
            ListAccessGrantsInput {
                domain_id: Some(UNHELD_DOMAIN_ID.to_string()),
                ..Default::default()
            },
            Vec::new(),
        ),
        (
            "principalId no grant carries",
            ListAccessGrantsInput {
                principal_id: Some("arn:aws:iam::000000000000:role/NobodyHere".to_string()),
                ..Default::default()
            },
            Vec::new(),
        ),
        (
            // `AccessGrantPrincipalType` declares IDC_USER
            // (`smithy_query(query="shape",shape="AccessGrantPrincipalType")`), and no
            // grant of this store carries it.
            "principalType the model declares and no grant carries",
            ListAccessGrantsInput {
                principal_type: Some("IDC_USER".to_string()),
                ..Default::default()
            },
            Vec::new(),
        ),
        (
            // `AccessGrantPermission` declares READ_WRITE_DELETE
            // (`smithy_query(query="shape",shape="AccessGrantPermission")`), and no grant
            // of this store carries it.
            "permission the model declares and no grant carries",
            ListAccessGrantsInput {
                permission: Some("READ_WRITE_DELETE".to_string()),
                ..Default::default()
            },
            Vec::new(),
        ),
        (
            "a live spaceId conjoined with a domainId that was deleted",
            ListAccessGrantsInput {
                space_id: Some(store.live_space.clone()),
                domain_id: Some(store.deleted_domain.clone()),
                ..Default::default()
            },
            Vec::new(),
        ),
        (
            "the live spaceId",
            ListAccessGrantsInput {
                space_id: Some(store.live_space.clone()),
                ..Default::default()
            },
            store.live_grants.clone(),
        ),
        (
            "the live domainId",
            ListAccessGrantsInput {
                domain_id: Some(store.live_domain.clone()),
                ..Default::default()
            },
            store.live_grants.clone(),
        ),
        (
            "no filter at all",
            ListAccessGrantsInput::default(),
            store.live_grants.clone(),
        ),
    ];

    for (case, input, expected) in rows {
        // The single page the request answers: `items` is what matched, and `nextToken`
        // is absent because nothing remains beyond it.
        let page = omni
            .list_grants(&input)
            .await
            .unwrap_or_else(|error| panic!("case {case:?}: ListAccessGrants failed: {error:?}"));
        let actual: Vec<String> = page.items.iter().map(|item| item.grant_id.clone()).collect();
        assert_eq!(
            (actual.clone(), page.next_token.clone()),
            (expected.clone(), None),
            "case {case:?}: expected {expected:?} and no token, got {actual:?} and {:?}",
            page.next_token
        );

        // The same answer under every page bound a paginator would ask for: the modelled
        // minimum, one in between and the modelled maximum
        // (`maxResults` carries @range(min: 1, max: 100)).
        for bound in [1, 2, 100] {
            let walked = walk_grants(
                omni,
                &format!("{case} at maxResults={bound}"),
                &ListAccessGrantsInput {
                    max_results: Some(bound),
                    ..input.clone()
                },
            )
            .await;
            assert_eq!(
                walked, expected,
                "case {case:?} at maxResults={bound}: expected {expected:?}, got {walked:?}"
            );
        }
    }

    // A listing is a read: the empty pages above changed nothing, so the live rows are
    // still there and still reported in the same order.
    let after = walk_grants(omni, "after the empty pages", &ListAccessGrantsInput::default()).await;
    assert_eq!(
        after, store.live_grants,
        "the listings must change nothing, the account now reports {after:?}"
    );
}

#[tokio::test]
async fn a_space_filter_the_store_matches_nothing_for_is_an_empty_page() {
    let store = store().await;
    let omni = &store.omni;

    // `ListSpaces` carries the same rule on its one filter: rows span a domain that was
    // deleted, one the store never held, and the live one.
    let rows: Vec<(&str, Option<String>, Vec<String>)> = vec![
        (
            "domainId naming a domain that was deleted",
            Some(store.deleted_domain.clone()),
            Vec::new(),
        ),
        (
            "domainId naming a domain the store never held",
            Some(UNHELD_DOMAIN_ID.to_string()),
            Vec::new(),
        ),
        (
            "the live domainId",
            Some(store.live_domain.clone()),
            vec![store.live_space.clone()],
        ),
        ("no filter at all", None, vec![store.live_space.clone()]),
    ];

    for (case, domain_id, expected) in rows {
        for bound in [None, Some(1), Some(100)] {
            let page = omni
                .list_spaces(&ListSpacesInput {
                    domain_id: domain_id.clone(),
                    next_token: None,
                    max_results: bound,
                })
                .await
                .unwrap_or_else(|error| {
                    panic!("case {case:?} at maxResults={bound:?}: ListSpaces failed: {error:?}")
                });
            let actual: Vec<String> = page
                .items
                .iter()
                .map(|item| item.space_id.clone())
                .collect();
            assert_eq!(
                (actual.clone(), page.next_token.clone()),
                (expected.clone(), None),
                "case {case:?} at maxResults={bound:?}: expected {expected:?} and no token, got \
                 {actual:?} and {:?}",
                page.next_token
            );
        }
    }
}

#[tokio::test]
async fn a_listing_that_addresses_a_space_still_refuses_one_the_store_does_not_hold() {
    let store = store().await;
    let omni = &store.omni;

    // The rule is about FILTERS. `ListAccessProfiles` and `ListOmniDashboards` carry
    // `spaceId` as `@required`
    // (`smithy_query(query="operation",operation="ListAccessProfiles,ListOmniDashboards")`):
    // the request ADDRESSES that space, so a space the store does not hold is the
    // modelled miss there. Rows span a space that was deleted and one never held.
    for (case, space_id) in [
        ("a space that was deleted", store.deleted_space.as_str()),
        ("a space the store never held", UNHELD_SPACE_ID),
    ] {
        expect_modelled_error(
            &format!("ListAccessProfiles for {case}"),
            omni.list_profiles(space_id).await,
            404,
            "ResourceNotFoundException",
        );
        expect_modelled_error(
            &format!("ListOmniDashboards for {case}"),
            omni.list_dashboards(space_id).await,
            404,
            "ResourceNotFoundException",
        );
    }

    // The live space still answers both, so the refusals are about the id each request
    // carried.
    omni.list_profiles(&store.live_space)
        .await
        .expect("the live space's access profiles list");
    omni.list_dashboards(&store.live_space)
        .await
        .expect("the live space's dashboards list");
}

#[tokio::test]
async fn an_empty_filtered_page_is_a_200_over_the_wire() {
    let store = store().await;
    let omni = &store.omni;
    let codec = CborCodec::default();

    // The same two requests a client makes after a delete: the status and the body are
    // what it sees, so they are checked over the protocol rather than through the handler.
    let (status, body) = omni
        .over_the_wire(
            "ListAccessGrants",
            LIST_ACCESS_GRANTS_INPUT_SCHEMA,
            &ListAccessGrantsInput {
                space_id: Some(store.deleted_space.clone()),
                ..Default::default()
            },
        )
        .await;
    assert_eq!(
        status,
        200,
        "ListAccessGrants filtered by a deleted space answered {status}: {:?}",
        String::from_utf8_lossy(&body)
    );
    let page = {
        let mut deserializer = codec.create_deserializer(&body);
        ListAccessGrantsOutput::deserialize(&mut deserializer).expect("the output deserializes")
    };
    assert!(
        page.items.is_empty() && page.next_token.is_none(),
        "the wire page should be empty with no token, got {page:?}"
    );

    let (status, body) = omni
        .over_the_wire(
            "ListSpaces",
            LIST_SPACES_INPUT_SCHEMA,
            &ListSpacesInput {
                domain_id: Some(store.deleted_domain.clone()),
                next_token: None,
                max_results: None,
            },
        )
        .await;
    assert_eq!(
        status,
        200,
        "ListSpaces filtered by a deleted domain answered {status}: {:?}",
        String::from_utf8_lossy(&body)
    );
    let page = {
        let mut deserializer = codec.create_deserializer(&body);
        ListSpacesOutput::deserialize(&mut deserializer).expect("the output deserializes")
    };
    assert!(
        page.items.is_empty() && page.next_token.is_none(),
        "the wire page should be empty with no token, got {page:?}"
    );
}
