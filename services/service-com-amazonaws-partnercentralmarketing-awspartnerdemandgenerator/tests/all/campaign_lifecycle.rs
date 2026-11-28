// SPDX-License-Identifier: CC0-1.0

//! A campaign's whole life, over this service's own `rpcv2Cbor` wire.
//!
//! Every assertion here goes through `MultiServiceRouter::dispatch`, so it exercises the
//! same path a real SDK client takes: routing, deserialization, boundary constraint
//! validation, the handler, and error rendering. Request and response bodies go through
//! the generated schema codec, so what is asserted is what a client reads.
//!
//! The rules under test, each over a SET of inputs rather than one request:
//!
//! * `CreateCampaign` persists a campaign built from the members the request names,
//!   mints an `Id` of the modelled form and an `Arn` of the modelled form over the
//!   request's own Region, and reports the identity of the new campaign and nothing
//!   else;
//! * a repeat carrying the same `ClientToken` reports that same identity and creates no
//!   second campaign;
//! * `GetCampaign` rebuilds the campaign from the stored record — including the
//!   `Language` its template is authored in — whether it is addressed by its id or by
//!   its ARN;
//! * `State` is DERIVED on every read from `EndOn` and the campaign's assets, and an
//!   update of a campaign whose window has closed is refused;
//! * `UpdateCampaign` applies the members the request names and leaves every other as it
//!   stands;
//! * `ListCampaigns` reports a summary per stored campaign, narrowed by every filter the
//!   request named, ordered as `Sort` asks, and paged with a token bound to those
//!   filters;
//! * `DeleteCampaign` removes the campaign, after which every read of it answers
//!   `ResourceNotFoundException`;
//! * a request this service refuses on its own terms names every member that failed, and
//!   persists nothing.
//!
//! This file also owns the harness the catalog suite shares.

use std::sync::Arc;
use std::time::{Duration, SystemTime};

use aws_smithy_schema::codec::{Codec, FinishSerializer};
use aws_smithy_schema::serde::{SerializableStruct, ShapeSerializer};
use aws_smithy_schema::Schema;
use aws_smithy_schema_server::{CborCodec, DeserializableStruct};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_partnercentralmarketing_awspartnerdemandgenerator::ops::types::{
    Campaign, CampaignSort, CreateCampaignInput, CreateCampaignOutput, DeleteCampaignInput,
    GetCampaignInput, GetCampaignOutput, ListCampaignsInput, ListCampaignsOutput, MonetaryValue,
    Tag, UpdateCampaignInput, UpdateCampaignOutput, CREATE_CAMPAIGN_INPUT_SCHEMA,
    DELETE_CAMPAIGN_INPUT_SCHEMA, GET_CAMPAIGN_INPUT_SCHEMA, LIST_CAMPAIGNS_INPUT_SCHEMA,
    UPDATE_CAMPAIGN_INPUT_SCHEMA,
};
use service_com_amazonaws_partnercentralmarketing_awspartnerdemandgenerator::{
    state, EmulatedService,
};

/// The Region the request's SigV4 credential scope names, which is also the Region the
/// minted ARNs are built over.
pub(crate) const REGION: &str = "us-west-2";

/// This service's SigV4 signing name, from the model (`OPERATIONS.md`, "Wire protocol").
pub(crate) const SIGNING_NAME: &str = "partnercentral-marketing";

/// The Smithy service shape name, which is the `rpcv2Cbor` URI's service segment
/// (`smithy_query(query="service_info")` -> `AWSPartnerDemandGenerator`).
const SERVICE_SHAPE: &str = "AWSPartnerDemandGenerator";

/// The media type an `rpcv2Cbor` body is labelled with, in both directions.
const CBOR_CONTENT_TYPE: &str = "application/cbor";

/// The value the `smithy-protocol` header carries on that wire.
const CBOR_PROTOCOL: &str = "rpc-v2-cbor";

/// The catalog these suites work in — a member of `Catalog` (`AWS | Sandbox`).
pub(crate) const CATALOG: &str = "Sandbox";

/// The other catalog, so an identifier naming one catalog can be shown to name nothing
/// in the other.
pub(crate) const OTHER_CATALOG: &str = "AWS";

/// What one dispatched request answered.
pub(crate) struct Answer {
    pub(crate) status: u16,
    content_type: String,
    smithy_protocol: String,
    body: Vec<u8>,
}

impl Answer {
    /// The body as printable text, for an assertion message.
    pub(crate) fn render(&self) -> String {
        String::from_utf8_lossy(&self.body).escape_debug().to_string()
    }

    /// Whether the body carries `needle` as text. A CBOR text string is its UTF-8 bytes
    /// behind a length prefix, so this is how an error envelope's own code and message —
    /// which belong to no operation shape — are read.
    pub(crate) fn mentions(&self, needle: &str) -> bool {
        self.body
            .windows(needle.len())
            .any(|window| window == needle.as_bytes())
    }

    /// The modelled output this answer carries, read back through the generated
    /// deserializer.
    pub(crate) fn output<O: DeserializableStruct>(&self, case: &str) -> O {
        let codec = CborCodec::default();
        let mut deserializer = codec.create_deserializer(&self.body);
        O::deserialize(&mut deserializer).unwrap_or_else(|error| {
            panic!(
                "case {case:?}: the response does not deserialize ({error}), body {}",
                self.render()
            )
        })
    }
}

/// One emulator over one shared engine: every call sees the same store, and a suite can
/// read that store to check what a call persisted.
pub(crate) struct Emulator {
    router: MultiServiceRouter,
    pub(crate) engine: Arc<engine::InMemoryEngine>,
}

