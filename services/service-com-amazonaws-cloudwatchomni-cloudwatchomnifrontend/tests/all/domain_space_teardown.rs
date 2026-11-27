// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for the TEARDOWN half of CloudWatchOmni's domain -> space estate:
//! every create, delete and read answers from the LIVE store.
//!
//! One rule, asserted over a table rather than over one recorded exchange: a create is
//! refused while the account's Region already holds a space and writes nothing, a delete
//! releases the deleted resource from every set the store counts it in, and a resource the
//! store no longer holds is unreadable. Together that is the documented teardown order
//! (delete the spaces, then the domain, then confirm it is gone), so the rows differ in
//! how much there is to tear down and in which order it goes:
//!
//! * while the account's Region holds a space, every further `CreateSpace` signed for that
//!   Region is the modelled `ConflictException` (409) naming only the caller's account and
//!   the request's Region — and the refusal mints nothing, so the domain holds exactly the
//!   spaces that were created;
//! * while a domain still holds spaces `DeleteDomain` is the modelled `ConflictException`
//!   (409) whose `conflictType` is NOT_EMPTY and whose message states the number of spaces
//!   the store holds for it AT THAT MOMENT — the count drops as the spaces go, which is
//!   what shows the guard reads the live set rather than a tally kept beside it;
//! * once the last space is deleted the delete goes through: `DeleteDomainOutput` has no
//!   members, so the answer is the modelled empty output;
//! * a deleted domain is unreadable: `GetDomain` answers the modelled
//!   `ResourceNotFoundException` (404) with its wholly fixed message, and serves neither
//!   the record nor the endpoint URLs it used to announce;
//! * `DeleteDomain` is `@idempotent`, so repeating it on the domain that is already gone
//!   still succeeds, and the account is left free to hold a domain and a space again.
//!
//! [`the_teardown_order_over_the_rpcv2cbor_wire`] drives the same chain over HTTP through
//! the framework's [`MultiServiceRouter`], so the status codes and the bodies a client
//! actually reads are asserted too.
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

use omni::ops::create_domain::CreateDomain;
use omni::ops::create_space::{CreateSpace, CreateSpaceError, CreateSpaceErrorModeled};
use omni::ops::delete_domain::{DeleteDomain, DeleteDomainError, DeleteDomainErrorModeled};
use omni::ops::delete_space::DeleteSpace;
use omni::ops::get_domain::{GetDomain, GetDomainError, GetDomainErrorModeled};
use omni::ops::list_spaces::ListSpaces;
use omni::ops::types::{
    ConflictException, CreateDomainInput, CreateSpaceInput, DeleteDomainInput, DeleteDomainOutput,
    DeleteSpaceInput, GetDomainInput, GetDomainOutput, ListSpacesInput, Space,
    CREATE_SPACE_INPUT_SCHEMA, DELETE_DOMAIN_INPUT_SCHEMA, DELETE_SPACE_INPUT_SCHEMA,
    GET_DOMAIN_INPUT_SCHEMA,
};
use omni::scaffolded::Scaffolded;

/// The Regions a space create is signed for, from the suite that owns the space rule.
use super::domain_space_lifecycle::SPACE_REGIONS;

/// The members a wire body carries at its top level, from the suite that owns the decoding.
use super::domain_listing_wire::top_level_members;

/// The `__type` and `message` of an rpcv2Cbor error envelope, from the suite that owns the
/// decoding.
use super::validation_message_wording::error_envelope;

/// AWS's message for a `domainId` the store holds no domain under: WHOLLY fixed text, with
/// no variable part — recorded behavior, the model fixes no text for
/// `ResourceNotFoundException`.
const DOMAIN_NOT_FOUND: &str = "Domain not found";

/// A role ARN satisfying `Arn`'s `^arn:.+$`, which `CreateSpace` requires.
const DATA_ACCESS_ROLE_ARN: &str = "arn:aws:iam::000000000000:role/OmniData";

