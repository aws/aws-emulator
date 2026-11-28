// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the channel-contract plane, and for the managed-services
//! members the same model revision added to the account and relationship planes.
//!
//! Every case drives the service the way a client does: an awsJson1_0 `POST /` with
//! `X-Amz-Target: PartnerCentralChannel.<Operation>`, dispatched through the same
//! [`MultiServiceRouter`] the emulator binary mounts. Nothing reaches into the store
//! directly, so a response that is not computed from persisted state fails here.

use serde_json::{json, Value};
use service_com_amazonaws_partnercentralchannel_partnercentralchannel::{
    ComAmazonawsPartnercentralchannelPartnercentralchannelState, EmulatedService,
};
use smithy_server_runtime::MultiServiceRouter;
use std::sync::Arc;

/// A router with one freshly-engined copy of this service mounted, for a caller
/// that holds an active AWS Partner benefit.
fn emulator() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    engine
        .state::<ComAmazonawsPartnercentralchannelPartnercentralchannelState>()
        .lock()
        .grant_partner_entitlement(engine::emulator_account_id());
    mount(engine)
}

/// A router whose calling account holds no AWS Partner benefit, for the cases
/// about the entitlement gate.
fn emulator_without_entitlement() -> MultiServiceRouter {
    mount(Arc::new(engine::InMemoryEngine::new()))
}

fn mount(engine: Arc<engine::InMemoryEngine>) -> MultiServiceRouter {
    let mut router = MultiServiceRouter::new();
    // Deterministic regardless of the process-wide default: these cases are about
    // the service's behaviour, not about SigV4.
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(engine));
    router
}

/// The region every case signs for.
const REGION: &str = "us-east-1";

/// Call one operation, and return `(http status, parsed body)`.
async fn call(router: &MultiServiceRouter, operation: &str, body: &Value) -> (u16, Value) {
    let request = http::Request::builder()
        .method("POST")
        .uri(format!("https://partnercentral-channel.{REGION}.amazonaws.com/"))
        .header("content-type", "application/x-amz-json-1.0")
        .header("x-amz-target", format!("PartnerCentralChannel.{operation}"))
        // The router keys on the SigV4 credential scope's service field
        // (`partnercentral-channel`, this service's signing name) and the service
        // reads the region segment out of the same scope. The signature itself is
        // not checked, because authentication is off above.
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIAEXAMPLEEXAMPLE/20260101/{REGION}/\
                 partnercentral-channel/aws4_request, SignedHeaders=host;x-amz-target, \
                 Signature=0"
            ),
        )
        .body(bytes::Bytes::from(
            serde_json::to_vec(body).expect("the request body serializes"),
        ))
        .expect("a well-formed request");

    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let bytes = response.body();
    let parsed = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(bytes).unwrap_or(Value::Null)
    };
    (status, parsed)
}

/// Call one operation, requiring success.
async fn call_ok(router: &MultiServiceRouter, operation: &str, body: &Value, case: &str) -> Value {
    let (status, parsed) = call(router, operation, body).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected {operation} to succeed with HTTP 200, got {status} and {parsed}"
    );
    parsed
}

/// The awsJson error code an error body carries in `__type`.
fn error_code(body: &Value) -> String {
    body.get("__type")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .rsplit(['#', '/'])
        .next()
        .unwrap_or_default()
        .to_string()
}

/// Assert a call was refused with `expected_code` and `expected_status`.
fn assert_refused(
    (status, body): (u16, Value),
    expected_status: u16,
    expected_code: &str,
    case: &str,
) {
    assert_eq!(
        status, expected_status,
        "case {case:?}: expected HTTP {expected_status}, got {status} and {body}"
    );
    assert_eq!(
        error_code(&body),
        expected_code,
        "case {case:?}: expected error code {expected_code}, got {body}"
    );
}

fn member<'a>(body: &'a Value, name: &str) -> &'a Value {
    body.get(name).unwrap_or(&Value::Null)
}

fn string_member(body: &Value, name: &str) -> String {
    member(body, name).as_str().unwrap_or_default().to_string()
}

/// Is `value` an id of `prefix`'s kind? `ChannelContractId` is modelled
/// `^cc-[a-z0-9]{13}$`, `RelationshipId` `^rs-[a-z0-9]{13}$` and
/// `ProgramManagementAccountId` `^pma-[a-z0-9]{13}$`, so each is its prefix plus
/// exactly 13 lowercase-alphanumeric characters.
fn is_id(value: &str, prefix: &str) -> bool {
    value.strip_prefix(prefix).is_some_and(|suffix| {
        suffix.len() == 13
            && suffix
                .bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit())
    })
}

/// The ARN a channel contract must carry, from the model's
/// `ChannelContractIdentifier` pattern
/// `^(arn:[a-z-]+:partnercentral:[a-z0-9-]+:[0-9]{12}:catalog/[a-zA-Z]+/channel-contract/)?cc-[a-z0-9]{13}$`.
fn expected_contract_arn(catalog: &str, id: &str) -> String {
    format!(
        "arn:aws:partnercentral:{REGION}:{}:catalog/{catalog}/channel-contract/{id}",
        engine::emulator_account_id()
    )
}

/// The catalog whose contracts `CreateChannelContract` documents as accepted
/// automatically, and the status rule that follows from it: everywhere else the
/// contract is sent to the signatory, so it is still awaiting a signature.
const AUTO_ACCEPT_CATALOG: &str = "Sandbox";

fn expected_status(catalog: &str) -> &'static str {
    if catalog == AUTO_ACCEPT_CATALOG {
        "ACCEPTED"
    } else {
        "PENDING"
    }
}

/// Every `ManagedServicesSupportedService` the model declares
/// (`smithy_query(query="shape",shape="ManagedServicesSupportedService")`).
const SUPPORTED_SERVICES: &[&str] = &[
    "MONITORING",
    "PATCHING",
    "SECURITY",
    "BACKUP_AND_DISASTER_RECOVERY",
    "FINANCIAL_OPERATIONS",
    "INFRASTRUCTURE_AS_CODE_AND_AUTOMATION",
    "WELL_ARCHITECTED",
    "ARTIFICIAL_INTELLIGENCE_AND_MACHINE_LEARNING_OPERATIONS",
];

