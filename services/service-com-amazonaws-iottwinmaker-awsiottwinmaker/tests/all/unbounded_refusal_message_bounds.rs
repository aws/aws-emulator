// SPDX-License-Identifier: CC0-1.0

//! Whether a refusal this service builds from request text the model bounds as
//! widely as — or more widely than — the member the reason travels on fits that
//! member, end to end over HTTP.
//!
//! RULE under test: every modelled refusal of this service states its reason on
//! the `ErrorMessage` shape, `@length(0..2048)`, so a reason naming text the
//! request chose is rendered within those 2048 characters — reported exactly as
//! the service words it where it fits, and at the bound with its leading text
//! and its trailing clause kept where it does not.
//!
//! The text each row names is text no narrower bound stops: an ARN, whose
//! `TwinMakerArn` is bounded at the very 2048 characters the reason has to fit
//! in; a `syncSource` label, a plain pattern string the model does not bound at
//! all; and a set of tag keys, each bounded at 128 characters but named as many
//! at a time as the request carried — as the two tagging calls report them, and
//! as each create that takes a `tags` map of its own does.
//!
//! A modelled ENUMERATION value is the one kind of request text that cannot
//! reach such a reason: the value list is a constraint the boundary validator
//! resolves before any handler runs, which
//! [`an_out_of_enum_value_is_refused_at_the_boundary`] pins over every member of
//! this service that carries one.
//!
//! `tests/all/error_message_bounds.rs` asserts the same rule where a single
//! `componentPath` is what overruns the member; this suite covers the refusals
//! built from the OTHER unbounded request text this service interpolates, over
//! every operation that interpolates it. The bound and what the rule asserts
//! about a reported reason come from that suite
//! ([`crate::error_message_bounds::assert_within_bound`]), so this crate states
//! them once.
//!
//! The rows differ along what the rule generalises over: WHICH operation reports
//! the reason, WHICH member the text comes from, WHAT KIND of text it is (an
//! ARN, an out-of-enum value, a label, a set of keys), and its SIZE — text short
//! enough for the reason to fit, text whose reason lands exactly on the member's
//! maximum, one character past it, and either the text's own modelled maximum or,
//! for text the model does not bound at all, a size far past what the reason
//! admits.

use serde_json::json;
use service_com_amazonaws_iottwinmaker_awsiottwinmaker::state::{
    PRICING_MODE_BASIC, RESERVED_TAG_KEY_PREFIX, SYNC_SOURCE_SITEWISE,
};

use crate::error_message_bounds::{assert_within_bound, ERROR_MESSAGE_MAX};
use crate::harness::{seed_sync_job, workspace_arn, Emulator, Resp, SYNC_ROLE};

/// The workspace every case below names where it needs one to exist.
const WORKSPACE: &str = "msgbounds";

/// The greatest number of characters the modelled `TwinMakerArn` admits —
/// `@length(20..2048)`, from `smithy_query(query="shape",shape="TwinMakerArn")`
/// — which is the member an IoT TwinMaker end of a transfer names a workspace on
/// and the member every tagging operation addresses a resource on.
const TWIN_MAKER_ARN_MAX: usize = 2048;

/// The greatest number of keys the modelled `TagMap` and `TagKeyList` admit
/// (`@length(0..50)`), and the greatest number of characters one `TagKey` admits
/// (`@length(1..128)`), from
/// `smithy_query(query="shape",shape="TagMap,TagKeyList,TagKey")`.
const TAG_KEYS_MAX: usize = 50;
/// See [`TAG_KEYS_MAX`].
const TAG_KEY_MAX: usize = 128;

/// An Amazon S3 location the model admits for a source
/// (`S3SourceConfiguration$location`), carried by the end of a transfer a row is
/// not refusing.
const S3_SOURCE: &str = "arn:aws:s3:::metadata-bucket/entities.json";
/// An Amazon S3 location the model admits for a destination
/// (`S3DestinationConfiguration$location`).
const S3_DESTINATION: &str = "arn:aws:s3:::metadata-bucket";