/// AWS's message for a create refused because the account's Region already holds a space:
/// the fixed text with the caller's account and the request's Region as its only variables.
///
/// Written here rather than read from the crate, so the assertion is independent of the
/// implementation's own formatter.
fn space_already_exists(region: &str) -> String {
    format!(
        "A space already exists for account {} in region {region}",
        engine::EMULATOR_ACCOUNT_ID
    )
}

/// AWS's message for a `DeleteDomain` refused by the spaces the domain still holds: the
/// fixed text with the `domainId` the request addressed and the number of spaces the store
/// holds for it as its only variables.
///
/// Written here rather than read from the crate, so the assertion is independent of the
/// implementation's own formatter.
fn domain_holds_spaces(domain_id: &str, spaces: usize) -> String {
    format!(
        "Cannot delete Domain {domain_id} because it has {spaces} associated Spaces. Delete \
         all Spaces first."
    )
}

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
}

impl Omni {
    fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        router.set_request_authentication(false);
        // The router's service and the operations constructed below hold the SAME engine,
        // so both front doors read and write one store.
        router.register_service(omni::EmulatedService::new(Arc::clone(&engine)));
        Self {
            service: omni::EmulatedService::new(Arc::clone(&engine)),
            engine,
            router,
        }
    }

    /// The request metadata a handler reads, signed for `region`.
    ///
    /// The parts are held by the caller, because an `OperationContext` borrows them.
    fn ctx<'a>(&'a self, parts: &'a http::request::Parts) -> OperationContext<'a> {
        OperationContext::new(
            parts,
            self.service.definition().identity(),
            EmbeddedEmulator::from_router(self.router.clone()),
        )
    }

    /// Create a domain and answer its `domainId`.
    async fn domain(&self, case: &str, name: &str) -> String {
        let parts = signed_request_parts_in(SPACE_REGIONS[0]);
        CreateDomain::new(Arc::clone(&self.engine))
            .handle(
                CreateDomainInput {
                    name: name.to_string(),
                    identity_providers: vec!["IAM".to_string()],
                    client_token: Some(format!("token-{name}")),
                    ..Default::default()
                },
                &self.ctx(&parts),
            )
            .await
            .unwrap_or_else(|error| panic!("case {case:?}: CreateDomain failed: {error:?}"))
            .domain
            .domain_id
    }

    /// Ask for a space in `region`, and answer what the handler answered.
    async fn create_space(
        &self,
        name: &str,
        domain_id: &str,
        region: &str,
    ) -> Result<Space, CreateSpaceError> {
        let parts = signed_request_parts_in(region);
        CreateSpace::new(Arc::clone(&self.engine))
            .handle(
                CreateSpaceInput {
                    name: name.to_string(),
                    domain_id: domain_id.to_string(),
                    data_access_role_arn: DATA_ACCESS_ROLE_ARN.to_string(),
                    ..Default::default()
                },
                &self.ctx(&parts),
            )
            .await
            .map(|output| output.space)
    }

    /// Remove the space `space_id` names.
    async fn delete_space(&self, case: &str, space_id: &str) {
        let parts = signed_request_parts_in(SPACE_REGIONS[0]);
        DeleteSpace::new(Arc::clone(&self.engine))
            .handle(
                DeleteSpaceInput {
                    space_id: space_id.to_string(),
                },
                &self.ctx(&parts),
            )
            .await
            .unwrap_or_else(|error| panic!("case {case:?}: DeleteSpace failed: {error:?}"));
    }

    async fn delete_domain(
        &self,
        domain_id: &str,
    ) -> Result<DeleteDomainOutput, DeleteDomainError> {
        let parts = signed_request_parts_in(SPACE_REGIONS[0]);
        DeleteDomain::new(Arc::clone(&self.engine))
            .handle(
                DeleteDomainInput {
                    domain_id: domain_id.to_string(),
                },
                &self.ctx(&parts),
            )
            .await
    }

    async fn get_domain(&self, domain_id: &str) -> Result<GetDomainOutput, GetDomainError> {
        let parts = signed_request_parts_in(SPACE_REGIONS[0]);
        GetDomain::new(Arc::clone(&self.engine))
            .handle(
                GetDomainInput {
                    domain_id: domain_id.to_string(),
                },
                &self.ctx(&parts),
            )
            .await
    }

    /// The ids of the spaces the store still holds in `domain_id`, through the listing a
    /// caller enumerates them with.
    async fn live_space_ids(&self, case: &str, domain_id: &str) -> Vec<String> {
        let parts = signed_request_parts_in(SPACE_REGIONS[0]);
        let mut ids: Vec<String> = ListSpaces::new(Arc::clone(&self.engine))
            .handle(
                ListSpacesInput {
                    domain_id: Some(domain_id.to_string()),
                    ..Default::default()
                },
                &self.ctx(&parts),
            )
            .await
            .unwrap_or_else(|error| panic!("case {case:?}: ListSpaces failed: {error:?}"))
            .items
            .into_iter()
            .map(|item| item.space_id)
            .collect();
        ids.sort();
        ids
    }

    /// Dispatch one rpcv2Cbor request over HTTP and return the status and body.
    async fn over_the_wire<I: SerializableStruct>(
        &self,
        operation: &str,
        input_schema: &Schema,
        input: &I,
        region: &str,
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
        for (name, value) in &signed_request_parts_in(region).headers {
            builder = builder.header(name, value);
        }
        let request = builder
            .body(bytes::Bytes::from(serializer.finish()))
            .expect("a well-formed rpcv2Cbor request");
        let response = self.router.dispatch(request).await;
        (response.status().as_u16(), response.body().clone())
    }
}