/// Which form of an identifier a case sends: both a bare id and the full ARN are
/// modelled, and both must resolve to the same record.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum IdentifierForm {
    BareId,
    Arn,
}

#[derive(Debug, Clone)]
struct ContractCase {
    case: &'static str,
    catalog: &'static str,
    program: &'static str,
    account_type: Option<&'static str>,
    pma_account_id: &'static str,
    association_type: &'static str,
    sector: &'static str,
    account_scope: Option<&'static str>,
    associated_account_id: Option<&'static str>,
    start_date: Option<i64>,
    legal_entity_name: &'static str,
    supported_services: &'static [&'static str],
    member_account_ids: Option<&'static [&'static str]>,
    end_date: i64,
    signatory_name: &'static str,
    signatory_email: &'static str,
    address_line2: Option<&'static str>,
    identifier_form: IdentifierForm,
}

/// A `legalEntityName` at the model's upper length bound would be 300 characters;
/// one character is its lower bound. Both forms are exercised below.
const LEGAL_ENTITY_NAME_1: &str = "X";

/// The timestamps the rows below send, as the seconds since the Unix epoch the
/// `DateTime` shape carries on the wire. Each is a distinct instant, so a member
/// reported from the wrong one is visible.
const START_DATE_2026: i64 = 1_772_323_200;
const END_DATE_2027: i64 = 1_800_000_000;
const END_DATE_2028: i64 = 1_843_200_000;
const END_DATE_2029: i64 = 1_866_000_000;
const END_DATE_2030: i64 = 1_924_991_999;

/// The table every create/read case walks. Its rows differ along what the rules
/// generalise over: the catalog (which fixes whether the contract needs a
/// signature), the program and account type, whether the relationship names an
/// associated account, an account scope and a start date, how many supported
/// services and member accounts the attestation carries, whether the legal entity
/// address has a second line, and which FORM of the contract identifier the read
/// uses. Every enum value here is the model's own.
fn contract_cases() -> Vec<ContractCase> {
    vec![
        ContractCase {
            case: "aws/managed-services/entire-organization/bare-id",
            catalog: "AWS",
            program: "MANAGED_SERVICES_PROVIDER",
            account_type: Some("MANAGED_SERVICES"),
            pma_account_id: "111111111111",
            association_type: "MANAGED_SERVICES_END_CUSTOMER",
            sector: "COMMERCIAL",
            account_scope: Some("ENTIRE_ORGANIZATION"),
            associated_account_id: Some("210987654321"),
            start_date: None,
            legal_entity_name: "Acme Managed Services GmbH",
            supported_services: &["MONITORING"],
            member_account_ids: None,
            end_date: END_DATE_2027,
            signatory_name: "Dana Signatory",
            signatory_email: "dana@example.com",
            address_line2: None,
            identifier_form: IdentifierForm::BareId,
        },
        ContractCase {
            case: "sandbox/managed-services/specific-member-accounts/arn",
            catalog: AUTO_ACCEPT_CATALOG,
            program: "MANAGED_SERVICES_PROVIDER",
            account_type: Some("MANAGED_SERVICES"),
            pma_account_id: "222222222222",
            association_type: "MANAGED_SERVICES_END_CUSTOMER",
            sector: "GOVERNMENT",
            account_scope: Some("SPECIFIC_MEMBER_ACCOUNTS"),
            associated_account_id: Some("000000000001"),
            start_date: Some(START_DATE_2026),
            legal_entity_name: LEGAL_ENTITY_NAME_1,
            supported_services: SUPPORTED_SERVICES,
            member_account_ids: Some(&["333333333333", "444444444444", "555555555555"]),
            end_date: END_DATE_2030,
            signatory_name: "S",
            signatory_email: "s@e.co",
            address_line2: Some("Floor 12"),
            identifier_form: IdentifierForm::Arn,
        },
        ContractCase {
            case: "aws/resale-billing-transfer/no-associated-account/arn",
            catalog: "AWS",
            program: "MANAGED_SERVICES_PROVIDER",
            account_type: Some("RESALE_BILLING_TRANSFER"),
            pma_account_id: "333333333333",
            association_type: "DOWNSTREAM_SELLER",
            sector: "GOVERNMENT_EXCEPTION",
            account_scope: None,
            associated_account_id: None,
            start_date: None,
            legal_entity_name: "Downstream Seller Ltd",
            supported_services: &["SECURITY", "WELL_ARCHITECTED"],
            member_account_ids: Some(&["666666666666"]),
            end_date: END_DATE_2028,
            signatory_name: "Robin Reseller",
            signatory_email: "robin.reseller@example.co.uk",
            address_line2: Some("Unit 4"),
            identifier_form: IdentifierForm::Arn,
        },
        ContractCase {
            case: "sandbox/solution-provider/untyped-account/bare-id",
            catalog: AUTO_ACCEPT_CATALOG,
            program: "SOLUTION_PROVIDER",
            account_type: None,
            pma_account_id: "444444444444",
            association_type: "END_CUSTOMER",
            sector: "COMMERCIAL",
            account_scope: None,
            associated_account_id: Some("777777777777"),
            start_date: Some(START_DATE_2026 + 10_000_000),
            legal_entity_name: "Solution Provider SA",
            supported_services: &[
                "FINANCIAL_OPERATIONS",
                "INFRASTRUCTURE_AS_CODE_AND_AUTOMATION",
                "ARTIFICIAL_INTELLIGENCE_AND_MACHINE_LEARNING_OPERATIONS",
            ],
            member_account_ids: None,
            end_date: END_DATE_2029,
            signatory_name: "Pat Provider",
            signatory_email: "pat+contracts@example.org",
            address_line2: None,
            identifier_form: IdentifierForm::BareId,
        },
    ]
}

/// The program management account and relationship a contract case hangs off.
#[derive(Debug, Clone)]
struct Resources {
    pma_id: String,
    pma_arn: String,
    relationship_id: String,
    relationship_arn: String,
}