impl Emulator {
    pub(crate) fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        router.set_request_authentication(false);
        router.register_service(EmulatedService::new(Arc::clone(&engine)));
        Self { router, engine }
    }

    /// Dispatch one `rpcv2Cbor` request for `operation`, with `input` serialized against
    /// `schema`.
    pub(crate) async fn call<I: SerializableStruct>(
        &self,
        operation: &str,
        schema: &Schema,
        input: &I,
    ) -> Answer {
        let codec = CborCodec::default();
        let mut serializer = codec.create_serializer();
        serializer
            .write_struct(schema, input)
            .expect("a serializable request body");
        let request = http::Request::builder()
            .method("POST")
            .uri(format!("/service/{SERVICE_SHAPE}/operation/{operation}"))
            .header("content-type", CBOR_CONTENT_TYPE)
            .header("smithy-protocol", CBOR_PROTOCOL)
            .header(
                "authorization",
                format!(
                    "AWS4-HMAC-SHA256 Credential=AKIDDEMANDGENTEST/20260101/{REGION}/\
                     {SIGNING_NAME}/aws4_request, SignedHeaders=host, Signature=0f0f"
                ),
            )
            .body(bytes::Bytes::from(serializer.finish()))
            .expect("a well-formed test request");
        let response = self.router.dispatch(request).await;
        let header = |name: &str| {
            response
                .headers()
                .get(name)
                .and_then(|value| value.to_str().ok())
                .unwrap_or_default()
                .to_string()
        };
        Answer {
            status: response.status().as_u16(),
            content_type: header("content-type"),
            smithy_protocol: header("smithy-protocol"),
            body: response.body().to_vec(),
        }
    }

    /// `CreateCampaign`, as a call.
    pub(crate) async fn create(&self, input: &CreateCampaignInput) -> Answer {
        self.call("CreateCampaign", CREATE_CAMPAIGN_INPUT_SCHEMA, input)
            .await
    }

    /// `GetCampaign` for an identifier in its id or its ARN form.
    pub(crate) async fn get(&self, catalog: &str, identifier: &str) -> Answer {
        self.call(
            "GetCampaign",
            GET_CAMPAIGN_INPUT_SCHEMA,
            &GetCampaignInput {
                catalog: catalog.to_string(),
                campaign_identifier: identifier.to_string(),
            },
        )
        .await
    }

    pub(crate) async fn update(&self, input: &UpdateCampaignInput) -> Answer {
        self.call("UpdateCampaign", UPDATE_CAMPAIGN_INPUT_SCHEMA, input)
            .await
    }

    pub(crate) async fn delete(&self, catalog: &str, identifier: &str) -> Answer {
        self.call(
            "DeleteCampaign",
            DELETE_CAMPAIGN_INPUT_SCHEMA,
            &DeleteCampaignInput {
                catalog: catalog.to_string(),
                campaign_identifier: identifier.to_string(),
            },
        )
        .await
    }

    pub(crate) async fn list(&self, input: &ListCampaignsInput) -> Answer {
        self.call("ListCampaigns", LIST_CAMPAIGNS_INPUT_SCHEMA, input)
            .await
    }

    /// The campaign ids `ListCampaigns` reports for `input`, in the order it reports
    /// them.
    pub(crate) async fn listed_ids(&self, case: &str, input: &ListCampaignsInput) -> Vec<String> {
        let answered = self.list(input).await;
        assert_eq!(
            answered.status,
            200,
            "case {case:?}: ListCampaigns expected 200, got {} with body {}",
            answered.status,
            answered.render()
        );
        answered
            .output::<ListCampaignsOutput>(case)
            .items
            .into_iter()
            .map(|summary| summary.campaign_id)
            .collect()
    }
}

/// Assert `answer` came back in this protocol's own envelope.
pub(crate) fn assert_envelope(case: &str, answer: &Answer) {
    assert_eq!(
        (
            answer.content_type.as_str(),
            answer.smithy_protocol.as_str()
        ),
        (CBOR_CONTENT_TYPE, CBOR_PROTOCOL),
        "case {case:?}: expected this protocol's envelope headers, got {:?} and {:?} with body {}",
        answer.content_type,
        answer.smithy_protocol,
        answer.render()
    );
}

/// Assert `answer` is the modelled refusal `code` with `status`, and that it carries
/// `message` verbatim.
pub(crate) fn assert_refusal(
    case: &str,
    answer: &Answer,
    status: u16,
    code: &str,
    message: &str,
) {
    assert_eq!(
        answer.status,
        status,
        "case {case:?}: expected HTTP {status}, got {} with body {}",
        answer.status,
        answer.render()
    );
    assert_envelope(case, answer);
    assert!(
        answer.mentions(code),
        "case {case:?}: expected the error class {code}, got {}",
        answer.render()
    );
    assert!(
        answer.mentions(message),
        "case {case:?}: expected the message {message:?}, got {}",
        answer.render()
    );
}

/// The UTC calendar date `days` days from now, read off the same date encoder the
/// service reports one with, so no calendar arithmetic is written here.
pub(crate) fn utc_date_in(days: i64) -> String {
    let span = Duration::from_secs(days.unsigned_abs().saturating_mul(86_400));
    let now = SystemTime::now();
    let at = if days < 0 { now - span } else { now + span };
    state::utc_date(at).expect("the emulator's clock falls on a UTC calendar date")
}

/// The templates a partner can see right now, which is what a campaign can be created
/// from. Read from the catalog itself rather than restated here.
pub(crate) fn effective_templates() -> Vec<&'static state::CampaignTemplateRecord> {
    state::effective_templates(SystemTime::now())
}

