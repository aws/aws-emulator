// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for CloudWatchOmni's domain -> space lifecycle.
//!
//! Every call goes through a real operation handler holding the SAME shared engine
//! the router hands every service, so a create is observable by the reader that runs
//! next — exactly the store contract `tests/all/state_roundtrip.rs` pins. Failures
//! are the operations' own modelled errors, so each assertion checks the wire facts
//! that are requirements: the HTTP status and the error code.
//!
//! [`the_space_lifecycle_over_the_rpcv2cbor_wire`] additionally drives the same store
//! over HTTP through the framework's [`MultiServiceRouter`], so routing,
//! deserialization, the handler and response serialization all run for the space
//! operations.
//!
//! The suite asserts the RULES rather than one recorded exchange:
//!
//! * a create persists, and the matching reader returns the persisted record
//!   verbatim and stably;
//! * a patch changes only the members the request carries, and the members a domain
//!   DERIVES from them move with them: its name's endpoint URLs follow the name, while
//!   the members built from its id (the ARN, the domain endpoint URL) do not;
//! * every derived member (id shape, ARN, endpoint URL, region, owner account,
//!   status, encryption configuration) is computed from the stored resource;
//! * a reader answers `ResourceNotFoundException` for a resource the store does not
//!   hold, except where the resource is a SPACE: a space is the service's unit of
//!   access, so an id the account holds no space record under authorizes nobody and
//!   every space operation on it is refused `AccessDeniedException` (403) before
//!   anything is read, written or reported;
//! * a create is admitted only WITH the `clientToken` the model declares for it, and an
//!   account holds at most one domain, so a further create conflicts with the one it
//!   holds — both refusals before anything is minted, and both with the message AWS
//!   words them with;
//! * `@idempotencyToken` replays return the original resource;
//! * `ListSpaces` pages a stable creation order under every page bound, and its
//!   token resumes exactly where the previous page stopped;
//! * the modelled input constraints refuse what they declare.
//!
//! Each assertion names its case and the actual value, so a failure identifies the
//! row of the table that broke.

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

use omni::ops::create_domain::{CreateDomain, CreateDomainError, CreateDomainErrorModeled};
use omni::ops::create_space::{CreateSpace, CreateSpaceError};
use omni::ops::delete_domain::{DeleteDomain, DeleteDomainError, DeleteDomainErrorModeled};
use omni::ops::delete_space::{DeleteSpace, DeleteSpaceError};
use omni::ops::get_domain::{GetDomain, GetDomainError};
use omni::ops::get_space::{GetSpace, GetSpaceError};
use omni::ops::list_spaces::{ListSpaces, ListSpacesError};
use omni::ops::types::{
    CreateDomainInput, CreateDomainOutput, CreateSpaceInput, CreateSpaceOutput, DeleteDomainInput,
    DeleteDomainOutput, DeleteSpaceInput, DeleteSpaceOutput, Domain, EncryptionConfiguration,
    GetDomainInput, GetDomainOutput, GetSpaceInput, GetSpaceOutput, IdentityCenterConfiguration,
    IdentityProviderConfiguration, ListSpacesInput, ListSpacesOutput, Space, UpdateDomainInput,
    UpdateDomainOutput, CREATE_DOMAIN_INPUT_SCHEMA, CREATE_SPACE_INPUT_SCHEMA,
    DELETE_DOMAIN_INPUT_SCHEMA, GET_SPACE_INPUT_SCHEMA, LIST_SPACES_INPUT_SCHEMA,
    UPDATE_DOMAIN_INPUT_SCHEMA,
};
use omni::ops::update_domain::{UpdateDomain, UpdateDomainError, UpdateDomainErrorModeled};
use omni::scaffolded::Scaffolded;
use omni::state::{IDENTITY_CENTER_INSTANCE_REQUIRED, SPACE_QUOTA};

use super::validation_message_wording::error_envelope;

/// The region the suite signs for; the handlers record the signed region on each
/// resource, so every ARN and `region` member below is expected to carry it.
const REGION: &str = "us-west-2";

/// `DomainId`'s modelled `@pattern`.
const DOMAIN_ID_PATTERN: &str = r"^d-[0-9a-z]{1,25}$";

/// `SpaceId`'s modelled `@pattern`.
const SPACE_ID_PATTERN: &str = r"^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$";

/// `AccountId`'s modelled `@pattern`.
const ACCOUNT_ID_PATTERN: &str = r"^[0-9]{12}$";

/// `Arn`'s modelled `@pattern`.
const ARN_PATTERN: &str = r"^arn:.+$";

/// A KMS key ARN satisfying `KmsKeyArn`'s modelled
/// `^arn:aws[a-zA-Z-]*:kms:[a-z0-9-]+:[0-9]{12}:key/(mrk-)?[a-f0-9-]+$`.
const CUSTOMER_KEY_ARN: &str =
    "arn:aws:kms:us-west-2:000000000000:key/8a4f1b3c-0e2d-4a5b-9c7d-1f2e3a4b5c6d";

/// A role ARN satisfying `Arn`'s `^arn:.+$`.
const DATA_ACCESS_ROLE_ARN: &str = "arn:aws:iam::000000000000:role/OmniData";

/// A well-formed `DomainId` that names nothing.
const MISSING_DOMAIN_ID: &str = "d-doesnotexist";

/// A well-formed `SpaceId` that names nothing.
const MISSING_SPACE_ID: &str = "00000000-0000-4000-8000-000000000000";

/// Whether `value` satisfies [`DOMAIN_ID_PATTERN`].
///
/// The modelled `@pattern`s below are checked by hand rather than through a regex
/// engine, so this crate declares no dependency beyond the ones it already has and the
/// fix carries no `Cargo.lock` delta. Each predicate states the pattern it implements
/// and is exercised on values that must FAIL it as well as pass
/// ([`the_model_patterns_are_checked_as_written`]).
fn is_domain_id(value: &str) -> bool {
    // ^d-[0-9a-z]{1,25}$
    value.strip_prefix("d-").is_some_and(|body| {
        (1..=25).contains(&body.len())
            && body
                .chars()
                .all(|c| c.is_ascii_digit() || c.is_ascii_lowercase())
    })
}

/// Whether `value` satisfies [`SPACE_ID_PATTERN`].
fn is_space_id(value: &str) -> bool {
    // ^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$
    let groups: Vec<&str> = value.split('-').collect();
    groups.len() == 5
        && groups
            .iter()
            .zip([8_usize, 4, 4, 4, 12])
            .all(|(group, width)| {
                group.len() == width
                    && group
                        .chars()
                        .all(|c| c.is_ascii_digit() || ('a'..='f').contains(&c))
            })
}

/// Whether `value` satisfies [`ACCOUNT_ID_PATTERN`].
fn is_account_id(value: &str) -> bool {
    // ^[0-9]{12}$
    value.len() == 12 && value.chars().all(|c| c.is_ascii_digit())
}

/// Whether `value` satisfies [`ARN_PATTERN`].
fn is_arn(value: &str) -> bool {
    // ^arn:.+$
    value
        .strip_prefix("arn:")
        .is_some_and(|rest| !rest.is_empty() && !rest.contains('\n'))
}

#[tokio::test]
async fn the_model_patterns_are_checked_as_written() {
    // Each row: the value, and whether the modelled pattern admits it. The predicates
    // above are what every other assertion in this suite relies on, so they are pinned
    // against values on both sides of each pattern.
    let domain_ids = [
        ("d-abc123", true),
        ("d-a", true),
        (&format!("d-{}", "a".repeat(25)), true),
        (&format!("d-{}", "a".repeat(26)), false),
        ("d-", false),
        ("d-ABC", false),
        ("d-ab_c", false),
        ("abc123", false),
    ];
    for (value, expected) in domain_ids {
        assert_eq!(
            is_domain_id(value),
            expected,
            "case {value:?}: {DOMAIN_ID_PATTERN} should {} it",
            if expected { "admit" } else { "reject" }
        );
    }

    let space_ids = [
        ("00000000-0000-4000-8000-000000000000", true),
        ("8a4f1b3c-0e2d-4a5b-9c7d-1f2e3a4b5c6d", true),
        ("8A4F1B3C-0E2D-4A5B-9C7D-1F2E3A4B5C6D", false),
        ("8a4f1b3c-0e2d-4a5b-9c7d-1f2e3a4b5c6", false),
        ("8a4f1b3c0e2d4a5b9c7d1f2e3a4b5c6d", false),
        ("8a4f1b3g-0e2d-4a5b-9c7d-1f2e3a4b5c6d", false),
    ];
    for (value, expected) in space_ids {
        assert_eq!(
            is_space_id(value),
            expected,
            "case {value:?}: {SPACE_ID_PATTERN} should {} it",
            if expected { "admit" } else { "reject" }
        );
    }

    for (value, expected) in [
        ("000000000000", true),
        ("123456789012", true),
        ("12345678901", false),
        ("1234567890123", false),
        ("12345678901a", false),
    ] {
        assert_eq!(
            is_account_id(value),
            expected,
            "case {value:?}: {ACCOUNT_ID_PATTERN} should {} it",
            if expected { "admit" } else { "reject" }
        );
    }

    for (value, expected) in [
        ("arn:aws:iam::000000000000:role/OmniData", true),
        ("arn:x", true),
        ("arn:", false),
        ("aws:arn:thing", false),
    ] {
        assert_eq!(
            is_arn(value),
            expected,
            "case {value:?}: {ARN_PATTERN} should {} it",
            if expected { "admit" } else { "reject" }
        );
    }

    // And the ids the service mints satisfy the patterns it models them against.
    let omni = Omni::new();
    let parent = domain(&omni, "minted", "omni-minted-ids", &["IAM"]).await;
    assert!(
        is_domain_id(&parent.domain_id),
        "case \"minted\": domainId {:?} does not satisfy {DOMAIN_ID_PATTERN}",
        parent.domain_id
    );
    let created = space(&omni, "minted", "minted-space", &parent.domain_id).await;
    assert!(
        is_space_id(&created.space_id),
        "case \"minted\": spaceId {:?} does not satisfy {SPACE_ID_PATTERN}",
        created.space_id
    );
}

/// A timestamp at millisecond resolution.
///
/// rpcv2Cbor encodes a `Timestamp` with millisecond resolution, so a value compared
/// across a wire round trip is compared at that resolution rather than at the
/// microsecond resolution the clock reports.
fn millis(at: aws_smithy_types::DateTime) -> (i64, u32) {
    (at.secs(), at.subsec_nanos() / 1_000_000)
}

/// A SigV4 `Authorization` header whose credential scope names this service's signing
/// name (the router's routing key) and the region the handlers record.
fn signed_request_parts() -> http::request::Parts {
    signed_request_parts_in(REGION)
}

/// The same, signed for `region` rather than for the suite's own.
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

/// The Regions a suite signs its space creates for, this suite's own first.
///
/// An account holds at most ONE space per Region, so a case needing more than one space
/// creates each in a Region of its own; everything after the create addresses the space
/// by its id, which no Region scopes. Every suite of this crate that needs a second space
/// shares this list, so one rule is stated in one place. The entries are real AWS Region
/// codes, because a Region reaches a space's `region` member and its ARN.
pub const SPACE_REGIONS: [&str; 20] = [
    REGION,
    "us-east-1",
    "us-east-2",
    "us-west-1",
    "ca-central-1",
    "eu-west-1",
    "eu-west-2",
    "eu-west-3",
    "eu-central-1",
    "eu-north-1",
    "eu-south-1",
    "ap-south-1",
    "ap-southeast-1",
    "ap-southeast-2",
    "ap-northeast-1",
    "ap-northeast-2",
    "ap-northeast-3",
    "sa-east-1",
    "me-south-1",
    "af-south-1",
];