async fn create_account(
    router: &MultiServiceRouter,
    catalog: &str,
    program: &str,
    account_type: Option<&str>,
    account_id: &str,
    display_name: &str,
    case: &str,
) -> (String, String) {
    let mut request = json!({
        "catalog": catalog,
        "program": program,
        "displayName": display_name,
        "accountId": account_id,
    });
    if let Some(account_type) = account_type {
        request["type"] = json!(account_type);
    }
    let created = call_ok(router, "CreateProgramManagementAccount", &request, case).await;
    let detail = member(&created, "programManagementAccountDetail").clone();
    let id = string_member(&detail, "id");
    assert!(
        is_id(&id, "pma-"),
        "case {case:?}: expected a ^pma-[a-z0-9]{{13}}$ id, got {id:?}"
    );
    (id, string_member(&detail, "arn"))
}

async fn create_relationship(
    router: &MultiServiceRouter,
    case: &ContractCase,
    parent: &(String, String),
    display_name: &str,
) -> (String, String) {
    let mut request = json!({
        "catalog": case.catalog,
        "associationType": case.association_type,
        "programManagementAccountIdentifier": parent.0,
        "displayName": display_name,
        "sector": case.sector,
    });
    if let Some(account_id) = case.associated_account_id {
        request["associatedAccountId"] = json!(account_id);
    }
    if let Some(scope) = case.account_scope {
        request["accountScope"] = json!(scope);
    }
    if let Some(start_date) = case.start_date {
        request["startDate"] = json!(start_date);
    }
    let created = call_ok(router, "CreateRelationship", &request, case.case).await;
    let detail = member(&created, "relationshipDetail").clone();
    let id = string_member(&detail, "id");
    assert!(
        is_id(&id, "rs-"),
        "case {:?}: expected a ^rs-[a-z0-9]{{13}}$ id, got {id:?}",
        case.case
    );
    (id, string_member(&detail, "arn"))
}

/// Create the case's account and relationship.
async fn resources(router: &MultiServiceRouter, case: &ContractCase) -> Resources {
    let parent = create_account(
        router,
        case.catalog,
        case.program,
        case.account_type,
        case.pma_account_id,
        "Contract PMA",
        case.case,
    )
    .await;
    let relationship = create_relationship(router, case, &parent, "Contract Relationship").await;
    Resources {
        pma_id: parent.0,
        pma_arn: parent.1,
        relationship_id: relationship.0,
        relationship_arn: relationship.1,
    }
}

/// The `managedServicesAttestationPayload` a case sends.
fn attestation_payload(case: &ContractCase, resources: &Resources) -> Value {
    let mut attestation = json!({
        "legalEntityName": case.legal_entity_name,
        "relationshipIdentifier": resources.relationship_id,
        "programManagementAccountIdentifier": resources.pma_id,
        "endDate": case.end_date,
        "supportedServices": case.supported_services,
        "legalEntityAddress": {
            "city": "Berlin",
            "stateOrRegion": "Berlin",
            "postalCode": "10115",
            "countryCode": "DE",
            "addressLine": "Unter den Linden 1",
        },
        "signatoryName": case.signatory_name,
        "signatoryEmail": case.signatory_email,
    });
    if let Some(line2) = case.address_line2 {
        attestation["legalEntityAddress"]["addressLine2"] = json!(line2);
    }
    if let Some(member_accounts) = case.member_account_ids {
        attestation["memberAccountIds"] = json!(member_accounts);
    }
    json!({ "managedServicesAttestationPayload": attestation })
}

/// Create the case's channel contract and return `(id, arn)`.
async fn create_contract(
    router: &MultiServiceRouter,
    case: &ContractCase,
    resources: &Resources,
    client_token: Option<&str>,
) -> (String, String) {
    let mut request = json!({
        "catalog": case.catalog,
        "contractType": "MANAGED_SERVICES_ATTESTATION",
        "payload": attestation_payload(case, resources),
    });
    if let Some(token) = client_token {
        request["clientToken"] = json!(token);
    }
    let created = call_ok(router, "CreateChannelContract", &request, case.case).await;
    let detail = member(&created, "channelContractDetail").clone();
    let id = string_member(&detail, "id");
    let arn = string_member(&detail, "arn");
    assert!(
        is_id(&id, "cc-"),
        "case {:?}: expected a ^cc-[a-z0-9]{{13}}$ id, got {id:?}",
        case.case
    );
    assert_eq!(
        arn,
        expected_contract_arn(case.catalog, &id),
        "case {:?}: expected the ARN to be built from the stored id and catalog, got {arn:?}",
        case.case
    );
    (id, arn)
}

/// The identifier a case reads with.
fn read_identifier(case: &ContractCase, contract: &(String, String)) -> String {
    match case.identifier_form {
        IdentifierForm::BareId => contract.0.clone(),
        IdentifierForm::Arn => contract.1.clone(),
    }
}

async fn get_contract(
    router: &MultiServiceRouter,
    catalog: &str,
    identifier: &str,
    case: &str,
) -> Value {
    let read = call_ok(
        router,
        "GetChannelContract",
        &json!({ "catalog": catalog, "identifier": identifier }),
        case,
    )
    .await;
    member(&read, "channelContractDetail").clone()
}