/// The one template of the catalog whose `EffectiveDateBegin` has not been reached.
pub(crate) fn upcoming_template() -> &'static state::CampaignTemplateRecord {
    state::catalog()
        .iter()
        .find(|record| !state::is_effective(record, SystemTime::now()))
        .expect("the catalog carries a template that is not yet effective")
}

/// The ARN a campaign of this catalog is minted with in [`REGION`].
pub(crate) fn expected_arn(catalog: &str, campaign_id: &str) -> String {
    format!(
        "arn:aws:partnercentral:{REGION}:{}:catalog/{catalog}/campaign/{campaign_id}",
        engine::EMULATOR_ACCOUNT_ID
    )
}

/// Whether `id` has the modelled `CampaignId` form, `^camp-[A-Za-z0-9]{13}$`.
fn is_campaign_id(id: &str) -> bool {
    id.strip_prefix("camp-").is_some_and(|suffix| {
        suffix.chars().count() == 13 && suffix.chars().all(|character| character.is_ascii_alphanumeric())
    })
}

/// The smallest `CreateCampaign` this service accepts, in [`CATALOG`].
pub(crate) fn create_input(name: &str, template_id: &str) -> CreateCampaignInput {
    CreateCampaignInput {
        catalog: CATALOG.to_string(),
        name: name.to_string(),
        campaign_template_id: template_id.to_string(),
        ..Default::default()
    }
}

/// Create a campaign and answer its `Id`, failing the case when the create is refused.
pub(crate) async fn created_id(
    emulator: &Emulator,
    case: &str,
    input: &CreateCampaignInput,
) -> String {
    let answered = emulator.create(input).await;
    assert_eq!(
        answered.status,
        200,
        "case {case:?}: CreateCampaign expected 200, got {} with body {}",
        answered.status,
        answered.render()
    );
    answered.output::<CreateCampaignOutput>(case).id
}

/// The campaign `identifier` names, failing the case when the read is refused.
pub(crate) async fn read_campaign(emulator: &Emulator, case: &str, identifier: &str) -> Campaign {
    let answered = emulator.get(CATALOG, identifier).await;
    assert_eq!(
        answered.status,
        200,
        "case {case:?}: GetCampaign({identifier}) expected 200, got {} with body {}",
        answered.status,
        answered.render()
    );
    answered.output::<GetCampaignOutput>(case).campaign
}

/// One row of the lifecycle table: a campaign to create, and the member values its
/// reads must report.
struct LifecycleCase {
    case: &'static str,
    name: &'static str,
    /// Which effective template of the catalog to draw from, by position.
    template: usize,
    sender_email: Option<&'static str>,
    privacy_policy_url: Option<&'static str>,
    /// `StartOn` / `EndOn` as a number of days from today, so the rows stay valid
    /// whenever the suite runs.
    start_in_days: Option<i64>,
    end_in_days: Option<i64>,
    budget: Option<&'static str>,
    tags: &'static [(&'static str, &'static str)],
    client_token: Option<&'static str>,
}