/// The shortest workspace ARN the cases below name: one this service holds no
/// workspace for, so every reader of it has to report the miss.
fn unheld_workspace_arn_base() -> String {
    workspace_arn("absentWorkspace")
}

/// A syntactically legal workspace ARN of exactly `length` characters that this
/// service holds no workspace for.
///
/// The padding is appended inside the resource segment, which the modelled
/// `TwinMakerArn` pattern admits (`[\/a-zA-Z0-9_\-\.:]+`), so the value a case
/// sends is one the boundary validator passes and the handler has to resolve.
fn unheld_workspace_arn(length: usize) -> String {
    let mut arn = unheld_workspace_arn_base();
    assert!(
        arn.chars().count() <= length,
        "an unheld workspace ARN of {length} characters cannot be shorter than {arn:?}"
    );
    while arn.chars().count() < length {
        arn.push('a');
    }
    arn
}

/// The smallest length a case drives a member at: short enough that every
/// reason built from it fits, whatever fixed text the reason adds.
const SHORT: usize = 8;

/// The sizes the rule is asserted over for a reason whose fixed text is
/// `fixed` characters long and whose variable text is at least `smallest`
/// characters: one that fits, one landing exactly on the member's maximum, one
/// character past it, and `widest` — the variable text's own modelled maximum,
/// or a size far past what the reason admits where the model declares none.
fn sizes(fixed: usize, smallest: usize, widest: usize) -> Vec<(String, usize)> {
    vec![
        (
            "text short enough for the reason to fit".to_string(),
            smallest,
        ),
        (
            "text whose reason renders exactly the characters the member admits".to_string(),
            ERROR_MESSAGE_MAX - fixed,
        ),
        (
            "text whose reason renders one character past the member's maximum".to_string(),
            ERROR_MESSAGE_MAX - fixed + 1,
        ),
        ("the widest text the request may carry".to_string(), widest),
    ]
}

/// The reason a reader of this service words for a resource no ARN in its index
/// names — the construction `ListTagsForResource`, `TagResource` and
/// `UntagResource` share.
fn resource_not_found(resource_arn: &str) -> String {
    format!("Resource {resource_arn} not found.")
}

/// The reason every operation that resolves a workspace in order to read or
/// write something it contains words for a workspace this service does not hold.
fn workspace_not_found(workspace: &str) -> String {
    format!("Unable to find workspace for id: {workspace}")
}

/// The reserved-namespace reason a tag writer words for the `keys` it refused,
/// stated on `member` — the one construction every tag writer of this service
/// reports, BEFORE the bound of the member it travels on is applied, which is
/// why this suite words it rather than reading the service's own (already
/// bounded) construction of it.
pub(crate) fn reserved_keys_reason(member: &str, keys: &[String]) -> String {
    let mut named: Vec<&str> = keys.iter().map(String::as_str).collect();
    named.sort_unstable();
    named.dedup();
    let noun = if named.len() == 1 { "key" } else { "keys" };
    format!(
        "{member} names the tag {noun} {} in the {RESERVED_TAG_KEY_PREFIX} namespace, which is \
         reserved for the tags Amazon Web Services applies itself: such a key cannot be created, \
         changed or removed by a caller.",
        named.join(", ")
    )
}

/// `count` distinct keys in the reserved namespace, each of the longest the
/// modelled `TagKey` admits, so the reason naming them all is as long as a legal
/// request can make it.
fn reserved_keys(count: usize) -> Vec<String> {
    reserved_keys_padded(count, TAG_KEY_MAX, 'k')
}