/// One emulator: one shared engine, the service mounted on a router, and the request
/// metadata a handler reads.
struct Omni {
    engine: Arc<engine::InMemoryEngine>,
    service: omni::EmulatedService<engine::InMemoryEngine>,
    router: MultiServiceRouter,
    parts: http::request::Parts,
    /// How many spaces this emulator has created through [`Omni::next_space_region`],
    /// which picks each create's Region out of [`SPACE_REGIONS`].
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
            parts: signed_request_parts(),
            spaces_created: std::sync::atomic::AtomicUsize::new(0),
        }
    }

    /// The Region the next space this emulator creates is signed for.
    fn next_space_region(&self) -> &'static str {
        let index = self
            .spaces_created
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        SPACE_REGIONS
            .get(index)
            .copied()
            .expect("the suite signs each space it creates for a Region of its own")
    }

    fn ctx(&self) -> OperationContext<'_> {
        OperationContext::new(
            &self.parts,
            self.service.definition().identity(),
            EmbeddedEmulator::from_router(self.router.clone()),
        )
    }

    async fn create_domain(
        &self,
        input: &CreateDomainInput,
    ) -> Result<CreateDomainOutput, CreateDomainError> {
        CreateDomain::new(Arc::clone(&self.engine))
            .handle(input.clone(), &self.ctx())
            .await
    }

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

    async fn update_domain(
        &self,
        input: &UpdateDomainInput,
    ) -> Result<UpdateDomainOutput, UpdateDomainError> {
        UpdateDomain::new(Arc::clone(&self.engine))
            .handle(input.clone(), &self.ctx())
            .await
    }

    async fn delete_domain(&self, domain_id: &str) -> Result<DeleteDomainOutput, DeleteDomainError> {
        DeleteDomain::new(Arc::clone(&self.engine))
            .handle(
                DeleteDomainInput {
                    domain_id: domain_id.to_string(),
                },
                &self.ctx(),
            )
            .await
    }

    async fn create_space(
        &self,
        input: &CreateSpaceInput,
    ) -> Result<CreateSpaceOutput, CreateSpaceError> {
        CreateSpace::new(Arc::clone(&self.engine))
            .handle(input.clone(), &self.ctx())
            .await
    }

    /// The same create, signed for `region` rather than for the suite's own — which is
    /// what a case needing more than one space of the account uses.
    async fn create_space_in_region(
        &self,
        input: &CreateSpaceInput,
        region: &str,
    ) -> Result<CreateSpaceOutput, CreateSpaceError> {
        let parts = signed_request_parts_in(region);
        let ctx = OperationContext::new(
            &parts,
            self.service.definition().identity(),
            EmbeddedEmulator::from_router(self.router.clone()),
        );
        CreateSpace::new(Arc::clone(&self.engine))
            .handle(input.clone(), &ctx)
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

    async fn delete_space(&self, space_id: &str) -> Result<DeleteSpaceOutput, DeleteSpaceError> {
        DeleteSpace::new(Arc::clone(&self.engine))
            .handle(
                DeleteSpaceInput {
                    space_id: space_id.to_string(),
                },
                &self.ctx(),
            )
            .await
    }

    async fn list_spaces(
        &self,
        domain_id: Option<&str>,
        max_results: Option<i32>,
        next_token: Option<String>,
    ) -> Result<ListSpacesOutput, ListSpacesError> {
        ListSpaces::new(Arc::clone(&self.engine))
            .handle(
                ListSpacesInput {
                    domain_id: domain_id.map(str::to_string),
                    next_token,
                    max_results,
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

/// One row of the `CreateSpace` table: the request members that vary, and the
/// `encryptionConfiguration` the created space must report back.
struct SpaceCase {
    name: String,
    requested_encryption: Option<EncryptionConfiguration>,
    agent_role: Option<&'static str>,
    expected_strategy: &'static str,
    expected_key: Option<&'static str>,
}

/// A `clientToken` no other call of this suite uses.
///
/// `CreateDomain` is admitted only WITH the `@idempotencyToken` the model declares for
/// it, so every create that is not testing a replay carries a token of its own: sharing
/// one would turn the next create into a replay of the last. The counter is only a
/// source of distinct tokens; nothing reads it back.
fn unique_client_token() -> String {
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    format!(
        "token-{}",
        NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    )
}

fn create_domain_input(name: &str, providers: &[&str]) -> CreateDomainInput {
    CreateDomainInput {
        name: name.to_string(),
        identity_providers: providers.iter().map(|p| (*p).to_string()).collect(),
        client_token: Some(unique_client_token()),
        ..Default::default()
    }
}

fn create_space_input(name: &str, domain_id: &str) -> CreateSpaceInput {
    CreateSpaceInput {
        name: name.to_string(),
        domain_id: domain_id.to_string(),
        data_access_role_arn: DATA_ACCESS_ROLE_ARN.to_string(),
        ..Default::default()
    }
}

async fn domain(omni: &Omni, case: &str, name: &str, providers: &[&str]) -> Domain {
    omni.create_domain(&create_domain_input(name, providers))
        .await
        .unwrap_or_else(|error| panic!("case {case:?}: CreateDomain failed: {error:?}"))
        .domain
}

/// A space of `domain_id`, created in a Region of its own ([`SPACE_REGIONS`]) so an
/// account that already holds one elsewhere can still have it.
async fn space(omni: &Omni, case: &str, name: &str, domain_id: &str) -> Space {
    omni.create_space_in_region(
        &create_space_input(name, domain_id),
        omni.next_space_region(),
    )
    .await
    .unwrap_or_else(|error| panic!("case {case:?}: CreateSpace failed: {error:?}"))
    .space
}

/// Walk every page of `ListSpaces` and return the space ids in the order they came
/// back, asserting each page respects the requested bound.
async fn paged_space_ids(
    omni: &Omni,
    case: &str,
    domain_id: Option<&str>,
    max_results: Option<i32>,
) -> Vec<String> {
    let mut ids = Vec::new();
    let mut token: Option<String> = None;
    loop {
        let page = omni
            .list_spaces(domain_id, max_results, token)
            .await
            .unwrap_or_else(|error| panic!("case {case:?}: ListSpaces failed: {error:?}"));
        if let Some(bound) = max_results {
            assert!(
                page.items.len() <= bound as usize,
                "case {case:?}: a page of {} spaces exceeds maxResults={bound}",
                page.items.len()
            );
        }
        ids.extend(page.items.iter().map(|item| item.space_id.clone()));
        match page.next_token {
            Some(next) => token = Some(next),
            None => break,
        }
        assert!(
            ids.len() < 500,
            "case {case:?}: pagination did not terminate after {} ids",
            ids.len()
        );
    }
    ids
}

#[tokio::test]
async fn create_domain_persists_a_record_get_domain_returns_it_verbatim() {
    // Rows differ along the identity-provider set (the model's `IdentityProvider`
    // value set is exactly IAM | IDC, `@length(1..2)` `@uniqueItems`) and along the
    // name's size, including both published bounds of `name`'s `@length(3..63)`.
    let longest_name = "a".repeat(63);
    let cases: Vec<(String, Vec<&str>, bool)> = vec![
        ("abc".to_string(), vec!["IAM"], false),
        ("omni-one".to_string(), vec!["IDC"], false),
        (
            "omni-two-with-several-hyphens".to_string(),
            vec!["IAM", "IDC"],
            true,
        ),
        (longest_name, vec!["IDC", "IAM"], false),
    ];

    for (name, providers, with_idc_instance) in cases {
        let case = format!("name={name} providers={providers:?}");
        // An account holds at most ONE domain, so each row runs against an account of
        // its own rather than adding a second domain beside the previous row's.
        let omni = Omni::new();
        let mut input = create_domain_input(&name, &providers);
        if with_idc_instance {
            input.identity_provider_configuration = Some(IdentityProviderConfiguration {
                identity_center_configuration: Some(IdentityCenterConfiguration {
                    identity_center_instance_arn: Some(
                        "arn:aws:sso:::instance/ssoins-1234567890abcdef".to_string(),
                    ),
                }),
            });
        }
        let created = omni
            .create_domain(&input)
            .await
            .unwrap_or_else(|error| panic!("case {case:?}: CreateDomain failed: {error:?}"));
        let domain = created.domain;

        assert!(
            is_domain_id(&domain.domain_id),
            "case {case:?}: domainId {:?} does not satisfy {DOMAIN_ID_PATTERN}",
            domain.domain_id
        );
        assert!(
            is_arn(&domain.domain_arn),
            "case {case:?}: domainArn {:?} does not satisfy {ARN_PATTERN}",
            domain.domain_arn
        );
        assert_eq!(
            domain.domain_arn,
            format!(
                "arn:aws:cloudwatch:{REGION}:{}:domain/{}",
                engine::EMULATOR_ACCOUNT_ID, domain.domain_id
            ),
            "case {case:?}: domainArn is not built in the service's ARN namespace"
        );
        assert_eq!(
            domain.name.as_deref(),
            Some(name.as_str()),
            "case {case:?}: name was not persisted"
        );
        assert_eq!(
            domain.identity_providers,
            providers
                .iter()
                .map(|p| (*p).to_string())
                .collect::<Vec<String>>(),
            "case {case:?}: identityProviders were not persisted"
        );
        assert_eq!(
            domain.region, REGION,
            "case {case:?}: region is not the signed region"
        );
        // `DomainStatus` declares exactly one value.
        assert_eq!(
            domain.status, "ACTIVE",
            "case {case:?}: unexpected status {:?}",
            domain.status
        );
        assert_eq!(
            domain.domain_endpoint_url,
            domain_endpoint_url(&domain.domain_id),
            "case {case:?}: domainEndpointUrl {:?} is not the address this emulator serves the domain at",
            domain.domain_endpoint_url
        );
        assert!(
            !domain.domain_endpoint_url.contains(REGION),
            "case {case:?}: domainEndpointUrl {:?} is global and must carry no region",
            domain.domain_endpoint_url
        );
        let custom = domain.custom_endpoint_urls.clone().unwrap_or_default();
        assert_eq!(
            custom,
            vec![custom_endpoint_url(&name)],
            "case {case:?}: customEndpointUrls {custom:?} are not the name's endpoint on this emulator"
        );
        // `identityCenterApplicationArn` is "Absent for IAM-only domains".
        assert_eq!(
            domain.identity_center_application_arn.is_some(),
            providers.contains(&"IDC"),
            "case {case:?}: identityCenterApplicationArn presence {:?} does not follow the IDC provider",
            domain.identity_center_application_arn
        );

        let fetched = omni
            .get_domain(&domain.domain_id)
            .await
            .unwrap_or_else(|error| panic!("case {case:?}: GetDomain failed: {error:?}"))
            .domain;
        assert_eq!(
            format!("{fetched:?}"),
            format!("{domain:?}"),
            "case {case:?}: GetDomain did not return the record CreateDomain persisted"
        );
        let refetched = omni
            .get_domain(&domain.domain_id)
            .await
            .unwrap_or_else(|error| panic!("case {case:?}: GetDomain failed: {error:?}"))
            .domain;
        assert_eq!(
            format!("{:?}", refetched.created_at),
            format!("{:?}", domain.created_at),
            "case {case:?}: createdAt moved between reads"
        );
    }
}

#[tokio::test]
async fn create_space_derives_every_member_from_its_parent_and_round_trips() {
    let omni = Omni::new();
    let parent = domain(&omni, "parent", "omni-parent", &["IAM"]).await;

    // Rows differ along the encryption configuration (absent, explicit AWS_OWNED,
    // CUSTOMER_MANAGED with a key — the `EncryptionStrategy` value set in full) and
    // along whether the optional AgentCore role is supplied; `name` spans both
    // published bounds of its `@length(3..64)`.
    let longest_name = "b".repeat(64);
    let cases: Vec<SpaceCase> = vec![
        SpaceCase {
            name: "spc".to_string(),
            requested_encryption: None,
            agent_role: None,
            expected_strategy: "AWS_OWNED",
            expected_key: None,
        },
        SpaceCase {
            name: "space-aws-owned".to_string(),
            requested_encryption: Some(EncryptionConfiguration {
                encryption_strategy: "AWS_OWNED".to_string(),
                kms_key_arn: None,
            }),
            agent_role: None,
            expected_strategy: "AWS_OWNED",
            expected_key: None,
        },
        SpaceCase {
            name: "space-customer-managed".to_string(),
            requested_encryption: Some(EncryptionConfiguration {
                encryption_strategy: "CUSTOMER_MANAGED".to_string(),
                kms_key_arn: Some(CUSTOMER_KEY_ARN.to_string()),
            }),
            agent_role: Some("arn:aws:iam::000000000000:role/AgentCoreEval"),
            expected_strategy: "CUSTOMER_MANAGED",
            expected_key: Some(CUSTOMER_KEY_ARN),
        },
        SpaceCase {
            name: longest_name,
            requested_encryption: None,
            agent_role: None,
            expected_strategy: "AWS_OWNED",
            expected_key: None,
        },
    ];

    for (index, SpaceCase {
        name,
        requested_encryption,
        agent_role,
        expected_strategy,
        expected_key,
    }) in cases.into_iter().enumerate()
    {
        // Each row's space is the account's only one in its Region, so each is created in
        // a Region of its own and every derived member is checked against THAT Region.
        let region = SPACE_REGIONS[index];
        let case = format!("name={name} strategy={expected_strategy} region={region}");
        let mut input = create_space_input(&name, &parent.domain_id);
        input.encryption_configuration = requested_encryption;
        input.agent_core_evaluation_role_arn = agent_role.map(str::to_string);
        let space = omni
            .create_space_in_region(&input, region)
            .await
            .unwrap_or_else(|error| panic!("case {case:?}: CreateSpace failed: {error:?}"))
            .space;

        assert!(
            is_space_id(&space.space_id),
            "case {case:?}: spaceId {:?} does not satisfy {SPACE_ID_PATTERN}",
            space.space_id
        );
        assert_eq!(
            space.space_arn,
            format!(
                "arn:aws:cloudwatch:{region}:{}:space/{}",
                engine::EMULATOR_ACCOUNT_ID, space.space_id
            ),
            "case {case:?}: spaceArn is not built in the service's ARN namespace"
        );
        assert_eq!(
            space.domain_arn.as_deref(),
            Some(parent.domain_arn.as_str()),
            "case {case:?}: domainArn was not copied from the parent domain"
        );
        assert_eq!(
            space.region, region,
            "case {case:?}: region is not the signed region"
        );
        assert!(
            is_account_id(&space.owner_account_id),
            "case {case:?}: ownerAccountId {:?} does not satisfy {ACCOUNT_ID_PATTERN}",
            space.owner_account_id
        );
        assert_eq!(
            space.data_access_role_arn, DATA_ACCESS_ROLE_ARN,
            "case {case:?}: dataAccessRoleArn was not persisted"
        );
        assert_eq!(
            space.agent_core_evaluation_role_arn.as_deref(),
            agent_role,
            "case {case:?}: agentCoreEvaluationRoleArn was not persisted"
        );
        assert_eq!(
            space.status, "ACTIVE",
            "case {case:?}: unexpected status {:?}",
            space.status
        );
        // `Space.encryptionConfiguration` is "Always populated".
        let reported = space
            .encryption_configuration
            .clone()
            .unwrap_or_else(|| panic!("case {case:?}: encryptionConfiguration is absent"));
        assert_eq!(
            reported.encryption_strategy, expected_strategy,
            "case {case:?}: unexpected encryptionStrategy {:?}",
            reported.encryption_strategy
        );
        assert_eq!(
            reported.kms_key_arn.as_deref(),
            expected_key,
            "case {case:?}: unexpected kmsKeyArn {:?}",
            reported.kms_key_arn
        );

        let fetched = omni
            .get_space(&space.space_id)
            .await
            .unwrap_or_else(|error| panic!("case {case:?}: GetSpace failed: {error:?}"))
            .space;
        assert_eq!(
            format!("{fetched:?}"),
            format!("{space:?}"),
            "case {case:?}: GetSpace did not return the record CreateSpace persisted"
        );
    }
}

#[tokio::test]
async fn an_inconsistent_encryption_configuration_is_refused() {
    let omni = Omni::new();
    let parent = domain(&omni, "parent", "omni-encryption", &["IAM"]).await;

    // `EncryptionConfiguration`'s model doc: `kmsKeyArn` is required for
    // CUSTOMER_MANAGED and must be absent for AWS_OWNED; "a mismatch is a
    // ValidationException rather than a silently ignored field".
    let cases: Vec<(&str, &str, Option<&str>)> = vec![
        ("customer-managed-without-key", "CUSTOMER_MANAGED", None),
        ("aws-owned-with-key", "AWS_OWNED", Some(CUSTOMER_KEY_ARN)),
        ("unmodelled-strategy", "SELF_MANAGED", None),
    ];

    for (name, strategy, key) in cases {
        let case = format!("{name}: strategy={strategy} key={key:?}");
        let mut input = create_space_input(name, &parent.domain_id);
        input.encryption_configuration = Some(EncryptionConfiguration {
            encryption_strategy: strategy.to_string(),
            kms_key_arn: key.map(str::to_string),
        });
        expect_modelled_error(
            &case,
            omni.create_space(&input).await,
            400,
            "ValidationException",
        );
    }

    // The refusals persisted nothing.
    let spaces = paged_space_ids(&omni, "after refusals", Some(&parent.domain_id), None).await;
    assert!(
        spaces.is_empty(),
        "a refused CreateSpace must store nothing, found {spaces:?}"
    );
}

#[tokio::test]
async fn a_resource_the_store_does_not_hold_is_not_found() {
    let omni = Omni::new();

    // Every operation that NAMES a domain words the miss the same way, and AWS's sentence
    // names the kind of resource alone: it quotes no `domainId`, so the rows below check
    // the text as well as the status and the code.
    expect_not_found(
        "unknown domain",
        omni.get_domain(MISSING_DOMAIN_ID).await,
        DOMAIN_NOT_FOUND,
    );
    expect_not_found(
        "space under an unknown domain",
        omni.create_space(&create_space_input("orphan-space", MISSING_DOMAIN_ID))
            .await,
        DOMAIN_NOT_FOUND,
    );
    // `domainId` is a filter, not a resource the request addresses: a value the store
    // holds no domain for matches no space, so the listing is an empty page.
    let filtered = omni
        .list_spaces(Some(MISSING_DOMAIN_ID), None, None)
        .await
        .expect("a listing filtered by a domain nothing holds succeeds");
    assert!(
        filtered.items.is_empty(),
        "case \"spaces filtered by an unknown domain\": expected no spaces, got {:?}",
        filtered.items
    );
    // The unfiltered listing names no resource, so it is an empty page rather than a miss.
    let unfiltered = omni
        .list_spaces(None, None, None)
        .await
        .expect("an unfiltered listing of an empty store succeeds");
    assert!(
        unfiltered.items.is_empty(),
        "case \"unfiltered empty store\": expected no spaces, got {:?}",
        unfiltered.items
    );

    // A domain created after the failed lookups is still reachable: a miss is not a
    // write.
    let created = domain(&omni, "after misses", "omni-after-misses", &["IAM"]).await;
    omni.get_domain(&created.domain_id)
        .await
        .expect("the domain created after the misses is readable");
}

/// AWS's sentence for a domain nothing holds: fixed text naming the kind of resource,
/// written here rather than read from the crate so the assertion is independent of the
/// implementation.
const DOMAIN_NOT_FOUND: &str = "Domain not found";

/// Require the modelled miss: HTTP 404 `ResourceNotFoundException` whose `message` is
/// exactly `expected`.
fn expect_not_found<O: std::fmt::Debug, E: OperationError + std::fmt::Debug>(
    case: &str,
    result: Result<O, E>,
    expected: &str,
) {
    match result {
        Ok(output) => {
            panic!("case {case:?}: expected ResourceNotFoundException (404), got Ok({output:?})")
        }
        Err(error) => {
            assert_eq!(
                (error.shape_name(), error.http_status()),
                ("ResourceNotFoundException", 404),
                "case {case:?}: expected ResourceNotFoundException (404), got {error:?}"
            );
            let rendered = format!("{error:?}");
            assert!(
                rendered.contains(&format!("message: {expected:?}")),
                "case {case:?}: expected the message {expected:?}, got {rendered:?}"
            );
        }
    }
}

/// AWS's one refusal for a request the addressed space does not authorize: the fixed
/// text the service states it with, written here rather than read from the crate so the
/// assertion is independent of the implementation.
///
/// Shared with the suites of the resources a space's access control governs, so the
/// refusal's required shape cannot drift between them.
pub(crate) const AUTHORIZATION_FAILED: &str = "Authorization failed";

/// Require the modelled authorization refusal: HTTP 403 `AccessDeniedException` carrying
/// [`AUTHORIZATION_FAILED`] and quoting none of `withheld` — the values of the request
/// the refusal must not disclose.
pub(crate) fn expect_authorization_failure<
    O: std::fmt::Debug,
    E: OperationError + std::fmt::Debug,
>(
    case: &str,
    result: Result<O, E>,
    withheld: &[&str],
) {
    match result {
        Ok(output) => {
            panic!("case {case:?}: expected AccessDeniedException (403), got Ok({output:?})")
        }
        Err(error) => {
            assert_eq!(
                error.shape_name(),
                "AccessDeniedException",
                "case {case:?}: expected error code AccessDeniedException, got {error:?}"
            );
            assert_eq!(
                error.http_status(),
                403,
                "case {case:?}: expected HTTP 403, got {} ({error:?})",
                error.http_status()
            );
            let rendered = format!("{error:?}");
            assert!(
                rendered.contains(AUTHORIZATION_FAILED),
                "case {case:?}: expected the refusal to read {AUTHORIZATION_FAILED:?}, \
                 got {rendered:?}"
            );
            for value in withheld {
                assert!(
                    !rendered.contains(value),
                    "case {case:?}: the refusal quotes {value:?}: {rendered:?}"
                );
            }
        }
    }
}

#[tokio::test]
async fn a_space_the_account_does_not_hold_authorizes_no_space_operation() {
    let omni = Omni::new();
    let parent = domain(&omni, "authorization", "omni-authorization", &["IAM"]).await;
    let held = space(&omni, "authorization", "authorized-space", &parent.domain_id).await;
    // A space of the account authorizes both operations, so the refusals below are about
    // the id each request addressed and not about the operations themselves.
    omni.get_space(&held.space_id)
        .await
        .expect("case \"a space the account holds\": GetSpace is authorized");

    // Rows span the `SpaceId` class — nil digits, every hexadecimal digit, and an id one
    // character from the held one — plus a space the account HELD and deleted, which is a
    // record it no longer has and therefore authorizes nothing either.
    let released = space(&omni, "released", "released-space", &parent.domain_id).await;
    omni.delete_space(&released.space_id)
        .await
        .expect("case \"a released space\": the delete the account is authorized for");
    for (case, space_id) in [
        ("an id of nil digits", MISSING_SPACE_ID.to_string()),
        (
            "an id of every hexadecimal digit",
            "0123456f-4567-89ab-cdef-0123456789ab".to_string(),
        ),
        (
            "an id one character from the held space",
            flip_last_digit(&held.space_id),
        ),
        ("a space the account deleted", released.space_id.clone()),
    ] {
        expect_authorization_failure(
            &format!("GetSpace, {case}"),
            omni.get_space(&space_id).await,
            &[space_id.as_str()],
        );
        expect_authorization_failure(
            &format!("DeleteSpace, {case}"),
            omni.delete_space(&space_id).await,
            &[space_id.as_str()],
        );
    }

    // None of the refusals touched the store: the space the account holds is still there,
    // still the only one listed, and still readable.
    let listed = paged_space_ids(&omni, "after the refusals", Some(&parent.domain_id), None).await;
    assert_eq!(
        listed,
        vec![held.space_id.clone()],
        "the refused calls must leave the account's spaces alone, got {listed:?}"
    );
    let reread = omni
        .get_space(&held.space_id)
        .await
        .expect("the space the account holds is still readable")
        .space;
    assert_eq!(
        (reread.space_id.as_str(), reread.status.as_str()),
        (held.space_id.as_str(), held.status.as_str()),
        "the refused calls changed the space the account holds: {reread:?}"
    );
}

/// `space_id` with its last hexadecimal digit replaced by a different one, which is a
/// well-formed `SpaceId` naming another space.
///
/// Shared with the suites of the resources a space's access control governs.
pub(crate) fn flip_last_digit(space_id: &str) -> String {
    let (head, last) = space_id.split_at(space_id.len() - 1);
    let replacement = if last == "0" { "1" } else { "0" };
    format!("{head}{replacement}")
}

/// The message a `CreateDomain` carrying no `clientToken` is refused with: fixed text
/// apart from the name of the operation that required one.
const CLIENT_TOKEN_REQUIRED: &str = "A clientToken is required for CreateDomain";

/// The message a `CreateDomain` for an account that already holds a domain is refused
/// with: wholly fixed text, naming neither the held domain nor the requested one.
const DOMAIN_ALREADY_EXISTS: &str = "A domain already exists for this account";

/// The message the operation's modelled `ValidationException` carries, or a panic naming
/// what came back instead.
fn validation_message<O: std::fmt::Debug>(
    case: &str,
    result: Result<O, CreateDomainError>,
) -> String {
    match result {
        Ok(output) => panic!("case {case:?}: expected a ValidationException, got Ok({output:?})"),
        Err(error) => {
            assert_eq!(
                error.http_status(),
                400,
                "case {case:?}: expected HTTP 400, got {} ({error:?})",
                error.http_status()
            );
            match error {
                Scaffolded::Modeled(CreateDomainErrorModeled::ValidationException(refusal)) => {
                    refusal.message
                }
                other => panic!(
                    "case {case:?}: expected the modelled ValidationException, got {other:?}"
                ),
            }
        }
    }
}

/// The message the operation's modelled `ConflictException` carries, or a panic naming
/// what came back instead.
fn conflict_message<O: std::fmt::Debug>(case: &str, result: Result<O, CreateDomainError>) -> String {
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
                Scaffolded::Modeled(CreateDomainErrorModeled::ConflictException(refusal)) => {
                    refusal.message
                }
                other => {
                    panic!("case {case:?}: expected the modelled ConflictException, got {other:?}")
                }
            }
        }
    }
}

/// One row of the token-requirement table: an otherwise admissible `CreateDomain` that
/// carries no `clientToken`.
struct TokenlessCase {
    label: &'static str,
    name: String,
    providers: Vec<&'static str>,
    tags: Option<HashMap<String, String>>,
    identity_center_instance: Option<&'static str>,
}

#[tokio::test]
async fn a_create_domain_without_a_client_token_is_refused_and_mints_nothing() {
    // Every row is a request the modelled constraints admit and that differs from the
    // others in everything BUT the missing token — the name's size, the identity
    // provider set, the optional tags and the optional identity-provider configuration
    // — because the refusal is about the absent `clientToken` alone.
    let cases = vec![
        TokenlessCase {
            label: "the shortest name the model admits, IAM only",
            name: "abc".to_string(),
            providers: vec!["IAM"],
            tags: None,
            identity_center_instance: None,
        },
        TokenlessCase {
            label: "a hyphenated name with tags",
            name: "omni-tokenless-domain".to_string(),
            providers: vec!["IDC"],
            tags: Some(HashMap::from([("team".to_string(), "obs".to_string())])),
            identity_center_instance: None,
        },
        TokenlessCase {
            label: "both providers and an Identity Center instance",
            name: "omni-tokenless-idc".to_string(),
            providers: vec!["IAM", "IDC"],
            tags: None,
            identity_center_instance: Some("arn:aws:sso:::instance/ssoins-1234567890abcdef"),
        },
        TokenlessCase {
            label: "the longest name the model admits",
            name: "d".repeat(63),
            providers: vec!["IDC", "IAM"],
            tags: Some(HashMap::new()),
            identity_center_instance: None,
        },
    ];

    for TokenlessCase {
        label,
        name,
        providers,
        tags,
        identity_center_instance,
    } in cases
    {
        // Each row is its own account, so the create that follows the refusal is the
        // account's first.
        let omni = Omni::new();
        let mut input = create_domain_input(&name, &providers);
        input.tags = tags;
        input.identity_provider_configuration =
            identity_center_instance.map(|arn| IdentityProviderConfiguration {
                identity_center_configuration: Some(IdentityCenterConfiguration {
                    identity_center_instance_arn: Some(arn.to_string()),
                }),
            });
        input.client_token = None;

        let message = validation_message(label, omni.create_domain(&input).await);
        assert_eq!(
            message, CLIENT_TOKEN_REQUIRED,
            "case {label:?}: expected {CLIENT_TOKEN_REQUIRED:?}, got {message:?}"
        );

        // The refusal minted nothing: the account is still free, so the SAME request
        // carrying a token is admitted. Were the refused create to have landed, the
        // one-domain-per-account rule would refuse this one as a conflict.
        input.client_token = Some(unique_client_token());
        let created = omni
            .create_domain(&input)
            .await
            .unwrap_or_else(|error| panic!("case {label:?}: the create with a token failed: {error:?}"))
            .domain;
        assert_eq!(
            (
                created.name.as_deref(),
                created.identity_providers.clone(),
                created.region.as_str(),
                created.status.as_str(),
            ),
            (
                Some(name.as_str()),
                providers
                    .iter()
                    .map(|p| (*p).to_string())
                    .collect::<Vec<String>>(),
                REGION,
                "ACTIVE",
            ),
            "case {label:?}: the admitted create did not persist the request, got {created:?}"
        );
        let fetched = omni
            .get_domain(&created.domain_id)
            .await
            .unwrap_or_else(|error| panic!("case {label:?}: GetDomain failed: {error:?}"))
            .domain;
        assert_eq!(
            format!("{fetched:?}"),
            format!("{created:?}"),
            "case {label:?}: GetDomain did not return the record CreateDomain persisted"
        );
    }
}

#[tokio::test]
async fn a_second_domain_for_the_account_conflicts_whatever_it_asks_for() {
    let omni = Omni::new();
    let mut held_input = create_domain_input("omni-singleton", &["IAM"]);
    let held = omni
        .create_domain(&held_input)
        .await
        .expect("the account's first domain is admitted")
        .domain;

    // Rows differ in everything a create can ask for — the same name as the held domain
    // and names that differ from it, both published bounds of `name`'s `@length(3..63)`,
    // every identity-provider set, with and without tags — because the conflict is with
    // the ACCOUNT's domain rather than with any member of the request.
    let longest_name = "e".repeat(63);
    let cases: Vec<(&str, &str, Vec<&str>, bool)> = vec![
        ("the held domain's own name", "omni-singleton", vec!["IAM"], false),
        ("a different name", "omni-singleton-two", vec!["IDC"], false),
        ("a different provider set, with tags", "abc", vec!["IAM", "IDC"], true),
        ("the longest name the model admits", longest_name.as_str(), vec!["IDC"], false),
    ];

    for (label, name, providers, with_tags) in cases {
        let mut input = create_domain_input(name, &providers);
        if with_tags {
            input.tags = Some(HashMap::from([("owner".to_string(), "sre".to_string())]));
        }
        let message = conflict_message(label, omni.create_domain(&input).await);
        assert_eq!(
            message, DOMAIN_ALREADY_EXISTS,
            "case {label:?}: expected {DOMAIN_ALREADY_EXISTS:?}, got {message:?}"
        );
    }

    // Every conflict left the held domain exactly as it was.
    let still_there = omni
        .get_domain(&held.domain_id)
        .await
        .expect("the refused creates left the held domain in place")
        .domain;
    assert_eq!(
        format!("{still_there:?}"),
        format!("{held:?}"),
        "a refused create changed the domain the account holds"
    );

    // A replay of the create that minted it is not a conflict with itself: the token
    // returns the original record.
    let replay = omni
        .create_domain(&held_input)
        .await
        .expect("replaying the original create returns the original domain")
        .domain;
    assert_eq!(
        format!("{replay:?}"),
        format!("{held:?}"),
        "the idempotent replay did not return the held domain"
    );

    // Releasing the domain frees the account to hold one again, so the rule is a rule
    // about the CURRENT state and not a one-way ratchet.
    omni.delete_domain(&held.domain_id)
        .await
        .expect("an empty domain deletes");
    held_input.client_token = Some(unique_client_token());
    let replacement = omni
        .create_domain(&held_input)
        .await
        .expect("a freed account accepts a domain again")
        .domain;
    assert_ne!(
        replacement.domain_id, held.domain_id,
        "creating after the delete must mint a new domain, not resurrect the old one"
    );
}

#[tokio::test]
async fn the_domain_admission_rules_hold_over_the_rpcv2cbor_wire() {
    let omni = Omni::new();
    let codec = CborCodec::default();
    let decode = |body: &bytes::Bytes| -> CreateDomainOutput {
        let mut deserializer = codec.create_deserializer(body);
        CreateDomainOutput::deserialize(&mut deserializer).expect("the output deserializes")
    };

    // A create with no token is refused 400 with the modelled error code and the
    // message, over the same wire a client speaks.
    let mut input = create_domain_input("omni-wire-admission", &["IAM"]);
    input.client_token = None;
    let (status, body) = omni
        .over_the_wire("CreateDomain", CREATE_DOMAIN_INPUT_SCHEMA, &input)
        .await;
    let rendered = String::from_utf8_lossy(&body).to_string();
    assert_eq!(
        status, 400,
        "a CreateDomain with no clientToken answered {status}: {rendered:?}"
    );
    for expected in ["ValidationException", CLIENT_TOKEN_REQUIRED] {
        assert!(
            rendered.contains(expected),
            "case {expected:?}: the wire refusal does not carry it: {rendered:?}"
        );
    }

    // With a token the same request is admitted.
    input.client_token = Some(unique_client_token());
    let (status, body) = omni
        .over_the_wire("CreateDomain", CREATE_DOMAIN_INPUT_SCHEMA, &input)
        .await;
    assert_eq!(
        status,
        200,
        "CreateDomain over the wire answered {status}: {:?}",
        String::from_utf8_lossy(&body)
    );
    let created = decode(&body).domain;
    assert_eq!(
        (
            created.name.as_deref(),
            created.region.as_str(),
            created.status.as_str()
        ),
        (Some("omni-wire-admission"), REGION, "ACTIVE"),
        "the wire create did not report the persisted domain, got {created:?}"
    );

    // And the next create for the account is refused 409 with the conflict message.
    let (status, body) = omni
        .over_the_wire(
            "CreateDomain",
            CREATE_DOMAIN_INPUT_SCHEMA,
            &create_domain_input("omni-wire-second", &["IDC"]),
        )
        .await;
    let rendered = String::from_utf8_lossy(&body).to_string();
    assert_eq!(
        status, 409,
        "a second CreateDomain answered {status}: {rendered:?}"
    );
    for expected in ["ConflictException", DOMAIN_ALREADY_EXISTS] {
        assert!(
            rendered.contains(expected),
            "case {expected:?}: the wire conflict does not carry it: {rendered:?}"
        );
    }
}

#[tokio::test]
async fn an_idempotency_token_replay_returns_the_original_resource() {
    let omni = Omni::new();
    // Rows span `ClientToken`'s `@length(1..64)` and its `^[!-~]+$` character class.
    let tokens: Vec<String> = vec![
        "t".to_string(),
        "token-one".to_string(),
        "~!#$%&*+".to_string(),
        "z".repeat(64),
    ];

    let parent = domain(&omni, "parent", "omni-idempotent", &["IAM"]).await;

    for (index, token) in tokens.iter().enumerate() {
        let case = format!("token={token}");

        // The domain replay runs against an account of its own: an account holds at
        // most one domain, so a row cannot mint its domain beside the previous row's.
        let account = Omni::new();
        let mut domain_input = create_domain_input(&format!("idem-domain-{index}"), &["IAM"]);
        domain_input.client_token = Some(token.clone());
        let first = account
            .create_domain(&domain_input)
            .await
            .unwrap_or_else(|error| panic!("case {case:?}: CreateDomain failed: {error:?}"))
            .domain;
        let replay = account
            .create_domain(&domain_input)
            .await
            .unwrap_or_else(|error| panic!("case {case:?}: the replay failed: {error:?}"))
            .domain;
        assert_eq!(
            format!("{replay:?}"),
            format!("{first:?}"),
            "case {case:?}: a CreateDomain replay did not return the original domain"
        );

        // Each row's space is the account's only one in its Region, so the pair of
        // creates the replay is observed through is signed for a Region of its own.
        let region = SPACE_REGIONS[index];
        let mut space_input = create_space_input(&format!("idem-space-{index}"), &parent.domain_id);
        space_input.client_token = Some(token.clone());
        let first = omni
            .create_space_in_region(&space_input, region)
            .await
            .unwrap_or_else(|error| panic!("case {case:?}: CreateSpace failed: {error:?}"))
            .space;
        let replay = omni
            .create_space_in_region(&space_input, region)
            .await
            .unwrap_or_else(|error| panic!("case {case:?}: the replay failed: {error:?}"))
            .space;
        assert_eq!(
            format!("{replay:?}"),
            format!("{first:?}"),
            "case {case:?}: a CreateSpace replay did not return the original space"
        );
    }

    // One space per token, not two: the replays minted nothing.
    let ids = paged_space_ids(&omni, "after replays", Some(&parent.domain_id), None).await;
    assert_eq!(
        ids.len(),
        tokens.len(),
        "replays minted extra spaces: {ids:?}"
    );
}

#[tokio::test]
async fn list_spaces_pages_a_stable_order_under_every_bound() {
    let omni = Omni::new();

    let empty = omni
        .list_spaces(None, None, None)
        .await
        .expect("an empty store lists successfully");
    assert!(
        empty.items.is_empty() && empty.next_token.is_none(),
        "an empty store must list no spaces and hand back no token, got {} items / {:?}",
        empty.items.len(),
        empty.next_token
    );

    let first = domain(&omni, "first", "omni-first", &["IAM"]).await;

    // Seven spaces in the account's one domain. A second domain cannot be created
    // beside it (an account holds one), so the `domainId` filter is exercised against
    // the domain that holds them and — in `a_resource_the_store_does_not_hold_is_not_found`
    // — against a domain id the store does not hold.
    let mut in_first = Vec::new();
    for index in 0..7 {
        let created = space(
            &omni,
            "seed first",
            &format!("first-space-{index}"),
            &first.domain_id,
        )
        .await;
        in_first.push(created.space_id);
    }
    let everything: Vec<String> = in_first.clone();

    // Rows differ along the page bound — both published bounds of `maxResults`'s
    // `@range(1..100)`, sizes either side of the collection, and the documented
    // default (absent) — and along the `domainId` filter.
    let bounds: Vec<Option<i32>> = vec![None, Some(1), Some(2), Some(3), Some(7), Some(100)];
    let filters: Vec<(Option<&str>, &Vec<String>)> = vec![
        (None, &everything),
        (Some(first.domain_id.as_str()), &in_first),
    ];

    for bound in &bounds {
        for (filter, expected) in &filters {
            let case = format!("maxResults={bound:?} domainId={filter:?}");
            let seen = paged_space_ids(&omni, &case, *filter, *bound).await;
            assert_eq!(
                &seen, *expected,
                "case {case:?}: paging did not traverse the collection in creation order, got {seen:?}"
            );
        }
    }

    // A bounded first page hands back a token; the token resumes right after it.
    let page = omni
        .list_spaces(Some(&first.domain_id), Some(2), None)
        .await
        .expect("the first page lists");
    assert_eq!(
        page.items.len(),
        2,
        "a bound of 2 over 7 spaces must fill the page, got {}",
        page.items.len()
    );
    let token = page
        .next_token
        .clone()
        .expect("spaces remain, so a token must be returned");
    let next = omni
        .list_spaces(Some(&first.domain_id), Some(2), Some(token))
        .await
        .expect("the second page lists");
    assert_eq!(
        next.items
            .iter()
            .map(|item| item.space_id.clone())
            .collect::<Vec<String>>(),
        in_first[2..4].to_vec(),
        "the token did not resume exactly where the previous page stopped"
    );

    // The summary a list returns reports the same values as the full space.
    let summary = &page.items[0];
    let full = omni
        .get_space(&summary.space_id)
        .await
        .expect("the listed space is readable")
        .space;
    assert_eq!(
        (
            summary.name.as_str(),
            summary.space_arn.as_str(),
            summary.domain_arn.as_deref(),
            summary.region.as_str(),
            summary.owner_account_id.as_str(),
            summary.status.as_str(),
            format!("{:?}", summary.created_at),
            format!("{:?}", summary.updated_at),
        ),
        (
            full.name.as_str(),
            full.space_arn.as_str(),
            full.domain_arn.as_deref(),
            full.region.as_str(),
            full.owner_account_id.as_str(),
            full.status.as_str(),
            format!("{:?}", full.created_at),
            format!("{:?}", full.updated_at),
        ),
        "a SpaceSummary must project the stored space, not a second copy of it"
    );
}

#[tokio::test]
async fn a_next_token_this_service_did_not_mint_is_refused() {
    let omni = Omni::new();
    // Rows: not hex at all, valid hex that decodes to another collection's cursor,
    // and hex that decodes to this collection's prefix with an unparseable cursor.
    for token in ["not-a-token", "646f6d61696e733a31", "7370616365733a78"] {
        expect_modelled_error(
            token,
            omni.list_spaces(None, None, Some(token.to_string())).await,
            400,
            "ValidationException",
        );
    }
}

#[tokio::test]
async fn deleting_a_space_removes_it_and_a_domain_refuses_while_it_holds_spaces() {
    let omni = Omni::new();
    let parent = domain(&omni, "lifecycle", "omni-lifecycle", &["IAM"]).await;
    let created = space(&omni, "lifecycle", "lifecycle-space", &parent.domain_id).await;

    // "A domain cannot be deleted while it contains spaces."
    expect_modelled_error(
        "domain still holding a space",
        omni.delete_domain(&parent.domain_id).await,
        409,
        "ConflictException",
    );
    omni.get_domain(&parent.domain_id)
        .await
        .expect("the refused delete left the domain in place");

    // The space authorizes the delete while the account holds it; once it is gone the
    // account holds no record of it, so the retry a client makes after a timeout is
    // refused rather than answered as a delete that removed nothing — and the refusal
    // says nothing about whether the id ever named a space.
    omni.delete_space(&created.space_id)
        .await
        .expect("case \"the first DeleteSpace\": the space the account holds deletes");
    for attempt in 0..2 {
        let case = format!("DeleteSpace retry {attempt}");
        expect_modelled_error(
            &case,
            omni.delete_space(&created.space_id).await,
            403,
            "AccessDeniedException",
        );
        expect_modelled_error(
            &case,
            omni.get_space(&created.space_id).await,
            403,
            "AccessDeniedException",
        );
        let remaining = paged_space_ids(&omni, &case, Some(&parent.domain_id), None).await;
        assert!(
            remaining.is_empty(),
            "case {case:?}: the deleted space is still listed: {remaining:?}"
        );
    }

    // With the domain empty the delete goes through, and the account is free to hold a
    // domain again.
    omni.delete_domain(&parent.domain_id)
        .await
        .expect("an empty domain deletes");
    expect_modelled_error(
        "deleted domain",
        omni.get_domain(&parent.domain_id).await,
        404,
        "ResourceNotFoundException",
    );
    let reused = domain(&omni, "name reuse", "omni-lifecycle", &["IAM"]).await;
    assert_ne!(
        reused.domain_id, parent.domain_id,
        "creating after the delete must mint a new domain, not resurrect the old one"
    );

    // DeleteDomain is @idempotent too.
    omni.delete_domain(MISSING_DOMAIN_ID)
        .await
        .expect("deleting a domain that is already gone succeeds");
}

#[tokio::test]
async fn the_modelled_input_constraints_refuse_what_they_declare() {
    // `aws_smithy_schema_server::constraints::validate` is the exact check the
    // framework runs on a deserialized input before any handler sees it (see
    // `run_operation_http`), and a non-empty result becomes a 400
    // `ValidationException`. Each row below is refused there, so the handler never
    // has to re-check a modelled constraint.
    let too_long_space_name = "c".repeat(65);
    let too_long_domain_name = "a".repeat(64);

    let refused_space_names: Vec<(&str, String)> = vec![
        ("shorter than @length(min: 3)", "ab".to_string()),
        ("longer than @length(max: 64)", too_long_space_name),
        ("outside the name pattern (upper case)", "Upper".to_string()),
        (
            "outside the name pattern (consecutive hyphens)",
            "double--hyphen".to_string(),
        ),
        (
            "outside the name pattern (leading hyphen)",
            "-leading".to_string(),
        ),
    ];
    for (why, name) in refused_space_names {
        let input = create_space_input(&name, "d-parent");
        assert!(
            aws_smithy_schema_server::validate_to_message(&input, CREATE_SPACE_INPUT_SCHEMA)
                .is_some(),
            "case {why:?}: CreateSpace name {name:?} was accepted"
        );
    }
    // Both published bounds of `name`'s @length(3..64) are accepted.
    for name in ["abc".to_string(), "b".repeat(64)] {
        let input = create_space_input(&name, "d-parent");
        assert_eq!(
            aws_smithy_schema_server::validate_to_message(&input, CREATE_SPACE_INPUT_SCHEMA),
            None,
            "case {name:?}: a name on the published bound was refused"
        );
    }

    let refused_provider_sets: Vec<(&str, Vec<&str>)> = vec![
        ("empty, under @length(min: 1)", vec![]),
        ("over @length(max: 2)", vec!["IAM", "IDC", "IAM"]),
        ("repeated, against @uniqueItems", vec!["IAM", "IAM"]),
        ("outside the IdentityProvider value set", vec!["SAML"]),
    ];
    for (why, providers) in refused_provider_sets {
        let input = create_domain_input("provider-case", &providers);
        assert!(
            aws_smithy_schema_server::validate_to_message(&input, CREATE_DOMAIN_INPUT_SCHEMA)
                .is_some(),
            "case {why:?}: identityProviders {providers:?} was accepted"
        );
    }
    for providers in [vec!["IAM"], vec!["IDC"], vec!["IAM", "IDC"]] {
        let input = create_domain_input("provider-case", &providers);
        assert_eq!(
            aws_smithy_schema_server::validate_to_message(&input, CREATE_DOMAIN_INPUT_SCHEMA),
            None,
            "case {providers:?}: a modelled provider set was refused"
        );
    }
    // `name` on CreateDomain carries @length(3..63), one character tighter than
    // CreateSpace's.
    let input = create_domain_input(&too_long_domain_name, &["IAM"]);
    assert!(
        aws_smithy_schema_server::validate_to_message(&input, CREATE_DOMAIN_INPUT_SCHEMA).is_some(),
        "a 64-character domain name was accepted past @length(max: 63)"
    );

    // `maxResults` carries @range(min: 1, max: 100): both bounds pass, one step past
    // either is refused.
    for bound in [1, 100] {
        let input = ListSpacesInput {
            domain_id: None,
            next_token: None,
            max_results: Some(bound),
        };
        assert_eq!(
            aws_smithy_schema_server::validate_to_message(&input, LIST_SPACES_INPUT_SCHEMA),
            None,
            "case maxResults={bound}: a published bound was refused"
        );
    }
    for bound in [0, 101] {
        let input = ListSpacesInput {
            domain_id: None,
            next_token: None,
            max_results: Some(bound),
        };
        assert!(
            aws_smithy_schema_server::validate_to_message(&input, LIST_SPACES_INPUT_SCHEMA)
                .is_some(),
            "case maxResults={bound}: a value outside @range(1..100) was accepted"
        );
    }
}

#[tokio::test]
async fn the_space_store_is_bounded_and_refuses_past_its_quota() {
    // Same bound for spaces, and a delete frees a slot again.
    //
    // An account holds one space per Region, so each of these spaces is created in a
    // Region of its own: the credential scope's Region is what gives a space its slot,
    // and [`SPACE_QUOTA`] is the store's own bound ABOVE that rule, which is what stops
    // the store growing over Regions without end.
    let omni = Omni::new();
    let parent = domain(&omni, "quota", "omni-space-quota", &["IAM"]).await;
    let input = create_space_input("quota-space", &parent.domain_id);
    let slot = |index: usize| format!("us-quota-{index}");
    let mut last = String::new();
    for index in 0..SPACE_QUOTA {
        last = omni
            .create_space_in_region(&input, &slot(index))
            .await
            .unwrap_or_else(|error| panic!("space {index} within the quota failed: {error:?}"))
            .space
            .space_id;
    }
    expect_modelled_error(
        "one space past SPACE_QUOTA",
        omni.create_space_in_region(&input, &slot(SPACE_QUOTA)).await,
        402,
        "ServiceQuotaExceededException",
    );

    omni.delete_space(&last)
        .await
        .expect("deleting a space releases its slot");
    omni.create_space_in_region(&input, &slot(SPACE_QUOTA + 1))
        .await
        .expect("a freed slot accepts another space");
    expect_modelled_error(
        "full again",
        omni.create_space_in_region(&input, &slot(SPACE_QUOTA + 2))
            .await,
        402,
        "ServiceQuotaExceededException",
    );
}

#[tokio::test]
async fn the_space_lifecycle_over_the_rpcv2cbor_wire() {
    let omni = Omni::new();
    // The parent domain is created through the handler holding the same engine the
    // router's service holds, which is what makes the wire calls below see it.
    let parent = domain(&omni, "wire", "omni-wire", &["IAM"]).await;

    let codec = CborCodec::default();
    let decode = |body: &bytes::Bytes| -> CreateSpaceOutput {
        let mut deserializer = codec.create_deserializer(body);
        CreateSpaceOutput::deserialize(&mut deserializer).expect("the output deserializes")
    };

    let (status, body) = omni
        .over_the_wire(
            "CreateSpace",
            CREATE_SPACE_INPUT_SCHEMA,
            &create_space_input("wire-space", &parent.domain_id),
        )
        .await;
    assert_eq!(
        status,
        200,
        "CreateSpace over the wire answered {status}: {:?}",
        String::from_utf8_lossy(&body)
    );
    let created = decode(&body).space;
    assert!(
        is_space_id(&created.space_id),
        "spaceId {:?} does not satisfy {SPACE_ID_PATTERN}",
        created.space_id
    );

    // The space the wire call created is readable through the handler's store.
    let fetched = omni
        .get_space(&created.space_id)
        .await
        .expect("the space created over the wire is in the store")
        .space;
    assert_eq!(
        (
            fetched.space_id.as_str(),
            fetched.name.as_str(),
            fetched.space_arn.as_str(),
            fetched.domain_arn.as_deref(),
            fetched.region.as_str(),
            fetched.owner_account_id.as_str(),
            fetched.data_access_role_arn.as_str(),
            fetched.status.as_str(),
            format!("{:?}", fetched.encryption_configuration),
        ),
        (
            created.space_id.as_str(),
            created.name.as_str(),
            created.space_arn.as_str(),
            created.domain_arn.as_deref(),
            created.region.as_str(),
            created.owner_account_id.as_str(),
            created.data_access_role_arn.as_str(),
            created.status.as_str(),
            format!("{:?}", created.encryption_configuration),
        ),
        "the wire response and the stored record disagree"
    );
    assert_eq!(
        (millis(fetched.created_at), millis(fetched.updated_at)),
        (millis(created.created_at), millis(created.updated_at)),
        "the wire response and the stored record disagree on the timestamps"
    );

    // And over the wire a space the account does not hold is the modelled 403 carrying
    // the service's one authorization refusal, with no mention of the id it addressed.
    let (status, body) = omni
        .over_the_wire(
            "GetSpace",
            GET_SPACE_INPUT_SCHEMA,
            &GetSpaceInput {
                space_id: MISSING_SPACE_ID.to_string(),
            },
        )
        .await;
    let rendered = String::from_utf8_lossy(&body).to_string();
    assert_eq!(
        status, 403,
        "GetSpace over the wire answered {status} for a space the account does not hold: \
         {rendered:?}"
    );
    assert!(
        rendered.contains("AccessDeniedException"),
        "the wire error body does not name AccessDeniedException: {rendered:?}"
    );
    assert!(
        rendered.contains("Authorization failed"),
        "the wire error body does not carry AWS's authorization refusal: {rendered:?}"
    );
    assert!(
        !rendered.contains(MISSING_SPACE_ID),
        "the wire error body quotes the space id it refused: {rendered:?}"
    );

    // A modelled constraint is refused before the handler runs, as a 400.
    let (status, body) = omni
        .over_the_wire(
            "CreateSpace",
            CREATE_SPACE_INPUT_SCHEMA,
            &create_space_input("ab", &parent.domain_id),
        )
        .await;
    let rendered = String::from_utf8_lossy(&body).to_string();
    assert_eq!(
        status, 400,
        "a name under @length(min: 3) answered {status}: {rendered:?}"
    );
    assert!(
        rendered.contains("ValidationException"),
        "the wire refusal does not name ValidationException: {rendered:?}"
    );

    // ListSpaces over the wire reports the one space the store holds.
    let (status, body) = omni
        .over_the_wire(
            "ListSpaces",
            LIST_SPACES_INPUT_SCHEMA,
            &ListSpacesInput {
                domain_id: Some(parent.domain_id.clone()),
                next_token: None,
                max_results: Some(10),
            },
        )
        .await;
    assert_eq!(
        status,
        200,
        "ListSpaces over the wire answered {status}: {:?}",
        String::from_utf8_lossy(&body)
    );
    let mut deserializer = codec.create_deserializer(&body);
    let listed = ListSpacesOutput::deserialize(&mut deserializer).expect("the output deserializes");
    assert_eq!(
        listed
            .items
            .iter()
            .map(|item| item.space_id.clone())
            .collect::<Vec<String>>(),
        vec![created.space_id.clone()],
        "ListSpaces over the wire did not report the stored space"
    );
    assert!(
        listed.next_token.is_none(),
        "one space under a bound of 10 needs no token, got {:?}",
        listed.next_token
    );
}

/// An `IdentityProviderConfiguration` naming one Identity Center instance.
///
/// The ARN satisfies `IdentityCenterInstanceArn`'s modelled
/// `^arn:aws[a-z-]*:sso:::instance/[a-zA-Z0-9-]+$`.
fn idc_configuration(instance_arn: &str) -> IdentityProviderConfiguration {
    IdentityProviderConfiguration {
        identity_center_configuration: Some(IdentityCenterConfiguration {
            identity_center_instance_arn: Some(instance_arn.to_string()),
        }),
    }
}

/// One Identity Center instance ARN.
const IDC_INSTANCE_ARN: &str = "arn:aws:sso:::instance/ssoins-1234567890abcdef";

/// A second one, so a case can move a domain's Identity Center instance.
const OTHER_IDC_INSTANCE_ARN: &str = "arn:aws:sso:::instance/ssoins-fedcba0987654321";

/// The endpoint URL a domain of this name is addressed by.
///
/// `Domain.customEndpointUrls` is "Additional endpoint URLs derived from the domain
/// name", against `domainEndpointUrl`'s "The HTTPS endpoint URL for accessing the
/// domain", which [`create_domain_persists_a_record_get_domain_returns_it_verbatim`]
/// already pins to the domain's ID. Each is an `https` URL on the address THIS emulator was
/// reached at — which for an in-process call is the unserved base
/// ([`smithy_server_runtime::UNSERVED_URL_BASE`]) this suite drives the handlers through —
/// with that identity in the path this emulator serves the domain under.
fn custom_endpoint_url(name: &str) -> String {
    announced_endpoint_url(&omni::state::domain_name_endpoint_path(name))
}

/// The endpoint URL a domain of this ID is addressed by, read the same way.
fn domain_endpoint_url(domain_id: &str) -> String {
    announced_endpoint_url(&omni::state::domain_endpoint_path(domain_id))
}

/// The URL the endpoint served at `path` is announced as, for the unserved base this suite's
/// in-process calls are answered on.
fn announced_endpoint_url(path: &str) -> String {
    let unserved = smithy_server_runtime::UNSERVED_URL_BASE
        .split_once("://")
        .map_or(smithy_server_runtime::UNSERVED_URL_BASE, |(_, rest)| rest);
    format!("https://{unserved}{path}")
}

/// The members of a domain that `UpdateDomain`'s input cannot name, rendered for
/// comparison: a patch must bring every one of them through untouched.
fn untouched_domain(domain: &Domain) -> String {
    format!(
        "{:?}",
        (
            &domain.domain_id,
            &domain.domain_arn,
            &domain.domain_endpoint_url,
            &domain.region,
            &domain.status,
            domain.created_at.secs(),
            domain.created_at.subsec_nanos(),
        )
    )
}

/// The identity providers of a domain, as a row writes them.
fn providers(names: &[&str]) -> Vec<String> {
    names.iter().map(|name| (*name).to_string()).collect()
}

/// One row of the `UpdateDomain` partial-update table.
struct DomainPatch {
    case: &'static str,
    /// The domain to start from.
    initial_name: String,
    initial_providers: Vec<&'static str>,
    initial_instance: Option<&'static str>,
    /// The members the `UpdateDomain` request carries.
    patch_name: Option<String>,
    patch_providers: Option<Vec<&'static str>>,
    patch_instance: Option<&'static str>,
    /// What the domain must report afterwards.
    expected_name: String,
    expected_providers: Vec<&'static str>,
}

fn domain_patch_table() -> Vec<DomainPatch> {
    // Rows differ along WHICH members the patch carries (each one alone, all three at
    // once, and none at all), along the shape of the domain they start from (IAM-only,
    // IDC with and without a named instance, both providers), along the new name's size
    // — "abc" and a 63-character name are both published bounds of the member's modelled
    // `@length(3..63)` — and along the provider count and order.
    let longest_name = "a".repeat(63);
    vec![
        DomainPatch {
            case: "a name-only patch keeps the providers",
            initial_name: "omni-patch-one".to_string(),
            initial_providers: vec!["IAM"],
            initial_instance: None,
            patch_name: Some("omni-patch-one-renamed".to_string()),
            patch_providers: None,
            patch_instance: None,
            expected_name: "omni-patch-one-renamed".to_string(),
            expected_providers: vec!["IAM"],
        },
        DomainPatch {
            case: "a rename down to the shortest admissible name",
            initial_name: "omni-patch-two".to_string(),
            initial_providers: vec!["IDC"],
            initial_instance: None,
            patch_name: Some("abc".to_string()),
            patch_providers: None,
            patch_instance: None,
            expected_name: "abc".to_string(),
            expected_providers: vec!["IDC"],
        },
        DomainPatch {
            case: "a rename up to the longest admissible name",
            initial_name: "omni-patch-three".to_string(),
            initial_providers: vec!["IAM", "IDC"],
            initial_instance: Some(IDC_INSTANCE_ARN),
            patch_name: Some(longest_name.clone()),
            patch_providers: None,
            patch_instance: None,
            expected_name: longest_name,
            expected_providers: vec!["IAM", "IDC"],
        },
        DomainPatch {
            case: "a providers-only patch adds IDC and keeps the name",
            initial_name: "omni-patch-four".to_string(),
            initial_providers: vec!["IAM"],
            initial_instance: Some(IDC_INSTANCE_ARN),
            patch_name: None,
            patch_providers: Some(vec!["IAM", "IDC"]),
            patch_instance: None,
            expected_name: "omni-patch-four".to_string(),
            expected_providers: vec!["IAM", "IDC"],
        },
        DomainPatch {
            case: "a providers-only patch drops IDC",
            initial_name: "omni-patch-five".to_string(),
            initial_providers: vec!["IDC"],
            initial_instance: Some(IDC_INSTANCE_ARN),
            patch_name: None,
            patch_providers: Some(vec!["IAM"]),
            patch_instance: None,
            expected_name: "omni-patch-five".to_string(),
            expected_providers: vec!["IAM"],
        },
        DomainPatch {
            case: "a configuration-only patch moves the Identity Center instance",
            initial_name: "omni-patch-six".to_string(),
            initial_providers: vec!["IDC"],
            initial_instance: Some(IDC_INSTANCE_ARN),
            patch_name: None,
            patch_providers: None,
            patch_instance: Some(OTHER_IDC_INSTANCE_ARN),
            expected_name: "omni-patch-six".to_string(),
            expected_providers: vec!["IDC"],
        },
        DomainPatch {
            case: "every member at once",
            initial_name: "omni-patch-seven".to_string(),
            initial_providers: vec!["IAM"],
            initial_instance: None,
            patch_name: Some("omni-patch-seven-renamed".to_string()),
            patch_providers: Some(vec!["IDC", "IAM"]),
            patch_instance: Some(IDC_INSTANCE_ARN),
            expected_name: "omni-patch-seven-renamed".to_string(),
            expected_providers: vec!["IDC", "IAM"],
        },
        DomainPatch {
            case: "a patch carrying no member at all",
            initial_name: "omni-patch-eight".to_string(),
            initial_providers: vec!["IAM", "IDC"],
            initial_instance: Some(IDC_INSTANCE_ARN),
            patch_name: None,
            patch_providers: None,
            patch_instance: None,
            expected_name: "omni-patch-eight".to_string(),
            expected_providers: vec!["IAM", "IDC"],
        },
    ]
}

#[tokio::test]
async fn update_domain_changes_only_the_members_the_request_carries() {
    for row in domain_patch_table() {
        let case = row.case;
        // An account holds at most ONE domain, so each row patches the domain of an
        // account of its own.
        let omni = Omni::new();
        let mut create = create_domain_input(&row.initial_name, &row.initial_providers);
        create.identity_provider_configuration = row.initial_instance.map(idc_configuration);
        let created = omni
            .create_domain(&create)
            .await
            .unwrap_or_else(|error| panic!("case {case:?}: CreateDomain failed: {error:?}"))
            .domain;

        let patch = UpdateDomainInput {
            domain_id: created.domain_id.clone(),
            name: row.patch_name.clone(),
            identity_providers: row.patch_providers.as_deref().map(providers),
            identity_provider_configuration: row.patch_instance.map(idc_configuration),
        };
        let updated = omni
            .update_domain(&patch)
            .await
            .unwrap_or_else(|error| panic!("case {case:?}: UpdateDomain failed: {error:?}"))
            .domain;

        assert_eq!(
            updated.name.as_deref(),
            Some(row.expected_name.as_str()),
            "case {case:?}: expected name {:?}, got {:?}",
            row.expected_name,
            updated.name
        );
        assert_eq!(
            updated.identity_providers,
            providers(&row.expected_providers),
            "case {case:?}: expected identityProviders {:?}, got {:?}",
            row.expected_providers,
            updated.identity_providers
        );
        // The name's endpoint URLs are derived, so they follow the name the patch left.
        let custom = updated.custom_endpoint_urls.clone().unwrap_or_default();
        assert_eq!(
            custom,
            vec![custom_endpoint_url(&row.expected_name)],
            "case {case:?}: customEndpointUrls {custom:?} do not follow the name {:?}",
            row.expected_name
        );
        // The members built from the domain's ID, and every member this input cannot
        // name, come through a rename untouched.
        assert_eq!(
            untouched_domain(&updated),
            untouched_domain(&created),
            "case {case:?}: a member UpdateDomain's input cannot name was changed"
        );
        // `identityCenterApplicationArn` is "Absent for IAM-only domains".
        let expects_application = row.expected_providers.contains(&"IDC");
        assert_eq!(
            updated.identity_center_application_arn.is_some(),
            expects_application,
            "case {case:?}: identityCenterApplicationArn presence {:?} does not follow the IDC \
             provider",
            updated.identity_center_application_arn
        );
        if let Some(application) = updated.identity_center_application_arn.as_deref() {
            assert!(
                is_arn(application),
                "case {case:?}: identityCenterApplicationArn {application:?} does not satisfy \
                 {ARN_PATTERN}"
            );
        }

        // `updatedAt` moves when a member of the domain moves, so a request carrying no
        // member leaves it exactly where the create left it.
        let requested_anything = row.patch_name.is_some()
            || row.patch_providers.is_some()
            || row.patch_instance.is_some();
        if requested_anything {
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
                "case {case:?}: a patch carrying no member moved updatedAt to {:?}",
                updated.updated_at
            );
        }

        // The patched record is the stored one: `GetDomain` answers from it rather than
        // from a second copy.
        let fetched = omni
            .get_domain(&created.domain_id)
            .await
            .unwrap_or_else(|error| panic!("case {case:?}: GetDomain failed: {error:?}"))
            .domain;
        assert_eq!(
            format!("{fetched:?}"),
            format!("{updated:?}"),
            "case {case:?}: GetDomain did not return the record UpdateDomain persisted"
        );

        // `UpdateDomain` is modelled `@idempotent`: replaying one patch answers the same
        // domain rather than moving it again.
        let replayed = omni
            .update_domain(&patch)
            .await
            .unwrap_or_else(|error| panic!("case {case:?}: replaying UpdateDomain failed: {error:?}"))
            .domain;
        assert_eq!(
            format!("{replayed:?}"),
            format!("{updated:?}"),
            "case {case:?}: replaying the patch answered a different domain"
        );
    }
}