#[tokio::test]
async fn a_campaign_is_created_read_updated_listed_and_deleted() {
    // The rows differ along what the rule generalises over: which template (so which
    // language) the campaign draws from, which optional members the create names, a run
    // window given in full, open-ended and absent, tags present and absent, and an
    // idempotency token present and absent.
    let cases = [
        LifecycleCase {
            case: "a campaign with every member this slice stores",
            name: "Q3 migration push",
            template: 0,
            sender_email: Some("campaigns@partner.example"),
            privacy_policy_url: Some("https://partner.example/privacy"),
            start_in_days: Some(1),
            end_in_days: Some(60),
            budget: Some("25000.00"),
            tags: &[("Owner", "demand-gen"), ("CostCentre", "4711")],
            client_token: Some("tok-full"),
        },
        LifecycleCase {
            case: "a campaign naming only what the model requires",
            name: "minimal",
            template: 1,
            sender_email: None,
            privacy_policy_url: None,
            start_in_days: None,
            end_in_days: None,
            budget: None,
            tags: &[],
            client_token: None,
        },
        LifecycleCase {
            case: "a campaign whose run window ends today",
            name: "ends today",
            template: 2,
            sender_email: Some("today@partner.example"),
            privacy_policy_url: None,
            start_in_days: Some(-30),
            end_in_days: Some(0),
            budget: None,
            tags: &[("Owner", "demand-gen")],
            client_token: Some("tok-today"),
        },
    ];

    for row in cases {
        let case = row.case;
        let emulator = Emulator::new();
        let templates = effective_templates();
        let template = templates
            .get(row.template)
            .unwrap_or_else(|| panic!("case {case:?}: the catalog has no template {}", row.template));

        let mut input = create_input(row.name, template.id);
        input.sender_email = row.sender_email.map(str::to_string);
        input.privacy_policy_url = row.privacy_policy_url.map(str::to_string);
        input.start_on = row.start_in_days.map(utc_date_in);
        input.end_on = row.end_in_days.map(utc_date_in);
        input.total_budget = row.budget.map(|amount| MonetaryValue {
            amount: amount.to_string(),
            currency_code: "USD".to_string(),
        });
        input.tags = (!row.tags.is_empty()).then(|| {
            row.tags
                .iter()
                .map(|(key, value)| Tag {
                    key: (*key).to_string(),
                    value: (*value).to_string(),
                })
                .collect()
        });
        input.client_token = row.client_token.map(str::to_string);

        // The create reports the identity of the new campaign, and nothing else.
        let created = emulator.create(&input).await;
        assert_eq!(
            created.status,
            200,
            "case {case:?}: CreateCampaign expected 200, got {} with body {}",
            created.status,
            created.render()
        );
        assert_envelope(case, &created);
        let identity = created.output::<CreateCampaignOutput>(case);
        assert!(
            is_campaign_id(&identity.id),
            "case {case:?}: expected an Id of the form camp-<13 alphanumerics>, got {:?}",
            identity.id
        );
        assert_eq!(
            identity.arn,
            expected_arn(CATALOG, &identity.id),
            "case {case:?}: expected the campaign ARN of this catalog and Region, got {:?}",
            identity.arn
        );

        // A repeat under the same token reports the same identity and creates nothing.
        if row.client_token.is_some() {
            let repeated = emulator.create(&input).await;
            let repeat = repeated.output::<CreateCampaignOutput>(case);
            assert_eq!(
                (repeat.id.as_str(), repeat.arn.as_str()),
                (identity.id.as_str(), identity.arn.as_str()),
                "case {case:?}: a repeat under the same ClientToken must report the first \
                 campaign, got {repeat:?}"
            );
            let listed = emulator
                .listed_ids(case, &list_input(CATALOG))
                .await;
            assert_eq!(
                listed,
                vec![identity.id.clone()],
                "case {case:?}: a repeat under the same ClientToken must create no second \
                 campaign, got {listed:?}"
            );
        }

        // The read rebuilds the campaign from what was stored — by id and by ARN alike.
        for (addressed_by, identifier) in [
            ("its id", identity.id.clone()),
            ("its ARN", identity.arn.clone()),
        ] {
            let campaign = read_campaign(&emulator, case, &identifier).await;
            assert_eq!(
                (
                    campaign.campaign_id.as_str(),
                    campaign.arn.as_str(),
                    campaign.name.as_str(),
                    campaign.campaign_template_id.as_deref(),
                    campaign.language.as_deref(),
                    campaign.sender_email.as_deref(),
                    campaign.privacy_policy_url.as_deref(),
                    campaign.start_on.as_deref(),
                    campaign.end_on.as_deref(),
                ),
                (
                    identity.id.as_str(),
                    identity.arn.as_str(),
                    row.name,
                    Some(template.id),
                    Some(template.language_code),
                    row.sender_email,
                    row.privacy_policy_url,
                    input.start_on.as_deref(),
                    input.end_on.as_deref(),
                ),
                "case {case:?}: addressed by {addressed_by}, the campaign must report what was \
                 stored, got {campaign:?}"
            );
            assert_eq!(
                campaign.total_budget.as_ref().map(|budget| budget.amount.as_str()),
                row.budget,
                "case {case:?}: addressed by {addressed_by}, expected the stored budget, got {:?}",
                campaign.total_budget
            );
            // No asset is live, and the window has not closed, so the campaign is a
            // draft and nothing is blocked.
            assert_eq!(
                campaign.state, "DRAFT",
                "case {case:?}: addressed by {addressed_by}, a campaign with no live asset and \
                 an open window is DRAFT, got {:?}",
                campaign.state
            );
            assert!(
                campaign.activation_issues.is_empty(),
                "case {case:?}: addressed by {addressed_by}, a campaign with no asset blocks \
                 nothing, got {:?}",
                campaign.activation_issues
            );
            assert!(
                campaign.created_at.is_some() && campaign.updated_at.is_some(),
                "case {case:?}: addressed by {addressed_by}, both audit instants must be \
                 reported, got {:?} and {:?}",
                campaign.created_at,
                campaign.updated_at
            );
        }

        // The update applies the members it names and leaves every other as it stands.
        let before = read_campaign(&emulator, case, &identity.id).await;
        let renamed = format!("{} (renamed)", row.name);
        let updated = emulator
            .update(&UpdateCampaignInput {
                catalog: CATALOG.to_string(),
                campaign_identifier: identity.arn.clone(),
                name: Some(renamed.clone()),
                partner_address: Some("1 Partner Way, Seattle WA".to_string()),
                ..Default::default()
            })
            .await;
        assert_eq!(
            updated.status,
            200,
            "case {case:?}: UpdateCampaign expected 200, got {} with body {}",
            updated.status,
            updated.render()
        );
        assert_eq!(
            updated.output::<UpdateCampaignOutput>(case).id,
            identity.id,
            "case {case:?}: the update reports the campaign it applied to"
        );
        let after = read_campaign(&emulator, case, &identity.id).await;
        assert_eq!(
            (
                after.name.as_str(),
                after.partner_address.as_deref(),
                after.sender_email.as_deref(),
                after.campaign_template_id.as_deref(),
                after.start_on.as_deref(),
                after.end_on.as_deref(),
            ),
            (
                renamed.as_str(),
                Some("1 Partner Way, Seattle WA"),
                before.sender_email.as_deref(),
                before.campaign_template_id.as_deref(),
                before.start_on.as_deref(),
                before.end_on.as_deref(),
            ),
            "case {case:?}: the update must change only the members it named, got {after:?}"
        );
        assert_eq!(
            after.created_at, before.created_at,
            "case {case:?}: an update does not move CreatedAt"
        );
        assert!(
            after.updated_at >= before.updated_at,
            "case {case:?}: expected UpdatedAt to move forward, got {:?} after {:?}",
            after.updated_at,
            before.updated_at
        );

        // The list reports the stored campaign, by the same values the read reports.
        let listing = emulator.list(&list_input(CATALOG)).await;
        let items = listing.output::<ListCampaignsOutput>(case).items;
        assert_eq!(
            items.len(),
            1,
            "case {case:?}: expected exactly the one stored campaign, got {items:?}"
        );
        let summary = &items[0];
        assert_eq!(
            (
                summary.campaign_id.as_str(),
                summary.arn.as_str(),
                summary.name.as_str(),
                summary.state.as_str(),
                summary.language.as_deref(),
                summary.start_on.as_deref(),
                summary.end_on.as_deref(),
            ),
            (
                identity.id.as_str(),
                identity.arn.as_str(),
                renamed.as_str(),
                after.state.as_str(),
                after.language.as_deref(),
                after.start_on.as_deref(),
                after.end_on.as_deref(),
            ),
            "case {case:?}: the summary must report the stored campaign, got {summary:?}"
        );

        // A filter nothing matches reports an empty page rather than everything.
        let mut no_match = list_input(CATALOG);
        no_match.name = Some("no campaign is called this".to_string());
        assert!(
            emulator.listed_ids(case, &no_match).await.is_empty(),
            "case {case:?}: a Name no campaign carries must report no item"
        );

        // The delete removes it, and every read of it then answers not-found.
        let deleted = emulator.delete(CATALOG, &identity.id).await;
        assert_eq!(
            deleted.status,
            200,
            "case {case:?}: DeleteCampaign expected 200, got {} with body {}",
            deleted.status,
            deleted.render()
        );
        let gone = emulator.get(CATALOG, &identity.id).await;
        assert_refusal(
            case,
            &gone,
            404,
            "ResourceNotFoundException",
            &format!("Campaign {} does not exist.", identity.id),
        );
        assert!(
            emulator.listed_ids(case, &list_input(CATALOG)).await.is_empty(),
            "case {case:?}: a deleted campaign must leave the listing"
        );
        // "Deleting a campaign that is already gone returns ResourceNotFoundException,
        // and nothing is left behind by a retry."
        let again = emulator.delete(CATALOG, &identity.id).await;
        assert_refusal(
            case,
            &again,
            404,
            "ResourceNotFoundException",
            &format!("Campaign {} does not exist.", identity.id),
        );
    }
}