/// `count` distinct keys in the reserved namespace, each padded to `chars`
/// characters with `filler` and named so their sorted order — the order a reason
/// reports them in — is the order they are built in.
///
/// The ONE construction of a row's key set, so a suite about how the reason is
/// MEASURED (`tests/all/unbounded_member_refusal_bounds.rs`, which pads with
/// multi-byte letters) builds its keys the same way this one does.
pub(crate) fn reserved_keys_padded(count: usize, chars: usize, filler: char) -> Vec<String> {
    (0..count)
        .map(|index| {
            let head = format!("{RESERVED_TAG_KEY_PREFIX}{index:03}");
            let padding = chars.saturating_sub(head.chars().count());
            format!(
                "{head}{}",
                std::iter::repeat_n(filler, padding).collect::<String>()
            )
        })
        .collect()
}

/// One end of a metadata transfer whose refusal names text the request chose.
#[derive(Debug, Clone, Copy)]
enum TransferEnd {
    /// The workspace ARN a source names, which the store holds no workspace for.
    SourceWorkspace,
    /// The same, named by the destination.
    DestinationWorkspace,
}

/// Every end of a transfer whose refusal interpolates such text.
const TRANSFER_ENDS: [TransferEnd; 2] = [
    TransferEnd::SourceWorkspace,
    TransferEnd::DestinationWorkspace,
];

impl TransferEnd {
    /// The text of `length` characters this end carries for a case.
    fn text(self, length: usize) -> String {
        unheld_workspace_arn(length)
    }

    /// The sizes this end is driven at: the ARN is bounded by the model at the
    /// very number of characters the reason has to fit in.
    fn sizes(self) -> Vec<(String, usize)> {
        let fixed = self.whole("").chars().count();
        sizes(
            fixed,
            unheld_workspace_arn_base().chars().count(),
            TWIN_MAKER_ARN_MAX,
        )
    }

    /// Drive `CreateMetadataTransferJob` with `text` at this end, every other
    /// member of the request describing a transfer the service could run.
    async fn send(self, emulator: &Emulator, text: &str) -> Resp {
        let request = match self {
            TransferEnd::SourceWorkspace => json!({
                "sources": [{
                    "type": "iottwinmaker",
                    "iotTwinMakerConfiguration": {"workspace": text},
                }],
                "destination": {
                    "type": "s3",
                    "s3Configuration": {"location": S3_DESTINATION},
                },
            }),
            TransferEnd::DestinationWorkspace => json!({
                "sources": [{"type": "s3", "s3Configuration": {"location": S3_SOURCE}}],
                "destination": {
                    "type": "iottwinmaker",
                    "iotTwinMakerConfiguration": {"workspace": text},
                },
            }),
        };
        emulator.create_metadata_transfer_job(request).await
    }

    /// The status and error code this end's refusal carries.
    fn refusal(self) -> (u16, &'static str) {
        (404, "ResourceNotFoundException")
    }

    /// The reason the service words for `text` at this end, in full — before the
    /// bound of the member it travels on is applied.
    fn whole(self, text: &str) -> String {
        workspace_not_found(text)
    }
}

/// RULE: `CreateMetadataTransferJob` states a refusal it builds from the request
/// — a workspace named by an ARN the model bounds as widely as the member the
/// reason travels on — within the 2048 characters that member admits, whatever
/// the request carried.
#[tokio::test]
async fn a_metadata_transfer_create_refusal_fits_its_modelled_member() {
    let emulator = Emulator::new();
    assert_eq!(
        emulator.create_workspace(WORKSPACE).await.status,
        200,
        "the workspace the cases are created beside must exist"
    );

    for end in TRANSFER_ENDS {
        for (size_case, length) in end.sizes() {
            let case = format!("{end:?} with {size_case}");
            let text = end.text(length);
            let (status, error) = end.refusal();
            let refused = end.send(&emulator, &text).await;
            assert_eq!(
                (refused.status, refused.error_type.as_deref()),
                (status, Some(error)),
                "case {case:?}: expected {status} {error}, got {refused:?}"
            );
            let reported = refused
                .member("message")
                .as_str()
                .unwrap_or_default()
                .to_string();
            assert_within_bound(&case, &reported, &end.whole(&text), ERROR_MESSAGE_MAX);
        }
    }
}