#[tokio::test]
async fn the_identity_center_application_follows_the_domains_providers() {
    let omni = Omni::new();
    let mut create = create_domain_input("omni-application", &["IDC"]);
    create.identity_provider_configuration = Some(idc_configuration(IDC_INSTANCE_ARN));
    let created = omni
        .create_domain(&create)
        .await
        .expect("CreateDomain with IDC succeeds")
        .domain;
    let minted = created
        .identity_center_application_arn
        .clone()
        .expect("an IDC domain reports an Identity Center application");
    // The application is minted under the instance the request named.
    assert!(
        minted.contains("ssoins-1234567890abcdef"),
        "case \"minted\": the application {minted:?} is not under the named instance"
    );

    // A rename touches nothing about the domain's identity, so the application stays.
    let renamed = omni
        .update_domain(&UpdateDomainInput {
            domain_id: created.domain_id.clone(),
            name: Some("omni-application-renamed".to_string()),
            ..Default::default()
        })
        .await
        .expect("the rename succeeds")
        .domain;
    assert_eq!(
        renamed.identity_center_application_arn.as_deref(),
        Some(minted.as_str()),
        "case \"rename\": the rename moved the Identity Center application to {:?}",
        renamed.identity_center_application_arn
    );

    // Naming another instance moves the application under that instance.
    let moved = omni
        .update_domain(&UpdateDomainInput {
            domain_id: created.domain_id.clone(),
            identity_provider_configuration: Some(idc_configuration(OTHER_IDC_INSTANCE_ARN)),
            ..Default::default()
        })
        .await
        .expect("moving the instance succeeds")
        .domain;
    let after_move = moved
        .identity_center_application_arn
        .clone()
        .expect("a domain still listing IDC reports an application");
    assert!(
        after_move.contains("ssoins-fedcba0987654321"),
        "case \"moved\": the application {after_move:?} is not under the instance named"
    );

    // Dropping IDC drops the application: it is "Absent for IAM-only domains".
    let iam_only = omni
        .update_domain(&UpdateDomainInput {
            domain_id: created.domain_id.clone(),
            identity_providers: Some(providers(&["IAM"])),
            ..Default::default()
        })
        .await
        .expect("dropping IDC succeeds")
        .domain;
    assert_eq!(
        iam_only.identity_center_application_arn, None,
        "case \"IAM-only\": the application survived dropping IDC: {:?}",
        iam_only.identity_center_application_arn
    );

    // And listing IDC again mints one.
    let idc_again = omni
        .update_domain(&UpdateDomainInput {
            domain_id: created.domain_id.clone(),
            identity_providers: Some(providers(&["IDC", "IAM"])),
            ..Default::default()
        })
        .await
        .expect("adding IDC back succeeds")
        .domain;
    assert!(
        idc_again
            .identity_center_application_arn
            .as_deref()
            .is_some_and(is_arn),
        "case \"IDC again\": no application was minted: {:?}",
        idc_again.identity_center_application_arn
    );
}