/// An unfiltered `ListCampaigns` of one catalog.
pub(crate) fn list_input(catalog: &str) -> ListCampaignsInput {
    ListCampaignsInput {
        catalog: catalog.to_string(),
        ..Default::default()
    }
}

#[tokio::test]
async fn the_state_a_read_reports_is_derived_from_end_on() {
    let emulator = Emulator::new();
    let template = effective_templates()[0].id.to_string();

    // The rows differ along where `EndOn` falls relative to today, which is the whole
    // input to the first rule of `CampaignState`: a window that has not closed is a
    // draft while no asset is live, and today's date still counts as inside it because
    // `EndOn` is inclusive.
    for (case, end_in_days) in [
        ("a window closing today", Some(0)),
        ("a window closing tomorrow", Some(1)),
        ("a campaign with no EndOn at all", None),
    ] {
        let mut input = create_input(case, &template);
        input.end_on = end_in_days.map(utc_date_in);
        let id = created_id(&emulator, case, &input).await;
        let campaign = read_campaign(&emulator, case, &id).await;
        assert_eq!(
            campaign.state, "DRAFT",
            "case {case:?}: expected DRAFT, got {:?}",
            campaign.state
        );
        assert!(
            emulator.delete(CATALOG, &id).await.status == 200,
            "case {case:?}: the campaign is removed before the next row"
        );
    }

    // A campaign whose window has CLOSED is completed on every read, is listed under
    // that state, and refuses an update — none of which any call did: the window closed
    // because the date moved past `EndOn`, which this suite brings about by storing the
    // window the clock would have left behind.
    let case = "a window that closed yesterday";
    let mut input = create_input(case, &template);
    input.end_on = Some(utc_date_in(0));
    let id = created_id(&emulator, case, &input).await;
    let closed_on = close_run_window(&emulator, &id);

    let campaign = read_campaign(&emulator, case, &id).await;
    assert_eq!(
        campaign.state, "COMPLETED",
        "case {case:?}: a UTC date after EndOn completes the campaign, got {:?}",
        campaign.state
    );
    let mut completed_only = list_input(CATALOG);
    completed_only.states = Some(vec!["COMPLETED".to_string()]);
    assert_eq!(
        emulator.listed_ids(case, &completed_only).await,
        vec![id.clone()],
        "case {case:?}: the listing derives the same state the read does"
    );
    let mut drafts_only = list_input(CATALOG);
    drafts_only.states = Some(vec!["DRAFT".to_string()]);
    assert!(
        emulator.listed_ids(case, &drafts_only).await.is_empty(),
        "case {case:?}: a completed campaign is not a draft"
    );

    let refused = emulator
        .update(&UpdateCampaignInput {
            catalog: CATALOG.to_string(),
            campaign_identifier: id.clone(),
            name: Some("too late".to_string()),
            ..Default::default()
        })
        .await;
    assert_refusal(
        case,
        &refused,
        409,
        "ConflictException",
        &format!("Campaign {id} completed after its EndOn of {closed_on} and cannot be changed."),
    );
    assert_eq!(
        read_campaign(&emulator, case, &id).await.name,
        case,
        "case {case:?}: a refused update changes nothing"
    );
    // "A completed campaign can be deleted."
    assert_eq!(
        emulator.delete(CATALOG, &id).await.status,
        200,
        "case {case:?}: a completed campaign is still deletable"
    );
}