/// The `ConflictException` a refused create carries, with the modelled code and status
/// checked first.
fn create_conflict(case: &str, result: Result<Space, CreateSpaceError>) -> ConflictException {
    match result {
        Ok(space) => panic!("case {case:?}: expected ConflictException (409), got Ok({space:?})"),
        Err(error) => {
            assert_eq!(
                (error.shape_name(), error.http_status()),
                ("ConflictException", 409),
                "case {case:?}: expected ConflictException (409), got {error:?}"
            );
            match error {
                Scaffolded::Modeled(CreateSpaceErrorModeled::ConflictException(conflict)) => {
                    conflict
                }
                other => {
                    panic!("case {case:?}: expected the modelled ConflictException, got {other:?}")
                }
            }
        }
    }
}

/// The `ConflictException` a refused `DeleteDomain` carries, with the modelled code and
/// status checked first.
fn delete_conflict(
    case: &str,
    result: Result<DeleteDomainOutput, DeleteDomainError>,
) -> ConflictException {
    match result {
        Ok(output) => {
            panic!("case {case:?}: expected ConflictException (409), got Ok({output:?})")
        }
        Err(error) => {
            assert_eq!(
                (error.shape_name(), error.http_status()),
                ("ConflictException", 409),
                "case {case:?}: expected ConflictException (409), got {error:?}"
            );
            match error {
                Scaffolded::Modeled(DeleteDomainErrorModeled::ConflictException(conflict)) => {
                    conflict
                }
                other => {
                    panic!("case {case:?}: expected the modelled ConflictException, got {other:?}")
                }
            }
        }
    }
}

/// The `message` of the `ResourceNotFoundException` a read of a domain the store does not
/// hold earns, with the modelled code and status checked first.
fn not_found_message(case: &str, result: Result<GetDomainOutput, GetDomainError>) -> String {
    match result {
        Ok(output) => panic!(
            "case {case:?}: expected ResourceNotFoundException (404), got Ok({:?})",
            output.domain
        ),
        Err(error) => {
            assert_eq!(
                (error.shape_name(), error.http_status()),
                ("ResourceNotFoundException", 404),
                "case {case:?}: expected ResourceNotFoundException (404), got {error:?}"
            );
            match error {
                Scaffolded::Modeled(GetDomainErrorModeled::ResourceNotFoundException(miss)) => {
                    miss.message
                }
                other => panic!(
                    "case {case:?}: expected the modelled ResourceNotFoundException, got {other:?}"
                ),
            }
        }
    }
}