#[tokio::test]
async fn renaming_a_domain_to_the_name_it_holds_is_not_a_conflict_with_itself() {
    let omni = Omni::new();
    // An account holds at most one domain, so no SIBLING domain can hold the name a
    // rename asks for: what is reachable here is the rename of the account's own domain,
    // and the patch that asks for the name it already carries.
    let held = domain(&omni, "conflict", "omni-conflict-first", &["IAM"]).await;
    let before = format!("{held:?}");

    // Naming the name the domain already carries is not a conflict with itself, and
    // changes nothing.
    let same = omni
        .update_domain(&UpdateDomainInput {
            domain_id: held.domain_id.clone(),
            name: held.name.clone(),
            ..Default::default()
        })
        .await
        .expect("renaming a domain to its own name succeeds")
        .domain;
    assert_eq!(
        format!("{same:?}"),
        before,
        "a patch asking for the name the domain holds changed it"
    );

    // A rename to another name goes through, and the name it released is free for the
    // account's next domain once this one is deleted.
    let renamed = omni
        .update_domain(&UpdateDomainInput {
            domain_id: held.domain_id.clone(),
            name: Some("omni-conflict-third".to_string()),
            ..Default::default()
        })
        .await
        .expect("the rename succeeds")
        .domain;
    assert_eq!(
        renamed.name.as_deref(),
        Some("omni-conflict-third"),
        "the rename did not take the name it asked for, got {:?}",
        renamed.name
    );
    omni.delete_domain(&held.domain_id)
        .await
        .expect("the renamed domain deletes");
    let reused = omni
        .create_domain(&create_domain_input("omni-conflict-first", &["IAM"]))
        .await
        .expect("the released name can be taken again")
        .domain;
    assert_ne!(
        reused.domain_id, held.domain_id,
        "taking the released name must mint a new domain, not resurrect the old one"
    );
}