/// Store the run window a campaign would have once the UTC date has moved past its
/// `EndOn`, and answer that date.
///
/// The store is reached directly rather than waiting for a day to pass: `EndOn` is the
/// only input to the completed branch of `CampaignState`, so a campaign whose stored
/// window closed yesterday is exactly what the clock would have left behind.
fn close_run_window(emulator: &Emulator, campaign_id: &str) -> String {
    let yesterday = utc_date_in(-1);
    let store =
        state::service_state(&*emulator.engine).expect("the service's store is mounted here");
    let mut guard = store.lock();
    let campaign = guard
        .campaign_mut(CATALOG, campaign_id)
        .unwrap_or_else(|| panic!("campaign {campaign_id} is not in the store"));
    campaign.end_on = Some(yesterday.clone());
    yesterday
}

#[tokio::test]
async fn list_campaigns_filters_sorts_and_pages() {
    let emulator = Emulator::new();
    let template = effective_templates()[0].id.to_string();

    // Five campaigns differing along every member a filter or a sort reads: the name,
    // the run window, and whether a `StartOn` was given at all.
    let mut ids = Vec::new();
    for (name, start_in_days) in [
        ("Alpha launch", Some(1)),
        ("beta launch", Some(2)),
        ("Gamma webinar", Some(3)),
        ("Delta webinar", None),
        ("epsilon roadshow", Some(0)),
    ] {
        let mut input = create_input(name, &template);
        input.start_on = start_in_days.map(utc_date_in);
        input.end_on = Some(utc_date_in(90));
        ids.push((name, created_id(&emulator, name, &input).await));
    }
    let id_of = |name: &str| {
        ids.iter()
            .find(|(stored, _)| *stored == name)
            .map(|(_, id)| id.clone())
            .unwrap_or_else(|| panic!("no campaign named {name} was created"))
    };

    // `Name`: "only campaigns whose name contains this, compared case-insensitively".
    for (case, filter, expected) in [
        ("a whole name", "Alpha launch", vec!["Alpha launch"]),
        (
            "a substring two names share",
            "launch",
            vec!["Alpha launch", "beta launch"],
        ),
        (
            "the same substring in another case",
            "LAUNCH",
            vec!["Alpha launch", "beta launch"],
        ),
        (
            "a substring of a word",
            "webin",
            vec!["Delta webinar", "Gamma webinar"],
        ),
        ("a substring nothing carries", "zeta", vec![]),
    ] {
        let mut input = list_input(CATALOG);
        input.name = Some(filter.to_string());
        let mut reported = emulator.listed_ids(case, &input).await;
        reported.sort();
        let mut wanted: Vec<String> = expected.iter().map(|name| id_of(name)).collect();
        wanted.sort();
        assert_eq!(
            reported, wanted,
            "case {case:?}: expected the campaigns whose name contains {filter:?}, got \
             {reported:?}"
        );
    }

    // `CampaignIdentifiers`: "only these campaigns, each named by its id or its campaign
    // ARN. A campaign that does not exist, was deleted, or whose ARN names another
    // account or catalog is left out rather than refused."
    let alpha = id_of("Alpha launch");
    let beta = id_of("beta launch");
    for (case, identifiers, expected) in [
        ("one id", vec![alpha.clone()], vec![alpha.clone()]),
        (
            "one ARN",
            vec![expected_arn(CATALOG, &beta)],
            vec![beta.clone()],
        ),
        (
            "an id and an ARN together",
            vec![alpha.clone(), expected_arn(CATALOG, &beta)],
            vec![alpha.clone(), beta.clone()],
        ),
        (
            "an ARN naming another catalog",
            vec![expected_arn(OTHER_CATALOG, &alpha)],
            vec![],
        ),
        (
            "an ARN naming another account",
            vec![format!(
                "arn:aws:partnercentral:{REGION}:123456789012:catalog/{CATALOG}/campaign/{alpha}"
            )],
            vec![],
        ),
        (
            "an id no campaign carries",
            vec!["camp-0000000000000".to_string()],
            vec![],
        ),
    ] {
        let mut input = list_input(CATALOG);
        input.campaign_identifiers = Some(identifiers.clone());
        let mut reported = emulator.listed_ids(case, &input).await;
        reported.sort();
        let mut wanted = expected.clone();
        wanted.sort();
        assert_eq!(
            reported, wanted,
            "case {case:?}: expected the campaigns {identifiers:?} name, got {reported:?}"
        );
    }

    // `Sort`: the attribute and the direction the request asks for, ties by id ascending.
    let by_name_ascending: Vec<String> = ["Alpha launch", "beta launch", "Delta webinar", "epsilon roadshow", "Gamma webinar"]
        .iter()
        .map(|name| id_of(name))
        .collect();
    let mut by_name = list_input(CATALOG);
    by_name.sort = Some(CampaignSort {
        sort_by: "NAME".to_string(),
        sort_order: "ASCENDING".to_string(),
    });
    assert_eq!(
        emulator.listed_ids("NAME ascending", &by_name).await,
        by_name_ascending,
        "NAME is compared without case, so the order is alphabetical whatever the case"
    );
    by_name.sort = Some(CampaignSort {
        sort_by: "NAME".to_string(),
        sort_order: "DESCENDING".to_string(),
    });
    let mut reversed = by_name_ascending.clone();
    reversed.reverse();
    assert_eq!(
        emulator.listed_ids("NAME descending", &by_name).await,
        reversed,
        "DESCENDING reverses the attribute's own order"
    );

    // "A campaign with none sorts last in either direction."
    let without_start = id_of("Delta webinar");
    for order in ["ASCENDING", "DESCENDING"] {
        let mut by_start = list_input(CATALOG);
        by_start.sort = Some(CampaignSort {
            sort_by: "START_ON".to_string(),
            sort_order: order.to_string(),
        });
        let reported = emulator
            .listed_ids(&format!("START_ON {order}"), &by_start)
            .await;
        assert_eq!(
            reported.last(),
            Some(&without_start),
            "case START_ON {order}: a campaign with no StartOn sorts last, got {reported:?}"
        );
    }

    // Pagination: every campaign is reported exactly once, in the order the sort asks,
    // and the last page carries no token.
    let mut page = list_input(CATALOG);
    page.max_results = Some(2);
    page.sort = Some(CampaignSort {
        sort_by: "NAME".to_string(),
        sort_order: "ASCENDING".to_string(),
    });
    let mut walked: Vec<String> = Vec::new();
    let mut pages = 0;
    loop {
        let answered = emulator.list(&page).await;
        assert_eq!(
            answered.status,
            200,
            "paging: ListCampaigns expected 200, got {} with body {}",
            answered.status,
            answered.render()
        );
        let output = answered.output::<ListCampaignsOutput>("paging");
        assert!(
            output.items.len() <= 2,
            "paging: a page must hold at most the MaxResults asked for, got {:?}",
            output.items
        );
        walked.extend(output.items.iter().map(|item| item.campaign_id.clone()));
        pages += 1;
        assert!(pages <= 10, "paging: expected to finish, walked {walked:?}");
        match output.next_token {
            Some(token) => page.next_token = Some(token),
            None => break,
        }
    }
    assert_eq!(
        walked, by_name_ascending,
        "paging: the pages together are the whole sorted sequence, got {walked:?}"
    );

    // A token is bound to the filters and the sort it was issued for, and to the store
    // that issued it.
    let mut first = list_input(CATALOG);
    first.max_results = Some(1);
    let token = emulator
        .list(&first)
        .await
        .output::<ListCampaignsOutput>("a first page")
        .next_token
        .expect("a first page of five campaigns offers a continuation");
    for (case, mut input) in [
        ("the same token under another Name filter", {
            let mut changed = list_input(CATALOG);
            changed.name = Some("launch".to_string());
            changed
        }),
        ("the same token under another sort", {
            let mut changed = list_input(CATALOG);
            changed.sort = Some(CampaignSort {
                sort_by: "NAME".to_string(),
                sort_order: "ASCENDING".to_string(),
            });
            changed
        }),
    ] {
        input.next_token = Some(token.clone());
        let refused = emulator.list(&input).await;
        assert_refusal(
            case,
            &refused,
            400,
            "ValidationException",
            "The NextToken is not a continuation token this operation issued for this request's \
             filters and sort order.",
        );
    }
    let mut forged = list_input(CATALOG);
    forged.next_token = Some("Zm9yZ2VkIHRva2VuIHRoYXQgaXMgbG9uZyBlbm91Z2ggdG8gbG9vayByZWFs".to_string());
    assert_refusal(
        "a token this store never minted",
        &emulator.list(&forged).await,
        400,
        "ValidationException",
        "The NextToken is not a continuation token this operation issued for this request's \
         filters and sort order.",
    );
    // `MaxResults` may change between pages, so the same token is served under a
    // different page size.
    let mut resized = list_input(CATALOG);
    resized.max_results = Some(3);
    resized.next_token = Some(token);
    let served = emulator.list(&resized).await;
    assert_eq!(
        served.status,
        200,
        "a token stays valid when only MaxResults changes, got {} with body {}",
        served.status,
        served.render()
    );
}