/// One member of this service carrying a MODELLED ENUMERATION, whose value the
/// boundary validator resolves against the model's value list before any handler
/// runs.
#[derive(Debug, Clone, Copy)]
enum OutOfEnum {
    /// `ListMetadataTransferJobs$sourceType`.
    TransferSourceType,
    /// `ListMetadataTransferJobs$destinationType`.
    TransferDestinationType,
    /// `ListMetadataTransferJobs`' `state` filter.
    TransferJobState,
    /// `CreateMetadataTransferJob`'s `sources[].type`.
    TransferSourceEndType,
    /// `CreateMetadataTransferJob`'s `destination.type`.
    TransferDestinationEndType,
    /// `ListSyncResources`' `resourceType` filter.
    SyncResourceType,
    /// `ListSyncResources`' `state` filter.
    SyncResourceState,
    /// The `dataType.type` a `CreateComponentType` definition declares.
    DefinitionDataType,
    /// `UpdatePricingPlan$pricingMode`.
    PricingMode,
}

/// Every member the rows drive an out-of-enum value at.
const OUT_OF_ENUM: [OutOfEnum; 9] = [
    OutOfEnum::TransferSourceType,
    OutOfEnum::TransferDestinationType,
    OutOfEnum::TransferJobState,
    OutOfEnum::TransferSourceEndType,
    OutOfEnum::TransferDestinationEndType,
    OutOfEnum::SyncResourceType,
    OutOfEnum::SyncResourceState,
    OutOfEnum::DefinitionDataType,
    OutOfEnum::PricingMode,
];

impl OutOfEnum {
    /// Drive the operation with `value` where the enumeration belongs.
    async fn send(self, emulator: &Emulator, value: &str) -> Resp {
        match self {
            OutOfEnum::TransferSourceType => {
                emulator
                    .list_metadata_transfer_jobs(
                        json!({"sourceType": value, "destinationType": "s3"}),
                    )
                    .await
            }
            OutOfEnum::TransferDestinationType => {
                emulator
                    .list_metadata_transfer_jobs(
                        json!({"sourceType": "s3", "destinationType": value}),
                    )
                    .await
            }
            OutOfEnum::TransferSourceEndType => {
                emulator
                    .create_metadata_transfer_job(json!({
                        "sources": [{"type": value}],
                        "destination": {
                            "type": "s3",
                            "s3Configuration": {"location": S3_DESTINATION},
                        },
                    }))
                    .await
            }
            OutOfEnum::TransferDestinationEndType => {
                emulator
                    .create_metadata_transfer_job(json!({
                        "sources": [{
                            "type": "s3",
                            "s3Configuration": {"location": S3_SOURCE},
                        }],
                        "destination": {"type": value},
                    }))
                    .await
            }
            OutOfEnum::TransferJobState => {
                emulator
                    .list_metadata_transfer_jobs(json!({
                        "sourceType": "s3",
                        "destinationType": "s3",
                        "filters": [{"state": value}],
                    }))
                    .await
            }
            OutOfEnum::SyncResourceType => {
                emulator
                    .list_sync_resources(
                        WORKSPACE,
                        SYNC_SOURCE_SITEWISE,
                        json!({"filters": [{"resourceType": value}]}),
                    )
                    .await
            }
            OutOfEnum::SyncResourceState => {
                emulator
                    .list_sync_resources(
                        WORKSPACE,
                        SYNC_SOURCE_SITEWISE,
                        json!({"filters": [{"state": value}]}),
                    )
                    .await
            }
            // The definition also declares a `defaultValue` of another type than
            // the `dataType` beside it, so a writer that got past the value list
            // would word a reason naming the `type` the request carried.
            OutOfEnum::DefinitionDataType => {
                emulator
                    .create_component_type(
                        WORKSPACE,
                        "com.example.outofenum",
                        json!({
                            "propertyDefinitions": {
                                "Reading": {
                                    "dataType": {"type": value},
                                    "defaultValue": {"stringValue": "of another type"},
                                },
                            },
                        }),
                    )
                    .await
            }
            OutOfEnum::PricingMode => {
                emulator.update_pricing_plan(json!({"pricingMode": value})).await
            }
        }
    }
}