#[tokio::test]
async fn updating_a_domain_the_store_does_not_hold_is_not_found() {
    let omni = Omni::new();
    // Well-formed `DomainId`s that name nothing: one the store never held, and one whose
    // domain was deleted.
    let deleted = domain(&omni, "deleted", "omni-update-deleted", &["IAM"]).await;
    omni.delete_domain(&deleted.domain_id)
        .await
        .expect("the domain deletes");

    for domain_id in [MISSING_DOMAIN_ID, deleted.domain_id.as_str()] {
        expect_modelled_error(
            domain_id,
            omni.update_domain(&UpdateDomainInput {
                domain_id: domain_id.to_string(),
                name: Some("omni-update-missing".to_string()),
                ..Default::default()
            })
            .await,
            404,
            "ResourceNotFoundException",
        );
    }

    // A miss is not a write: the name the misses asked for is still free, and a domain
    // created after them still updates.
    let created = omni
        .create_domain(&create_domain_input("omni-update-missing", &["IAM"]))
        .await
        .expect("the name a missed patch asked for was never taken")
        .domain;
    let updated = omni
        .update_domain(&UpdateDomainInput {
            domain_id: created.domain_id.clone(),
            name: Some("omni-update-after-misses".to_string()),
            ..Default::default()
        })
        .await
        .expect("the domain created after the misses updates")
        .domain;
    assert_eq!(
        updated.name.as_deref(),
        Some("omni-update-after-misses"),
        "the update after the misses did not persist, got {:?}",
        updated.name
    );
}