// ---------------------------------------------------------------------------
// Creating a contract and reading it back
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_created_channel_contract_reads_back_with_the_values_it_was_created_with() {
    for case in contract_cases() {
        let router = emulator();
        let resources = resources(&router, &case).await;
        let contract = create_contract(&router, &case, &resources, None).await;

        let detail = get_contract(
            &router,
            case.catalog,
            &read_identifier(&case, &contract),
            case.case,
        )
        .await;

        assert_eq!(
            string_member(&detail, "id"),
            contract.0,
            "case {:?}: Get must report the id the create minted, got {detail}",
            case.case
        );
        assert_eq!(
            string_member(&detail, "arn"),
            contract.1,
            "case {:?}: Get must report the stored ARN, got {detail}",
            case.case
        );
        assert_eq!(
            string_member(&detail, "catalog"),
            case.catalog,
            "case {:?}: expected catalog {}, got {detail}",
            case.case,
            case.catalog
        );
        assert_eq!(
            string_member(&detail, "contractType"),
            "MANAGED_SERVICES_ATTESTATION",
            "case {:?}: expected the contract type the create sent, got {detail}",
            case.case
        );
        assert_eq!(
            string_member(&detail, "status"),
            expected_status(case.catalog),
            "case {:?}: a contract is accepted automatically only in the {AUTO_ACCEPT_CATALOG} \
             catalog, got {detail}",
            case.case
        );
        let responded_at = member(&detail, "respondedAt").clone();
        assert_eq!(
            !responded_at.is_null(),
            expected_status(case.catalog) == "ACCEPTED",
            "case {:?}: respondedAt must be reported exactly when the contract has been \
             answered, got {responded_at}",
            case.case
        );
        assert!(
            member(&detail, "cancellationReason").is_null(),
            "case {:?}: a contract nobody canceled must report no cancellationReason, got {detail}",
            case.case
        );
        let created_at = member(&detail, "createdAt").clone();
        assert!(
            !created_at.is_null(),
            "case {:?}: expected a createdAt timestamp, got {detail}",
            case.case
        );
        assert_eq!(
            member(&detail, "updatedAt"),
            &created_at,
            "case {:?}: an unmodified contract's updatedAt must equal its createdAt, got {detail}",
            case.case
        );

        let details = member(&detail, "details").clone();
        let attestation = member(&details, "managedServicesAttestationDetails").clone();
        assert_eq!(
            string_member(&attestation, "legalEntityName"),
            case.legal_entity_name,
            "case {:?}: expected the legal entity name the create sent, got {attestation}",
            case.case
        );
        assert_eq!(
            string_member(&attestation, "relationshipIdentifier"),
            resources.relationship_arn,
            "case {:?}: expected the stored relationship's ARN, got {attestation}",
            case.case
        );
        assert_eq!(
            string_member(&attestation, "programManagementAccountIdentifier"),
            resources.pma_arn,
            "case {:?}: expected the stored account's ARN, got {attestation}",
            case.case
        );
        assert_eq!(
            member(&attestation, "endDate"),
            &json!(case.end_date),
            "case {:?}: expected the end date the create sent, got {attestation}",
            case.case
        );
        assert_eq!(
            member(&attestation, "supportedServices"),
            &json!(case.supported_services),
            "case {:?}: expected the supported services the create sent, got {attestation}",
            case.case
        );
        assert_eq!(
            member(&attestation, "memberAccountIds"),
            &match case.member_account_ids {
                Some(ids) => json!(ids),
                None => Value::Null,
            },
            "case {:?}: expected the member accounts the create sent, got {attestation}",
            case.case
        );
        assert_eq!(
            string_member(&attestation, "signatoryName"),
            case.signatory_name,
            "case {:?}: expected the signatory the create sent, got {attestation}",
            case.case
        );
        assert_eq!(
            string_member(&attestation, "signatoryEmail"),
            case.signatory_email,
            "case {:?}: expected the signatory email the create sent, got {attestation}",
            case.case
        );
        let address = member(&attestation, "legalEntityAddress").clone();
        assert_eq!(
            string_member(&address, "countryCode"),
            "DE",
            "case {:?}: expected the stored legal entity address, got {address}",
            case.case
        );
        assert_eq!(
            member(&address, "addressLine2").as_str(),
            case.address_line2,
            "case {:?}: expected the address line the create sent, got {address}",
            case.case
        );

        // Both identifier forms name the same record, so the other form must
        // report exactly the same detail.
        let other = match case.identifier_form {
            IdentifierForm::BareId => contract.1.clone(),
            IdentifierForm::Arn => contract.0.clone(),
        };
        let again = get_contract(&router, case.catalog, &other, case.case).await;
        assert_eq!(
            again, detail,
            "case {:?}: a bare id and the full ARN must resolve to the same contract",
            case.case
        );
    }
}

#[tokio::test]
async fn an_accepted_channel_contract_becomes_the_one_in_effect_for_its_relationship() {
    for case in contract_cases() {
        let router = emulator();
        let resources = resources(&router, &case).await;

        let before = call_ok(
            &router,
            "GetRelationship",
            &json!({
                "catalog": case.catalog,
                "programManagementAccountIdentifier": resources.pma_id,
                "identifier": resources.relationship_id,
            }),
            case.case,
        )
        .await;
        let before = member(&before, "relationshipDetail").clone();
        assert!(
            member(&before, "channelContractId").is_null()
                && member(&before, "endDate").is_null(),
            "case {:?}: a relationship with no contract in effect must report neither a \
             channelContractId nor an endDate, got {before}",
            case.case
        );

        let contract = create_contract(&router, &case, &resources, None).await;
        let after = call_ok(
            &router,
            "GetRelationship",
            &json!({
                "catalog": case.catalog,
                "programManagementAccountIdentifier": resources.pma_arn,
                "identifier": resources.relationship_arn,
            }),
            case.case,
        )
        .await;
        let after = member(&after, "relationshipDetail").clone();

        if expected_status(case.catalog) == "ACCEPTED" {
            assert_eq!(
                string_member(&after, "channelContractId"),
                contract.0,
                "case {:?}: an accepted contract is the one in effect for its relationship, got \
                 {after}",
                case.case
            );
            assert_eq!(
                member(&after, "endDate"),
                &json!(case.end_date),
                "case {:?}: the relationship ends when the contract in effect does, got {after}",
                case.case
            );
        } else {
            assert!(
                member(&after, "channelContractId").is_null()
                    && member(&after, "endDate").is_null(),
                "case {:?}: a contract still awaiting a signature is in effect for nothing, got \
                 {after}",
                case.case
            );
        }
    }
}