#[tokio::test]
async fn a_create_is_refused_on_its_own_terms_and_stores_nothing() {
    let emulator = Emulator::new();
    let template = effective_templates()[0].id.to_string();
    let today = utc_date_in(0);
    let yesterday = utc_date_in(-1);

    // A template the catalog does not hold, and one it holds but has not published yet:
    // both name no template a partner can create from.
    for (case, identifier) in [
        ("an id no template carries", "tmpl-0000000000000".to_string()),
        (
            "a template whose EffectiveDateBegin is in the future",
            upcoming_template().id.to_string(),
        ),
    ] {
        let refused = emulator.create(&create_input(case, &identifier)).await;
        assert_refusal(
            case,
            &refused,
            404,
            "ResourceNotFoundException",
            &format!("Campaign template {identifier} does not exist."),
        );
    }

    // The run-window rules, with every failing member reported together.
    let mut past_end = create_input("past end", &template);
    past_end.end_on = Some(yesterday.clone());
    let refused = emulator.create(&past_end).await;
    assert_refusal(
        "an EndOn in the past",
        &refused,
        400,
        "ValidationException",
        &format!("EndOn {yesterday} is in the past: a campaign's last day must be {today} or later."),
    );
    assert!(
        refused.mentions("FIELD_VALIDATION_FAILED") && refused.mentions("INVALID_VALUE"),
        "an EndOn in the past: expected the coarse reason and the field code, got {}",
        refused.render()
    );

    let start = utc_date_in(10);
    let end = utc_date_in(5);
    let mut inverted = create_input("inverted window", &template);
    inverted.start_on = Some(start.clone());
    inverted.end_on = Some(end.clone());
    assert_refusal(
        "a StartOn after EndOn",
        &emulator.create(&inverted).await,
        400,
        "ValidationException",
        &format!("StartOn {start} must not be after EndOn {end}."),
    );

    // Both at once: `FieldList` carries "every field that failed, not just the first".
    let mut both = create_input("both bounds wrong", &template);
    both.start_on = Some(today.clone());
    both.end_on = Some(yesterday.clone());
    let refused = emulator.create(&both).await;
    assert_refusal(
        "both run-window rules broken",
        &refused,
        400,
        "ValidationException",
        "The following fields failed validation: EndOn, StartOn.",
    );

    // The tag rules, each refused with a message naming the key.
    for (case, tags, message) in [
        (
            "a key reserved for AWS",
            vec![("aws:created-by", "someone")],
            "The tag key aws:created-by is reserved for AWS use: a key beginning with \"aws:\" \
             cannot be applied."
                .to_string(),
        ),
        (
            "the same key in another case",
            vec![("AWS:Created-By", "someone")],
            "The tag key AWS:Created-By is reserved for AWS use: a key beginning with \"aws:\" \
             cannot be applied."
                .to_string(),
        ),
        (
            "a key repeated in the list",
            vec![("Owner", "one"), ("Owner", "two")],
            "The tag key Owner appears more than once in Tags.".to_string(),
        ),
    ] {
        let mut input = create_input(case, &template);
        input.tags = Some(
            tags.iter()
                .map(|(key, value)| Tag {
                    key: (*key).to_string(),
                    value: (*value).to_string(),
                })
                .collect(),
        );
        assert_refusal(
            case,
            &emulator.create(&input).await,
            400,
            "ValidationException",
            &message,
        );
    }

    // Nothing above was stored: a refused create creates nothing.
    assert!(
        emulator
            .listed_ids("after every refusal", &list_input(CATALOG))
            .await
            .is_empty(),
        "a refused create must leave the store empty"
    );
}