#[tokio::test]
async fn an_identity_provider_outside_the_modelled_value_set_is_refused() {
    let omni = Omni::new();
    let created = domain(&omni, "providers", "omni-bad-provider", &["IAM"]).await;
    let before = format!("{created:?}");

    // `IdentityProvider`'s value set is exactly IAM | IDC, so nothing else is a provider
    // this domain can be configured with.
    for provider in ["SAML", "iam", "IDC2", ""] {
        expect_modelled_error(
            provider,
            omni.update_domain(&UpdateDomainInput {
                domain_id: created.domain_id.clone(),
                name: Some("omni-bad-provider-renamed".to_string()),
                identity_providers: Some(vec![provider.to_string()]),
                ..Default::default()
            })
            .await,
            400,
            "ValidationException",
        );
        let after = omni
            .get_domain(&created.domain_id)
            .await
            .unwrap_or_else(|error| panic!("case {provider:?}: GetDomain failed: {error:?}"))
            .domain;
        assert_eq!(
            format!("{after:?}"),
            before,
            "case {provider:?}: a refused UpdateDomain changed the domain"
        );
    }
}

#[tokio::test]
async fn the_modelled_update_domain_constraints_refuse_what_they_declare() {
    // `aws_smithy_schema_server::validate_to_message` is the exact check the framework
    // runs on a deserialized input before the handler sees it, so the handler never
    // re-checks a modelled constraint.
    let too_long = "a".repeat(64);
    let refused: Vec<(&str, String)> = vec![
        ("shorter than @length(min: 3)", "ab".to_string()),
        ("longer than @length(max: 63)", too_long),
        ("outside the name pattern (upper case)", "Upper".to_string()),
        (
            "outside the name pattern (consecutive hyphens)",
            "double--hyphen".to_string(),
        ),
        (
            "outside the name pattern (leading hyphen)",
            "-leading".to_string(),
        ),
        (
            "outside the name pattern (trailing hyphen)",
            "trailing-".to_string(),
        ),
    ];
    for (why, name) in refused {
        let input = UpdateDomainInput {
            domain_id: MISSING_DOMAIN_ID.to_string(),
            name: Some(name.clone()),
            ..Default::default()
        };
        assert!(
            aws_smithy_schema_server::validate_to_message(&input, UPDATE_DOMAIN_INPUT_SCHEMA)
                .is_some(),
            "case {why:?}: UpdateDomain name {name:?} was accepted"
        );
    }
    // Both published bounds of `name`'s @length(3..63) are accepted.
    for name in ["abc".to_string(), "a".repeat(63)] {
        let input = UpdateDomainInput {
            domain_id: MISSING_DOMAIN_ID.to_string(),
            name: Some(name.clone()),
            ..Default::default()
        };
        assert_eq!(
            aws_smithy_schema_server::validate_to_message(&input, UPDATE_DOMAIN_INPUT_SCHEMA),
            None,
            "case {name:?}: a name on the published bound was refused"
        );
    }
    // `domainId` carries `DomainId`'s pattern, and `identityProviders` its
    // `@length(1..2)` `@uniqueItems`.
    let outside: Vec<(&str, UpdateDomainInput)> = vec![
        (
            "a domainId outside the modelled pattern",
            UpdateDomainInput {
                domain_id: "not-a-domain-id".to_string(),
                ..Default::default()
            },
        ),
        (
            "an empty identityProviders list",
            UpdateDomainInput {
                domain_id: MISSING_DOMAIN_ID.to_string(),
                identity_providers: Some(Vec::new()),
                ..Default::default()
            },
        ),
        (
            "a repeated identity provider",
            UpdateDomainInput {
                domain_id: MISSING_DOMAIN_ID.to_string(),
                identity_providers: Some(providers(&["IAM", "IAM"])),
                ..Default::default()
            },
        ),
        (
            "more identity providers than @length(max: 2)",
            UpdateDomainInput {
                domain_id: MISSING_DOMAIN_ID.to_string(),
                identity_providers: Some(providers(&["IAM", "IDC", "IAM"])),
                ..Default::default()
            },
        ),
    ];
    for (why, input) in outside {
        assert!(
            aws_smithy_schema_server::validate_to_message(&input, UPDATE_DOMAIN_INPUT_SCHEMA)
                .is_some(),
            "case {why:?}: the input was accepted"
        );
    }
}