#[tokio::test]
async fn a_relationship_reports_the_account_scope_and_start_date_it_was_created_with() {
    let router = emulator();
    for case in contract_cases() {
        let resources = resources(&router, &case).await;
        let read = call_ok(
            &router,
            "GetRelationship",
            &json!({
                "catalog": case.catalog,
                "programManagementAccountIdentifier": resources.pma_id,
                "identifier": resources.relationship_id,
            }),
            case.case,
        )
        .await;
        let detail = member(&read, "relationshipDetail").clone();

        assert_eq!(
            member(&detail, "accountScope").as_str(),
            case.account_scope,
            "case {:?}: expected the account scope the create sent, got {detail}",
            case.case
        );
        assert_eq!(
            member(&detail, "associatedAccountId").as_str(),
            case.associated_account_id,
            "case {:?}: expected the associated account the create sent, got {detail}",
            case.case
        );
        match case.start_date {
            Some(start_date) => assert_eq!(
                member(&detail, "startDate"),
                &json!(start_date),
                "case {:?}: expected the start date the create sent, got {detail}",
                case.case
            ),
            // A relationship the request gave no start date for starts when it
            // was created.
            None => assert_eq!(
                member(&detail, "startDate"),
                member(&detail, "createdAt"),
                "case {:?}: expected startDate to fall back to createdAt, got {detail}",
                case.case
            ),
        }
    }
}

#[tokio::test]
async fn a_program_management_account_reports_the_type_it_was_created_with() {
    let router = emulator();
    // Each row is one `ProgramManagementAccountType` the model declares, plus a
    // program for which the model documents no type is needed.
    let rows: &[(&str, &str, Option<&str>)] = &[
        ("managed-services", "MANAGED_SERVICES_PROVIDER", Some("MANAGED_SERVICES")),
        (
            "resale-billing-transfer",
            "MANAGED_SERVICES_PROVIDER",
            Some("RESALE_BILLING_TRANSFER"),
        ),
        ("untyped", "SOLUTION_PROVIDER", None),
    ];
    let mut created: Vec<(&str, String, Option<&str>)> = Vec::new();
    for (index, (case, program, account_type)) in rows.iter().enumerate() {
        let (id, _arn) = create_account(
            &router,
            "AWS",
            program,
            *account_type,
            &format!("10000000000{index}"),
            case,
            case,
        )
        .await;
        created.push((case, id, *account_type));
    }

    let listed = call_ok(
        &router,
        "ListProgramManagementAccounts",
        &json!({ "catalog": "AWS", "maxResults": 100 }),
        "every-type",
    )
    .await;
    let items = member(&listed, "items").as_array().cloned().unwrap_or_default();
    for (case, id, account_type) in &created {
        let summary = items
            .iter()
            .find(|item| string_member(item, "id") == *id)
            .unwrap_or_else(|| panic!("case {case:?}: expected {id} in {listed}"));
        assert_eq!(
            member(summary, "type").as_str(),
            *account_type,
            "case {case:?}: expected the type the create sent, got {summary}"
        );
    }

    // The `type` filter reports exactly the accounts stored with that type, so an
    // account created without one matches no value of it.
    for (case, id, account_type) in &created {
        for filter in ["MANAGED_SERVICES", "RESALE_BILLING_TRANSFER"] {
            let filtered = call_ok(
                &router,
                "ListProgramManagementAccounts",
                &json!({ "catalog": "AWS", "maxResults": 100, "type": filter }),
                case,
            )
            .await;
            let ids: Vec<String> = member(&filtered, "items")
                .as_array()
                .cloned()
                .unwrap_or_default()
                .iter()
                .map(|item| string_member(item, "id"))
                .collect();
            assert_eq!(
                ids.contains(id),
                *account_type == Some(filter),
                "case {case:?}: filtering on type {filter} must report {id} only when it is \
                 stored with that type, got {ids:?}"
            );
        }
    }
}

#[tokio::test]
async fn an_account_for_the_managed_services_provider_program_needs_a_type() {
    let router = emulator();
    // The model documents `type` as required for the managed services provider
    // program, and optional for every other program.
    for (case, program, expected) in [
        ("managed-services-provider", "MANAGED_SERVICES_PROVIDER", 400),
        ("solution-provider", "SOLUTION_PROVIDER", 200),
        ("distribution", "DISTRIBUTION", 200),
        ("distribution-seller", "DISTRIBUTION_SELLER", 200),
    ] {
        let answered = call(
            &router,
            "CreateProgramManagementAccount",
            &json!({
                "catalog": "AWS",
                "program": program,
                "displayName": case,
                "accountId": "999999999999",
            }),
        )
        .await;
        if expected == 400 {
            assert_refused(answered, 400, "ValidationException", case);
        } else {
            assert_eq!(
                answered.0, 200,
                "case {case:?}: expected HTTP 200, got {} and {}",
                answered.0, answered.1
            );
        }
    }
}

// ---------------------------------------------------------------------------
// Listing contracts
// ---------------------------------------------------------------------------

/// The ids a list page reports, in the order they came back.
fn listed_ids(page: &Value) -> Vec<String> {
    member(page, "items")
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|item| string_member(item, "id"))
        .collect()
}

/// Walk every page with `max_results` and return the ids in the order they came
/// back.
async fn page_through(router: &MultiServiceRouter, max_results: i64, case: &str) -> Vec<String> {
    let mut ids = Vec::new();
    let mut next_token: Option<String> = None;
    // Bounded so a token that never advances fails the assertion below instead of
    // looping forever.
    for _ in 0..64 {
        let mut request = json!({
            "catalog": "AWS",
            "contractType": "MANAGED_SERVICES_ATTESTATION",
            "maxResults": max_results,
        });
        if let Some(token) = next_token.clone() {
            request["nextToken"] = json!(token);
        }
        let page = call_ok(router, "ListChannelContracts", &request, case).await;
        let page_ids = listed_ids(&page);
        assert!(
            i64::try_from(page_ids.len()).unwrap_or(i64::MAX) <= max_results,
            "case {case:?}: a page must not exceed maxResults {max_results}, got {page_ids:?}"
        );
        ids.extend(page_ids);
        match member(&page, "nextToken").as_str() {
            Some(token) => next_token = Some(token.to_string()),
            None => return ids,
        }
    }
    panic!("case {case:?}: paging did not terminate within 64 pages");
}