/// One row of the teardown table.
struct Teardown {
    /// What the row is.
    label: &'static str,
    /// The spaces the domain is filled with, each created in a Region of its own because
    /// the account holds at most one space per Region. The names span `CreateSpace`'s
    /// modelled `name` class (`@length(3..64)`, `^[a-z0-9]+(-[a-z0-9]+)*$`).
    space_names: Vec<String>,
    /// The order the spaces are deleted in, as indices into `space_names`.
    delete_order: Vec<usize>,
}

/// The teardown table: rows differ in HOW MANY spaces the domain holds, the shape of the
/// names they were created under, and the ORDER they are deleted in — none of which may
/// change whether the empty domain deletes.
fn teardown_table() -> Vec<Teardown> {
    vec![
        Teardown {
            label: "one space, the shortest name the model admits",
            space_names: vec!["abc".to_string()],
            delete_order: vec![0],
        },
        Teardown {
            label: "two spaces, deleted in creation order",
            space_names: vec!["teardown-a".to_string(), "teardown-b".to_string()],
            delete_order: vec![0, 1],
        },
        Teardown {
            label: "three spaces, deleted in reverse order",
            space_names: vec![
                "reverse-one".to_string(),
                "reverse-two".to_string(),
                "reverse-3".to_string(),
            ],
            delete_order: vec![2, 1, 0],
        },
        Teardown {
            label: "three spaces, the middle one deleted first",
            space_names: vec![
                "middle-first-a".to_string(),
                "m2".to_string() + &"x".repeat(62),
                "middle-first-c".to_string(),
            ],
            delete_order: vec![1, 2, 0],
        },
    ]
}