#[tokio::test]
async fn update_domain_over_the_rpcv2cbor_wire() {
    let omni = Omni::new();
    let mut create = create_domain_input("omni-wire-domain", &["IAM", "IDC"]);
    create.identity_provider_configuration = Some(idc_configuration(IDC_INSTANCE_ARN));
    let (status, body) = omni
        .over_the_wire("CreateDomain", CREATE_DOMAIN_INPUT_SCHEMA, &create)
        .await;
    assert_eq!(
        status,
        200,
        "CreateDomain over the wire answered {status}: {:?}",
        String::from_utf8_lossy(&body)
    );
    let codec = CborCodec::default();
    let mut deserializer = codec.create_deserializer(&body);
    let created = CreateDomainOutput::deserialize(&mut deserializer)
        .expect("the output deserializes")
        .domain;

    let (status, body) = omni
        .over_the_wire(
            "UpdateDomain",
            UPDATE_DOMAIN_INPUT_SCHEMA,
            &UpdateDomainInput {
                domain_id: created.domain_id.clone(),
                name: Some("omni-wire-domain-renamed".to_string()),
                ..Default::default()
            },
        )
        .await;
    assert_eq!(
        status,
        200,
        "UpdateDomain over the wire answered {status}: {:?}",
        String::from_utf8_lossy(&body)
    );
    let mut deserializer = codec.create_deserializer(&body);
    let patched = UpdateDomainOutput::deserialize(&mut deserializer)
        .expect("the output deserializes")
        .domain;
    assert_eq!(
        (
            patched.name.as_deref(),
            patched.custom_endpoint_urls.clone().unwrap_or_default(),
            patched.domain_endpoint_url.as_str(),
            patched.identity_providers.clone(),
        ),
        (
            Some("omni-wire-domain-renamed"),
            vec![custom_endpoint_url("omni-wire-domain-renamed")],
            created.domain_endpoint_url.as_str(),
            providers(&["IAM", "IDC"]),
        ),
        "the wire response did not move the name's endpoint URLs and keep the rest: {patched:?}"
    );
    assert_eq!(
        patched.identity_center_application_arn,
        created.identity_center_application_arn,
        "the rename moved the Identity Center application to {:?}",
        patched.identity_center_application_arn
    );

    // And a miss over the wire is the modelled 404 with its modelled error code.
    let (status, body) = omni
        .over_the_wire(
            "UpdateDomain",
            UPDATE_DOMAIN_INPUT_SCHEMA,
            &UpdateDomainInput {
                domain_id: MISSING_DOMAIN_ID.to_string(),
                name: Some("omni-wire-domain-missing".to_string()),
                ..Default::default()
            },
        )
        .await;
    let rendered = String::from_utf8_lossy(&body).to_string();
    assert_eq!(
        status, 404,
        "UpdateDomain over the wire answered {status} for a missing domain: {rendered:?}"
    );
    assert!(
        rendered.contains("ResourceNotFoundException"),
        "the wire error body does not name ResourceNotFoundException: {rendered:?}"
    );
}