/// A store holding three contracts in the `AWS` catalog: two under one
/// relationship and one under another, with two distinct legal entity names, and
/// the last one canceled.
struct ListFixture {
    router: MultiServiceRouter,
    first_relationship: Resources,
    second_relationship: Resources,
    /// `(id, legal entity name, status)` in creation order.
    contracts: Vec<(String, String, String)>,
}

async fn list_fixture() -> ListFixture {
    let router = emulator();
    let mut case = contract_cases()[0].clone();
    let first = resources(&router, &case).await;
    case.pma_account_id = "888888888888";
    let second = resources(&router, &case).await;

    let mut contracts = Vec::new();
    for (index, (resources, legal_entity_name)) in [
        (&first, "Acme Managed Services GmbH"),
        (&first, "Second Entity SARL"),
        (&second, "Acme Managed Services GmbH"),
    ]
    .into_iter()
    .enumerate()
    {
        let mut row = case.clone();
        row.legal_entity_name = legal_entity_name;
        let (id, _arn) = create_contract(&router, &row, resources, None).await;
        let status = if index == 2 {
            let canceled = call_ok(
                &router,
                "CancelChannelContract",
                &json!({
                    "catalog": "AWS",
                    "identifier": id,
                    "reason": "superseded by a later attestation",
                }),
                "list-fixture",
            )
            .await;
            string_member(&member(&canceled, "channelContractDetail").clone(), "status")
        } else {
            expected_status("AWS").to_string()
        };
        contracts.push((id, legal_entity_name.to_string(), status));
    }

    ListFixture {
        router,
        first_relationship: first,
        second_relationship: second,
        contracts,
    }
}

#[tokio::test]
async fn listing_channel_contracts_reports_every_stored_contract_in_creation_order() {
    let fixture = list_fixture().await;
    let expected: Vec<String> = fixture
        .contracts
        .iter()
        .map(|(id, _, _)| id.clone())
        .collect();

    // Each row is a page size: the published `maxResults` bounds (1 and 100), one
    // in between, and one that exactly divides the record count.
    for max_results in [1, 2, 3, 100] {
        let paged = page_through(
            &fixture.router,
            max_results,
            &format!("maxResults={max_results}"),
        )
        .await;
        assert_eq!(
            paged, expected,
            "case maxResults={max_results}: expected every stored contract once in creation \
             order, got {paged:?} against {expected:?}"
        );
    }
}

#[tokio::test]
async fn listing_channel_contracts_honours_each_filter_the_model_declares() {
    let fixture = list_fixture().await;
    let ids: Vec<String> = fixture
        .contracts
        .iter()
        .map(|(id, _, _)| id.clone())
        .collect();

    // Each row is one filter the model declares, with the contracts it must
    // report: a status, a relationship (as a bare id and as an ARN) and a legal
    // entity name.
    let rows: Vec<(&str, Value, Vec<String>)> = vec![
        (
            "statuses=PENDING",
            json!({ "statuses": ["PENDING"] }),
            vec![ids[0].clone(), ids[1].clone()],
        ),
        (
            "statuses=CANCELED",
            json!({ "statuses": ["CANCELED"] }),
            vec![ids[2].clone()],
        ),
        (
            "statuses=PENDING+CANCELED",
            json!({ "statuses": ["PENDING", "CANCELED"] }),
            ids.clone(),
        ),
        ("statuses=ACCEPTED", json!({ "statuses": ["ACCEPTED"] }), vec![]),
        (
            "relationshipIdentifiers=first/bare-id",
            json!({
                "contractTypeFilters": {
                    "managedServicesAttestationTypeFilters": {
                        "relationshipIdentifiers": [fixture.first_relationship.relationship_id],
                    }
                }
            }),
            vec![ids[0].clone(), ids[1].clone()],
        ),
        (
            "relationshipIdentifiers=second/arn",
            json!({
                "contractTypeFilters": {
                    "managedServicesAttestationTypeFilters": {
                        "relationshipIdentifiers": [fixture.second_relationship.relationship_arn],
                    }
                }
            }),
            vec![ids[2].clone()],
        ),
        (
            "legalEntityNames=second-entity",
            json!({
                "contractTypeFilters": {
                    "managedServicesAttestationTypeFilters": {
                        "legalEntityNames": ["Second Entity SARL"],
                    }
                }
            }),
            vec![ids[1].clone()],
        ),
        (
            "legalEntityNames+statuses",
            json!({
                "statuses": ["CANCELED"],
                "contractTypeFilters": {
                    "managedServicesAttestationTypeFilters": {
                        "legalEntityNames": ["Acme Managed Services GmbH"],
                    }
                }
            }),
            vec![ids[2].clone()],
        ),
    ];

    for (case, filter, expected) in rows {
        let mut request = json!({
            "catalog": "AWS",
            "contractType": "MANAGED_SERVICES_ATTESTATION",
            "maxResults": 100,
        });
        for (name, value) in filter.as_object().expect("the filter is an object") {
            request[name] = value.clone();
        }
        let page = call_ok(&fixture.router, "ListChannelContracts", &request, case).await;
        assert_eq!(
            listed_ids(&page),
            expected,
            "case {case:?}: expected {expected:?}, got {page}"
        );
    }

    // A catalog holding no contract at all is an empty success, not a refusal.
    let empty = call_ok(
        &fixture.router,
        "ListChannelContracts",
        &json!({
            "catalog": AUTO_ACCEPT_CATALOG,
            "contractType": "MANAGED_SERVICES_ATTESTATION",
            "maxResults": 100,
        }),
        "another-catalog",
    )
    .await;
    assert_eq!(
        member(&empty, "items"),
        &json!([]),
        "case \"another-catalog\": expected an empty items list, got {empty}"
    );
}