#[tokio::test]
async fn the_spaces_a_domain_holds_are_released_as_they_are_deleted_and_the_empty_domain_deletes() {
    for (row, case) in teardown_table().into_iter().enumerate() {
        let label = case.label;
        // A fresh emulator per row: the account holds at most one domain, so each row
        // tears its own estate down from nothing.
        let omni = Omni::new();
        let domain_id = omni.domain(label, &format!("omni-teardown-{row}")).await;

        // Fill the domain, one space per Region.
        let mut created: Vec<String> = Vec::new();
        for (index, name) in case.space_names.iter().enumerate() {
            let region = SPACE_REGIONS[index];
            let space = omni
                .create_space(name, &domain_id, region)
                .await
                .unwrap_or_else(|error| {
                    panic!("case {label:?}: creating {name:?} in {region:?} failed: {error:?}")
                });
            assert_eq!(
                (space.region.as_str(), space.name.as_str()),
                (region, name.as_str()),
                "case {label:?}: the created space reports {space:?}"
            );
            created.push(space.space_id);
        }

        // Every further create signed for a Region the account already holds a space in is
        // the modelled conflict, and writes nothing: the account's Region is the slot.
        for (index, region) in SPACE_REGIONS
            .iter()
            .take(case.space_names.len())
            .enumerate()
        {
            let second = format!("{label}: a second create in {region}");
            let conflict = create_conflict(
                &second,
                omni.create_space(&format!("second-{index}"), &domain_id, region)
                    .await,
            );
            let expected = space_already_exists(region);
            assert_eq!(
                conflict.message, expected,
                "case {second:?}: expected {expected:?}, got {:?}",
                conflict.message
            );
        }
        let mut expected_live = created.clone();
        expected_live.sort();
        assert_eq!(
            omni.live_space_ids(label, &domain_id).await,
            expected_live,
            "case {label:?}: a refused create must mint nothing"
        );

        // The teardown order: while the domain holds spaces the delete is refused, and the
        // refusal counts the spaces the store holds AT THAT MOMENT.
        for (step, index) in case.delete_order.iter().copied().enumerate() {
            let remaining = created.len() - step;
            let blocked = format!("{label}: DeleteDomain with {remaining} space(s) live");
            let conflict = delete_conflict(&blocked, omni.delete_domain(&domain_id).await);
            let expected = domain_holds_spaces(&domain_id, remaining);
            assert_eq!(
                (
                    conflict.message.as_str(),
                    conflict.conflict_type.as_deref(),
                    conflict.resource_id.as_deref(),
                    conflict.resource_type.as_deref(),
                ),
                (
                    expected.as_str(),
                    Some("NOT_EMPTY"),
                    Some(domain_id.as_str()),
                    Some("Domain"),
                ),
                "case {blocked:?}: the refusal must state the live count, got {conflict:?}"
            );
            // A refused delete performs nothing: the domain is still readable.
            omni.get_domain(&domain_id).await.unwrap_or_else(|error| {
                panic!("case {blocked:?}: the refused delete removed the domain: {error:?}")
            });

            // Deleting a space releases it from the domain's set, so the next refusal
            // counts one fewer and the listing reports one fewer.
            let space_id = &created[index];
            omni.delete_space(&blocked, space_id).await;
            let live = omni.live_space_ids(&blocked, &domain_id).await;
            assert_eq!(
                live.len(),
                remaining - 1,
                "case {blocked:?}: deleting {space_id:?} left {live:?}"
            );
            assert!(
                !live.contains(space_id),
                "case {blocked:?}: the deleted space {space_id:?} is still listed: {live:?}"
            );
        }

        // With the last space gone the delete goes through, and the modelled output has no
        // members at all — which the wire half of this suite asserts on the body itself.
        let emptied = format!("{label}: DeleteDomain on the emptied domain");
        omni.delete_domain(&domain_id).await.unwrap_or_else(|error| {
            panic!("case {emptied:?}: the empty domain must delete: {error:?}")
        });

        // A deleted domain is unreadable, and the miss states AWS's fixed text with no
        // variable part — not the id it was asked about.
        let gone = format!("{label}: GetDomain after the delete");
        let message = not_found_message(&gone, omni.get_domain(&domain_id).await);
        assert_eq!(
            message, DOMAIN_NOT_FOUND,
            "case {gone:?}: expected {DOMAIN_NOT_FOUND:?}, got {message:?}"
        );
        assert!(
            !message.contains(&domain_id),
            "case {gone:?}: the fixed message quotes the domain id: {message:?}"
        );

        // `@idempotent`: repeating the delete on a domain that is already gone succeeds, so
        // a retrying client converges instead of looping on a refusal.
        for attempt in 0..2 {
            let retry = format!("{label}: DeleteDomain retry {attempt}");
            omni.delete_domain(&domain_id).await.unwrap_or_else(|error| {
                panic!("case {retry:?}: the idempotent delete must succeed: {error:?}")
            });
        }

        // And the account is not wedged: it can hold a domain again, as a NEW record, and
        // the Regions the deleted spaces held are free again.
        let replacement = omni.domain(label, &format!("omni-teardown-{row}-again")).await;
        assert_ne!(
            replacement, domain_id,
            "case {label:?}: the create after the delete must mint a new domain, got \
             {replacement:?} twice"
        );
        let refilled = omni
            .create_space("refilled", &replacement, SPACE_REGIONS[0])
            .await
            .unwrap_or_else(|error| {
                panic!("case {label:?}: the freed Region refused a create: {error:?}")
            });
        assert!(
            !created.contains(&refilled.space_id),
            "case {label:?}: the create after the teardown resurrected {:?}",
            refilled.space_id
        );
    }
}