/// The message the operation's modelled `ConflictException` carries, or a panic naming
/// what came back instead.
fn delete_conflict_message<O: std::fmt::Debug>(
    case: &str,
    result: Result<O, DeleteDomainError>,
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
                Scaffolded::Modeled(DeleteDomainErrorModeled::ConflictException(refusal)) => {
                    refusal.message
                }
                other => {
                    panic!("case {case:?}: expected the modelled ConflictException, got {other:?}")
                }
            }
        }
    }
}

/// The message `UpdateDomain`'s modelled `ValidationException` carries, or a panic naming
/// what came back instead.
fn patch_validation_message<O: std::fmt::Debug>(
    case: &str,
    result: Result<O, UpdateDomainError>,
) -> String {
    match result {
        Ok(output) => panic!("case {case:?}: expected a ValidationException, got Ok({output:?})"),
        Err(error) => {
            assert_eq!(
                error.http_status(),
                400,
                "case {case:?}: expected HTTP 400, got {} ({error:?})",
                error.http_status()
            );
            match error {
                Scaffolded::Modeled(UpdateDomainErrorModeled::ValidationException(refusal)) => {
                    refusal.message
                }
                other => panic!(
                    "case {case:?}: expected the modelled ValidationException, got {other:?}"
                ),
            }
        }
    }
}

#[tokio::test]
async fn a_domain_that_still_holds_spaces_is_refused_with_the_count_it_holds() {
    let omni = Omni::new();
    let parent = domain(&omni, "not empty", "omni-not-empty", &["IAM"]).await;

    // Rows walk the count UP and then back DOWN, so the number in the message is read
    // from the store on each call rather than fixed or remembered: one space (the
    // smallest count that blocks the delete) and then a second and a third.
    let mut held: Vec<String> = Vec::new();
    for count in 1..=3_usize {
        held.push(
            space(
                &omni,
                "not empty",
                &format!("omni-not-empty-{count}"),
                &parent.domain_id,
            )
            .await
            .space_id,
        );
        let case = format!("{count} space(s) in the domain");
        let expected = format!(
            "Cannot delete Domain {} because it has {count} associated Spaces. Delete all \
             Spaces first.",
            parent.domain_id
        );
        let message = delete_conflict_message(&case, omni.delete_domain(&parent.domain_id).await);
        assert_eq!(
            message, expected,
            "case {case:?}: expected {expected:?}, got {message:?}"
        );
        // The template is the service's one sentence for this class, so the expectation
        // above is the one the shared template renders.
        assert_eq!(
            message,
            omni::state::domain_holds_spaces_message(&parent.domain_id, count),
            "case {case:?}: the refusal does not state the service's template"
        );
        // The refused delete left the domain in place and ACTIVE.
        let still_there = omni
            .get_domain(&parent.domain_id)
            .await
            .unwrap_or_else(|error| panic!("case {case:?}: GetDomain failed: {error:?}"))
            .domain;
        assert_eq!(
            still_there.status, "ACTIVE",
            "case {case:?}: the refused delete moved the domain's status to {:?}",
            still_there.status
        );
    }

    // Removing the spaces one at a time walks the count back down, and the last removal
    // frees the domain.
    while let Some(space_id) = held.pop() {
        omni.delete_space(&space_id)
            .await
            .unwrap_or_else(|error| panic!("DeleteSpace failed: {error:?}"));
        let remaining = held.len();
        if remaining == 0 {
            break;
        }
        let case = format!("{remaining} space(s) left in the domain");
        let expected = format!(
            "Cannot delete Domain {} because it has {remaining} associated Spaces. Delete all \
             Spaces first.",
            parent.domain_id
        );
        let message = delete_conflict_message(&case, omni.delete_domain(&parent.domain_id).await);
        assert_eq!(
            message, expected,
            "case {case:?}: expected {expected:?}, got {message:?}"
        );
    }
    omni.delete_domain(&parent.domain_id)
        .await
        .expect("the domain deletes once its last space is gone");
}

/// One row of the Identity Center precondition table: a patch of a domain that starts out
/// with `initial_providers` and, where `initial_instance` names one, an Identity Center
/// instance.
struct FederationPatch {
    case: &'static str,
    initial_providers: Vec<&'static str>,
    initial_instance: Option<&'static str>,
    patch_providers: Option<Vec<&'static str>>,
    /// The `identityProviderConfiguration` the patch carries: `None` for a patch that
    /// omits the member, `Some(None)` for one that carries it naming no instance.
    patch_configuration: Option<Option<&'static str>>,
    patch_name: Option<&'static str>,
    /// Whether the patch must be refused.
    refused: bool,
}

fn federation_patch_table() -> Vec<FederationPatch> {
    // Rows differ along which members the patch carries (the provider list, the
    // configuration, the name, and combinations), along the provider sets the model
    // admits (`IdentityProviderList` @length(1..2) @uniqueItems over [IAM, IDC]) and
    // along whether an instance is reachable from the request or from the domain.
    vec![
        FederationPatch {
            case: "switching to IDC with no instance anywhere",
            initial_providers: vec!["IAM"],
            initial_instance: None,
            patch_providers: Some(vec!["IDC"]),
            patch_configuration: None,
            patch_name: None,
            refused: true,
        },
        FederationPatch {
            case: "adding IDC beside IAM with no instance anywhere",
            initial_providers: vec!["IAM"],
            initial_instance: None,
            patch_providers: Some(vec!["IAM", "IDC"]),
            patch_configuration: None,
            patch_name: None,
            refused: true,
        },
        FederationPatch {
            case: "switching to IDC with a configuration naming no instance",
            initial_providers: vec!["IAM"],
            initial_instance: None,
            patch_providers: Some(vec!["IDC"]),
            patch_configuration: Some(None),
            patch_name: None,
            refused: true,
        },
        FederationPatch {
            case: "renaming and switching to IDC with no instance",
            initial_providers: vec!["IAM"],
            initial_instance: None,
            patch_providers: Some(vec!["IDC", "IAM"]),
            patch_configuration: None,
            patch_name: Some("omni-federation-renamed"),
            refused: true,
        },
        FederationPatch {
            case: "switching to IDC and naming an instance",
            initial_providers: vec!["IAM"],
            initial_instance: None,
            patch_providers: Some(vec!["IDC"]),
            patch_configuration: Some(Some(IDC_INSTANCE_ARN)),
            patch_name: None,
            refused: false,
        },
        FederationPatch {
            case: "adding IDC to a domain that already holds an instance",
            initial_providers: vec!["IAM"],
            initial_instance: Some(IDC_INSTANCE_ARN),
            patch_providers: Some(vec!["IAM", "IDC"]),
            patch_configuration: None,
            patch_name: None,
            refused: false,
        },
        FederationPatch {
            case: "dropping IDC needs no instance",
            initial_providers: vec!["IDC"],
            initial_instance: Some(IDC_INSTANCE_ARN),
            patch_providers: Some(vec!["IAM"]),
            patch_configuration: None,
            patch_name: None,
            refused: false,
        },
        FederationPatch {
            case: "a rename alone touches no federation",
            initial_providers: vec!["IDC"],
            initial_instance: None,
            patch_providers: None,
            patch_configuration: None,
            patch_name: Some("omni-federation-rename-only"),
            refused: false,
        },
        FederationPatch {
            case: "moving the instance of a domain that lists IDC",
            initial_providers: vec!["IDC"],
            initial_instance: Some(IDC_INSTANCE_ARN),
            patch_providers: None,
            patch_configuration: Some(Some(OTHER_IDC_INSTANCE_ARN)),
            patch_name: None,
            refused: false,
        },
    ]
}

#[tokio::test]
async fn a_patch_that_would_leave_idc_without_an_instance_is_refused_and_writes_nothing() {
    for row in federation_patch_table() {
        let case = row.case;
        // An account holds at most ONE domain, so each row patches the domain of an
        // account of its own.
        let omni = Omni::new();
        let mut create = create_domain_input("omni-federation", &row.initial_providers);
        create.identity_provider_configuration = row.initial_instance.map(idc_configuration);
        let created = omni
            .create_domain(&create)
            .await
            .unwrap_or_else(|error| panic!("case {case:?}: CreateDomain failed: {error:?}"))
            .domain;
        let before = format!("{created:?}");

        let patch = UpdateDomainInput {
            domain_id: created.domain_id.clone(),
            name: row.patch_name.map(str::to_string),
            identity_providers: row.patch_providers.as_deref().map(providers),
            identity_provider_configuration: row.patch_configuration.map(|instance| {
                instance.map_or_else(IdentityProviderConfiguration::default, idc_configuration)
            }),
        };
        let result = omni.update_domain(&patch).await;

        if !row.refused {
            result
                .unwrap_or_else(|error| panic!("case {case:?}: the patch was refused: {error:?}"));
            continue;
        }

        let message = patch_validation_message(case, result);
        assert_eq!(
            message, IDENTITY_CENTER_INSTANCE_REQUIRED,
            "case {case:?}: expected {IDENTITY_CENTER_INSTANCE_REQUIRED:?}, got {message:?}"
        );
        // The refusal wrote nothing: the domain still reports what it held, including the
        // provider list and the absence of an Identity Center application.
        let after = omni
            .get_domain(&created.domain_id)
            .await
            .unwrap_or_else(|error| panic!("case {case:?}: GetDomain failed: {error:?}"))
            .domain;
        assert_eq!(
            format!("{after:?}"),
            before,
            "case {case:?}: the refused patch changed the domain"
        );
        assert_eq!(
            after.identity_center_application_arn, None,
            "case {case:?}: an Identity Center application was minted for a domain with no \
             instance: {:?}",
            after.identity_center_application_arn
        );
    }
}

#[tokio::test]
async fn both_refusals_carry_their_message_over_the_rpcv2cbor_wire() {
    let omni = Omni::new();
    let parent = domain(&omni, "wire refusals", "omni-wire-refusals", &["IAM"]).await;
    space(&omni, "wire refusals", "omni-wire-refusals-a", &parent.domain_id).await;

    let expected = format!(
        "Cannot delete Domain {} because it has 1 associated Spaces. Delete all Spaces first.",
        parent.domain_id
    );
    let (status, body) = omni
        .over_the_wire(
            "DeleteDomain",
            DELETE_DOMAIN_INPUT_SCHEMA,
            &DeleteDomainInput {
                domain_id: parent.domain_id.clone(),
            },
        )
        .await;
    let (code, message) = error_envelope("a domain that still holds a space", &body);
    assert_eq!(
        (status, code.as_str(), message.as_str()),
        (409, "ConflictException", expected.as_str()),
        "the wire must answer 409 ConflictException with {expected:?}, got {status} {code:?}: \
         {message:?}"
    );

    let (status, body) = omni
        .over_the_wire(
            "UpdateDomain",
            UPDATE_DOMAIN_INPUT_SCHEMA,
            &UpdateDomainInput {
                domain_id: parent.domain_id.clone(),
                identity_providers: Some(providers(&["IDC"])),
                ..Default::default()
            },
        )
        .await;
    let (code, message) = error_envelope("a switch to IDC with no instance", &body);
    assert_eq!(
        (status, code.as_str(), message.as_str()),
        (400, "ValidationException", IDENTITY_CENTER_INSTANCE_REQUIRED),
        "the wire must answer 400 ValidationException with \
         {IDENTITY_CENTER_INSTANCE_REQUIRED:?}, got {status} {code:?}: {message:?}"
    );
}