#[tokio::test]
async fn sorting_channel_contracts_orders_them_by_their_stored_update_time() {
    let fixture = list_fixture().await;
    // The model's only `ManagedServicesAttestationTypeSortName` is `UpdatedAt`,
    // so each row is one `SortOrder` and the key is the stored `updatedAt`.
    for (case, order) in [("ascending", "Ascending"), ("descending", "Descending")] {
        let page = call_ok(
            &fixture.router,
            "ListChannelContracts",
            &json!({
                "catalog": "AWS",
                "contractType": "MANAGED_SERVICES_ATTESTATION",
                "maxResults": 100,
                "contractTypeSort": {
                    "managedServicesAttestationTypeSort": {
                        "sortBy": "UpdatedAt",
                        "sortOrder": order,
                    }
                },
            }),
            case,
        )
        .await;
        let items = member(&page, "items").as_array().cloned().unwrap_or_default();
        assert_eq!(
            items.len(),
            fixture.contracts.len(),
            "case {case:?}: a sort must report every matching contract, got {page}"
        );
        let times: Vec<String> = items
            .iter()
            .map(|item| string_member(item, "updatedAt"))
            .collect();
        let ordered = times.windows(2).all(|pair| {
            if order == "Descending" {
                pair[0] >= pair[1]
            } else {
                pair[0] <= pair[1]
            }
        });
        assert!(
            ordered,
            "case {case:?}: expected updatedAt to be {order}, got {times:?}"
        );
    }
}

#[tokio::test]
async fn listing_channel_contracts_refuses_a_token_from_another_walk() {
    let fixture = list_fixture().await;
    let first = call_ok(
        &fixture.router,
        "ListChannelContracts",
        &json!({
            "catalog": "AWS",
            "contractType": "MANAGED_SERVICES_ATTESTATION",
            "maxResults": 1,
        }),
        "first-page",
    )
    .await;
    let token = string_member(&first, "nextToken");
    assert!(
        !token.is_empty(),
        "case \"first-page\": expected a nextToken, got {first}"
    );

    // Each row is a token a walk cannot continue from: one taken with different
    // filters, and one this service never handed out.
    for (case, body) in [
        (
            "different-filters",
            json!({
                "catalog": "AWS",
                "contractType": "MANAGED_SERVICES_ATTESTATION",
                "maxResults": 1,
                "statuses": ["PENDING"],
                "nextToken": token,
            }),
        ),
        (
            "not-a-token",
            json!({
                "catalog": "AWS",
                "contractType": "MANAGED_SERVICES_ATTESTATION",
                "maxResults": 1,
                "nextToken": "nonsense",
            }),
        ),
    ] {
        let answered = call(&fixture.router, "ListChannelContracts", &body).await;
        assert_refused(answered, 400, "ValidationException", case);
    }
}

// ---------------------------------------------------------------------------
// Cancelling a contract
// ---------------------------------------------------------------------------

#[tokio::test]
async fn cancelling_a_pending_channel_contract_records_the_cancellation() {
    let case = contract_cases()[0].clone();
    let router = emulator();
    let resources = resources(&router, &case).await;

    // Each row is a form of the identifier, with its own contract: both name the
    // same kind of record, so both must cancel it.
    for form in [IdentifierForm::BareId, IdentifierForm::Arn] {
        let mut row = case.clone();
        row.identifier_form = form;
        let contract = create_contract(&router, &row, &resources, None).await;
        let identifier = read_identifier(&row, &contract);
        let case_name = format!("{:?}", form);

        let canceled = call_ok(
            &router,
            "CancelChannelContract",
            &json!({
                "catalog": row.catalog,
                "identifier": identifier,
                "reason": "the customer withdrew the request",
            }),
            &case_name,
        )
        .await;
        let detail = member(&canceled, "channelContractDetail").clone();
        assert_eq!(
            string_member(&detail, "id"),
            contract.0,
            "case {case_name:?}: expected the stored id, got {detail}"
        );
        assert_eq!(
            string_member(&detail, "status"),
            "CANCELED",
            "case {case_name:?}: expected a canceled contract, got {detail}"
        );
        assert_eq!(
            string_member(&detail, "cancellationReason"),
            "the customer withdrew the request",
            "case {case_name:?}: expected the reason the request gave, got {detail}"
        );

        // The cancellation is persisted, so the read reports it too.
        let read = get_contract(&router, row.catalog, &identifier, &case_name).await;
        assert_eq!(
            string_member(&read, "status"),
            "CANCELED",
            "case {case_name:?}: expected Get to report the canceled contract, got {read}"
        );
        assert_eq!(
            string_member(&read, "cancellationReason"),
            "the customer withdrew the request",
            "case {case_name:?}: expected Get to report the stored reason, got {read}"
        );
        assert!(
            !member(&read, "respondedAt").is_null(),
            "case {case_name:?}: a canceled contract has been answered, so it must report \
             respondedAt, got {read}"
        );
    }
}

#[tokio::test]
async fn cancelling_a_channel_contract_that_is_not_pending_is_refused() {
    // Each row is a contract whose status forbids a cancellation: one already
    // canceled, and one accepted when it was created.
    let pending_case = contract_cases()[0].clone();
    let accepted_case = contract_cases()[1].clone();

    let router = emulator();
    let pending_resources = resources(&router, &pending_case).await;
    let (pending_id, _arn) = create_contract(&router, &pending_case, &pending_resources, None).await;
    call_ok(
        &router,
        "CancelChannelContract",
        &json!({ "catalog": pending_case.catalog, "identifier": pending_id }),
        "first-cancel",
    )
    .await;
    let twice = call(
        &router,
        "CancelChannelContract",
        &json!({ "catalog": pending_case.catalog, "identifier": pending_id }),
        )
    .await;
    assert_refused(twice, 400, "ValidationException", "already-canceled");

    let accepted_resources = resources(&router, &accepted_case).await;
    let (accepted_id, _arn) =
        create_contract(&router, &accepted_case, &accepted_resources, None).await;
    let refused = call(
        &router,
        "CancelChannelContract",
        &json!({ "catalog": accepted_case.catalog, "identifier": accepted_id }),
    )
    .await;
    assert_refused(refused, 400, "ValidationException", "already-accepted");
}