/// RULE: a value outside a modelled enumeration's value list is refused with
/// HTTP 400 `ValidationException` and selects nothing, however long the value is
/// — the value list is a constraint of the model, so the request is answered at
/// the boundary and the handler's own check of the same list is never what a
/// caller sees.
///
/// This is why the reasons those handlers build from an end type, a filter's
/// `resourceType` or a `pricingMode` are the crate's defence rather than a
/// caller-visible message: the rows below name values of every size, including
/// ones far past what the `ErrorMessage` member admits, and none of them is
/// answered by the handler.
#[tokio::test]
async fn an_out_of_enum_value_is_refused_at_the_boundary() {
    let emulator = Emulator::new();
    assert_eq!(
        emulator.create_workspace(WORKSPACE).await.status,
        200,
        "the workspace the sync job is seeded in must exist"
    );
    // `ListSyncResources` answers for a job the workspace holds, so the job is
    // seeded before the rows that list it.
    seed_sync_job(&emulator, WORKSPACE, SYNC_SOURCE_SITEWISE);

    // The rows differ along the SIZE of the value: one a reason could state
    // whole, one past what the `ErrorMessage` member admits, and one far past
    // it — the model bounds such a value nowhere.
    for member in OUT_OF_ENUM {
        for length in [SHORT, ERROR_MESSAGE_MAX + 1, 2 * ERROR_MESSAGE_MAX] {
            let case = format!("{member:?} with a value of {length} characters");
            let value = "v".repeat(length);
            let refused = member.send(&emulator, &value).await;
            assert_eq!(
                (refused.status, refused.error_type.as_deref()),
                (400, Some("ValidationException")),
                "case {case:?}: expected 400 ValidationException, got {refused:?}"
            );
        }
    }

    // Nothing the refused calls asked for happened, read back through the
    // operations that would report it: no job was created by the refused
    // creates, and the account is still on the plan it started on.
    let listed = emulator
        .list_metadata_transfer_jobs(json!({"sourceType": "s3", "destinationType": "s3"}))
        .await;
    assert_eq!(
        (
            listed.status,
            listed
                .member("metadataTransferJobSummaries")
                .as_array()
                .map(Vec::len)
        ),
        (200, Some(0)),
        "a refused create stores no job, got {listed:?}"
    );
    let plan = emulator.get_pricing_plan().await;
    assert_eq!(
        (
            plan.status,
            plan.member("currentPricingPlan")["pricingMode"].as_str()
        ),
        (200, Some(PRICING_MODE_BASIC)),
        "a refused plan update leaves the account on the plan it was on, got {plan:?}"
    );
    let component_type = emulator
        .get_component_type(WORKSPACE, "com.example.outofenum")
        .await;
    assert_eq!(
        component_type.status, 404,
        "a refused create stores no component type, got {component_type:?}"
    );
}

/// One tagging operation, which addresses a resource by an ARN the model bounds
/// as widely as the member its refusal travels on.
#[derive(Debug, Clone, Copy)]
enum Tagging {
    /// `TagResource`.
    Write,
    /// `UntagResource`.
    Remove,
    /// `ListTagsForResource`.
    Read,
}

/// The whole set of operations that addresses a resource by `resourceARN`.
const TAGGING: [Tagging; 3] = [Tagging::Write, Tagging::Remove, Tagging::Read];