#[tokio::test]
async fn the_teardown_order_over_the_rpcv2cbor_wire() {
    let omni = Omni::new();
    let region = SPACE_REGIONS[0];
    let domain_id = omni.domain("wire", "omni-teardown-wire").await;
    let space = omni
        .create_space("wire-space", &domain_id, region)
        .await
        .expect("case \"wire\": the first space of the Region is created");

    // A second create in the Region the account already holds a space in: 409, with the
    // conflict's own message in the envelope a client reads.
    let (status, body) = omni
        .over_the_wire(
            "CreateSpace",
            CREATE_SPACE_INPUT_SCHEMA,
            &CreateSpaceInput {
                name: "wire-space-again".to_string(),
                domain_id: domain_id.clone(),
                data_access_role_arn: DATA_ACCESS_ROLE_ARN.to_string(),
                ..Default::default()
            },
            region,
        )
        .await;
    let (code, message) = error_envelope("a second CreateSpace in the Region", &body);
    let expected = space_already_exists(region);
    assert_eq!(
        (status, code.as_str(), message.as_str()),
        (409, "ConflictException", expected.as_str()),
        "the wire must answer 409 ConflictException with {expected:?}, got {status} {code:?}: \
         {message:?}"
    );

    // The domain still holds its one space, so the delete is still refused over the wire.
    let (status, body) = omni
        .over_the_wire(
            "DeleteDomain",
            DELETE_DOMAIN_INPUT_SCHEMA,
            &DeleteDomainInput {
                domain_id: domain_id.clone(),
            },
            region,
        )
        .await;
    let (code, message) = error_envelope("DeleteDomain while a space is live", &body);
    let expected = domain_holds_spaces(&domain_id, 1);
    assert_eq!(
        (status, code.as_str(), message.as_str()),
        (409, "ConflictException", expected.as_str()),
        "the wire must answer 409 ConflictException with {expected:?}, got {status} {code:?}: \
         {message:?}"
    );

    // Delete the space over the wire, then the domain: an emptied domain answers 200 with
    // an EMPTY body, because `DeleteDomainOutput` has no members.
    let (status, body) = omni
        .over_the_wire(
            "DeleteSpace",
            DELETE_SPACE_INPUT_SCHEMA,
            &DeleteSpaceInput {
                space_id: space.space_id.clone(),
            },
            region,
        )
        .await;
    assert_eq!(
        status,
        200,
        "DeleteSpace over the wire answered {status}: {:?}",
        String::from_utf8_lossy(&body)
    );

    let (status, body) = omni
        .over_the_wire(
            "DeleteDomain",
            DELETE_DOMAIN_INPUT_SCHEMA,
            &DeleteDomainInput {
                domain_id: domain_id.clone(),
            },
            region,
        )
        .await;
    assert_eq!(
        status,
        200,
        "DeleteDomain over the wire answered {status} for an emptied domain: {:?}",
        String::from_utf8_lossy(&body)
    );
    let members = top_level_members("DeleteDomain on the emptied domain", &body);
    assert!(
        members.is_empty(),
        "the emptied domain's delete must answer an empty body, got members {members:?}"
    );
    let codec = CborCodec::default();
    let mut deserializer = codec.create_deserializer(&body);
    DeleteDomainOutput::deserialize(&mut deserializer).expect("the empty output deserializes");

    // And the deleted domain is unreadable over the wire: 404, the fixed message, and none
    // of the endpoint URLs the record used to announce.
    let (status, body) = omni
        .over_the_wire(
            "GetDomain",
            GET_DOMAIN_INPUT_SCHEMA,
            &GetDomainInput {
                domain_id: domain_id.clone(),
            },
            region,
        )
        .await;
    let (code, message) = error_envelope("GetDomain after the delete", &body);
    assert_eq!(
        (status, code.as_str(), message.as_str()),
        (404, "ResourceNotFoundException", DOMAIN_NOT_FOUND),
        "the wire must answer 404 ResourceNotFoundException with {DOMAIN_NOT_FOUND:?}, got \
         {status} {code:?}: {message:?}"
    );
    // Read as the map keys they are rather than scanned as text, so a member name occurring
    // inside a value is not mistaken for the member itself.
    let members = top_level_members("GetDomain after the delete", &body);
    for member in ["domain", "domainEndpointUrl", "customEndpointUrls"] {
        assert!(
            !members.iter().any(|carried| carried == member),
            "the miss still carries {member:?}: {members:?}"
        );
    }
}