#[tokio::test]
async fn an_identifier_that_names_no_campaign_is_refused_by_every_operation_taking_one() {
    let emulator = Emulator::new();
    let template = effective_templates()[0].id.to_string();
    let live = created_id(&emulator, "a campaign to delete", &create_input("live", &template)).await;
    let deleted = created_id(
        &emulator,
        "a campaign to delete",
        &create_input("deleted", &template),
    )
    .await;
    assert_eq!(
        emulator.delete(CATALOG, &deleted).await.status,
        200,
        "the campaign whose id must then resolve to nothing is removed"
    );

    // Every identifier the modelled pattern admits that names no campaign of this
    // catalog: one never minted, one removed, and the ARN forms naming another catalog
    // or another account.
    for (case, identifier) in [
        ("an id that was never minted", "camp-ZZZZZZZZZZZZZ".to_string()),
        ("an id whose campaign was deleted", deleted.clone()),
        (
            "an ARN naming another catalog",
            expected_arn(OTHER_CATALOG, &live),
        ),
        (
            "an ARN naming another account",
            format!(
                "arn:aws:partnercentral:{REGION}:123456789012:catalog/{CATALOG}/campaign/{live}"
            ),
        ),
    ] {
        let message = format!("Campaign {identifier} does not exist.");
        assert_refusal(
            case,
            &emulator.get(CATALOG, &identifier).await,
            404,
            "ResourceNotFoundException",
            &message,
        );
        assert_refusal(
            case,
            &emulator
                .update(&UpdateCampaignInput {
                    catalog: CATALOG.to_string(),
                    campaign_identifier: identifier.clone(),
                    name: Some("nobody".to_string()),
                    ..Default::default()
                })
                .await,
            404,
            "ResourceNotFoundException",
            &message,
        );
        assert_refusal(
            case,
            &emulator.delete(CATALOG, &identifier).await,
            404,
            "ResourceNotFoundException",
            &message,
        );
    }

    // The campaign that WAS stored is untouched by any of it.
    assert_eq!(
        read_campaign(&emulator, "the stored campaign", &live).await.name,
        "live",
        "a refused request must not change the campaign that exists"
    );
}

#[tokio::test]
async fn an_update_under_the_same_client_token_is_applied_once() {
    let emulator = Emulator::new();
    let template = effective_templates()[0].id.to_string();
    let case = "an update replayed under its own token";
    let id = created_id(&emulator, case, &create_input("idempotent", &template)).await;

    let update = UpdateCampaignInput {
        catalog: CATALOG.to_string(),
        campaign_identifier: id.clone(),
        name: Some("applied once".to_string()),
        client_token: Some("tok-update-1".to_string()),
        ..Default::default()
    };
    assert_eq!(
        emulator
            .update(&update)
            .await
            .output::<UpdateCampaignOutput>(case)
            .id,
        id,
        "case {case:?}: the update reports the campaign it applied to"
    );
    let applied = read_campaign(&emulator, case, &id).await;

    // A later request carrying the same token reports the same campaign and applies
    // nothing again, so a member the replay names is not written over the applied one.
    let mut replay = update.clone();
    replay.name = Some("a different name under the same token".to_string());
    assert_eq!(
        emulator
            .update(&replay)
            .await
            .output::<UpdateCampaignOutput>(case)
            .id,
        id,
        "case {case:?}: a replay reports the same campaign"
    );
    let after = read_campaign(&emulator, case, &id).await;
    assert_eq!(
        (after.name.as_str(), after.updated_at),
        (applied.name.as_str(), applied.updated_at),
        "case {case:?}: a replay under the same ClientToken applies nothing again, got {after:?}"
    );

    // Another token is another request, so it applies.
    let mut next = update.clone();
    next.name = Some("applied again".to_string());
    next.client_token = Some("tok-update-2".to_string());
    assert_eq!(
        emulator
            .update(&next)
            .await
            .output::<UpdateCampaignOutput>(case)
            .id,
        id,
        "case {case:?}: a new token is a new request"
    );
    assert_eq!(
        read_campaign(&emulator, case, &id).await.name,
        "applied again",
        "case {case:?}: an update under a token this store has not seen is applied"
    );
}