impl Tagging {
    /// Drive this operation against `resource_arn`.
    async fn send(self, emulator: &Emulator, resource_arn: &str) -> Resp {
        match self {
            Tagging::Write => {
                emulator
                    .tag_resource(resource_arn, json!({"team": "whoever"}))
                    .await
            }
            Tagging::Remove => emulator.untag_resource(resource_arn, &["team"]).await,
            Tagging::Read => {
                emulator.list_tags_for_resource(resource_arn, json!({})).await
            }
        }
    }
}

/// RULE: every operation that addresses a resource by ARN states the miss within
/// the 2048 characters the member the reason travels on admits — the modelled
/// `TwinMakerArn` is bounded at those same 2048 characters, so any fixed text
/// joining it overruns the member.
#[tokio::test]
async fn a_tagging_refusal_fits_its_modelled_member() {
    let emulator = Emulator::new();

    let fixed = resource_not_found("").chars().count();
    for operation in TAGGING {
        for (size_case, length) in sizes(
            fixed,
            unheld_workspace_arn_base().chars().count(),
            TWIN_MAKER_ARN_MAX,
        ) {
            let case = format!("{operation:?} with {size_case}");
            let resource_arn = unheld_workspace_arn(length);
            let refused = operation.send(&emulator, &resource_arn).await;
            assert_eq!(
                (refused.status, refused.error_type.as_deref()),
                (404, Some("ResourceNotFoundException")),
                "case {case:?}: expected 404 ResourceNotFoundException, got {refused:?}"
            );
            let reported = refused
                .member("message")
                .as_str()
                .unwrap_or_default()
                .to_string();
            assert_within_bound(
                &case,
                &reported,
                &resource_not_found(&resource_arn),
                ERROR_MESSAGE_MAX,
            );
        }
    }
}

/// One operation that takes a `tags` map on a CREATE, and so refuses a key in
/// the reserved namespace before anything is stored — the same reason the
/// tagging calls report, built by each create's own refusal builder.
#[derive(Debug, Clone, Copy)]
enum TaggedCreate {
    Workspace,
    Scene,
    ComponentType,
}

/// The creates the rule is asserted over.
const TAGGED_CREATES: [TaggedCreate; 3] = [
    TaggedCreate::Workspace,
    TaggedCreate::Scene,
    TaggedCreate::ComponentType,
];

impl TaggedCreate {
    /// Drive this create with `tags`, every other member being one the service
    /// accepts. The reserved-key rule is decided on the request alone, before
    /// the store is reached, so no row depends on what the store holds.
    async fn send(self, emulator: &Emulator, tags: serde_json::Value) -> Resp {
        match self {
            TaggedCreate::Workspace => {
                emulator
                    .create_workspace_with("msgboundstagged", json!({"tags": tags}))
                    .await
            }
            TaggedCreate::Scene => {
                emulator
                    .create_scene(
                        WORKSPACE,
                        json!({
                            "sceneId": "taggedScene",
                            "contentLocation": "s3://bucket/tagged.json",
                            "tags": tags,
                        }),
                    )
                    .await
            }
            TaggedCreate::ComponentType => {
                emulator
                    .create_component_type(WORKSPACE, "com.example.tagged", json!({"tags": tags}))
                    .await
            }
        }
    }
}