#[tokio::test]
async fn a_client_token_replays_a_create_and_a_cancel_instead_of_repeating_it() {
    let case = contract_cases()[0].clone();
    let router = emulator();
    let resources = resources(&router, &case).await;

    let first = create_contract(&router, &case, &resources, Some("token-create")).await;
    let again = create_contract(&router, &case, &resources, Some("token-create")).await;
    assert_eq!(
        first, again,
        "case \"create-replay\": a retry with the same clientToken must answer the first \
         contract, not mint a second"
    );
    let listed = call_ok(
        &router,
        "ListChannelContracts",
        &json!({
            "catalog": case.catalog,
            "contractType": "MANAGED_SERVICES_ATTESTATION",
            "maxResults": 100,
        }),
        "create-replay",
    )
    .await;
    assert_eq!(
        listed_ids(&listed),
        vec![first.0.clone()],
        "case \"create-replay\": exactly one contract must be stored, got {listed}"
    );

    let cancel = json!({
        "catalog": case.catalog,
        "identifier": first.0,
        "reason": "duplicate request",
        "clientToken": "token-cancel",
    });
    let canceled = call_ok(&router, "CancelChannelContract", &cancel, "cancel-replay").await;
    let replayed = call_ok(&router, "CancelChannelContract", &cancel, "cancel-replay").await;
    assert_eq!(
        canceled, replayed,
        "case \"cancel-replay\": a retry with the same clientToken must answer the stored \
         cancellation rather than refusing it"
    );

    // The same token with different parameters is a conflict, not a second
    // cancellation.
    let mismatched = call(
        &router,
        "CancelChannelContract",
        &json!({
            "catalog": case.catalog,
            "identifier": first.0,
            "reason": "a different reason",
            "clientToken": "token-cancel",
        }),
    )
    .await;
    assert_refused(mismatched, 409, "ConflictException", "token-mismatch");
}

// ---------------------------------------------------------------------------
// Refusals
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_channel_contract_operation_refuses_a_resource_it_cannot_find() {
    let case = contract_cases()[0].clone();
    let router = emulator();
    let resources = resources(&router, &case).await;
    let (id, _arn) = create_contract(&router, &case, &resources, None).await;
    // Well-formed per the model's `^cc-[a-z0-9]{13}$`, but nothing is stored
    // under it.
    let unknown = "cc-0000000000000";

    // Each row is a call naming a resource that is not there, or one in another
    // catalog: both are a 404 rather than an empty success.
    let rows: Vec<(&str, &str, Value)> = vec![
        (
            "get-unknown",
            "GetChannelContract",
            json!({ "catalog": case.catalog, "identifier": unknown }),
        ),
        (
            "get-other-catalog",
            "GetChannelContract",
            json!({ "catalog": AUTO_ACCEPT_CATALOG, "identifier": id }),
        ),
        (
            "cancel-unknown",
            "CancelChannelContract",
            json!({ "catalog": case.catalog, "identifier": unknown }),
        ),
        (
            "cancel-other-catalog",
            "CancelChannelContract",
            json!({ "catalog": AUTO_ACCEPT_CATALOG, "identifier": id }),
        ),
        (
            "create-unknown-relationship",
            "CreateChannelContract",
            json!({
                "catalog": case.catalog,
                "contractType": "MANAGED_SERVICES_ATTESTATION",
                "payload": attestation_payload(
                    &case,
                    &Resources {
                        pma_id: resources.pma_id.clone(),
                        pma_arn: resources.pma_arn.clone(),
                        relationship_id: "rs-0000000000000".to_string(),
                        relationship_arn: String::new(),
                    },
                ),
            }),
        ),
        (
            "create-other-catalog",
            "CreateChannelContract",
            json!({
                "catalog": AUTO_ACCEPT_CATALOG,
                "contractType": "MANAGED_SERVICES_ATTESTATION",
                "payload": attestation_payload(&case, &resources),
            }),
        ),
    ];
    for (name, operation, body) in rows {
        let answered = call(&router, operation, &body).await;
        assert_refused(answered, 404, "ResourceNotFoundException", name);
    }
}

#[tokio::test]
async fn creating_a_channel_contract_refuses_an_account_that_is_not_the_relationships_parent() {
    let case = contract_cases()[0].clone();
    let router = emulator();
    let resources = resources(&router, &case).await;
    let other = create_account(
        &router,
        case.catalog,
        "SOLUTION_PROVIDER",
        None,
        "121212121212",
        "Another PMA",
        "other-parent",
    )
    .await;

    let answered = call(
        &router,
        "CreateChannelContract",
        &json!({
            "catalog": case.catalog,
            "contractType": "MANAGED_SERVICES_ATTESTATION",
            "payload": attestation_payload(
                &case,
                &Resources {
                    pma_id: other.0,
                    pma_arn: other.1,
                    relationship_id: resources.relationship_id.clone(),
                    relationship_arn: resources.relationship_arn.clone(),
                },
            ),
        }),
    )
    .await;
    assert_refused(answered, 400, "ValidationException", "other-parent");
}

#[tokio::test]
async fn every_channel_contract_operation_is_gated_on_an_active_partner_benefit() {
    let case = contract_cases()[0].clone();
    // The resources the calls name are created on an entitled router, so the gate
    // is the only difference between the two.
    let entitled = emulator();
    let resources = resources(&entitled, &case).await;
    let (id, _arn) = create_contract(&entitled, &case, &resources, None).await;

    let gated = emulator_without_entitlement();
    let rows: Vec<(&str, &str, Value)> = vec![
        (
            "create",
            "CreateChannelContract",
            json!({
                "catalog": case.catalog,
                "contractType": "MANAGED_SERVICES_ATTESTATION",
                "payload": attestation_payload(&case, &resources),
            }),
        ),
        (
            "get",
            "GetChannelContract",
            json!({ "catalog": case.catalog, "identifier": id }),
        ),
        (
            "list",
            "ListChannelContracts",
            json!({
                "catalog": case.catalog,
                "contractType": "MANAGED_SERVICES_ATTESTATION",
                "maxResults": 100,
            }),
        ),
        (
            "cancel",
            "CancelChannelContract",
            json!({ "catalog": case.catalog, "identifier": id }),
        ),
    ];
    for (name, operation, body) in rows {
        let answered = call(&gated, operation, &body).await;
        assert_refused(answered, 403, "AccessDeniedException", name);
    }
}