/// RULE: a refusal that names the reserved tag keys a request carried states
/// them within the 2048 characters the member it travels on admits, however many
/// keys the request named — the keys are bounded one at a time, not as the set a
/// reason renders.
#[tokio::test]
async fn a_reserved_tag_key_refusal_fits_its_modelled_member() {
    let emulator = Emulator::new();

    // The rows differ along HOW MANY keys the reason names — one, a set whose
    // reason is already past what the member admits, and the largest set the
    // model admits — and along which operation reports it: the two tagging
    // calls, and the creates that take a `tags` map of their own.
    for count in [1, 20, TAG_KEYS_MAX] {
        let keys = reserved_keys(count);
        let tags: serde_json::Map<String, serde_json::Value> = keys
            .iter()
            .map(|key| (key.clone(), json!("whoever")))
            .collect();

        let case = format!("TagResource with {count} reserved tag keys");
        let refused = emulator
            .tag_resource(
                &workspace_arn(WORKSPACE),
                serde_json::Value::Object(tags.clone()),
            )
            .await;
        assert_eq!(
            (refused.status, refused.error_type.as_deref()),
            (400, Some("ValidationException")),
            "case {case:?}: expected 400 ValidationException, got {refused:?}"
        );
        let reported = refused
            .member("message")
            .as_str()
            .unwrap_or_default()
            .to_string();
        assert_within_bound(
            &case,
            &reported,
            &reserved_keys_reason("tags", &keys),
            ERROR_MESSAGE_MAX,
        );

        let case = format!("UntagResource with {count} reserved tag keys");
        let borrowed: Vec<&str> = keys.iter().map(String::as_str).collect();
        let refused = emulator
            .untag_resource(&workspace_arn(WORKSPACE), &borrowed)
            .await;
        assert_eq!(
            (refused.status, refused.error_type.as_deref()),
            (400, Some("ValidationException")),
            "case {case:?}: expected 400 ValidationException, got {refused:?}"
        );
        let reported = refused
            .member("message")
            .as_str()
            .unwrap_or_default()
            .to_string();
        assert_within_bound(
            &case,
            &reported,
            &reserved_keys_reason("tagKeys", &keys),
            ERROR_MESSAGE_MAX,
        );

        for create in TAGGED_CREATES {
            let case = format!("{create:?} with {count} reserved tag keys");
            let refused = create
                .send(&emulator, serde_json::Value::Object(tags.clone()))
                .await;
            assert_eq!(
                (refused.status, refused.error_type.as_deref()),
                (400, Some("ValidationException")),
                "case {case:?}: expected 400 ValidationException, got {refused:?}"
            );
            let reported = refused
                .member("message")
                .as_str()
                .unwrap_or_default()
                .to_string();
            assert_within_bound(
                &case,
                &reported,
                &reserved_keys_reason("tags", &keys),
                ERROR_MESSAGE_MAX,
            );
        }
    }
}

/// RULE: `CreateSyncJob` names the `syncSource` it does not support within the
/// 2048 characters the member the reason travels on admits — the modelled
/// `SyncSource` is a pattern string the model gives no `@length`, so the service
/// is what brings the reason inside the bound.
#[tokio::test]
async fn a_sync_job_create_refusal_fits_its_modelled_member() {
    // The create resolves its `syncRole` against the IAM records, so IAM is
    // mounted beside this service; the role is one those records do not hold,
    // which this service accepts, so what the rows exercise is the source label.
    let emulator = Emulator::with_iam();
    assert_eq!(
        emulator.create_workspace(WORKSPACE).await.status,
        200,
        "the workspace the sync job would be created in must exist"
    );

    let whole = |source: &str| {
        format!(
            "syncSource {source} is not supported; the only supported syncSource is \
             {SYNC_SOURCE_SITEWISE}"
        )
    };
    let fixed = whole("").chars().count();
    // The label travels in the request's PATH, so the widest row is the one that
    // lands one character past the member's maximum rather than a size far
    // beyond it.
    for (size_case, length) in sizes(fixed, SHORT, ERROR_MESSAGE_MAX - fixed + 1) {
        let case = format!("CreateSyncJob with {size_case}");
        let sync_source = "S".repeat(length);
        let refused = emulator
            .create_sync_job(WORKSPACE, &sync_source, json!({"syncRole": SYNC_ROLE}))
            .await;
        assert_eq!(
            (refused.status, refused.error_type.as_deref()),
            (400, Some("ValidationException")),
            "case {case:?}: expected 400 ValidationException, got {refused:?}"
        );
        let reported = refused
            .member("message")
            .as_str()
            .unwrap_or_default()
            .to_string();
        assert_within_bound(&case, &reported, &whole(&sync_source), ERROR_MESSAGE_MAX);
    }
}
